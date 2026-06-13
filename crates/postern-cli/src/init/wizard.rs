//! 接入向导状态机编排（设计承诺级桩）。
//!
//! 职责（07-postern-cli §3.7，F-8，L-8）：把多步接入流程编排为同一流程内的顺序往返——
//! ①建资源（`POST /v1/resources`，拿回 `code`/`version`）→ ②触发探测
//! （`POST /v1/resources/{code}/discover`，daemon 侧真实连资源跑 `Adapter::discover`，回报
//! 候选对象 + 缺口清单）→ ③呈现候选 + 缺口（逐条列出）→ ④圈选回写（把人选择转译为后续
//! 控制面写调用：tier `.../constraints`、绑定 `/v1/bindings`、细则等，即"代修"）。
//!
//! 关键取舍（§3.7）：向导**把多步编排放在客户端、把每步判定放在 daemon**。编排（先建后探
//! 再圈选）是纯流程顺序、无安全语义；而每步的"能不能 / 对不对"（探测结果、tier 子集、缺口
//! 是否真消解）一旦放客户端就成本地安全逻辑——故状态机只管"下一步发哪个已有控制面调用"，
//! 从不自行下安全结论。缺口未消解前 CLI 不标资源可用；修正动作语义合法性仍由 daemon 裁决。

use serde::{Deserialize, Serialize};

use crate::error::CliError;
use crate::reqspec::query::Query;
use crate::reqspec::{Method, RequestSpec, WriteBody};

/// 向导对控制面的**唯一**出口（§3.7，F-8）：把一个请求规格发出去、拿回 daemon 应答字节。
///
/// 抽象成 trait 的理由：向导只编排"下一步发哪个已有控制面调用"，**不**关心传输细节——真
/// 实运行时由 [`crate::transport::UdsTransport`] 适配（经 `control.sock` 一次往返）；测试侧
/// 由内存 Fake 适配（记录调用序列、回放注入的候选 / 缺口），无需真实 daemon（§9）。
///
/// 红线（L-8/§4）：本 trait 只有"发起一次控制面调用"这**一个**能力——向导经它**不能**自行
/// 探测、不能直连资源、不能下任何安全结论；候选 / 缺口 / 缺口是否消解全由 daemon 经应答回报。
pub trait ControlPlane {
    /// 发起**一次**控制面调用并拿回 daemon 应答的响应体字节（§3.7 步骤序列的每一步）。
    ///
    /// 向导把每一步（建资源 / 触发探测 / 圈选回写）都表达为一个 [`RequestSpec`] 交由本方法
    /// 发出；候选 / 缺口 / 版本等**全部**由 daemon 经返回字节回报，向导不自造。失败一律
    /// fail-closed 上抛（公理二）——向导不在本地补偿、不假定部分生效（L-7）。
    fn call(&mut self, spec: &RequestSpec) -> Result<Vec<u8>, CliError>;
}

/// ①建资源后 daemon 回报的资源句柄（§3.7 步骤 1）：资源 `code` + 乐观锁基线 `version`。
///
/// `code` 用于装配步骤 2 的 `POST /v1/resources/{code}/discover` 路径；`version` 是搬运型
/// 乐观锁基线（F-7），后续写沿用、CLI 不自造 / 不自增。是 daemon 应答的 CLI 侧只读视图
/// （`Deserialize`），向导不补全、不推测。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct ResourceHandle {
    /// 资源代号（匿名化代号，非真实地址），取自建资源应答，供后续步骤路径填充。
    pub code: String,
    /// 建资源应答携带的乐观锁基线 `version`（搬运型，F-7），后续写沿用、不自造。
    pub version: u64,
}

/// ②探测后 daemon 回报的**候选对象**之一（§3.7 步骤 2/3）：可绑定的能力面 / 账号 / 库表等。
///
/// 候选**完全**取自 daemon 回报——向导只呈现、供人圈选，**不**自行发现、不解析协议（L-12）。
/// 本结构是 daemon 应答的 CLI 侧只读视图，字段为不透明事实串。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Candidate {
    /// 候选对象的稳定标识（如能力面 / 表 / 账号代号），原样取自 daemon 回报，供圈选回引。
    pub key: String,
    /// 候选对象的人类可读标签（事实），原样呈现，CLI 不补全、不推测。
    pub label: String,
}

