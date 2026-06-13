//! 伪装攻击语料全集单元（L-1/L-2/L-6，§7/§8 二组，核心红队）。
//!
//! 表驱动：从 `tests/corpus/disguise.json` 读伪装攻击 case 全集（写 CTE 包裹 / 子查询藏写 /
//! 多语句 / 注释混淆 / 匿名块 DO / 会话语义篡改 SET / `EXPLAIN ANALYZE` 写），逐条断言
//! **两层**：
//! 1. **精确档**：`classify` 结果恰为语料 `expect`——归其**真实最高危档**（destroy/mutate）
//!    或具体 `Err`（multi/parse/unclassifiable）。档错绝不互换。
//! 2. **绝不降级（fail-closed 核心）**：结果**绝不低于** `min_tier`——把伪装写归为低危档
//!    （query/observe）放行即判红。`Err` 视为「拒绝」=最安全，永不构成降级。
//!
//! 语料（含 SQL 原文）全部放数据文件，本 `.rs` 只含数据文件路径与期望档名 / 错误变体的
//! 符号名，零 SQL 标记（B 方案）。

use serde::Deserialize;

use postern_core::domain::Capability;
use postern_core::error::ClassifyError;
use postern_core::request::Intent;

use postern_adapters::postgres::classify::classify;
use postern_adapters::postgres::intent::PgRequest;

const CORPUS: &str = include_str!("corpus/disguise.json");

#[derive(Deserialize)]
struct Corpus {
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    name: String,
    /// 伪装写 SQL 原文（数据文件承载，本 `.rs` 不含其字面量）。
    sql: String,
    /// 期望：真实最高危档名（destroy/mutate）或具体错误（err_multi/err_parse/
    /// err_unclassifiable）。
    expect: String,
    /// 该伪装写的真实危险下界——结果绝不低于此（低于即降级放行）。
    min_tier: String,
    /// 可选：期望采集到的对象全集（点分 schema.table，去重稳定排序后逐字相等）。
    /// 仅当语料显式声明时断言——用于钉死「写外壳的写目标必被采集」（如 INTO 目标），
    /// 漏采写目标即对象视图与定档不一致（§3.1）。
    #[serde(default)]
    objects: Option<Vec<String>>,
}

/// 危险度全序（与 §3.1 一致）：只读档 < Mutate < Destroy。归低于真实下界即「降级放行」。
fn danger_rank(cap: Capability) -> u8 {
    match cap {
        // 只读档：Observe / Query / Execute / Manage 在本语料语境均属「非写」最低危一档。
        Capability::Observe | Capability::Query | Capability::Execute | Capability::Manage => 0,
        Capability::Mutate => 1,
        Capability::Destroy => 2,
    }
}

/// 语料档名 → `Capability`（精确档断言用）。
fn cap_of(s: &str) -> Capability {
    match s {
        "observe" => Capability::Observe,
        "query" => Capability::Query,
        "mutate" => Capability::Mutate,
        "destroy" => Capability::Destroy,
        other => panic!("语料档名非法: {other}"),
    }
}

/// 语料错误名 → `ClassifyError`（精确变体断言用）。
fn err_of(s: &str) -> ClassifyError {
    match s {
        "err_parse" => ClassifyError::ParseFailed,
        "err_multi" => ClassifyError::MultiStatement,
        "err_unknown" => ClassifyError::UnknownConstruct,
        "err_unclassifiable" => ClassifyError::Unclassifiable,
        other => panic!("语料错误名非法: {other}"),
    }
}

fn intent_for(sql: &str) -> Intent {
    let req = PgRequest {
        statement: sql.to_string(),
        params: vec![],
    };
    let bytes = serde_json::to_vec(&req).expect("PgRequest 应可序列化为 Intent 负载");
    Intent::new(bytes)
}

#[test]
fn disguise_corpus_never_downgrades() {
    let corpus: Corpus = serde_json::from_str(CORPUS).expect("disguise.json 应可解析");
    assert!(
        !corpus.cases.is_empty(),
        "伪装攻击语料不应为空（§8 二组 L-6:CTE/子查询/多语句/注释/DO/SET/EXPLAIN ANALYZE 写）"
    );

    for case in &corpus.cases {
        let got = classify(&intent_for(&case.sql));

        // ── 层 1：精确档 / 精确错误 ───────────────────────────────────────────
        if case.expect.starts_with("err_") {
            let expected = err_of(&case.expect);
            match &got {
                Err(e) => assert_eq!(
                    *e, expected,
                    "[{}] 伪装写期望 Err({expected:?})，得 Err({e:?})（档错绝不互换）",
                    case.name
                ),
                Ok(ci) => panic!(
                    "[{}] 伪装写期望 Err({expected:?})，却归类放行为 {:?}（fail-closed 失效）",
                    case.name, ci.capability
                ),
            }
        } else {
            let expected_cap = cap_of(&case.expect);
            let ci = got.as_ref().unwrap_or_else(|e| {
                panic!(
                    "[{}] 伪装写期望 Ok({expected_cap:?})，却 Err({e:?})",
                    case.name
                )
            });
            assert_eq!(
                ci.capability, expected_cap,
                "[{}] 伪装写必归真实最高危档 {expected_cap:?}（穿透外壳，绝不降级）",
                case.name
            );

            // 可选：对象全集逐字相等——钉死写外壳的写目标必被采集（如 INTO 物化目标），
            // 与定档看到同一对象视图（§3.1）；漏采写目标即对象视图不一致。
            if let Some(expected_objs) = &case.objects {
                let got_objs: Vec<&str> = ci.objects.iter().map(|o| o.as_str()).collect();
                assert_eq!(
                    got_objs, expected_objs.as_slice(),
                    "[{}] 对象视图不符:期望 {expected_objs:?}，得 {got_objs:?}（写外壳的写目标须被采集，§3.1 同遍历）",
                    case.name
                );
            }
        }

        // ── 层 2：绝不降级（核心）——结果绝不低于真实危险下界 min_tier ──────────
        let floor = danger_rank(cap_of(&case.min_tier));
        match &got {
            // Err = 拒绝 = 最安全，永不构成降级放行。
            Err(_) => {}
            // Ok 放行：其危险档必 >= 真实下界，否则即「伪装写被降为低危档放行」。
            Ok(ci) => assert!(
                danger_rank(ci.capability) >= floor,
                "[{}] 伪装写被降级放行:归 {:?}(rank {})低于真实下界 {}(rank {})——降级放行即破网（L-6 fail-closed）",
                case.name,
                ci.capability,
                danger_rank(ci.capability),
                case.min_tier,
                floor
            ),
        }
    }
}
