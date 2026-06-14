//! 控制面 handler 骨架（按域分文件）：axum 提取器 → [`endpoints`](super::endpoints) → 响应。
//!
//! 每个 handler 取 [`State<ControlState>`](axum::extract::State) + 分页/Json 提取器，调
//! [`endpoints::list`](super::endpoints::list) / [`endpoints::write`](super::endpoints::write)
//! 跑读/写端点逻辑，把结果装配为 axum 响应：
//! - **读**：`Page<serde_json::Value>` → `200 Json`（`items` 信封，F-6）；store 读失败 → 500 +
//!   错误信封 [`ApiErrorBody`](super::dto::ApiErrorBody)。
//! - **写**：[`WriteHttp`](super::endpoints::WriteHttp) → `Committed` 200 + [`WriteAck`](super::dto::WriteAck)
//!   （`policy_rev` 字符串）/ `Conflict` 409 + 错误信封 / `Failed` 500 + 错误信封。
//!
//! 同步 store/audit 调用经 [`endpoints`](super::endpoints) 内部 spawn_blocking 边界（§5）。
//! 写 handler 须把控制面来源 [`Origin`](postern_core::request::ConnOrigin) 透传给 `endpoints::write`
//! （三联动审计支需要 origin）——来源由控制面 listener（shells/）经 SO_PEERCRED 采集，本目录
//! **非** shells，故以别名读、绝不构造字面来源类型（SEC_CONSTRUCTION_SITES）。
//!
//! **本波次为骨架**：提取器签名 + endpoints 接线形态已定，响应装配体留 `unimplemented!()`
//! 占位（保证编译、RED 测试可挂）。router.rs 把对应路由从 stub_handler 改挂这些骨架 handler。

pub mod bindings;
pub mod misc;
pub mod mode_grants;
pub mod principals;
pub mod resources;
pub mod roles;

use serde::Deserialize;

use postern_core::page::PageQuery;

use super::endpoints::page_query;

/// 集合 GET 的可选分页查询参数（axum `Query` 提取）。
///
/// 两者皆可缺（types.ts 集合 GET 的 `page_no`/`page_size` 均可选）；缺省/钳制委托
/// [`page_query`]（缺省 20、钳 200，F-6）。core [`PageQuery`] 两字段必填，故此处用可选包装。
#[derive(Debug, Clone, Copy, Default, Deserialize)]
pub struct PageParams {
    /// 页号（可缺，缺省 1）。
    pub page_no: Option<u32>,
    /// 页大小（可缺，缺省 20、钳 200）。
    pub page_size: Option<u32>,
}

impl PageParams {
    /// 解析为已缺省填充 + 钳制的 [`PageQuery`]（委托 [`page_query`] 唯一钳制点）。
    pub fn to_query(self) -> PageQuery {
        page_query(self.page_no, self.page_size)
    }
}