/// daemon 为某缺口 / 候选回报的**代修写调用描述**（§3.7 步骤 4，L-8）：method + path + 体字段。
///
/// 这是 daemon 在探测应答里**已给出**的后续控制面写调用形态——向导把它原样转成 [`RequestSpec`]
/// 发出即"代修"，**不**自造、**不**判定修正是否合法（裁决在 daemon，L-8）。是序列化视图，
/// 故缺口清单可整体经控制面应答回报。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct RepairSpec {
    /// 代修写调用的 HTTP 方法文本（如 `POST`/`PUT`），取自 daemon 回报。
    pub method: String,
    /// 代修写调用的控制面路径（如 `/v1/resources/{code}/constraints`、`/v1/bindings`），
    /// 取自 daemon 回报；向导原样发出、不改写。
    pub path: String,
    /// 代修写调用的请求体字段（不透明键值），取自 daemon 回报；向导原样落入、不增删、不推测。
    pub fields: std::collections::BTreeMap<String, String>,
}

/// ②探测后 daemon 回报的**缺口**之一（§3.7 步骤 2/3，L-8，对 docs/examples/02 §4.2
/// E2/E3/E5）：端口不可达 / tier 名实不符 / 2FA 须人在场等。
///
/// 缺口**完全**取自 daemon 回报——向导只逐条呈现，**不**自行判定缺口是否消解、**不**比对
/// "声明 ⊆ 真实权限"（那是 daemon 的 tier 子集校验，§4/L-8）。`fix` 携 daemon 给出的**代修
/// 调用描述**（若该缺口可经控制面写调用代修）：向导把它原样发出即"代修"，修正动作的语义
/// 合法性裁决仍在 daemon。`fix=None`（如 2FA 须人在场）= 该缺口无客户端代修路径，只呈现。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Gap {
    /// 缺口稳定标识（如 `port-unreachable`/`tier-mismatch`/`twofa-needs-human`），原样取自
    /// daemon 回报，供呈现与（若有）代修回引。
    pub key: String,
    /// 缺口的人类可读描述（事实，已由 daemon 脱敏——不含真实地址 / 端口明文，§4/L-4）。
    /// 原样呈现，CLI 不展开、不补全、不推测、不重写。
    pub detail: String,
    /// 该缺口的**代修调用描述**（若可经控制面写调用代修）：`Some(spec)` = 向导把它原样发出
    /// 即"代修"，daemon 裁决修正动作语义合法性；`None`（如 2FA 须人在场）= 无客户端代修路径，
    /// 向导只呈现、不发任何写调用、更不自行判定缺口消解（L-8）。
    pub fix: Option<RepairSpec>,
}

/// ②探测后 daemon 回报的整体（§3.7 步骤 2）：候选清单 + 缺口清单。
///
/// 向导对它的处置只有"呈现 + 据人圈选发后续控制面写调用"——本结构本身**不**含任何"是否
/// 可用 / 是否消解"的判定字段，因为那一律由 daemon 在后续调用里裁决（L-8）。是 daemon 探测
/// 应答的 CLI 侧只读视图（`Deserialize`）。
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize, Serialize)]
pub struct DiscoveryReport {
    /// daemon 回报的候选对象清单（可为空）。
    pub candidates: Vec<Candidate>,
    /// daemon 回报的缺口清单（可为空 = 无缺口）。逐条呈现（步骤 3）。
    pub gaps: Vec<Gap>,
}

/// 人在步骤 ④的**圈选**（§3.7 步骤 4）：选定要纳入的候选 + 选定要代修的缺口。
///
/// 圈选**只**承载"人选了哪些 daemon 回报的候选 / 缺口"——向导据此把每个选择转译为**对应
/// daemon 回报里已给出的**后续控制面写调用（候选 → tier/绑定写、缺口 → 其 `fix` 代修写），
/// **不**自造任何 daemon 未列出的调用、**不**自行判定选择是否合法（裁决在 daemon，L-8）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Selection {
    /// 选定纳入的候选 `key` 集（须为本轮 `DiscoveryReport.candidates` 子集；向导只回引、
    /// 不自造）。
    pub chosen_candidates: Vec<String>,
    /// 选定代修的缺口 `key` 集（须为本轮 `DiscoveryReport.gaps` 子集；向导对每个发出其
    /// `fix` 写调用，`fix=None` 的缺口无代修路径、不可被选中代修）。
    pub repair_gaps: Vec<String>,
}

