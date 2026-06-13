//! clap 命令树定义（设计承诺级桩）。
//!
//! 职责（07-postern-cli §3.1 步骤 1，F-1，对人接口 §5）：以 clap（derive）声明面向人的
//! 命令行契约——22 个命令组及其子命令 / 参数（见 §3 表与 6.5 端点一一对应）。语法层校验
//! （缺参 / 互斥 / 格式不合法的本地拒绝）由 clap 与少量本地字面量校验完成，对 `control.sock`
//! 零请求（L-1）。
//!
//! 红线（§3.1/§4，L-1）：clap 层只产出"形态合法"的意图，**任何语义合法性判断一律不在此**
//! ——不判"是否被允许"，否则即成客户端安全逻辑。clap 参数结构是 CLI 内部类型、不构成
//! 对外库接口（本 crate 不向工作区其他 crate 暴露库接口，§5）。
//!
//! 红线（L-12，CONS-20）：`resource discover` 是**控制面**接入侧探测，命令树里**不出现**
//! 任何数据面 `postern_surface` 投影或 `Adapter::discover` 直连——CLI 不在数据面，命令树
//! 只声明控制面命令面。

use clap::{Parser, Subcommand};

/// 二进制 `postern` 的顶层命令行契约（§3、§5 对人接口）。clap derive 声明，解析产物经
/// [`Command`] 落到强类型管理意图，再映射到 6.5 控制面端点。
#[derive(Debug, Parser)]
#[command(name = "postern", about = "postern control-plane thin client")]
pub struct Cli {
    /// 控制面命令组（§3 全表 22 组）。
    #[command(subcommand)]
    pub command: Command,
}

/// §3 全表 22 个命令组（F-1：缺任一组即不过）。变体名 = 命令组名（clap kebab-case 转换后
/// 即 `daemon`/`init`/`resource`/.../`mcp-stdio`）。每组携该组子命令 / 参数；映射到 6.5
/// 端点在 [`super::intent`] 一处声明。
#[derive(Debug, Subcommand)]
pub enum Command {
    /// `daemon status|stop` → `GET /v1/health` · `POST /v1/shutdown`。
    Daemon {
        #[command(subcommand)]
        action: DaemonAction,
    },
    /// `init`（接入向导）→ 建资源 → discover → 回写（§3.7，F-8）。
    Init,
    /// `resource add|list|disable|discover` → `/v1/resources`（含 `{code}/discover`）。
    Resource {
        #[command(subcommand)]
        action: ResourceAction,
    },
    /// `principal add|list|...` → `/v1/principals`。
    Principal {
        #[command(subcommand)]
        action: PrincipalAction,
    },
    /// `role add|list|bind|...` → `/v1/roles` · `/v1/bindings`。
    Role {
        #[command(subcommand)]
        action: RoleAction,
    },
    /// `credential add|revoke|rotate|...` → `/v1/credentials`（含 `{id}/revoke`）。
    Credential {
        #[command(subcommand)]
        action: CredentialAction,
    },
    /// `grants <principal>` → `GET /v1/grants/{principal}`。
    Grants {
        /// 目标 Principal 代号 / id（路径参数 `{principal}`）。
        principal: String,
    },
    /// `elevate <principal> --cap <res:verb> --ttl <dur>` → `POST /v1/grants/temp`。
    Elevate {
        /// 目标 Principal（落体字段 `principal`）。
        principal: String,
        /// 能力字面量 `<res:verb>`——本地字面量校验（六动词集），非授权判断（L-1）。
        #[arg(long = "cap")]
        cap: String,
        /// 临时授权时长（落体字段 `ttl`）。**必填**：缺则 clap 本地拒绝、零请求（L-1）。
        #[arg(long = "ttl")]
        ttl: String,
    },
    /// `revoke-grant <id>` → `DELETE /v1/grants/temp/{id}`。
    RevokeGrant {
        /// 临时授权 id（路径参数 `{id}`）。
        id: String,
        /// 期望乐观锁版本——唯一来源是先前读取响应（F-7，只透传不自造）。
        #[arg(long = "version")]
        version: Option<u64>,
    },
    /// `mode set <observe|maintain|freeze|normal> [--resource] [--ttl]` → `PUT /v1/mode`。
    Mode {
        #[command(subcommand)]
        action: ModeAction,
    },
    /// `freeze`（= `mode set freeze` 全局别名）→ `PUT /v1/mode`。
    Freeze,
    /// `constraint add|list|rm <res> ...` → `/v1/resources/{code}/constraints`。
    Constraint {
        #[command(subcommand)]
        action: ConstraintAction,
    },
    /// `condition add|list|rm <res> ...` → `/v1/resources/{code}/conditions`。
    Condition {
        #[command(subcommand)]
        action: ConditionAction,
    },
    /// `deny-note set|list|rm <res> <verb>` → `/v1/resources/{code}/deny-notes`。
    DenyNote {
        #[command(subcommand)]
        action: DenyNoteAction,
    },
    /// `settings get|set <key> [<value>]` → `GET/PUT /v1/settings/{key}`。
    Settings {
        #[command(subcommand)]
        action: SettingsAction,
    },
    /// `approvals list|approve|deny <id>` → `/v1/approvals`（含 `{id}/approve|deny`）。
    Approvals {
        #[command(subcommand)]
        action: ApprovalsAction,
    },
    /// `denials [--window 7d]` → `GET /v1/denials/summary?window=7d`。
    Denials {
        /// 聚合窗口（查询键 `window`）；缺则不带键，由后端取默认。
        #[arg(long = "window")]
        window: Option<String>,
    },
    /// `audit [--principal] [--since] [--page-no] [--page-size] [--format jsonl]`
    /// → `GET /v1/audit`。
    Audit {
        /// 过滤：Principal（查询键 `principal`）。
        #[arg(long = "principal")]
        principal: Option<String>,
        /// 过滤：起始时刻（查询键 `since`）。
        #[arg(long = "since")]
        since: Option<String>,
        /// 分页页号（查询键 `page_no`）；缺则不带键（F-6）。
        #[arg(long = "page-no")]
        page_no: Option<u32>,
        /// 分页页大小（查询键 `page_size`）；缺则不带键（F-6）。
        #[arg(long = "page-size")]
        page_size: Option<u32>,
        /// 机器形态 `--format jsonl`（默认表格，§3.3）。
        #[arg(long = "format")]
        format: Option<String>,
    },
    /// `verify` → `POST /v1/verify`（红队自检触发，执行在 daemon）。
    Verify,
    /// `export <file.toml>` → `POST /v1/export`（响应 TOML 写入文件 / stdout）。
    Export {
        /// 导出落点文件（渲染落点改址，§3.1）；缺省写 stdout。
        file: Option<String>,
    },
    /// `import <file.toml>` → `POST /v1/import`（文件原样作 body，不解析 TOML 语义）。
    Import {
        /// 导入源文件（原样作请求体；CLI 仅做可读 / 非空形态检查，§3.1）。
        file: String,
    },
    /// `mcp-stdio` → 数据面 `data.sock` 的 `/mcp` 字节桥（§3.8；非控制面端点）。
    McpStdio,
}

