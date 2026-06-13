//! 命令面行为测试（RED）。
//!
//! 被测对象（cli / command unit）：`postern_cli::command` 的 clap 命令树
//! （`tree::{Cli, Command}`）、强类型管理意图（`intent::ManagementIntent`）、意图 → 6.5
//! 请求规格映射（`ManagementIntent::to_request_spec`）、解析层 → 意图层转换
//! （`Command::into_intent`，含 `--cap` 本地字面量校验）。
//!
//! 测试策略（07-postern-cli §3.1/§3.6/§4/§9）：
//!   ① 枚举 clap 命令树 → 断言 §3 全表 22 个命令组全部存在且无遗漏（F-1）；
//!   ② 给定意图 → 断言映射出的请求规格 `(method, path_template, query, body)` 恰为 6.5
//!      对应行（键值精确、顺序不限），任一映射到非 6.5 端点即不过（F-2）；
//!   ③ 失败路径一等公民：缺必填 `--ttl` → clap 本地拒绝 + 用法 + 零请求；`--cap` 非六动词
//!      字面量 → 本地字面量拒绝 + 零请求（L-1）；
//!   ④ `resource discover <code>` 只映射控制面 `POST /v1/resources/{code}/discover`，
//!      CLI 源码无 `postern_surface` 投影 / `Adapter::discover` 直连（L-12 构造签名）。
//!
//! 不需要真实 daemon——映射断言对纯函数 `to_request_spec`，clap 拒绝对 `try_parse_from`，
//! 构造签名对 `include_str!` 的 CLI 源码做文本结构扫描。每条只钉一个行为，断言精确到具体
//! 值 / 变体 / 错误字段，禁弱断言。
//!
//! 雷区（本测试遵守）：不构造任何机密族类型（`ResolvedTarget`/`ResourceCredential`/
//! `PresentedCredential`/`ScrubSet`）；不嵌裸数据库写标记；不写 `ConnOrigin` 字面双冒号；
//! 不引用数据面 `Intent`/`NormalizedRequest`/`Sanitizer`/`postern_surface`/`Adapter::discover`
//! 任何符号——它们只作为 L-12 / 数据面分离构造签名扫描的**被禁 token 文本**出现在断言串里。

use clap::{CommandFactory, Parser};

use postern_cli::command::intent::ManagementIntent;
use postern_cli::command::tree::Cli;
use postern_cli::reqspec::capability::{parse_cap, CapParseError};
use postern_cli::reqspec::Method;

// CLI 命令面源码（构造签名 / 数据面分离扫描；§8 L-1/L-12 构造检查的输入）。
const TREE_SRC: &str = include_str!("../src/command/tree.rs");
const INTENT_SRC: &str = include_str!("../src/command/intent.rs");
const DISPATCH_SRC: &str = include_str!("../src/command/dispatch.rs");
const MAIN_SRC: &str = include_str!("../src/main.rs");

// ════════════════════════════════════════════════════════════════════════════
// F-1 · 命令树覆盖 §3 全表 22 个命令组（缺任一组即不过）
// ════════════════════════════════════════════════════════════════════════════

/// §3 全表 22 个命令组（与 07-postern-cli §3 表 / 6.5 端点一一对应）。clap kebab-case
/// 渲染后的子命令名（顶层 `postern <group>`）。
const TWENTY_TWO_GROUPS: [&str; 22] = [
    "daemon",
    "init",
    "resource",
    "principal",
    "role",
    "credential",
    "grants",
    "elevate",
    "revoke-grant",
    "mode",
    "freeze",
    "constraint",
    "condition",
    "deny-note",
    "settings",
    "approvals",
    "denials",
    "audit",
    "verify",
    "export",
    "import",
    "mcp-stdio",
];

// §8 F-1：枚举 clap 命令树 → §3 全表 22 个命令组全部存在（缺任一组即不过）。直接对 clap
// `CommandFactory` 反射出的子命令名集合做"22 组 ⊆ 命令树"断言——这是命令树完整性的权威检查。
#[test]
fn command_tree_has_all_twenty_two_groups() {
    let cmd = Cli::command();
    let present: Vec<String> = cmd
        .get_subcommands()
        .map(|sub| sub.get_name().to_string())
        .collect();

    for group in TWENTY_TWO_GROUPS {
        assert!(
            present.iter().any(|name| name == group),
            "F-1：命令树缺命令组 `{group}`——§3 全表 22 组必须全在；实际命令树 = {present:?}"
        );
    }
}

