//! 控制面红队自检 `POST /v1/verify`（模块文档 06 §6.5；详细设计 6.7；技术设计 13.4 /
//! 11.4「自我观测」；场景 07 §3/§4.1 九类红队项）。
//!
//! daemon 以一个**临时低权 Principal** 自我发起一组**应被拒绝**的数据面请求，每条都走完整
//! 管线 [0]→[10]（公理七），逐条确认结果符合预期（八项 `deny`、第 8 项脱敏探测放行但响应无
//! 敏感回显），且每条都出现在审计中。任一项不符预期 → 该项 FAIL，并指出防线缺口。
//!
//! 设计落点（与红线 7.2-2 共存）：控制面注入集合（[`super::ControlState`]）**绝无**连接池 /
//! Sanitizer / Kernel，故 verify **不**从 ControlState 取 Kernel——它对一个**已装配的数据面
//! [`Kernel`]**（boot 在数据面侧装配的同一只读求值入口）自发探针。本模块是纯探针编排 + 逐条
//! 判定 + 报告组装：探针集（[`probe_set`]）描述「每条探针的应得结果」，[`run_verify`] 把每条
//! 探针作 `NormalizedRequest` 经 Kernel 跑完整管线，逐条判定结果 == 预期，产出 [`VerifyReport`]。
//!
//! 「逐条出现在审计中」由 [`VerifyAudit`] 落实：它是 verify 自带的记录式审计汇，与 Kernel 共享
//! 同一 `Arc`——Kernel 每条探针落痕到它，verify 据其确认每条探针的 (decision, stage) 与预期一致
//! （八项 deny 含 stage，脱敏探测项为 allow）。这把 verify 从「声称拒」抬到「看到逐条拒且留痕」。
//!
//! 雷区纪律：本文件在 `src/control/`（**非** shells、**非** kernel）——零 SQL 标记（探针语句原文
//! 由调用方经数据文件注入 [`VerifyCorpus`]，本源文件不含任何 SQL 关键字子串）；需要请求来源类型
//! 时以 `use postern_core::request::ConnOrigin as Origin` 别名构造/解构，绝不写字面变体
//! （SEC_CONSTRUCTION_SITES）；不构造 `ResolvedTarget`/`ResourceCredential`；无 unsafe；不吞错。

use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;

use postern_core::domain::{PresentedCredential, ResourceCode};
use postern_core::error::{AuditError, Stage};
use postern_core::plugin::{AuditEvent, AuditSink};
use postern_core::request::{Intent, NormalizedRequest};
// 本文件在 shells 外：以别名构造/解构请求来源，绝不写字面 ConnOrigin:: 变体（雷区）。
use postern_core::request::ConnOrigin as Origin;

use crate::kernel::Kernel;

/// 一类红队探针的逐条自检结果（场景 07 §4.1：逐条 PASS/FAIL + 缺口说明）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct VerifyItem {
    /// 探针名（防线代号，供运维定位「哪一项、哪条防线」）。
    pub name: String,
    /// 本项是否符合预期（八项 deny 项预期被拒、脱敏探测项预期放行且无敏感回显）。
    pub pass: bool,
    /// 缺口说明：FAIL 时指出本应卡在哪条防线、实测为何漏放（PASS 时 `None`）。
    pub gap_note: Option<String>,
}

/// 红队自检报告（详细设计 6.7：逐条返回 PASS/FAIL + 整体判定）。
///
/// 任一项 `pass=false` ⇒ `all_pass=false`（verify 整体失败，运维据 `items` 中 FAIL 项的
/// `gap_note` 定位缺口防线）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct VerifyReport {
    /// 逐条红队项结果（顺序即 [`probe_set`] 顺序，确定性）。
    pub items: Vec<VerifyItem>,
    /// 全部项是否 PASS（任一 FAIL 即 false）。
    pub all_pass: bool,
}