/// `daemon` 子命令（§3）。
#[derive(Debug, Subcommand)]
pub enum DaemonAction {
    /// `daemon status` → `GET /v1/health`。
    Status,
    /// `daemon stop` → `POST /v1/shutdown`。
    Stop,
}

/// `resource` 子命令（§3）。
#[derive(Debug, Subcommand)]
pub enum ResourceAction {
    /// `resource add` → `POST /v1/resources`。
    Add {
        /// 资源代号（人类输入，落体）。
        codename: String,
    },
    /// `resource list` → `GET /v1/resources`。
    List {
        /// 分页页号（缺则不带键，F-6）。
        #[arg(long = "page-no")]
        page_no: Option<u32>,
        /// 分页页大小（缺则不带键，F-6）。
        #[arg(long = "page-size")]
        page_size: Option<u32>,
    },
    /// `resource disable <code>` → 资源停用（携期望 `version` 回传，F-7）。
    Disable {
        /// 资源代号（路径参数 `{code}`）。
        code: String,
        /// 期望乐观锁版本（唯一来源先前读取，F-7）。
        #[arg(long = "version")]
        version: Option<u64>,
    },
    /// `resource discover <code>` → 控制面 `POST /v1/resources/{code}/discover`
    /// （L-12：控制面接入侧探测，非数据面 surface）。
    Discover {
        /// 资源代号（路径参数 `{code}`）。
        code: String,
    },
}

/// `principal` 子命令（§3）。
#[derive(Debug, Subcommand)]
pub enum PrincipalAction {
    /// `principal add` → `POST /v1/principals`。
    Add {
        /// Principal 代号（人类输入，落体）。
        codename: String,
    },
    /// `principal list` → `GET /v1/principals`。
    List {
        #[arg(long = "page-no")]
        page_no: Option<u32>,
        #[arg(long = "page-size")]
        page_size: Option<u32>,
    },
}

/// `role` 子命令（§3）。
#[derive(Debug, Subcommand)]
pub enum RoleAction {
    /// `role add` → `POST /v1/roles`。
    Add {
        /// 角色名（落体）。
        name: String,
    },
    /// `role list` → `GET /v1/roles`。
    List {
        #[arg(long = "page-no")]
        page_no: Option<u32>,
        #[arg(long = "page-size")]
        page_size: Option<u32>,
    },
    /// `role bind <principal> <role>` → `POST /v1/bindings`。
    Bind {
        /// 目标 Principal（落体）。
        principal: String,
        /// 目标角色（落体）。
        role: String,
    },
}

