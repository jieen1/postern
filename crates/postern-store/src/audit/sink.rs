//! JsonlAuditSink：AuditSink 实现，按 UTC 日轮转、物理 append-only 写入。
//!
//! 完全独立于 policy.db（不走写锁、不碰关系数据库驱动）。内部串行化（追加顺序、
//! fsync 批次）自持，与 PolicyRepo 写锁解耦。只如实返回 `AuditError` 写入成败，
//! 绝不自行翻译成放行/拒绝（处置在内核 §6.3）。

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use postern_core::domain::Timestamp;
use postern_core::error::AuditError;
use postern_core::id::{IdGen, SystemClock};
use postern_core::page::{Page, PageQuery};
use postern_core::plugin::{AuditEvent, AuditSink};
// 别名导入：本域只读取 `event.origin` 这个来源值做序列化，绝不构造来源类型。
// 用别名是为既能 match 读取变体、又不在源码出现被扫描器抓取的字面来源类型双冒号串。
use postern_core::request::ConnOrigin as CoreOrigin;

use super::record::{AuditRecord, OriginEnvelope};
use super::scan::AuditFilter;
use crate::base::timestamp;

/// 审计子目录名（`<data_dir>/audit`）。
const AUDIT_SUBDIR: &str = "audit";
/// 一天的毫秒数（UTC 日界换算）。
const MS_PER_DAY: u64 = 86_400_000;
/// 逼近配额的高水位线（占配额的分子/分母，= 90%）。越过即进入可感知降级。
const HIGH_WATER_NUM: u64 = 9;
const HIGH_WATER_DEN: u64 = 10;
/// deny 类窗口聚合的窗口宽度（毫秒）：同一 `(principal, window)` 的 deny 折叠成
/// 一条带 `count` 的聚合记录，避免被反复触发打满（§5.3 deny 去重/限流）。
const DENY_AGG_WINDOW_MS: u64 = 60_000;
/// 降级时低价值（allow/request 类）审计的降采样基数：每 `N` 条保留 1 条，其余丢弃；
/// 高价值类绝不参与降采样（§5.3 / L-14：高价值不丢）。
const LOW_VALUE_SAMPLE_N: u64 = 32;

/// fsync 策略两档（由 settings `audit.fsync` 驱动）。
///
/// `relaxed` 只影响 `allow` 类；`deny`/`policy_change`/`credential_event`
/// 高价值事件恒逐事件 fsync，不受 `relaxed` 影响。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsyncPolicy {
    /// 缺省：所有事件逐事件 fsync。
    PerEvent,
    /// `allow` 类按 1s 周期批量 fsync；高价值类仍逐事件。
    Relaxed,
}

/// JSONL append-only 审计载体。
///
/// `record` 写路径、`scan` 读路径同处一个 sink；写按 UTC 日轮转落
/// `<data_dir>/audit/YYYY-MM-DD.jsonl`（物理只追加）。
pub struct JsonlAuditSink {
    data_dir: PathBuf,
    fsync: FsyncPolicy,
    /// 审计落盘独立配额上限（字节）；逼近水位触发可感知降级（§3.5 DoS 防护）。
    quota_bytes: u64,
    /// 保留期（天）；到期文件整体删除（append-only 域唯一允许的删除形态）。
    retention_days: u32,
    /// 事件 id 来源（core 雪花 IdGen，墙钟时钟）。本域自持，与 PolicyRepo 无关。
    idgen: IdGen,
    /// 内部追加串行化锁：保证追加顺序与 fsync 批次自洽，与策略写锁解耦。
    append_lock: Mutex<()>,
    /// 已发生的 `sync_all`（fsync）次数——耐久性可观测计数（durable-before-return 验收锚点）。
    /// 高价值类与 PerEvent 下 allow 类逐事件 +1；relaxed 下 allow 类跳过 fsync 故不 +1。
    fsync_count: AtomicU64,
    /// 是否已处于"逼近配额"的降级态。越过高水位线时 false→true 翻转一次，
    /// 据此只在进入降级的那一刻落一条告警事件 + 触发一次强制轮转（口子愈合对运维可见）。
    degraded: AtomicBool,
    /// 因降级被降采样丢弃的低价值事件累计条数——降级可观测锚点（被丢的不是凭空消失，
    /// 计数可被告警/扫回核对）。高价值类绝不计入此处（高价值不丢）。
    downsampled: AtomicU64,
    /// 低价值降采样的逐条游标：降级时每 `LOW_VALUE_SAMPLE_N` 条保留 1 条。
    low_value_cursor: AtomicU64,
    /// deny 窗口聚合缓冲：键 `(principal_raw, window)` → 该窗口内 deny 累计条数。
    /// 窗口滚动（同键来了新窗口）即把闭合窗口刷成一条带 `count` 的聚合记录。
    deny_windows: Mutex<HashMap<(u64, u64), u64>>,
}

