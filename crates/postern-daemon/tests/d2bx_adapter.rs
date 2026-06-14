//! D2b-ext 写接缝 — [`StorePolicyRepoAdapter`] 全实体接通行为测试（RED→GREEN）。
//!
//! 钉死适配器把控制面缝 [`PolicyRepo`](postern_daemon::control::PolicyRepo) 接到 store 全部
//! `*_and_rebuild` 写接缝 + `list_*` 读模型 + 审计读句柄（§8 L-14）：
//! - **每实体 commit_write → rev 前进**：constraints / conditions / deny-notes / mode / grants /
//!   settings 各经 store 三联动（实体写 + bump_policy_rev + COMMIT + 重建发布快照），`policy_rev`
//!   严格前进。
//! - **list 投影 id-string**：每实体 `list(entity, page)` 经 store `list_*` 取读模型行，投影为
//!   `serde_json::Value`，id / 资源 / 主体一律雪花**字符串**（不丢精度）。
//! - **mode upsert**：同辖区二次写改既有行（version 自增），不新增。
//! - **grants elevate/revoke**：elevate 新增（version=0）、revoke 乐观锁置终态（version 自增）。
//! - **settings upsert**：同 key 二次写改既有行（version 自增）。
//! - **audit / denials list 返投影**：经注入的审计读句柄回投影行（不构造 `ConnOrigin`、不泄露真实地址）。
//! - **VersionConflict 全或无**：乐观锁期望版本不符 ⇒ [`WriteError::VersionConflict`]，整体 ROLLBACK
//!   （rev 不进、快照不换）。
//! - **不泄露**：资源 / deny / grant 投影只出雪花 id（绝非真实地址）；audit 投影 origin 为脱敏文本
//!   （绝不构造 `ConnOrigin`、绝不回显 TCP 真实地址）。
//!
//! 雷区纪律：本文件**零 SQL 标记**（建库 / 迁移 / 写 / 读全经 store 公共 API）；不构造 `ConnOrigin`
//! / 机密类型；`anyhow` 禁用。argon2 不在本路径（store 写 / 快照 / JSONL 审计无 KDF），故可直接
//! `cargo test -p postern-daemon --test d2bx_adapter` 跑，无需 systemd-run 内存包裹。

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use std::sync::Arc;

use postern_core::domain::PolicySnapshot;
use postern_core::id::{Clock, IdGen};
use postern_core::page::{Page, PageQuery};

use postern_daemon::control::audit_read::JsonlAuditReader;
use postern_daemon::control::repo::StorePolicyRepoAdapter;
use postern_daemon::control::{Actor, AuditRead, PolicyRepo, WriteError, WriteIntent};
use postern_daemon::error::DaemonError;

use postern_store::audit::sink::{FsyncPolicy, JsonlAuditSink};
use postern_store::base::db::Db;
use postern_store::base::write::Actor as StoreActor;
use postern_store::migrate;
use postern_store::policy::PolicyRepo as StoreRepo;
use postern_store::snapshot::SnapshotView;

// ════════════════════════════════════════════════════════════════════════════
//  固定测试材料
// ════════════════════════════════════════════════════════════════════════════

/// 固定时钟（确定 now，时间列得固定宽度文本）。
struct FixedClock(u64);
impl Clock for FixedClock {
    fn now_unix_ms(&self) -> u64 {
        self.0
    }
}

/// 雪花纪元偏移墙钟。
const EPOCH_UNIX_MS: u64 = 1_767_225_600_000;

/// 已迁移到当前版本的内存库。
fn migrated_db() -> Db {
    let db = Db::open_in_memory().expect("in-memory db opens");
    migrate::migrate(&db).expect("migrate builds full schema on empty db");
    db
}

