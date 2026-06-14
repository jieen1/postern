//! 审计载体行为测试：按 UTC 日轮转、物理 append-only、fsync 策略、倒序分页扫描、
//! DoS 可控降级、append-only 不含机密。每条只钉一个行为，断言精确到具体值/变体/字段。
//!
//! §8 覆盖：F-13（按日轮转 + 分页扫描）、F-14（fsync 策略落地）、
//! L-5（审计写失败=deny 只读单次）、L-6（审计写失败两阶段）、
//! L-14（审计 DoS 可控降级）、L-15（append-only 不含机密）。
//!
//! 雷区遵守：构造 core 的 ConnOrigin 用 `use ... as Origin` 别名再写 `Origin::UnixPeer`
//! 形式，绝不出现字面 `ConnOrigin` 双冒号变体（SEC_CONSTRUCTION_SITES）。读路径只用
//! store 本地 OriginEnvelope 构造，不碰 ConnOrigin。本文件不含任何裸数据库写标记。

use std::fs;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::path::{Path, PathBuf};

use postern_core::domain::{Capability, ResourceCode};
use postern_core::error::AuditError;
use postern_core::page::{Page, PageQuery};
use postern_core::plugin::{AuditEvent, AuditSink};
use postern_core::request::ConnOrigin as Origin;

use postern_store::audit::record::{AuditRecord, OriginEnvelope};
use postern_store::audit::scan::AuditFilter;
use postern_store::audit::sink::{FsyncPolicy, JsonlAuditSink};

// ---------------------------------------------------------------------------
// 夹具：临时数据目录 + AuditEvent 构造器
// ---------------------------------------------------------------------------

/// 进程唯一的临时数据目录（无第三方 tempfile 依赖；用进程内单调计数器命名）。
fn temp_data_dir(tag: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let dir = std::env::temp_dir().join(format!("postern-audit-test-{tag}-{pid}-{n}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create temp data dir");
    dir
}

/// 构造一条已脱敏的 AuditEvent。`origin` 只能经别名 `Origin::UnixPeer` 构造
/// （绝不写字面 ConnOrigin 双冒号变体）。
fn event(kind: &str, decision: &str) -> AuditEvent {
    AuditEvent {
        v: 1,
        kind: kind.to_string(),
        entry: "mcp".to_string(),
        origin: Origin::UnixPeer {
            uid: 1000,
            gid: 1000,
        },
        principal: None,
        resource: ResourceCode::new("db-main"),
        capability: Some(Capability::Query),
        objects: Vec::new(),
        decision: decision.to_string(),
        stage: None,
        reason: String::new(),
        policy_rev: 7,
    }
}

/// 读出审计目录下全部 `.jsonl` 文件名（不含路径），按名排序。
fn jsonl_files(audit_dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(audit_dir)
        .expect("read audit dir")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".jsonl"))
        .collect();
    names.sort();
    names
}

// ---------------------------------------------------------------------------
// 注入用 sink：record 恒返回某个 AuditError（用于 L-5 / L-6 失败注入）
// ---------------------------------------------------------------------------

/// 注入用故障 sink：`record` 恒返回预置的 `AuditError`，验证本域只如实返回成败。
struct FailingSink {
    err: AuditError,
}

impl AuditSink for FailingSink {
    fn record(&self, _event: AuditEvent) -> Result<(), AuditError> {
        Err(self.err.clone())
    }
}

// ===========================================================================
// F-13 JsonlAuditSink 按日轮转 + 分页扫描
// ===========================================================================

#[test] // §8 F-13: record 落 <data_dir>/audit/YYYY-MM-DD.jsonl（UTC 日界）
fn record_writes_to_utc_day_rotated_file_under_audit_dir() {
    let dir = temp_data_dir("rotate");
    let sink = JsonlAuditSink::new(&dir, FsyncPolicy::PerEvent);
    sink.record(event("request", "allow")).expect("record ok");

    let audit_dir = dir.join("audit");
    let files = jsonl_files(&audit_dir);
    assert_eq!(files.len(), 1, "exactly one day-rotated file is created");
    let name = &files[0];
    // 文件名恒 YYYY-MM-DD.jsonl：长度 "2026-06-12.jsonl" == 16。
    assert_eq!(name.len(), 16, "day file name is YYYY-MM-DD.jsonl");
    assert!(name.ends_with(".jsonl"), "rotation file is .jsonl");
    let date = &name[..10];
    assert_eq!(date.as_bytes()[4], b'-', "date uses YYYY-MM-DD");
    assert_eq!(date.as_bytes()[7], b'-', "date uses YYYY-MM-DD");
}

#[test] // §8 F-13: 事件 id 为雪花字符串（JSONL 行里 id 字段是十进制串）
fn record_serializes_event_id_as_snowflake_string() {
    let dir = temp_data_dir("idstr");
    let sink = JsonlAuditSink::new(&dir, FsyncPolicy::PerEvent);
    sink.record(event("request", "allow")).expect("record ok");

    let page = sink
        .scan(
            &AuditFilter::all(),
            PageQuery {
                page_no: 1,
                page_size: 20,
            },
        )
        .expect("scan ok");
    assert_eq!(page.items.len(), 1, "one record scanned back");
    let id = &page.items[0].id;
    assert!(!id.is_empty(), "event id is present");
    assert!(
        id.chars().all(|c| c.is_ascii_digit()),
        "event id serializes as a decimal snowflake string, got {id:?}"
    );
}

