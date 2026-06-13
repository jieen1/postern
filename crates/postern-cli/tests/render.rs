//! 信封三分支渲染与雪花-字符串纪律的行为测试（RED）。
//!
//! 被测对象：`postern_cli::render` 的信封三分支渲染面——
//! `envelope`（`{error:{code,message}}` 统一错误信封）、`table`（`Page<T>` 对齐表格 +
//! `--format jsonl` 逐行）、`deny_view`（`DenyResponse`/`DeniedFacts` 视图）。
//!
//! 测试策略（07-postern-cli §3.3/§3.4/§9）：喂固定 JSON 响应（无需真实 daemon），断言
//! 输出 / 字段 / 错误类型恰为预期。每条只钉一个行为，断言精确到具体值 / 变体 / 错误字段。
//! 失败路径一等公民：缺字段 → 解析错误（不补全、不当成功），错误信封 → 逐字符原样。
//!
//! 关键事实（雷区）：core 的 `DenyResponse`/`DeniedFacts`/`Capability`/`ObjectRef` 只
//! derive Serialize（不 Deserialize），CLI 不能 `serde_json::from_*` 进它们——故本测试只
//! 喂字节给本 unit 自定义的 `#[derive(Deserialize)]` 视图（`DenyView`/`RowView`/
//! `ErrorEnvelope`），从不构造 core 那些只读类型，亦不构造任何机密族类型。
//!
//! 覆盖 §8 条目：F-4（`Page<T>` 表格回显 `version`；错误信封逐字符；`--format jsonl`
//! 逐行可解析）、F-5（>2^53 雪花 id 字符串字节相等 + id 字段静态类型为 `String`）、
//! L-3（缺 `decision` 畸形拒绝响应 → 解析错误、不补默认）、L-4（拒绝事实逐项相等、错误
//! 信封原样、无 CLI 追加话术）。

use std::collections::BTreeMap;

use postern_core::page::Page;

use postern_cli::error::CliError;
use postern_cli::render::deny_view::{parse_deny, render_deny, DenyView};
use postern_cli::render::envelope::{parse_error_envelope, render_error};
use postern_cli::render::table::{render_jsonl, render_page, render_page_envelope, RowView};
use postern_cli::render::Format;

// >2^53 的雪花 id 样本（F-5）：2^53 = 9_007_199_254_740_992，本值远小于它的"大整数感"
// 不重要——关键是它一旦被读入 IEEE-754 双精度或经科学计数格式化即丢精度。CLI 必须从类型层
// 把它当字符串透传。
const BIG_SNOWFLAKE_ID: &str = "7300000000000000123";

// ── 辅助：构造定值 Page<RowView>（不触 core 只读类型、不触机密族） ──────────────

/// 构造一个携指定 `version` 与单行（含 >2^53 字符串 id）的定值 `Page<RowView>`。
/// 本辅助同时是 F-5 的**构造签名守卫**：`id` 只能填 `String`——若 `RowView::id` 是
/// 整型，这段 `Some(BIG_SNOWFLAKE_ID.to_string())` 直接不编译。
fn page_one_row(version: u64) -> Page<RowView> {
    Page {
        items: vec![RowView {
            id: Some(BIG_SNOWFLAKE_ID.to_string()),
            principal_id: None,
            resource_id: None,
            credential_id: None,
            name: Some("agent3".to_string()),
            version: Some(version),
        }],
        page_no: 1,
        page_size: 20,
        total: 1,
    }
}

// ── F-4：信封与错误渲染 ───────────────────────────────────────────────────────