/// 装配持视图的 store 写句柄（首份快照 policy_rev=0）。
fn store_repo_with_view() -> Arc<StoreRepo> {
    let db = migrated_db();
    let view = Arc::new(SnapshotView::new(Arc::new(PolicySnapshot::default())));
    let repo = StoreRepo::with_view(
        db,
        IdGen::new(FixedClock(EPOCH_UNIX_MS)),
        Box::new(FixedClock(EPOCH_UNIX_MS)),
        view,
    );
    Arc::new(repo)
}

/// 审计读 Fake（policy.db 实体测试不触审计载体）——两读支恒回空页。
struct NoAuditRead;
impl AuditRead for NoAuditRead {
    fn scan_audit(&self, page: PageQuery) -> Result<Page<serde_json::Value>, DaemonError> {
        Ok(empty_page(page))
    }
    fn denials_summary(&self, page: PageQuery) -> Result<Page<serde_json::Value>, DaemonError> {
        Ok(empty_page(page))
    }
}

/// 空分页信封（回显 clamp 后分页参数）。
fn empty_page(page: PageQuery) -> Page<serde_json::Value> {
    let page = page.clamp();
    Page {
        items: Vec::new(),
        page_no: page.page_no,
        page_size: page.page_size,
        total: 0,
    }
}

/// 控制面操作者。
fn operator() -> Actor {
    Actor::Operator("tester".to_string())
}

/// store 写操作者。
fn store_actor() -> StoreActor {
    StoreActor::Operator("tester".into())
}

/// 装配适配器（policy.db 句柄 + 审计读 Fake）。
fn adapter_with_fake_audit(store: Arc<StoreRepo>) -> StorePolicyRepoAdapter {
    StorePolicyRepoAdapter::new(store, Arc::new(NoAuditRead))
}

/// 默认整页查询。
fn full_page() -> PageQuery {
    PageQuery {
        page_no: 1,
        page_size: 200,
    }
}

/// 取首项的字符串字段（缺 / 非字符串 ⇒ panic，钉 id-string 纪律）。
fn first_str<'a>(page: &'a Page<serde_json::Value>, key: &str) -> &'a str {
    page.items[0]
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or_else(|| panic!("字段 {key} 存在且为字符串"))
}

/// 断言一个字段是十进制雪花字符串（非空、全数字）。
fn assert_snowflake_string(s: &str, ctx: &str) {
    assert!(
        !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()),
        "{ctx}：应为十进制雪花字符串（不丢精度），得 {s:?}"
    );
}

/// 经 store 直接 seed 一个资源，返回其雪花 id（grants/constraints 需资源 id）。
fn seed_resource(store: &StoreRepo, code: &str) -> postern_core::id::SnowflakeId {
    store
        .create_resource(&store_actor(), code, "pg", "tcp")
        .expect("seed resource")
}

/// 经 store 直接 seed 一个主体，返回其雪花 id（grants 需主体 id）。
fn seed_principal(store: &StoreRepo, name: &str) -> postern_core::id::SnowflakeId {
    store
        .create_principal(&store_actor(), name, "agent")
        .expect("seed principal")
}

// ════════════════════════════════════════════════════════════════════════════
//  constraints：commit_write → rev 前进 + list 投影 id-string
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn constraints_commit_advances_rev_and_lists_id_string() {
    let store = store_repo_with_view();
    let res = seed_resource(&store, "db-main");
    let adapter = adapter_with_fake_audit(Arc::clone(&store));
    let rev_before = adapter.policy_rev().expect("rev before");

    let intent = WriteIntent {
        entity: "constraints",
        fields: serde_json::json!({
            "resource_id": res.as_raw().to_string(),
            "capability": "mutate",
            "kind": "row_limit",
            "spec": "{\"max\":10}",
        }),
        expected_version: None,
    };
    let outcome = adapter
        .commit_write(&operator(), &intent)
        .expect("create constraint commits");
    assert_eq!(outcome.version, 0, "新增 constraint 新行 version = 0");
    assert!(
        outcome.policy_rev > rev_before,
        "constraint 写经三联动 ⇒ policy_rev 严格前进"
    );

    let page = adapter.list("constraints", full_page()).expect("list");
    assert_eq!(page.items.len(), 1, "列读回刚落的一项");
    assert_snowflake_string(first_str(&page, "id"), "constraint id");
    assert_snowflake_string(first_str(&page, "resource"), "constraint resource(雪花 id)");
    // 不泄露：resource 投影是雪花 id 字符串，绝非真实地址 / codename。
    assert_eq!(
        first_str(&page, "resource"),
        res.as_raw().to_string(),
        "constraint.resource 为受约束资源雪花 id（恒 id、绝非真实地址）"
    );
}