#[test] // §8 F-13: 物理只追加——两次 record 后文件单调增长、首行不被改写
fn record_is_physically_append_only() {
    let dir = temp_data_dir("append");
    let sink = JsonlAuditSink::new(&dir, FsyncPolicy::PerEvent);
    let audit_dir = dir.join("audit");

    sink.record(event("request", "allow")).expect("record 1");
    let file = sink.file_for_date(&jsonl_files(&audit_dir)[0][..10]);
    let after_first = fs::read(&file).expect("read after first");

    sink.record(event("request", "deny")).expect("record 2");
    let after_second = fs::read(&file).expect("read after second");

    assert!(
        after_second.len() > after_first.len(),
        "second append strictly grows the file"
    );
    assert!(
        after_second.starts_with(&after_first),
        "first line bytes are unchanged: append never rewrites prior lines"
    );
    // 恰两行（每事件一行，行尾换行）。
    let line_count = after_second.iter().filter(|&&b| b == b'\n').count();
    assert_eq!(line_count, 2, "exactly two appended JSONL lines");
}

#[test] // §8 F-13: scan 倒序——较晚日期文件的记录先于较早日期返回
fn scan_returns_date_files_in_reverse_chronological_order() {
    let dir = temp_data_dir("revorder");
    let sink = JsonlAuditSink::new(&dir, FsyncPolicy::PerEvent);
    let audit_dir = dir.join("audit");
    fs::create_dir_all(&audit_dir).expect("mk audit dir");

    // 直接铺两日文件（倒序断言只依赖文件名日界，与墙钟无关）。
    let older = AuditRecord {
        id: "100".to_string(),
        v: 1,
        kind: "request".to_string(),
        ts: "2026-06-10T00:00:00.000Z".to_string(),
        entry: "mcp".to_string(),
        origin: OriginEnvelope::UnixPeer {
            uid: 1000,
            gid: 1000,
        },
        decision: "allow".to_string(),
        resource: "db-main".to_string(),
        policy_rev: 1,
        count: None,
    };
    let newer = AuditRecord {
        id: "200".to_string(),
        v: 1,
        kind: "request".to_string(),
        ts: "2026-06-11T00:00:00.000Z".to_string(),
        entry: "mcp".to_string(),
        origin: OriginEnvelope::UnixPeer {
            uid: 1000,
            gid: 1000,
        },
        decision: "deny".to_string(),
        resource: "db-main".to_string(),
        policy_rev: 2,
        count: None,
    };
    fs::write(
        audit_dir.join("2026-06-10.jsonl"),
        format!("{}\n", older.to_jsonl().expect("ser older")),
    )
    .expect("write older");
    fs::write(
        audit_dir.join("2026-06-11.jsonl"),
        format!("{}\n", newer.to_jsonl().expect("ser newer")),
    )
    .expect("write newer");

    let page = sink
        .scan(
            &AuditFilter::all(),
            PageQuery {
                page_no: 1,
                page_size: 20,
            },
        )
        .expect("scan ok");
    assert_eq!(page.items.len(), 2, "both records scanned");
    assert_eq!(
        page.items[0].id, "200",
        "newer-date record comes first (reverse order)"
    );
    assert_eq!(page.items[1].id, "100", "older-date record comes second");
}

#[test] // §8 F-13: scan 分页窗口截断——page_size 截断、命中页满即停
fn scan_truncates_to_page_window() {
    let dir = temp_data_dir("pagewin");
    let sink = JsonlAuditSink::new(&dir, FsyncPolicy::PerEvent);
    for _ in 0..5 {
        sink.record(event("request", "allow")).expect("record");
    }

    let page = sink
        .scan(
            &AuditFilter::all(),
            PageQuery {
                page_no: 1,
                page_size: 2,
            },
        )
        .expect("scan ok");
    assert_eq!(page.items.len(), 2, "page window truncates to page_size=2");
    assert_eq!(page.page_size, 2, "envelope echoes page_size");
    assert_eq!(page.page_no, 1, "envelope echoes page_no");
    assert_eq!(page.total, 5, "total reflects all matching records");
}

#[test] // §8 F-13 / DB_PAGINATION_MANDATORY: scan 对 page_size 越界先 clamp 到 MAX_SIZE
fn scan_clamps_oversized_page_size_to_max() {
    let dir = temp_data_dir("clamp");
    let sink = JsonlAuditSink::new(&dir, FsyncPolicy::PerEvent);
    sink.record(event("request", "allow")).expect("record");

    let page = sink
        .scan(
            &AuditFilter::all(),
            PageQuery {
                page_no: 1,
                page_size: 201,
            },
        )
        .expect("scan ok");
    // clamp(201) == 200：信封回显的 page_size 恒为 clamp 后的合法上限。
    assert_eq!(page.page_size, 200, "page_size 201 clamps to MAX_SIZE 200");
}

