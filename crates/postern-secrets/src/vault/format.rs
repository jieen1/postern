//! vault 文件整体格式与 `format_version`（设计承诺级桩，函数体未实现）。
//!
//! 文件布局（详细设计 5.4 / §7-6）：
//! ```text
//! magic "PSTRN" + format_version u8 + source  ← payload AEAD 的 AAD（AAD_PREFIX_LEN）
//! 包裹槽其余 slot{kdf_params?,salt?,            ← 随 rekey/rotate-kdf 重写，不在 AAD 内
//!            nonce_i, wrapped_data_key}            （F-9：换 KDF 只重包裹、payload 密文不动）
//! nonce (24B)
//! ciphertext = XChaCha20-Poly1305(payload)
//! ```
//! AAD = 明文头里 rekey / rotate-kdf **绝不重写**的前缀（魔数 + `format_version` + source），
//! 防头部前缀篡改 / `format_version` 降级（L-2）；被排除的包裹槽材料篡改改由派生主密钥 /
//! 包裹槽 AEAD 拦截，仍 fail-closed（详见 `AAD_PREFIX_LEN`）。`format_version` 不识别即拒锁
//! （L-1，fail-closed，绝不按未知格式尝试解密）。
//!
//! 雷区纪律：本文件只做字节编解码（`std` 字节切片，不碰数据库、不引任何数据库 / SQL 解析依赖）；
//! 任何切片 / 算术用 `get` / `checked` 形式，解析失败一律 map 成 `UnlockError`（B-6）。

use crate::error::UnlockError;
use crate::vault::header::{Header, KdfParams, Slot, SlotSource};

/// vault 文件魔数：`b"PSTRN"`（5 字节）。文件首段，先于 `format_version`。
pub const MAGIC: [u8; 5] = *b"PSTRN";

/// 当前实现识别的唯一 `format_version`。明文头 `format_version` ≠ 此值即拒锁
/// （L-1：`UnknownFormatVersion`，绝不按未知格式尝试解密）。
pub const FORMAT_VERSION: u8 = 1;

/// XChaCha20-Poly1305 的 payload nonce 字节长度（24B；24B 空间使随机生成在 nonce
/// 空间内安全、无需计数器，L-3）。
pub const NONCE_LEN: usize = 24;

/// 解析后的 vault 文件结构（明文头 + payload nonce + payload 密文）。
///
/// 这是 `decode` 的产物 / `encode` 的输入——只承载已切分的字节段，不持有任何明文
/// 机密（payload 仍是密文，解密在 `crypto` 单元、产物在 `Zeroizing`）。
pub struct VaultFile {
    /// 明文头（魔数 + `format_version` + 包裹槽）。全段作为 AEAD AAD。
    pub header: Header,
    /// payload 的 24B nonce（每次写入 CSPRNG 重生成，绝不复用，L-3）。
    pub payload_nonce: [u8; NONCE_LEN],
    /// payload 密文（XChaCha20-Poly1305 输出，含 Poly1305 tag）。
    pub ciphertext: Vec<u8>,
}

/// payload AEAD 的 AAD 前缀长度（固定）：magic(5) + format_version(1) + source(1) = 7。
///
/// AAD 只绑定 rekey / rotate-kdf **绝不重写**的头部字段（魔数、`format_version`、来源
/// 判别）；包裹槽里随轮换被重写的材料——`kdf_params` / `salt`（rotate-kdf 换）、
/// `nonce_i` / `wrapped_data_key`（rekey + rotate-kdf 换）——一律**排除**在 AAD 之外，
/// 否则换 KDF 参数 / salt 会改动 AAD、使原 payload 密文（一字未动）解锁必败（违 F-9：
/// 「换 argon2id 参数后同一 data-key 仍解原 payload、payload 密文未变」、违详细设计 5.4
/// 「换 KDF 只需重包裹 data-key、无需整体重加密」）。
///
/// 排除这些字段不削弱 fail-closed（L-2）：`kdf_params` / `salt` 篡改使 passphrase 派生出
/// 不同主密钥 → 开包裹槽失败（`PayloadDecryptFailed`）；`nonce_i` / `wrapped_data_key`
/// 篡改使包裹槽 AEAD 校验失败（`PayloadDecryptFailed`）；魔数 / 版本 / source 篡改仍由
/// 此 AAD（或 decode 层）拦截。故明文头任一字节篡改仍拒锁，仅错误码按被篡改区段而异。
const AAD_PREFIX_LEN: usize = MAGIC.len() + 1 + 1;