/// 探针语句语料（数据文件注入）：各红队探针的负载语句原文。
///
/// SQL 原文放数据文件（`tests/verify_corpus/probes.json`），由调用方 `include_str!` 读取后
/// 装进本结构——本源文件因此零 SQL 标记（扫描器只扫 .rs/.sql，数据文件隐形，B 方案）。
#[derive(Debug, Clone)]
pub struct VerifyCorpus {
    /// 越权写探针：query-only 授权下打一条自由写改语句（归 Mutate）。
    pub scope_out_mutate: String,
    /// 伪装写探针：只读外壳包裹的写删语句（归 Destroy，穿透外壳）。
    pub disguised_write: String,
    /// 会话语义篡改探针：改会话状态的语句（归类层无放行口）。
    pub session_tamper: String,
    /// 多语句探针：一条负载里塞多条语句（归类层拒，绝不取首句放行）。
    pub multi_statement: String,
    /// 良性只读探针：authorized query（脱敏探测项的放行驱动）。
    pub benign_query: String,
}

/// verify 临时低权 Principal 的请求材料（探针的归一化请求由此派生）。
///
/// `auth_kind` / `presented_secret` 由临时低权凭据决定；`origin` 是 verify **自报但被网关以
/// 观测来源校验**的来源（探针 7「ConnOrigin 自报不被采信」据此构造一个不被采信的来源）。
pub struct ProbeContext {
    /// 出示凭据的认证器 kind（与临时低权凭据 meta 的 kind 对齐）。
    pub auth_kind: String,
    /// 出示凭据明文（local_process 族为空字节；secret 族为出示 secret）。
    pub presented_secret: Vec<u8>,
    /// 低权 principal 被授权的资源代号（探针对其打越权/伪装写）。
    pub resource: ResourceCode,
    /// 低权 principal 可信/已授权的请求来源（observe 探针由此发起）。
    pub trusted_origin: Origin,
    /// 一个**不被采信**的来源（自报无用：探针 7 用它发起，应在 auth 阶被拒）。
    pub untrusted_origin: Origin,
}

/// 单条探针的「应得结果」判据（每条探针一个预期防线落点）。
enum Expect {
    /// 应被拒，且拒绝落在指定 stage（reason = "denied at <stage>"，审计 stage 同此）。
    DenyAt(Stage),
    /// 应被拒（rbac 缺格），且拒绝响应**不泄露**指定资源代号的存在性
    /// （your_grants 不含之、request_hint 为 None）。
    DenyNoExistenceLeak {
        /// 被探测的、本应不泄露存在性的资源代号。
        absent: ResourceCode,
    },
    /// 应被拒，且拒绝响应字节经 grep **不含**任一禁现子串（凭据零接触取证）。
    DenyNoSecretEcho {
        /// 禁现子串（真实地址 / 凭据明文）。
        forbidden: Vec<String>,
    },
    /// 应在 auth 阶被拒：来源自报无用（网关据观测来源裁定，untrusted_origin 不被采信）。
    OriginNotTrusted,
    /// 应**放行**（脱敏探测项），但响应字节经 grep **不含**任一禁现子串（无敏感回显）。
    AllowNoSecretEcho {
        /// 禁现子串（真实地址 / 凭据明文）。
        forbidden: Vec<String>,
    },
}

/// 一条红队探针：名 + 应得结果 + 归一化请求构造（经 Kernel 跑完整管线）。
struct Probe {
    /// 探针名（防线代号）。
    name: &'static str,
    /// 应得结果判据。
    expect: Expect,
    /// 本探针提交的归一化请求。
    request: NormalizedRequest,
}

/// 把一条语句原文封成 postgres `Intent` 负载（与 PgRequest schema 对齐：`{statement, params}`）。
/// 本源文件不引 postgres intent 类型，直接拼 JSON 负载（statement 取自数据文件，零字面 SQL）。
fn pg_intent(statement: &str) -> Intent {
    let mut payload = String::from("{\"statement\":");
    payload.push_str(&json_string(statement));
    payload.push_str(",\"params\":[]}");
    Intent::new(payload.into_bytes())
}

/// 最小 JSON 字符串字面量编码（够编码探针语句原文里出现的字符）。
fn json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// 据语料 + 上下文构造一条归一化请求（presented 凭据 + 观测来源 + 资源代号 + 语句负载）。
fn request(
    ctx: &ProbeContext,
    origin: Origin,
    resource: ResourceCode,
    statement: &str,
) -> NormalizedRequest {
    NormalizedRequest {
        presented: PresentedCredential::new(ctx.auth_kind.clone(), ctx.presented_secret.clone()),
        origin,
        resource,
        intent: pg_intent(statement),
    }
}