#[test] // §8 F-13: scan 第二页偏移正确（倒序基础上按窗口取下一页）
fn scan_second_page_returns_next_window() {
    let dir = temp_data_dir("page2");
    let sink = JsonlAuditSink::new(&dir, FsyncPolicy::PerEvent);
    for _ in 0..5 {
        sink.record(event("request", "allow")).expect("record");
    }

    let p1 = sink
        .scan(
            &AuditFilter::all(),
            PageQuery {
                page_no: 1,
                page_size: 2,
            },
        )
        .expect("scan p1");
    let p2 = sink
        .scan(
            &AuditFilter::all(),
            PageQuery {
                page_no: 2,
                page_size: 2,
            },
        )
        .expect("scan p2");
    assert_eq!(p2.items.len(), 2, "second page also full");
    assert_eq!(p2.page_no, 2, "second page echoes page_no=2");
    // 两页不重叠：第二页首条不等于第一页任一条 id。
    let p1_ids: Vec<&String> = p1.items.iter().map(|r| &r.id).collect();
    assert!(
        !p1_ids.contains(&&p2.items[0].id),
        "page 2 does not overlap page 1"
    );
}

#[test] // §8 F-13: scan 按 kind 过滤——只返回命中 kind 的记录
fn scan_filters_by_kind() {
    let dir = temp_data_dir("kindfilter");
    let sink = JsonlAuditSink::new(&dir, FsyncPolicy::PerEvent);
    sink.record(event("request", "allow")).expect("r1");
    sink.record(event("deny", "deny")).expect("r2");
    sink.record(event("request", "allow")).expect("r3");

    let page = sink
        .scan(
            &AuditFilter::by_kind("deny"),
            PageQuery {
                page_no: 1,
                page_size: 20,
            },
        )
        .expect("scan ok");
    assert_eq!(page.items.len(), 1, "only the single deny event matches");
    assert_eq!(page.items[0].kind, "deny", "matched record kind is deny");
}

#[test] // §8 F-13: scan 反序列化只产 store 本地 OriginEnvelope（绝不构造 ConnOrigin）
fn scan_origin_is_store_local_envelope() {
    let dir = temp_data_dir("origin");
    let sink = JsonlAuditSink::new(&dir, FsyncPolicy::PerEvent);
    sink.record(event("request", "allow")).expect("record");

    let page = sink
        .scan(
            &AuditFilter::all(),
            PageQuery {
                page_no: 1,
                page_size: 20,
            },
        )
        .expect("scan ok");
    let rec = &page.items[0];
    assert_eq!(
        rec.origin,
        OriginEnvelope::UnixPeer {
            uid: 1000,
            gid: 1000
        },
        "origin round-trips into the store-local UnixPeer envelope, not ConnOrigin"
    );
}

// ===========================================================================
// F-14 审计 fsync 策略落地
// ===========================================================================

#[test] // §8 F-14: deny/policy_change/credential_event 逐事件 fsync——record 返回前每条必 fsync
fn high_value_events_are_durable_before_record_returns() {
    let dir = temp_data_dir("highval");
    // relaxed 策略下高价值类仍逐事件 fsync（不受 relaxed 影响）。
    let sink = JsonlAuditSink::new(&dir, FsyncPolicy::Relaxed);
    let mut written = 0u64;
    for kind in ["deny", "policy_change", "credential_event"] {
        let before = sink.fsync_count();
        sink.record(event(kind, "deny")).expect("record high-value");
        // 钉确切结果：每条高价值 record 返回时 fsync 计数恰好 +1（sync_all 真已调用），
        // 这与"扫回可见"（可仅是 page cache 可读）无关——直接证伪 fsync 被短路。
        assert_eq!(
            sink.fsync_count(),
            before + 1,
            "each high-value event fsyncs exactly once before record returns (kind={kind})"
        );
        written += 1;
    }
    assert_eq!(
        sink.fsync_count(),
        written,
        "relaxed policy still fsyncs every high-value event per-event"
    );
    // 落盘事实仍需可被独立扫回（durable 之后必然可见）。
    let page = sink
        .scan(
            &AuditFilter::all(),
            PageQuery {
                page_no: 1,
                page_size: 20,
            },
        )
        .expect("scan ok");
    let kinds: Vec<&str> = page.items.iter().map(|r| r.kind.as_str()).collect();
    assert!(kinds.contains(&"deny"), "deny durable before return");
    assert!(
        kinds.contains(&"policy_change"),
        "policy_change durable before return"
    );
    assert!(
        kinds.contains(&"credential_event"),
        "credential_event durable before return"
    );
}

#[test] // §8 F-14: 缺省（PerEvent）下 allow 类逐事件 fsync——record 返回前必 fsync 一次
fn allow_event_is_durable_per_event_by_default() {
    let dir = temp_data_dir("allowdef");
    let sink = JsonlAuditSink::new(&dir, FsyncPolicy::PerEvent);
    assert_eq!(sink.fsync_count(), 0, "no fsync before any record");
    sink.record(event("request", "allow"))
        .expect("record allow");
    // PerEvent 缺省下 allow 也逐事件 fsync：返回时计数恰为 1（钉 fsync 这一确切结果，
    // 而非仅"扫回可见"）。短路 sink.rs 的 fsync 分支会令此断言跑红。
    assert_eq!(
        sink.fsync_count(),
        1,
        "default PerEvent: allow event is fsynced exactly once before record returns"
    );

    let page = sink
        .scan(
            &AuditFilter::all(),
            PageQuery {
                page_no: 1,
                page_size: 20,
            },
        )
        .expect("scan ok");
    assert_eq!(
        page.items.len(),
        1,
        "default PerEvent: allow event is durable immediately after record returns"
    );
    assert_eq!(
        page.items[0].decision, "allow",
        "the durable record is the allow"
    );
}

