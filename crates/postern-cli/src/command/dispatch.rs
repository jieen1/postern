//! 翻译管线公共主干（设计承诺级桩）。
//!
//! 职责（07-postern-cli §3.1 步骤 2→4，§3.6，F-3）：把意图驱动过同一条无分支主干——
//! 意图 → 请求规格 → 序列化 → 经传输层发起一次往返 → 按信封分流反序列化 → 选定渲染器
//! 输出 → 据成败置进程退出码。公共主干只有一份，使"一条命令恰一次往返、无隐式重试 /
//! 保活"（F-3）成为结构性事实而非每命令各自保证。
//!
//! 解析层 → 意图层（§3.1 步骤 1，L-1）：[`Command::into_intent`] 是 clap 解析产物到强类型
//! 管理意图的转换——在此（且仅在此）做少量**本地字面量语法校验**（`--cap <res:verb>` 经
//! [`crate::reqspec::capability::parse_cap`] 校验六动词字面量）。字面量非法 → 本地拒绝
//! （`CliError::LocalReject`），对 `control.sock` **零请求**（唯一"未发请求即失败"类别）。
//! 这里**只判形态合法、绝不判是否被允许**——任何授权判断都是客户端安全逻辑（禁，§4/L-1）。
//!
//! 退出码与失败映射（§3.6，L-1/L-2/L-5/L-7）：本地语法拒绝 / daemon 不可达 / daemon
//! 返回错误信封三类失败映射为非零退出 + 明确呈现，不在本地补偿；成功 0。`409 Conflict`
//! 原样呈现冲突 + 提示重读最新 `version`，**绝不**自动重试覆盖（L-5）。写命令失败如实
//! 呈现、不假定部分生效、不本地补写 / 回滚 / 重试（L-7）。
//!
//! `export`/`import` 是主干上"请求体来源 / 渲染落点改址"的唯一变体（§3.1）：仍走同一主干、
//! 仍是一次往返；本地至多做"文件可读 / 非空"形态检查，TOML 语义校验与整体拒绝全在 daemon。

use crate::command::intent::ManagementIntent;
use crate::command::tree::{
    ApprovalsAction, Command, ConditionAction, ConstraintAction, CredentialAction, DaemonAction,
    DenyNoteAction, ModeAction, PrincipalAction, ResourceAction, RoleAction, SettingsAction,
};
use crate::error::CliError;
use crate::render::deny_view::{parse_deny, render_deny};
use crate::render::envelope::{parse_error_envelope, Format};
use crate::render::table::render_page_envelope;
use crate::reqspec::capability::parse_cap;
use crate::transport::{HttpResponse, UdsTransport};