impl VaultFile {
    /// 把整文件字节解析为 `VaultFile`：校验魔数、读 `format_version`、切出明文头 /
    /// payload nonce / 密文。
    ///
    /// 单槽起步（结构预留多槽）：当前格式恰一个包裹槽，固定布局。`source` 字节即便为
    /// 未识别值也不在 `decode` 处拒绝——解锁路径用**原始前缀字节**作 AAD（见
    /// `aad_slice`），任何前缀字节篡改由 AEAD 校验拦为 `AadMismatch`（L-2），故 `decode`
    /// 只需稳定切分、不抢先判别 source 合法性。
    ///
    /// fail-closed（B-6）：魔数不符 / 长度不足 / `format_version` 不识别一律返回
    /// `UnlockError`（不识别版本返回 `UnknownFormatVersion`，绝不按未知格式继续解析），
    /// 绝不 unwrap / panic / 越界索引。
    pub fn decode(bytes: &[u8]) -> Result<Self, UnlockError> {
        let mut r = Reader::new(bytes);

        // 魔数：不符即拒（损坏 / 非 vault 文件）。
        let magic = r
            .take(MAGIC.len())
            .ok_or(UnlockError::PayloadDecryptFailed)?;
        if magic != MAGIC {
            return Err(UnlockError::PayloadDecryptFailed);
        }

        // format_version：不识别即在 codec 层 fail-closed（L-1），绝不按未知格式继续切分。
        let version = r.take_u8().ok_or(UnlockError::UnknownFormatVersion)?;
        if version != FORMAT_VERSION {
            return Err(UnlockError::UnknownFormatVersion);
        }

        let slot = decode_slot(&mut r)?;

        // payload nonce + 密文（密文取剩余全部字节）。
        let payload_nonce = r.take_nonce().ok_or(UnlockError::PayloadDecryptFailed)?;
        let ciphertext = r.take_rest().to_vec();

        Ok(VaultFile {
            header: Header {
                format_version: version,
                slots: vec![slot],
            },
            payload_nonce,
            ciphertext,
        })
    }

    /// 把 `VaultFile` 序列化回整文件字节（魔数 + `format_version` + 包裹槽 + payload
    /// nonce + 密文）。`encode` 与 `decode` 互逆。
    pub fn encode(&self) -> Vec<u8> {
        let mut out = self.header_bytes();
        out.extend_from_slice(&self.payload_nonce);
        out.extend_from_slice(&self.ciphertext);
        out
    }

    /// 明文头全段的规范字节（魔数 + `format_version` + 单包裹槽）。`encode` 的头部分。
    fn header_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&MAGIC);
        out.push(self.header.format_version);
        match self.header.primary_slot() {
            Some(slot) => encode_slot(slot, &mut out),
            // 损坏的空槽头：编码退化为占位空槽（解码端会 fail-closed）。
            None => encode_slot(&empty_slot(), &mut out),
        }
        out
    }

    /// payload AEAD 的 AAD：明文头里**轮换绝不重写的稳定前缀**——即魔数、`format_version`、
    /// source（**不含** `kdf_params` / `salt` / `nonce_i` / `wrapped_data_key`，这些是包裹槽
    /// 随 rekey / rotate-kdf 被重写的材料，见 `AAD_PREFIX_LEN` 注释）。
    ///
    /// 设计权威以详细设计第八部分 / 5.4 为准：§8 F-9 要求 rekey / rotate-kdf 重写包裹槽后
    /// payload 密文一字不动、同一 data-key 仍解原 payload；5.4「换 KDF 只需重包裹 data-key、
    /// 无需整体重加密」。要让 rotate-kdf 换 `kdf_params` / `salt` 后原 payload 仍可解，AAD 必须
    /// **排除**这些被重写字段。L-2「头部任一字节篡改即拒锁」不被削弱：被排除字段的篡改改由
    /// 派生主密钥 / 包裹槽 AEAD 拦截（fail-closed，仅错误码不同），见 `AAD_PREFIX_LEN`。
    ///
    /// 本方法据结构重新序列化前缀，供写入 / 测试夹具计算加密期 AAD；解锁期则用
    /// `aad_slice` 直接取**原始字节**前缀（保证 source 字节即便被改成未识别值也参与校验）。
    pub fn aad_bytes(&self) -> Vec<u8> {
        let header = self.header_bytes();
        header
            .get(..AAD_PREFIX_LEN)
            .map_or(header.clone(), <[u8]>::to_vec)
    }
}