/// §8 F-4：喂 `Page<T>` JSON → 渲染出 items 表格且原样回显该响应携带的 `version`。
/// 钉"输出真正含该 `version` 文本，且该出现非由其它字段的数字伪满足"（供乐观锁回传）。
///
/// 判别有效性（mutation-killing，防止断言被无关字符旁路满足）：
///   1. 选 `version=9`——本行 `id`=BIG_SNOWFLAKE_ID 仅含数字 {0,1,2,3,7}、`name`="agent3"
///      仅 {3}、单行页脚 `page 1 / size 20 / has_next false / total 1` 仅 {0,1,2}，故 '9'
///      在 version 之外的全部渲染内容里都不出现；前置自检把这一前提钉死。
///   2. 差分对照：渲染同一页但把 `version` 置空（`None`）→ 两份输出必须**不同**，且只有
///      携带 version 的那份含 '9'。若实现彻底丢掉 version 列（如 `String::new()`），两份输出
///      相等、'9' 从输出消失，本测试 FAIL。
#[test]
fn render_page_echoes_response_version_in_output() {
    let page = page_one_row(9);
    let rendered = render_page(&page).expect("page renders");

    // 前置自检：'9' 在 version 之外的渲染内容里确实不出现——否则断言会被旁路满足。
    assert!(
        !BIG_SNOWFLAKE_ID.contains('9'),
        "test invariant broken: id must not contain the version digit '9'"
    );
    assert!(
        !"agent3".contains('9'),
        "test invariant broken: name must not contain the version digit '9'"
    );

    // 差分对照：同一页去掉 version 后必须产生不同的输出，证明 version 被真正渲染而非被忽略。
    let mut page_without_version = page_one_row(9);
    page_without_version.items[0].version = None;
    let rendered_without_version =
        render_page(&page_without_version).expect("version-stripped page renders");

    assert_ne!(
        rendered, rendered_without_version,
        "dropping the version column must change the table output (version is not relayed otherwise)"
    );
    assert!(
        rendered.contains('9'),
        "table output must echo the carried version 9 for optimistic-lock relay, got: {rendered}"
    );
    assert!(
        !rendered_without_version.contains('9'),
        "the version digit must come only from the version column, got: {rendered_without_version}"
    );
}

/// §8 F-4：喂 `Page<T>` JSON → 渲染出 items 表格，行内 `name` 列原样出现（列取自 DTO
/// 字段）。钉"表格含该行的可读字段值"。
#[test]
fn render_page_table_contains_item_field_value() {
    let page = page_one_row(3);
    let rendered = render_page(&page).expect("page renders");
    assert!(
        rendered.contains("agent3"),
        "items table must show the row's name column value, got: {rendered}"
    );
}

/// §8 F-4 / §3.3：喂 `{error:{code,message}}` → 输出原样含该 `code` 与 `message`，
/// **逐字符不增删（zero characters added/removed）**。该不变量完全机械可验：输出必须恰为
/// `code` + 单一分隔符 + `message`，不在外侧包裹任何 CLI 自造话术——故这里用 `assert_eq!`
/// 钉死全文，而非松散的 `contains` 子串检查。
///
/// 判别有效性（mutation-killing）：若实现在 `code`/`message` 外侧加 prose（前缀 / 后缀 /
/// 改写分隔符），全文即不再相等，本断言 FAIL。下方两条 `contains` 仅作子串文档说明，
/// 真正钉"零增删"的是 `assert_eq!`。
#[test]
fn render_error_envelope_echoes_code_and_message_verbatim() {
    let bytes = br#"{"error":{"code":"E_CONFLICT","message":"version mismatch, re-read latest"}}"#;
    let env = parse_error_envelope(bytes).expect("error envelope parses");
    let rendered = render_error(&env).expect("error envelope renders");

    // 全文精确钉死：恰为 code + 单空格分隔 + message，零增删字符、无外侧话术。
    assert_eq!(
        rendered, "E_CONFLICT version mismatch, re-read latest",
        "error render must be exactly `code message` with zero characters added/removed, got: {rendered}"
    );
    // 子串文档（非主断言）：code 与 message 必须逐字原样出现。
    assert!(
        rendered.contains("E_CONFLICT"),
        "code must be echoed verbatim, got: {rendered}"
    );
    assert!(
        rendered.contains("version mismatch, re-read latest"),
        "message must be echoed verbatim with zero chars added/removed, got: {rendered}"
    );
}