impl Command {
    /// clap 解析产物 → 强类型管理意图（§3.1 步骤 1，L-1）。在此做少量本地字面量语法校验
    /// （`--cap` 六动词字面量），字面量非法即 `CliError::LocalReject`、对 `control.sock`
    /// **零请求**（本转换在请求规格 / 传输之前，结构上保证 L-1 的"未发请求即失败"）。
    ///
    /// `mcp-stdio` 不产管理意图（它是数据面字节桥，走 `bridge` 域，不入本翻译管线），故此
    /// 转换不为 `Command::McpStdio` 产出 [`ManagementIntent`]——调用方在 `main` 早分流到桥。
    ///
    /// 红线（§4/L-1）：本转换**只**判"形态合法"（含 `--cap` 字面量是否属六动词集），**绝不**
    /// 判"是否被允许 / 是否被授权"——授权裁决全在 daemon，CLI 不持、不比对。
    pub fn into_intent(self) -> Result<ManagementIntent, CliError> {
        let intent = match self {
            Command::Daemon { action } => match action {
                DaemonAction::Status => ManagementIntent::DaemonStatus,
                DaemonAction::Stop => ManagementIntent::DaemonStop,
            },
            Command::Init => {
                // `init` 是多步编排（§3.7），由 `init` 域驱动；不在单往返翻译管线内。这里不为
                // 它产单条管理意图——调用方据 `Command::Init` 早分流到向导，不经本转换。
                return Err(CliError::LocalReject {
                    usage: "init is an interactive wizard, not a single request".to_string(),
                });
            }
            Command::Resource { action } => match action {
                ResourceAction::Add { codename } => ManagementIntent::ResourceAdd { codename },
                ResourceAction::List { page_no, page_size } => {
                    ManagementIntent::ResourceList { page_no, page_size }
                }
                ResourceAction::Disable { code, version } => {
                    ManagementIntent::ResourceDisable { code, version }
                }
                ResourceAction::Discover { code } => ManagementIntent::ResourceDiscover { code },
            },
            Command::Principal { action } => match action {
                PrincipalAction::Add { codename } => ManagementIntent::PrincipalAdd { codename },
                PrincipalAction::List { page_no, page_size } => {
                    ManagementIntent::PrincipalList { page_no, page_size }
                }
            },
            Command::Role { action } => match action {
                RoleAction::Add { name } => ManagementIntent::RoleAdd { name },
                RoleAction::List { page_no, page_size } => {
                    ManagementIntent::RoleList { page_no, page_size }
                }
                RoleAction::Bind { principal, role } => {
                    ManagementIntent::RoleBind { principal, role }
                }
            },
            Command::Credential { action } => match action {
                CredentialAction::Add { resource } => ManagementIntent::CredentialAdd { resource },
                CredentialAction::Revoke { id } => ManagementIntent::CredentialRevoke { id },
                CredentialAction::Rotate { id } => ManagementIntent::CredentialRotate { id },
            },
            Command::Grants { principal } => ManagementIntent::Grants { principal },
            // `--cap <res:verb>` 本地字面量校验（L-1）：非法（缺冒号 / 非六动词）即 `LocalReject`，
            // 对 `control.sock` 零请求——本转换在请求规格 / 传输之前，结构上保证"未发请求即失败"。
            Command::Elevate {
                principal,
                cap,
                ttl,
            } => {
                let literal = parse_cap(&cap).map_err(|_| CliError::LocalReject {
                    usage: "elevate --cap must be <resource:verb> with a known verb".to_string(),
                })?;
                ManagementIntent::Elevate {
                    principal,
                    resource: literal.resource,
                    verb: literal.verb,
                    ttl,
                }
            }
            Command::RevokeGrant { id, version } => ManagementIntent::RevokeGrant { id, version },
            Command::Mode { action } => match action {
                ModeAction::Set {
                    mode,
                    resource,
                    ttl,
                } => ManagementIntent::ModeSet {
                    mode,
                    resource,
                    ttl,
                },
            },
            Command::Freeze => ManagementIntent::Freeze,
            Command::Constraint { action } => match action {
                ConstraintAction::Add { resource } => ManagementIntent::ConstraintAdd { resource },
                ConstraintAction::List { resource } => {
                    ManagementIntent::ConstraintList { resource }
                }
                ConstraintAction::Rm { resource, id } => {
                    ManagementIntent::ConstraintRm { resource, id }
                }
            },
            Command::Condition { action } => match action {
                ConditionAction::Add { resource } => ManagementIntent::ConditionAdd { resource },
                ConditionAction::List { resource } => ManagementIntent::ConditionList { resource },
                ConditionAction::Rm { resource, id } => {
                    ManagementIntent::ConditionRm { resource, id }
                }
            },
            Command::DenyNote { action } => match action {
                DenyNoteAction::Set { resource, verb } => {
                    ManagementIntent::DenyNoteSet { resource, verb }
                }
                DenyNoteAction::List { resource } => ManagementIntent::DenyNoteList { resource },
                DenyNoteAction::Rm { resource, verb } => {
                    ManagementIntent::DenyNoteRm { resource, verb }
                }
            },
            Command::Settings { action } => match action {
                SettingsAction::Get { key } => ManagementIntent::SettingsGet { key },
                SettingsAction::Set { key, value } => ManagementIntent::SettingsSet { key, value },
            },
            Command::Approvals { action } => match action {
                ApprovalsAction::List { page_no, page_size } => {
                    ManagementIntent::ApprovalsList { page_no, page_size }
                }
                ApprovalsAction::Approve { id } => ManagementIntent::ApprovalsApprove { id },
                ApprovalsAction::Deny { id } => ManagementIntent::ApprovalsDeny { id },
            },
            Command::Denials { window } => ManagementIntent::Denials { window },
            Command::Audit {
                principal,
                since,
                page_no,
                page_size,
                format: _,
            } => ManagementIntent::Audit {
                principal,
                since,
                page_no,
                page_size,
            },
            Command::Verify => ManagementIntent::Verify,
            Command::Export { file } => ManagementIntent::Export { file },
            Command::Import { file } => ManagementIntent::Import { file },
            // `mcp-stdio` 不产管理意图（数据面字节桥，走 `bridge` 域，不入翻译管线）。调用方在
            // `main` 早分流到桥，不经本转换；落到此处即编程错误，按本地拒绝 fail-closed。
            Command::McpStdio => {
                return Err(CliError::LocalReject {
                    usage: "mcp-stdio is a data-plane byte bridge, not a control-plane command"
                        .to_string(),
                })
            }
        };
        Ok(intent)
    }
}

