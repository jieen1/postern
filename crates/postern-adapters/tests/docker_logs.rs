//! docker_logs 适配器单元（RED · F-1 / F-6 / F-8 / F-10 / F-11 / F-12 / L-9 / L-13）。
//!
//! docker_logs 是只读容器日志适配器，`engine_enforced=false`——归类+细则是唯一防线。
//! 行为断言两类来源：
//! - **能力声明侧**（骨架阶段即实现、非 `todo!()`）：协议名、恒只读动词集、
//!   `engine_enforced()==false`、模块文档「归类+细则是唯一防线」标注串结构存在（L-9）。
//! - **行为侧**（实现波次填实，当前 `todo!()` → 本文件观察到红）：`classify` 恒
//!   `Observe` + `objects=[container:<名>]`（F-6）、`container_prefix` 命中/不命中/
//!   缺信息（F-8 / L-7）。
//! - **发现侧（F-11 / §3.5 / L-12）**：`discover` 是控制面能力面探测、**发现≠授权**。
//!   远端探针协议协商（`protocol_version`，详设 6.12）未落地前 `discover` **fail-closed**
//!   返回 `Err(DiscoverError::ProbeFailed)`——绝不凭空伪造能力面（公理二）；其唯一成功产物
//!   `CapabilitySurface` 是**纯事实**类型，结构上只含 `capabilities` / `objects`，**无任何
//!   allow/tier/grant 授权字段**（核心类型层固化，§3.5）。本文件钉死这两层：失败路径恰为
//!   `ProbeFailed`（若某波次改填携授权字段的伪造 `Ok` 即变红），成功产物的字段集经解构
//!   穷尽匹配（核心类型新增授权字段会使此处编译失败）。
//!
//! SQL 纪律（B 方案）：docker_logs 无 SQL，本 `.rs` 与语料天然零 SQL 标记；取数请求
//! 语料放 `tests/corpus/docker_logs_cases.json`，表驱动读取 → 逐 case 断言。每条断言
//! 精确到 `Capability` 具体档 / `ObjectRef` 具体值 / 具体 `ConstraintError` 变体。

use serde::Deserialize;

use postern_core::domain::{Capability, ConstraintSpec};
use postern_core::error::DiscoverError;
use postern_core::plugin::{Adapter, CapabilitySurface, Channel};
use postern_core::request::{ClassifiedIntent, Intent, ObjectRef};

use postern_adapters::docker_logs::intent::{DockerLogsRequest, LogsRequest};
use postern_adapters::docker_logs::DockerLogsAdapter;

// ── 语料 schema（表驱动） ────────────────────────────────────────────────────

const CORPUS: &str = include_str!("corpus/docker_logs_cases.json");

#[derive(Deserialize)]
struct Corpus {
    classify_cases: Vec<ClassifyCase>,
    constraint_cases: Vec<ConstraintCase>,
    missing_objects_case: MissingObjectsCase,
    parse_fail_cases: Vec<ParseFailCase>,
}

#[derive(Deserialize)]
struct ClassifyCase {
    name: String,
    /// docker_logs `Intent` 负载形态（action/container/since/tail/follow）。
    request: LogsRequest,
    /// 期望归类档（恒 `"observe"`）。
    expect_capability: String,
    /// 期望对象集（恒 `["container:<名>"]`）。
    expect_objects: Vec<String>,
}

#[derive(Deserialize)]
struct ConstraintCase {
    name: String,
    /// 物化 `ClassifiedIntent` 的容器名（→ `container:<名>` 对象）。
    container: String,
    /// 细则 kind（`container_prefix` 或异类用于 UnknownKind）。
    spec_kind: String,
    /// 细则 spec 负载（适配器解释的 JSON）。
    spec: serde_json::Value,
    /// 期望结果：`ok_true` / `ok_false` / `err_unknown_kind` / `err_invalid_spec`。
    expect: String,
}

#[derive(Deserialize)]
struct MissingObjectsCase {
    spec_kind: String,
    spec: serde_json::Value,
    expect: String,
}

#[derive(Deserialize)]
struct ParseFailCase {
    name: String,
    /// 不可解码的原始 `Intent` 负载字节（UTF-8 字符串形态）——任意伪装 / 异常输入。
    raw_payload: String,
    /// 期望结果（恒 `"err_parse"`）。
    expect: String,
}

/// 把语料的 `LogsRequest` 包成封闭枚举唯一变体并编码为 [`Intent`]（外壳层装箱形态）。
fn intent_from(req: LogsRequest) -> Intent {
    let payload = DockerLogsRequest::Logs(req)
        .encode()
        .expect("docker_logs 负载应可序列化为 Intent 字节");
    Intent::new(payload)
}