/// §8 F-4 / L-4：错误信封渲染**不增删字符**——CLI 不在 `message` 外侧附加任何引导 /
/// 建议话术。钉"输出不含 CLI 自造的固定话术串"（如 "try"/"suggestion"/"hint:"）。
#[test]
fn render_error_envelope_appends_no_cli_prose() {
    let bytes = br#"{"error":{"code":"E_DENIED","message":"access denied"}}"#;
    let env = parse_error_envelope(bytes).expect("error envelope parses");
    let rendered = render_error(&env).expect("error envelope renders");
    for prose in [
        "suggestion",
        "you should",
        "try running",
        "hint:",
        "recommended",
    ] {
        assert!(
            !rendered.to_lowercase().contains(prose),
            "CLI must not append prose {prose:?} to a sanitized error message, got: {rendered}"
        );
    }
}

/// §8 F-4：`--format jsonl` → 输出逐行 JSON，每行可被 JSON 独立解析。钉"非空行数 = items
/// 数，且每行 `serde_json::from_str::<serde_json::Value>` 成功且是对象"。
#[test]
fn render_jsonl_emits_one_parseable_object_per_line() {
    let mut page = page_one_row(2);
    // 追加第二行，验证"逐行"而非"整体一个 JSON 数组"。
    page.items.push(RowView {
        id: Some("7300000000000000999".to_string()),
        principal_id: None,
        resource_id: None,
        credential_id: None,
        name: Some("agent4".to_string()),
        version: Some(5),
    });
    page.total = 2;

    let rendered = render_jsonl(&page).expect("jsonl renders");
    let lines: Vec<&str> = rendered.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(
        lines.len(),
        2,
        "jsonl must print one line per paged item (no client re-aggregation), got: {rendered}"
    );
    for line in lines {
        let value: serde_json::Value =
            serde_json::from_str(line).expect("each jsonl line is independently JSON-parseable");
        assert!(
            value.is_object(),
            "each jsonl line must be a JSON object, got line: {line}"
        );
    }
}

// ── F-5：雪花 id 字符串渲染（>2^53 字节相等 + 静态类型 String） ──────────────────

/// §8 F-5：喂含 id = 7300000000000000123（>2^53）的响应 → 渲染输出该 id 字符串与输入逐字符
/// 相等（无精度丢失、无科学计数）。钉"输出含该精确字符串、且不含科学计数形态 'e' 表示"。
#[test]
fn render_page_renders_big_snowflake_id_byte_identical() {
    let page = page_one_row(1);
    let rendered = render_page(&page).expect("page renders");
    assert!(
        rendered.contains(BIG_SNOWFLAKE_ID),
        "big snowflake id must render byte-identical as string, got: {rendered}"
    );
    // 科学计数形态（如 7.3e18）会含 'e'/'E' 紧邻数字——精度丢失的征兆，必须缺席。
    assert!(
        !rendered.contains("e18") && !rendered.contains("E18") && !rendered.contains("e+"),
        "id must never render in scientific notation, got: {rendered}"
    );
}

/// §8 F-5：`--format jsonl` 下 id 仍为字符串——逐行 JSON 里 `id` 字段值是 JSON 字符串
/// `"7300000000000000123"`，绝非 JSON 数字。钉"该行解析出的 `id` 是字符串且等于输入"。
#[test]
fn render_jsonl_keeps_big_id_as_json_string() {
    let page = page_one_row(1);
    let rendered = render_jsonl(&page).expect("jsonl renders");
    let line = rendered
        .lines()
        .find(|l| !l.trim().is_empty())
        .expect("at least one jsonl line");
    let value: serde_json::Value = serde_json::from_str(line).expect("jsonl line parses");
    let id = value.get("id").expect("row carries an id field");
    assert_eq!(
        id.as_str(),
        Some(BIG_SNOWFLAKE_ID),
        "id must remain a JSON string equal to the input, never a number, got: {id:?}"
    );
}

