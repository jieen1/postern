//! 意图 → 请求规格映射的行为测试（RED）。
//!
//! 被测对象：`postern_cli::reqspec` 的请求规格底座——`RequestSpec { method, path_template,
//! query, body }`（命令 → HTTP 形态的唯一落点）、`reqspec::query::Query`（分页 / 过滤装配，
//! 缺则不带键）、`reqspec::capability::parse_cap`（`--cap <res:verb>` 本地字面量校验）。
//!
//! 测试策略（07-postern-cli §3.1/§3.4/§3.5/§6.1/§9）：给定命令参数 → 断言产出的请求规格
//! `(method, path_template, query, body)` 恰为预期（键值精确、顺序不限），无需真实 daemon。
//! 每条只钉一个行为，断言精确到具体值 / 变体 / 错误字段；失败路径一等公民（非法字面量 →
//! 该具体拒绝变体）。
//!
//! 覆盖 §8 条目：
//!   - F-6 分页参数透传：`--page-no 2 --page-size 50` → query 含 `page_no=2`&`page_size=50`；
//!     不给分页 → query 不含这两键（daemon 取默认 20）；无"取回全量再切片"路径（构造签名）。
//!   - F-7 乐观锁版本透传：写体期望 `version` 原样取自先前读取值（3 → 体 version 3）；无
//!     自增 / 自造 version 路径（构造签名）。
//!   - `--cap` 字面量校验（喂 L-1 的 `cli` unit）：`redis-main:destroy` 解析通过；
//!     `frobnicate` / 缺冒号 / 非六动词 → 本地字面量拒绝；校验只比对 `Capability::as_str()`
//!     集，不下授权判断。
//!
//! 雷区（本测试遵守）：不构造任何机密族类型（`ResolvedTarget`/`ResourceCredential`/
//! `PresentedCredential`/`ScrubSet`）；不嵌裸数据库写标记；不写 `ConnOrigin` 字面双冒号；
//! `version` 只作为外部供入的不透明数值搬运，测试自身亦不"产生"它。

use postern_core::domain::Capability;

use postern_cli::reqspec::capability::{parse_cap, valid_verbs, CapLiteral, CapParseError};
use postern_cli::reqspec::query::Query;
use postern_cli::reqspec::{
    audit_spec, elevate_spec, revoke_grant_spec, Method, RequestSpec, WriteBody,
};

// ════════════════════════════════════════════════════════════════════════════
// F-6 · 分页参数透传（缺则不带键，由后端取默认 20；无取回全量再切片路径）
// ════════════════════════════════════════════════════════════════════════════

// §8 F-6：`--page-no 2 --page-size 50` → 装配出的查询参数恰含 page_no=2 & page_size=50。
// 钉键名与值文本两者精确——值是十进制串、非空、非默认。
#[test]
fn query_carries_page_no_and_page_size_when_given() {
    let pairs = Query {
        page_no: Some(2),
        page_size: Some(50),
        ..Query::default()
    }
    .into_pairs();

    assert_eq!(
        pairs.get("page_no").map(String::as_str),
        Some("2"),
        "page_no=2 必须出现且值为十进制 \"2\""
    );
    assert_eq!(
        pairs.get("page_size").map(String::as_str),
        Some("50"),
        "page_size=50 必须出现且值为十进制 \"50\""
    );
}

// §8 F-6：不给分页参数 → 查询串不含 page_no/page_size 任一键（daemon 取默认 20）。
// 关键反向断言：None 必须是"整键缺席"，而非 page_no=（空串）或 page_no=0（默认值）。
#[test]
fn query_omits_pagination_keys_entirely_when_absent() {
    let pairs = Query::default().into_pairs();

    assert!(
        !pairs.contains_key("page_no"),
        "缺 --page-no 时不得带 page_no 键（不发空串、不替人填默认），实得键集: {:?}",
        pairs.keys().collect::<Vec<_>>()
    );
    assert!(
        !pairs.contains_key("page_size"),
        "缺 --page-size 时不得带 page_size 键，实得键集: {:?}",
        pairs.keys().collect::<Vec<_>>()
    );
}