/// `credential` 子命令（§3）。凭据材料只转发控制面、CLI 不在本地经手明文（§4）。
#[derive(Debug, Subcommand)]
pub enum CredentialAction {
    /// `credential add` → `POST /v1/credentials`。
    Add {
        /// 目标资源代号（落体）。
        resource: String,
    },
    /// `credential revoke <id>` → `POST /v1/credentials/{id}/revoke`。
    Revoke {
        /// 凭据 id（路径参数 `{id}`）。
        id: String,
    },
    /// `credential rotate <id>` → `POST /v1/credentials/{id}/rotate`。
    Rotate {
        /// 凭据 id（路径参数 `{id}`）。
        id: String,
    },
}

/// `mode set` 子命令（§3）：四态切换，可带单资源与 ttl。
#[derive(Debug, Subcommand)]
pub enum ModeAction {
    /// `mode set <mode> [--resource] [--ttl]` → `PUT /v1/mode`。
    Set {
        /// 目标模式（`observe|maintain|freeze|normal` 之一，落体）。
        mode: String,
        /// 单资源限定（缺省 = 全局，落体 / 查询）。
        #[arg(long = "resource")]
        resource: Option<String>,
        /// 模式 ttl（落体）。
        #[arg(long = "ttl")]
        ttl: Option<String>,
    },
}

/// `constraint` 子命令（§3）。
#[derive(Debug, Subcommand)]
pub enum ConstraintAction {
    /// `constraint add <res>` → `POST /v1/resources/{code}/constraints`。
    Add {
        /// 资源代号（路径参数 `{code}`）。
        resource: String,
    },
    /// `constraint list <res>` → `GET /v1/resources/{code}/constraints`。
    List {
        /// 资源代号（路径参数 `{code}`）。
        resource: String,
    },
    /// `constraint rm <res> <id>` → `DELETE /v1/resources/{code}/constraints`。
    Rm {
        /// 资源代号（路径参数 `{code}`）。
        resource: String,
        /// 细则 id。
        id: String,
    },
}

/// `condition` 子命令（§3）。
#[derive(Debug, Subcommand)]
pub enum ConditionAction {
    /// `condition add <res>` → `POST /v1/resources/{code}/conditions`。
    Add {
        /// 资源代号（路径参数 `{code}`）。
        resource: String,
    },
    /// `condition list <res>` → `GET /v1/resources/{code}/conditions`。
    List {
        /// 资源代号（路径参数 `{code}`）。
        resource: String,
    },
    /// `condition rm <res> <id>` → `DELETE /v1/resources/{code}/conditions`。
    Rm {
        /// 资源代号（路径参数 `{code}`）。
        resource: String,
        /// 条件 id。
        id: String,
    },
}

/// `deny-note` 子命令（§3，公理六）。
#[derive(Debug, Subcommand)]
pub enum DenyNoteAction {
    /// `deny-note set <res> <verb>` → `POST /v1/resources/{code}/deny-notes`。
    Set {
        /// 资源代号（路径参数 `{code}`）。
        resource: String,
        /// 动词。
        verb: String,
    },
    /// `deny-note list <res>` → `GET /v1/resources/{code}/deny-notes`。
    List {
        /// 资源代号（路径参数 `{code}`）。
        resource: String,
    },
    /// `deny-note rm <res> <verb>` → `DELETE /v1/resources/{code}/deny-notes`。
    Rm {
        /// 资源代号（路径参数 `{code}`）。
        resource: String,
        /// 动词。
        verb: String,
    },
}

/// `settings` 子命令（§3）。
#[derive(Debug, Subcommand)]
pub enum SettingsAction {
    /// `settings get <key>` → `GET /v1/settings/{key}`。
    Get {
        /// 设置项键（路径参数 `{key}`）。
        key: String,
    },
    /// `settings set <key> [<value>]` → `PUT /v1/settings/{key}`。
    Set {
        /// 设置项键（路径参数 `{key}`）。
        key: String,
        /// 设置项值（落体；缺省由命令语义决定）。
        value: Option<String>,
    },
}

/// `approvals` 子命令（§3，6.10）。
#[derive(Debug, Subcommand)]
pub enum ApprovalsAction {
    /// `approvals list` → `GET /v1/approvals`。
    List {
        #[arg(long = "page-no")]
        page_no: Option<u32>,
        #[arg(long = "page-size")]
        page_size: Option<u32>,
    },
    /// `approvals approve <id>` → `POST /v1/approvals/{id}/approve`。
    Approve {
        /// 审批项 id（路径参数 `{id}`）。
        id: String,
    },
    /// `approvals deny <id>` → `POST /v1/approvals/{id}/deny`。
    Deny {
        /// 审批项 id（路径参数 `{id}`）。
        id: String,
    },
}
