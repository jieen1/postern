//! postgres `classify` 归类单元（§8: F-2~F-5, L-1~L-5, L-16）。
//!
//! 表驱动：从 `tests/corpus/classify_cases.json` 读 case 集，逐条
//! `classify(case.sql)` 并断言**恰为** case.expect（observe/query/mutate/destroy 档名或
//! err_*）。语料（含 SQL 原文）全部放数据文件，本 `.rs` 只含数据文件路径与期望档名 / 错误
//! 变体的符号名，零 SQL 标记（B 方案）。断言失败信息只用 case 名 / 期望符号名，绝不回显
//! sql 文本（注释、断言消息、字符串字面量一律零 SQL 标记）。
//!
//! 每条断言精确到具体 `Capability` 档 / 具体 `ClassifyError` 变体：档错绝不互换、写绝不
//! 降级（顶层删除恒 Destroy、CTE/子查询藏写穿透提升不下调）、解析失败 / 多语句 / 未知形态 /
//! 会话语义篡改一律 `Err`（fail-closed，公理二）。可选 `objects` 钉死「定档与对象提取同
//! 遍历看到一致对象视图」（§3.1，去重稳定排序后逐字段相等）。

use serde::Deserialize;

use postern_core::domain::Capability;
use postern_core::error::ClassifyError;
use postern_core::request::{ClassifiedIntent, Intent, ObjectRef};

use postern_adapters::postgres::classify::classify;
use postern_adapters::postgres::intent::PgRequest;

const CORPUS: &str = include_str!("corpus/classify_cases.json");

#[derive(Deserialize)]
struct Corpus {
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    /// 用例名（断言失败时定位，不回显 sql 文本）。
    name: String,
    /// SQL 原文（数据文件承载，本 `.rs` 不含其字面量）。
    sql: String,
    /// 期望：档名（observe/query/mutate/destroy）或错误变体（err_parse/err_multi/
    /// err_unknown/err_unclassifiable）。
    expect: String,
    /// 可选：期望对象集（schema.table 点分，去重稳定排序后逐字段相等）。
    #[serde(default)]
    objects: Option<Vec<String>>,
}

/// 把 SQL 原文包成 postgres `Intent` 负载（`{statement, params}` 对象形态）。
fn intent_for(sql: &str) -> Intent {
    let req = PgRequest {
        statement: sql.to_string(),
        params: vec![],
    };
    let bytes = serde_json::to_vec(&req).expect("PgRequest 应可序列化为 Intent 负载");
    Intent::new(bytes)
}

/// 语料档名 → `Capability`（断言用，精确到具体档；非法档名即语料错）。
fn cap_of(s: &str) -> Capability {
    match s {
        "observe" => Capability::Observe,
        "query" => Capability::Query,
        "mutate" => Capability::Mutate,
        "destroy" => Capability::Destroy,
        other => panic!("语料 expect 档名非法: {other}"),
    }
}

/// 语料错误名 → `ClassifyError`（断言用，精确到具体变体）。
fn err_of(s: &str) -> ClassifyError {
    match s {
        "err_parse" => ClassifyError::ParseFailed,
        "err_multi" => ClassifyError::MultiStatement,
        "err_unknown" => ClassifyError::UnknownConstruct,
        "err_unclassifiable" => ClassifyError::Unclassifiable,
        other => panic!("语料 expect 错误名非法: {other}"),
    }
}