// §8 F-6（差分守卫）：None 的省略不是"恰好值为空"的巧合。同时给 page_size、省 page_no
// → 结果只含 page_size，绝不冒出一个 page_no 键（如 page_no=0 / page_no=）。
// 若实现把缺省悄悄补成 0 / 空串，本断言 FAIL。
#[test]
fn query_one_pagination_key_given_does_not_synthesize_the_other() {
    let pairs = Query {
        page_no: None,
        page_size: Some(20),
        ..Query::default()
    }
    .into_pairs();

    assert_eq!(
        pairs.get("page_size").map(String::as_str),
        Some("20"),
        "给定的 page_size 必须落键"
    );
    assert!(
        !pairs.contains_key("page_no"),
        "未给的 page_no 不得被合成（不出现 page_no=0 / 空串），实得键集: {:?}",
        pairs.keys().collect::<Vec<_>>()
    );
}

// §8 F-6（对 docs/examples/07 §4.1-A）：audit --principal agent3 --page-no 1 --page-size 20
// → 方法=GET、路径=/v1/audit、查询集恰含 principal=agent3 / page_no=1 / page_size=20。
// 钉端点形态 + 三键精确（顺序不限，按键取值比对）。
#[test]
fn audit_spec_maps_to_get_v1_audit_with_filter_and_pagination() {
    let spec: RequestSpec = audit_spec(Some("agent3"), None, Some(1), Some(20));

    assert_eq!(spec.method, Method::Get, "audit 是读端点，方法必须 GET");
    assert_eq!(
        spec.path_template, "/v1/audit",
        "audit 必须落 6.5 的 /v1/audit 路径"
    );
    assert!(
        spec.body.is_none(),
        "读端点 audit 不得带请求体（body 必须 None）"
    );

    let pairs = spec.query.into_pairs();
    assert_eq!(pairs.get("principal").map(String::as_str), Some("agent3"));
    assert_eq!(pairs.get("page_no").map(String::as_str), Some("1"));
    assert_eq!(pairs.get("page_size").map(String::as_str), Some("20"));
}

// §8 F-6：audit 不给分页时，请求规格的 query 里无分页键（由后端取默认）。
// 这把"缺则不带键"钉到端点构造层面（而非仅 Query 单元层面）。
#[test]
fn audit_spec_without_pagination_omits_pagination_keys() {
    let spec = audit_spec(Some("agent3"), None, None, None);
    let pairs = spec.query.into_pairs();

    assert_eq!(
        pairs.get("principal").map(String::as_str),
        Some("agent3"),
        "过滤键 principal 仍按命令携带"
    );
    assert!(
        !pairs.contains_key("page_no") && !pairs.contains_key("page_size"),
        "不给分页 → audit query 不含 page_no/page_size，实得键集: {:?}",
        pairs.keys().collect::<Vec<_>>()
    );
}

// ════════════════════════════════════════════════════════════════════════════
// F-7 · 乐观锁版本透传（期望 version 原样取自先前读取；CLI 不自增 / 不自造）
//
// 载体必须是 **update/delete 命令**：§8 F-7 通过判定原文为「读响应 version=3 → 后续
// `update`/`delete` 命令请求体期望 version 恰为先前读取的 3（原样回传）」，§3.5 把期望
// version 系到「后续 update/delete/disable 等写命令」。下列用 `revoke-grant <id>`
// → `DELETE /v1/grants/temp/{id}`（对 temp_grants 既有行的**删除型乐观锁写**，
// docs/examples/06 §3.1 表行 + §4.2-F/步骤 10 携期望 version 的条件写语义——写库时按期望
// version 比对、不匹配即 daemon 返回 409）作版本载体。
//
// 反例守卫：`elevate`（`POST /v1/grants/temp`）是**创建型**插入新行，§3 端点表该行只列
// principal/capability/ttl、不列 version——协议上根本不带 version，故 F-7 绝不钉在该端点上
// （见末尾 `elevate_create_spec_carries_no_version` 锁死创建端点写体 version 恒 None）。
// ════════════════════════════════════════════════════════════════════════════