#[test]
fn constraints_delete_optimistic_lock_advances_version() {
    let store = store_repo_with_view();
    let res = seed_resource(&store, "db-main");
    let adapter = adapter_with_fake_audit(Arc::clone(&store));
    let create = WriteIntent {
        entity: "constraints",
        fields: serde_json::json!({
            "resource_id": res.as_raw().to_string(), "capability": "mutate", "kind": "k",
        }),
        expected_version: None,
    };
    adapter.commit_write(&operator(), &create).expect("create");
    let page = adapter.list("constraints", full_page()).expect("list");
    let id = first_str(&page, "id").to_string();

    // 逻辑删除（乐观锁期望版本 0）⇒ version 前进到 1。
    let del = WriteIntent {
        entity: "constraints",
        fields: serde_json::json!({ "id": id }),
        expected_version: Some(0),
    };
    let outcome = adapter
        .commit_write(&operator(), &del)
        .expect("delete commits");
    assert_eq!(outcome.version, 1, "逻辑删除乐观锁 ⇒ version = 0 + 1");
    let after = adapter
        .list("constraints", full_page())
        .expect("list after");
    assert_eq!(after.items.len(), 0, "逻辑删除后默认作用域不再列出");
}

// ════════════════════════════════════════════════════════════════════════════
//  conditions：可空 resource/capability 投影
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn conditions_commit_and_list_nullable_fields() {
    let store = store_repo_with_view();
    let adapter = adapter_with_fake_audit(Arc::clone(&store));
    let rev_before = adapter.policy_rev().expect("rev before");

    // 全局通用条件：resource_id / capability 皆缺（null）。
    let intent = WriteIntent {
        entity: "conditions",
        fields: serde_json::json!({
            "resource_id": serde_json::Value::Null,
            "capability": serde_json::Value::Null,
            "predicate": "business_hours",
        }),
        expected_version: None,
    };
    let outcome = adapter.commit_write(&operator(), &intent).expect("commit");
    assert!(outcome.policy_rev > rev_before, "condition 写 ⇒ rev 前进");

    let page = adapter.list("conditions", full_page()).expect("list");
    assert_eq!(page.items.len(), 1);
    assert_snowflake_string(first_str(&page, "id"), "condition id");
    assert!(
        page.items[0]
            .get("resource")
            .map(|v| v.is_null())
            .unwrap_or(false),
        "全局通用条件 resource 为 null（对齐 types.ts string|null）"
    );
    assert!(
        page.items[0]
            .get("capability")
            .map(|v| v.is_null())
            .unwrap_or(false),
        "全动词通用条件 capability 为 null"
    );
    assert_eq!(
        page.items[0].get("predicate").and_then(|v| v.as_str()),
        Some("business_hours"),
        "predicate 如实投影"
    );
}