/// 把容器名物化为 `container_prefix` 细则的 `ClassifiedIntent`（恒 `Observe`，对象集
/// 为单个 `container:<名>`）——模拟 `classify` 产出后内核传给 `check_constraint` 的入参。
fn ci_for(container: &str) -> ClassifiedIntent {
    ClassifiedIntent {
        capability: Capability::Observe,
        objects: vec![ObjectRef::new(format!("container:{container}"))],
    }
}

/// 语料字符串 → `Capability`（断言用，精确到具体档）。
fn cap_of(s: &str) -> Capability {
    match s {
        "observe" => Capability::Observe,
        "query" => Capability::Query,
        "mutate" => Capability::Mutate,
        "execute" => Capability::Execute,
        "manage" => Capability::Manage,
        "destroy" => Capability::Destroy,
        other => panic!("语料 expect_capability 非法: {other}"),
    }
}

// ── 能力声明侧（骨架阶段即绿，作回归护栏） ───────────────────────────────────

/// F-1 / F-10 / L-9：协议名 + `engine_enforced()==false`（无引擎账号兜底）。
#[test]
fn docker_logs_engine_not_enforced() {
    let a = DockerLogsAdapter;
    assert!(
        !a.engine_enforced(),
        "docker_logs 必须 engine_enforced=false（§3.3）"
    );
    assert_eq!(a.protocol(), "docker_logs");
}

/// F-6 能力面：容器日志恒只读 → 动词集**恰**为 `[Observe]`（§3.3）。
#[test]
fn docker_logs_observe_only() {
    let a = DockerLogsAdapter;
    assert_eq!(a.capabilities(), &[Capability::Observe]);
}

/// L-9 结构检查：模块文档（`mod.rs` 顶部 `//!` 文档串）必须含「归类+细则是唯一防线」
/// 标注串——`engine_enforced=false` 协议须如实标注（公理三）；漏标即不过。
#[test]
fn docker_logs_doc_marks_sole_defense() {
    const MOD_SRC: &str = include_str!("../src/docker_logs/mod.rs");
    assert!(
        MOD_SRC.contains("归类+细则是唯一防线"),
        "docker_logs mod.rs 文档必须含「归类+细则是唯一防线」标注串（L-9）"
    );
}

// ── F-12：负载序列化往返逐字段相等（封闭枚举无写变体可表达） ──────────────────

/// F-12：docker_logs `Intent` 负载 encode → decode 往返后逐字段相等（对外 schema 稳定）。
#[test]
fn docker_logs_payload_roundtrip() {
    let original = DockerLogsRequest::Logs(LogsRequest {
        container: "app-order".to_string(),
        since: Some("1h".to_string()),
        tail: Some(200),
        follow: true,
    });
    let bytes = original.encode().expect("应可序列化");
    let decoded = DockerLogsRequest::decode(&bytes).expect("应可反序列化");
    assert_eq!(decoded, original, "F-12：往返后逐字段相等");
}

/// F-12 / §3.1 类型层只读：唯一变体匹配为 `Logs`——封闭枚举结构上无写 / 控制变体可表达
/// （只读性下沉到类型，非运行期判别）。新增写变体会使此 `match` 因不可穷尽而编译失败。
#[test]
fn docker_logs_payload_has_only_readonly_variant() {
    let req = DockerLogsRequest::Logs(LogsRequest {
        container: "app-order".to_string(),
        since: None,
        tail: None,
        follow: false,
    });
    match req {
        DockerLogsRequest::Logs(l) => assert_eq!(l.container, "app-order"),
    }
}

// ── F-6：classify 恒 Observe + objects=[container:<名>]（表驱动，当前 todo! → 红） ──

/// F-6 / 场景 04 §4.1 Trace ③[2]：取容器日志请求恒归 `Observe`，`objects` 恰为
/// `[container:<名>]`——逐 case 精确断言具体 `Capability` 档与具体 `ObjectRef` 值。
#[test]
fn docker_logs_classify_always_observe() {
    let corpus: Corpus = serde_json::from_str(CORPUS).expect("docker_logs_cases.json 应可解析");
    let a = DockerLogsAdapter;
    assert!(!corpus.classify_cases.is_empty(), "classify 语料不应为空");

    for case in &corpus.classify_cases {
        let intent = intent_from(case.request.clone());
        let ci = a.classify(&intent).unwrap_or_else(|e| {
            panic!(
                "[{}] classify 应 Ok（恒 Observe），得 Err: {e:?}",
                case.name
            )
        });

        // 恒 Observe（§3.1）——精确到具体档。
        assert_eq!(
            ci.capability,
            cap_of(&case.expect_capability),
            "[{}] classify 必须恒归 Observe",
            case.name
        );
        assert_eq!(
            ci.capability,
            Capability::Observe,
            "[{}] 档必为 Observe",
            case.name
        );

        // objects 恰为 container:<名>（§3.1）——精确到具体 ObjectRef 值。
        let expected: Vec<ObjectRef> = case.expect_objects.iter().map(ObjectRef::new).collect();
        assert_eq!(
            ci.objects, expected,
            "[{}] objects 必为 [container:<名>]",
            case.name
        );
    }
}