// §8 F-7：update/delete 写命令体携带的期望 version 恰等于先前读取值——读得 3 → 写体
// version 为 3。version 是测试外部供入的不透明数值（模拟"人从上条读输出取得"），
// CLI 原样落入；载体是真正的乐观锁删除端点 DELETE /v1/grants/temp/{id}。
#[test]
fn delete_body_carries_expected_version_verbatim() {
    let version_from_prior_read: u64 = 3;

    let spec = revoke_grant_spec("7300000000000000123", Some(version_from_prior_read));

    assert_eq!(
        spec.method,
        Method::Delete,
        "revoke-grant 是删除型乐观锁写，方法必须 DELETE（version 只系于 update/delete 写）"
    );
    let body: &WriteBody = spec
        .body
        .as_ref()
        .expect("乐观锁删除是写端点，必须带请求体携期望 version");
    assert_eq!(
        body.version,
        Some(3),
        "写体期望 version 必须原样等于先前读取的 3（不自增 / 不重写）"
    );
}

// §8 F-7（差分守卫）：换一个先前读取值（7）→ 写体 version 必为 7，绝不被改写成
// 7+1 / 8 等"自增后"值。若实现对供入版本做任何算术，本断言 FAIL。
#[test]
fn delete_body_does_not_increment_supplied_version() {
    let spec = revoke_grant_spec("7300000000000000123", Some(7));
    let body = spec.body.as_ref().expect("写端点带体");

    assert_eq!(
        body.version,
        Some(7),
        "供入 7 必须原样为 7——出现 8（自增）即违反 F-7"
    );
    assert_ne!(body.version, Some(8), "version 绝不被 CLI 递增（+1）");
}

// §8 F-7：不给期望版本（如非乐观锁写）→ 写体 version 为 None，CLI 不替人造一个。
// 钉"缺省即不带 version 键"，杜绝 CLI 自造默认版本（如 0）。
#[test]
fn delete_body_omits_version_when_none_supplied() {
    let spec = revoke_grant_spec("7300000000000000123", None);
    let body = spec.body.as_ref().expect("写端点带体");

    assert_eq!(
        body.version, None,
        "未供期望 version → 写体 version 必须 None（CLI 不自造 0 或其它默认）"
    );
}

// §8 F-7（端点映射 + 路径参数填充）：revoke-grant <id> → DELETE /v1/grants/temp/{id}，
// {id} 由意图字段原样填充（雪花 id 恒为字符串、>2^53 不丢精度）；删除端点无命令载荷字段。
#[test]
fn revoke_grant_spec_maps_to_delete_grants_temp_by_id() {
    let spec = revoke_grant_spec("7300000000000000123", Some(3));

    assert_eq!(spec.method, Method::Delete, "revoke-grant 方法必须 DELETE");
    assert_eq!(
        spec.path_template, "/v1/grants/temp/7300000000000000123",
        "{{id}} 必须原样填入路径（雪花 id 字符串，>2^53 不丢精度）"
    );
    let body = spec.body.as_ref().expect("乐观锁删除带体");
    assert!(
        body.fields.is_empty(),
        "删除端点无命令载荷字段（version 是其全部乐观锁前置），实得字段: {:?}",
        body.fields.keys().collect::<Vec<_>>()
    );
}

// §8 F-7（反例守卫 / 创建端点 version 红线）：elevate 是**创建型**插入（POST /v1/grants/temp），
// §3 端点表该行只列 principal/capability/ttl、协议上不带 version。故其写体 version 必恒为 None
// ——CLI 绝不在创建端点凭空塞入期望版本（期望 version 只系于 update/delete 写，§3.5/F-7）。
// 若实现把 version 字段塞进创建端点 body，本断言 FAIL。
#[test]
fn elevate_create_spec_carries_no_version() {
    let spec = elevate_spec("agent2", "redis-main", "destroy", "30m");
    let body = spec.body.as_ref().expect("写端点带体");

    assert_eq!(
        body.version, None,
        "创建端点 POST /v1/grants/temp 协议不带 version，写体 version 必恒 None（不在创建侧自造期望版本）"
    );
}