// ════════════════════════════════════════════════════════════════════════════
//  deny-notes：commit + list 投影
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn deny_notes_commit_advances_rev_and_lists() {
    let store = store_repo_with_view();
    let res = seed_resource(&store, "db-main");
    let adapter = adapter_with_fake_audit(Arc::clone(&store));
    let rev_before = adapter.policy_rev().expect("rev before");

    let intent = WriteIntent {
        entity: "deny_notes",
        fields: serde_json::json!({
            "resource_id": res.as_raw().to_string(),
            "capability": "drop",
            "note": "destructive op — denied by operator",
        }),
        expected_version: None,
    };
    let outcome = adapter.commit_write(&operator(), &intent).expect("commit");
    assert!(outcome.policy_rev > rev_before, "deny-note 写 ⇒ rev 前进");

    let page = adapter.list("deny_notes", full_page()).expect("list");
    assert_eq!(page.items.len(), 1);
    assert_snowflake_string(first_str(&page, "id"), "deny-note id");
    assert_snowflake_string(first_str(&page, "resource"), "deny-note resource(雪花 id)");
    assert_eq!(
        page.items[0].get("note").and_then(|v| v.as_str()),
        Some("destructive op — denied by operator"),
        "note 如实投影（人亲笔说明）"
    );
}

// ════════════════════════════════════════════════════════════════════════════
//  mode：upsert（二次写改既有行 version 自增）+ list 投影
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn mode_upsert_advances_version_in_place() {
    let store = store_repo_with_view();
    let adapter = adapter_with_fake_audit(Arc::clone(&store));

    // 全局模式（scope null）首次写 ⇒ insert，version = 0。
    let set_observe = WriteIntent {
        entity: "mode",
        fields: serde_json::json!({ "scope": serde_json::Value::Null, "mode": "observe" }),
        expected_version: None,
    };
    let first = adapter
        .commit_write(&operator(), &set_observe)
        .expect("insert mode");
    assert_eq!(first.version, 0, "首次模式写 ⇒ insert version = 0");

    // 同辖区二次写 ⇒ update（upsert），version 自增到 1，不新增行。
    let set_freeze = WriteIntent {
        entity: "mode",
        fields: serde_json::json!({ "scope": serde_json::Value::Null, "mode": "freeze" }),
        expected_version: None,
    };
    let second = adapter
        .commit_write(&operator(), &set_freeze)
        .expect("mode upsert commits");
    assert_eq!(second.version, 1, "同辖区二次写 ⇒ upsert version = 0 + 1");
    assert!(
        second.policy_rev > first.policy_rev,
        "二次写 ⇒ rev 继续前进"
    );

    let page = adapter.list("mode", full_page()).expect("list mode");
    assert_eq!(page.items.len(), 1, "upsert ⇒ 同辖区仍是一行（未新增）");
    assert_eq!(
        page.items[0].get("mode").and_then(|v| v.as_str()),
        Some("freeze"),
        "就地改 mode 为 freeze"
    );
    assert!(
        page.items[0]
            .get("scope")
            .map(|v| v.is_null())
            .unwrap_or(false),
        "全局辖区 scope 为 null"
    );
    assert_snowflake_string(first_str(&page, "id"), "mode row id");
}

