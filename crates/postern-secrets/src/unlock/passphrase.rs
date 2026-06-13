//! passphrase 来源：**唯一**经 argon2id KDF 派生 32B 主密钥的来源（§5.2 / 详细设计 5.4）。
//!
//! 真实强度（诚实表述，§5.2 表 + §7-7）：**取决于口令熵 + argon2id 参数**。本来源
//! 持有 salt 与 argon2id 参数（m_cost/t_cost/p_cost），`obtain` 以 argon2id 把口令
//! 派生为 32 字节主密钥。
//!
//! 诚实约束（§5.2 / §7-7、契约 B-8）：`passphrase` **仅适用于有人值守场景**
//! （启动交互输入），与「常驻 daemon 重启需无人干预解锁」**互斥**——无人值守常驻
//! 应改选受保护的自动解锁来源（`systemd_cred`/TPM、`os_keychain`），不以明文缓存
//! 口令作变通。
//!
//! 依赖纪律（雷区）：argon2 是 secrets 专属允许依赖，**仅在本实现引入使用**；
//! `key_file`/`os_keychain`/`systemd_cred` 不得引 argon2（强度诚实：无 KDF 加固）。
//! 口令与 KDF 中间态一律 `Zeroizing` 持有，离作用域清零；不 Debug 出明文。

use zeroize::Zeroizing;

use crate::error::UnlockError;
use crate::unlock::source::MasterKeySource;

/// argon2id 派生参数（仅本来源持有）。强度随这些参数与口令熵变化，不一概宣称固定强度。
#[derive(Clone, Copy)]
pub struct Argon2Params {
    /// 内存成本（KiB）。
    pub m_cost: u32,
    /// 迭代次数。
    pub t_cost: u32,
    /// 并行度。
    pub p_cost: u32,
}

/// argon2id 参数的**安全上限**（fail-closed 校验，防 unlock 期 OOM 拒绝服务）。
///
/// 设计文档（详细设计 5.4 / 模块 §5.2）只规定 argon2id「内存硬」与强度诚实，未给出
/// 具体数值上限；这里取**拒绝病态大值**为目标的保守上限：production 默认 19~64 MiB
/// （19_456~65_536 KiB）必须仍被接受，GB/TB 级值必被拒。
///
/// `m_cost` 以 KiB 计——`MAX_M_COST` = 1 GiB（1_048_576 KiB）即 argon2 内存峰值上限 1 GiB，
/// 远高于 production 默认（64 MiB）、远低于被篡改文件可写入的 ~4 TiB（`u32::MAX` KiB）。
/// 一个被篡改保险箱把 m_cost 写成接近 `u32::MAX` 时，本上限在调用 argon2 前即拦截，
/// argon2 永不据此申请内存。
const MAX_M_COST: u32 = 1_048_576; // KiB = 1 GiB 内存峰值上限
/// `t_cost`（迭代）上限——production 默认 1~4，64 已极保守。
const MAX_T_COST: u32 = 64;
/// `p_cost`（并行度）上限——production 默认 1~4，16 已极保守。
const MAX_P_COST: u32 = 16;

impl Argon2Params {
    /// fail-closed 范围校验：m_cost/t_cost/p_cost 任一超出安全上限即 `Err`，
    /// **在调用 argon2 之前**拒绝。防被篡改保险箱文件以病态大 m_cost 触发 unlock 期 OOM。
    fn validate(&self) -> Result<(), UnlockError> {
        if self.m_cost > MAX_M_COST || self.t_cost > MAX_T_COST || self.p_cost > MAX_P_COST {
            return Err(UnlockError::KdfParamsOutOfRange);
        }
        Ok(())
    }
}

/// passphrase 解锁来源：经 argon2id KDF 把口令派生为 32B 主密钥。
///
/// 行为承诺（§8 F-2）：**同口令 + 同 salt + 同参数 → 同一 32B 主密钥**（确定性派生，
/// 无随机化）；口令熵不足时强度即随之下降——KDF 不凭空创造熵（§5.2 诚实表述）。
///
/// 口令以 `Zeroizing` 持有，不 derive `Debug`（避免明文外泄）。
pub struct Passphrase {
    passphrase: Zeroizing<Vec<u8>>,
    salt: Vec<u8>,
    params: Argon2Params,
}

impl Passphrase {
    /// 以口令、salt、argon2id 参数构造。口令立即移入 `Zeroizing` 容器。
    pub fn new(passphrase: Zeroizing<Vec<u8>>, salt: Vec<u8>, params: Argon2Params) -> Self {
        Self {
            passphrase,
            salt,
            params,
        }
    }
}

impl MasterKeySource for Passphrase {
    fn obtain(&self) -> Result<Zeroizing<[u8; 32]>, UnlockError> {
        // **越界校验先行（fail-closed）**：被篡改保险箱可把 m_cost 写成 GB/TB 级病态值，
        // argon2 0.5.x 的 Params::new 不设 m_cost 上限（MAX_M_COST == u32::MAX，不校验），
        // 会直接据此申请内存→OOM 拒绝服务。故在**触碰 argon2 之前**先按安全上限拒绝越界参数，
        // 返回 KdfParamsOutOfRange，绝不进入 argon2 的内存分配路径（B-6 / L-2）。
        self.params.validate()?;

        // argon2id 派生：以本来源参数构造 argon2id KDF，把口令派生为 32B 主密钥。
        // 任何环节失败（参数非法 / salt 过短 / 派生出错）一律 map 为
        // UnlockError::KdfFailed（fail-closed），绝不 unwrap / panic（B-6）。
        let params = argon2::Params::new(
            self.params.m_cost,
            self.params.t_cost,
            self.params.p_cost,
            Some(32),
        )
        .map_err(|_| UnlockError::KdfFailed)?;
        let kdf = argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);
        // 输出缓冲以 Zeroizing 持有，离作用域清零；不 Debug、不 log 明文。
        let mut out: Zeroizing<[u8; 32]> = Zeroizing::new([0u8; 32]);
        kdf.hash_password_into(&self.passphrase, &self.salt, out.as_mut())
            .map_err(|_| UnlockError::KdfFailed)?;
        Ok(out)
    }
}
