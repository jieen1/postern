//! 强类型管理意图（设计承诺级桩）。
//!
//! 职责（07-postern-cli §3.1 步骤 1→2，F-2/L-12）：clap 解析产物——每个命令组一个枚举变体，
//! 携已做语法层校验的本地参数。意图是命令树与请求规格之间的中间表示：意图 → 请求规格
//! `(method, path_template, query, body)` 的映射集中**一处**声明（[`ManagementIntent::to_request_spec`]），
//! 这是命令与 6.5 端点之间唯一的映射表，杜绝散落拼 URL，使"命令 ⊆ 6.5 端点"可在一处审视。
//!
//! 关键取舍（§3.1）：**意图与请求构造分离**——意图只承载"人想做什么"的已校验形态，不含
//! 任何 HTTP 拼装或安全判断；意图不持任何本地状态（§7 零本地状态），命令结束即随进程退出
//! 销毁。
//!
//! 命名红线（L-12 / 数据面分离）：本枚举命名为 `ManagementIntent`——**绝不**命名为或引用
//! core 数据面的 `Intent`（`postern_core::request::Intent` 是归一化求值管线 [0]~[6] 的产物，
//! CLI 不在数据面、不参与求值，引用它即构造签名红线）。`mode set` 之外 `freeze` 单独成意图，
//! 但二者**同映射** `PUT /v1/mode`（全局冻结别名，§3 表）；映射表里不出现任何 6.5 未列端点、
//! 不触达数据面 `postern_surface` / `Adapter::discover`（L-12）。

use std::collections::BTreeMap;

use crate::error::CliError;
use crate::reqspec::query::Query;
use crate::reqspec::{audit_spec, elevate_spec, revoke_grant_spec, Method, RequestSpec, WriteBody};

