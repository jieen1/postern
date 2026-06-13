//! 信封三分支分流（设计承诺级桩）。
//!
//! 职责（07-postern-cli §3.3）：响应字节按"先判信封类别再选目标类型"反序列化——HTTP
//! 状态与响应体顶层形状共同决定走 `Page<T>` / 单条 DTO / `{error:{code,message}}` 三条
//! 渲染分支之一。反序列化失败（缺字段 / 类型错）即本地报错非零退出，不猜测补全、不忽略
//! 字段、不当成功（L-3，fail-closed 的客户端延续）。
//!
//! 类型两端契约取自 core 共享 DTO（§5、§6.1）：`Page<T>` 用 core（core DOES derive
//! Deserialize），但拒绝事实（`DenyResponse`/`DeniedFacts`）与单条 DTO 由本 unit 自定义
//! `#[derive(Deserialize)]` 视图镜像——core 那些类型只 Serialize，CLI 不能 from_* 进去。

use serde::Deserialize;

use crate::error::CliError;

/// 输出形态选择（§3.3）：默认人类可读，`--format jsonl` 机器形态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// 人类可读对齐表格 / 纵向字段表。
    Table,
    /// 机器形态：逐行可独立解析的 JSON（雪花 id 仍为字符串）。
    Jsonl,
}

/// `{error:{code,message}}` 统一错误信封的 CLI 侧视图（Deserialize）。
///
/// L-4：`code`/`message` 原样转述、逐字符不增删；`message` 是 daemon 侧已脱敏的常量安全
/// 文案，CLI 不展开底层原因、不补话术、不重写。本视图只承载这两个字段。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ErrorEnvelope {
    /// 错误信封载荷。
    pub error: ErrorBody,
}

/// `{error:{code,message}}` 内层载荷。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ErrorBody {
    /// 稳定错误码（daemon 侧常量）。
    pub code: String,
    /// 已脱敏的常量安全文案（daemon 侧）。
    pub message: String,
}

/// 把统一错误信封渲染为人类可读文本——逐字符原样含 `code`/`message`，无 CLI 追加话术
/// （L-4，公理六的客户端延续）。
///
/// 设计承诺：输出里 `code` 与 `message` 的字符序列与输入完全一致（不增删字符），CLI 不在
/// 其外侧附加任何引导 / 建议文案。
pub fn render_error(envelope: &ErrorEnvelope) -> Result<String, CliError> {
    // 仅由信封字段值构成：稳定错误码与已脱敏常量文案逐字转述，无任何 CLI 自造话术。
    Ok(format!(
        "{} {}",
        envelope.error.code, envelope.error.message
    ))
}

/// 据响应体顶层形状把"是不是错误信封"判出来（信封三分支分流入口的一支）。
///
/// 顶层含 `error` 键 → 错误信封分支；否则交由 `Page<T>` / 单条 DTO 分支。反序列化失败即
/// 返回 `CliError::DecodeFailed`（L-3：不当成功、不补全）。
pub fn parse_error_envelope(bytes: &[u8]) -> Result<ErrorEnvelope, CliError> {
    // 缺 `code`/`message` 或类型错 → 返回 DecodeFailed（不补空串、不当成功，fail-closed）。
    serde_json::from_slice(bytes).map_err(|_| CliError::DecodeFailed {
        detail: "error envelope did not match {error:{code,message}} shape".to_string(),
    })
}