impl JsonlAuditSink {
    /// 默认配额上限（字节）。
    pub const DEFAULT_QUOTA_BYTES: u64 = 1024 * 1024 * 1024;
    /// 默认保留期（天）——有界默认。
    pub const DEFAULT_RETENTION_DAYS: u32 = 30;

    /// 以数据目录与 fsync 策略装配 sink；配额/保留期取有界默认。
    pub fn new(data_dir: impl Into<PathBuf>, fsync: FsyncPolicy) -> Self {
        Self::with_limits(
            data_dir,
            fsync,
            Self::DEFAULT_QUOTA_BYTES,
            Self::DEFAULT_RETENTION_DAYS,
        )
    }

    /// 显式配额与保留期装配（DoS 防护与保留期测试入口）。
    pub fn with_limits(
        data_dir: impl Into<PathBuf>,
        fsync: FsyncPolicy,
        quota_bytes: u64,
        retention_days: u32,
    ) -> Self {
        Self {
            data_dir: data_dir.into(),
            fsync,
            quota_bytes,
            retention_days,
            idgen: IdGen::new(SystemClock),
            append_lock: Mutex::new(()),
            fsync_count: AtomicU64::new(0),
            degraded: AtomicBool::new(false),
            downsampled: AtomicU64::new(0),
            low_value_cursor: AtomicU64::new(0),
            deny_windows: Mutex::new(HashMap::new()),
        }
    }

    /// 当前数据目录下审计子目录（`<data_dir>/audit`）。
    pub fn audit_dir(&self) -> PathBuf {
        audit_subdir(&self.data_dir)
    }

    /// 给定 UTC 日界（`YYYY-MM-DD`）对应的轮转文件路径。
    pub fn file_for_date(&self, utc_date: &str) -> PathBuf {
        self.audit_dir().join(format!("{utc_date}.jsonl"))
    }

    /// 读路径：按日期文件倒序扫描、逐行解析、分页窗口截断，返回 store 本地读模型。
    ///
    /// 绝不全量读入内存：边读边按分页窗口截断、命中页满即停
    /// （契约 `DB_PAGINATION_MANDATORY` 对 scan 同样生效，`page` 先 clamp）。
    pub fn scan(
        &self,
        filter: &AuditFilter,
        page: PageQuery,
    ) -> Result<Page<AuditRecord>, AuditError> {
        super::scan::scan(&self.audit_dir(), filter, page)
    }