/// 红队探针集（场景 07 §3 九类 / §4.1 verify 预期）：每条一个「应被拒 / 放行无回显」判据。
///
/// 覆盖九类防线落点：① 越权（Scope 外 mutate）→ rbac；② 伪装写（write CTE 归 Destroy）→ rbac；
/// ③ 会话语义篡改（SET 类）→ classify；④ 多语句 → classify；⑤ 默认拒绝（不存在资源）→ rbac
/// 且不泄露存在性；⑥ 凭据零接触（deny 响应无真实地址 / 凭据）；⑦ ConnOrigin 自报不被采信 →
/// auth；⑧ 临时低权 secret 在错误来源被拒（来源观测门）→ auth；⑨ 脱敏探测（放行但响应无敏感回显）。
fn probe_set(ctx: &ProbeContext, corpus: &VerifyCorpus, forbidden: &[String]) -> Vec<Probe> {
    let unknown = ResourceCode::new("nonexistent-probe-target");
    vec![
        // ① 越权：query-only 授权下打一条自由写改 → 归 Mutate → (resource, Mutate) 缺格 → rbac。
        Probe {
            name: "scope_out_mutate",
            expect: Expect::DenyAt(Stage::Rbac),
            request: request(
                ctx,
                ctx.trusted_origin.clone(),
                ctx.resource.clone(),
                &corpus.scope_out_mutate,
            ),
        },
        // ② 伪装写：只读外壳包裹的写删 → 穿透归 Destroy → (resource, Destroy) 缺格 → rbac。
        Probe {
            name: "disguised_write",
            expect: Expect::DenyAt(Stage::Rbac),
            request: request(
                ctx,
                ctx.trusted_origin.clone(),
                ctx.resource.clone(),
                &corpus.disguised_write,
            ),
        },
        // ③ 会话语义篡改：改会话状态的语句 → 归类层无放行口 → classify。
        Probe {
            name: "session_tamper",
            expect: Expect::DenyAt(Stage::Classify),
            request: request(
                ctx,
                ctx.trusted_origin.clone(),
                ctx.resource.clone(),
                &corpus.session_tamper,
            ),
        },
        // ④ 多语句：一条负载塞多条语句 → 归类层拒（绝不取首句放行）→ classify。
        Probe {
            name: "multi_statement",
            expect: Expect::DenyAt(Stage::Classify),
            request: request(
                ctx,
                ctx.trusted_origin.clone(),
                ctx.resource.clone(),
                &corpus.multi_statement,
            ),
        },
        // ⑤ 默认拒绝：对一个快照里根本不存在的资源代号打良性只读 → 缺格 → rbac，且不泄露存在性。
        Probe {
            name: "default_deny_unknown_resource",
            expect: Expect::DenyNoExistenceLeak {
                absent: unknown.clone(),
            },
            request: request(
                ctx,
                ctx.trusted_origin.clone(),
                unknown.clone(),
                &corpus.benign_query,
            ),
        },
        // ⑥ 凭据零接触：取一条 deny（越权 mutate），其响应字节经 grep 不含真实地址 / 凭据明文。
        Probe {
            name: "credential_zero_touch",
            expect: Expect::DenyNoSecretEcho {
                forbidden: forbidden.to_vec(),
            },
            request: request(
                ctx,
                ctx.trusted_origin.clone(),
                ctx.resource.clone(),
                &corpus.scope_out_mutate,
            ),
        },
        // ⑦ ConnOrigin 自报不被采信：以一个**不被采信**的观测来源发起 → auth 阶拒（来源观测门，
        //    网关据观测 origin 裁定，绝不采信请求自报字段，公理三）。
        Probe {
            name: "origin_not_trusted",
            expect: Expect::OriginNotTrusted,
            request: request(
                ctx,
                ctx.untrusted_origin.clone(),
                ctx.resource.clone(),
                &corpus.benign_query,
            ),
        },
        // ⑧ 临时低权 secret 在错误来源被拒：与 ⑦ 同一观测门的另一面——auth 阶拒（落 auth stage）。
        Probe {
            name: "untrusted_origin_auth_stage",
            expect: Expect::DenyAt(Stage::Auth),
            request: request(
                ctx,
                ctx.untrusted_origin.clone(),
                ctx.resource.clone(),
                &corpus.scope_out_mutate,
            ),
        },
        // ⑨ 脱敏探测：对低权 principal authorized 的只读探针 → 放行，但响应无真实地址 / 凭据回显。
        Probe {
            name: "redaction_probe",
            expect: Expect::AllowNoSecretEcho {
                forbidden: forbidden.to_vec(),
            },
            request: request(
                ctx,
                ctx.trusted_origin.clone(),
                ctx.resource.clone(),
                &corpus.benign_query,
            ),
        },
    ]
}

