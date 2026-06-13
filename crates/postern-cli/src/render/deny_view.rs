//! 结构化拒绝渲染（设计承诺级桩）。
//!
//! 职责（07-postern-cli §3.3，L-3/L-4）：core 的 `DenyResponse`/`DeniedFacts`/`Capability`/
//! `ObjectRef` 只 derive Serialize（不 Deserialize），CLI **不能** `serde_json::from_*`
//! 进它们。故本 unit 自定义 `#[derive(Deserialize)]` 视图 `DenyView`/`DeniedFactsView`，
//! 字段集镜像 core（`decision`/`denied`/`reason`/`your_grants`/`request_hint`/
//! `operator_note`），只反序列化与渲染、不改 core（core 冻结）。
//!
//! 渲染纪律（L-4）：字段值与输入逐项相等，无 CLI 追加话术；`reason`/`operator_note`/
//! `message` 是 daemon 侧已脱敏的常量安全文案，CLI 不展开底层原因、不补全、不推测、不重写
//! （公理六的客户端延续），绝不外泄真实地址 / 凭据。
//!
//! 反序列化与 envelope 分流同源（§3.3）：缺字段（如缺 `decision`）即返回
//! `RenderError::Deserialize` 非零退出，不补默认值、不当成功渲染（L-3，fail-closed）。

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::error::CliError;

/// `DeniedFacts` 的 CLI 侧 Deserialize 视图——镜像 core `DeniedFacts` 字段集
/// （`resource`/`capability`/`objects`），全部以字符串承载（代号 / 动词名 / 对象引用文本），
/// 绝不触机密族、绝不数值化任何 id。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct DeniedFactsView {
    /// 目标资源代号（恒为代号字符串，绝非真实地址）。
    pub resource: String,
    /// 归类动词名（core 序列化为小写动词文本，如 `"destroy"`）。
    pub capability: String,
    /// 归类对象引用（每个序列化为纯 JSON 字符串）。
    pub objects: Vec<String>,
}

/// `DenyResponse` 的 CLI 侧 Deserialize 视图——镜像 core `DenyResponse` 字段集。
///
/// L-3 钉子：`decision` 是**必需字段**（无 `#[serde(default)]`、无 `Option`），缺它的畸形
/// 拒绝响应反序列化即失败 → `RenderError::Deserialize`，绝不补默认值、绝不当成功。
/// `operator_note` 在 core 配了 `skip_serializing_if`（缺省时 JSON 不出现），故视图侧为
/// `Option` 容缺省；其余字段镜像 core 必需字段集。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct DenyView {
    /// 恒 `"deny"`（core 侧常量）。**必需字段**：缺它即解析失败（L-3）。
    pub decision: String,
    /// 脱敏拒绝事实。
    pub denied: DeniedFactsView,
    /// 引用策略事实的拒绝理由（已脱敏常量文案，原样转述）。
    pub reason: String,
    /// 该 Principal **自身**授权世界：资源代号 → 能力名列表（Scope 受限）。
    pub your_grants: BTreeMap<String, Vec<String>>,
    /// 机械生成的 `postern elevate` 命令；不可授权时为 `None`（JSON `null`）。
    pub request_hint: Option<String>,
    /// 运营者预写注记，原样转述；JSON 缺省时该字段不出现 → `None`。
    #[serde(default)]
    pub operator_note: Option<String>,
}

/// 把拒绝视图渲染为人类可读文本——只转述字段值，逐项与输入相等，无 CLI 追加话术（L-4）。
///
/// 设计承诺：输出仅由 `DenyView` 字段值构成；`reason`/`operator_note`/`your_grants`/
/// `request_hint` 原样呈现，CLI 不展开底层原因、不补"建议"、不重写（公理六）。
///
/// 渲染失败（格式化 / 写出）→ `CliError`，不 `panic`（§3.9 fail-closed）。
pub fn render_deny(view: &DenyView) -> Result<String, CliError> {
    use std::fmt::Write as _;

    // 仅拼接字段值，逐项原样转述；标签是中立的字段名，绝非引导 / 建议话术。
    let mut out = String::new();
    let _ = writeln!(out, "decision: {}", view.decision);
    let _ = writeln!(out, "resource: {}", view.denied.resource);
    let _ = writeln!(out, "capability: {}", view.denied.capability);
    let _ = writeln!(out, "objects: {}", view.denied.objects.join(", "));
    let _ = writeln!(out, "reason: {}", view.reason);

    for (resource, caps) in &view.your_grants {
        let _ = writeln!(out, "grant {}: {}", resource, caps.join(", "));
    }
    if let Some(hint) = &view.request_hint {
        let _ = writeln!(out, "request_hint: {hint}");
    }
    if let Some(note) = &view.operator_note {
        let _ = writeln!(out, "operator_note: {note}");
    }

    Ok(out)
}

/// 把拒绝响应字节反序列化为 `DenyView`（信封分流的拒绝一支）。
///
/// L-3：缺 `decision`（或任一必需字段）/ 类型错 → `CliError::DecodeFailed`，不补默认值、
/// 不当成功。
pub fn parse_deny(bytes: &[u8]) -> Result<DenyView, CliError> {
    serde_json::from_slice(bytes).map_err(|_| CliError::DecodeFailed {
        detail: "deny response did not match shared-type contract".to_string(),
    })
}