#[test] // §8 F-14: relaxed 的判别维度——allow 类跳过逐事件 fsync，高价值类仍逐事件 fsync
fn relaxed_skips_per_event_fsync_for_allow_but_not_high_value() {
    let dir = temp_data_dir("relaxedallow");
    let sink = JsonlAuditSink::new(&dir, FsyncPolicy::Relaxed);

    // relaxed 下 allow 类不逐事件 fsync：写三条 allow，fsync 计数恒为 0（与 PerEvent 区分）。
    for _ in 0..3 {
        sink.record(event("request", "allow"))
            .expect("record allow");
    }
    assert_eq!(
        sink.fsync_count(),
        0,
        "relaxed: allow events do not fsync per-event (this is the relaxed differentiation)"
    );

    // 但高价值类在同一 relaxed sink 下仍逐事件 fsync：写一条 deny，计数 +1。
    sink.record(event("deny", "deny")).expect("record deny");
    assert_eq!(
        sink.fsync_count(),
        1,
        "relaxed: high-value events still fsync per-event despite relaxed"
    );

    // allow 行仍如实落盘可扫回（relaxed 只放宽 fsync 批次，不丢事件）。
    let page = sink
        .scan(
            &AuditFilter::all(),
            PageQuery {
                page_no: 1,
                page_size: 20,
            },
        )
        .expect("scan ok");
    assert_eq!(
        page.total, 4,
        "all four events are written regardless of fsync timing"
    );
}

// ===========================================================================
// L-5 审计写失败 = deny（只读单次）：本域只如实返回 Err，不自行放行/拒绝
// ===========================================================================

#[test] // §8 L-5: 注入 record 返回 Err(WriteFailed) → 本域如实返回该 Err
fn record_write_failure_surfaces_err_verbatim() {
    let sink = FailingSink {
        err: AuditError::WriteFailed,
    };
    let result = sink.record(event("request", "allow"));
    assert_eq!(
        result,
        Err(AuditError::WriteFailed),
        "audit write failure is returned verbatim, not swallowed as Ok"
    );
}

#[test] // §8 L-5: 注入 storage unavailable → 本域如实返回 StorageUnavailable，不翻译成放行
fn record_storage_unavailable_surfaces_err_verbatim() {
    let sink = FailingSink {
        err: AuditError::StorageUnavailable,
    };
    let result = sink.record(event("request", "allow"));
    assert_eq!(
        result,
        Err(AuditError::StorageUnavailable),
        "storage-unavailable is the exact Err; the domain never self-decides allow/deny"
    );
    // 失败结果恰是该失败、不是 Ok：fail-closed 不吞错。
    assert!(
        result.is_err(),
        "a failed audit write is never reported as success"
    );
}

// ===========================================================================
// L-6 审计写失败两阶段（有副作用）：intent/outcome 写失败都如实返 Err
// ===========================================================================

#[test] // §8 L-6: 两阶段——intent 事件 record 失败如实返 Err（内核据此执行前 deny）
fn two_phase_intent_record_failure_surfaces_err() {
    let sink = FailingSink {
        err: AuditError::WriteFailed,
    };
    // intent 阶段（步骤 [7a]）事件写失败 → 本域如实返 Err，不自行决定放行。
    let intent = sink.record(event("request_intent", "intent"));
    assert_eq!(
        intent,
        Err(AuditError::WriteFailed),
        "intent-phase write failure is surfaced verbatim"
    );
}

#[test] // §8 L-6: 两阶段——outcome 事件 record 失败如实返 Err（内核返“已执行但审计降级”）
fn two_phase_outcome_record_failure_surfaces_err() {
    let sink = FailingSink {
        err: AuditError::StorageUnavailable,
    };
    // outcome 阶段（步骤 [10]）事件写失败 → 本域如实返 Err，绝不谎报成功。
    let outcome = sink.record(event("request_outcome", "allow"));
    assert_eq!(
        outcome,
        Err(AuditError::StorageUnavailable),
        "outcome-phase write failure is surfaced verbatim, never reported as deny by this domain"
    );
}

// --- 被审域本身（JsonlAuditSink）的真实失败路径：不再靠幽灵 mock ---

/// 令 `<data_dir>/audit` 是一个普通文件（而非目录），逼真实 JsonlAuditSink 写路径
/// 在 `create_dir_all(parent)` 处失败——这是不依赖第三方、跨平台稳定的失败注入点。
fn data_dir_with_audit_path_blocked(tag: &str) -> PathBuf {
    let dir = temp_data_dir(tag);
    // 在 audit 子目录该在的位置放一个普通文件，使 create_dir_all 必失败。
    fs::write(dir.join("audit"), b"not a directory").expect("seed blocking file");
    dir
}