/// §8 F-5（构造签名检查）：CLI 视图 DTO 的每个 id 字段静态类型是 `String`，从类型层杜绝
/// JSON 数字解析路径。本测试若 `RowView` 的任一 id 字段是整型即不编译——构造即守卫。
/// 同时验证：喂"id 是 JSON 数字"的畸形行 → 反序列化失败（String 字段不接受数字）。
#[test]
fn row_view_id_fields_are_statically_string_typed() {
    // 编译期守卫：四个 id 字段都只接受 `String`。
    let row = RowView {
        id: Some(BIG_SNOWFLAKE_ID.to_string()),
        principal_id: Some("7300000000000000124".to_string()),
        resource_id: Some("7300000000000000125".to_string()),
        credential_id: Some("7300000000000000126".to_string()),
        name: None,
        version: Some(0),
    };
    assert_eq!(row.id.as_deref(), Some(BIG_SNOWFLAKE_ID));
    assert_eq!(row.principal_id.as_deref(), Some("7300000000000000124"));

    // 运行期守卫：`id` 是 JSON 数字（无引号）的行不能反序列化进 String 字段——证明
    // 不存在"读入整型再格式化"的精度丢失路径。
    let numeric_id_row = br#"{"id":7300000000000000123}"#;
    let parsed: Result<RowView, _> = serde_json::from_slice(numeric_id_row);
    assert!(
        parsed.is_err(),
        "a numeric (unquoted) id must be rejected by the String-typed id field"
    );
}

// ── L-3：响应不可解析即报错（缺 decision 不静默成功） ──────────────────────────

/// §8 L-3：喂缺 `decision` 字段的畸形 `DenyResponse` JSON → 解析失败返回
/// `CliError::DecodeFailed`，不补默认值、不当成功渲染。钉"返回 Err 且变体恰为
/// DecodeFailed"。
#[test]
fn parse_deny_missing_decision_returns_decode_failed() {
    // 完整的 deny 形状，唯独缺 `decision`——L-3 的精确触发点。
    let malformed = br#"{
        "denied": {"resource": "redis-main", "capability": "destroy", "objects": []},
        "reason": "no grant cell",
        "your_grants": {},
        "request_hint": null
    }"#;
    let result = parse_deny(malformed);
    match result {
        Err(CliError::DecodeFailed { .. }) => {}
        other => panic!(
            "missing `decision` must yield CliError::DecodeFailed (fail-closed, no default-fill), got: {other:?}"
        ),
    }
}

/// §8 L-3：缺 `decision` 绝不被"补默认值后当成功"——`parse_deny` 对此输入必返回 `Err`，
/// 从不返回一个 `decision` 被默认填充的 `Ok(DenyView)`。钉"结果是 Err，不是任何 Ok"。
#[test]
fn parse_deny_missing_decision_is_never_silently_filled() {
    let malformed = br#"{
        "denied": {"resource": "db-main", "capability": "mutate", "objects": []},
        "reason": "policy fact",
        "your_grants": {}
    }"#;
    let result = parse_deny(malformed);
    assert!(
        result.is_err(),
        "missing decision must not be default-filled and returned as Ok(success)"
    );
}

/// §8 L-3：`denied.capability` 类型错（喂 JSON 数字而非动词字符串）→ 同样解析失败为
/// DecodeFailed，不忽略字段、不当成功。钉"类型错也走 DecodeFailed"。
#[test]
fn parse_deny_wrong_typed_field_returns_decode_failed() {
    let wrong_typed = br#"{
        "decision": "deny",
        "denied": {"resource": "db-main", "capability": 42, "objects": []},
        "reason": "policy fact",
        "your_grants": {}
    }"#;
    let result = parse_deny(wrong_typed);
    match result {
        Err(CliError::DecodeFailed { .. }) => {}
        other => panic!("a wrong-typed field must yield CliError::DecodeFailed, got: {other:?}"),
    }
}