// §8 F-1：命令树**恰好** 22 个顶层命令组——既不少（缺组）也不多（不偷加 6.5 之外的私有
// 命令组）。多一组意味着引入了 §3 表 / 6.5 未列的命令面，违反"命令 ⊆ 6.5"承诺。
#[test]
fn command_tree_has_exactly_twenty_two_groups_no_extras() {
    let cmd = Cli::command();
    let count = cmd.get_subcommands().count();
    assert_eq!(
        count, 22,
        "F-1：命令树必须恰 22 个命令组（§3 全表）；多一组 = 引入 6.5 未列命令面"
    );
}

// §8 F-1：docs/examples 02/03/06/07 §3 出现的命令逐条能在命令树找到顶层入口（场景规格
// 中的 elevate/revoke-grant/mode/freeze/credential/audit/denials/verify/settings/init/
// resource/export 等都有对应命令组）。这里钉"场景规格里出现过的命令组都在树里"。
#[test]
fn example_referenced_commands_have_entry_points() {
    let cmd = Cli::command();
    let present: Vec<String> = cmd
        .get_subcommands()
        .map(|sub| sub.get_name().to_string())
        .collect();

    // docs/examples 02/03/06/07 §3 实际出现过的命令组（grep 自示例提取的去重集）。
    let referenced = [
        "init",
        "resource",
        "credential",
        "export",
        "import",
        "role",
        "grants",
        "elevate",
        "revoke-grant",
        "mode",
        "freeze",
        "constraint",
        "denials",
        "audit",
        "verify",
        "settings",
    ];
    for name in referenced {
        assert!(
            present.iter().any(|p| p == name),
            "F-1：场景规格 02/03/06/07 出现的命令 `{name}` 在命令树无对应入口"
        );
    }
}

// ════════════════════════════════════════════════════════════════════════════
// F-2 · 意图 → 6.5 端点映射（命令 ⊆ 6.5；键值精确、顺序不限）
// ════════════════════════════════════════════════════════════════════════════

// §8 F-2：`postern elevate agent2 --cap redis-main:destroy --ttl 30m` → 映射恰为
// `POST /v1/grants/temp`，体含 `principal`/`capability`/`ttl`（对 docs/examples/06 §3.1）。
// 钉方法 = POST、路径 = /v1/grants/temp、体三字段精确。
#[test]
fn elevate_intent_maps_to_post_grants_temp_with_body() {
    let intent = ManagementIntent::Elevate {
        principal: "agent2".to_string(),
        resource: "redis-main".to_string(),
        verb: "destroy".to_string(),
        ttl: "30m".to_string(),
    };

    let spec = intent
        .to_request_spec()
        .expect("elevate intent must map to a 6.5 RequestSpec");

    assert_eq!(spec.method, Method::Post, "F-2：elevate 方法必须是 POST");
    assert_eq!(
        spec.path_template, "/v1/grants/temp",
        "F-2：elevate 必须映射到 6.5 端点 /v1/grants/temp，不得映射到任何非 6.5 端点"
    );

    let body = spec.body.expect("F-2：elevate 是写端点，必须有请求体");
    assert_eq!(
        body.fields.get("principal").map(String::as_str),
        Some("agent2"),
        "F-2：elevate 体 principal 字段恰为目标 Principal"
    );
    assert_eq!(
        body.fields.get("capability").map(String::as_str),
        Some("destroy"),
        "F-2：elevate 体 capability 字段恰为动词字面量"
    );
    assert_eq!(
        body.fields.get("ttl").map(String::as_str),
        Some("30m"),
        "F-2：elevate 体 ttl 字段恰为时长字面量"
    );
}