/// 接入向导一次完整编排的**有序控制面调用轨迹**（§3.7，F-8）：建资源 → 触发探测 → 回写。
///
/// 这是向导唯一对外可观测的"做了什么"——一串**已发出**的控制面请求规格，顺序即编排顺序。
/// 测试据此断言序列 = 建资源 → discover → 回写（F-8），且其中**没有**任何 CLI 自探测 /
/// 自行 tier 校验调用（出现即不过，L-8）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WizardTrace {
    /// 本次编排按顺序经控制面发出的请求规格清单。第 0 项恒为建资源、第 1 项恒为触发探测，
    /// 其后为据圈选发出的回写 / 代修调用（步骤 4）。向导不在此插入任何探测 / tier 校验调用。
    pub calls: Vec<RequestSpec>,
    /// 本次编排拿回的探测应答（候选 + 缺口，全由 daemon 回报）。供呈现 / 核验：向导只转述、
    /// 不据此自行判定缺口消解或资源可用（L-8）。
    pub report: DiscoveryReport,
}

/// 驱动一次接入向导编排（§3.7 步骤 1→4，F-8/L-8）：建资源 → 触发探测 → 呈现 →（据 `select`
/// 回调圈选）→ 把圈选转译为后续控制面写调用。
///
/// 编排序列（纯流程顺序，无安全语义）：
/// 1. 经 `cp` 发 `POST /v1/resources` 建资源 → 拿回 [`ResourceHandle`]（`code`/`version`）。
/// 2. 经 `cp` 发 `POST /v1/resources/{code}/discover` 触发探测 → 拿回 [`DiscoveryReport`]
///    （候选 + 缺口，全由 daemon 回报；向导**不**直连资源、**不**跑 `Adapter::discover`、
///    **不**解析协议，L-12）。
/// 3. 把候选 + 缺口逐条呈现交 `select` 回调（人圈选的纯输入边界）。
/// 4. 据回调返回的 [`Selection`] 把每个选择转译为**daemon 回报里已给出的**后续控制面写
///    调用（候选 → tier/绑定写、缺口 → 其 `fix` 代修写）并经 `cp` 发出（"代修"）。
///
/// 红线（§3.7/§4/L-8）：本函数**不**自行探测、**不**比对"声明 ⊆ 真实权限"、**不**自行判定
/// 缺口是否消解、**不**标资源可用——每步判定全在 daemon。它只负责"按顺序发哪个已有控制面
/// 调用"，候选 / 缺口 / 修正合法性全经 `cp` 应答由 daemon 回报与裁决。返回本次编排的有序
/// 调用轨迹（[`WizardTrace`]）供呈现 / 核验。任一步控制面失败即 fail-closed 上抛、不补偿（L-7）。
pub fn run_wizard<C, S>(
    cp: &mut C,
    codename: &str,
    adapter: &str,
    transport: &str,
    select: S,
) -> Result<WizardTrace, CliError>
where
    C: ControlPlane,
    S: FnOnce(&DiscoveryReport) -> Selection,
{
    let mut calls = Vec::new();

    // 步骤 1：建资源 `POST /v1/resources`。把适配器 / 传输等人给参数原样落入写体——CLI 只
    // 搬运、不自造；daemon 回报 `code` + 乐观锁基线 `version`。任一步失败即 fail-closed 上抛、
    // 不补偿、不半截推进（建资源没成就绝不去探测）。
    let create_spec = create_resource_spec(codename, adapter, transport);
    let handle_bytes = cp.call(&create_spec)?;
    calls.push(create_spec);
    let handle: ResourceHandle = decode(&handle_bytes)?;

    // 步骤 2：触发探测 `POST /v1/resources/{code}/discover`，`{code}` 搬运自上一步应答（非自造）。
    // daemon 侧真实连资源跑探测、回报候选 + 缺口；CLI 不直连资源、不解析协议、不在本地跑探测。
    let discover_spec = discover_spec(&handle.code);
    let report_bytes = cp.call(&discover_spec)?;
    calls.push(discover_spec);
    let report: DiscoveryReport = decode(&report_bytes)?;

    // 步骤 3：把候选 + 缺口逐条呈现交回调（人圈选的纯输入边界）；CLI 不自行判定。
    let selection = select(&report);

    // 步骤 4：把圈选转译为后续控制面写调用。"代修"= 把 daemon 在缺口侧**已给出**的 `fix`
    // 原样发出；`fix=None` 的缺口（如 2FA 须人在场）无客户端代修路径、不发任何写。CLI 不自造
    // 调用、不自比对"声明 ⊆ 真实权限"、不自行判定缺口消解——合法性裁决仍在 daemon（L-8）。
    for gap_key in &selection.repair_gaps {
        let Some(gap) = report.gaps.iter().find(|g| &g.key == gap_key) else {
            continue;
        };
        let Some(fix) = gap.fix.as_ref() else {
            continue;
        };
        let write_spec = repair_spec_into_request(fix)?;
        cp.call(&write_spec)?;
        calls.push(write_spec);
    }

    Ok(WizardTrace { calls, report })
}

