//! postgres `Intent` 负载序列化往返 + 解析入口单元（行为测试，§3.6 F-12 / F-1）。
//!
//! 表驱动：从 `tests/corpus/intent_roundtrip.json` 读 case 集。两组语料：
//! - `roundtrip`：每条是一份动词工具 `request` 负载（[`PgRequest`] 形态）。断言
//!   `from_payload(serialize(request))` 反序列化后与原 `request` **逐字段相等**，且再次
//!   序列化得到同一份 JSON（往返稳定，F-12 判定基准）。
//! - `parse_failure`：每条是一段非法字节。断言 [`PgRequest::from_payload`] 恰返回
//!   [`ClassifyError::ParseFailed`]（解析失败 → fail-closed，公理二）。
//!
//! 语句原文（含各类危险/伪装写法）全部放数据文件，本 `.rs` 只含数据文件路径与符号名，
//! 零 SQL 文本标记（B 方案）——连断言消息、注释、字符串字面量都不含任何语句关键字。

use postern_core::domain::Capability;
use postern_core::error::ClassifyError;
use postern_core::plugin::Adapter;
use postern_core::request::Intent;

use postern_adapters::postgres::intent::PgRequest;
use postern_adapters::postgres::PostgresAdapter;

use serde::Deserialize;

const CORPUS: &str = include_str!("corpus/intent_roundtrip.json");
const CLASSIFY_CORPUS: &str = include_str!("corpus/intent_classify.json");

#[derive(Deserialize)]
struct Corpus {
    roundtrip: Vec<RoundtripCase>,
    parse_failure: Vec<ParseFailureCase>,
}