/// 一条命令解析后的**强类型管理意图**（§3.1，每个命令组一个语义变体）。携已做语法层
/// 校验（缺参 / 互斥 / 格式 / `--cap` 字面量）的本地参数；不含任何 HTTP 拼装或授权判断。
///
/// 变体与 §3 全表 22 命令组对应：每条命令解析为恰一个变体，再经 [`Self::to_request_spec`]
/// 映射到恰一个 6.5 控制面端点（`mcp-stdio` 例外——它是数据面字节桥，不产请求规格、不入
/// 本枚举的请求映射）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManagementIntent {
    /// `daemon status` → `GET /v1/health`。
    DaemonStatus,
    /// `daemon stop` → `POST /v1/shutdown`。
    DaemonStop,
    /// `resource add <codename>` → `POST /v1/resources`。
    ResourceAdd { codename: String },
    /// `resource list` → `GET /v1/resources`（分页透传，缺则不带键 F-6）。
    ResourceList {
        page_no: Option<u32>,
        page_size: Option<u32>,
    },
    /// `resource disable <code>` → 资源停用（携期望 `version` 回传 F-7）。
    ResourceDisable { code: String, version: Option<u64> },
    /// `resource discover <code>` → **控制面** `POST /v1/resources/{code}/discover`
    /// （L-12：接入侧探测，**非**数据面 surface / `Adapter::discover`）。
    ResourceDiscover { code: String },
    /// `principal add <codename>` → `POST /v1/principals`。
    PrincipalAdd { codename: String },
    /// `principal list` → `GET /v1/principals`。
    PrincipalList {
        page_no: Option<u32>,
        page_size: Option<u32>,
    },
    /// `role add <name>` → `POST /v1/roles`。
    RoleAdd { name: String },
    /// `role list` → `GET /v1/roles`。
    RoleList {
        page_no: Option<u32>,
        page_size: Option<u32>,
    },
    /// `role bind <principal> <role>` → `POST /v1/bindings`。
    RoleBind { principal: String, role: String },
    /// `credential add <resource>` → `POST /v1/credentials`（凭据材料只转发，§4）。
    CredentialAdd { resource: String },
    /// `credential revoke <id>` → `POST /v1/credentials/{id}/revoke`。
    CredentialRevoke { id: String },
    /// `credential rotate <id>` → `POST /v1/credentials/{id}/rotate`。
    CredentialRotate { id: String },
    /// `grants <principal>` → `GET /v1/grants/{principal}`。
    Grants { principal: String },
    /// `elevate <principal> --cap <res:verb> --ttl <dur>` → `POST /v1/grants/temp`
    /// （体含 `principal`/`capability`/`ttl`，F-2）。`verb` 已经 `--cap` 字面量校验。
    Elevate {
        principal: String,
        resource: String,
        verb: String,
        ttl: String,
    },
    /// `revoke-grant <id>` → `DELETE /v1/grants/temp/{id}`（携期望 `version` F-7）。
    RevokeGrant { id: String, version: Option<u64> },
    /// `mode set <mode> [--resource] [--ttl]` → `PUT /v1/mode`。
    ModeSet {
        mode: String,
        resource: Option<String>,
        ttl: Option<String>,
    },
    /// `freeze`（= `mode set freeze` 全局别名）→ `PUT /v1/mode`（§3 表，同端点）。
    Freeze,
    /// `constraint add|list|rm` → `/v1/resources/{code}/constraints`。
    ConstraintAdd { resource: String },
    /// `constraint list <res>` → `GET /v1/resources/{code}/constraints`。
    ConstraintList { resource: String },
    /// `constraint rm <res> <id>` → `DELETE /v1/resources/{code}/constraints`。
    ConstraintRm { resource: String, id: String },
    /// `condition add <res>` → `POST /v1/resources/{code}/conditions`。
    ConditionAdd { resource: String },
    /// `condition list <res>` → `GET /v1/resources/{code}/conditions`。
    ConditionList { resource: String },
    /// `condition rm <res> <id>` → `DELETE /v1/resources/{code}/conditions`。
    ConditionRm { resource: String, id: String },
    /// `deny-note set <res> <verb>` → `POST /v1/resources/{code}/deny-notes`。
    DenyNoteSet { resource: String, verb: String },
    /// `deny-note list <res>` → `GET /v1/resources/{code}/deny-notes`。
    DenyNoteList { resource: String },
    /// `deny-note rm <res> <verb>` → `DELETE /v1/resources/{code}/deny-notes`。
    DenyNoteRm { resource: String, verb: String },
    /// `settings get <key>` → `GET /v1/settings/{key}`。
    SettingsGet { key: String },
    /// `settings set <key> [<value>]` → `PUT /v1/settings/{key}`。
    SettingsSet { key: String, value: Option<String> },
    /// `approvals list` → `GET /v1/approvals`。
    ApprovalsList {
        page_no: Option<u32>,
        page_size: Option<u32>,
    },
    /// `approvals approve <id>` → `POST /v1/approvals/{id}/approve`。
    ApprovalsApprove { id: String },
    /// `approvals deny <id>` → `POST /v1/approvals/{id}/deny`。
    ApprovalsDeny { id: String },
    /// `denials [--window]` → `GET /v1/denials/summary`（窗口落查询键 `window`）。
    Denials { window: Option<String> },
    /// `audit [...]` → `GET /v1/audit`（过滤 + 分页落查询键，缺则不带键 F-6）。
    Audit {
        principal: Option<String>,
        since: Option<String>,
        page_no: Option<u32>,
        page_size: Option<u32>,
    },
    /// `verify` → `POST /v1/verify`（红队自检触发，执行在 daemon）。
    Verify,
    /// `export [<file>]` → `POST /v1/export`（响应 TOML 写文件 / stdout，渲染落点改址）。
    Export { file: Option<String> },
    /// `import <file>` → `POST /v1/import`（文件原样作 body；CLI 不解析 TOML 语义，§3.1）。
    Import { file: String },
}

