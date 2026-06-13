//! 请求规格面（骨架占位）。
//!
//! 职责（07-postern-cli §3.1 步骤 2，F-2/F-6/F-7）：把每个强类型管理意图变体映射到一个
//! **请求规格** `(method, path_template, query, body)`——这是命令与 6.5 端点之间唯一的
//! 映射表，集中一处声明，杜绝散落拼 URL。`path_template` 的路径参数（`{code}`/`{id}`/
//! `{principal}`）由意图字段填充；`query` 收集分页与过滤键（`page_no/page_size/since/
//! principal/kind/decision/window` 等，缺则不带该键，由后端取默认）；`body` 仅写端点有，
//! 由 core 共享 DTO 序列化，写端点期望 `version` 取自先前读取响应原样落入（只透传不自造）。
//!
//! 关键取舍：意图与请求规格分离（非每命令各自拼 HTTP）——映射表集中后"命令 ⊆ 6.5 端点"
//! 这一设计承诺可在一处审视；公共主干只有一份，使"一条命令恰一次往返、无隐式重试 / 保活"
//! （F-3）成为结构性事实。设计承诺：CLI 不得调用 6.5 未列端点、不得自定义私有控制协议。
//!
//! 子模块：`capability`（`--cap <res:verb>` 等本地字面量语法校验）、`query`（分页 / 过滤
//! 查询参数装配，分页交后端、不前端切片）。
pub mod capability;
pub mod query;

use std::collections::BTreeMap;

use self::query::Query;

/// 控制面端点的 HTTP 方法（§3 表 / 6.5 端点的方法列）。core 不暴露 HTTP 方法类型（它是
/// 数据面无关的客户端关切），故在本 unit 就近定义这一封闭集。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
    Put,
    Delete,
}

impl Method {
    /// 方法的规范大写文本，用于装配请求行。穷尽 match，无 `_ =>` 兜底。
    pub fn as_str(self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Post => "POST",
            Method::Put => "PUT",
            Method::Delete => "DELETE",
        }
    }
}

/// 写端点请求体（§3.1 步骤 2、F-7、§3.5）：携命令载荷 + **期望 `version`**。
///
/// `version` 是**搬运型**数据——其唯一来源是先前读取响应、由人作为参数原样供入
/// （`Some(n)`）；CLI **绝不**自读自比、自增、自造。读端点 / 不需乐观锁的写无期望版本时为
/// `None`（不发 `version` 键）。`payload` 是已就绪的命令载荷键值（雪花 id 等恒为字符串）。
///
/// 红线（F-7）：本类型只**接受**外部供入的 `version`，不提供任何"产生 version"的构造路径
/// （无 `next_version`/`bump`/自增方法）——使"version 只透传不自造"成为构造签名可核的事实。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteBody {
    /// 命令载荷键值（如 `principal`/`capability`/`ttl`）。值为不透明字符串原文，雪花 id
    /// 在协议中恒为字符串，CLI 不数值化。
    pub fields: BTreeMap<String, String>,
    /// 期望乐观锁版本：`Some(n)` = 人从先前读取响应取得并原样供入；`None` = 该写不带
    /// `version` 键。CLI 永不在此自造 / 自增。
    pub version: Option<u64>,
}

/// 一条命令映射出的**请求规格**——命令 → HTTP 形态的唯一落点（§3.1 步骤 2、F-2）。
/// 集中一处声明，杜绝散落拼 URL；公共主干据此发起恰一次往返。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestSpec {
    /// HTTP 方法（6.5 端点方法列）。
    pub method: Method,
    /// 路径模板（如 `/v1/grants/temp`、`/v1/resources/{code}/discover`）；`{code}`/`{id}`/
    /// `{principal}` 已由意图字段填充后落入（本类型持已填充结果，非待填模板的悬空占位）。
    pub path_template: String,
    /// 查询参数装配器（分页 + 过滤；缺则不带键，F-6）。
    pub query: Query,
    /// 请求体——**仅写端点有**（`Some`），读端点为 `None`。
    pub body: Option<WriteBody>,
}