// §8 F-2：`postern audit --principal agent3 --page-no 1 --page-size 20` → 方法 = GET、
// 路径 = /v1/audit、查询参数集**恰**含 {principal=agent3, page_no=1, page_size=20}（键值
// 精确、顺序不限；对 docs/examples/07 §4.1-A）。多 / 少 / 错任一键即不过。
#[test]
fn audit_intent_maps_to_get_v1_audit_with_exact_query_set() {
    let intent = ManagementIntent::Audit {
        principal: Some("agent3".to_string()),
        since: None,
        page_no: Some(1),
        page_size: Some(20),
    };

    let spec = intent
        .to_request_spec()
        .expect("audit intent must map to a 6.5 RequestSpec");

    assert_eq!(spec.method, Method::Get, "F-2：audit 方法必须是 GET");
    assert_eq!(
        spec.path_template, "/v1/audit",
        "F-2：audit 必须映射到 6.5 端点 /v1/audit"
    );
    assert!(spec.body.is_none(), "F-2：audit 是读端点，无请求体");

    let pairs = spec.query.into_pairs();
    assert_eq!(
        pairs.get("principal").map(String::as_str),
        Some("agent3"),
        "F-2：audit 查询键 principal=agent3"
    );
    assert_eq!(
        pairs.get("page_no").map(String::as_str),
        Some("1"),
        "F-2：audit 查询键 page_no=1"
    );
    assert_eq!(
        pairs.get("page_size").map(String::as_str),
        Some("20"),
        "F-2：audit 查询键 page_size=20"
    );
    assert_eq!(
        pairs.len(),
        3,
        "F-2：audit 查询参数集恰 3 键 {{principal,page_no,page_size}}——未给 since 不得凭空带键"
    );
}

// §8 F-2：`postern audit` 不给过滤 / 分页 → 查询参数集为空（缺则不带键，由后端取默认；
// 对齐 F-6"缺省由后端取默认 20"）。CLI 不替 daemon 凑任何默认键。
#[test]
fn audit_intent_without_filters_yields_empty_query() {
    let intent = ManagementIntent::Audit {
        principal: None,
        since: None,
        page_no: None,
        page_size: None,
    };

    let spec = intent
        .to_request_spec()
        .expect("bare audit intent must map to a 6.5 RequestSpec");

    let pairs = spec.query.into_pairs();
    assert!(
        pairs.is_empty(),
        "F-2/F-6：无过滤无分页的 audit 查询参数集必须为空（缺则不带键），实际 = {pairs:?}"
    );
}

// §8 F-2：`revoke-grant <id>` → `DELETE /v1/grants/temp/{id}`，且写体期望 `version` 原样
// 取自调用方供入（F-7：只透传不自造）。供入 7 → 体 version 7（不增不减）。
#[test]
fn revoke_grant_intent_maps_to_delete_with_verbatim_version() {
    let intent = ManagementIntent::RevokeGrant {
        id: "9001".to_string(),
        version: Some(7),
    };

    let spec = intent
        .to_request_spec()
        .expect("revoke-grant intent must map to a 6.5 RequestSpec");

    assert_eq!(
        spec.method,
        Method::Delete,
        "F-2：revoke-grant 方法必须是 DELETE"
    );
    assert_eq!(
        spec.path_template, "/v1/grants/temp/9001",
        "F-2：revoke-grant 路径参数 {{id}} 必须由意图字段填充为 /v1/grants/temp/9001"
    );
    let body = spec.body.expect("revoke-grant 是乐观锁删除写，必须有体");
    assert_eq!(
        body.version,
        Some(7),
        "F-7：写体期望 version 必须原样取自先前读取值（7），CLI 不自增 / 自造"
    );
}

// §8 F-2：`freeze` 是 `mode set freeze` 全局别名 → 同映射 `PUT /v1/mode`（§3 表）。别名
// 不得映射到任何独立 / 私有端点；freeze 与 mode 落同一 6.5 端点。
#[test]
fn freeze_intent_maps_to_put_v1_mode_alias() {
    let intent = ManagementIntent::Freeze;

    let spec = intent
        .to_request_spec()
        .expect("freeze intent must map to a 6.5 RequestSpec");

    assert_eq!(
        spec.method,
        Method::Put,
        "F-2：freeze 方法必须是 PUT（全局冻结）"
    );
    assert_eq!(
        spec.path_template, "/v1/mode",
        "F-2：freeze 是 `mode set freeze` 全局别名，必须同映射 6.5 端点 PUT /v1/mode"
    );
}