// ── §8 L-5 / §3.1：classify 的唯一 fail-closed 分支——负载解码失败 → Err(ParseFailed) ──

/// L-5 / §3.1 / 公理二 fail-closed：`classify` 唯一的归类拒绝路径是「负载解码失败 →
/// `Err(ClassifyError::ParseFailed)`」（实现 `src/docker_logs/classify.rs`：`decode(...)
/// .map_err(|_| ParseFailed)?`）。happy-path 测试只遍历合法语料、恒 `Observe`，从不触达
/// 这条分支；本测试以**不可解码 / 伪装 / 异常负载**逐 case 钉死该 deny 必为 `ParseFailed`。
///
/// 这道断言堵的是「伪装 / 异常输入被静默降级放行」缺口：若某波次把该分支改为吞错放行
/// （如对不可解码负载 `unwrap_or` 默认 `Observe`、或返回 `Ok(Observe)`），本表中至少
/// `unknown_action_disguise`（封闭枚举无法表达的写 / 控制动词）会从 `Err` 退化为 `Ok`，
/// 此处即变红，强制实现者保持 fail-closed 并显式更新断言（先红再绿）。
#[test]
fn docker_logs_classify_undecodable_payload_denies() {
    let corpus: Corpus = serde_json::from_str(CORPUS).expect("docker_logs_cases.json 应可解析");
    let a = DockerLogsAdapter;
    assert!(
        !corpus.parse_fail_cases.is_empty(),
        "parse_fail 语料不应为空（fail-closed 分支必须有用例覆盖）"
    );

    for case in &corpus.parse_fail_cases {
        assert_eq!(
            case.expect, "err_parse",
            "[{}] parse_fail 语料 expect 必为 err_parse",
            case.name
        );

        // 不经 schema 物化：直接把原始字节装箱为 Intent（绕开合法 LogsRequest 形态），
        // 精确命中 classify 的 decode 失败分支。
        let intent = Intent::new(case.raw_payload.clone().into_bytes());
        let got = a.classify(&intent);

        // 不可解码负载必 fail-closed deny，且失败语义恰为 ParseFailed——绝不降级放行为
        // Ok(Observe)，也不接受其它 ClassifyError 变体（钉死「解析失败 → deny」）。
        assert_eq!(
            got,
            Err(postern_core::error::ClassifyError::ParseFailed),
            "[{}] 不可解码 / 伪装负载必为 Err(ParseFailed)，绝不静默降级为 Ok(Observe)（L-5/§3.1）",
            case.name
        );
    }
}

// ── F-8 / L-7：container_prefix 命中 / 不命中 / 缺信息（表驱动，当前 todo! → 红） ──

/// F-8 / L-7 / 场景 04 §4.1 Trace ③[4]：`container_prefix` 前缀命中 → `Ok(true)`、
/// 不命中 → `Ok(false)`、未知 kind / 非法 spec → 具体 `ConstraintError`。逐 case 精确
/// 断言（命中/不命中绝不互换，err 绝不降为 `Ok(true)`，L-7）。
#[test]
fn docker_logs_container_prefix() {
    let corpus: Corpus = serde_json::from_str(CORPUS).expect("docker_logs_cases.json 应可解析");
    let a = DockerLogsAdapter;
    assert!(
        !corpus.constraint_cases.is_empty(),
        "constraint 语料不应为空"
    );

    for case in &corpus.constraint_cases {
        let spec = ConstraintSpec {
            kind: case.spec_kind.clone(),
            spec: case.spec.to_string(),
        };
        let ci = ci_for(&case.container);
        let got = a.check_constraint(&spec, &ci);

        match case.expect.as_str() {
            "ok_true" => assert_eq!(
                got,
                Ok(true),
                "[{}] 前缀命中必为 Ok(true)（F-8）",
                case.name
            ),
            "ok_false" => assert_eq!(
                got,
                Ok(false),
                "[{}] 前缀不命中必为 Ok(false)，绝不 Ok(true)（F-8）",
                case.name
            ),
            "err_unknown_kind" => assert_eq!(
                got,
                Err(postern_core::error::ConstraintError::UnknownKind),
                "[{}] 非属主 kind 必为 Err(UnknownKind)，绝不 Ok(true)（L-7）",
                case.name
            ),
            "err_invalid_spec" => assert_eq!(
                got,
                Err(postern_core::error::ConstraintError::InvalidSpec),
                "[{}] 非法 spec 必为 Err(InvalidSpec)，绝不 Ok(true)（L-7）",
                case.name
            ),
            other => panic!("[{}] 语料 expect 非法: {other}", case.name),
        }
    }
}

