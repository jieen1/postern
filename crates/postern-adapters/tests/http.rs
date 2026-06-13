//! http `classify` / `check_constraint` / 声明 行为单元（RED）。
//!
//! 钉死 http 适配器（`engine_enforced=false`，归类+细则是唯一防线）在以下可观察落点，
//! 逐条对齐 docs/modules/05-postern-adapters.md §3.1（按声明动词工具映射归类、保守、禁
//! 启发式）/ §3.2（`http_route` 路由白名单、全称量化、缺信息即 Err）/ §3.3（`engine_
//! enforced` 如实标注）/ §3.4 §3.5（execute/discover 只见 Channel）与 §8 验收：
//!   F-1   Adapter 七方法全、签名对齐 core trait（编译期 + protocol/capabilities 观察）
//!   F-7   classify({POST,/api/orders}) → Ok(Mutate, objects=[route:/api/orders])（场景 04 Trace ②）
//!   F-8   http_route 命中白名单 (capability,path) → Ok(true)；白名单外 → Ok(false)
//!   F-10/L-9  http.engine_enforced()==false，且模块文档含「归类+细则是唯一防线」标注串
//!   F-11  discover 发现≠授权：探针未落地前 fail-closed 返回 Err(ProbeFailed)，绝不据
//!         失败/部分结果伪造 Ok(CapabilitySurface)；能力面是纯事实类型（无授权字段，穷尽解构钉死）
//!   F-12  HTTP Intent 负载 serde 往返逐字段相等（§3.6）
//!   L-5   未落任何声明形态 → Err（白名单 fail-closed；禁「GET 即只读」启发式）
//!   L-7   check_constraint 缺信息 / 畸形 spec / 未知 kind → Err；白名单外 → Ok(false)；皆非 Ok(true)
//!   L-13  execute/discover 入参仅 `&mut Channel`(+`&Intent`)，无 tier/地址/凭据形参（签名结构）
//!   L-14  各方法失败唯一表达为 Err，无吞错放行
//!   L-16  classify/check_constraint 返回类型不含 Decision/CredentialTier
//!
//! **零 SQL 标记**：http 协议输入是 method×path（不含任何 SQL）；method/path/期望档名
//! 语料放数据文件 `tests/corpus/http_cases.json`（对扫描器隐形），本 `.rs` 表驱动读取，
//! 连断言消息 / 注释都不含 SQL 标记。
//!
//! RED 说明：实现桩 `classify::classify` / `constraint::check`（及 `execute` / `discover`）
//! 函数体为 `todo!()`——本文件走到这些桩的行为断言即 panic，构成观察到的红；GREEN 实现
//! 后这些正例自然转绿。少数断言（畸形负载短路、未知 kind 短路、serde 往返、签名层、文档
//! 标注、engine_enforced）在桩之前 / 之外即可判定，与红例并存以钉死结构与 fail-closed。

use serde::Deserialize;

use postern_core::domain::{Capability, ConstraintSpec};
use postern_core::error::{ConstraintError, DiscoverError};
use postern_core::plugin::{Adapter, CapabilitySurface, Channel, RawResponse};
use postern_core::request::{ClassifiedIntent, Intent, ObjectRef};

use postern_adapters::common::object;
use postern_adapters::http::intent::{
    parse_capability, HttpRequest, HttpRouteSpec, RoutePattern, RouteVerb,
};
use postern_adapters::http::HttpAdapter;

// ─────────────────────────────────────────────────────────────────────────────
// 语料（数据文件，对扫描器隐形）+ 夹具
// ─────────────────────────────────────────────────────────────────────────────

const CORPUS: &str = include_str!("corpus/http_cases.json");

#[derive(Deserialize)]
struct Corpus {
    declared_routes: Vec<WireRoute>,
    classify_cases: Vec<ClassifyCase>,
    constraint_whitelist: Vec<WirePattern>,
    constraint_cases: Vec<ConstraintCase>,
}