// §8 F-2：`mode set freeze`（显式）也映射 `PUT /v1/mode`——与 `freeze` 别名同端点，证两条
// 命令路径汇于同一 6.5 端点行（命令 ⊆ 6.5，无分叉私有端点）。
#[test]
fn mode_set_intent_maps_to_put_v1_mode() {
    let intent = ManagementIntent::ModeSet {
        mode: "freeze".to_string(),
        resource: None,
        ttl: None,
    };

    let spec = intent
        .to_request_spec()
        .expect("mode set intent must map to a 6.5 RequestSpec");

    assert_eq!(spec.method, Method::Put, "F-2：mode set 方法必须是 PUT");
    assert_eq!(
        spec.path_template, "/v1/mode",
        "F-2：mode set 必须映射到 6.5 端点 PUT /v1/mode"
    );
}

// ════════════════════════════════════════════════════════════════════════════
// L-12 · 接入侧 discover 只触发控制面端点、不借数据面术语（CONS-20）
// ════════════════════════════════════════════════════════════════════════════

// §8 L-12：`resource discover <code>` → 映射**恰为**控制面
// `POST /v1/resources/{code}/discover`（`{code}` 由意图字段填充）。不得映射到任何数据面
// MCP / surface 端点。
#[test]
fn resource_discover_intent_maps_to_control_plane_endpoint_only() {
    let intent = ManagementIntent::ResourceDiscover {
        code: "redis-main".to_string(),
    };

    let spec = intent
        .to_request_spec()
        .expect("resource discover intent must map to a 6.5 RequestSpec");

    assert_eq!(
        spec.method,
        Method::Post,
        "L-12：discover 方法必须是 POST（控制面触发）"
    );
    assert_eq!(
        spec.path_template, "/v1/resources/redis-main/discover",
        "L-12：resource discover 必须映射控制面 POST /v1/resources/{{code}}/discover，绝非数据面 surface"
    );
    // 触发即一次往返——端点必须在控制面 /v1/ 命名空间内，不得是数据面 /mcp 端点。
    assert!(
        spec.path_template.starts_with("/v1/"),
        "L-12：discover 端点必须在控制面 /v1/ 命名空间内，不得是数据面 /mcp 端点"
    );
}

// §8 L-12（构造签名）：CLI 命令面源码内**无** `postern_surface` 投影逻辑、**无**
// `Adapter::discover` 直连——CLI 不在数据面、不实现能力发现语义（`mcp-stdio` 外无任何数据面
// 端点路径）。对 tree/intent/dispatch/main 源码逐文件文本扫描这些被禁 token。
#[test]
fn cli_command_source_has_no_data_plane_capability_discovery() {
    // 被禁数据面 token：能力面投影、适配器直连探测——CLI 引用任一即越过数据面分离红线。
    let forbidden = [
        "postern_surface",   // 数据面授权快照投影（CONS-20，CLI 不触达）
        "Adapter::discover", // 适配器直连资源探测（探测在 daemon 侧，CLI 只触发控制面端点）
        "CapabilitySurface", // 能力面投影类型（数据面，CLI 不构造 / 不持有）
    ];
    for (name, src) in [
        ("command/tree.rs", TREE_SRC),
        ("command/intent.rs", INTENT_SRC),
        ("command/dispatch.rs", DISPATCH_SRC),
        ("main.rs", MAIN_SRC),
    ] {
        let code = code_only(src);
        for token in forbidden {
            assert!(
                !code.contains(token),
                "L-12 构造签名：{name} 代码体出现数据面 token `{token}`——CLI 不实现能力发现 / 不直连资源探测"
            );
        }
    }
}

// §8 L-12 / 数据面分离（构造签名）：CLI 管理意图枚举**绝不**命名为或引用 core 数据面
// 求值类型 `Intent`/`NormalizedRequest`/`Sanitizer`——CLI 不在数据面 [0]~[6] 求值管线内，
// 引用它们即构造签名红线。CLI 的管理意图是独立命名的 `ManagementIntent`。
#[test]
fn cli_command_source_does_not_reference_data_plane_eval_types() {
    // 被禁数据面求值 token（路径 / 类型名形态，避免误伤散文）。
    let forbidden = [
        "NormalizedRequest",         // 归一化求值产物（数据面 [0]，CLI 不构造）
        "Sanitizer",                 // 脱敏器 trait（数据面 [9]，CLI 不实现）
        "request::Intent",           // core 数据面 Intent 的限定路径
        "use postern_core::request", // 数据面 request 模块（CLI 只用 core 共享 DTO，不用求值类型）
    ];
    for (name, src) in [
        ("command/tree.rs", TREE_SRC),
        ("command/intent.rs", INTENT_SRC),
        ("command/dispatch.rs", DISPATCH_SRC),
        ("main.rs", MAIN_SRC),
    ] {
        let code = code_only(src);
        for token in forbidden {
            assert!(
                !code.contains(token),
                "数据面分离构造签名：{name} 代码体出现 `{token}`——CLI 管理意图与数据面求值类型必须解耦"
            );
        }
    }
}