// ════════════════════════════════════════════════════════════════════════════
//  settings：upsert by key + list 投影
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn settings_upsert_by_key_advances_version() {
    let store = store_repo_with_view();
    let adapter = adapter_with_fake_audit(Arc::clone(&store));

    let set_v1 = WriteIntent {
        entity: "settings",
        fields: serde_json::json!({ "key": "approval.enabled", "value": "false" }),
        expected_version: None,
    };
    let first = adapter
        .commit_write(&operator(), &set_v1)
        .expect("insert setting");
    assert_eq!(first.version, 0, "首次 settings 写 ⇒ insert version = 0");

    let set_v2 = WriteIntent {
        entity: "settings",
        fields: serde_json::json!({ "key": "approval.enabled", "value": "true" }),
        expected_version: None,
    };
    let second = adapter
        .commit_write(&operator(), &set_v2)
        .expect("setting upsert commits");
    assert_eq!(second.version, 1, "同 key 二次写 ⇒ upsert version = 0 + 1");

    // 列读 = 已知设置目录叠加 store 持有值：固定小集（目录全员），持久化的 key 出存值 + version。
    let page = adapter
        .list("settings", full_page())
        .expect("list settings");
    let enabled = page
        .items
        .iter()
        .find(|r| r.get("key").and_then(|v| v.as_str()) == Some("approval.enabled"))
        .expect("目录含 approval.enabled");
    assert_eq!(
        enabled.get("value").and_then(|v| v.as_str()),
        Some("true"),
        "持久化值叠加进目录：就地改 value 为 true"
    );
    assert_eq!(
        enabled.get("version").and_then(|v| v.as_i64()),
        Some(1),
        "version 随 upsert 前进"
    );
    // 元数据由目录定义、不入库：default / writable / kind 恒随每行出线。
    assert_eq!(
        enabled.get("default").and_then(|v| v.as_str()),
        Some("false"),
        "approval.enabled 目录默认值 false"
    );
    assert_eq!(
        enabled.get("kind").and_then(|v| v.as_str()),
        Some("bool"),
        "approval.enabled 类型 bool"
    );
    assert_eq!(
        enabled.get("writable").and_then(|v| v.as_bool()),
        Some(true),
        "approval.enabled 可写"
    );

    // 未持久化的 key 仍出目录默认值 + version 0；on_timeout 恒不可写（L-12）。
    let on_timeout = page
        .items
        .iter()
        .find(|r| r.get("key").and_then(|v| v.as_str()) == Some("approval.on_timeout"))
        .expect("目录含 approval.on_timeout");
    assert_eq!(
        on_timeout.get("value").and_then(|v| v.as_str()),
        Some("deny"),
        "未持久化 ⇒ 出目录默认值 deny"
    );
    assert_eq!(
        on_timeout.get("writable").and_then(|v| v.as_bool()),
        Some(false),
        "approval.on_timeout 恒不可写（L-12）"
    );
}

// ════════════════════════════════════════════════════════════════════════════
//  grants：elevate（新增）/ revoke（乐观锁置终态）+ list 投影 id-string
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn grants_elevate_then_revoke() {
    let store = store_repo_with_view();
    let principal = seed_principal(&store, "agent-a");
    let resource = seed_resource(&store, "db-main");
    let adapter = adapter_with_fake_audit(Arc::clone(&store));

    // elevate：新增临时授权（version = 0）。principal 为雪花 id 字符串入线；resource 为资源
    // **代号**（恒为代号，经 store resource_id_by_code 反查为 id）。
    let elevate = WriteIntent {
        entity: "grants",
        fields: serde_json::json!({
            "op": "elevate",
            "principal": principal.as_raw().to_string(),
            "resource": "db-main",
            "capability": "mutate",
            "ttl_ms": 60000,
        }),
        expected_version: None,
    };
    let granted = adapter
        .commit_write(&operator(), &elevate)
        .expect("elevate commits");
    assert_eq!(granted.version, 0, "新增临时授权 version = 0");

    let page = adapter.list("grants", full_page()).expect("list grants");
    assert_eq!(page.items.len(), 1, "列读回刚发的临时授权");
    assert_snowflake_string(first_str(&page, "id"), "grant id");
    assert_snowflake_string(first_str(&page, "principal"), "grant principal(雪花 id)");
    assert_snowflake_string(first_str(&page, "resource"), "grant resource(雪花 id)");
    // 不泄露：principal/resource 投影是雪花 id 字符串，绝非真实地址。
    assert_eq!(
        first_str(&page, "resource"),
        resource.as_raw().to_string(),
        "grant.resource 为雪花 id（恒 id、绝非真实地址）"
    );
    assert!(
        page.items[0]
            .get("ended_at")
            .map(|v| v.is_null())
            .unwrap_or(false),
        "活跃授权 ended_at 为 null"
    );
    let grant_id = first_str(&page, "id").to_string();

    // revoke：乐观锁置终态（version 0 → 1），end_reason = revoked。
    let revoke = WriteIntent {
        entity: "grants",
        fields: serde_json::json!({ "op": "revoke", "id": grant_id }),
        expected_version: Some(0),
    };
    let revoked = adapter
        .commit_write(&operator(), &revoke)
        .expect("revoke commits");
    assert_eq!(revoked.version, 1, "撤销乐观锁 ⇒ version = 0 + 1");

    let after = adapter
        .list("grants", full_page())
        .expect("list after revoke");
    assert_eq!(
        after.items[0].get("end_reason").and_then(|v| v.as_str()),
        Some("revoked"),
        "撤销置 end_reason = revoked（终态、非逻辑删除，仍在列表）"
    );
}