/// 断言单条 case：成功路径钉档 + 可选对象集；失败路径钉具体错误变体。
fn assert_case(case: &Case) {
    let got = classify(&intent_for(&case.sql));

    // 失败路径一等公民：expect 以 `err_` 起头者必为 Err，且变体恰为语料指定
    // （fail-closed，绝不放行 / 绝不分流到别的变体）。
    if case.expect.starts_with("err_") {
        let expected = err_of(&case.expect);
        match got {
            Err(e) => assert_eq!(
                e, expected,
                "[{}] 期望 Err({expected:?})，得 Err({e:?})（错误变体绝不互换）",
                case.name
            ),
            Ok(ci) => panic!(
                "[{}] 期望 Err({expected:?})，却归类放行为 {:?}（fail-closed 失效）",
                case.name, ci.capability
            ),
        }
        return;
    }

    // 成功路径：必为 Ok，且档恰为语料指定（写绝不降级、读绝不提升）。
    let expected_cap = cap_of(&case.expect);
    let ci = match got {
        Ok(ci) => ci,
        Err(e) => panic!("[{}] 期望 Ok({expected_cap:?})，却 Err({e:?})", case.name),
    };
    assert_eq!(
        ci.capability, expected_cap,
        "[{}] 归类档必恰为 {expected_cap:?}（写绝不降级、读绝不提升）",
        case.name
    );

    // 可选对象集：钉死定档与对象提取同遍历看到一致对象视图（§3.1，去重稳定排序）。
    if let Some(expected_objs) = &case.objects {
        let want: Vec<ObjectRef> = expected_objs.iter().map(ObjectRef::new).collect();
        assert_eq!(
            ci.objects, want,
            "[{}] 对象集必恰为 {expected_objs:?}（去重稳定排序后逐字段相等）",
            case.name
        );
    }
}

/// §8 全表覆盖：F-2~F-5（纯读/写/删/观测分档）、L-1/L-2（CTE/子查询藏写穿透提升不降级）、
/// L-3（任意 SET 形态拒）、L-4（EXPLAIN ANALYZE 取内部最高危）、L-5（解析失败/多语句/匿名块/
/// 拷贝/过程调用 fail-closed）。逐条断言**恰为** case.expect。
#[test]
fn classify_corpus_matches_expected_exactly() {
    let corpus: Corpus = serde_json::from_str(CORPUS).expect("classify_cases.json 应可解析");
    assert!(
        !corpus.cases.is_empty(),
        "classify 语料不应为空（须覆盖 §8 F-2~F-5 / L-1~L-5）"
    );

    for case in &corpus.cases {
        assert_case(case);
    }
}

/// §8 F-2/L-1：纯只读查询顶层 Query 归 `Query`；而同一只读外壳一旦其 CTE/子查询藏删除，
/// 必穿透提升到 `Destroy`、绝不停留 `Query`。把「不降级」单独锚定一条，确保只读外壳不是
/// 放行后门（与表驱动互为冗余防护）。
#[test]
fn readonly_shell_hiding_destroy_never_downgrades() {
    let corpus: Corpus = serde_json::from_str(CORPUS).expect("classify_cases.json 应可解析");

    let read = corpus
        .cases
        .iter()
        .find(|c| c.name == "F2_pure_read_select")
        .expect("语料应含纯只读查询用例 F2_pure_read_select");
    let hidden_destroy = corpus
        .cases
        .iter()
        .find(|c| c.name == "L1_cte_delete_no_downgrade_destroy")
        .expect("语料应含 CTE 藏删用例 L1_cte_delete_no_downgrade_destroy");

    let read_ci = classify(&intent_for(&read.sql)).expect("纯只读应 Ok");
    assert_eq!(read_ci.capability, Capability::Query, "纯只读查询归 Query");

    let hidden_ci = classify(&intent_for(&hidden_destroy.sql))
        .expect("CTE 藏删的顶层是只读外壳，仍应 Ok（穿透提升而非拒）");
    assert_eq!(
        hidden_ci.capability,
        Capability::Destroy,
        "只读外壳藏删必穿透提升到 Destroy，绝不降级 Query（L-1 根因）"
    );
    assert_ne!(
        hidden_ci.capability,
        Capability::Query,
        "只读外壳藏删绝不归 Query（不降级断言，L-1）"
    );
}

/// §8 L-16：`classify` 返回类型恰为 `Result<ClassifiedIntent, ClassifyError>`——
/// **不含** `Decision` / `CredentialTier`。归类只产出 `Capability` 档与对象集，授权与凭据
/// 分级是内核 / 连接层的事，绝不在归类返回里泄漏。此处用类型级绑定钉死签名承诺：若 src
/// 把返回类型改成含 `Decision`/`CredentialTier` 的结构，此函数取址绑定即编译失败。
#[test]
fn classify_signature_excludes_decision_and_tier() {
    // 类型级承诺：函数指针类型必须精确匹配，否则编译期即拒（L-16）。
    let f: fn(&Intent) -> Result<ClassifiedIntent, ClassifyError> = classify;
    // 运行期顺带触达一次（不影响类型断言；用纯只读 case 以免桩阶段误判）。
    let _ = f;
}