/// 从**原始 vault 字节**取 AAD 前缀切片（魔数 + `format_version` + source，不含轮换重写材料）。
///
/// 用于解锁期 payload AEAD 的 AAD：直接取原始字节使 source 字节即便被改成未识别值也参与
/// 校验（篡改 → `AadMismatch`，L-2），无需先把字节解读成合法枚举。前缀长度恒为
/// `AAD_PREFIX_LEN`（固定 7B）；`kdf_params` / `salt` / 包裹材料排除在 AAD 外（F-9，见
/// `AAD_PREFIX_LEN` 注释）。
pub fn aad_slice(bytes: &[u8]) -> Result<&[u8], UnlockError> {
    bytes
        .get(..AAD_PREFIX_LEN)
        .ok_or(UnlockError::PayloadDecryptFailed)
}

/// 来源判别的字节编码（单字节，确定）。
fn source_to_u8(s: SlotSource) -> u8 {
    match s {
        SlotSource::Passphrase => 0,
        SlotSource::KeyFile => 1,
        SlotSource::OsKeychain => 2,
        SlotSource::SystemdCred => 3,
    }
}

/// 字节→来源判别。未识别值回退为 `KeyFile`（直接持有型）——`decode` 不据此拒锁，
/// 篡改由原始字节 AAD 校验拦截（L-2）。
fn source_from_u8(b: u8) -> SlotSource {
    match b {
        0 => SlotSource::Passphrase,
        2 => SlotSource::OsKeychain,
        3 => SlotSource::SystemdCred,
        _ => SlotSource::KeyFile,
    }
}

/// 一个占位空槽（仅 `header_bytes` 在头损坏时用于退化编码；解码端 fail-closed）。
fn empty_slot() -> Slot {
    Slot {
        source: SlotSource::KeyFile,
        kdf_params: None,
        salt: None,
        nonce_i: [0u8; NONCE_LEN],
        wrapped_data_key: Vec::new(),
    }
}

/// argon2id 参数的固定 13 字节编码：1 字节存在标志 + 12 字节 (m,t,p) LE。
/// 无 KDF 时存在标志 0 且参数区全零——**固定长度**，使解码不因标志位翻转而错位。
/// KDF 区不在 payload AAD 内（rotate-kdf 重写它，见 `AAD_PREFIX_LEN`）：篡改 KDF 参数使
/// passphrase 派生主密钥不同 → 开包裹槽失败（`PayloadDecryptFailed`），仍 fail-closed（L-2）。
fn put_kdf(slot: &Slot, out: &mut Vec<u8>) {
    match &slot.kdf_params {
        Some(p) => {
            out.push(1);
            out.extend_from_slice(&p.m_cost.to_le_bytes());
            out.extend_from_slice(&p.t_cost.to_le_bytes());
            out.extend_from_slice(&p.p_cost.to_le_bytes());
        }
        None => {
            out.push(0);
            out.extend_from_slice(&[0u8; 12]);
        }
    }
}

/// 把单包裹槽追加到字节缓冲（**固定布局**，与 `decode_slot` 互逆）：
/// source(1) + KDF 区(13，含存在标志) + salt 存在标志(1) + salt_len(4) + salt +
/// nonce_i(24) + wrapped_len(4) + wrapped_data_key。
///
/// payload AAD 仅取 source 之前（含 source）的前缀（`AAD_PREFIX_LEN`）；`kdf_params` / `salt` /
/// `nonce_i` / `wrapped_data_key` 都是 rekey / rotate-kdf 会重写的包裹槽材料，故不在 AAD 内（F-9）。
fn encode_slot(slot: &Slot, out: &mut Vec<u8>) {
    out.push(source_to_u8(slot.source));
    put_kdf(slot, out);
    let salt = slot.salt.as_deref().unwrap_or(&[]);
    out.push(u8::from(slot.salt.is_some()));
    out.extend_from_slice(&len_u32(salt.len()).to_le_bytes());
    out.extend_from_slice(salt);
    out.extend_from_slice(&slot.nonce_i);
    out.extend_from_slice(&len_u32(slot.wrapped_data_key.len()).to_le_bytes());
    out.extend_from_slice(&slot.wrapped_data_key);
}