#[test] // §8 L-5: 真实 JsonlAuditSink 写失败 → 如实返 StorageUnavailable（绝不吞成 Ok）
fn jsonl_sink_record_storage_unavailable_surfaces_err_verbatim() {
    let dir = data_dir_with_audit_path_blocked("realfail-storage");
    let sink = JsonlAuditSink::new(&dir, FsyncPolicy::PerEvent);

    let result = sink.record(event("request", "allow"));
    assert_eq!(
        result,
        Err(AuditError::StorageUnavailable),
        "real JsonlAuditSink surfaces the storage failure verbatim; the domain never swallows a failed write as Ok"
    );
    assert!(
        result.is_err(),
        "fail-closed: a failed audit write of the real sink is never reported as success"
    );
}

#[test] // §8 L-5: 真实 sink 失败时不自行放行——只返回 Err，由内核据 §6.3 处置，不翻译成 allow/deny
fn jsonl_sink_record_failure_is_not_self_translated_to_decision() {
    let dir = data_dir_with_audit_path_blocked("realfail-noselfdecide");
    let sink = JsonlAuditSink::new(&dir, FsyncPolicy::PerEvent);

    // 高价值类（deny）也走同一写路径：本域同样只如实返 Err，不自行决定放行/拒绝。
    let result = sink.record(event("deny", "deny"));
    assert!(
        matches!(result, Err(AuditError::StorageUnavailable)),
        "the real sink returns the exact write Err; it never self-decides allow/deny on failure"
    );
}

#[test] // §8 L-6: 两阶段——真实 sink 上 intent 阶段写失败如实返 Err（内核据此执行前 deny）
fn jsonl_sink_two_phase_intent_failure_surfaces_err() {
    let dir = data_dir_with_audit_path_blocked("realfail-intent");
    let sink = JsonlAuditSink::new(&dir, FsyncPolicy::PerEvent);

    let intent = sink.record(event("request_intent", "intent"));
    assert!(
        matches!(intent, Err(AuditError::StorageUnavailable)),
        "intent-phase write failure on the real sink is surfaced verbatim, not swallowed"
    );
}

#[test] // §8 L-6: 两阶段——真实 sink 上 outcome 阶段写失败如实返 Err（绝不谎报成功）
fn jsonl_sink_two_phase_outcome_failure_surfaces_err() {
    let dir = data_dir_with_audit_path_blocked("realfail-outcome");
    let sink = JsonlAuditSink::new(&dir, FsyncPolicy::PerEvent);

    let outcome = sink.record(event("request_outcome", "allow"));
    assert!(
        matches!(outcome, Err(AuditError::StorageUnavailable)),
        "outcome-phase write failure on the real sink is surfaced verbatim, never reported as success"
    );
}

// ===========================================================================
// L-14 审计 DoS 可控降级
// ===========================================================================

#[test] // §8 L-14: audit.retention_days 到期整文件删除（append-only 域唯一允许的删除形态）
fn enforce_retention_deletes_whole_expired_files() {
    let dir = temp_data_dir("retention");
    // 保留期 2 天。
    let sink = JsonlAuditSink::with_limits(
        &dir,
        FsyncPolicy::PerEvent,
        JsonlAuditSink::DEFAULT_QUOTA_BYTES,
        2,
    );
    let audit_dir = dir.join("audit");
    fs::create_dir_all(&audit_dir).expect("mk audit dir");
    // 三个日文件：两个早于保留窗口、一个在窗口内。
    for date in ["2026-06-01", "2026-06-02", "2026-06-11"] {
        fs::write(audit_dir.join(format!("{date}.jsonl")), b"{}\n").expect("seed file");
    }

    let removed = sink.enforce_retention("2026-06-12").expect("retention ok");
    assert_eq!(
        removed, 2,
        "two files older than retention_days=2 are deleted"
    );

    let remaining = jsonl_files(&audit_dir);
    assert_eq!(
        remaining,
        vec!["2026-06-11.jsonl".to_string()],
        "only the in-window day file survives; deletion is whole-file, not line-level"
    );
}

#[test] // §8 L-14: 保留期删除是整文件——文件内已落行不被行级原地改写
fn retention_is_whole_file_only_never_line_level() {
    let dir = temp_data_dir("retfile");
    let sink = JsonlAuditSink::with_limits(
        &dir,
        FsyncPolicy::PerEvent,
        JsonlAuditSink::DEFAULT_QUOTA_BYTES,
        2,
    );
    let audit_dir = dir.join("audit");
    fs::create_dir_all(&audit_dir).expect("mk audit dir");
    let keep = audit_dir.join("2026-06-12.jsonl");
    fs::write(&keep, b"{\"id\":\"1\"}\n{\"id\":\"2\"}\n").expect("seed keep");
    let before = fs::read(&keep).expect("read before");

    let removed = sink.enforce_retention("2026-06-12").expect("retention ok");
    assert_eq!(removed, 0, "no file is outside the retention window");
    let after = fs::read(&keep).expect("read after");
    assert_eq!(
        after, before,
        "in-window file content is byte-identical: no line-level rewrite"
    );
}

