//! postgres `check_constraint` 细则单元（F-8, L-7/L-8；设计 05 §3.2）。
//!
//! 被测：`PostgresAdapter::check_constraint(spec, ci) -> Result<bool, ConstraintError>`
//! ——一个纯函数（只读 `spec` 与已物化 `ci.objects`，不发 IO，§3.2）。本单元属主两 `kind`：
//!
//! - `table_allow`（集合包含类·**全称量化**）：`ci` 触达的**每一个** schema.table 都须落在
//!   `spec.tables` 白名单内；**任一**越界即 `Ok(false)`（绝非「有一个命中即放行」的存在
//!   量化 fail-open，L-8）。
//! - `column_mask`（**触达禁止类**，求值期形态）：`ci` 列集与 `spec.columns` 禁止集的交集
//!   **必须为空**，非空即 `Ok(false)`。它在求值期拒绝**触达**，区别于出口期擦响应的
//!   `mask_fields`——后者对本函数是未知 kind（两道防线不可混）。
//!
//! 三类期望逐条钉死（§3.2 / Trace①[4]）:
//! 1. **白名单内** → `Ok(true)`；
//! 2. **白名单外 / 触达禁止列** → `Ok(false)`（是「不通过」，不是 `Err`）；
//! 3. **判定信息缺失 / spec 非法 / 未知 kind** → `Err(ConstraintError)`——「判不了」等价
//!    「不通过」，**绝不** `Ok(true)`（L-7，公理二）。
//!
//! 表驱动：从 `tests/corpus/constraint_cases.json` 读 case 集；语料**直接给 `ci`**
//! （`capability` + `objects` 字符串），不经 classify——把单元收敛到细则判定一处。对象用
//! `ObjectRef` 字符串承载（表维度裸 `schema.table`、列维度 `col:` 前缀），故本 `.rs`
//! **零 SQL 文本标记**（连断言消息 / 注释 / 字面量都不含；B 方案）。

use serde::Deserialize;

use postern_core::domain::{Capability, ConstraintSpec};
use postern_core::error::ConstraintError;
use postern_core::plugin::Adapter;
use postern_core::request::{ClassifiedIntent, ObjectRef};

use postern_adapters::postgres::PostgresAdapter;

const CORPUS: &str = include_str!("corpus/constraint_cases.json");

#[derive(Deserialize)]
struct Corpus {
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    name: String,
    /// 细则 kind（`table_allow` / `column_mask`，或语料里的越权/未知 kind）。
    kind: String,
    /// `ConstraintSpec.spec` 的 raw JSON 负载（数据文件承载，含合法与非法两形态）。
    spec_json: String,
    /// 已物化的归类结果（直接给，不经 classify）。
    ci: CiInput,
    /// 期望：`{ok: bool}`（通过 / 不通过）或 `{err: variant}`（拒绝原因）。
    expect: Expect,
}

