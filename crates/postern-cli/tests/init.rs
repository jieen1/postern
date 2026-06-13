//! 接入向导编排（`init`）的行为测试（RED）。
//!
//! 被测对象：`postern_cli::init::wizard`（呈现—圈选—回写状态机：`run_wizard` + `ControlPlane`
//! trait + `DiscoveryReport`/`Candidate`/`Gap`/`Selection`/`WizardTrace` 等只读视图）与
//! `postern_cli::init::claude_md`（`render_claude_md` + `AuthorizationFacts`，纯文本渲染）。
//!
//! 测试策略（07-postern-cli §3.7/§4/§8/§9）：对**内存 Fake 控制面**驱动向导——Fake 记录每步
//! 经控制面发出的请求规格、并回放注入的候选 / 缺口；无需真实 daemon。每条只钉一个行为，断言
//! 精确到具体值 / 变体 / 端点形态；失败路径一等公民（控制面失败 → fail-closed 上抛、不补偿）。
//!
//! 覆盖 §8 条目（逐条 `// §8` 注释）：
//!   - F-8：`init` 主线控制面调用序列 = 建资源 `POST /v1/resources` → 触发
//!     `POST /v1/resources/{code}/discover` → 回写（对 docs/examples/02 §4.1 步骤 1→8）；候选 /
//!     缺口取自 Fake；出现 CLI 自探测 / 自行 tier 校验调用即不过。
//!   - F-9：喂授权事实 {端点=`data.sock` 的 `/mcp`、已授权动词集=∅} → 片段如实写"暂无已授权
//!     动词"、不含编造话术；喂非空动词集 → 片段恰列该集合；输入之外任何固定话术即不过（语义
//!     部分 → L-6）。
//!   - L-6（机器部分）：空集片段不含输入授权事实集之外的固定散文模板（构造签名）。
//!   - L-8：注入缺口（端口不可达 E3 / tier 名实不符 E5 / 2FA 须人在场 E2）→ 向导呈现该缺口、
//!     "代修"仅转译为后续控制面写调用、不自行判定缺口消解；`fix=None`（2FA）缺口不发任何写。
//!
//! 雷区（本测试遵守）：不构造任何机密族类型（凭据 / 解析目标 / 已呈递凭据 / 擦净集句柄）；
//! 不嵌裸数据库写标记；不写连接来源枚举的构造字面；不直连资源、不在 CLI 跑能力探测；
//! 候选 / 缺口 / 版本全由 Fake 回报、测试不自造任何判定。

use std::collections::BTreeMap;

use postern_cli::error::CliError;
use postern_cli::init::claude_md::{render_claude_md, AuthorizationFacts};
use postern_cli::init::wizard::{
    run_wizard, Candidate, ControlPlane, DiscoveryReport, Gap, RepairSpec, Selection,
};
use postern_cli::reqspec::{Method, RequestSpec};

// ════════════════════════════════════════════════════════════════════════════
// 测试夹具：内存 Fake 控制面
//
// 记录每步经控制面发出的请求规格（供序列断言），并据调用序号回放注入响应字节——
// 第 0 次（建资源）回放 ResourceHandle JSON、第 1 次（触发探测）回放 DiscoveryReport JSON，
// 其后（回写 / 代修）回放空 ack。向导经它**只能**发起调用、拿回字节——无任何自探测路径。
// ════════════════════════════════════════════════════════════════════════════

struct FakeControlPlane {
    /// 据调用序号取的回放响应字节队列（VecDeque 语义，按 pop_front 顺序回放）。
    scripted: Vec<Vec<u8>>,
    /// 已发出的请求规格按序记录——序列断言的唯一事实来源。
    recorded: Vec<RequestSpec>,
}

impl FakeControlPlane {
    fn new(scripted: Vec<Vec<u8>>) -> Self {
        FakeControlPlane {
            scripted,
            recorded: Vec::new(),
        }
    }
}

impl ControlPlane for FakeControlPlane {
    fn call(&mut self, spec: &RequestSpec) -> Result<Vec<u8>, CliError> {
        self.recorded.push(spec.clone());
        let idx = self.recorded.len() - 1;
        match self.scripted.get(idx) {
            Some(bytes) => Ok(bytes.clone()),
            // 脚本耗尽 = 向导发起了超出脚本预期的调用：fail-closed 上抛，使任何"多发一步"
            // 的回归都暴露为 DaemonUnreachable，而非静默成功。
            None => Err(CliError::DaemonUnreachable),
        }
    }
}