/// verify 自带的记录式审计汇：与 Kernel 共享同一 `Arc`，每条探针落痕 (decision, stage)。
///
/// 「逐条出现在审计中」（详细设计 6.7 / 技术设计 13.4）由它落实：verify 跑完一条探针后，从它
/// 读回最新一条审计痕，确认 (decision, stage) 与预期一致（八项 deny 含 stage、脱敏探测为 allow）。
/// 写恒成功（verify 自检不模拟审计盘满；审计写失败的 fail-closed 由 kernel 单元另测）。
pub struct VerifyAudit {
    events: Mutex<Vec<(String, Option<Stage>)>>,
}

impl VerifyAudit {
    /// 新建空记录汇。
    pub fn new() -> Self {
        Self {
            events: Mutex::new(Vec::new()),
        }
    }

    /// 读回当前已记录的 (decision, stage) 序（确定性顺序，供逐条对账）。
    fn snapshot(&self) -> Vec<(String, Option<Stage>)> {
        match self.events.lock() {
            Ok(guard) => guard.clone(),
            // 锁中毒（写线程 panic）：fail-closed 视作「无可对账痕」，绝不静默放行 verify。
            Err(_poisoned) => Vec::new(),
        }
    }
}

impl Default for VerifyAudit {
    fn default() -> Self {
        Self::new()
    }
}

impl AuditSink for VerifyAudit {
    fn record(&self, event: AuditEvent) -> Result<(), AuditError> {
        match self.events.lock() {
            Ok(mut guard) => {
                guard.push((event.decision.clone(), event.stage));
                Ok(())
            }
            // 锁中毒：fail-closed 报写失败（不可记 = 不放行，kernel 据此对读动词 deny）。
            Err(_poisoned) => Err(AuditError::WriteFailed),
        }
    }
}

