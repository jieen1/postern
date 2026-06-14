//! D2b 写接缝 — control::PolicyRepo 适配器层行为测试（RED）。
//!
//! 钉死 [`StorePolicyRepoAdapter`](postern_daemon::control::repo::StorePolicyRepoAdapter) 把控制面
//! 缝 [`PolicyRepo`](postern_daemon::control::PolicyRepo) 接到 store 的写接缝（§8 L-14）：
//! - **写 → rev 前进**：`commit_write` 经 store `*_and_rebuild` 原子「实体写 + bump_policy_rev +
//!   COMMIT + 重建发布快照」，返 [`WriteOutcome`]，`policy_rev` 较前一次严格前进。
//! - **list 投影**：`list(entity, page)` 经 store `list_*` 取读模型行，投影为
//!   `serde_json::Value`（id 一律字符串，雪花不丢精度）。
//! - **VersionConflict 全或无**：乐观锁期望版本不符 ⇒ store `VersionConflict` ⇒
//!   [`WriteError::VersionConflict`]，且整体 ROLLBACK（库不变、rev 不进、快照不换）。
//!
//! 驱动方式：以真实 store（内存库 + migrate + 持视图 `PolicyRepo`）装配适配器——store 侧写接缝
//! （`*_and_rebuild`）已实现，故「store 写 + rev + 重建」可端到端观测；适配器解构 / 投影体本波次
//! 为 `unimplemented!()` 骨架，故经 `commit_write` / `list` 驱动的断言**当前红（panic 于
//! unimplemented）**，GreenAuth 域内填体后转绿。store 侧 `*_and_rebuild` 直驱的几条**当前即绿**
//! （证明写接缝的 store 半边确已落地、rev 严格前进、冲突全或无）。
//!
//! 雷区纪律：本文件**零 SQL 标记**（建库 / 迁移 / 写 / 读全经 store 公共 API，绝不拼裸 SQL）；
//! 不构造 `ConnOrigin` / 机密类型；`anyhow` 禁用。argon2 不在本路径（store 写 / 快照无 KDF），
//! 故可直接 `cargo test -p postern-daemon --test d2b-repo` 跑，无需 systemd-run 内存包裹。

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use std::sync::Arc;

use postern_core::domain::PolicySnapshot;
use postern_core::id::{Clock, IdGen};
use postern_core::page::PageQuery;

use postern_daemon::control::repo::StorePolicyRepoAdapter;
use postern_daemon::control::{Actor, PolicyRepo, WriteError, WriteIntent};

use postern_store::base::db::Db;
use postern_store::base::meta::read_policy_rev;
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

/// 已迁移到当前版本的内存库（空库 → `migrate` 建全套表 + policy_meta + 前进 user_version）。
fn migrated_db() -> Db {
    let db = Db::open_in_memory().expect("in-memory db opens");
    migrate::migrate(&db).expect("migrate builds full schema on empty db");
    db
}

/// 装配持视图的 store 写句柄 + 它发布的初始视图（首份快照 policy_rev=0）。
fn store_repo_with_view() -> (Arc<StoreRepo>, Arc<SnapshotView>) {
    let db = migrated_db();
    let view = Arc::new(SnapshotView::new(Arc::new(PolicySnapshot::default())));
    let repo = StoreRepo::with_view(
        db,
        IdGen::new(FixedClock(EPOCH_UNIX_MS)),
        Box::new(FixedClock(EPOCH_UNIX_MS)),
        Arc::clone(&view),
    );
    (Arc::new(repo), view)
}

/// 控制面操作者（落 created_by / updated_by）。
fn operator() -> Actor {
    Actor::Operator("tester".to_string())
}

/// 一次新增主体的写意图（业务字段 JSON；新增 ⇒ expected_version None）。
fn create_principal_intent() -> WriteIntent {
    WriteIntent {
        entity: "principals",
        fields: serde_json::json!({ "name": "agent-a", "kind": "agent" }),
        expected_version: None,
    }
}

// ════════════════════════════════════════════════════════════════════════════
//  store 侧写接缝（*_and_rebuild）直驱——当前即绿（证明写接缝 store 半边已落地）
// ════════════════════════════════════════════════════════════════════════════

/// store `create_principal_and_rebuild`：新增主体 ⇒ rev 由 0 前进到 1，新行 version=0，
/// 视图发布的快照 policy_rev 与返回 rev 一致（写 + bump + 重建同一临界区原子）。
#[test]
fn store_create_and_rebuild_advances_rev_and_publishes_snapshot() {
    let (repo, view) = store_repo_with_view();

    let (version, rev) = repo
        .create_principal_and_rebuild(&StoreActor::Operator("tester".into()), "agent-a", "agent")
        .expect("create principal + rebuild succeeds");

    assert_eq!(
        version, 0,
        "INSERT 落新行 version = 0（乐观锁下一期望前驱）"
    );
    assert_eq!(
        rev, 1,
        "首次实体写 ⇒ policy_rev 由 0 前进到 1（写接缝原子 bump）"
    );
    assert_eq!(
        read_policy_rev(repo.db()).expect("read policy_rev"),
        1,
        "持久 policy_rev 与返回 rev 一致（同事务 bump + COMMIT）"
    );
    // 重建已在同一临界区发布：视图快照的 policy_rev 即新 rev（无 torn 态）。
    use postern_core::plugin::PolicyView;
    assert_eq!(
        view.snapshot().policy_rev,
        1,
        "重建后视图发布的快照 policy_rev == 新 rev（写锁内原子 replace，单一权威状态）"
    );
}

