//! `common-scaffold` 单元行为测试（RED）。
//!
//! 覆盖两件事:
//!
//! 1. **脚手架存在性（§8 F-1）**:三适配器(postgres / docker_logs / http)可构造、
//!    作为 `dyn Adapter` 对象安全、`protocol()` 各返回登记键。这是「整 crate 任何单元
//!    的 cargo test 都能编译 lib」(B-6 整 crate 编译)的最小可观察印证。
//! 2. **`common::object` 对象规范化助手**:三类构造助手（`table_ref` / `route_ref` /
//!    `container_ref`）把各协议提取的对象统一规范化为 [`ObjectRef`](§3.1 对象提取)；
//!    `dedup` 以全序去重 + 稳定排序收敛为无重复 `Vec<ObjectRef>`(§3.1「去重后随
//!    `ClassifiedIntent.objects` 返回」)。规范化产物**既供** §3.2 细则判定**又供**
//!    内核审计消费,二者必须看到逐字段一致的对象视图,故断言精确到具体字符串值。
//!
//! 本单元只立骨架与公共助手(§8 F-1「本单元只立骨架与公共助手」),不含任何协议级
//! 归类语义,故测试 .rs 零协议关键字标记。

use postern_adapters::common::object;
use postern_core::plugin::Adapter;
use postern_core::request::ObjectRef;

/// §8 F-1:三适配器可构造且作为 `dyn Adapter` 对象安全,`protocol()` 返回登记键。
///
/// 这是骨架阶段「整 crate 可编译 + Adapter trait 对象安全」的结构存在性判定:缺一
/// 方法或签名不符即编译失败,protocol 名不符即此处红。
#[test]
fn adapters_construct_as_trait_objects() {
    let pg: &dyn Adapter = &postern_adapters::postgres::PostgresAdapter;
    let dl: &dyn Adapter = &postern_adapters::docker_logs::DockerLogsAdapter;
    let http: &dyn Adapter = &postern_adapters::http::HttpAdapter;

    assert_eq!(pg.protocol(), "postgres", "postgres 登记键");
    assert_eq!(dl.protocol(), "docker_logs", "docker_logs 登记键");
    assert_eq!(http.protocol(), "http", "http 登记键");
}

/// §3.1 对象提取:`table_ref(schema, table)` 规范化为 `schema.table` 形态。
///
/// postgres 归类把 `ObjectName` 规范化为 `schema.table`(F-2 期望
/// `objects=["public.orders"]`);本助手是其属主,断言精确到字符串值。
#[test]
fn table_ref_normalizes_schema_dot_table() {
    let cases: &[(&str, &str, &str)] = &[
        ("public", "orders", "public.orders"),
        ("billing", "invoice", "billing.invoice"),
        ("analytics", "daily_rollup", "analytics.daily_rollup"),
    ];
    for (schema, table, expected) in cases {
        let got = object::table_ref(schema, table);
        assert_eq!(
            got.as_str(),
            *expected,
            "table_ref({schema:?}, {table:?}) 须规范化为 schema.table 形态",
        );
    }
}

/// §3.1 对象提取:`route_ref(path)` 规范化为 `route:<path>` 形态。
///
/// http 归类把请求路径规范化为 `route:<path>` 供 `http_route` 细则与审计消费
/// (F-7 期望 `objects=[route:/api/orders]`);断言精确到带前缀的字符串值。
#[test]
fn route_ref_prefixes_with_route_marker() {
    let cases: &[(&str, &str)] = &[
        ("/api/orders", "route:/api/orders"),
        ("/v1/health", "route:/v1/health"),
        ("/", "route:/"),
    ];
    for (path, expected) in cases {
        let got = object::route_ref(path);
        assert_eq!(
            got.as_str(),
            *expected,
            "route_ref({path:?}) 须以 route: 前缀拼装",
        );
    }
}

/// §3.1 对象提取:`container_ref(name)` 规范化为 `container:<名>` 形态。
///
/// docker_logs 归类把容器选择符规范化为 `container:<名>` 供 `container_prefix`
/// 细则与审计消费(F-6 期望 `objects=["container:<名>"]`);断言精确到带前缀的值。
#[test]
fn container_ref_prefixes_with_container_marker() {
    let cases: &[(&str, &str)] = &[
        ("app-order", "container:app-order"),
        ("payment-worker", "container:payment-worker"),
        ("db-primary", "container:db-primary"),
    ];
    for (name, expected) in cases {
        let got = object::container_ref(name);
        assert_eq!(
            got.as_str(),
            *expected,
            "container_ref({name:?}) 须以 container: 前缀拼装",
        );
    }
}

/// §3.1 去重:`dedup` 把重复对象收敛为无重复集合。
///
/// 同一对象在语句树内多次触达(如同表在多处出现)只应保留一份,供细则全称量化
/// 判定与审计记录消费一致的对象视图。断言去重后无任何重复元素。
#[test]
fn dedup_removes_duplicates() {
    let input = vec![
        ObjectRef::new("public.orders"),
        ObjectRef::new("public.orders"),
        ObjectRef::new("public.customers"),
        ObjectRef::new("public.orders"),
    ];
    let got = object::dedup(input);
    assert_eq!(
        got,
        vec![
            ObjectRef::new("public.customers"),
            ObjectRef::new("public.orders"),
        ],
        "dedup 须去重为两个不同对象",
    );
}

/// §3.1 去重:`dedup` 以全序(`BTreeSet`)稳定排序,产出与提取顺序无关的稳定视图。
///
/// 「判定看到的对象」与「审计记录的对象」须逐字段一致、不因提取顺序漂移(§3.1):
/// 故 dedup 对乱序输入恒产出同一字典序结果。两次不同顺序的同一集合须得到相同 Vec。
#[test]
fn dedup_sorts_stably_regardless_of_input_order() {
    let forward = object::dedup(vec![
        ObjectRef::new("container:app-order"),
        ObjectRef::new("route:/api/orders"),
        ObjectRef::new("public.orders"),
    ]);
    let shuffled = object::dedup(vec![
        ObjectRef::new("route:/api/orders"),
        ObjectRef::new("public.orders"),
        ObjectRef::new("container:app-order"),
    ]);

    let expected = vec![
        ObjectRef::new("container:app-order"),
        ObjectRef::new("public.orders"),
        ObjectRef::new("route:/api/orders"),
    ];
    assert_eq!(forward, expected, "dedup 须按字典序稳定排序");
    assert_eq!(
        forward, shuffled,
        "不同提取顺序的同一对象集 dedup 后须逐字段相等(稳定视图)",
    );
}

/// §3.1 去重:空输入产出空 `Vec`(边界,无 panic、无伪造元素)。
#[test]
fn dedup_empty_yields_empty() {
    let got = object::dedup(Vec::new());
    assert!(got.is_empty(), "空对象集 dedup 后仍为空");
}

/// §3.1/§3.2:三类助手 + dedup 端到端——异协议对象混入同一集合,去重排序后
/// 得到稳定、无重复、逐字段精确的 `Vec<ObjectRef>`(细则与审计共享同一视图)。
#[test]
fn helpers_compose_into_stable_deduped_view() {
    let refs = object::dedup(vec![
        object::table_ref("public", "orders"),
        object::route_ref("/api/orders"),
        object::container_ref("app-order"),
        object::table_ref("public", "orders"),
    ]);
    let got: Vec<&str> = refs.iter().map(ObjectRef::as_str).collect();
    assert_eq!(
        got,
        vec!["container:app-order", "public.orders", "route:/api/orders"],
        "三协议对象混入后须去重 + 字典序稳定排序,且各保留其规范化前缀",
    );
}