// §8 F-2/F-7（对 docs/examples/06 §3.1）：elevate agent2 --cap redis-main:destroy --ttl 30m
// → POST /v1/grants/temp，体含 principal / capability / ttl 三字段且值精确，且体无 version 键。
// 钉端点映射 + 体字段值（capability 取 verb 段 destroy，principal/ttl 原样）。
#[test]
fn elevate_spec_maps_to_post_grants_temp_with_body_fields() {
    let spec = elevate_spec("agent2", "redis-main", "destroy", "30m");

    assert_eq!(spec.method, Method::Post, "elevate 是写端点，方法 POST");
    assert_eq!(
        spec.path_template, "/v1/grants/temp",
        "elevate 必须落 6.5 的 /v1/grants/temp"
    );

    let body = spec.body.as_ref().expect("写端点带体");
    assert_eq!(
        body.fields.get("principal").map(String::as_str),
        Some("agent2"),
        "体含 principal=agent2"
    );
    assert_eq!(
        body.fields.get("capability").map(String::as_str),
        Some("destroy"),
        "体含 capability=destroy（取自 --cap 的 verb 段）"
    );
    assert_eq!(
        body.fields.get("ttl").map(String::as_str),
        Some("30m"),
        "体含 ttl=30m（原样）"
    );
    assert_eq!(
        body.version, None,
        "创建端点写体不含 version 键（version 只系于 update/delete 写，§3.5/F-7）"
    );
}

// ════════════════════════════════════════════════════════════════════════════
// --cap <res:verb> 本地字面量校验（喂 L-1；仅比对 Capability::as_str() 集，不授权）
// ════════════════════════════════════════════════════════════════════════════

// §8 --cap 字面量校验：redis-main:destroy 解析通过——destroy ∈ 六动词字面量集。
// 钉拆段结果：resource=redis-main、verb=destroy（原样字符串，无授权语义附加）。
#[test]
fn parse_cap_accepts_legal_resource_and_six_verb() {
    let parsed: CapLiteral = parse_cap("redis-main:destroy").expect("destroy 是合法动词字面量");

    assert_eq!(parsed.resource, "redis-main", "资源段原样拆出");
    assert_eq!(parsed.verb, "destroy", "动词段原样拆出（∈ 六动词集）");
}

// §8 --cap 字面量校验（对 docs/examples/03 §4.2-H）：frobnicate（非动词、且缺冒号）
// → 本地拒绝。这是"未发请求即失败"的本地语法类（L-1）。断言到具体错误变体 MissingColon。
#[test]
fn parse_cap_rejects_missing_colon() {
    let err: CapParseError = parse_cap("frobnicate").expect_err("frobnicate 无冒号，非法形态");
    assert_eq!(
        err,
        CapParseError::MissingColon,
        "缺冒号必须是 MissingColon（纯语法事实，非授权判断）"
    );
}

// §8 --cap 字面量校验：合法 <res:verb> 形态但 verb 非六动词（如 frobnicate）
// → UnknownVerb，且错误携被拒 verb 原文。钉"只比对 as_str() 集"——非集内即拒。
#[test]
fn parse_cap_rejects_non_six_verb() {
    let err = parse_cap("redis-main:frobnicate").expect_err("frobnicate 不在六动词集");
    assert_eq!(
        err,
        CapParseError::UnknownVerb {
            verb: "frobnicate".to_string()
        },
        "非六动词必须是 UnknownVerb 并回带 verb 原文"
    );
}

// §8 --cap 字面量校验：空资源段（:destroy）→ EmptyResource。
// <res:verb> 形态要求资源段非空——形态非法即本地拒绝。
#[test]
fn parse_cap_rejects_empty_resource() {
    let err = parse_cap(":destroy").expect_err("资源段为空，形态非法");
    assert_eq!(
        err,
        CapParseError::EmptyResource,
        "空资源段必须是 EmptyResource"
    );
}