// ════════════════════════════════════════════════════════════════════════════
// L-1 · 语法层本地拒绝、零请求（缺参 / 非法 `--cap` 字面量）
// ════════════════════════════════════════════════════════════════════════════

// §8 L-1：`postern elevate agent2 --cap redis-main:destroy`（缺必填 `--ttl`，对
// docs/examples/06 §4.2-13）→ clap 本地拒绝（非零）+ 用法，**不**产出任何意图、不触网络。
// 直接对 `try_parse_from` 断言 `Err`——clap 缺必填参数即拒绝，是"未发请求即失败"的本地拒绝类。
#[test]
fn elevate_missing_ttl_is_locally_rejected_by_clap() {
    let result = Cli::try_parse_from([
        "postern",
        "elevate",
        "agent2",
        "--cap",
        "redis-main:destroy",
        // 故意缺 `--ttl`：clap 必填参数缺失 → 本地拒绝。
    ]);
    // 直接对 `Result` 模式匹配——`Ok` 即放过缺参（不过）；`Err` 必须是『缺必填参数』语法类，
    // 非任何授权 / 语义判断（避免 `.err().expect()`，遵 B-5 无 expect / unwrap）。
    match result {
        Err(err) => assert_eq!(
            err.kind(),
            clap::error::ErrorKind::MissingRequiredArgument,
            "L-1：缺 --ttl 的拒绝必须是『缺必填参数』类（语法层），非任何授权 / 语义判断"
        ),
        Ok(cli) => panic!(
            "L-1：缺必填 --ttl 必须被 clap 本地拒绝（非零 + 用法），对 control.sock 零请求；实际解析通过 = {:?}",
            cli.command
        ),
    }
}

// §8 L-1：`postern elevate agent2 --cap redis-main:destroy --ttl 30m`（完整）→ clap 解析
// **通过**（语法形态合法）。证 clap 层只判形态、不越权拒绝合法命令——L-1 拒绝只针对语法
// 非法，合法命令必须放行到意图层。
#[test]
fn elevate_with_all_required_args_parses_at_clap_layer() {
    let result = Cli::try_parse_from([
        "postern",
        "elevate",
        "agent2",
        "--cap",
        "redis-main:destroy",
        "--ttl",
        "30m",
    ]);
    assert!(
        result.is_ok(),
        "L-1：语法完整的 elevate 必须通过 clap 解析（clap 只判形态、不下授权判断）"
    );
}

// §8 L-1：`--cap frobnicate`（非六动词字面量，对 docs/examples/03 §4.2-H）→ **本地字面量
// 拒绝**，对 control.sock 零请求。`Command::into_intent` 是字面量校验落点：非法 `--cap` →
// `Err(CliError::LocalReject)`，不产出意图、不映射请求规格、不触网络。
#[test]
fn elevate_bad_cap_literal_is_locally_rejected_before_any_request() {
    use postern_cli::command::tree::Command;
    use postern_cli::error::CliError;

    // 语法形态合法（clap 放行），但 `frobnicate` 缺冒号 / 非六动词——字面量校验须本地拒绝。
    let cmd = Command::Elevate {
        principal: "agent2".to_string(),
        cap: "frobnicate".to_string(),
        ttl: "30m".to_string(),
    };

    let outcome = cmd.into_intent();
    match outcome {
        Err(CliError::LocalReject { .. }) => { /* L-1：本地拒绝、零请求——符合预期 */
        }
        Err(other) => {
            panic!("L-1：非法 --cap 必须是本地语法拒绝 LocalReject，实际错误变体 = {other:?}")
        }
        Ok(intent) => panic!(
            "L-1：非法 --cap `frobnicate` 不得产出意图（那会进而触网络），实际产出 = {intent:?}"
        ),
    }
}