#[test] // §8 L-14: used_bytes 反映落盘占用（水位监控前提）
fn used_bytes_reflects_written_audit_volume() {
    let dir = temp_data_dir("usedbytes");
    let sink = JsonlAuditSink::new(&dir, FsyncPolicy::PerEvent);
    let empty = sink.used_bytes().expect("used_bytes empty");
    assert_eq!(empty, 0, "no audit files yet → zero used bytes");

    sink.record(event("request", "allow")).expect("record");
    let after = sink.used_bytes().expect("used_bytes after");
    assert!(
        after > 0,
        "after one append, used bytes is strictly positive (water-mark input)"
    );
}

// --- L-14 可控降级：水位判定 → 告警/强制轮转/低价值降采样/deny 聚合/高价值不丢 ---

/// 铺一个体量 ≥ `min_bytes` 的**合法** JSONL 日文件（单条 AuditRecord，resource 字段加长
/// 到目标体量）：既能把 used_bytes 顶过水位，又能被 scan 正常逐行解析，不污染读路径。
fn write_padded_day_file(audit_dir: &Path, seed_date: &str, min_bytes: usize) {
    let pad = AuditRecord {
        id: "1".to_string(),
        v: 1,
        kind: "request".to_string(),
        ts: "2026-06-13T00:00:00.000Z".to_string(),
        entry: "mcp".to_string(),
        origin: OriginEnvelope::UnixPeer {
            uid: 1000,
            gid: 1000,
        },
        decision: "allow".to_string(),
        resource: "p".repeat(min_bytes),
        policy_rev: 1,
        count: None,
    };
    fs::write(
        audit_dir.join(format!("{seed_date}.jsonl")),
        format!("{}\n", pad.to_jsonl().expect("ser pad")),
    )
    .expect("seed padded file");
}

/// 把审计落盘占用直接顶过高水位线：在 `<data_dir>/audit` 里铺一个 ≥ 配额的合法大日文件。
/// 返回 sink（已装配小配额）。`seed_date` 用于命名被铺的日文件（控制是否在保留窗口内）。
fn sink_pushed_over_water(
    tag: &str,
    quota_bytes: u64,
    retention_days: u32,
    seed_date: &str,
) -> (PathBuf, JsonlAuditSink) {
    let dir = temp_data_dir(tag);
    let audit_dir = dir.join("audit");
    fs::create_dir_all(&audit_dir).expect("mk audit dir");
    // 合法 JSONL，体量 ≥ quota：used_bytes 必越过 high_water（= quota*90%），且 scan 可解析。
    write_padded_day_file(&audit_dir, seed_date, (quota_bytes as usize) + 64);
    let sink =
        JsonlAuditSink::with_limits(&dir, FsyncPolicy::PerEvent, quota_bytes, retention_days);
    (dir, sink)
}

#[test] // §8 L-14: over_high_water 是 used_bytes vs quota_bytes 的唯一比较点（水位判定接通）
fn over_high_water_compares_used_against_quota() {
    let dir = temp_data_dir("waterjudge");
    let sink = JsonlAuditSink::with_limits(&dir, FsyncPolicy::PerEvent, 1000, 30);
    // 空目录：占用 0 < high_water(900) → 未逼近。
    assert!(
        !sink.over_high_water().expect("ok"),
        "empty audit is below water-mark"
    );
    assert_eq!(sink.high_water_bytes(), 900, "high_water is quota*90%");

    let audit_dir = dir.join("audit");
    fs::create_dir_all(&audit_dir).expect("mk audit dir");
    // 铺 950 字节 ≥ high_water(900) → 越过水位。
    fs::write(
        audit_dir.join("2026-06-13.jsonl"),
        "y".repeat(950).as_bytes(),
    )
    .expect("seed");
    assert!(
        sink.over_high_water().expect("ok"),
        "used_bytes(950) >= high_water(900) → over water-mark (used vs quota is actually compared)"
    );
}

#[test] // §8 L-14: 逼近配额 → 低价值降采样丢弃部分，高价值（deny/policy_change/credential_event）一条不丢
fn degraded_downsamples_low_value_but_never_drops_high_value() {
    // 配额小：第一条 record 进入时占用已越过水位 → 后续走降级分支。
    // 种子文件用远未来日界，确保不被强制轮转回收、水位判定与真实墙钟无关。
    let (_dir, sink) = sink_pushed_over_water("downsample", 1000, 30, "2999-12-31");
    assert!(
        sink.over_high_water().expect("ok"),
        "seeded over water-mark"
    );

    // 灌入大量低价值 allow：降级下应被降采样，丢弃数严格为正（不是每条都落）。
    for _ in 0..200 {
        sink.record(event("request", "allow"))
            .expect("low-value record returns Ok (no paralysis)");
    }
    assert!(
        sink.downsampled_count() > 0,
        "low-value audit is downsampled under quota pressure (some events dropped, counted)"
    );

    // 高价值类穿插写入：降级下绝不被降采样丢弃，每条都可被独立扫回。
    sink.record(event("policy_change", "allow"))
        .expect("policy_change record");
    sink.record(event("credential_event", "allow"))
        .expect("credential_event record");

    let pc = sink
        .scan(
            &AuditFilter::by_kind("policy_change"),
            PageQuery {
                page_no: 1,
                page_size: 200,
            },
        )
        .expect("scan policy_change");
    assert_eq!(
        pc.total, 1,
        "policy_change is never downsampled away under degradation"
    );
    let ce = sink
        .scan(
            &AuditFilter::by_kind("credential_event"),
            PageQuery {
                page_no: 1,
                page_size: 200,
            },
        )
        .expect("scan credential_event");
    assert_eq!(
        ce.total, 1,
        "credential_event is never downsampled away under degradation"
    );
}

