//! 查询参数装配（骨架占位）。
//!
//! 职责（07-postern-cli §3.4，F-6、契约 DB_PAGINATION_MANDATORY）：把集合命令的分页与
//! 过滤参数装配进 query string——`page_no/page_size` 直接透传，**不给则不带该键**，由
//! daemon 取默认（20，上限 200 由 daemon 钳制）；过滤键（`since/principal/kind/decision/
//! window` 等）按命令携带。键值精确、顺序不限。
//!
//! 关键纪律：CLI 端**不存在**"取回全量再本地切片"的代码路径——分页职责整体在后端，
//! 客户端只透传游标、不持有分页语义（构造签名可核，F-6）。
//!
//! 构造签名要点（F-6）：分页字段在类型层是 `Option<u32>`——`None` 表"该命令未给该键"，
//! 装配时整键省略（既不发 `page_no=` 空串、也不替人填默认 `0`/`20`）；默认值由 daemon
//! 取。这把"缺则不带键、由后端取默认"做成结构性事实而非运行期约定。

use std::collections::BTreeMap;

/// 一条集合命令的查询参数装配器（§3.4、F-6）。分页键以 `Option<u32>` 承载——`None` 即
/// "该命令未给该键"，[`Query::into_pairs`] 时整键省略；过滤键按需 `insert`。
///
/// 设计取舍：分页字段**不**用 `u32` 带"魔法默认值"——那会让 CLI 替 daemon 产出一个
/// `page_no=`/`page_size=` 键，破坏 F-6"缺省由后端取默认"。故缺省必须是类型层的 `None`。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Query {
    /// 页号（1-based）。`None` = 命令未给 `--page-no` → 装配时不带 `page_no` 键。
    pub page_no: Option<u32>,
    /// 页大小。`None` = 命令未给 `--page-size` → 装配时不带 `page_size` 键，由 daemon 取
    /// 默认 20（上限 200 由 daemon 钳制，CLI 不钳）。
    pub page_size: Option<u32>,
    /// 过滤键（`since/principal/kind/decision/window` 等），按命令携带；缺则不带该键。
    /// 键值为不透明字符串原文，CLI 不解释其语义。
    pub filters: BTreeMap<String, String>,
}

impl Query {
    /// 把装配好的查询参数展平为 `(key, value)` 对集合（§3.4）。**省略**任何 `None` 的分页键
    /// （不产出 `page_no=`/`page_size=` 空串、不替人填默认值）；`Some(n)` 落 `n` 的十进制
    /// 文本；过滤键原样并入。顺序不要求（消费侧按键比对）。
    ///
    /// F-6 结构性事实：本函数是分页键"缺则不带键"的唯一落点——`None` 在此被丢弃，绝无
    /// 旁路把缺省转成一个发往 daemon 的键。
    pub fn into_pairs(self) -> BTreeMap<String, String> {
        let mut pairs = self.filters;
        if let Some(page_no) = self.page_no {
            pairs.insert("page_no".to_string(), page_no.to_string());
        }
        if let Some(page_size) = self.page_size {
            pairs.insert("page_size".to_string(), page_size.to_string());
        }
        pairs
    }
}