#[test]
fn grants_elevate_nonpositive_ttl_is_rejected() {
    let store = store_repo_with_view();
    let principal = seed_principal(&store, "agent-a");
    // 资源代号存在（隔离出 ttl 这一项失败因素）：resource 反查成功，唯 ttl<=0 触发拒绝。
    seed_resource(&store, "db-main");
    let adapter = adapter_with_fake_audit(Arc::clone(&store));

    // ttl_ms <= 0 ⇒ 适配器解构 fail-closed（绝不发永久升权）。
    let bad = WriteIntent {
        entity: "grants",
        fields: serde_json::json!({
            "op": "elevate",
            "principal": principal.as_raw().to_string(),
            "resource": "db-main",
            "capability": "mutate",
            "ttl_ms": 0,
        }),
        expected_version: None,
    };
    let err = adapter
        .commit_write(&operator(), &bad)
        .expect_err("non-positive ttl rejected");
    assert_eq!(
        err,
        WriteError::Transaction,
        "ttl<=0 ⇒ 解构失败折为事务级失败（fail-closed，绝不发永久升权）"
    );
}

#[test]
fn grants_elevate_unknown_resource_code_is_not_found() {
    let store = store_repo_with_view();
    let principal = seed_principal(&store, "agent-a");
    // 刻意不 seed 任何资源：代号 "db-main" 库中不存在。
    let adapter = adapter_with_fake_audit(Arc::clone(&store));

    let elevate = WriteIntent {
        entity: "grants",
        fields: serde_json::json!({
            "op": "elevate",
            "principal": principal.as_raw().to_string(),
            "resource": "db-main",
            "capability": "mutate",
            "ttl_ms": 60000,
        }),
        expected_version: None,
    };
    let err = adapter
        .commit_write(&operator(), &elevate)
        .expect_err("unknown resource code rejected");
    assert_eq!(
        err,
        WriteError::ResourceNotFound,
        "未知资源代号 ⇒ ResourceNotFound（端点据此回 404，绝不误折为 500，绝不臆造资源）"
    );
    // fail-closed：未命中绝不留下半态临时授权。
    let page = adapter.list("grants", full_page()).expect("list grants");
    assert_eq!(page.items.len(), 0, "未命中代号 ⇒ 无任何临时授权落库");
}

#[test]
fn grants_elevate_resolves_code_case_insensitively() {
    let store = store_repo_with_view();
    let principal = seed_principal(&store, "agent-a");
    let resource = seed_resource(&store, "db-main");
    let adapter = adapter_with_fake_audit(Arc::clone(&store));

    // 入线代号大小写 / 首尾空白与入库归一化（trim + 小写）对齐：" DB-Main " 命中 "db-main"。
    let elevate = WriteIntent {
        entity: "grants",
        fields: serde_json::json!({
            "op": "elevate",
            "principal": principal.as_raw().to_string(),
            "resource": " DB-Main ",
            "capability": "mutate",
            "ttl_ms": 60000,
        }),
        expected_version: None,
    };
    let granted = adapter
        .commit_write(&operator(), &elevate)
        .expect("normalized code resolves to the same resource");
    assert_eq!(granted.version, 0, "新增临时授权 version = 0");

    let page = adapter.list("grants", full_page()).expect("list grants");
    assert_eq!(page.items.len(), 1, "归一化代号命中同一资源 ⇒ 落一条授权");
    assert_eq!(
        first_str(&page, "resource"),
        resource.as_raw().to_string(),
        "授权 resource 为反查到的资源雪花 id（恒 id、绝非代号 / 真实地址）"
    );
}