impl ManagementIntent {
    /// 把管理意图映射到**恰一个** 6.5 控制面端点的请求规格（§3.1 步骤 2，F-2）。这是命令与
    /// 端点之间**唯一**映射落点——集中一处声明，杜绝散落拼 URL；任何变体只能映射到 §3 表
    /// （= 6.5）列出的端点，**绝不**自定义私有控制协议、**绝不**触达数据面端点（L-12）。
    ///
    /// 路径参数（`{code}`/`{id}`/`{principal}`/`{key}`）由意图字段填充；查询键（分页 +
    /// 过滤）缺则不带键（F-6）；写体携命令载荷 + 期望 `version`（只透传不自造，F-7）。
    /// `freeze` 与 `mode set freeze` 同映射 `PUT /v1/mode`（全局冻结别名）。
    ///
    /// 失败：理论上每个 22 组变体都有恒定映射、不产生失败；返回 `Result` 仅为前向兼容
    /// （未来携带本地形态检查的变体——如 `import` 文件不可读 / 空——可在此返回 `LocalReject`，
    /// 仍属 §3.6 本地语法拒绝类、对 `control.sock` 零请求）。
    pub fn to_request_spec(&self) -> Result<RequestSpec, CliError> {
        // 命令 ⊆ 6.5：每个变体落到 §3 表（= 详细设计 6.5）列出的恰一个端点。复用 `reqspec`
        // 已有的 `elevate_spec`/`revoke_grant_spec`/`audit_spec`；其余变体在此就地装配 RequestSpec。
        let spec = match self {
            // daemon → 生命周期端点（6.5 末行）。
            ManagementIntent::DaemonStatus => read(Method::Get, "/v1/health"),
            ManagementIntent::DaemonStop => write_no_body(Method::Post, "/v1/shutdown"),

            // resources（6.5：`POST/GET /v1/resources` · `POST /v1/resources/{code}/discover`）。
            ManagementIntent::ResourceAdd { codename } => {
                write_fields(Method::Post, "/v1/resources", [("codename", codename)])
            }
            ManagementIntent::ResourceList { page_no, page_size } => {
                read_paged(Method::Get, "/v1/resources", *page_no, *page_size)
            }
            ManagementIntent::ResourceDisable { code, version } => RequestSpec {
                method: Method::Post,
                path_template: format!("/v1/resources/{code}/disable"),
                query: Query::default(),
                body: Some(WriteBody {
                    fields: BTreeMap::new(),
                    version: *version,
                }),
            },
            // L-12：控制面接入侧探测端点，**非**数据面 surface。
            ManagementIntent::ResourceDiscover { code } => {
                write_no_body(Method::Post, &format!("/v1/resources/{code}/discover"))
            }

            // principals（6.5：`POST/GET /v1/principals`）。
            ManagementIntent::PrincipalAdd { codename } => {
                write_fields(Method::Post, "/v1/principals", [("codename", codename)])
            }
            ManagementIntent::PrincipalList { page_no, page_size } => {
                read_paged(Method::Get, "/v1/principals", *page_no, *page_size)
            }

            // roles & bindings（6.5：`POST/GET /v1/roles` · `/v1/bindings`）。
            ManagementIntent::RoleAdd { name } => {
                write_fields(Method::Post, "/v1/roles", [("name", name)])
            }
            ManagementIntent::RoleList { page_no, page_size } => {
                read_paged(Method::Get, "/v1/roles", *page_no, *page_size)
            }
            ManagementIntent::RoleBind { principal, role } => write_fields(
                Method::Post,
                "/v1/bindings",
                [("principal", principal), ("role", role)],
            ),

            // credentials（6.5：`POST /v1/credentials` · `.../{id}/revoke` · `.../rotate`）。
            ManagementIntent::CredentialAdd { resource } => {
                write_fields(Method::Post, "/v1/credentials", [("resource", resource)])
            }
            ManagementIntent::CredentialRevoke { id } => {
                write_no_body(Method::Post, &format!("/v1/credentials/{id}/revoke"))
            }
            ManagementIntent::CredentialRotate { id } => {
                write_no_body(Method::Post, &format!("/v1/credentials/{id}/rotate"))
            }

            // grants 视图（6.5：`GET /v1/grants/{principal}`）。
            ManagementIntent::Grants { principal } => {
                read(Method::Get, &format!("/v1/grants/{principal}"))
            }

            // 临时授权（6.5：`POST /v1/grants/temp` · `DELETE /v1/grants/temp/{id}`）。复用
            // reqspec 已落地构造器，保持与 reqspec unit 的端点承诺单一来源。
            ManagementIntent::Elevate {
                principal,
                resource,
                verb,
                ttl,
            } => elevate_spec(principal, resource, verb, ttl),
            ManagementIntent::RevokeGrant { id, version } => revoke_grant_spec(id, *version),

            // 模式切换（6.5：`PUT /v1/mode`）。`freeze` 是 `mode set freeze` 全局别名，同端点。
            ManagementIntent::ModeSet {
                mode,
                resource,
                ttl,
            } => {
                let mut fields = BTreeMap::new();
                fields.insert("mode".to_string(), mode.clone());
                if let Some(resource) = resource {
                    fields.insert("resource".to_string(), resource.clone());
                }
                if let Some(ttl) = ttl {
                    fields.insert("ttl".to_string(), ttl.clone());
                }
                RequestSpec {
                    method: Method::Put,
                    path_template: "/v1/mode".to_string(),
                    query: Query::default(),
                    body: Some(WriteBody {
                        fields,
                        version: None,
                    }),
                }
            }
            ManagementIntent::Freeze => {
                let mut fields = BTreeMap::new();
                fields.insert("mode".to_string(), "freeze".to_string());
                RequestSpec {
                    method: Method::Put,
                    path_template: "/v1/mode".to_string(),
                    query: Query::default(),
                    body: Some(WriteBody {
                        fields,
                        version: None,
                    }),
                }
            }

            // 细则（6.5：`POST/GET/DELETE /v1/resources/{code}/constraints`）。
            ManagementIntent::ConstraintAdd { resource } => write_no_body(
                Method::Post,
                &format!("/v1/resources/{resource}/constraints"),
            ),
            ManagementIntent::ConstraintList { resource } => read(
                Method::Get,
                &format!("/v1/resources/{resource}/constraints"),
            ),
            ManagementIntent::ConstraintRm { resource, id } => write_fields(
                Method::Delete,
                &format!("/v1/resources/{resource}/constraints"),
                [("id", id)],
            ),

            // 条件谓词（6.5：`POST/GET/DELETE /v1/resources/{code}/conditions`）。
            ManagementIntent::ConditionAdd { resource } => write_no_body(
                Method::Post,
                &format!("/v1/resources/{resource}/conditions"),
            ),
            ManagementIntent::ConditionList { resource } => {
                read(Method::Get, &format!("/v1/resources/{resource}/conditions"))
            }
            ManagementIntent::ConditionRm { resource, id } => write_fields(
                Method::Delete,
                &format!("/v1/resources/{resource}/conditions"),
                [("id", id)],
            ),

            // 拒绝注记（6.5：`POST/GET/DELETE /v1/resources/{code}/deny-notes`）。
            ManagementIntent::DenyNoteSet { resource, verb } => write_fields(
                Method::Post,
                &format!("/v1/resources/{resource}/deny-notes"),
                [("capability", verb)],
            ),
            ManagementIntent::DenyNoteList { resource } => {
                read(Method::Get, &format!("/v1/resources/{resource}/deny-notes"))
            }
            ManagementIntent::DenyNoteRm { resource, verb } => write_fields(
                Method::Delete,
                &format!("/v1/resources/{resource}/deny-notes"),
                [("capability", verb)],
            ),

            // 设置项（6.5：`GET/PUT /v1/settings/{key}`）。
            ManagementIntent::SettingsGet { key } => {
                read(Method::Get, &format!("/v1/settings/{key}"))
            }
            ManagementIntent::SettingsSet { key, value } => {
                let mut fields = BTreeMap::new();
                if let Some(value) = value {
                    fields.insert("value".to_string(), value.clone());
                }
                RequestSpec {
                    method: Method::Put,
                    path_template: format!("/v1/settings/{key}"),
                    query: Query::default(),
                    body: Some(WriteBody {
                        fields,
                        version: None,
                    }),
                }
            }

            // 审批挂起（6.5：`GET /v1/approvals` · `POST /v1/approvals/{id}/approve|deny`）。
            ManagementIntent::ApprovalsList { page_no, page_size } => {
                read_paged(Method::Get, "/v1/approvals", *page_no, *page_size)
            }
            ManagementIntent::ApprovalsApprove { id } => {
                write_no_body(Method::Post, &format!("/v1/approvals/{id}/approve"))
            }
            ManagementIntent::ApprovalsDeny { id } => {
                write_no_body(Method::Post, &format!("/v1/approvals/{id}/deny"))
            }

            // 拒绝聚合（6.5：`GET /v1/denials/summary?window=7d`）。
            ManagementIntent::Denials { window } => {
                let mut filters = BTreeMap::new();
                if let Some(window) = window {
                    filters.insert("window".to_string(), window.clone());
                }
                RequestSpec {
                    method: Method::Get,
                    path_template: "/v1/denials/summary".to_string(),
                    query: Query {
                        page_no: None,
                        page_size: None,
                        filters,
                    },
                    body: None,
                }
            }

            // 审计查询（6.5：`GET /v1/audit?...`）。复用 reqspec 已落地构造器。
            ManagementIntent::Audit {
                principal,
                since,
                page_no,
                page_size,
            } => audit_spec(principal.as_deref(), since.as_deref(), *page_no, *page_size),

            // 红队自检触发（6.5：`POST /v1/verify`）。
            ManagementIntent::Verify => write_no_body(Method::Post, "/v1/verify"),

            // 声明式导出 / 导入（6.5：`POST /v1/export` · `POST /v1/import`）。请求体来源 /
            // 渲染落点改址在 dispatch 主干处理；映射只钉端点（§3.1）。
            ManagementIntent::Export { .. } => write_no_body(Method::Post, "/v1/export"),
            ManagementIntent::Import { .. } => write_no_body(Method::Post, "/v1/import"),
        };
        Ok(spec)
    }
}

