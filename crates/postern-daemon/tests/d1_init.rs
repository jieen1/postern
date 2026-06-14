//! D1 首启初始化行为测试（RED）。
//!
//! 钉死 [`bootstrap::init`](postern_daemon::bootstrap::init)（模块文档 06 §8.x / §5.1）：
//! 在临时目录一次性物化三件启动前置——
//! - **keyfile**：32B 主密钥文件，权限 **0600**（仅属主可读写）；
//! - **vault**：以 keyfile 主密钥可解锁的空 vault（两段 payload 皆空），经真 `vault::unlock` 验证；
//! - **db**：已迁移（`PRAGMA user_version == CURRENT_SCHEMA_VERSION`）的 policy.db。
//!
//! 并钉**幂等拒绝覆盖**：任一目标文件已存在时再 init ⇒ `Err`，现有文件不被毁。
//!
//! **KeyFile 路径无 argon2**（§5.2）：主密钥经 `KeyFile` 直接持有解锁、不跑 KDF，故本测试可直接
//! 跑、**无需** `systemd-run` 内存上限包裹（无 Passphrase/argon2 路径）。
//!
//! 先红后绿：`init` 体为 `unimplemented!()`，调用即 panic → 红。雷区：本文件零 SQL 标记
//! （建库 / 迁移全经 store API）、不构造 `ConnOrigin`/`ResolvedTarget`/`ResourceCredential`。

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use zeroize::Zeroizing;

use postern_daemon::bootstrap::init;
use postern_daemon::config::{DaemonConfig, Subcommand};

use postern_secrets::unlock::key_file::KeyFile;
use postern_secrets::unlock::source::MasterKeySource;
use postern_secrets::vault;

use postern_store::base::db::Db;
use postern_store::migrate::schema_version;
use postern_store::schema::CURRENT_SCHEMA_VERSION;

/// 进程唯一临时目录（无第三方 tempfile 依赖；进程内单调计数器命名）。
fn temp_dir(tag: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let dir = std::env::temp_dir().join(format!("postern-d1-init-{tag}-{pid}-{n}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

/// 在临时目录布一份配置（六路径全落该目录，组为 `None`）。
fn cfg_in(dir: &std::path::Path) -> DaemonConfig {
    DaemonConfig {
        db_path: dir.join("policy.db"),
        vault_path: dir.join("vault.postern"),
        keyfile_path: dir.join("keyfile"),
        control_sock: dir.join("control.sock"),
        control_token_path: dir.join("control.token"),
        data_sock: dir.join("data.sock"),
        data_sock_group: None,
    }
}

/// init 在干净目录生成三文件：keyfile(0600) + 可解锁 vault + 已迁移 db。
#[test]
fn init_creates_keyfile_vault_and_migrated_db() {
    let dir = temp_dir("happy");
    let cfg = cfg_in(&dir);

    init(&cfg).expect("init on clean dir must succeed");

    // keyfile 存在且权限恰 0600（低 9 位）。
    let kf_meta = std::fs::metadata(&cfg.keyfile_path).expect("keyfile must exist");
    assert_eq!(
        kf_meta.permissions().mode() & 0o777,
        0o600,
        "keyfile must be 0600 (owner-only)"
    );
    // keyfile 恰 32 字节主密钥。
    assert_eq!(kf_meta.len(), 32, "keyfile must hold a 32B master key");

    // vault 文件存在，且用 keyfile 主密钥可经真 vault::unlock 解锁（空 payload 两段）。
    let key_bytes = std::fs::read(&cfg.keyfile_path).expect("read keyfile");
    let mut master = Zeroizing::new([0u8; 32]);
    master.copy_from_slice(&key_bytes);
    let source = KeyFile::new(master);
    let master_key = source.obtain().expect("KeyFile obtain (no argon2)");

    let vault_bytes = std::fs::read(&cfg.vault_path).expect("vault file must exist");
    let unlocked = vault::unlock(&master_key, &vault_bytes).expect("vault must unlock via keyfile");
    // 空 vault：两段皆空（init 不预置任何凭据 / 地址）。
    assert!(
        unlocked.payload().secret_refs().is_empty(),
        "fresh vault secrets section must be empty"
    );
    assert!(
        unlocked.payload().target_codes().is_empty(),
        "fresh vault targets section must be empty"
    );

    // db 已迁移：user_version 追平到当前 schema 版本。
    let db = Db::open(&cfg.db_path).expect("db file must exist & open");
    assert_eq!(
        schema_version(&db).expect("read user_version"),
        CURRENT_SCHEMA_VERSION,
        "db must be migrated to current schema version"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// 重复 init 拒绝覆盖：任一目标文件已存在 ⇒ Err，现有 keyfile 字节不被毁。
#[test]
fn init_refuses_to_overwrite_existing_files() {
    let dir = temp_dir("idempotent");
    let cfg = cfg_in(&dir);

    init(&cfg).expect("first init must succeed");
    let key_before = std::fs::read(&cfg.keyfile_path).expect("read keyfile after first init");

    // 第二次 init 必须 fail-closed（拒绝覆盖现有主密钥 / vault / db）。
    let second = init(&cfg);
    assert!(
        second.is_err(),
        "repeat init must refuse to overwrite existing files"
    );

    // 现有 keyfile 字节逐字保留（绝不被半写 / 覆盖损坏）。
    let key_after = std::fs::read(&cfg.keyfile_path).expect("read keyfile after refused init");
    assert_eq!(
        key_before, key_after,
        "existing master key must survive refused re-init"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// init 子命令判别（与 config 解析对齐：argv `init` ⇒ Init）——锚定 init 路径的入口判别。
#[test]
fn init_subcommand_is_distinct_from_run() {
    assert_ne!(Subcommand::Init, Subcommand::Run);
}