/// 建资源 `POST /v1/resources`（步骤 1）：把人给的适配器 / 传输原样落入写体。创建型写，
/// 无前置读 → 无期望乐观锁版本（`version: None`，搬运型语义见 [`crate::reqspec::WriteBody`]）。
fn create_resource_spec(codename: &str, adapter: &str, transport: &str) -> RequestSpec {
    let mut fields = std::collections::BTreeMap::new();
    fields.insert("code".to_string(), codename.to_string());
    fields.insert("adapter".to_string(), adapter.to_string());
    fields.insert("transport".to_string(), transport.to_string());

    RequestSpec {
        method: Method::Post,
        path_template: "/v1/resources".to_string(),
        query: Query::default(),
        body: Some(WriteBody {
            fields,
            version: None,
        }),
    }
}

/// 触发探测 `POST /v1/resources/{code}/discover`（步骤 2）：`{code}` 搬运自建资源应答。
/// 读式触发（无命令载荷体）——daemon 据 code 真实连资源跑探测，CLI 不直连、不解析协议。
fn discover_spec(code: &str) -> RequestSpec {
    RequestSpec {
        method: Method::Post,
        path_template: format!("/v1/resources/{code}/discover"),
        query: Query::default(),
        body: None,
    }
}

/// 把 daemon 回报的 [`RepairSpec`] 原样转成 [`RequestSpec`]（步骤 4 代修写）：方法 / 路径 /
/// 体字段逐项搬运，向导不增删、不改写。方法文本经封闭集解析——非法方法即 fail-closed 上抛
/// （`DecodeFailed`），绝不崩进程、不静默吞掉（公理二）。代修写恒携体（写端点），无期望
/// 乐观锁版本（`version: None`）：CLI 不自读自比、不自造 `version`（F-7）。
fn repair_spec_into_request(fix: &RepairSpec) -> Result<RequestSpec, CliError> {
    let method = parse_method(&fix.method)?;
    Ok(RequestSpec {
        method,
        path_template: fix.path.clone(),
        query: Query::default(),
        body: Some(WriteBody {
            fields: fix.fields.clone(),
            version: None,
        }),
    })
}

/// 把 daemon 回报的方法文本解析为封闭集 [`Method`]——大小写不敏感的规范名匹配。无 `_ =>`
/// 兜底成默认方法（那会让 CLI 替 daemon 猜测）：未知方法一律 fail-closed 报 `DecodeFailed`
/// （L-3），绝不静默降级。
fn parse_method(text: &str) -> Result<Method, CliError> {
    match text.to_ascii_uppercase().as_str() {
        "GET" => Ok(Method::Get),
        "POST" => Ok(Method::Post),
        "PUT" => Ok(Method::Put),
        "DELETE" => Ok(Method::Delete),
        _ => Err(CliError::DecodeFailed {
            detail: "repair spec carried an unrecognized HTTP method".to_string(),
        }),
    }
}

/// 把 daemon 应答字节解码为 CLI 侧只读视图——任何不符合共享类型契约（缺字段 / 类型错）即
/// fail-closed 报 `DecodeFailed`（L-3），绝不补默认值、绝不当成功。`detail` 为常量类别描述，
/// 不回显响应原文（避免外泄未脱敏字节）。
fn decode<T>(bytes: &[u8]) -> Result<T, CliError>
where
    T: for<'de> Deserialize<'de>,
{
    serde_json::from_slice(bytes).map_err(|_| CliError::DecodeFailed {
        detail: "control-plane response did not match shared-type contract".to_string(),
    })
}