/// 建资源应答字节（ResourceHandle JSON）：daemon 回报 `code` + 乐观锁基线 `version`。
fn resource_handle_bytes(code: &str, version: u64) -> Vec<u8> {
    format!(r#"{{"code":"{code}","version":{version}}}"#).into_bytes()
}

/// 探测应答字节（DiscoveryReport JSON）：候选 + 缺口全由 daemon 回报；测试经此注入。
fn discovery_bytes(report: &DiscoveryReport) -> Vec<u8> {
    serde_json::to_vec(report).expect("fixture DiscoveryReport must serialize")
}

/// 空 ack 字节——回写 / 代修写调用的 daemon 确认（向导不解析其语义，只视为该步成功）。
fn ack_bytes() -> Vec<u8> {
    b"{}".to_vec()
}

/// 一份"无候选无缺口"的干净探测报告（主线序列断言用，缺口注入另置专门用例）。
fn empty_report() -> DiscoveryReport {
    DiscoveryReport {
        candidates: Vec::new(),
        gaps: Vec::new(),
    }
}

// ════════════════════════════════════════════════════════════════════════════
// F-8 · 接入向导主线调用序列 = 建资源 → discover → 回写
// （对 docs/examples/02 §4.1 步骤 1→8；候选 / 缺口取自 Fake；CLI 仅发起与呈现）
// ════════════════════════════════════════════════════════════════════════════

// §8 F-8：第 0 步必为建资源 `POST /v1/resources`（步骤 1）。钉方法 + 路径精确。
#[test]
fn wizard_first_call_is_create_resource_post_v1_resources() {
    let mut fake = FakeControlPlane::new(vec![
        resource_handle_bytes("db-main", 1),
        discovery_bytes(&empty_report()),
    ]);

    let _ = run_wizard(&mut fake, "db-main", "postgres", "ssm", |_report| {
        Selection::default()
    });

    let first = fake.recorded.first().expect("向导必须先发一次建资源调用");
    assert_eq!(
        first.method,
        Method::Post,
        "步骤 1 建资源必须是 POST，实得 {:?}",
        first.method
    );
    assert_eq!(
        first.path_template, "/v1/resources",
        "步骤 1 建资源必须落 6.5 的 /v1/resources，实得 {:?}",
        first.path_template
    );
}

// §8 F-8：第 1 步必为触发探测 `POST /v1/resources/{code}/discover`（步骤 6），且 `{code}`
// **取自上一步建资源应答回报的 code（搬运，非自造）**。钉方法 + 完整路径（含 code 填充）。
//
// 差分夹具（trace-3）：输入 codename（`input-codename`）**故意不等于** daemon 回报的 code
// （`daemon-code`）——故 discover 路径必含 daemon 回报的 `daemon-code`、且**不得**含输入
// codename。一个把 discover 装配成 `discover_spec(codename)`（自造、取输入而非应答回报）的
// 回归会被本断言捕获；旧夹具里二者同值，对"搬运 vs 自造"零分辨力。
#[test]
fn wizard_second_call_is_discover_on_returned_code() {
    // daemon 回报的 code 与输入 codename 不同 → 唯有取应答回报的 code 才能让路径对上。
    let mut fake = FakeControlPlane::new(vec![
        resource_handle_bytes("daemon-code", 1),
        discovery_bytes(&empty_report()),
    ]);

    let _ = run_wizard(&mut fake, "input-codename", "postgres", "ssm", |_report| {
        Selection::default()
    });

    let second = fake
        .recorded
        .get(1)
        .expect("向导第 2 步必须触发探测 discover");
    assert_eq!(
        second.method,
        Method::Post,
        "触发探测必须是 POST，实得 {:?}",
        second.method
    );
    assert_eq!(
        second.path_template, "/v1/resources/daemon-code/discover",
        "触发探测路径的 {{code}} 必须搬运自上一步建资源应答回报的 code（daemon-code），\
         非自造取输入 codename（input-codename），且为控制面 discover 端点，实得 {:?}",
        second.path_template
    );
    assert!(
        !second.path_template.contains("input-codename"),
        "discover 路径绝不得含输入 codename（那是自造、非搬运应答 code），实得 {:?}",
        second.path_template
    );
}

// §8 F-8（步骤序：建资源 → discover 之间无任何中间调用）：前两步严格相邻、且建资源在前。
// 守卫"先建后探"这一纯流程顺序——若向导在二者之间插入任何自探测调用，本断言 FAIL。
#[test]
fn wizard_create_precedes_discover_with_nothing_in_between() {
    let mut fake = FakeControlPlane::new(vec![
        resource_handle_bytes("svc-order", 7),
        discovery_bytes(&empty_report()),
    ]);

    let _ = run_wizard(&mut fake, "svc-order", "http", "ssh", |_report| {
        Selection::default()
    });

    assert_eq!(
        fake.recorded[0].path_template, "/v1/resources",
        "第 0 步必为建资源"
    );
    assert_eq!(
        fake.recorded[1].path_template, "/v1/resources/svc-order/discover",
        "第 1 步必紧接为 discover，二者之间不得有任何中间调用"
    );
}

// §8 F-8：无圈选（候选 / 缺口皆空、Selection 默认）→ 主线恰两次调用（建资源 + discover），
// **无**任何回写。守卫"无选择即不写"——CLI 不替人凭空发回写。
#[test]
fn wizard_with_no_selection_emits_exactly_create_and_discover() {
    let mut fake = FakeControlPlane::new(vec![
        resource_handle_bytes("db-main", 1),
        discovery_bytes(&empty_report()),
    ]);

    let _ = run_wizard(&mut fake, "db-main", "postgres", "ssm", |_report| {
        Selection::default()
    });

    assert_eq!(
        fake.recorded.len(),
        2,
        "无圈选时主线恰两次控制面调用（建资源 + discover），实得 {} 次：{:?}",
        fake.recorded.len(),
        fake.recorded
            .iter()
            .map(|s| s.path_template.as_str())
            .collect::<Vec<_>>()
    );
}

// §8 F-8（步骤 4 回写：选中**带 fix 的缺口** → 发其 daemon 回报的写）：人选中一条携 fix
// （控制面写）的缺口 → 向导把该缺口转译为 daemon 回报里**已给出的**后续控制面写调用并发出，
// 发生在 discover 之后。这是"建资源 → discover → 回写"的回写段：钉第 2 步为该缺口 fix 的
// 方法 / 路径 / 体字段（原样取自 daemon 回报、非自造）。
//
// 注（trace-4）：回写**由 daemon 回报的缺口 fix 驱动**——本用例不再宣称"由 chosen_candidates
// 驱动"（候选侧 `Candidate` 只有 key/label、不携任何 daemon 给出的写规格，CLI 据裸候选自造写
// 即违反 L-8）。候选是否非空对本写零影响，故本用例 chosen_candidates 留空；候选→写回的
// "不自造"差分另由 `wizard_does_not_fabricate_write_from_chosen_candidate_without_fix` 钉死。
#[test]
fn wizard_forwards_daemon_given_gap_fix_as_followup_write_call() {
    let mut tier_fields = BTreeMap::new();
    tier_fields.insert("tier".to_string(), "ro".to_string());
    // daemon 在缺口侧给出的回写规格：把该对象纳入 query 细则的控制面写。向导只原样发出。
    let gap_fix = RepairSpec {
        method: "POST".to_string(),
        path: "/v1/resources/db-main/constraints".to_string(),
        fields: tier_fields,
    };
    // 探测报告：一条携 fix（控制面写）的缺口。选中缺口 `bind-orders` 即把其 daemon 回报的
    // fix 转译为后续控制面回写调用。候选仅作呈现、不驱动任何写（见下方差分用例）。
    let report = DiscoveryReport {
        candidates: vec![Candidate {
            key: "orders-table".to_string(),
            label: "public.orders".to_string(),
        }],
        gaps: vec![Gap {
            key: "bind-orders".to_string(),
            detail: "daemon-given write-back".to_string(),
            fix: Some(gap_fix),
        }],
    };

    let mut fake = FakeControlPlane::new(vec![
        resource_handle_bytes("db-main", 1),
        discovery_bytes(&report),
        ack_bytes(),
    ]);

    // chosen_candidates 留空：证明写由缺口 fix 驱动、与候选选择无关。
    let _ = run_wizard(&mut fake, "db-main", "postgres", "ssm", |_report| {
        Selection {
            chosen_candidates: Vec::new(),
            repair_gaps: vec!["bind-orders".to_string()],
        }
    });

    let writeback = fake
        .recorded
        .get(2)
        .expect("选中带 fix 的缺口后必须发出第 3 步回写调用（discover 之后）");
    assert_eq!(
        writeback.method,
        Method::Post,
        "回写方法必须取自 daemon 回报的 fix（POST），实得 {:?}",
        writeback.method
    );
    assert_eq!(
        writeback.path_template, "/v1/resources/db-main/constraints",
        "回写路径必须取自 daemon 回报的 fix，向导不自造，实得 {:?}",
        writeback.path_template
    );
    let body = writeback.body.as_ref().expect("回写是写端点，必须有请求体");
    assert_eq!(
        body.fields.get("tier").map(String::as_str),
        Some("ro"),
        "回写体字段必须原样取自 daemon 回报的 fix，向导不增删，实得 {:?}",
        body.fields
    );
}

// §8 F-8/L-8（候选→写回的"不自造"差分守卫，对 trace-4/failclosed-5）：人**圈选了一个候选**，
// 但该候选侧 daemon **未给出**任何写规格（`Candidate` 只有 key/label，且无对应的带 fix 缺口）
// → 向导**绝不**据裸候选自造任何后续控制面写调用（自造 daemon 未列出的调用即违反 L-8）。
// 守卫"圈选候选本身不产生写、CLI 不替 daemon 凭空发候选回写"——主线恰建资源 + discover 两次
// 调用，无第三次写。一个据裸 chosen_candidates 自造写的回归会被本断言（恰 2 次调用）捕获。
#[test]
fn wizard_does_not_fabricate_write_from_chosen_candidate_without_fix() {
    // 报告含候选、但**无任何缺口**（故无 daemon 给出的写规格可转译）。
    let report = DiscoveryReport {
        candidates: vec![Candidate {
            key: "orders-table".to_string(),
            label: "public.orders".to_string(),
        }],
        gaps: Vec::new(),
    };
    // 给足第三项 ack：若实现据裸候选自造写并发出，会消费此 ack（recorded 长度变 3）——本用例
    // 据此区分"不自造（恰 2 次调用）"与"据候选自造写（3 次调用）"。
    let mut fake = FakeControlPlane::new(vec![
        resource_handle_bytes("db-main", 1),
        discovery_bytes(&report),
        ack_bytes(),
    ]);

    let _ = run_wizard(&mut fake, "db-main", "postgres", "ssm", |_report| {
        Selection {
            // 人圈选了该候选，但它无 daemon 给出的写规格 → 不得据此自造任何写。
            chosen_candidates: vec!["orders-table".to_string()],
            repair_gaps: Vec::new(),
        }
    });

    assert_eq!(
        fake.recorded.len(),
        2,
        "圈选一个无 daemon 写规格的候选不得产生任何控制面写（CLI 不据裸候选自造写，L-8），\
         主线应恰建资源 + discover 两次调用，实得 {} 次：{:?}",
        fake.recorded.len(),
        fake.recorded
            .iter()
            .map(|s| s.path_template.as_str())
            .collect::<Vec<_>>()
    );
}

// §8 F-8（CLI 仅发起与呈现 / 不自探测）：向导拿回的探测报告原样出现在 WizardTrace.report；
// 候选 / 缺口逐条来自 Fake 回报。守卫"候选 / 缺口取自 Fake"——向导不自造、不丢字段。
#[test]
fn wizard_trace_carries_daemon_reported_candidates_verbatim() {
    let report = DiscoveryReport {
        candidates: vec![
            Candidate {
                key: "orders".to_string(),
                label: "public.orders".to_string(),
            },
            Candidate {
                key: "items".to_string(),
                label: "public.items".to_string(),
            },
        ],
        gaps: Vec::new(),
    };
    let mut fake = FakeControlPlane::new(vec![
        resource_handle_bytes("db-main", 1),
        discovery_bytes(&report),
    ]);

    let trace = run_wizard(&mut fake, "db-main", "postgres", "ssm", |_report| {
        Selection::default()
    })
    .expect("无缺口主线应成功");

    assert_eq!(
        trace.report.candidates, report.candidates,
        "向导呈现的候选必须与 daemon 回报逐条相等（不自造、不丢字段）"
    );
}

// §8 F-8（失败路径：建资源失败 → fail-closed，不继续探测）：第 0 步控制面失败 → 向导上抛
// 错误、且**不**发出 discover（脚本里只放一项即耗尽，第二次调用会撞空脚本）。守卫"半截不
// 推进"——建资源没成不能去探测。
#[test]
fn wizard_aborts_without_discover_when_create_resource_fails() {
    // 脚本为空 → 第一次 call（建资源）即撞空脚本、Fake 回 DaemonUnreachable。
    let mut fake = FakeControlPlane::new(Vec::new());

    let outcome = run_wizard(&mut fake, "db-main", "postgres", "ssm", |_report| {
        Selection::default()
    });

    assert!(
        matches!(outcome, Err(CliError::DaemonUnreachable)),
        "建资源失败必须 fail-closed 上抛，实得 {:?}",
        outcome
    );
    assert_eq!(
        fake.recorded.len(),
        1,
        "建资源失败后不得继续触发 discover（半截不推进），实得调用数 {}",
        fake.recorded.len()
    );
}

// §8 F-8（失败路径：discover 失败 → fail-closed，不半截推进）：第 0 步建资源成功、第 1 步
// discover 控制面失败（脚本只放建资源应答一项，第 2 次 call 撞空脚本 → DaemonUnreachable）。
// 守卫"discover 失败即上抛、不吞错、不继续回写"——一个在 discover 失败后仍半截推进 / 吞错
// 的回归会被本断言（错误类型 + 恰 2 次调用、无第三次写）捕获。
#[test]
fn wizard_aborts_when_discover_call_fails() {
    // 脚本只放建资源应答 → 第 0 次 call（建资源）成功，第 1 次 call（discover）撞空脚本失败。
    let mut fake = FakeControlPlane::new(vec![resource_handle_bytes("db-main", 1)]);

    let outcome = run_wizard(&mut fake, "db-main", "postgres", "ssm", |_report| {
        Selection::default()
    });

    assert!(
        matches!(outcome, Err(CliError::DaemonUnreachable)),
        "discover 控制面失败必须 fail-closed 上抛（不吞错、不补默认报告），实得 {:?}",
        outcome
    );
    assert_eq!(
        fake.recorded.len(),
        2,
        "discover 失败时向导恰发过建资源 + discover 两次调用、不再继续回写（半截不推进），\
         实得调用数 {}",
        fake.recorded.len()
    );
}

// §8 F-8/L-3（解析失败路径：建资源应答字节不符共享类型契约 → DecodeFailed，不继续探测）：
// 第 0 步 daemon 回了畸形 / 缺字段字节（这里缺 `code`/`version` 字段），向导解码 ResourceHandle
// 失败即 fail-closed 报 DecodeFailed、绝不补默认 / 当成功 → **不**发 discover。守卫"解码失败
// 不静默吞成空、不半截推进"——一个把 decode 失败当成功继续的回归会被本断言捕获。
#[test]
fn wizard_fails_closed_when_resource_handle_bytes_are_malformed() {
    // 缺 `code`/`version` 的畸形 JSON：合法 JSON 对象但不符 ResourceHandle 契约 → DecodeFailed。
    let malformed_handle = br#"{"unexpected":"field"}"#.to_vec();
    let mut fake = FakeControlPlane::new(vec![malformed_handle, discovery_bytes(&empty_report())]);

    let outcome = run_wizard(&mut fake, "db-main", "postgres", "ssm", |_report| {
        Selection::default()
    });

    assert!(
        matches!(outcome, Err(CliError::DecodeFailed { .. })),
        "建资源应答畸形必须 fail-closed 报 DecodeFailed（不补默认、不当成功），实得 {:?}",
        outcome
    );
    assert_eq!(
        fake.recorded.len(),
        1,
        "建资源应答解码失败后不得继续触发 discover（半截不推进），实得调用数 {}",
        fake.recorded.len()
    );
}

// §8 F-8/L-3（解析失败路径：discover 应答字节不符共享类型契约 → DecodeFailed，不继续回写）：
// 第 1 步 daemon 回了畸形字节（这里 `gaps` 字段类型错——给成字符串而非数组），向导解码
// DiscoveryReport 失败即 fail-closed 报 DecodeFailed、绝不补空报告 / 当成功 → **不**发任何回写。
// 守卫"解码失败不静默吞成空报告、不半截推进"——一个把 decode 失败吞成空 DiscoveryReport
// 继续编排的回归会被本断言捕获。
#[test]
fn wizard_fails_closed_when_discovery_bytes_are_malformed() {
    // `gaps` 类型错（字符串而非数组）：合法 JSON 但不符 DiscoveryReport 契约 → DecodeFailed。
    let malformed_report = br#"{"candidates":[],"gaps":"not-an-array"}"#.to_vec();
    let mut fake = FakeControlPlane::new(vec![
        resource_handle_bytes("db-main", 1),
        malformed_report,
        ack_bytes(),
    ]);

    let outcome = run_wizard(&mut fake, "db-main", "postgres", "ssm", |_report| {
        Selection {
            chosen_candidates: Vec::new(),
            repair_gaps: vec!["whatever".to_string()],
        }
    });

    assert!(
        matches!(outcome, Err(CliError::DecodeFailed { .. })),
        "discover 应答畸形必须 fail-closed 报 DecodeFailed（不补空报告、不当成功），实得 {:?}",
        outcome
    );
    assert_eq!(
        fake.recorded.len(),
        2,
        "discover 应答解码失败后不得继续发任何回写（半截不推进），实得调用数 {}",
        fake.recorded.len()
    );
}

// §8 L-7/L-8（代修写失败路径：写失败即 fail-closed 上抛、不本地补偿）：人选中带 fix 的缺口，
// 向导发出代修写（第 2 次 call），但该写控制面失败（脚本只放建资源 + discover 两项，第 3 次
// call 撞空脚本 → DaemonUnreachable）。守卫"代修写失败即上抛、不补写 / 不回滚 / 不重试 /
// 不再发后续补偿调用"——一个在代修写失败后做本地重试 / 补偿的回归会被本断言（错误类型 +
// 恰 3 次调用：建资源 + discover + 这一次失败的代修写，无第 4 次补偿）捕获。
#[test]
fn wizard_fails_closed_when_repair_write_call_fails() {
    let mut fix_fields = BTreeMap::new();
    fix_fields.insert("tier".to_string(), "ro".to_string());
    let gap = Gap {
        key: "tier-mismatch".to_string(),
        detail: "declared ro includes mutate but account has no write privilege".to_string(),
        fix: Some(RepairSpec {
            method: "PUT".to_string(),
            path: "/v1/resources/db-main/constraints".to_string(),
            fields: fix_fields,
        }),
    };
    let report = DiscoveryReport {
        candidates: Vec::new(),
        gaps: vec![gap],
    };
    // 脚本只放建资源 + discover 两项 → 第 2 次 call（代修写）撞空脚本失败。
    let mut fake = FakeControlPlane::new(vec![
        resource_handle_bytes("db-main", 1),
        discovery_bytes(&report),
    ]);

    let outcome = run_wizard(&mut fake, "db-main", "postgres", "ssm", |_report| {
        Selection {
            chosen_candidates: Vec::new(),
            repair_gaps: vec!["tier-mismatch".to_string()],
        }
    });

    assert!(
        matches!(outcome, Err(CliError::DaemonUnreachable)),
        "代修写控制面失败必须 fail-closed 上抛（不补写、不回滚、不重试），实得 {:?}",
        outcome
    );
    assert_eq!(
        fake.recorded.len(),
        3,
        "代修写失败后不得再发任何后续补偿 / 重试调用——恰发过建资源 + discover + 这一次失败的\
         代修写共 3 次，实得调用数 {}：{:?}",
        fake.recorded.len(),
        fake.recorded
            .iter()
            .map(|s| s.path_template.as_str())
            .collect::<Vec<_>>()
    );
}

// §8 L-3/L-8（代修方法文本非法 → fail-closed，不静默降级为默认方法）：daemon 回报的缺口 fix
// 携一个**不在封闭集**的方法文本（这里 `PATCH`），人选中代修 → 向导解析方法失败即 fail-closed
// 报 DecodeFailed、**绝不**把未知方法静默降级为默认 GET 后照发。守卫"未知方法即不过"——一个把
// 非法方法降级为某默认方法并发出写的回归会被本断言（错误类型 + 不发该写：恰 2 次调用）捕获。
#[test]
fn wizard_fails_closed_when_repair_method_is_not_in_closed_set() {
    let mut fix_fields = BTreeMap::new();
    fix_fields.insert("tier".to_string(), "ro".to_string());
    let gap = Gap {
        key: "tier-mismatch".to_string(),
        detail: "declared ro includes mutate but account has no write privilege".to_string(),
        // 非法方法文本：不在 GET/POST/PUT/DELETE 封闭集 → 解析即 DecodeFailed，不降级默认。
        fix: Some(RepairSpec {
            method: "PATCH".to_string(),
            path: "/v1/resources/db-main/constraints".to_string(),
            fields: fix_fields,
        }),
    };
    let report = DiscoveryReport {
        candidates: Vec::new(),
        gaps: vec![gap],
    };
    // 给足 ack：若实现把非法方法静默降级为默认方法照发，会消费第 3 项 ack 而非报错——本用例
    // 据此区分"fail-closed 报错（不发写）"与"静默降级照发"。
    let mut fake = FakeControlPlane::new(vec![
        resource_handle_bytes("db-main", 1),
        discovery_bytes(&report),
        ack_bytes(),
    ]);

    let outcome = run_wizard(&mut fake, "db-main", "postgres", "ssm", |_report| {
        Selection {
            chosen_candidates: Vec::new(),
            repair_gaps: vec!["tier-mismatch".to_string()],
        }
    });

    assert!(
        matches!(outcome, Err(CliError::DecodeFailed { .. })),
        "代修 fix 携封闭集外方法文本必须 fail-closed 报 DecodeFailed（不静默降级为默认方法），\
         实得 {:?}",
        outcome
    );
    assert_eq!(
        fake.recorded.len(),
        2,
        "非法方法不得被降级照发——向导恰发过建资源 + discover 两次、不发该代修写，实得调用数 {}",
        fake.recorded.len()
    );
}

// ════════════════════════════════════════════════════════════════════════════
// L-8 · 接入缺口呈现即停、不自行修补语义
// （注入 E3 端口不可达 / E5 tier 名实不符 / E2 2FA；CLI 呈现 + 仅转译为后续控制面写、
//  不自行判定缺口消解；无"声明 ⊆ 真实权限"比对、无探测逻辑）
// ════════════════════════════════════════════════════════════════════════════

// §8 L-8（E3 端口不可达，对 docs/examples/02 §4.2 E3）：注入该缺口 → 向导呈现该缺口
// （原样出现在 trace.report.gaps），不自行下任何"是否可用"结论。钉缺口被如实呈现。
#[test]
fn wizard_presents_port_unreachable_gap_verbatim() {
    let gap = Gap {
        key: "port-unreachable".to_string(),
        detail: "target port unreachable, forward likely not published".to_string(),
        fix: None,
    };
    let report = DiscoveryReport {
        candidates: Vec::new(),
        gaps: vec![gap.clone()],
    };
    let mut fake = FakeControlPlane::new(vec![
        resource_handle_bytes("svc-order", 1),
        discovery_bytes(&report),
    ]);

    let trace = run_wizard(&mut fake, "svc-order", "http", "ssh", |_report| {
        // 不选择代修该缺口（只呈现）。
        Selection::default()
    })
    .expect("呈现缺口本身不应使向导出错——它只转述事实");

    assert_eq!(
        trace.report.gaps,
        vec![gap],
        "E3 端口不可达缺口必须原样呈现（detail 逐字、CLI 不展开 / 不补全 / 不重写）"
    );
}

// §8 L-8（E2 2FA 须人在场，对 docs/examples/02 §4.2 E2）：`fix=None` 的缺口被呈现，但向导
// 对它**不发任何控制面写调用**（无客户端代修路径）→ 即便人"选中"它代修也不产生写。守卫
// "2FA 缺口无代修路径、不自行绕过"——主线恰建资源 + discover 两次调用，无第三次写。
#[test]
fn wizard_does_not_emit_write_for_twofa_gap_with_no_fix() {
    let gap = Gap {
        key: "twofa-needs-human".to_string(),
        detail: "resource enforces 2FA; a human must complete OTP in the control plane".to_string(),
        fix: None,
    };
    let report = DiscoveryReport {
        candidates: Vec::new(),
        gaps: vec![gap],
    };
    let mut fake = FakeControlPlane::new(vec![
        resource_handle_bytes("svc-order", 1),
        discovery_bytes(&report),
    ]);

    let _ = run_wizard(&mut fake, "svc-order", "http", "ssh", |_report| Selection {
        chosen_candidates: Vec::new(),
        // 人"选中"该 2FA 缺口代修，但它 fix=None → 无写调用路径。
        repair_gaps: vec!["twofa-needs-human".to_string()],
    });

    assert_eq!(
        fake.recorded.len(),
        2,
        "fix=None 的 2FA 缺口不得产生任何控制面写调用（不自行绕过 2FA），主线应恰 2 次调用，\
         实得 {} 次：{:?}",
        fake.recorded.len(),
        fake.recorded
            .iter()
            .map(|s| s.path_template.as_str())
            .collect::<Vec<_>>()
    );
}

// §8 L-8（E5 tier 名实不符，对 docs/examples/02 §4.2 E5）：注入带 `fix`（控制面写）的缺口，
// 人选中代修 → 向导**仅**把缺口转译为后续控制面写调用（发其 fix），方法 / 路径 / 体原样取自
// daemon 回报。守卫"代修 = 转译为控制面写、合法性裁决仍在 daemon"——向导不自比对、不自判
// 消解。钉代修写调用为 daemon 回报的 fix。
#[test]
fn wizard_repairs_tier_mismatch_only_by_forwarding_daemon_given_write() {
    let mut fix_fields = BTreeMap::new();
    fix_fields.insert("tier".to_string(), "ro".to_string());
    fix_fields.insert("capabilities".to_string(), "observe,query".to_string());
    let fix = RepairSpec {
        method: "PUT".to_string(),
        path: "/v1/resources/db-main/constraints".to_string(),
        fields: fix_fields.clone(),
    };
    let gap = Gap {
        key: "tier-mismatch".to_string(),
        detail: "declared ro includes mutate but account has no write privilege".to_string(),
        fix: Some(fix),
    };
    let report = DiscoveryReport {
        candidates: Vec::new(),
        gaps: vec![gap],
    };
    let mut fake = FakeControlPlane::new(vec![
        resource_handle_bytes("db-main", 3),
        discovery_bytes(&report),
        ack_bytes(),
    ]);

    let _ = run_wizard(&mut fake, "db-main", "postgres", "ssm", |_report| {
        Selection {
            chosen_candidates: Vec::new(),
            repair_gaps: vec!["tier-mismatch".to_string()],
        }
    });

    let repair = fake
        .recorded
        .get(2)
        .expect("选中代修后必须发出第 3 步代修写调用");
    assert_eq!(
        repair.method,
        Method::Put,
        "代修写方法必须取自 daemon 回报的 fix（PUT），向导不自造，实得 {:?}",
        repair.method
    );
    assert_eq!(
        repair.path_template, "/v1/resources/db-main/constraints",
        "代修写路径必须取自 daemon 回报的 fix，实得 {:?}",
        repair.path_template
    );
    let body = repair.body.as_ref().expect("代修是写端点，必须有体");
    assert_eq!(
        body.fields.get("capabilities").map(String::as_str),
        Some("observe,query"),
        "代修体字段必须原样取自 daemon 回报的 fix（向导不自比对 / 不重写声明），实得 {:?}",
        body.fields
    );
}

// §8 L-8（缺口未消解前不自标可用 / 不自判消解）：向导对缺口报告**不**返回任何"已消解 /
// 资源可用"的布尔判定——WizardTrace 不含此类字段，缺口原样留在 trace.report.gaps 即停。
// 守卫"CLI 不自行裁决缺口消解"——发完代修写后，向导仍把缺口如实留在报告里，由 daemon 在
// 后续调用裁决，绝不在客户端把它标成"已解决"。
#[test]
fn wizard_does_not_self_adjudicate_gap_resolution() {
    let mut fix_fields = BTreeMap::new();
    fix_fields.insert("forward".to_string(), "declared".to_string());
    let gap = Gap {
        key: "port-unreachable".to_string(),
        detail: "target port unreachable, forward likely not published".to_string(),
        fix: Some(RepairSpec {
            method: "PUT".to_string(),
            path: "/v1/resources/svc-order/constraints".to_string(),
            fields: fix_fields,
        }),
    };
    let report = DiscoveryReport {
        candidates: Vec::new(),
        gaps: vec![gap.clone()],
    };
    let mut fake = FakeControlPlane::new(vec![
        resource_handle_bytes("svc-order", 1),
        discovery_bytes(&report),
        ack_bytes(),
    ]);

    let trace = run_wizard(&mut fake, "svc-order", "http", "ssh", |_report| Selection {
        chosen_candidates: Vec::new(),
        repair_gaps: vec!["port-unreachable".to_string()],
    })
    .expect("发起代修写本身不应使向导出错");

    // 发完代修写后，缺口仍原样留在报告——向导没有把它"标记为已消解"（无该路径）。
    assert_eq!(
        trace.report.gaps,
        vec![gap],
        "向导发完代修写后仍把缺口如实留在报告里，不自行标记消解（裁决在 daemon，L-8）"
    );
}

// ════════════════════════════════════════════════════════════════════════════
// F-9 / L-6 · `CLAUDE.md` 片段只渲染控制面回报的授权事实，不编造话术
// （对 docs/examples/02 §4.1 步骤 8）
// ════════════════════════════════════════════════════════════════════════════

// §8 F-9：喂授权事实 {端点=`data.sock` 的 `/mcp`、已授权动词集=∅} → 片段如实呈现"暂无已
// 授权动词"。钉片段含端点位置事实、且含"暂无已授权动词"的如实表述。
#[test]
fn claude_md_empty_verb_set_states_no_authorized_verbs_yet() {
    let facts = AuthorizationFacts {
        mcp_endpoint: "data.sock /mcp".to_string(),
        verbs: Vec::new(),
    };

    let fragment = render_claude_md(&facts);

    assert!(
        fragment.contains("data.sock /mcp"),
        "片段必须含控制面回报的 MCP 端点位置事实，实得：{fragment}"
    );
    assert!(
        fragment.contains("no authorized verbs yet"),
        "空动词集必须如实呈现 'no authorized verbs yet'，实得：{fragment}"
    );
}

/// 从渲染片段中抽出"动词清单行"（`- <verb>` 形态）的渲染动词**集合**——这是片段实际
/// 列出的动词全集，供"恰列出且仅列出"做集合相等断言（不靠 `contains` 子串、不靠固定黑名单）。
/// 任何编造的集合外动词只要被 push 进片段，就会落进本集合从而被精确捕获。
fn rendered_verb_set(fragment: &str) -> std::collections::BTreeSet<String> {
    fragment
        .lines()
        .filter_map(|line| line.strip_prefix("- "))
        .map(|verb| verb.trim().to_string())
        .collect()
}

// §8 F-9：喂非空动词集 → 片段恰列出**且仅列出**该集合。不再靠 `contains` 子串（会漏过
// 集合外编造动词），而是抽出片段实际列出的动词集合、与输入集合做**精确相等**断言——
// 任一输入动词缺失、或任一集合外编造动词混入，本断言即 FAIL（fail-closed 验收钉确切结果）。
#[test]
fn claude_md_nonempty_verb_set_lists_exactly_those_verbs() {
    let facts = AuthorizationFacts {
        mcp_endpoint: "data.sock /mcp".to_string(),
        verbs: vec!["observe".to_string(), "query".to_string()],
    };

    let fragment = render_claude_md(&facts);

    let listed = rendered_verb_set(&fragment);
    let expected: std::collections::BTreeSet<String> = facts.verbs.iter().cloned().collect();
    assert_eq!(
        listed, expected,
        "片段必须恰列出且仅列出输入动词集合 {expected:?}（不增不减、无集合外编造动词），\
         实得清单 {listed:?}，片段：{fragment}"
    );
}

// §8 F-9（差分守卫：单元素集也恰列且仅列该集合）：喂仅含 observe 的集合 → 片段列出的动词
// 集合**精确等于** {observe}。不再靠"否定 mutate/destroy 两个字面量"（开放宇宙逃逸：
// execute/manage 及任意编造串 admin 等都躲过）——抽出片段实际列出的动词集合做集合相等，
// 任何集合外动词（无论是否六动词之一）混入即 FAIL。
#[test]
fn claude_md_does_not_fabricate_verbs_outside_input_set() {
    let facts = AuthorizationFacts {
        mcp_endpoint: "data.sock /mcp".to_string(),
        verbs: vec!["observe".to_string()],
    };

    let fragment = render_claude_md(&facts);

    let listed = rendered_verb_set(&fragment);
    let expected: std::collections::BTreeSet<String> =
        ["observe".to_string()].into_iter().collect();
    assert_eq!(
        listed, expected,
        "片段列出的动词集合必须精确等于输入集 {{observe}}（不补全、不编造任何集合外动词，\
         含 execute/manage 等六动词其余成员与 admin 等集合外串），实得清单 {listed:?}，\
         片段：{fragment}"
    );
}

// §8 F-9 / L-6（机器部分：空集片段形态完全确定 → 精确匹配，非黑名单）：空动词集片段除
// "端点位置事实 + 暂无已授权动词"这一如实陈述与纯结构标签外，不得含**任何**输入授权事实集
// 之外的固定散文串。空集片段形态完全确定，故直接对其全文做**精确相等**断言（构造签名检查）：
// 实现若在末尾 push 任意编造引导话术（`Bind a role to grant access`/`Ask your admin` 等），
// 全文即不再相等、本断言 FAIL——黑名单放过的"躲开固定短语的编造话术"在此被一并钉死。
#[test]
fn claude_md_empty_set_contains_no_fabricated_guidance_prose() {
    let facts = AuthorizationFacts {
        mcp_endpoint: "data.sock /mcp".to_string(),
        verbs: Vec::new(),
    };

    let fragment = render_claude_md(&facts);

    // 空集片段的**唯一**合法形态：端点位置事实 + "暂无已授权动词"如实陈述，别无任何散文。
    // 端点子串原样取自输入授权事实（非编造），其余每一字符都须是结构标签 / 如实陈述。
    let expected = format!(
        "MCP endpoint: {}\nAuthorized verbs: no authorized verbs yet\n",
        facts.mcp_endpoint
    );
    assert_eq!(
        fragment, expected,
        "空集片段形态完全确定，必须恰为端点事实 + '暂无已授权动词'如实陈述，\
         不得含任何输入授权事实集之外的固定散文（L-6 机器部分构造签名），实得：{fragment}"
    );
}

// §8 F-9（端点事实不硬编码、原样取自输入）：换一个端点位置文本 → 片段含该新文本（证明
// 端点来自输入授权事实、非 CLI 内置常量）。守卫"端点位置取自控制面回报"。
#[test]
fn claude_md_endpoint_is_taken_from_input_not_hardcoded() {
    let facts = AuthorizationFacts {
        mcp_endpoint: "alt.sock /mcp".to_string(),
        verbs: Vec::new(),
    };

    let fragment = render_claude_md(&facts);

    assert!(
        fragment.contains("alt.sock /mcp"),
        "端点位置必须原样取自输入授权事实（非硬编码 data.sock），实得：{fragment}"
    );
}