// ── L-4：结构化拒绝原样转述（字段值逐项相等、无追加话术） ──────────────────────

/// §8 L-4：喂含 `reason`/`your_grants` 的合法拒绝响应 → 反序列化出的 `DenyView` 字段值与
/// 输入逐项相等（无 CLI 改写）。钉"`reason` 文本恰等于输入、`your_grants` 映射逐项相等"。
#[test]
fn parse_deny_preserves_reason_and_grants_item_for_item() {
    let body = br#"{
        "decision": "deny",
        "denied": {"resource": "redis-main", "capability": "destroy", "objects": ["key:session"]},
        "reason": "role observer lacks destroy on redis-main",
        "your_grants": {"redis-main": ["observe", "query"]},
        "request_hint": "postern elevate agent2 redis-main destroy"
    }"#;
    let view = parse_deny(body).expect("legal deny parses");

    assert_eq!(view.decision, "deny");
    assert_eq!(view.denied.resource, "redis-main");
    assert_eq!(view.denied.capability, "destroy");
    assert_eq!(view.denied.objects, vec!["key:session".to_string()]);
    assert_eq!(view.reason, "role observer lacks destroy on redis-main");

    let mut expected_grants: BTreeMap<String, Vec<String>> = BTreeMap::new();
    expected_grants.insert(
        "redis-main".to_string(),
        vec!["observe".to_string(), "query".to_string()],
    );
    assert_eq!(view.your_grants, expected_grants);
    assert_eq!(
        view.request_hint.as_deref(),
        Some("postern elevate agent2 redis-main destroy")
    );
}

/// §8 L-4：渲染拒绝视图 → 输出字段值与输入逐项相等，无 CLI 追加话术。钉"`reason` 原样
/// 出现在输出，且输出不含 CLI 自造的固定引导话术"。
#[test]
fn render_deny_outputs_field_values_without_appended_prose() {
    let view = DenyView {
        decision: "deny".to_string(),
        denied: postern_cli::render::deny_view::DeniedFactsView {
            resource: "db-main".to_string(),
            capability: "mutate".to_string(),
            objects: vec!["table:orders".to_string()],
        },
        reason: "no grant cell for mutate on db-main".to_string(),
        your_grants: BTreeMap::new(),
        request_hint: None,
        operator_note: Some("ask the on-call DBA".to_string()),
    };
    let rendered = render_deny(&view).expect("deny view renders");

    assert!(
        rendered.contains("no grant cell for mutate on db-main"),
        "reason must be relayed verbatim, got: {rendered}"
    );
    assert!(
        rendered.contains("ask the on-call DBA"),
        "operator_note must be relayed verbatim, got: {rendered}"
    );
    for prose in ["suggestion", "you should", "recommended", "try running"] {
        assert!(
            !rendered.to_lowercase().contains(prose),
            "CLI must not invent guidance prose {prose:?} around deny facts, got: {rendered}"
        );
    }
}

/// §8 L-4：错误信封的已脱敏 `message` 原样回显——绝不被 CLI 展开成真实地址或底层原因。
/// 钉"输出含已脱敏 message、且不含真实地址样本"。脱敏文案是 daemon 侧常量。
#[test]
fn render_error_does_not_expand_sanitized_message_into_real_address() {
    let bytes = br#"{"error":{"code":"E_UPSTREAM","message":"upstream resource unavailable"}}"#;
    let env = parse_error_envelope(bytes).expect("error envelope parses");
    let rendered = render_error(&env).expect("error renders");
    assert!(
        rendered.contains("upstream resource unavailable"),
        "sanitized message must be echoed verbatim, got: {rendered}"
    );
    // CLI 结构上无真实地址可触达；绝不把脱敏文案展开为地址 / 端口 / 主机样本。
    for leak in ["10.0.", "5432", "localhost:", "127.0.0.1"] {
        assert!(
            !rendered.contains(leak),
            "CLI must never expand a sanitized message into a real address {leak:?}, got: {rendered}"
        );
    }
}