// §8 L-1（语法基元锚）：`--cap` 本地字面量校验基元 `parse_cap` 对 `frobnicate`（缺冒号）
// 返回 `MissingColon`、对 `redis-main:frobnicate`（非六动词）返回 `UnknownVerb`——证"非法
// 字面量"由本地纯函数判定（不经网络、不下授权判断）。这是 into_intent 拒绝所依赖的基元。
#[test]
fn parse_cap_rejects_non_verb_literals_locally() {
    assert_eq!(
        parse_cap("frobnicate"),
        Err(CapParseError::MissingColon),
        "L-1：`frobnicate` 缺冒号 → 本地形态拒绝（MissingColon）"
    );
    match parse_cap("redis-main:frobnicate") {
        Err(CapParseError::UnknownVerb { verb }) => assert_eq!(
            verb, "frobnicate",
            "L-1：非六动词 → UnknownVerb，原文回显供本地用法呈现（纯语法事实）"
        ),
        other => panic!("L-1：非六动词必须是 UnknownVerb 本地拒绝，实际 = {other:?}"),
    }
    // 合法六动词字面量必须放行（本地只判形态，不越权拒绝合法 verb）。
    assert!(
        parse_cap("redis-main:destroy").is_ok(),
        "L-1：合法 `<res:verb>`（六动词）必须本地放行到意图层"
    );
}

// §8 L-1（构造签名）：`into_intent` 解析层 → 意图层转换体内**不**触达传输层——`--cap` 校验
// 在请求规格 / 网络之前，结构上保证"非法 --cap → 零请求"。dispatch 源码内 `into_intent` 这
// 段不出现任何传输 / 往返 token；任何网络往返只在 `dispatch` 主干（意图之后）。
#[test]
fn into_intent_does_not_touch_transport_layer() {
    // 传输 / 网络往返的代码 token——`into_intent`（解析 → 意图，含 --cap 校验）绝不出现其一，
    // 否则意味着本地校验前就可能触网络，破坏 L-1"未发请求即失败"。
    let transport_tokens = [
        "round_trip",   // 一次往返发起（只该在 dispatch 主干）
        "UnixStream",   // UDS 连接
        ".connect(",    // 建连
        "send_request", // 发请求
    ];
    let code = code_only(DISPATCH_SRC);
    // 取 `into_intent` 函数体范围（从签名到 dispatch 自由函数之前）做局部扫描。
    let into_intent_segment = isolate_into_intent_segment(&code);
    for token in transport_tokens {
        assert!(
            !into_intent_segment.contains(token),
            "L-1 构造签名：into_intent 段出现传输 token `{token}`——本地 --cap 校验必须先于任何网络往返（零请求）"
        );
    }
}

// ════════════════════════════════════════════════════════════════════════════
// 源码文本结构扫描工具（剥行尾 `//` 注释，只对真实代码 token 判定，避免误伤声明红线的散文）
// ════════════════════════════════════════════════════════════════════════════

/// 剥掉每行 `//` 之后的注释，只留真实代码 token（与 reqspec/transport 兄弟测试同源约定）。
/// 散文 / 文档注释里合法出现的被禁 token（如声明"不引用 postern_surface"的注释）不参与
/// 结构扫描——只对真实代码判定，避免把"声明红线的注释"误读成"违反红线的代码"。
fn code_only(src: &str) -> String {
    src.lines()
        .map(|line| match line.find("//") {
            Some(idx) => &line[..idx],
            None => line,
        })
        .filter(|code| !code.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// 从 dispatch 已剥注释代码里截出 `into_intent` 函数体区段（到下一个自由函数 `pub async fn
/// dispatch` 之前），供 L-1 构造签名局部扫描。找不到锚点时退化为全文件（保守扫描，不漏判）。
fn isolate_into_intent_segment(code: &str) -> String {
    let start = code.find("fn into_intent").unwrap_or(0);
    let tail = &code[start..];
    let end = tail.find("pub async fn dispatch").unwrap_or(tail.len());
    tail[..end].to_string()
}
