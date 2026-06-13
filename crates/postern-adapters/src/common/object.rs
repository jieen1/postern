//! 对象引用规范化（骨架占位，承诺级签名）。
//!
//! 把各协议在 `classify` 中提取的对象统一规范化为
//! [`postern_core::request::ObjectRef`]——postgres 的 `schema.table`、
//! docker_logs 的 `container:<名>`、http 的 `route:<path>`——去重后随
//! `ClassifiedIntent.objects` 返回。规范化的对象集**既供** §3.2 细则判定
//! （`table_allow`/`http_route`/`container_prefix` 等）**又供**内核审计消费，
//! 二者必须看到与定档**完全一致**的对象视图（§3.1「同遍历完成对象提取」）。
//!
//! 三类构造助手（[`table_ref`] / [`route_ref`] / [`container_ref`]）只做与具体
//! 协议无关的字符串规范化与前缀拼装；去重 [`dedup`] 以全序（`BTreeSet`）收敛为
//! 稳定排序、无重复的 `Vec<ObjectRef>`——稳定排序使「判定看到的对象」与「审计记录
//! 的对象」逐字段一致，不因提取顺序漂移（§3.1）。
//!
//! 规范化与去重为纯函数:无内部可变共享态,`&str` 入参、值出参,clippy 净
//! (无 `unwrap`/`expect`/`panic`/`todo!`)。

use postern_core::request::ObjectRef;
use std::collections::BTreeSet;

/// 把 `schema` 与 `table` 规范化为 `schema.table` 形态的 [`ObjectRef`]
/// （postgres `table_allow`/审计的对象维度，§3.1/§3.2）。
pub fn table_ref(schema: &str, table: &str) -> ObjectRef {
    ObjectRef::new(format!("{schema}.{table}"))
}

/// 把 HTTP 请求路径规范化为 `route:<path>` 形态的 [`ObjectRef`]
/// （http `http_route`/审计的对象维度，§3.1）。
pub fn route_ref(path: &str) -> ObjectRef {
    ObjectRef::new(format!("route:{path}"))
}

/// 把容器选择符规范化为 `container:<名>` 形态的 [`ObjectRef`]
/// （docker_logs `container_prefix`/审计的对象维度，§3.1）。
pub fn container_ref(name: &str) -> ObjectRef {
    ObjectRef::new(format!("container:{name}"))
}

/// 以全序（`BTreeSet`）对对象集去重并稳定排序，收敛为无重复的
/// `Vec<ObjectRef>`（§3.1「去重后随 `ClassifiedIntent.objects` 返回」）。
///
/// 全序去重使「判定看到的对象」与「审计记录的对象」逐字段一致、与提取顺序无关。
pub fn dedup(refs: Vec<ObjectRef>) -> Vec<ObjectRef> {
    refs.into_iter()
        .collect::<BTreeSet<ObjectRef>>()
        .into_iter()
        .collect()
}