/// L-7：判定所需信息缺失（`ci.objects` 无 `container:<名>` 维度）→
/// `Err(ConstraintError::MissingObjects)`——「判不了」等价「不通过」，绝不 `Ok(true)`。
#[test]
fn docker_logs_container_prefix_missing_objects_denies() {
    let corpus: Corpus = serde_json::from_str(CORPUS).expect("docker_logs_cases.json 应可解析");
    let case = &corpus.missing_objects_case;
    let a = DockerLogsAdapter;

    let spec = ConstraintSpec {
        kind: case.spec_kind.clone(),
        spec: case.spec.to_string(),
    };
    // 缺信息：objects 为空（classify 未能提取出 container 维度）。
    let ci = ClassifiedIntent {
        capability: Capability::Observe,
        objects: vec![],
    };
    let got = a.check_constraint(&spec, &ci);

    assert_eq!(case.expect, "err_missing_objects", "语料断言项校验");
    assert_eq!(
        got,
        Err(postern_core::error::ConstraintError::MissingObjects),
        "缺信息必为 Err(MissingObjects)，绝不 Ok(true)（L-7）"
    );
}

// ── F-11 / §3.5 / L-12：discover 发现≠授权（控制面能力面探测，fail-closed） ──────

/// 测试用不透明 `Channel` 句柄——`Channel.handle` 是 `Box<dyn Send + Sync>`，对适配器
/// 不透明（适配器拿不到、也不可下转传输私有句柄，L-13）。`discover` 在其上做探针协议
/// 协商；句柄具体载荷与本断言无关，置最小占位。
fn dummy_channel() -> Channel {
    Channel {
        handle: Box::new(()),
    }
}

/// F-11 / §3.5：远端探针协议协商（`protocol_version`，详设 6.12）未落地前，`discover`
/// **fail-closed** 返回 `Err(DiscoverError::ProbeFailed)`——绝不凭空伪造能力面（公理二）。
///
/// 这道断言钉死「发现失败即 `Err`、绝不据失败 / 部分结果伪造 `Ok(CapabilitySurface)`」：
/// 若某实现波次把 `discover` 改填成返回**任何** `Ok`（尤其携授权字段的伪造能力面），
/// 此处即变红，强制实现者**同时**落地真实探针协商并显式更新本断言（先红再绿）。
#[tokio::test]
async fn docker_logs_discover_fail_closed_until_probe() {
    let a = DockerLogsAdapter;
    let mut ch = dummy_channel();
    // `CapabilitySurface` 不派生 `Debug`/`PartialEq`（核心类型，不外泄诊断态），故 `Ok`
    // 分支不可 `assert_eq!`——以 `match` 显式拒绝**任何** `Ok`（含伪造能力面），并钉死
    // 失败变体恰为 `ProbeFailed`（非 `ChannelLost`），不接受其它失败语义。
    match a.discover(&mut ch).await {
        Err(DiscoverError::ProbeFailed) => {}
        Err(DiscoverError::ChannelLost) => {
            panic!("探针未落地前 discover 应为 ProbeFailed，得 ChannelLost（F-11/§3.5）")
        }
        Ok(_) => panic!(
            "探针协议未落地前 discover 必 fail-closed 为 Err，绝不伪造 Ok 能力面（F-11/§3.5）"
        ),
    }
}

/// F-11 / §3.5：`discover` 的**唯一**成功产物 `CapabilitySurface` 是**纯事实**类型——
/// 结构上只含 `capabilities`（资源具备何种能力）与 `objects`（探得的对象引用），**无任何
/// allow/tier/grant 授权字段**（发现≠授权，授权化是人经控制面圈选的后续动作）。
///
/// 以**穷尽解构**钉死字段集：核心 `CapabilitySurface` 若新增任一授权字段，此 `let { .. }`
/// 解构因字段不全而编译失败——把「能力面零授权字段」从散文约束升为编译期护栏（§3.5）。
#[test]
fn docker_logs_capability_surface_is_facts_only() {
    // 构造一个只读能力面（docker_logs 能力面恒只读，§3.5）——纯事实，无授权维度。
    let surface = CapabilitySurface {
        capabilities: vec![Capability::Observe],
        objects: vec!["container:app-order".to_string()],
    };
    // 穷尽解构：字段恰为 (capabilities, objects)。新增 allow/tier/grant 字段即编译失败。
    let CapabilitySurface {
        capabilities,
        objects,
    } = surface;
    assert_eq!(
        capabilities,
        vec![Capability::Observe],
        "能力面只装资源具备何种能力（事实），docker_logs 恒只读"
    );
    assert_eq!(
        objects,
        vec!["container:app-order".to_string()],
        "能力面只装探得的对象引用（事实），无授权维度"
    );
}