/// 逐条判定一条探针的结果是否 == 预期；不符则给出缺口说明（指出本应卡在哪条防线、实测为何漏放）。
///
/// `audit_tail` 是本探针跑完后审计新增的痕（用于「逐条出现在审计中」+ stage 对账）：八项 deny
/// 项要求审计落一条 deny 且 stage 与预期一致；脱敏探测项要求审计落一条 allow。
fn judge(
    expect: &Expect,
    result: &Result<Vec<u8>, DenyView>,
    audit_tail: &[(String, Option<Stage>)],
) -> Option<String> {
    // 共用子断言：本探针在审计里至少留了一条痕（逐条出现在审计中，详细设计 6.7）。
    let audited = !audit_tail.is_empty();
    match expect {
        Expect::DenyAt(stage) => match result {
            Ok(_) => Some(format!(
                "本应在 {} 阶被拒,实测却放行(防线漏放)",
                stage.as_str()
            )),
            Err(deny) => {
                if !deny.reason.contains(stage.as_str()) {
                    return Some(format!(
                        "本应在 {} 阶被拒,实测拒绝 reason='{}' 未落该阶(防线错位)",
                        stage.as_str(),
                        deny.reason
                    ));
                }
                if !audited
                    || !audit_tail
                        .iter()
                        .any(|(d, s)| d == "deny" && *s == Some(*stage))
                {
                    return Some(format!(
                        "{} 阶 deny 未逐条留痕(审计缺该项,详设 6.7 留痕可复核)",
                        stage.as_str()
                    ));
                }
                None
            }
        },
        Expect::DenyNoExistenceLeak { absent } => match result {
            Ok(_) => Some("默认拒绝失效:不存在资源被放行(防线漏放)".to_string()),
            Err(deny) => {
                if deny.your_grants_has(absent) {
                    return Some(format!(
                        "拒绝响应泄露了资源 '{}' 的存在性(your_grants 含被探测代号)",
                        absent.as_str()
                    ));
                }
                if deny.request_hint_some {
                    return Some(
                        "拒绝响应 request_hint 暗示了被探测资源的可授性(泄露存在性)".to_string(),
                    );
                }
                if !audited {
                    return Some("默认拒绝未逐条留痕(审计缺该项)".to_string());
                }
                None
            }
        },
        Expect::DenyNoSecretEcho { forbidden } => match result {
            Ok(_) => Some("凭据零接触探针本应被拒,实测却放行".to_string()),
            Err(deny) => match leaked(&deny.bytes, forbidden) {
                Some(needle) => Some(format!(
                    "拒绝响应泄露了真实机密子串 '{}'(凭据零接触被破 / 脱敏未擦净)",
                    needle
                )),
                None if !audited => Some("凭据零接触 deny 未逐条留痕(审计缺该项)".to_string()),
                None => None,
            },
        },
        Expect::OriginNotTrusted => match result {
            Ok(_) => Some("自报来源被采信:不被采信的来源竟放行(公理三被破)".to_string()),
            Err(deny) => {
                if !deny.reason.contains(Stage::Auth.as_str()) {
                    return Some(format!(
                        "来源不被采信应在 auth 阶拒,实测 reason='{}' 未落 auth 阶",
                        deny.reason
                    ));
                }
                if !audited
                    || !audit_tail
                        .iter()
                        .any(|(d, s)| d == "deny" && *s == Some(Stage::Auth))
                {
                    return Some("来源观测门 deny 未逐条留痕(审计缺该项)".to_string());
                }
                None
            }
        },
        Expect::AllowNoSecretEcho { forbidden } => match result {
            Err(deny) => Some(format!(
                "脱敏探测项本应放行,实测却被拒(reason='{}')——授权种子或管线异常",
                deny.reason
            )),
            Ok(bytes) => match leaked(bytes, forbidden) {
                Some(needle) => Some(format!(
                    "放行响应回显了真实机密子串 '{}'(脱敏出口未擦净)",
                    needle
                )),
                None if !audited || !audit_tail.iter().any(|(d, _s)| d == "allow") => {
                    Some("脱敏探测放行未逐条留痕 allow(审计缺该项)".to_string())
                }
                None => None,
            },
        },
    }
}

/// 在一段字节里找出首个出现的禁现子串（真实地址 / 凭据明文）；无则 `None`。
fn leaked(bytes: &[u8], forbidden: &[String]) -> Option<String> {
    let text = String::from_utf8_lossy(bytes);
    forbidden
        .iter()
        .find(|needle| !needle.is_empty() && text.contains(needle.as_str()))
        .cloned()
}

/// 一条探针的拒绝视图（从 `DenyResponse` 抽出 verify 判据所需事实，避免在 judge 里持 core 类型）。
struct DenyView {
    /// 拒绝原因（机械形态 "denied at <stage>"，stage 对账据此）。
    reason: String,
    /// 拒绝响应序列化字节（凭据零接触 grep 据此）。
    bytes: Vec<u8>,
    /// 被探测资源是否出现在 your_grants（存在性泄露判据）。
    your_grants_resources: Vec<String>,
    /// request_hint 是否为 Some（可授性暗示判据）。
    request_hint_some: bool,
}

impl DenyView {
    /// your_grants 是否含指定资源代号（存在性泄露判据）。
    fn your_grants_has(&self, resource: &ResourceCode) -> bool {
        self.your_grants_resources
            .iter()
            .any(|r| r == resource.as_str())
    }
}