/// 构造 `elevate <principal> --cap <res:verb> --ttl <dur>` 的请求规格（F-2，对
/// `docs/examples/06 §3.1`）：`POST /v1/grants/temp`，体含 `principal`/`capability`/`ttl`。
///
/// `elevate` 是**创建型**写——在 `temp_grants` 表插入一行新临时授权（`docs/examples/06 §3.1`）；
/// §3 端点表该行字段恰为 `principal`/`capability`/`ttl`，**不含** `version`（创建无乐观锁前置
/// 读，期望 `version` 只系于后续 `update`/`delete`/`disable` 写，见 [`revoke_grant_spec`] 与
/// §3.5/F-7）。故本写体 `version` 恒为 `None`，CLI 不在创建端点凭空塞入期望版本。
///
/// 本函数**不**校验 `--cap` 字面量（那在 [`capability::parse_cap`]）、**不**下授权判断。
pub fn elevate_spec(principal: &str, _resource: &str, verb: &str, ttl: &str) -> RequestSpec {
    let mut fields = BTreeMap::new();
    fields.insert("principal".to_string(), principal.to_string());
    fields.insert("capability".to_string(), verb.to_string());
    fields.insert("ttl".to_string(), ttl.to_string());

    RequestSpec {
        method: Method::Post,
        path_template: "/v1/grants/temp".to_string(),
        query: Query::default(),
        body: Some(WriteBody {
            fields,
            version: None,
        }),
    }
}

/// 构造 `revoke-grant <id>` 的请求规格（F-2/F-7，对 `docs/examples/06 §3.1` 表行
/// 「主动撤销临时授权」与 §4.2-F/步骤 10 乐观锁语义）：`DELETE /v1/grants/temp/{id}`——
/// 对 `temp_grants` 既有行的**删除型乐观锁写**，请求体携期望 `version`。
///
/// `version` 是该删除命令的期望乐观锁版本（搬运型，F-7、§3.5）：其**唯一来源是先前读取响应**
/// （由人从上一条读命令输出取得并原样供入）；`Some(n)` 原样落入写体（`UPDATE/DELETE … WHERE
/// version=n` 不匹配即 daemon 返回 `409`），`None` 表此命令不携期望版本。CLI **绝不**自读自比、
/// 自增、自造 `version`——本函数只搬运调用方供入值，无任何 `version` 派生路径。
///
/// `{id}` 已由意图字段填充后落入路径模板（本类型持已填充结果）。删除端点无命令载荷字段，
/// 故写体 `fields` 为空——`version` 是该写携带的全部乐观锁前置。
pub fn revoke_grant_spec(id: &str, version: Option<u64>) -> RequestSpec {
    RequestSpec {
        method: Method::Delete,
        path_template: format!("/v1/grants/temp/{id}"),
        query: Query::default(),
        body: Some(WriteBody {
            fields: BTreeMap::new(),
            version,
        }),
    }
}

/// 构造 `audit [--principal] [--since] [--page-no] [--page-size]` 的请求规格（F-2/F-6，对
/// `docs/examples/07 §4.1`）：`GET /v1/audit`，分页与过滤键落 query；缺则不带键。
///
/// 读端点：`body` 恒为 `None`。分页参数以 `Option<u32>` 透传——`None` 即不带该键。
pub fn audit_spec(
    principal: Option<&str>,
    since: Option<&str>,
    page_no: Option<u32>,
    page_size: Option<u32>,
) -> RequestSpec {
    let mut filters = BTreeMap::new();
    if let Some(principal) = principal {
        filters.insert("principal".to_string(), principal.to_string());
    }
    if let Some(since) = since {
        filters.insert("since".to_string(), since.to_string());
    }

    RequestSpec {
        method: Method::Get,
        path_template: "/v1/audit".to_string(),
        query: Query {
            page_no,
            page_size,
            filters,
        },
        body: None,
    }
}