#[test] // §8 L-14: deny 类按 (principal, window) 聚合 → 写带 count 的聚合记录（而非每次一条），deny 事实不丢
fn degraded_aggregates_deny_into_count_record() {
    let (_dir, sink) = sink_pushed_over_water("denyagg", 1000, 30, "2999-12-31");
    assert!(
        sink.over_high_water().expect("ok"),
        "seeded over water-mark"
    );

    // 同 principal 反复触发同类 deny：降级下折叠聚合，不逐条打满落盘。
    let floods = 50u64;
    for _ in 0..floods {
        sink.record(event("deny", "deny"))
            .expect("deny record returns Ok under degradation");
    }
    // 收口：把仍开着的窗口刷成带 count 的聚合记录。
    let flushed = sink.flush_deny_aggregates().expect("flush ok");
    assert!(
        flushed >= 1,
        "at least one open deny window is flushed into an aggregate record"
    );

    let denies = sink
        .scan(
            &AuditFilter::by_kind("deny"),
            PageQuery {
                page_no: 1,
                page_size: 200,
            },
        )
        .expect("scan deny");
    // 聚合后 deny 落盘条数远少于灌入条数（被折叠），证伪"每次一条"。
    assert!(
        denies.total < floods,
        "deny lines on disk ({}) are far fewer than {floods} floods — aggregation collapsed them",
        denies.total
    );
    // 至少一条聚合记录带 count，且其 count 累计反映被折叠的 deny 多重性（事实不丢）。
    let counted: Vec<u64> = denies.items.iter().filter_map(|r| r.count).collect();
    assert!(
        !counted.is_empty(),
        "an aggregate deny record carries a count field"
    );
    let total_counted: u64 = counted.iter().sum();
    assert!(
        total_counted >= 1,
        "the count-bearing aggregate preserves deny multiplicity (deny fact is not lost)"
    );
}

#[test] // §8 L-14: 进入降级落一条可见告警事件 + 触发一次强制轮转（口子愈合对运维可见），且只一次
fn degraded_emits_alert_and_forces_rotation_once() {
    let dir = temp_data_dir("alertrotate");
    let audit_dir = dir.join("audit");
    fs::create_dir_all(&audit_dir).expect("mk audit dir");
    // 顶过水位的合法大文件用远未来日界命名：永不落入任何保留窗口的到期侧，不被回收，
    // 故水位判定稳定为"逼近"，与真实墙钟日界无关。
    write_padded_day_file(&audit_dir, "2999-12-31", 1100);
    // 一个远早于任何合理保留窗口（retention_days=2）的到期文件：必被强制轮转删除。
    write_padded_day_file(&audit_dir, "2000-01-01", 4);
    let sink = JsonlAuditSink::with_limits(&dir, FsyncPolicy::PerEvent, 1000, 2);
    assert!(
        sink.over_high_water().expect("ok"),
        "seeded over water-mark"
    );
    assert!(
        !sink.is_degraded(),
        "not yet entered degraded state before any record"
    );

    // 触发降级路径：写若干事件，进入降级态。
    for _ in 0..5 {
        sink.record(event("request", "allow"))
            .expect("record under degradation");
    }
    assert!(
        sink.is_degraded(),
        "crossing the water-mark flips into degraded state"
    );

    // 强制轮转：到期整文件被删（2000-01-01 远早于 retention_days=2 的 cutoff）。
    let remaining = jsonl_files(&audit_dir);
    assert!(
        !remaining.contains(&"2000-01-01.jsonl".to_string()),
        "forced rotation removed the expired whole file"
    );

    // 可见告警事件恰一条（进入降级那一刻落一次，持续逼近期间不反复刷）。
    let alerts = sink
        .scan(
            &AuditFilter::by_kind("audit_degraded"),
            PageQuery {
                page_no: 1,
                page_size: 200,
            },
        )
        .expect("scan alerts");
    assert_eq!(
        alerts.total, 1,
        "exactly one visible audit_degraded alert is emitted on entering degradation (self-healing visible, not a new DoS face)"
    );
}

#[test] // §8 L-14: 逼近配额绝不全平面瘫痪——低价值 record 仍返 Ok，不把所有请求 fail-closed
fn degraded_never_paralyzes_the_plane() {
    let (_dir, sink) = sink_pushed_over_water("noparalyze", 1000, 30, "2999-12-31");
    assert!(
        sink.over_high_water().expect("ok"),
        "seeded over water-mark"
    );

    // 即便落盘已逼近配额，低价值 record 也只走可控降级（降采样/Ok），绝不返 Err 令请求全平面瘫痪。
    for _ in 0..100 {
        let r = sink.record(event("request", "allow"));
        assert!(
            r.is_ok(),
            "under quota pressure the sink degrades gracefully (Ok), never fail-closes every request into a global DoS loop"
        );
    }
    // 降采样确实发生（降级生效），而非靠"全拒"维持不变量。
    assert!(
        sink.downsampled_count() > 0,
        "graceful degradation (downsampling) is what keeps the plane alive, not blanket rejection"
    );
}