// §8 --cap 字面量校验（覆盖完整六动词集）：observe/query/mutate/execute/manage/destroy
// 逐一解析通过，verb 段原样回带。钉"接受集恰为 Capability::as_str() 的六个值"——
// 既不漏（六个都过）、也不多（下一条钉一个集外串被拒）。
#[test]
fn parse_cap_accepts_each_of_the_six_verbs() {
    for verb in valid_verbs() {
        let raw = format!("svc-x:{verb}");
        let parsed = parse_cap(&raw).unwrap_or_else(|e| panic!("{verb} 应通过，却得 {e:?}"));
        assert_eq!(parsed.verb, verb, "verb 段原样回带 {verb}");
    }
}

// §8 --cap 字面量校验（红线对照）：valid_verbs() 与 Capability::as_str() 集逐项一致——
// 锁死"合法集唯一来源是 core 枚举"。若校验集与权威枚举漂移（漏一个 / 多硬编码一个），
// 本断言 FAIL（构造签名级守卫，§6.1 雷区）。
#[test]
fn valid_verbs_mirror_capability_as_str_set() {
    let expected = [
        Capability::Observe.as_str(),
        Capability::Query.as_str(),
        Capability::Mutate.as_str(),
        Capability::Execute.as_str(),
        Capability::Manage.as_str(),
        Capability::Destroy.as_str(),
    ];
    assert_eq!(
        valid_verbs(),
        expected,
        "合法动词集必须逐项等于 Capability::as_str() 的六个值，不得漂移"
    );
}

// §8 --cap 字面量校验（红线）：校验只判形态，不下授权判断——destroy 这一最危动词的
// 字面量同样接受（是否真被授权由 daemon 裁决）。若 CLI 在此对 destroy 做任何
// "拒绝高危动词"的本地判断，即越界成客户端安全逻辑，本断言 FAIL。
#[test]
fn parse_cap_does_not_make_authorization_decision_on_destroy() {
    let parsed = parse_cap("redis-main:destroy").expect("字面量合法即接受——授权与否不在 CLI 判定");
    assert_eq!(parsed.verb, "destroy");
}

// ════════════════════════════════════════════════════════════════════════════
// 构造签名级守卫（source-scan）：扫描 reqspec 各源文件的**代码**（剥注释行 + 行内尾注），
// 钉两条 §8 结构子项——F-6「CLI 源码内无『取回全量再切片』代码路径」、F-7「CLI 源码无
// version 自增/自造路径」。这两项是独立的构造签名检查（§8 通过判定指明判定方式可为结构
// 检查，非必单元值断言），值路径断言无法覆盖：值断言只证某一输入下某一 helper 的行为，
// 排除不了别处（如真实命令 dispatch 路径）出现本地切片 / version 派生。故以源码结构钉死。
// ════════════════════════════════════════════════════════════════════════════

// reqspec 三源文件全文（编译期内嵌；相对本测试文件 tests/reqspec.rs 的路径）。结构检查覆盖
// 请求规格底座（mod.rs）、查询装配（query.rs）、字面量校验（capability.rs）整个 reqspec 面。
const REQSPEC_MOD_SRC: &str = include_str!("../src/reqspec/mod.rs");
const REQSPEC_QUERY_SRC: &str = include_str!("../src/reqspec/query.rs");
const REQSPEC_CAPABILITY_SRC: &str = include_str!("../src/reqspec/capability.rs");

// 取一段 Rust 源码的**纯代码体**：丢弃整行注释（trim 后以 `//` 起头者）与每行的行内尾注
// （`//` 之后的内容）。散文/文档注释里合法出现的 "切片"/"自增"/"version" 等词不参与结构扫描，
// 只对真实代码 token 判定——避免把"声明红线的注释"误读成"违反红线的代码"。
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

