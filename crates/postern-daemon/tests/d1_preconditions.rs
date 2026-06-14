//! D1 真实前置（RealPreconditions）行为测试（RED）。
//!
//! 钉死 [`RealPreconditions`](postern_daemon::boot::real::RealPreconditions)（模块文档 06 §3.1
//! / §8 F-1）对真实临时 db + keyfile vault 的四前置：
//! - happy：`open_db` / `migrate` / `rebuild_first_snapshot` / `unlock_vault` 全 `Ok`；
//! - **坏 vault → `unlock_vault` Err**（fail-closed）：vault 字节损坏时解锁必失败，data.sock 不创建。
//!
//! 顺序即依赖顺序：`open_db` 产 `Db` 留给 `migrate`/`rebuild_first_snapshot` 消费、`unlock_vault`
//! 经 keyfile 主密钥 + vault 字节解锁。**KeyFile 路径无 argon2**（§5.2），可直接跑、无需 systemd-run。
//!
//! 先红后绿：四方法体 `unimplemented!()`，调用即 panic → 红。雷区：本文件零 SQL 标记、不构造
//! `ConnOrigin`/`ResolvedTarget`/`ResourceCredential`；vault 经 secrets 真实构造路径封装。

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use zeroize::Zeroizing;

use postern_daemon::boot::real::RealPreconditions;
use postern_daemon::boot::Preconditions;

use postern_secrets::vault::crypto;
use postern_secrets::vault::format::{VaultFile, FORMAT_VERSION, NONCE_LEN};
use postern_secrets::vault::header::{Header, Slot, SlotSource};
use postern_secrets::vault::payload::Payload;

/// 固定 32B 主密钥（KeyFile 直接持有型来源，避开 argon2id KDF）。
const MASTER_KEY: [u8; 32] = [
    0xa1, 0xb2, 0xc3, 0xd4, 0xe5, 0xf6, 0x07, 0x18, 0x29, 0x3a, 0x4b, 0x5c, 0x6d, 0x7e, 0x8f, 0x90,
    0x01, 0x12, 0x23, 0x34, 0x45, 0x56, 0x67, 0x78, 0x89, 0x9a, 0xab, 0xbc, 0xcd, 0xde, 0xef, 0xf0,
];
/// 固定 32B data-key（包裹槽包裹的就是它）。
const DATA_KEY: [u8; 32] = [
    0x10, 0x20, 0x30, 0x40, 0x50, 0x60, 0x70, 0x80, 0x90, 0xa0, 0xb0, 0xc0, 0xd0, 0xe0, 0xf0, 0x00,
    0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff, 0x01,
];

/// 进程唯一临时目录（无第三方 tempfile 依赖）。
fn temp_dir(tag: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let dir = std::env::temp_dir().join(format!("postern-d1-pre-{tag}-{pid}-{n}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

/// 封一份**空两段** vault 字节（用 MASTER_KEY 包裹 DATA_KEY，KeyFile 来源、无 KDF）。
fn empty_vault_bytes() -> Vec<u8> {
    let payload = Payload::from_sections(Default::default(), Default::default());
    let plaintext = payload.to_plaintext().expect("serialize empty payload");

    let dk = Zeroizing::new(DATA_KEY);
    let (slot_nonce, wrapped) =
        crypto::wrap_data_key(&MASTER_KEY, &dk).expect("wrap data-key under master key");
    let header = Header {
        format_version: FORMAT_VERSION,
        slots: vec![Slot {
            source: SlotSource::KeyFile,
            kdf_params: None,
            salt: None,
            nonce_i: slot_nonce,
            wrapped_data_key: wrapped,
        }],
    };
    let mut vf = VaultFile {
        header,
        payload_nonce: [0u8; NONCE_LEN],
        ciphertext: Vec::new(),
    };
    let aad = vf.aad_bytes();
    let (payload_nonce, ciphertext) =
        crypto::encrypt_payload(&dk, &plaintext, &aad).expect("encrypt empty payload");
    vf.payload_nonce = payload_nonce;
    vf.ciphertext = ciphertext;
    vf.encode()
}

/// 在临时目录写 keyfile（32B 主密钥）+ vault（`vault_bytes`）；返回 db/vault/keyfile 路径。
fn lay_out(dir: &Path, vault_bytes: &[u8]) -> (PathBuf, PathBuf, PathBuf) {
    let keyfile = dir.join("keyfile");
    let vault = dir.join("vault.postern");
    let db = dir.join("policy.db");
    std::fs::write(&keyfile, MASTER_KEY).expect("write keyfile");
    std::fs::write(&vault, vault_bytes).expect("write vault");
    (db, vault, keyfile)
}

/// happy：四前置对真实临时 db + keyfile vault 全 Ok（顺序即依赖顺序）。
#[test]
fn real_preconditions_all_ok_on_good_db_and_vault() {
    let dir = temp_dir("ok");
    let (db, vault, keyfile) = lay_out(&dir, &empty_vault_bytes());
    let pre = RealPreconditions::new(db, vault, keyfile);

    pre.open_db().expect("open_db on fresh path must Ok");
    pre.migrate().expect("migrate must Ok");
    pre.rebuild_first_snapshot()
        .expect("rebuild_first_snapshot must Ok");
    pre.unlock_vault()
        .expect("unlock_vault on good vault must Ok");

    let _ = std::fs::remove_dir_all(&dir);
}

/// 坏 vault → unlock_vault Err（fail-closed：解锁失败，data.sock 不创建）。
#[test]
fn real_preconditions_bad_vault_fails_unlock_closed() {
    let dir = temp_dir("badvault");
    // 损坏 vault 字节（非法魔数 / 截断）——解锁必失败。
    let (db, vault, keyfile) = lay_out(&dir, b"not-a-valid-vault-file");
    let pre = RealPreconditions::new(db, vault, keyfile);

    // 前置链可先走开库 / 迁移（db 正常），但解锁坏 vault 必 fail-closed。
    pre.open_db().expect("open_db Ok");
    pre.migrate().expect("migrate Ok");
    pre.rebuild_first_snapshot()
        .expect("rebuild_first_snapshot Ok");
    assert!(
        pre.unlock_vault().is_err(),
        "corrupt vault must fail unlock_vault (fail-closed)"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
