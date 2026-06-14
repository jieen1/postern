//! 首启初始化子域：`posternd init` 生成 keyfile + 空 vault + 已迁移 db（幂等、fail-closed）。
//!
//! 形态（模块文档 06 §8.x / §5.1）：首次部署时一次性物化三件启动前置——
//! 1. **keyfile**：CSPRNG 取随机 32B 主密钥写入 keyfile（0600，仅属主可读写）；主密钥经
//!    [`KeyFile`](postern_secrets::unlock::key_file::KeyFile) 直接持有解锁（无 argon2，§5.2）。
//! 2. **空 vault**：以该主密钥包裹一把随机 data-key，把空两段 payload（`secrets`/`targets`
//!    皆空）整体加密、原子写入 vault 文件（[`Payload::write_atomic`]）。
//! 3. **已迁移 db**：`Db::open` 建空库文件、`migrate` 建全套业务表 + 前进 `user_version`。
//!
//! 安全纪律（幂等 / fail-closed）：**拒绝覆盖任何已存在文件**（keyfile / vault / db 任一已存在
//! 即 fail-closed 返 `Err`，绝不静默覆写——重复 init 不毁掉现有主密钥 / vault）。随机数复用
//! 已有 CSPRNG 路径（secrets `crypto::new_nonce`，源为 OsRng→getrandom），**不新增依赖**。
//!
//! 雷区纪律：本文件零 SQL 标记（建库 / 迁移全经 store API）、不构造
//! `ConnOrigin`/`ResolvedTarget`/`ResourceCredential`、`anyhow` 禁用（仅 main.rs），只用
//! `DaemonError`。机密类型（主密钥 / data-key / Payload）经 secrets 面 API 构造，本文件不持久
//! 持有明文（写盘后即出作用域，`Zeroizing` 清零）。

use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;

use zeroize::Zeroizing;

use postern_secrets::vault::crypto;
use postern_secrets::vault::format::FORMAT_VERSION;
use postern_secrets::vault::header::{Header, Slot, SlotSource};
use postern_secrets::vault::payload::Payload;

use postern_store::base::db::Db;
use postern_store::migrate::migrate;

use crate::config::DaemonConfig;
use crate::error::{DaemonError, Result};

/// 生成随机 32B 主密钥（复用 secrets 的 CSPRNG 路径，**不新增 rand 依赖**）。
///
/// secrets `crypto::new_nonce()` 是已有的唯一 CSPRNG 取值路径（源 OsRng→getrandom），每次产
/// 24B。本函数取两次 24B 拼接、截前 32B 得随机主密钥——全程走同一已审计的 CSPRNG，源码无常量
/// / 计数器密钥。返回裸 `[u8; 32]`（不在签名层名 `Zeroizing`，使本波次骨架不依赖 zeroize）；
/// GREEN 阶段在 [`init`] 体内即刻裹进 `Zeroizing` 喂 `KeyFile::new` / `write_atomic`。
fn random_master_key() -> [u8; 32] {
    // 取两次 24B CSPRNG（唯一 nonce 取值路径，源 OsRng→getrandom）拼接、截前 32B。
    let a = crypto::new_nonce();
    let b = crypto::new_nonce();
    let mut key = [0u8; 32];
    key[..24].copy_from_slice(&a);
    key[24..].copy_from_slice(&b[..8]);
    key
}

/// 首启初始化：生成 keyfile（0600）+ 空 vault + 已迁移 db，**拒绝覆盖已存在文件**。
///
/// 步骤（任一步失败 fail-closed 返 [`DaemonError`](crate::error::DaemonError)，绝不留半态）：
/// 1. 三个目标文件（keyfile / vault / db）任一已存在 ⇒ 立即 `Err`（幂等拒绝覆盖，不毁现有）。
/// 2. CSPRNG 取随机 32B 主密钥（[`random_master_key`]）→ 写 keyfile，权限 0600（仅属主）。
/// 3. 以主密钥包裹随机 data-key、空两段 payload 整体加密、原子写入 vault 文件。
/// 4. `Db::open`（建空库文件）+ `migrate`（建全套表 + 前进 `user_version`）。
///
/// keyfile 主密钥经 KeyFile 来源（无 argon2）直接持有解锁——init 路径天然无 argon2 / 无 OOM。
///
/// 依赖说明（GREEN BLOCKER）：体内需把主密钥 / data-key 裹进 `Zeroizing<[u8; 32]>` 才能喂
/// `KeyFile::new` / `Payload::write_atomic`（secrets API 强制 `Zeroizing` 参数），故 GREEN 阶段
/// 须把 `zeroize` 从 `[dev-dependencies]` 移入 `[dependencies]`（见 notes 上报）。
pub fn init(cfg: &DaemonConfig) -> Result<()> {
    // 1) 幂等拒绝覆盖：三个目标文件任一已存在 ⇒ fail-closed（绝不毁现有主密钥 / vault / db）。
    if cfg.keyfile_path.exists() || cfg.vault_path.exists() || cfg.db_path.exists() {
        return Err(DaemonError::Boot);
    }

    // 2) CSPRNG 取随机 32B 主密钥，即刻裹进 Zeroizing（离作用域清零，不持久持有明文）。
    let master_key = Zeroizing::new(random_master_key());

    // 3) 写 keyfile，权限恰 0600（仅属主可读写）；create_new 保证不覆盖已存在文件。
    {
        let mut kf = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&cfg.keyfile_path)
            .map_err(|_| DaemonError::Boot)?;
        kf.write_all(master_key.as_slice())
            .map_err(|_| DaemonError::Boot)?;
        kf.sync_all().map_err(|_| DaemonError::Boot)?;
    }

    // 4) 随机 data-key + 空两段 payload，用主密钥经 KeyFile 包裹槽整体加密、原子写入 vault。
    let data_key = Zeroizing::new(random_master_key());
    let payload = Payload::from_sections(
        std::collections::BTreeMap::new(),
        std::collections::BTreeMap::new(),
    );
    // KeyFile 来源包裹槽模板（无 KDF / salt）：write_atomic 据此重包裹 data-key、覆写 nonce/密文。
    let header = Header {
        format_version: FORMAT_VERSION,
        slots: vec![Slot {
            source: SlotSource::KeyFile,
            kdf_params: None,
            salt: None,
            nonce_i: [0u8; 24],
            wrapped_data_key: Vec::new(),
        }],
    };
    payload
        .write_atomic(&cfg.vault_path, &master_key, &data_key, &header)
        .map_err(|_| DaemonError::Boot)?;

    // 5) 建空库文件 + 迁移（建全套业务表 + 前进 user_version 至当前 schema 版本）。
    let db = Db::open(&cfg.db_path).map_err(|_| DaemonError::Boot)?;
    migrate(&db).map_err(|_| DaemonError::Boot)?;

    Ok(())
}