/// 从游标解出单包裹槽（**固定布局**，与 `encode_slot` 互逆）。任一截断即 fail-closed。
///
/// KDF / salt 元数据字节不在 payload AAD 内（rekey / rotate-kdf 重写它们，见
/// `AAD_PREFIX_LEN`），故 AAD 校验无法拦其篡改。为不在直接持有型来源
/// （KeyFile/OsKeychain/SystemdCred，无 KDF / salt）上留下「这些字节被解码后丢弃、
/// 篡改不被察觉」的 L-2 缺口，本函数把 KDF / salt 元数据与 **source 字节**（**在** AAD 内）
/// 强绑定并做规范性校验——任一不一致即 `AadMismatch`，使明文头**每个字节**都被解锁路径消费、
/// 篡改即拒锁（L-2 对所有来源成立），同时不破坏 F-9（passphrase 槽 has_kdf/has_salt 恒为 1，
/// 直接持有型恒为 0，rekey / rotate-kdf 重写后仍满足此规范）。
fn decode_slot(r: &mut Reader<'_>) -> Result<Slot, UnlockError> {
    let source = source_from_u8(r.take_u8().ok_or(UnlockError::PayloadDecryptFailed)?);
    let source_has_kdf = matches!(source, SlotSource::Passphrase);

    // KDF 区固定 13 字节：存在标志 + (m,t,p) LE。
    let has_kdf = r.take_u8().ok_or(UnlockError::PayloadDecryptFailed)?;
    let m_cost = r.take_u32().ok_or(UnlockError::PayloadDecryptFailed)?;
    let t_cost = r.take_u32().ok_or(UnlockError::PayloadDecryptFailed)?;
    let p_cost = r.take_u32().ok_or(UnlockError::PayloadDecryptFailed)?;
    // has_kdf 必为规范 0/1，且与 source 一致（仅 passphrase 带 KDF）；不一致 → 篡改 → 拒锁。
    if has_kdf > 1 || (has_kdf == 1) != source_has_kdf {
        return Err(UnlockError::AadMismatch);
    }
    let kdf_params = if has_kdf == 1 {
        Some(KdfParams {
            m_cost,
            t_cost,
            p_cost,
        })
    } else {
        // 无 KDF 时参数区必为规范全零；任一参数字节被篡改即拒锁（堵直接持有型来源的 L-2 缺口）。
        if m_cost != 0 || t_cost != 0 || p_cost != 0 {
            return Err(UnlockError::AadMismatch);
        }
        None
    };

    let has_salt = r.take_u8().ok_or(UnlockError::PayloadDecryptFailed)?;
    let salt_len = r.take_u32().ok_or(UnlockError::PayloadDecryptFailed)? as usize;
    let salt_bytes = r.take(salt_len).ok_or(UnlockError::PayloadDecryptFailed)?;
    // has_salt 必为规范 0/1，且与 source 一致（仅 passphrase 带 salt）；无 salt 时长度必为 0。
    if has_salt > 1 || (has_salt == 1) != source_has_kdf || (has_salt == 0 && salt_len != 0) {
        return Err(UnlockError::AadMismatch);
    }
    let salt = if has_salt == 1 {
        Some(salt_bytes.to_vec())
    } else {
        None
    };

    let nonce_i = r.take_nonce().ok_or(UnlockError::PayloadDecryptFailed)?;
    let wrapped_len = r.take_u32().ok_or(UnlockError::PayloadDecryptFailed)? as usize;
    let wrapped_data_key = r
        .take(wrapped_len)
        .ok_or(UnlockError::PayloadDecryptFailed)?
        .to_vec();

    Ok(Slot {
        source,
        kdf_params,
        salt,
        nonce_i,
        wrapped_data_key,
    })
}

/// 长度的 u32 编码（饱和到 `u32::MAX`，避免 `as` 截断告警与潜在溢出）。
fn len_u32(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// 只读字节游标：所有取值用 `get` / 切片 `get`，绝不索引越界、绝不 panic（B-6）。
struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    /// 取 `n` 字节切片并前移游标；不足返回 `None`。
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.pos.checked_add(n)?;
        let slice = self.bytes.get(self.pos..end)?;
        self.pos = end;
        Some(slice)
    }

    fn take_u8(&mut self) -> Option<u8> {
        self.take(1).and_then(|s| s.first().copied())
    }

    fn take_u32(&mut self) -> Option<u32> {
        let s = self.take(4)?;
        let arr: [u8; 4] = s.try_into().ok()?;
        Some(u32::from_le_bytes(arr))
    }

    fn take_nonce(&mut self) -> Option<[u8; NONCE_LEN]> {
        let s = self.take(NONCE_LEN)?;
        let arr: [u8; NONCE_LEN] = s.try_into().ok()?;
        Some(arr)
    }

    /// 取剩余全部字节（游标推到末尾）。
    fn take_rest(&mut self) -> &'a [u8] {
        let rest = self.bytes.get(self.pos..).unwrap_or(&[]);
        self.pos = self.bytes.len();
        rest
    }
}