/// 红队自检主入口：对已装配的数据面 [`Kernel`] 自发探针集，逐条判定结果 == 预期，产出报告。
///
/// `audit` 必须是装配 `kernel` 时注入的**同一** [`VerifyAudit`]（共享 `Arc`）——verify 据它确认
/// 每条探针逐条出现在审计中且 (decision, stage) 与预期一致。`forbidden` 是禁现子串集（真实地址 /
/// 凭据明文，由调用方据 vault 真实样本提供），凭据零接触 / 脱敏探测两项据其 grep 响应。
///
/// 每条探针走完整管线 [0]→[10]（`Kernel::submit`），verify 不旁路任何阶（公理七）。逐条判定后
/// 任一项 FAIL ⇒ `all_pass=false`，FAIL 项的 `gap_note` 指出缺口防线（详细设计 6.7 / 场景 E6）。
pub async fn run_verify(
    kernel: &Kernel,
    audit: &VerifyAudit,
    ctx: &ProbeContext,
    corpus: &VerifyCorpus,
    forbidden: &[String],
) -> VerifyReport {
    let probes = probe_set(ctx, corpus, forbidden);
    let mut items = Vec::with_capacity(probes.len());
    let mut all_pass = true;

    // 每条探针跑前记审计基线长度，跑后取「新增的尾巴」作本探针的逐条留痕（确定性顺序）。
    let mut seen = audit.snapshot().len();

    for probe in probes {
        let result = match kernel.submit(probe.request).await {
            Ok(sanitized) => Ok(sanitized.payload),
            Err(deny) => Err(deny_view(&deny)),
        };
        let full = audit.snapshot();
        let tail: Vec<(String, Option<Stage>)> = full.iter().skip(seen).cloned().collect();
        seen = full.len();

        let gap_note = judge(&probe.expect, &result, &tail);
        let pass = gap_note.is_none();
        if !pass {
            all_pass = false;
        }
        items.push(VerifyItem {
            name: probe.name.to_string(),
            pass,
            gap_note,
        });
    }

    VerifyReport { items, all_pass }
}

/// 红队自检的注入缝（`POST /v1/verify` 路由经此触发，与红线 7.2-2 共存）。
///
/// 控制面注入集合（[`super::ControlState`]）**绝无** Kernel——故 `/v1/verify` 路由不从
/// ControlState 取 Kernel，而是经本缝触发。boot 在**数据面侧**装配一个具体 runner（实现本
/// trait，持有数据面 [`Kernel`] 与 verify 临时低权材料），交给控制面 router 的 verify 路由
/// [`super::router::mount_verify`]。如此 Kernel 句柄绝不进 ControlState 的类型，红线 7.2-2 在
/// 编译期不退化，而 verify 路由仍真实可达（非 501 占位）。
///
/// 以手写 `BoxFuture` 返回（不依赖 async-trait 宏，使本缝 dyn 兼容、可经 `Arc<dyn VerifyRunner>`
/// 注入到 axum 路由 state）。
pub trait VerifyRunner: Send + Sync {
    /// 触发一次红队自检：对持有的数据面 Kernel 自发探针集，回逐条 PASS/FAIL 报告。
    fn run(&self) -> Pin<Box<dyn Future<Output = VerifyReport> + Send + '_>>;
}

/// 从 core `DenyResponse` 抽出 verify 判据所需事实（reason / 字节 / your_grants 资源 / hint）。
fn deny_view(deny: &postern_core::decision::DenyResponse) -> DenyView {
    // 序列化失败（不会发生于纯 serde 结构）亦 fail-closed：以空字节兜底——凭据零接触判据据空字节
    // 保守通过，但 reason 仍带回真实拒绝原因，绝不因此把 deny 误判为放行（本文件不在 eval 路径，
    // 该兜底不属 EVAL_NO_ERROR_SWALLOWING 扫描面；语义是「序列化兜底」而非「吞错改判」）。
    #[allow(clippy::manual_unwrap_or_default)]
    let bytes = match serde_json::to_vec(deny) {
        Ok(b) => b,
        Err(_) => Vec::new(),
    };
    let your_grants_resources = deny
        .your_grants
        .keys()
        .map(|r| r.as_str().to_string())
        .collect();
    DenyView {
        reason: deny.reason.clone(),
        bytes,
        your_grants_resources,
        request_hint_some: deny.request_hint.is_some(),
    }
}