// §8 F-6（构造签名检查）：reqspec 代码体内**不存在**"取回全量再本地切片"的代码路径——
// 即无任何切片 / 截断 / 跳取 token（本地分页只会经这些 idiom 实现）。分页职责整体在后端，
// 客户端只透传 page_no/page_size 游标。若日后有人在 CLI 引入本地切片，本断言 FAIL
// （兜底契约 DB_PAGINATION_MANDATORY 只约束 store 集合查询，对 cli 前端切片零覆盖，故此处补钉）。
#[test]
fn no_fetch_all_then_slice_code_path_in_reqspec() {
    // 本地切片 / 截断 / 跳取的代码 idiom 集（取回全量再切片必经其一）。
    let slice_idioms = [
        ".skip(",     // 跳过前 N 条（本地翻页）
        ".take(",     // 取前 N 条（本地截断为一页）
        ".chunks(",   // 按页大小切块
        ".windows(",  // 滑窗切片
        ".truncate(", // 截断到一页
        ".drain(",    // 抽取区间
        "[..",        // 区间切片（前缀）
        "..]",        // 区间切片（后缀）
    ];

    for (name, src) in [
        ("reqspec/mod.rs", REQSPEC_MOD_SRC),
        ("reqspec/query.rs", REQSPEC_QUERY_SRC),
        ("reqspec/capability.rs", REQSPEC_CAPABILITY_SRC),
    ] {
        let code = code_only(src);
        for idiom in slice_idioms {
            assert!(
                !code.contains(idiom),
                "F-6 构造签名：{name} 代码体出现本地切片 idiom `{idiom}`——CLI 不得『取回全量再切片』，分页整体交后端"
            );
        }
    }
}

// §8 F-7（构造签名检查）：reqspec 代码体内**不存在** version 自增 / 自造路径——`version`
// 在代码中只作为外部供入的不透明 `Option<u64>` 字段被搬运（赋值 / move），绝无任何对它的
// 算术（递增）、生成（next_version/bump/默认 0）或反序列化构造。值路径断言（如供入 7→体 7）
// 排除不了别处出现 bump/next_version；故以源码结构钉死"CLI 永不产生 version"。
#[test]
fn no_version_increment_or_generation_path_in_reqspec() {
    // version 自增 / 自造的代码 idiom 集（任一出现即"产生 version"，违反 F-7 透传红线）。
    // reqspec 干净基线里这些 token 全不出现：`version` 唯一的代码动作是作为 `Option<u64>`
    // 字段被原样 move/赋值。故任一算术 / 变换 / 生成 token 出现即视作引入了 version 派生路径。
    let generation_idioms = [
        "next_version",   // 生成下一版本
        "bump",           // 递增/跃迁版本
        "wrapping_add",   // 环绕自增
        "checked_add",    // 受检自增
        "saturating_add", // 饱和自增
        "version +",      // 对 version 直接加法
        "+ version",      // version 参与加法
        "+ 1",            // 任意自增（如 `v + 1` 对 version 变换；基线无任何 +1）
        "+= 1",           // 复合自增
        "- 1",            // 任意自减
        ".map(",          // 对 Option<version> 做变换映射（基线只 move、绝不 map 变换）
    ];

    for (name, src) in [
        ("reqspec/mod.rs", REQSPEC_MOD_SRC),
        ("reqspec/query.rs", REQSPEC_QUERY_SRC),
        ("reqspec/capability.rs", REQSPEC_CAPABILITY_SRC),
    ] {
        let code = code_only(src);
        for idiom in generation_idioms {
            assert!(
                !code.contains(idiom),
                "F-7 构造签名：{name} 代码体出现 version 派生 idiom `{idiom}`——CLI 只透传不自造，期望 version 唯一来源是先前读取值"
            );
        }
    }

    // 正向锚：reqspec 写体类型字段确为搬运型 `version: Option<u64>`（外部供入、原样落入），
    // 而非任何自造载体——锁死"version 是被搬运的不透明数值"这一构造事实。
    assert!(
        code_only(REQSPEC_MOD_SRC).contains("version: Option<u64>"),
        "F-7 构造签名：写体 version 必须是外部供入的 Option<u64> 搬运字段（透传载体），不得改成自造类型"
    );
}