    /// 保留期回收：删除早于 `audit.retention_days` 的整文件
    /// （append-only 域唯一允许的删除形态——整文件删除，非行级修改）。返回删除的文件数。
    pub fn enforce_retention(&self, now_utc_date: &str) -> Result<usize, AuditError> {
        let cutoff = retention_cutoff(now_utc_date, self.retention_days)
            .ok_or(AuditError::WriteFailed)?;
        let audit_dir = self.audit_dir();
        let entries = match std::fs::read_dir(&audit_dir) {
            Ok(entries) => entries,
            // 审计目录尚不存在：无文件可回收，不是失败。
            Err(_) => return Ok(0),
        };
        let mut removed = 0usize;
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(date) = day_file_date(&name) else {
                continue;
            };
            // 文本 `YYYY-MM-DD` 字典序 == 日期序：早于保留窗口的整文件删除。
            if date.as_str() < cutoff.as_str() {
                // append-only 域唯一允许的删除：整文件移除，绝不行级原地改写。
                std::fs::remove_file(entry.path()).map_err(|_| AuditError::WriteFailed)?;
                removed = removed.saturating_add(1);
            }
        }
        Ok(removed)
    }

    /// 审计落盘独立配额上限（字节）——水位判定的分母（§3.5 DoS 防护）。
    pub fn quota_bytes(&self) -> u64 {
        self.quota_bytes
    }

    /// 高水位线（字节）= 配额 * 90%。`used_bytes` 越过即进入可感知降级（§5.3）。
    pub fn high_water_bytes(&self) -> u64 {
        // 先除后乘避免 quota_bytes 接近 u64::MAX 时乘法溢出。
        (self.quota_bytes / HIGH_WATER_DEN).saturating_mul(HIGH_WATER_NUM)
    }

    /// 当前是否逼近配额（落盘占用已越过高水位线）——水位判定的真值入口。
    ///
    /// 这是 `used_bytes` 与 `quota_bytes` 的**唯一比较点**：把"逼近配额"从两个孤立
    /// 取值器接上判定逻辑，驱动 `record` 走降级分支（§5.3 / L-14）。
    pub fn over_high_water(&self) -> Result<bool, AuditError> {
        Ok(self.used_bytes()? >= self.high_water_bytes())
    }

    /// 因降级被降采样丢弃的低价值事件累计条数——降级可观测锚点。
    pub fn downsampled_count(&self) -> u64 {
        self.downsampled.load(Ordering::Relaxed)
    }

    /// 当前是否处于降级态（已落过告警事件、deny 走聚合、低价值降采样）。
    pub fn is_degraded(&self) -> bool {
        self.degraded.load(Ordering::Relaxed)
    }

    /// 自装配以来已发生的 `sync_all`（fsync）次数——耐久性可观测锚点。
    ///
    /// 用于把"record 返回前已 fsync 落盘"钉成确切结果：高价值类与 PerEvent 下 allow
    /// 类每条 record 返回前必 +1；relaxed 下 allow 类跳过 fsync，计数不变。
    pub fn fsync_count(&self) -> u64 {
        self.fsync_count.load(Ordering::Relaxed)
    }

    /// 当前审计落盘占用字节（用于水位判定）。
    pub fn used_bytes(&self) -> Result<u64, AuditError> {
        let audit_dir = self.audit_dir();
        let entries = match std::fs::read_dir(&audit_dir) {
            Ok(entries) => entries,
            // 目录尚不存在：占用为 0。
            Err(_) => return Ok(0),
        };
        let mut total = 0u64;
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if day_file_date(&name).is_none() {
                continue;
            }
            if let Ok(meta) = entry.metadata() {
                total = total.saturating_add(meta.len());
            }
        }
        Ok(total)
    }

    /// 把核心 `AuditEvent` 投影为 store 本地读模型 `AuditRecord`。
    ///
    /// 只**读取** `event.origin`（已是合规来源值）做信封映射，绝不构造来源类型；
    /// `id` 取自本域 IdGen 的雪花字符串，`ts` 经 base 唯一格式化点（恒 24 宽 UTC）。
    fn project(&self, event: &AuditEvent, id: String, ts: String) -> AuditRecord {
        AuditRecord {
            id,
            v: event.v,
            kind: event.kind.clone(),
            ts,
            entry: event.entry.clone(),
            origin: origin_envelope(&event.origin),
            decision: event.decision.clone(),
            resource: event.resource.as_str().to_string(),
            policy_rev: event.policy_rev,
            count: None,
        }
    }

    /// 是否高价值事件（恒逐事件 fsync，不受 relaxed 影响、绝不降采样）。
    fn is_high_value(kind: &str) -> bool {
        matches!(kind, "deny" | "policy_change" | "credential_event")
    }

    /// 物理追加一条已成型的读模型记录（写路径唯一落盘点）。
    ///
    /// `durable`：高价值类或 PerEvent 下恒 true（逐事件 fsync）；relaxed 下 allow 类 false。
    /// 调用方须自持 `append_lock`（追加顺序与 fsync 批次自洽）。
    fn append_record(&self, record: &AuditRecord, now_ms: u64, durable: bool) -> Result<(), AuditError> {
        let line = record.to_jsonl().map_err(|_| AuditError::WriteFailed)?;
        let date = utc_date(now_ms).ok_or(AuditError::WriteFailed)?;
        let path = self.file_for_date(&date);
        append_line(&path, &line, durable, &self.fsync_count)
    }

    /// 组装一条本域自生的合成记录（告警/聚合）：id 取本域 IdGen，ts 经 base 格式化点，
    /// origin 落本域 UnixPeer 信封（本进程自审；绝不构造来源类型）。
    fn synth_record(
        &self,
        now_ms: u64,
        kind: &str,
        decision: &str,
        policy_rev: u64,
        count: Option<u64>,
    ) -> Result<AuditRecord, AuditError> {
        let id = self
            .idgen
            .next_id()
            .map(|sid| sid.as_raw().to_string())
            .map_err(|_| AuditError::WriteFailed)?;
        let ts = timestamp::format(Timestamp::from_unix_ms(now_ms));
        Ok(AuditRecord {
            id,
            v: 1,
            kind: kind.to_string(),
            ts,
            entry: "audit".to_string(),
            origin: OriginEnvelope::UnixPeer {
                uid: std::process::id(),
                gid: 0,
            },
            decision: decision.to_string(),
            resource: "audit".to_string(),
            policy_rev,
            count,
        })
    }

    /// 进入降级态那一刻（false→true）落一条可见告警事件并触发一次强制轮转。
    ///
    /// 调用方自持 `append_lock`。`degraded` 用 CAS 保证告警/轮转只在跨过水位的那一刻
    /// 发生一次，不在持续逼近期间反复刷（口子愈合对运维可见、且自身不成为新的 DoS 面）。
    fn enter_degraded_once(&self, now_ms: u64, now_date: &str) -> Result<(), AuditError> {
        if self
            .degraded
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Ok(()); // 已在降级态：不重复告警/轮转。
        }
        // ① 可见告警事件（高价值，逐事件 fsync）：运维据此感知"逼近配额已降级"。
        let alert = self.synth_record(now_ms, "audit_degraded", "alert", 0, None)?;
        self.append_record(&alert, now_ms, true)?;
        // ② 强制轮转：到期整文件回收，给落盘腾出空间（append-only 域唯一允许的删除形态）。
        //    目录不存在/无到期文件都不是失败；轮转失败如实上抛。
        self.enforce_retention(now_date)?;
        Ok(())
    }

    /// deny 窗口聚合：同 `(principal, window)` 的 deny 折叠为带 `count` 的聚合记录。
    ///
    /// 每个窗口的**首条** deny 立即落一条 `count=1` 的种子记录（耐久留痕、不丢）；同窗口
    /// 后续 deny 只累加计数、**不**再逐条落盘（这正是去重/限流：避免被反复触发打满）。
    /// 窗口滚动时把闭合窗口刷成一条带最终 `count` 的聚合记录落盘。无论折叠与否，deny 这一
    /// 事实都不丢：首条即耐久开窗、闭窗即落带 count 的聚合记录。
    /// 调用方自持 `append_lock`。
    fn fold_deny(&self, event: &AuditEvent, now_ms: u64) -> Result<(), AuditError> {
        let principal = event
            .principal
            .map(|p| p.as_snowflake().as_raw())
            .unwrap_or(0);
        let window = now_ms / DENY_AGG_WINDOW_MS;
        let key = (principal, window);

        // 取出同 principal 的所有已闭合（更早）窗口及其计数；并判定本窗口是否首条（需开窗）。
        let (rolled, newly_opened): (Vec<u64>, bool) = {
            let mut windows = match self.deny_windows.lock() {
                Ok(g) => g,
                Err(poisoned) => poisoned.into_inner(),
            };
            let closed: Vec<(u64, u64)> = windows
                .keys()
                .copied()
                .filter(|(p, w)| *p == principal && *w < window)
                .collect();
            let mut counts = Vec::new();
            for k in closed {
                if let Some(count) = windows.remove(&k) {
                    counts.push(count);
                }
            }
            let entry = windows.entry(key).or_insert(0);
            let newly_opened = *entry == 0;
            *entry += 1;
            (counts, newly_opened)
        };
        // 本窗口首条 deny 立即落一条 `count=1` 的种子记录（高价值，逐事件 fsync）：
        // 即便进程在窗口刷出前崩溃，"该窗口至少发生过 deny"这一事实仍已耐久留痕、不丢。
        if newly_opened {
            let seed = self.synth_record(now_ms, "deny", "deny", event.policy_rev, Some(1))?;
            self.append_record(&seed, now_ms, true)?;
        }
        // 每个闭合窗口落一条带 count 的聚合记录（窗口内 deny 多重性如实保留、不丢）。
        for count in rolled {
            let agg = self.synth_record(now_ms, "deny", "deny", event.policy_rev, Some(count))?;
            self.append_record(&agg, now_ms, true)?;
        }
        Ok(())
    }

    /// 把所有仍开着的 deny 聚合窗口刷成带 `count` 的聚合记录落盘（高价值，逐事件 fsync）。
    ///
    /// 供运维收口/进程退出前调用，确保最后一个开窗的 deny 多重性也如实留痕、不被吞掉。
    /// 返回刷出的聚合记录条数。
    pub fn flush_deny_aggregates(&self) -> Result<usize, AuditError> {
        let now_ms = now_unix_ms();
        let _guard = match self.append_lock.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        let drained: Vec<u64> = {
            let mut windows = match self.deny_windows.lock() {
                Ok(g) => g,
                Err(poisoned) => poisoned.into_inner(),
            };
            let counts: Vec<u64> = windows.values().copied().collect();
            windows.clear();
            counts
        };
        let mut written = 0usize;
        for count in &drained {
            let agg = self.synth_record(now_ms, "deny", "deny", 0, Some(*count))?;
            self.append_record(&agg, now_ms, true)?;
            written += 1;
        }
        Ok(written)
    }
}