// ── 信封分流：缺字段错误信封不被当成功 ───────────────────────────────────────

/// §8 L-3（错误信封侧）：缺 `message` 的畸形错误信封 → `parse_error_envelope` 解析失败为
/// DecodeFailed，不补空串、不当成功。钉"返回 DecodeFailed 变体"。
#[test]
fn parse_error_envelope_missing_message_returns_decode_failed() {
    let malformed = br#"{"error":{"code":"E_X"}}"#;
    let result = parse_error_envelope(malformed);
    match result {
        Err(CliError::DecodeFailed { .. }) => {}
        other => panic!(
            "an error envelope missing `message` must yield CliError::DecodeFailed, got: {other:?}"
        ),
    }
}

/// §8 F-4 / F-5：从原始字节经信封分流入口 `render_page_envelope` 走 `Page<RowView>` 一支
/// （`Format::Table`）→ 输出含 >2^53 字符串 id 与回显 `version`。钉"字节→表格全链路保字符串
/// id 与 version"。
#[test]
fn render_page_envelope_table_branch_preserves_string_id_and_version() {
    let body = br#"{
        "items": [{"id": "7300000000000000123", "name": "agent3", "version": 9}],
        "page_no": 1,
        "page_size": 20,
        "total": 1
    }"#;
    let rendered = render_page_envelope(body, Format::Table).expect("page envelope renders");
    assert!(
        rendered.contains(BIG_SNOWFLAKE_ID),
        "table branch must keep the big id as a byte-identical string, got: {rendered}"
    );
    assert!(
        rendered.contains('9'),
        "table branch must echo the carried version 9, got: {rendered}"
    );
}

/// §8 L-3（集合侧）：`Page` 信封 `items` 内某行 id 是 JSON 数字（无引号）→ 经
/// `render_page_envelope` 反序列化失败为 DecodeFailed，不忽略该行、不当成功。钉"返回
/// DecodeFailed 变体"。
#[test]
fn render_page_envelope_numeric_id_in_items_returns_decode_failed() {
    let body = br#"{
        "items": [{"id": 7300000000000000123}],
        "page_no": 1,
        "page_size": 20,
        "total": 1
    }"#;
    let result = render_page_envelope(body, Format::Jsonl);
    match result {
        Err(CliError::DecodeFailed { .. }) => {}
        other => panic!(
            "a numeric id inside Page items must yield CliError::DecodeFailed (no number path), got: {other:?}"
        ),
    }
}

/// §8 F-4：`render_page` 与 `--format jsonl` 是同一已分页 `items` 的两种落点——jsonl 不做
/// 客户端重排。钉"jsonl 行序 = items 原序（第一行的 id 即 items[0] 的 id）"。
#[test]
fn render_jsonl_preserves_backend_item_order_no_resort() {
    let mut page = page_one_row(1); // items[0].id = BIG_SNOWFLAKE_ID
    page.items.push(RowView {
        id: Some("7300000000000000888".to_string()),
        principal_id: None,
        resource_id: None,
        credential_id: None,
        name: None,
        version: Some(1),
    });
    page.total = 2;

    let rendered = render_jsonl(&page).expect("jsonl renders");
    let first_line = rendered
        .lines()
        .find(|l| !l.trim().is_empty())
        .expect("first jsonl line");
    let value: serde_json::Value = serde_json::from_str(first_line).expect("first line parses");
    assert_eq!(
        value.get("id").and_then(|v| v.as_str()),
        Some(BIG_SNOWFLAKE_ID),
        "jsonl must preserve backend item order (no client re-sort), first line must be items[0]"
    );
}