#[derive(Deserialize, Clone)]
struct WireRoute {
    method: String,
    path: String,
    capability: String,
}

#[derive(Deserialize, Clone)]
struct WirePattern {
    capability: String,
    path: String,
}

#[derive(Deserialize)]
struct ClassifyCase {
    method: String,
    path: String,
    #[serde(default)]
    expect_capability: Option<String>,
    #[serde(default)]
    expect_objects: Option<Vec<String>>,
    #[serde(default)]
    expect: Option<String>,
}

#[derive(Deserialize)]
struct ConstraintCase {
    capability: String,
    path: String,
    expect: String,
}

fn corpus() -> Corpus {
    serde_json::from_str(CORPUS).expect("http_cases.json 应可解析")
}

/// 把语料的声明映射转成 [`RouteVerb`]——`classify` 据此白名单反查，绝不启发式推断。
fn declared_routes(c: &Corpus) -> Vec<RouteVerb> {
    c.declared_routes
        .iter()
        .map(|r| RouteVerb {
            method: r.method.clone(),
            path: r.path.clone(),
            capability: r.capability.clone(),
        })
        .collect()
}

/// 把 `(method, path)` + 声明映射装成 HTTP `Intent` 负载（序列化进 `Intent`）。
fn intent_for(method: &str, path: &str, routes: &[RouteVerb]) -> Intent {
    let req = HttpRequest {
        method: method.into(),
        path: path.into(),
        headers: Vec::new(),
        body: Vec::new(),
        declared_routes: routes.to_vec(),
    };
    Intent::new(req.encode().expect("HttpRequest 序列化"))
}

/// 把语料动词名解回 [`Capability`]（未知名 panic——语料笔误应当暴露，非测试逻辑分支）。
fn cap(name: &str) -> Capability {
    parse_capability(name).unwrap_or_else(|| panic!("语料动词名未登记: {name}"))
}

/// 构造一个 `http_route` 的 `ConstraintSpec`：白名单按 `(capability, path)` 声明。
fn http_route_spec(whitelist: &[WirePattern]) -> ConstraintSpec {
    let spec = HttpRouteSpec {
        routes: whitelist
            .iter()
            .map(|p| RoutePattern {
                capability: p.capability.clone(),
                path: p.path.clone(),
            })
            .collect(),
    };
    ConstraintSpec {
        kind: "http_route".into(),
        spec: serde_json::to_string(&spec).expect("HttpRouteSpec 序列化"),
    }
}