// ════════════════════════════════════════════════════════════════════════════
//  VersionConflict 全或无（乐观锁）
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn stale_version_maps_to_version_conflict_all_or_nothing() {
    let store = store_repo_with_view();
    let res = seed_resource(&store, "db-main");
    let adapter = adapter_with_fake_audit(Arc::clone(&store));
    // seed 一个 constraint。
    adapter
        .commit_write(
            &operator(),
            &WriteIntent {
                entity: "constraints",
                fields: serde_json::json!({
                    "resource_id": res.as_raw().to_string(), "capability": "mutate", "kind": "k",
                }),
                expected_version: None,
            },
        )
        .expect("seed constraint");
    let page = adapter.list("constraints", full_page()).expect("list");
    let id = first_str(&page, "id").to_string();
    let rev_before = adapter.policy_rev().expect("rev before");

    // 用过期期望版本（999 ≠ 实际 0）删除 ⇒ 乐观锁冲突。
    let del = WriteIntent {
        entity: "constraints",
        fields: serde_json::json!({ "id": id }),
        expected_version: Some(999),
    };
    let err = adapter
        .commit_write(&operator(), &del)
        .expect_err("stale version conflicts");
    assert_eq!(
        err,
        WriteError::VersionConflict,
        "乐观锁冲突 ⇒ WriteError::VersionConflict（端点映 409）"
    );
    // 全或无：rev 不进、行未删（仍在列表）。
    assert_eq!(
        adapter.policy_rev().expect("rev after"),
        rev_before,
        "冲突 ⇒ rev 不前进（整体 ROLLBACK）"
    );
    let after = adapter
        .list("constraints", full_page())
        .expect("list after");
    assert_eq!(after.items.len(), 1, "冲突 ⇒ 行未删（无半态）");
}

// ════════════════════════════════════════════════════════════════════════════
//  bindings 全量列读（store list_bindings 接通）
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn bindings_full_list_projects_id_strings() {
    let store = store_repo_with_view();
    let principal = seed_principal(&store, "agent-a");
    let role = store
        .create_role(&store_actor(), "reader", None)
        .expect("seed role");
    let adapter = adapter_with_fake_audit(Arc::clone(&store));
    adapter
        .commit_write(
            &operator(),
            &WriteIntent {
                entity: "bindings",
                fields: serde_json::json!({
                    "principal_id": principal.as_raw().to_string(),
                    "role_id": role.as_raw().to_string(),
                }),
                expected_version: None,
            },
        )
        .expect("create binding");

    let page = adapter
        .list("bindings", full_page())
        .expect("list bindings");
    assert_eq!(page.items.len(), 1, "全量 bindings 列读回刚建的一条");
    assert_snowflake_string(first_str(&page, "id"), "binding id");
    assert_snowflake_string(first_str(&page, "principal_id"), "binding principal_id");
    assert_snowflake_string(first_str(&page, "role_id"), "binding role_id");
}

// ════════════════════════════════════════════════════════════════════════════
//  audit / denials：经真实 JsonlAuditReader 投影（不构造 ConnOrigin、不泄露地址）
// ════════════════════════════════════════════════════════════════════════════