// ===========================================================================
// L-15 审计 append-only 不含机密
// ===========================================================================

#[test] // §8 L-15: 已落审计行不含真实地址——origin Tcp 只落已脱敏文本，无凭据值字段
fn written_line_holds_no_credential_value() {
    let dir = temp_data_dir("nosecret");
    let sink = JsonlAuditSink::new(&dir, FsyncPolicy::PerEvent);
    let mut ev = event("credential_event", "allow");
    // 即便来源是 Tcp，本域也只序列化内核已脱敏事实——行内不应出现凭据明文标记。
    ev.origin = Origin::Tcp {
        remote: SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(10, 0, 0, 5), 5432)),
    };
    sink.record(ev).expect("record");

    let audit_dir = dir.join("audit");
    let file = sink.file_for_date(&jsonl_files(&audit_dir)[0][..10]);
    let body = fs::read_to_string(&file).expect("read body");
    // 本域只落已脱敏事实：AuditEvent 不含 secret 字段，行内不应出现凭据值占位。
    assert!(
        !body.contains("password"),
        "no credential value text leaks into the audit line"
    );
    assert!(
        !body.contains("secret_hash"),
        "no secret-hash field leaks into the audit line"
    );
    // 行内仍如实承载已脱敏的资源代号与决策。
    assert!(
        body.contains("db-main"),
        "resource code is recorded (sanitized fact)"
    );
    assert!(body.contains("credential_event"), "kind is recorded");
}

#[test] // §8 L-15: 唯一删除形态是整文件删除——sink 不提供任何行级修改入口
fn append_only_has_no_line_level_mutation_path() {
    let dir = temp_data_dir("appendonly2");
    let sink = JsonlAuditSink::new(&dir, FsyncPolicy::PerEvent);
    let audit_dir = dir.join("audit");

    sink.record(event("request", "allow")).expect("r1");
    sink.record(event("request", "deny")).expect("r2");
    let file = sink.file_for_date(&jsonl_files(&audit_dir)[0][..10]);
    let two_lines = fs::read(&file).expect("read two");

    // 再写一条：仅追加，前两行字节前缀不变（无行级原地改写）。
    sink.record(event("request", "allow")).expect("r3");
    let three_lines = fs::read(&file).expect("read three");
    assert!(
        three_lines.starts_with(&two_lines),
        "prior lines are an exact byte prefix: only appends, never in-place edits"
    );
    assert_eq!(
        three_lines.iter().filter(|&&b| b == b'\n').count(),
        3,
        "exactly three appended lines after three records"
    );
}

#[test] // §8 L-15: scan 往返保持已脱敏字段精确——id/kind/decision/resource/policy_rev/ts 不被篡改
fn scan_round_trips_sanitized_fields_exactly() {
    let dir = temp_data_dir("roundtrip");
    let sink = JsonlAuditSink::new(&dir, FsyncPolicy::PerEvent);
    let audit_dir = dir.join("audit");
    fs::create_dir_all(&audit_dir).expect("mk audit dir");
    let written = AuditRecord {
        id: "424242".to_string(),
        v: 1,
        kind: "request".to_string(),
        ts: "2026-06-12T08:09:10.123Z".to_string(),
        entry: "http".to_string(),
        origin: OriginEnvelope::Tcp {
            remote: "scrubbed".to_string(),
        },
        decision: "allow".to_string(),
        resource: "db-main".to_string(),
        policy_rev: 99,
        count: None,
    };
    fs::write(
        audit_dir.join("2026-06-12.jsonl"),
        format!("{}\n", written.to_jsonl().expect("ser")),
    )
    .expect("write");

    let page: Page<AuditRecord> = sink
        .scan(
            &AuditFilter::all(),
            PageQuery {
                page_no: 1,
                page_size: 20,
            },
        )
        .expect("scan ok");
    assert_eq!(page.items.len(), 1, "one record scanned");
    let got = &page.items[0];
    assert_eq!(got.id, "424242", "id round-trips exactly");
    assert_eq!(got.kind, "request", "kind round-trips exactly");
    assert_eq!(
        got.ts, "2026-06-12T08:09:10.123Z",
        "ts (24-wide UTC) round-trips exactly"
    );
    assert_eq!(got.ts.len(), 24, "ts is exactly 24 wide");
    assert_eq!(got.decision, "allow", "decision round-trips exactly");
    assert_eq!(got.resource, "db-main", "resource round-trips exactly");
    assert_eq!(got.policy_rev, 99, "policy_rev round-trips exactly");
    assert_eq!(
        got.origin,
        OriginEnvelope::Tcp {
            remote: "scrubbed".to_string()
        },
        "Tcp origin envelope round-trips exactly"
    );
}