/// 构造已物化的 `ClassifiedIntent`（动词 + `route:<path>` 对象），喂 check_constraint。
/// 对象用 `common::object::route_ref` 走与归类**同一条**规范化路径（§3.1 同一视图）。
fn ci_with_route(capability: Capability, path: &str) -> ClassifiedIntent {
    ClassifiedIntent {
        capability,
        objects: vec![object::route_ref(path)],
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// F-1 · Adapter 七方法全、签名对齐 core trait
// ─────────────────────────────────────────────────────────────────────────────

/// §8 F-1：`HttpAdapter` 实现 core `Adapter` trait——以「能被当作 `&dyn Adapter` 使用」
/// 在类型层钉死七方法签名逐一对齐 core 定义（缺一方法或签名不符则本文件无法编译）。
#[test]
fn http_is_a_core_adapter_with_registered_protocol() {
    let http = HttpAdapter;
    let erased: &dyn Adapter = &http;
    assert_eq!(erased.protocol(), "http", "protocol 登记键恒为 http（§5）");
    assert!(
        !erased.capabilities().is_empty(),
        "http 须申明其可承载的动词集（§3.3）"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// F-10 / L-9 · engine_enforced 如实声明 + 文档标注「归类+细则是唯一防线」
// ─────────────────────────────────────────────────────────────────────────────

/// §8 F-10 / L-9：http `engine_enforced()` 恒 `false`——无引擎账号兜底（返回值单元判定）。
#[test]
fn engine_enforced_is_false() {
    assert!(
        !HttpAdapter.engine_enforced(),
        "http 无引擎级强制兜底，engine_enforced 必须为 false（§3.3 / F-10 / L-9）"
    );
}

/// §8 L-9（结构检查）：`engine_enforced=false` 的协议须在模块文档如实标注「归类+细则
/// 是唯一防线」标注串——读源文件断言该标注串在场（缺标注即红）。
#[test]
fn module_doc_carries_sole_defense_marker() {
    let src = include_str!("../src/http/mod.rs");
    assert!(
        src.contains("归类+细则是唯一防线"),
        "http 模块文档须含「归类+细则是唯一防线」标注串（L-9 结构检查）"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// F-7 / L-5 · classify 按声明动词工具映射归类（命中 → 动词+route:<path>；未声明 → Err）
// ─────────────────────────────────────────────────────────────────────────────

/// §8 F-7（核心，场景 04 §4.1 Trace ②）+ L-5（fail-closed）：表驱动遍历语料，逐条
/// `classify((method,path) + 声明映射)`：命中声明形态须 `Ok` 且 capability **恰为**声明
/// 动词、objects **恰为** `[route:<path>]`（逐字段）；未落任何声明形态须 `Err`（白名单，
/// 未声明即不可归类，禁「GET 即只读」启发式）。每条用例的 `_why` 见语料。
#[test]
fn classify_matches_declared_mapping_or_errs() {
    let c = corpus();
    let routes = declared_routes(&c);
    let http = HttpAdapter;

    for case in &c.classify_cases {
        let intent = intent_for(&case.method, &case.path, &routes);
        let got = http.classify(&intent);

        match (&case.expect_capability, &case.expect) {
            // 命中声明形态：capability + objects 逐字段恰为期望。
            (Some(expect_cap), _) => {
                let ci = got.unwrap_or_else(|e| {
                    panic!(
                        "已声明形态 {} {} 须 Ok（命中声明动词工具映射，F-7），实得 Err({e:?})",
                        case.method, case.path
                    )
                });
                assert_eq!(
                    ci.capability,
                    cap(expect_cap),
                    "{} {} 须归声明动词 {expect_cap}（F-7，归类完全由声明决定、禁启发式）",
                    case.method,
                    case.path,
                );
                let expect_objects: Vec<ObjectRef> = case
                    .expect_objects
                    .as_ref()
                    .expect("命中用例须带 expect_objects")
                    .iter()
                    .map(ObjectRef::new)
                    .collect();
                assert_eq!(
                    ci.objects, expect_objects,
                    "{} {} 的 objects 须恰为 route:<path>（F-7 对象提取，供 http_route 细则与审计）",
                    case.method, case.path,
                );
            }
            // 未落任何声明形态：必须 Err，绝不 Ok 放行（L-5 白名单 fail-closed）。
            (None, Some(expect)) => {
                assert_eq!(expect, "err", "classify 用例 expect 只支持 err");
                assert!(
                    got.is_err(),
                    "未声明的 {} {} 必须 Err（白名单 fail-closed，L-5），绝不 Ok 放行",
                    case.method,
                    case.path,
                );
            }
            _ => panic!("classify 用例须二选一: expect_capability 或 expect=err"),
        }
    }
}

/// §8 F-7（逐字段精确钉死单点）：`classify({POST,/api/orders})` 恰为
/// `Ok(ClassifiedIntent{ capability=Mutate, objects=[route:/api/orders] })`——场景 04
/// §4.1 Trace ②[2] 的字面落点，独立于表驱动再钉一次，确保该核心点不被语料漂移稀释。
#[test]
fn classify_post_orders_is_exactly_mutate_with_route_object() {
    let c = corpus();
    let http = HttpAdapter;
    let ci = http
        .classify(&intent_for("POST", "/api/orders", &declared_routes(&c)))
        .expect("已声明 (POST,/api/orders) 须 Ok");
    assert_eq!(
        ci,
        ClassifiedIntent {
            capability: Capability::Mutate,
            objects: vec![ObjectRef::new("route:/api/orders")],
        },
        "classify({{POST,/api/orders}}) 须恰为 Ok(Mutate, objects=[route:/api/orders])（F-7）"
    );
}

/// §8 L-5 / L-14（畸形负载即 Err，不依赖桩——解析失败在桩之前短路）：负载无法解析为
/// HTTP `request`（畸形字节）→ classify 返回 `Err`（fail-closed），绝不吞错默归某档。
#[test]
fn classify_unparseable_payload_is_err() {
    let http = HttpAdapter;
    let garbage = Intent::new(b"\xff\xff not a json http request".to_vec());
    assert!(
        http.classify(&garbage).is_err(),
        "畸形负载必须 Err（不吞错放行，L-5 / L-14）"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// F-8 / L-7 · check_constraint: http_route 路由白名单（命中 true / 越界 false / 缺信息 Err）
// ─────────────────────────────────────────────────────────────────────────────

/// §8 F-8（核心，场景 04 §4.1 Trace ②[4]）+ L-7（fail-closed）：表驱动遍历约束语料，
/// 逐条 `check_constraint(http_route_spec, ci(capability, route:<path>))`：白名单内
/// `(capability,path)` 须 `Ok(true)`；白名单外（路径越界 **或** 动词越界）须 `Ok(false)`，
/// **绝不** `Ok(true)`。动词越界用例（路径在白名单内但 capability 未声明）钉死 method 维度
/// 经 `ci.capability` 保留——只比 path 忽略动词是 fail-open（越权写穿过读 / 写子集白名单）。
#[test]
fn check_http_route_whitelist_in_true_out_false() {
    let c = corpus();
    let spec = http_route_spec(&c.constraint_whitelist);
    let http = HttpAdapter;

    for case in &c.constraint_cases {
        let ci = ci_with_route(cap(&case.capability), &case.path);
        let got = http.check_constraint(&spec, &ci);
        match case.expect.as_str() {
            "ok_true" => assert_eq!(
                got,
                Ok(true),
                "白名单内 ({},{}) 须 Ok(true)（F-8）",
                case.capability,
                case.path,
            ),
            "ok_false" => assert_eq!(
                got,
                Ok(false),
                "白名单外 ({},{}) 须 Ok(false)（fail-closed，F-8 / L-7），绝不 Ok(true)",
                case.capability,
                case.path,
            ),
            other => panic!("constraint 用例 expect 不支持: {other}"),
        }
    }
}

/// §8 F-8 / L-7（全称量化判别——钉死 `all` 杀 `any`，§3.2 典型 fail-open）：一条请求触达
/// **多个** `route:<path>` 对象时，**每一个**都须在白名单内方可 `Ok(true)`；只要**任一**
/// 越界即整体 `Ok(false)`。构造单 `ci`、同一 capability=mutate、含两个 route 对象——一个在
/// 白名单内（`/api/orders`）、一个在白名单外（`/api/admin/reset`）——断言 `Ok(false)`。
/// 这是「一条触达多路由的请求只要有一条命中白名单就整体放行」这一 fail-open 的判别用例：
/// 全称量化（`all`）此处必 `Ok(false)`，存在量化（`any`）此处会错放成 `Ok(true)`，故本断言
/// 把 `all` 与 `any` 区分开。约束语料的 `ConstraintCase` 每条仅单 `(capability,path)`，无法
/// 表达多对象，故此判别用例在此手工构造（不经表驱动）。
#[test]
fn check_http_route_universal_quantification_one_out_of_many_is_false() {
    let c = corpus();
    let spec = http_route_spec(&c.constraint_whitelist);
    let http = HttpAdapter;
    // 同一 capability=mutate；两个 route 对象：一个白名单内、一个白名单外。
    let ci = ClassifiedIntent {
        capability: Capability::Mutate,
        objects: vec![
            object::route_ref("/api/orders"), // 白名单内 (mutate,/api/orders)
            object::route_ref("/api/admin/reset"), // 白名单外 (mutate,/api/admin/reset)
        ],
    };
    assert_eq!(
        http.check_constraint(&spec, &ci),
        Ok(false),
        "触达多路由的请求只要有一条越界即整体 Ok(false)（全称量化，§3.2）——\
         存在量化是典型 fail-open（一条命中即放行），绝不 Ok(true)",
    );
}

/// §8 L-7（缺信息即 Err）：`ci.objects` 不含任何 `route:<path>`（归类未能提取路由对象）
/// → `check_constraint` 须 `Err(ConstraintError)`——「判不了」等价于「不通过」，绝不
/// `Ok(true)`（L-7）。
#[test]
fn check_http_route_missing_route_object_is_err() {
    let c = corpus();
    let spec = http_route_spec(&c.constraint_whitelist);
    let http = HttpAdapter;
    let ci = ClassifiedIntent {
        capability: Capability::Mutate,
        objects: Vec::new(), // 无 route:<path> 对象
    };
    let got = http.check_constraint(&spec, &ci);
    assert!(
        got.is_err(),
        "缺 route 对象时须 Err（判不了 = 不通过，L-7），绝不 Ok(true)，实得 {got:?}"
    );
}

/// §8 L-7（畸形 spec 即 Err）：`http_route` 的 `spec` 串畸形（非合法白名单 JSON）→
/// `Err(ConstraintError)`，绝不 `Ok(true)`。
#[test]
fn check_http_route_malformed_spec_is_err() {
    let http = HttpAdapter;
    let spec = ConstraintSpec {
        kind: "http_route".into(),
        spec: "{ not valid spec json".into(),
    };
    let ci = ci_with_route(Capability::Mutate, "/api/orders");
    let got = http.check_constraint(&spec, &ci);
    assert!(
        got.is_err(),
        "畸形 http_route spec 须 Err（L-7），绝不 Ok(true)，实得 {got:?}"
    );
}

/// §8 L-7 / L-14（未知 kind 即 Err，不依赖桩——kind 不匹配在桩之前短路）：未知 `kind`
/// （非 `http_route`，http 非其属主）→ `Err(ConstraintError::UnknownKind)`。钉死「未知
/// kind 不被静默放行」（fail-closed），且变体**恰为** `UnknownKind`。
#[test]
fn check_unknown_constraint_kind_is_err_unknown_kind() {
    let http = HttpAdapter;
    let spec = ConstraintSpec {
        kind: "table_allow".into(), // 非 http 属主的 kind
        spec: "{}".into(),
    };
    let ci = ci_with_route(Capability::Mutate, "/api/orders");
    assert_eq!(
        http.check_constraint(&spec, &ci),
        Err(ConstraintError::UnknownKind),
        "http 不识别的 kind 须 Err(UnknownKind)，绝不放行（L-7 / L-14）"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// F-12 · HTTP Intent 负载 serde 往返逐字段相等（§3.6，本 crate 定义的 request schema）
// ─────────────────────────────────────────────────────────────────────────────

/// §8 F-12：HTTP `Intent` 负载（`HttpRequest`，MCP `request` schema）encode→decode 往返后
/// **逐字段相等**——跨进程搬运后须能无损反序列化回适配器（§3.6）。本断言不依赖桩，是当前
/// 即应为绿的真往返守护（含 headers / body / 声明映射全字段）。
#[test]
fn http_request_payload_round_trips_field_for_field() {
    let original = HttpRequest {
        method: "POST".into(),
        path: "/api/orders".into(),
        headers: vec![("content-type".into(), "application/json".into())],
        body: b"{\"sku\":\"x\"}".to_vec(),
        declared_routes: vec![RouteVerb {
            method: "POST".into(),
            path: "/api/orders".into(),
            capability: "mutate".into(),
        }],
    };
    let bytes = original.encode().expect("encode");
    let back = HttpRequest::decode(&bytes).expect("decode");
    assert_eq!(
        back, original,
        "HttpRequest encode/decode 往返须逐字段相等（F-12）"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// F-11 · discover 发现≠授权：fail-closed 返回 Err(ProbeFailed)，能力面是纯事实类型
// ─────────────────────────────────────────────────────────────────────────────

/// 测试用不透明 `Channel` 句柄——`Channel.handle` 是 `Box<dyn Send + Sync>`，对适配器
/// 不透明（适配器拿不到、也不可下转传输私有句柄，L-13）。`discover` 在其上做探针协议
/// 协商；句柄具体载荷与本断言无关，置最小占位。
fn dummy_channel() -> Channel {
    Channel {
        handle: Box::new(()),
    }
}

/// §8 F-11 / §3.5 / L-12：远端探针协议协商（`protocol_version`，详设 6.12）未落地前，
/// http `discover` **fail-closed** 返回 `Err(DiscoverError::ProbeFailed)`——绝不凭空伪造
/// 能力面（公理二）。这道断言钉死「发现失败即 `Err`、绝不据失败 / 部分结果伪造
/// `Ok(CapabilitySurface)`」：若某实现波次把 http `discover` 改填成返回**任何** `Ok`
/// （尤其携授权字段的伪造能力面），此处即变红，强制实现者**同时**落地真实探针协商并显式
/// 更新本断言（先红再绿）。`CapabilitySurface` 不派生 `Debug`/`PartialEq`（核心类型，
/// 不外泄诊断态），故 `Ok` 分支不可 `assert_eq!`——以 `match` 显式拒绝**任何** `Ok`
/// （含伪造能力面），并钉死失败变体恰为 `ProbeFailed`（非 `ChannelLost`），不接受其它失败语义。
#[tokio::test]
async fn http_discover_fail_closed_until_probe() {
    let http = HttpAdapter;
    let mut ch = dummy_channel();
    match http.discover(&mut ch).await {
        Err(DiscoverError::ProbeFailed) => {}
        Err(DiscoverError::ChannelLost) => {
            panic!("探针未落地前 discover 应为 ProbeFailed，得 ChannelLost（F-11/§3.5）")
        }
        Ok(_) => panic!(
            "探针协议未落地前 discover 必 fail-closed 为 Err，绝不伪造 Ok 能力面（F-11/§3.5）"
        ),
    }
}

/// §8 F-11 / §3.5：`discover` 的**唯一**成功产物 `CapabilitySurface` 是**纯事实**类型——
/// 结构上只含 `capabilities`（资源具备何种能力）与 `objects`（探得的对象引用），**无任何
/// allow/tier/grant 授权字段**（发现≠授权，授权化是人经控制面圈选的后续动作）。
///
/// 以**穷尽解构**钉死字段集：核心 `CapabilitySurface` 若新增任一授权字段，此 `let { .. }`
/// 解构因字段不全而编译失败——把「能力面零授权字段」从散文约束升为编译期护栏（§3.5）。
/// 若某实现波次把 http `discover` 改填成返回携授权字段的伪造能力面，须先令此核心类型新增
/// 字段，此处即编译失败——发现≠授权这一核心 fail-open 在编译期被钉死。
#[test]
fn http_capability_surface_is_facts_only() {
    // 构造一个能力面（http 能力面随声明动词集，此处取读写两类）——纯事实，无授权维度。
    let surface = CapabilitySurface {
        capabilities: vec![Capability::Observe, Capability::Mutate],
        objects: vec!["route:/api/orders".to_string()],
    };
    // 穷尽解构：字段恰为 (capabilities, objects)。新增 allow/tier/grant 字段即编译失败。
    let CapabilitySurface {
        capabilities,
        objects,
    } = surface;
    assert_eq!(
        capabilities,
        vec![Capability::Observe, Capability::Mutate],
        "能力面只装资源具备何种能力（事实），无授权维度",
    );
    assert_eq!(
        objects,
        vec!["route:/api/orders".to_string()],
        "能力面只装探得的对象引用（事实），无授权维度",
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// L-13 / L-16 · 签名结构检查（只见 Channel；只产归类、不产决策）
// ─────────────────────────────────────────────────────────────────────────────

/// §8 L-13：`execute` / `discover` 入参仅 `&mut Channel`(+`&Intent`)——无 `CredentialTier` /
/// `ResolvedTarget` / `ResourceCredential` / 地址类型形参。以「把方法绑定到只声明**入参**
/// 形态的同形函数指针」在类型层钉死签名（入参漂移成含 tier / 地址 / 凭据即编译失败）。
/// **不调用**这些 `todo!()` 桩，故不引入 panic——纯签名结构断言。
///
/// 入参形态由独立同形 `fn` 桩 [`exec_shape`] / [`discover_shape`] 声明（与 core trait 方法
/// 入参逐一对齐）；把 trait 方法赋给该形态的 `fn` 指针变量即在类型层校验入参一致。返回
/// 的 future 类型经统一函数指针类型推断，避免依赖 `async_trait` 脱糖的具体 future 形状。
#[test]
fn execute_and_discover_see_only_channel() {
    // 同形入参桩：execute 仅 (&self, &mut Channel, &Intent)。
    fn exec_shape<'a>(
        this: &'a HttpAdapter,
        ch: &'a mut Channel,
        intent: &'a Intent,
    ) -> core::pin::Pin<
        Box<
            dyn core::future::Future<Output = Result<RawResponse, postern_core::error::ExecError>>
                + Send
                + 'a,
        >,
    > {
        this.execute(ch, intent)
    }
    // 同形入参桩：discover 仅 (&self, &mut Channel)。
    fn discover_shape<'a>(
        this: &'a HttpAdapter,
        ch: &'a mut Channel,
    ) -> core::pin::Pin<
        Box<
            dyn core::future::Future<
                    Output = Result<CapabilitySurface, postern_core::error::DiscoverError>,
                > + Send
                + 'a,
        >,
    > {
        this.discover(ch)
    }
    // 同形桩内部经 `this.execute(ch, intent)` / `this.discover(ch)` 调用 trait 方法（仅
    // 构造 future、不 poll，故不进 `todo!()` 体、无 panic）——入参一旦多出 tier / 地址 /
    // 凭据，桩内调用即不成立、`_shape` 编译失败。仅取地址不执行。
    let _exec = exec_shape;
    let _discover = discover_shape;
}

/// §8 L-16：`classify` 返回 `ClassifiedIntent`、`check_constraint` 返回 `bool`——返回类型
/// 均**不含** `Decision` / `CredentialTier`。以「把返回值显式绑定到 core 的归类 / 布尔类型」
/// 在类型层钉死签名（返回类型漂移成含 Decision / tier 即编译失败）。畸形负载短路即得 `Err`，
/// 仍是 `Result<ClassifiedIntent, _>` / `Result<bool, _>` 类型，足以钉签名，不依赖桩逻辑。
#[test]
fn classify_and_check_return_classification_not_decision() {
    let http = HttpAdapter;
    let classified: Result<ClassifiedIntent, postern_core::error::ClassifyError> =
        http.classify(&Intent::new(b"not json".to_vec()));
    assert!(classified.is_err());

    let checked: Result<bool, ConstraintError> = http.check_constraint(
        &ConstraintSpec {
            kind: "table_allow".into(),
            spec: "{}".into(),
        },
        &ci_with_route(Capability::Mutate, "/api/orders"),
    );
    assert!(
        checked.is_err(),
        "未知 kind 短路得 Err，类型为 Result<bool, _>（L-16）"
    );
}