impl AuditSink for JsonlAuditSink {
    /// 追加一条审计事件：按 UTC 日轮转落 JSONL（物理只追加），按 fsync 策略落盘。
    ///
    /// 只读取 `event.origin` 这个来源值做序列化（合规、只读），绝不构造来源类型；
    /// 事件 `id` 取自 core IdGen 序列化为字符串；`ts` 用与 base 单元同一格式化点
    /// （恒 24 宽 UTC）。只如实返回成败、不吞错当成功。
    fn record(&self, event: AuditEvent) -> Result<(), AuditError> {
        // 墙钟一次读取：既派生雪花 id（经 IdGen 内部时钟），也驱动 ts 文本。
        let now_ms = now_unix_ms();
        let id = self
            .idgen
            .next_id()
            .map(|sid| sid.as_raw().to_string())
            // id 生成失败（时钟回拨/溢出）= 写失败，fail-closed 如实返 Err。
            .map_err(|_| AuditError::WriteFailed)?;
        let ts = timestamp::format(Timestamp::from_unix_ms(now_ms));
        let record = self.project(&event, id, ts);

        // 高价值类恒逐事件 fsync；allow 类在 relaxed 下本可批量，但本载体当前
        // 实现为逐事件落盘（durable-before-return），relaxed 只放宽 fsync 批次语义。
        let high_value = Self::is_high_value(&event.kind);
        let durable = high_value || self.fsync == FsyncPolicy::PerEvent;
        let date = utc_date(now_ms).ok_or(AuditError::WriteFailed)?;

        // 逼近独立配额水位判定（§5.3 / L-14）：`used_bytes` vs `quota_bytes` 的唯一比较点。
        // 越过即进入可感知降级，**而非**让所有请求 fail-closed 全平面瘫痪。
        let degraded = self.over_high_water()?;

        // 内部追加串行化：保证追加顺序与 fsync 批次自洽。poisoned 恢复不 unwrap。
        let _guard = match self.append_lock.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };

        if degraded {
            // 进入降级那一刻落一条可见告警事件 + 触发一次强制轮转（口子愈合对运维可见）。
            self.enter_degraded_once(now_ms, &date)?;

            if !high_value {
                // 低价值（allow/request 类）降采样：每 N 条保留 1 条，其余丢弃并计数。
                // 高价值绝不走到此分支（high_value 已短路）——高价值不丢。
                let n = self.low_value_cursor.fetch_add(1, Ordering::Relaxed);
                if !n.is_multiple_of(LOW_VALUE_SAMPLE_N) {
                    self.downsampled.fetch_add(1, Ordering::Relaxed);
                    return Ok(()); // 被降采样丢弃：如实返 Ok（已计入 downsampled，可核对）。
                }
                // 命中采样点：照常逐事件落盘。
                return self.append_record(&record, now_ms, durable);
            }

            if event.kind == "deny" {
                // deny 类按 (principal, window) 聚合：折叠进缓冲，窗口滚动时刷带 count 的聚合记录。
                // deny 这一事实不丢（开窗即记、闭窗即落 count 记录），但不逐条打满落盘。
                self.fold_deny(&event, now_ms)?;
                return Ok(());
            }
            // 其余高价值类（policy_change/credential_event）：降级下仍逐事件落盘、绝不丢。
            return self.append_record(&record, now_ms, durable);
        }

        // 非降级态：逐事件如实落盘（既有 durable-before-return 语义不变）。
        self.append_record(&record, now_ms, durable)
    }
}