#[derive(Deserialize)]
struct RoundtripCase {
    /// 人读用例名，断言失败时定位用。
    name: String,
    /// 原始 `request` 负载（任意 JSON object，须能反序列化为 `PgRequest`）。
    request: serde_json::Value,
    /// 可选：当 `request` 省略 `params` 等字段时，期望再序列化后的规范形态。
    /// 缺省则期望再序列化结果与 `request` 逐字段相等。
    #[serde(default)]
    expect_serialized: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct ParseFailureCase {
    /// 人读用例名。
    name: String,
    /// 一段非法负载字节（解析必败）。
    bytes: String,
}

fn load_corpus() -> Corpus {
    serde_json::from_str(CORPUS).expect("intent_roundtrip.json 应可解析为 Corpus")
}

// §8 F-12 —— 负载序列化往返逐字段相等：把原始 request 序列化为字节，经 from_payload
// 反序列化回 PgRequest，再序列化；断言反序列化结果与原 request 字段一致，且二次序列化
// 稳定。covers: 只读/观测/写/销毁/伪装写/参数缺省/异构标量/unicode/空语句各形态。
#[test]
fn payload_roundtrips_field_equal() {
    let corpus = load_corpus();
    assert!(
        corpus.roundtrip.len() >= 10,
        "roundtrip 语料至少应覆盖 10 类形态，实得 {}",
        corpus.roundtrip.len()
    );

    for case in &corpus.roundtrip {
        // 原始 request -> 字节（模拟外壳层装箱搬运的负载形态）。
        let payload = serde_json::to_vec(&case.request)
            .unwrap_or_else(|e| panic!("[{}] request 应可序列化为字节: {e}", case.name));

        // 解析入口：字节 -> PgRequest（这是适配器对负载的唯一解释路径，§3.6）。
        let decoded: PgRequest = PgRequest::from_payload(&payload)
            .unwrap_or_else(|e| panic!("[{}] 合法负载 from_payload 应成功，得 {e:?}", case.name));

        // 再序列化为规范 JSON。
        let reserialized: serde_json::Value = serde_json::to_value(&decoded)
            .unwrap_or_else(|e| panic!("[{}] PgRequest 应可序列化回 JSON: {e}", case.name));

        // 期望：缺 params 等字段的用例给出 expect_serialized 规范形态，否则与原 request 相等。
        let expected = case.expect_serialized.as_ref().unwrap_or(&case.request);
        assert_eq!(
            &reserialized, expected,
            "[{}] 往返后逐字段应相等（F-12 判定基准）",
            case.name
        );

        // 往返幂等：把规范 JSON 再次过一遍 from_payload，应得到字段完全相同的 PgRequest。
        let canonical_bytes = serde_json::to_vec(&reserialized)
            .unwrap_or_else(|e| panic!("[{}] 规范 JSON 应可序列化: {e}", case.name));
        let decoded_again = PgRequest::from_payload(&canonical_bytes)
            .unwrap_or_else(|e| panic!("[{}] 规范负载二次解析应成功，得 {e:?}", case.name));
        assert_eq!(
            decoded, decoded_again,
            "[{}] 二次往返应得逐字段相等的 PgRequest（往返稳定）",
            case.name
        );
    }
}

// §8 F-12 —— 解析失败一律 ClassifyError::ParseFailed（fail-closed，公理二）：
// 非法 JSON / 类型不符 / 字段缺失 / 截断 / 尾随垃圾各形态均须恰为 ParseFailed，
// 绝不吞错放行、绝不映射为其他变体。
#[test]
fn malformed_payload_is_parse_failed() {
    let corpus = load_corpus();
    assert!(
        corpus.parse_failure.len() >= 6,
        "parse_failure 语料至少应覆盖 6 类非法形态，实得 {}",
        corpus.parse_failure.len()
    );

    for case in &corpus.parse_failure {
        let result = PgRequest::from_payload(case.bytes.as_bytes());
        assert_eq!(
            result,
            Err(ClassifyError::ParseFailed),
            "[{}] 非法负载应恰返回 ParseFailed，实得 {result:?}",
            case.name
        );
    }
}

// §8 F-12 —— 负载解析与归类入参解耦：from_payload 只做反序列化、不做归类，故合法但
// 内容危险（伪装写）的负载在 from_payload 阶段必须成功（危险度判定属 classify，§3.1）。
// 这里逐字段断言一个携位置参数的负载，statement 与 params 顺序/值/类型逐一稳定。
#[test]
fn decoded_payload_preserves_statement_and_params_exactly() {
    let corpus = load_corpus();
    let case = corpus
        .roundtrip
        .iter()
        .find(|c| c.name == "read_only_positional_params_mixed_scalars")
        .expect("语料应含 read_only_positional_params_mixed_scalars 用例");

    let payload = serde_json::to_vec(&case.request).expect("request 应可序列化");
    let decoded = PgRequest::from_payload(&payload).expect("合法负载应解析成功");

    // statement 字段须与语料原文逐字相等（不改写、不规范化）。
    let want_statement = case.request["statement"]
        .as_str()
        .expect("语料 statement 应为字符串");
    assert_eq!(
        decoded.statement, want_statement,
        "statement 须逐字保真，不被解析阶段改写"
    );

    // params 须保序、保类型、保值（异构标量 + null）。
    let want_params = case.request["params"]
        .as_array()
        .expect("语料 params 应为数组");
    assert_eq!(
        decoded.params.len(),
        want_params.len(),
        "params 元素个数须保持"
    );
    for (i, (got, want)) in decoded.params.iter().zip(want_params.iter()).enumerate() {
        assert_eq!(got, want, "params[{i}] 须逐元素保真（顺序/类型/值）");
    }
}

// §8 F-12 —— params 缺省即空：省略 params 字段的负载经往返须物化为空向量（serde default），
// 而非解析失败，保证动词工具 request schema 对“无参数语句”友好。
#[test]
fn omitted_params_defaults_to_empty() {
    let corpus = load_corpus();
    let case = corpus
        .roundtrip
        .iter()
        .find(|c| c.name == "read_only_params_omitted_defaults_empty")
        .expect("语料应含 read_only_params_omitted_defaults_empty 用例");

    let payload = serde_json::to_vec(&case.request).expect("request 应可序列化");
    let decoded = PgRequest::from_payload(&payload).expect("省略 params 的负载应解析成功");

    assert!(
        decoded.params.is_empty(),
        "省略 params 字段时应默认为空向量，实得 {:?}",
        decoded.params
    );
}

// §8 F-1 / F-10 —— PostgresAdapter 能力声明三方法存在且签名对齐 core trait：
// protocol()=="postgres"、capabilities()=[Observe,Query,Mutate,Destroy]、engine_enforced()==true
// （SQL 类有引擎账号兜底，编译期常量，不做运行期推断）。
#[test]
fn adapter_capability_surface_matches_design() {
    let adapter = PostgresAdapter;

    assert_eq!(
        adapter.protocol(),
        "postgres",
        "协议注册键恒为 postgres（F-1/§5）"
    );

    assert_eq!(
        adapter.capabilities(),
        &[
            Capability::Observe,
            Capability::Query,
            Capability::Mutate,
            Capability::Destroy,
        ],
        "SQL 可承载 Observe/Query/Mutate/Destroy 四档（§3.3）"
    );

    assert!(
        adapter.engine_enforced(),
        "postgres 存在引擎级强制兜底，engine_enforced 恒为 true（F-10/L-9）"
    );
}

// ============================================================ classify 归类阶段
//
// 以下断言针对**语法树归类阶段**（PostgresAdapter::classify），与上方
// `malformed_payload_is_parse_failed` 覆盖的**负载反序列化阶段**（PgRequest::from_payload）
// 是两个不同阶段：前者把已反序列化的语句原文解析为语法树并按最高危写节点定档，后者只判
// 负载字节能否反序列化为 PgRequest。failclosed-2 关切的 L-3（SET→Err）/ L-4（EXPLAIN
// ANALYZE 内含写→Destroy/Mutate 而非 Observe）/ L-5（解析失败/多语句/未知节点→Err→deny）
// 全在归类路径，故在此补齐归类级断言。语句原文（含 SET / EXPLAIN ANALYZE 写 / 多语句 /
// DO/COPY/CALL / CTE 藏写）全部放数据文件，本 .rs 零 SQL 标记（B 方案）。

#[derive(Deserialize)]
struct ClassifyCorpus {
    classify_ok: Vec<ClassifyOkCase>,
    classify_err: Vec<ClassifyErrCase>,
}

#[derive(Deserialize)]
struct ClassifyOkCase {
    /// 人读用例名，断言失败时定位用。
    name: String,
    /// 待归类的单条语句原文（数据文件提供，.rs 不含其字面量）。
    statement: String,
    /// 期望归类档名：query / observe / mutate / destroy。
    expect: String,
}

#[derive(Deserialize)]
struct ClassifyErrCase {
    /// 人读用例名。
    name: String,
    /// 待归类的语句原文（数据文件提供）。
    statement: String,
    /// 期望归类错误变体名：parse_failed / multi_statement / unclassifiable / unknown_construct。
    expect: String,
}

fn load_classify_corpus() -> ClassifyCorpus {
    serde_json::from_str(CLASSIFY_CORPUS).expect("intent_classify.json 应可解析为 ClassifyCorpus")
}

/// 把一条语句原文装成 postgres `Intent` 负载（`{statement, params:[]}` 形态），模拟外壳层
/// 装箱搬运后内核递给适配器的 `&Intent`。语句文本来自数据文件参数，非源码字面量。
fn intent_for(statement: &str) -> Intent {
    let payload = serde_json::to_vec(&serde_json::json!({
        "statement": statement,
        "params": [],
    }))
    .expect("Intent 负载应可序列化");
    Intent::new(payload)
}

/// 把期望档名映射为 `Capability`（query→Query 等）。未知档名即语料笔误，直接 panic。
fn capability_for(expect: &str) -> Capability {
    match expect {
        "query" => Capability::Query,
        "observe" => Capability::Observe,
        "mutate" => Capability::Mutate,
        "destroy" => Capability::Destroy,
        other => panic!("classify_ok 用例期望档名未知: {other}"),
    }
}

/// 把期望错误名映射为 `ClassifyError` 变体。未知名即语料笔误，直接 panic。
fn classify_error_for(expect: &str) -> ClassifyError {
    match expect {
        "parse_failed" => ClassifyError::ParseFailed,
        "multi_statement" => ClassifyError::MultiStatement,
        "unclassifiable" => ClassifyError::Unclassifiable,
        "unknown_construct" => ClassifyError::UnknownConstruct,
        other => panic!("classify_err 用例期望错误名未知: {other}"),
    }
}

// §8 F-2~F-5 / L-1 / L-2 / L-4 —— 归类阶段正向定档：每条语料 classify 后 capability 恰为
// 其真实最高危档。covers：纯只读→Query、写→Mutate、删除/删表/清表→Destroy、
// Show/非ANALYZE EXPLAIN→Observe、CTE/子查询藏写不降级、EXPLAIN ANALYZE 内含写按内部
// 最高危写定档（非 Observe）。无一条降级为低危档（单调取最大值）。
#[test]
fn classify_assigns_exact_highest_write_tier() {
    let corpus = load_classify_corpus();
    let adapter = PostgresAdapter;

    assert!(
        corpus.classify_ok.len() >= 15,
        "classify_ok 语料至少应覆盖 15 类正向形态，实得 {}",
        corpus.classify_ok.len()
    );

    for case in &corpus.classify_ok {
        let intent = intent_for(&case.statement);
        let ci = adapter
            .classify(&intent)
            .unwrap_or_else(|e| panic!("[{}] 合法可归类语句 classify 应成功，得 {e:?}", case.name));
        assert_eq!(
            ci.capability,
            capability_for(&case.expect),
            "[{}] 归类档应恰为 {}（按语句树内最高危写节点定档，不降级）",
            case.name,
            case.expect
        );
    }
}

// §8 L-3 / L-4 / L-5 —— 归类阶段失败一律 Err（fail-closed，公理二）：SET 任意形态 →
// Unclassifiable（改会话语义、无白名单逃生口，L-3）；多语句 → MultiStatement、空语句/匿名
// 块/语法垃圾 → ParseFailed（L-5）；COPY/CALL/PREPARE/建表 DDL 等白名单外形态 →
// UnknownConstruct（L-5）。每条恰为对应 ClassifyError 变体，绝不吞错放行、绝不归任何档。
#[test]
fn classify_rejects_unclassifiable_with_exact_error() {
    let corpus = load_classify_corpus();
    let adapter = PostgresAdapter;

    assert!(
        corpus.classify_err.len() >= 8,
        "classify_err 语料至少应覆盖 8 类失败形态，实得 {}",
        corpus.classify_err.len()
    );

    for case in &corpus.classify_err {
        let intent = intent_for(&case.statement);
        let result = adapter.classify(&intent);
        assert_eq!(
            result,
            Err(classify_error_for(&case.expect)),
            "[{}] 不可靠归类应恰返回 {}（fail-closed，绝不归档放行），实得 {result:?}",
            case.name,
            case.expect
        );
    }
}

// §8 L-3 —— SET 一律拒绝、无白名单逃生口（核心红线，单列强断言）：直接用一条 SET 语料
// 断言 classify 恰为 Err（任一 ClassifyError 即 deny），印证 SET 无任何放行分支。
#[test]
fn classify_set_is_always_denied() {
    let corpus = load_classify_corpus();
    let adapter = PostgresAdapter;

    let set_cases: Vec<&ClassifyErrCase> = corpus
        .classify_err
        .iter()
        .filter(|c| c.name.starts_with("l3_set_"))
        .collect();
    assert!(
        set_cases.len() >= 3,
        "应至少有 3 条 SET 形态语料覆盖会话语义篡改的不同写法，实得 {}",
        set_cases.len()
    );

    for case in set_cases {
        let intent = intent_for(&case.statement);
        let result = adapter.classify(&intent);
        assert!(
            result.is_err(),
            "[{}] SET 改会话语义、无归属动词，必须 deny（L-3），实得 {result:?}",
            case.name
        );
    }
}

// §8 L-4 —— EXPLAIN ANALYZE 内含写按内部最高危写定档、绝不降为 Observe（核心红线）：
// 直接断言 EXPLAIN ANALYZE DELETE → Destroy（真实执行被解释语句，非只读观测）。
#[test]
fn classify_explain_analyze_write_is_not_observe() {
    let corpus = load_classify_corpus();
    let adapter = PostgresAdapter;

    let case = corpus
        .classify_ok
        .iter()
        .find(|c| c.name == "l4_explain_analyze_delete_to_destroy_not_observe")
        .expect("语料应含 l4_explain_analyze_delete_to_destroy_not_observe 用例");

    let intent = intent_for(&case.statement);
    let ci = adapter
        .classify(&intent)
        .expect("EXPLAIN ANALYZE DELETE 应可归类（真实执行被解释语句）");
    assert_eq!(
        ci.capability,
        Capability::Destroy,
        "EXPLAIN ANALYZE 包裹的删除须按内部最高危写定档为 Destroy，绝不降为 Observe（L-4）"
    );
}