/// store `rename_principal_and_rebuild` 乐观锁冲突 ⇒ `VersionConflict` 且全或无：
/// 库不变、rev 不进、视图快照不换（整体 ROLLBACK）。
#[test]
fn store_rename_and_rebuild_version_conflict_is_all_or_nothing() {
    let (repo, view) = store_repo_with_view();
    let id = repo
        .create_principal(&StoreActor::Operator("tester".into()), "agent-a", "agent")
        .expect("seed principal");
    // 上面 create_principal 不重建（各自 with_write_txn），故 rev 仍为 0；新行 version = 0。
    let rev_before = read_policy_rev(repo.db()).expect("read rev before");

    use postern_core::plugin::PolicyView;
    let snap_rev_before = view.snapshot().policy_rev;

    // 用错误的期望版本（999 ≠ 实际 0）改名 ⇒ 乐观锁冲突。
    let err = repo
        .rename_principal_and_rebuild(&StoreActor::Operator("tester".into()), id, 999, "agent-b")
        .expect_err("stale expected_version must conflict");
    assert!(
        matches!(err, postern_store::base::error::StoreError::VersionConflict),
        "乐观锁期望版本不符 ⇒ VersionConflict（独立变体，端点据此 409）"
    );
    assert_eq!(
        read_policy_rev(repo.db()).expect("read rev after"),
        rev_before,
        "冲突 ⇒ rev 不前进（写接缝全或无：实体写失败则不 bump）"
    );
    assert_eq!(
        view.snapshot().policy_rev,
        snap_rev_before,
        "冲突 ⇒ 视图快照不换（不重建、不 replace，无半态）"
    );
}

// ════════════════════════════════════════════════════════════════════════════
//  适配器层（commit_write / list / policy_rev）——当前红（unimplemented 骨架）
// ════════════════════════════════════════════════════════════════════════════

/// 适配器 `commit_write` 新增主体 ⇒ rev 前进：返回的 `WriteOutcome.policy_rev` 较初始严格前进，
/// version 为新行版本（0）。当前红（适配器写解构 unimplemented）。
#[test]
fn adapter_commit_write_advances_policy_rev() {
    let (store, _view) = store_repo_with_view();
    let adapter = StorePolicyRepoAdapter::new(store);

    let outcome = adapter
        .commit_write(&operator(), &create_principal_intent())
        .expect("commit_write succeeds");

    assert_eq!(outcome.version, 0, "新增主体新行 version = 0");
    assert_eq!(
        outcome.policy_rev, 1,
        "写经三联动 ⇒ policy_rev 由 0 前进到 1（rev 前进）"
    );
}

/// 适配器 `list` 投影：新增一行后列读 principals ⇒ `Page` 信封含一项，且 id 投影为**字符串**
/// （雪花不丢精度）。当前红（适配器 list 投影 unimplemented）。
#[test]
fn adapter_list_projects_id_as_string() {
    let (store, _view) = store_repo_with_view();
    // 经 store 公共写接缝先落一行（已实现），再经适配器列读（待实现）。
    store
        .create_principal_and_rebuild(&StoreActor::Operator("tester".into()), "agent-a", "agent")
        .expect("seed via store write seam");
    let adapter = StorePolicyRepoAdapter::new(store);

    let page = adapter
        .list(
            "principals",
            PageQuery {
                page_no: 1,
                page_size: 20,
            },
        )
        .expect("list principals succeeds");

    assert_eq!(page.items.len(), 1, "列读回一项（刚落的主体）");
    let id = page.items[0]
        .get("id")
        .and_then(|v| v.as_str())
        .expect("id 字段存在且为字符串（雪花一律 string，绝不为 JSON number）");
    assert!(
        !id.is_empty() && id.chars().all(|c| c.is_ascii_digit()),
        "id 为十进制雪花字符串（不丢精度）"
    );
}

/// 适配器 `commit_write` 乐观锁冲突 ⇒ `WriteError::VersionConflict`（端点据此 409）。
/// 当前红（适配器写解构 unimplemented）。
#[test]
fn adapter_commit_write_stale_version_maps_to_version_conflict() {
    let (store, _view) = store_repo_with_view();
    let id = store
        .create_principal(&StoreActor::Operator("tester".into()), "agent-a", "agent")
        .expect("seed principal");
    let adapter = StorePolicyRepoAdapter::new(store);

    // 改名意图带过期期望版本（999 ≠ 实际 0）。
    let intent = WriteIntent {
        entity: "principals",
        fields: serde_json::json!({ "id": id.as_raw().to_string(), "name": "agent-b" }),
        expected_version: Some(999),
    };
    let err = adapter
        .commit_write(&operator(), &intent)
        .expect_err("stale version must be a write error");
    assert_eq!(
        err,
        WriteError::VersionConflict,
        "乐观锁冲突 ⇒ WriteError::VersionConflict（端点映射 409 Conflict）"
    );
}

/// 适配器 `policy_rev`：取自 store 快照视图（健康端点对账锚点）。当前红（unimplemented）。
#[test]
fn adapter_policy_rev_reflects_snapshot() {
    let (store, _view) = store_repo_with_view();
    let adapter = StorePolicyRepoAdapter::new(store);
    assert_eq!(
        adapter.policy_rev().expect("policy_rev readable"),
        0,
        "首份快照 policy_rev = 0（尚无提交）"
    );
}