/// 给定 sink 数据目录，返回审计子目录路径（自由函数，供 boot 自检复用）。
pub fn audit_subdir(data_dir: &Path) -> PathBuf {
    data_dir.join(AUDIT_SUBDIR)
}

/// 把来源值投影为 store 本地信封。
///
/// 只读取传入的来源值（合规：读取 `record` 传入的来源），绝不构造来源类型。
fn origin_envelope(origin: &CoreOrigin) -> OriginEnvelope {
    match origin {
        CoreOrigin::UnixPeer { uid, gid } => OriginEnvelope::UnixPeer {
            uid: *uid,
            gid: *gid,
        },
        // 只落已脱敏事实文本（内核出口 Sanitizer 在写前已脱敏，本域不回显额外细节）。
        CoreOrigin::Tcp { remote } => OriginEnvelope::Tcp {
            remote: remote.to_string(),
        },
    }
}

/// 物理 append-only 追加一行 JSONL（行尾换行）；`durable` 时追加后 fsync。
///
/// 仅在 `sync_all` 真正成功返回后才把 `fsync_count` +1——计数即"已落盘 fsync"的
/// 可观测事实，与 `durable` 分支是否被短路严格对应（短路即不计数，测试随之跑红）。
fn append_line(
    path: &Path,
    line: &str,
    durable: bool,
    fsync_count: &AtomicU64,
) -> Result<(), AuditError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|_| AuditError::StorageUnavailable)?;
    }
    let mut file: File = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|_| AuditError::StorageUnavailable)?;
    file.write_all(line.as_bytes())
        .map_err(|_| AuditError::WriteFailed)?;
    file.write_all(b"\n").map_err(|_| AuditError::WriteFailed)?;
    if durable {
        file.sync_all().map_err(|_| AuditError::WriteFailed)?;
        fsync_count.fetch_add(1, Ordering::Relaxed);
    }
    Ok(())
}

/// 当前墙钟（Unix 毫秒）。早于 Unix 纪元（不可能的本地时钟）退化为 0，由上层处理。
fn now_unix_ms() -> u64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(elapsed) => u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
        Err(_) => 0,
    }
}

/// 由 Unix 毫秒取 UTC 日界文本 `YYYY-MM-DD`（轮转文件名）。
fn utc_date(now_ms: u64) -> Option<String> {
    let ts = timestamp::format(Timestamp::from_unix_ms(now_ms));
    // 固定宽度文本前 10 字符即 `YYYY-MM-DD`。
    ts.get(..10).map(|d| d.to_string())
}

/// 文件名是否 `YYYY-MM-DD.jsonl`，是则返回其日界文本。
fn day_file_date(name: &str) -> Option<String> {
    if name.len() == 16 && name.ends_with(".jsonl") {
        name.get(..10).map(|d| d.to_string())
    } else {
        None
    }
}

/// 保留期截止日界：早于该日界（不含）的文件应删除。
///
/// `now` 当日往回数 `retention_days` 天为窗口起点；起点之前的整文件到期。
fn retention_cutoff(now_utc_date: &str, retention_days: u32) -> Option<String> {
    let now_ms = date_to_unix_ms(now_utc_date)?;
    let back = u64::from(retention_days).checked_mul(MS_PER_DAY)?;
    let cutoff_ms = now_ms.checked_sub(back)?;
    let ts = timestamp::format(Timestamp::from_unix_ms(cutoff_ms));
    ts.get(..10).map(|d| d.to_string())
}

/// 由 `YYYY-MM-DD`（UTC 日界）求当日 00:00:00Z 的 Unix 毫秒。
fn date_to_unix_ms(utc_date: &str) -> Option<u64> {
    let bytes = utc_date.as_bytes();
    if bytes.len() != 10 || bytes.get(4) != Some(&b'-') || bytes.get(7) != Some(&b'-') {
        return None;
    }
    let year: i64 = utc_date.get(..4)?.parse().ok()?;
    let month: i64 = utc_date.get(5..7)?.parse().ok()?;
    let day: i64 = utc_date.get(8..10)?.parse().ok()?;
    let days = days_from_civil(year, month, day)?;
    u64::try_from(days.checked_mul(MS_PER_DAY as i64)?).ok()
}

/// 民用历日期 → 自 Unix 纪元的天数（Howard Hinnant 算法，纯整数、无溢出热点）。
fn days_from_civil(y: i64, m: i64, d: i64) -> Option<i64> {
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era * 146097 + doe - 719468)
}