/// 翻译管线公共主干（§3.1 步骤 2→4，F-3）：管理意图 → 请求规格 → 一次 HTTP-over-UDS 往返
/// → 按信封分流渲染 → 返回成败。公共主干**一份**，使"一条命令恰一次往返、无隐式重试 /
/// 保活"成为结构性事实（F-3）。
///
/// 失败映射（§3.6，L-1/L-2/L-5/L-7）：本地语法拒绝（含 `into_intent` 的 `--cap` 校验）/
/// daemon 不可达 / daemon 返回错误信封（含 `409` 冲突、写端点 5xx）→ `CliError`，由 `main`
/// 映射非零退出码；`409` 原样呈现冲突 + 提示重读 `version`、绝不自动重试覆盖（L-5）；写失败
/// 如实呈现、不本地补偿（L-7）。
///
/// 成功路径返回 `Ok(rendered)`——已渲染好的人类可读输出文本（或 `--format jsonl` 机器形态）。
pub async fn dispatch(
    intent: ManagementIntent,
    transport: &UdsTransport,
) -> Result<String, CliError> {
    // 意图 → 请求规格（命令 ⊆ 6.5 的唯一映射落点）→ 一次 HTTP-over-UDS 往返（恰一次，F-3）。
    let spec = intent.to_request_spec()?;
    let response = transport.round_trip(&spec).await?;
    // 信封分流渲染（§3.3）：据状态与体顶层形状走 `Page<T>` / 拒绝 / `{error:..}` 三分支。
    // 表格为默认形态；`--format jsonl` 在 `main` 据命令选择，此主干据信封类别转述、不加工。
    render_response(&response, Format::Table)
}

/// 信封三分支渲染（§3.3，F-4/L-3/L-4/L-7）：据 HTTP 状态与响应体顶层形状选渲染器，每支只
/// 转述、不加工。错误信封（含 `409` 冲突、写端点 5xx）原样呈现；拒绝事实按拒绝视图原样转述；
/// 集合按 `Page<T>` 表格 / jsonl 渲染。任一形态反序列化失败即 `DecodeFailed`（fail-closed）。
fn render_response(response: &HttpResponse, format: Format) -> Result<String, CliError> {
    let bytes = &response.body;
    // 先判"是不是统一错误信封"（顶层含 `error` 键）——含则把 `code`/`message` 原样转为
    // `DaemonError`（非零退出、不本地补偿，L-7；`409` 冲突亦走此支、绝不自动重试，L-5）。
    // `message` 是 daemon 侧已脱敏常量文案，CLI 逐字转述、不展开、不补全、不重写（L-4）。
    if let Ok(envelope) = parse_error_envelope(bytes) {
        return Err(CliError::DaemonError {
            code: envelope.error.code,
            message: envelope.error.message,
        });
    }
    // 拒绝事实（`DenyResponse`，顶层含 `decision`）——按拒绝视图原样转述字段值（L-4）。
    if let Ok(view) = parse_deny(bytes) {
        return render_deny(&view);
    }
    // 否则按集合 `Page<T>` 信封渲染（表格 / jsonl）；不符共享类型契约即 `DecodeFailed`（L-3）。
    render_page_envelope(bytes, format)
}