/// 读端点请求规格：无查询、无体（路径已由调用方填充路径参数）。
fn read(method: Method, path: &str) -> RequestSpec {
    RequestSpec {
        method,
        path_template: path.to_string(),
        query: Query::default(),
        body: None,
    }
}

/// 集合读端点请求规格：分页键透传（缺则不带键，F-6），无体。
fn read_paged(
    method: Method,
    path: &str,
    page_no: Option<u32>,
    page_size: Option<u32>,
) -> RequestSpec {
    RequestSpec {
        method,
        path_template: path.to_string(),
        query: Query {
            page_no,
            page_size,
            filters: BTreeMap::new(),
        },
        body: None,
    }
}

/// 无载荷写端点请求规格：空体（无 fields、无期望 `version`）。
fn write_no_body(method: Method, path: &str) -> RequestSpec {
    RequestSpec {
        method,
        path_template: path.to_string(),
        query: Query::default(),
        body: Some(WriteBody {
            fields: BTreeMap::new(),
            version: None,
        }),
    }
}

/// 携命令载荷字段的写端点请求规格：`fields` 由 `(key, value)` 对装配，无期望 `version`
/// （创建型写无乐观锁前置读，F-7）。
fn write_fields<'a, const N: usize>(
    method: Method,
    path: &str,
    pairs: [(&'a str, &'a str); N],
) -> RequestSpec {
    let mut fields = BTreeMap::new();
    for (key, value) in pairs {
        fields.insert(key.to_string(), value.to_string());
    }
    RequestSpec {
        method,
        path_template: path.to_string(),
        query: Query::default(),
        body: Some(WriteBody {
            fields,
            version: None,
        }),
    }
}