#[derive(Deserialize)]
struct CiInput {
    /// 归类档名（`observe`/`query`/`mutate`/`destroy`）。
    capability: String,
    /// 触达对象集的 `ObjectRef` 字符串（表维度裸 `schema.table`、列维度 `col:` 前缀）。
    objects: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum Expect {
    /// `Ok(bool)`：`true`=白名单内通过、`false`=白名单外/触达禁止集不通过。
    Ok(bool),
    /// `Err(ConstraintError)`：拒绝原因变体名（见 [`err_of`]）。
    Err(String),
}

/// 语料档名 → `Capability`（物化 `ClassifiedIntent.capability`）。
fn cap_of(s: &str) -> Capability {
    match s {
        "observe" => Capability::Observe,
        "query" => Capability::Query,
        "mutate" => Capability::Mutate,
        "destroy" => Capability::Destroy,
        other => panic!("语料档名非法: {other}"),
    }
}

/// 语料错误名 → `ConstraintError`（精确变体断言用；恰映射 core 三变体，无 `_` 兜底）。
fn err_of(s: &str) -> ConstraintError {
    match s {
        "unknown_kind" => ConstraintError::UnknownKind,
        "invalid_spec" => ConstraintError::InvalidSpec,
        "missing_objects" => ConstraintError::MissingObjects,
        other => panic!("语料错误名非法: {other}"),
    }
}

/// 把语料 `ci` 物化为 `ClassifiedIntent`（对象字符串逐一包成 `ObjectRef`）。
fn ci_of(input: &CiInput) -> ClassifiedIntent {
    ClassifiedIntent {
        capability: cap_of(&input.capability),
        objects: input.objects.iter().map(ObjectRef::new).collect(),
    }
}

/// 把语料 `kind` / `spec_json` 物化为 `ConstraintSpec`（spec 是 raw JSON 负载）。
fn spec_of(kind: &str, spec_json: &str) -> ConstraintSpec {
    ConstraintSpec {
        kind: kind.to_string(),
        spec: spec_json.to_string(),
    }
}

/// 表驱动主单元：逐条物化 `(spec, ci)` → `check_constraint` → 精确断言三类期望。
///
/// 每条钉死到具体 `bool` / 具体 `ConstraintError` 变体；`Ok(false)` 与 `Err` 互不混淆
/// （白名单外是「不通过」`Ok(false)`，判不了 / 非法是 `Err`），且**绝无**一条「判不了」
/// 退化成 `Ok(true)`（L-7 fail-closed）。
#[test]
fn check_constraint_corpus_three_classes() {
    let adapter = PostgresAdapter;
    let corpus: Corpus = serde_json::from_str(CORPUS).expect("constraint_cases.json 应可解析");
    assert!(
        !corpus.cases.is_empty(),
        "细则语料不应为空（§3.2 F-8 / L-7 / L-8：白名单内/外、触达禁止集、缺信息三类）"
    );

    for case in &corpus.cases {
        let spec = spec_of(&case.kind, &case.spec_json);
        let ci = ci_of(&case.ci);
        let got = adapter.check_constraint(&spec, &ci);

        match &case.expect {
            // ── 通过 / 不通过：精确到 bool；白名单外是 Ok(false) 而非 Err ──────────
            Expect::Ok(want) => match &got {
                Ok(passed) => assert_eq!(
                    *passed, *want,
                    "[{}] 期望 Ok({want})，得 Ok({passed})（{} 判定档错——白名单内须 true、\
                     白名单外/触达禁止集须 false）",
                    case.name, case.kind
                ),
                Err(e) => panic!(
                    "[{}] 期望 Ok({want})（细则判通过/不通过），却 Err({e:?})——\
                     白名单外/触达禁止集应是「不通过」Ok(false)，不应升格为拒绝原因",
                    case.name
                ),
            },
            // ── 拒绝：精确到 ConstraintError 变体；绝不退化为 Ok(true)（L-7） ─────────
            Expect::Err(want_name) => {
                let want = err_of(want_name);
                match &got {
                    Err(e) => assert_eq!(
                        *e, want,
                        "[{}] 期望 Err({want:?})，得 Err({e:?})（拒绝原因变体不可互换）",
                        case.name
                    ),
                    Ok(passed) => panic!(
                        "[{}] 期望 Err({want:?})（判不了/spec 非法/未知 kind），却 Ok({passed})\
                         ——「判不了」绝不放行，必须 Err 而非 Ok(true)（L-7 fail-closed）",
                        case.name
                    ),
                }
            }
        }
    }
}

// §8 / 设计 §3.2 逐条覆盖（语料外的不变量与边角，钉死签名承诺与 fail-closed 关键路径）。

/// §8 F-8 / Trace①[4]：`table_allow` 白名单**内**单表 → `Ok(true)`（白名单内放行）。
#[test]
fn s8_table_allow_inside_is_ok_true() {
    let adapter = PostgresAdapter;
    let spec = spec_of("table_allow", "{\"tables\":[\"public.orders\"]}");
    let ci = ClassifiedIntent {
        capability: Capability::Query,
        objects: vec![ObjectRef::new("public.orders")],
    };
    assert_eq!(adapter.check_constraint(&spec, &ci), Ok(true));
}

/// §8 F-8 / Trace①[4]：`table_allow` 白名单**外** → `Ok(false)`（不通过，非 `Err`）。
#[test]
fn s8_table_allow_outside_is_ok_false() {
    let adapter = PostgresAdapter;
    let spec = spec_of("table_allow", "{\"tables\":[\"public.orders\"]}");
    let ci = ClassifiedIntent {
        capability: Capability::Query,
        objects: vec![ObjectRef::new("public.payments")],
    };
    assert_eq!(adapter.check_constraint(&spec, &ci), Ok(false));
}

/// §8 L-8 全称量化（fail-closed 关键）：触达多表、仅一张越界 → 整体 `Ok(false)`。
///
/// 这是 fail-open 的反面教材锚点：存在量化（有一张命中即放行）会让此例错放行为
/// `Ok(true)`。判据必须是「每一张都在白名单内」，任一越界即不通过。
#[test]
fn s8_table_allow_universal_one_outside_is_false() {
    let adapter = PostgresAdapter;
    let spec = spec_of(
        "table_allow",
        "{\"tables\":[\"public.orders\",\"public.customers\"]}",
    );
    let ci = ClassifiedIntent {
        capability: Capability::Query,
        // 两张表：一张白名单内、一张越界——全称量化下整体不通过。
        objects: vec![
            ObjectRef::new("public.orders"),
            ObjectRef::new("public.payments"),
        ],
    };
    assert_eq!(
        adapter.check_constraint(&spec, &ci),
        Ok(false),
        "全称量化：触达多对象时仅一张越界即整体 Ok(false)（绝非存在量化 fail-open）"
    );
}

/// §8 F-8：`column_mask` 求值期触达**禁止列** → `Ok(false)`（拒绝触达）。
#[test]
fn s8_column_mask_touched_forbidden_is_false() {
    let adapter = PostgresAdapter;
    let spec = spec_of("column_mask", "{\"columns\":[\"public.customers.email\"]}");
    let ci = ClassifiedIntent {
        capability: Capability::Query,
        objects: vec![
            ObjectRef::new("public.customers"),
            ObjectRef::new("col:public.customers.email"),
        ],
    };
    assert_eq!(adapter.check_constraint(&spec, &ci), Ok(false));
}

/// §8 F-8：`column_mask` **不触达**禁止列 → `Ok(true)`（交集空即放行）。
#[test]
fn s8_column_mask_not_touched_is_true() {
    let adapter = PostgresAdapter;
    let spec = spec_of("column_mask", "{\"columns\":[\"public.customers.email\"]}");
    let ci = ClassifiedIntent {
        capability: Capability::Query,
        objects: vec![
            ObjectRef::new("public.orders"),
            ObjectRef::new("col:public.orders.total"),
        ],
    };
    assert_eq!(adapter.check_constraint(&spec, &ci), Ok(true));
}

/// §8 L-7：判定所需对象缺失（`table_allow` 无表维度对象）→ `Err(MissingObjects)`，
/// 绝不 `Ok(true)`——「判不了」等价「不通过」（fail-closed 关键，公理二）。
#[test]
fn s8_missing_objects_is_err_not_ok_true() {
    let adapter = PostgresAdapter;
    let spec = spec_of("table_allow", "{\"tables\":[\"public.orders\"]}");
    let ci = ClassifiedIntent {
        capability: Capability::Query,
        objects: vec![],
    };
    let got = adapter.check_constraint(&spec, &ci);
    assert_eq!(got, Err(ConstraintError::MissingObjects));
    assert_ne!(
        got,
        Ok(true),
        "判不了绝不放行：缺对象必 Err，不得 Ok(true)（L-7）"
    );
}

/// §8 / §3.2：`spec` 负载非法 JSON → `Err(InvalidSpec)`（解析失败不吞错放行）。
#[test]
fn s8_invalid_spec_json_is_err() {
    let adapter = PostgresAdapter;
    let spec = spec_of("table_allow", "{broken");
    let ci = ClassifiedIntent {
        capability: Capability::Query,
        objects: vec![ObjectRef::new("public.orders")],
    };
    assert_eq!(
        adapter.check_constraint(&spec, &ci),
        Err(ConstraintError::InvalidSpec)
    );
}

/// §8 / §3.2：未知 kind（非本适配器属主，且 `mask_fields` 是出口防线不在此求值）→
/// `Err(UnknownKind)`，绝不当 `column_mask` 误判。
#[test]
fn s8_unknown_kind_is_err() {
    let adapter = PostgresAdapter;
    let ci = ClassifiedIntent {
        capability: Capability::Query,
        objects: vec![ObjectRef::new("public.orders")],
    };
    // 跨协议 kind：http 属主，不在 postgres 属主集。
    let foreign = spec_of("http_route", "{\"routes\":[\"GET /api/orders\"]}");
    assert_eq!(
        adapter.check_constraint(&foreign, &ci),
        Err(ConstraintError::UnknownKind)
    );
    // 出口期 mask_fields：擦响应防线，不在求值期参与，对本函数即未知 kind。
    let exit_only = spec_of("mask_fields", "{\"fields\":[\"customers.email\"]}");
    assert_eq!(
        adapter.check_constraint(&exit_only, &ci),
        Err(ConstraintError::UnknownKind)
    );
}

/// §8 L-16：`check_constraint` 的返回是 `Result<bool, ConstraintError>`——只表「通过/
/// 不通过/判不了」，**不含** `Decision` / `CredentialTier`。本测试在类型层固定该承诺：
/// 把返回值绑定为 `Result<bool, ConstraintError>` 即编译期证明其不携带选档信息。
#[test]
fn s8_l16_returns_plain_bool_no_tier() {
    let adapter = PostgresAdapter;
    let spec = spec_of("table_allow", "{\"tables\":[\"public.orders\"]}");
    let ci = ClassifiedIntent {
        capability: Capability::Query,
        objects: vec![ObjectRef::new("public.orders")],
    };
    // 显式标注返回类型：若签名混入 Decision/CredentialTier 即类型不符、无法编译。
    let r: Result<bool, ConstraintError> = adapter.check_constraint(&spec, &ci);
    assert!(r.is_ok());
}