/// 装配真实审计读句柄（JsonlAuditReader over JsonlAuditSink），先把 JSONL 事件直接落到审计目录
/// （经 store 读模型 `AuditRecord::to_jsonl`——用本地 `OriginEnvelope`，**绝不**构造 `ConnOrigin`，
/// 雷区纪律），再经适配器 `list("audit")` / `list("denials_summary")` 投影。
#[test]
fn audit_list_projects_events_without_conn_origin() {
    use postern_store::audit::record::{AuditRecord, OriginEnvelope};

    let dir = std::env::temp_dir().join(format!("postern-d2bx-audit-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let sink = Arc::new(JsonlAuditSink::new(dir.clone(), FsyncPolicy::PerEvent));

    // 直接以 store 本地读模型 AuditRecord 落 JSONL（origin 用 OriginEnvelope，绝不构造 ConnOrigin）：
    // 一条 policy_change + 一条 deny。文件名为 UTC 日界（scan 按文件名倒序、日界窗口预筛）。
    let record = |id: &str, kind: &str, decision: &str| AuditRecord {
        id: id.to_string(),
        v: 1,
        kind: kind.to_string(),
        ts: "2026-06-14T00:00:00.000Z".to_string(),
        entry: "control".to_string(),
        // 本地信任域门（uid/gid）——store 本地信封，绝非 core `ConnOrigin`（读路径不构造来源类型）。
        origin: OriginEnvelope::UnixPeer {
            uid: 1000,
            gid: 1000,
        },
        decision: decision.to_string(),
        resource: "db-main".to_string(),
        policy_rev: 7,
        count: None,
    };
    let audit_dir = sink.audit_dir();
    std::fs::create_dir_all(&audit_dir).expect("audit dir");
    let line1 = record("100", "policy_change", "allow")
        .to_jsonl()
        .expect("jsonl 1");
    let line2 = record("101", "deny", "deny").to_jsonl().expect("jsonl 2");
    std::fs::write(
        audit_dir.join("2026-06-14.jsonl"),
        format!("{line1}\n{line2}\n"),
    )
    .expect("seed audit jsonl");

    let store = store_repo_with_view();
    let audit: Arc<dyn AuditRead> = Arc::new(JsonlAuditReader::new(Arc::clone(&sink)));
    let adapter = StorePolicyRepoAdapter::new(store, audit);

    let page = adapter.list("audit", full_page()).expect("list audit");
    assert_eq!(page.total, 2, "审计列读回两条事件");
    // origin 投影为脱敏不透明文本（绝不构造 ConnOrigin、绝不回显真实地址）。
    for item in &page.items {
        let origin = item
            .get("origin")
            .and_then(|v| v.as_str())
            .expect("origin 文本");
        assert!(
            origin.starts_with("unix:"),
            "本地对端 origin 投影为脱敏 unix:uid:gid 文本，得 {origin:?}"
        );
        // policy_rev 字符串化（u64 同雪花纪律）。
        assert_eq!(
            item.get("policy_rev").and_then(|v| v.as_str()),
            Some("7"),
            "audit.policy_rev 字符串化出线"
        );
        assert!(
            item.get("id").and_then(|v| v.as_str()).is_some(),
            "audit.id 为字符串"
        );
    }

    let denials = adapter
        .list("denials_summary", full_page())
        .expect("list denials");
    assert_eq!(
        denials.total, 1,
        "denials 摘要只回 deny 类（policy_change 不计）"
    );
    assert_eq!(
        denials.items[0].get("resource").and_then(|v| v.as_str()),
        Some("db-main"),
        "denials.resource 恒代号（绝非真实地址）"
    );
    assert!(
        denials.items[0]
            .get("count")
            .and_then(|v| v.as_u64())
            .is_some(),
        "denials.count 为计数"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

// ════════════════════════════════════════════════════════════════════════════
//  unknown entity / unknown grant op：fail-closed
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn unknown_entity_fails_closed_not_silently() {
    let store = store_repo_with_view();
    let adapter = adapter_with_fake_audit(store);
    let err = adapter
        .commit_write(
            &operator(),
            &WriteIntent {
                entity: "totally_unknown",
                fields: serde_json::json!({}),
                expected_version: None,
            },
        )
        .expect_err("unknown entity rejected");
    assert_eq!(
        err,
        WriteError::Transaction,
        "未知实体 ⇒ fail-closed 事务级失败（绝不静默放行）"
    );
}
