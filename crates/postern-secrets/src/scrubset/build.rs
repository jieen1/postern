//! 系统级擦除集 ScrubSet 的构造路径——由已解锁 `Payload` 派生 match-and-erase 句柄。
//!
//! 派生（§3 / §5.4，详细设计 6.4 / 8.8）：从 `payload.targets`（全部真实地址字符串/IP）
//! 与 `payload.secrets`（全部凭据值）叶子明文收集全部匹配项，连同其**常见编码形态**
//! （URL-encode / base64 / 连接串内嵌）一并纳入，预编译成**手写多模式匹配结构**（算法上
//! 属 Aho-Corasick 一类多串匹配；但 `aho-corasick` 不在本 crate 依赖白名单，必须手写、不引
//! 该 crate，亦不引 `regex`）。
//!
//! 内存纪律（§7-1）：匹配集与派生中间态全程 `Zeroizing`，离作用域清零；派生产物封装进
//! 不透明 `ScrubSet` 句柄，匹配项明文不出句柄、不进任何输出路径。
//!
//! 更新语义（§3 / §6.4）：句柄签发于解锁之后、更新于每次保险箱写入之后；本单元只提供
//! 「据新 `targets`/`secrets` 重新 build 新版本句柄」的**纯构造路径**——原子替换交付给
//! 内核是 daemon 侧职责，本单元只产句柄。

use zeroize::Zeroizing;

use crate::scrubset::handle::ScrubSet;
use crate::vault::payload::Payload;

impl ScrubSet {
    /// 由已解锁 `Payload` 的 `targets`/`secrets` 全段叶子明文派生一个单向 match-and-erase
    /// 句柄（纯内存构造、无 IO、不跑 KDF）。
    ///
    /// 覆盖面：`targets` 中全部真实地址字符串/IP、`secrets` 中全部凭据值，及其常见编码形态
    /// （URL-encode / base64 / 连接串内嵌）（§5.4）。诚实度是黑名单兜底——不承诺识别全部编码
    /// 变体（§5.4）。匹配项明文与派生中间态全程 `Zeroizing`；产物为不透明句柄，集合内容不可
    /// 读出（§7-5 / L-12）。
    pub fn from_payload(payload: &Payload) -> ScrubSet {
        let mut patterns: Zeroizing<Vec<Zeroizing<Vec<u8>>>> = Zeroizing::new(Vec::new());

        // 两段全部叶子明文（真实地址字段值 / 凭据字段值）逐个纳入匹配集，并附其常见编码形态。
        // 字段 `pub(crate)`，同 crate 直接读取；不构造 ResolvedTarget/ResourceCredential。
        for section in [&payload.targets, &payload.secrets] {
            for fields in section.values() {
                for value in fields.values() {
                    add_leaf_variants(&mut patterns, value);
                }
            }
        }

        ScrubSet::from_patterns(patterns)
    }
}

/// 把一个叶子明文值连同其常见编码形态加入匹配集（去空、去重）。
///
/// 形态（§5.4 常见编码兜底，非穷举——诚实度边界）：
/// 1. 原文字节本身；
/// 2. URL-encode（百分号转义保留集以外的字节）；
/// 3. 标准 base64（连接串内嵌 / 头部承载常见形态）。
fn add_leaf_variants(patterns: &mut Vec<Zeroizing<Vec<u8>>>, value: &str) {
    let raw = value.as_bytes();
    if raw.is_empty() {
        return;
    }
    push_unique(patterns, Zeroizing::new(raw.to_vec()));
    push_unique(patterns, url_encode(raw));
    push_unique(patterns, base64_standard(raw));
}

/// 去重追加：相同字节的模式只保留一份（避免匹配集冗余膨胀）。空模式丢弃。
fn push_unique(patterns: &mut Vec<Zeroizing<Vec<u8>>>, candidate: Zeroizing<Vec<u8>>) {
    if candidate.is_empty() {
        return;
    }
    if patterns
        .iter()
        .any(|p| p.as_slice() == candidate.as_slice())
    {
        return;
    }
    patterns.push(candidate);
}

/// 对字节串做 URL 百分号编码：未保留字符（`A-Z a-z 0-9 - _ . ~`）原样，其余转 `%XX`。
/// 产物 `Zeroizing` 持有（仍是敏感原文的一种编码形态）。
fn url_encode(raw: &[u8]) -> Zeroizing<Vec<u8>> {
    let mut out = Zeroizing::new(Vec::with_capacity(raw.len()));
    for &b in raw {
        if is_url_unreserved(b) {
            out.push(b);
        } else {
            out.push(b'%');
            out.push(hex_upper(b >> 4));
            out.push(hex_upper(b & 0x0f));
        }
    }
    out
}

/// URL 未保留字符判定（RFC 3986 unreserved 集）。
fn is_url_unreserved(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~')
}

/// 取低 4 位对应的大写十六进制字符。
fn hex_upper(nibble: u8) -> u8 {
    match nibble & 0x0f {
        n @ 0..=9 => b'0'.wrapping_add(n),
        n => b'A'.wrapping_add(n.wrapping_sub(10)),
    }
}

/// 标准 base64 编码（带 `=` 填充，`+`/`/` 字母表）。手写、不引依赖。
/// 产物 `Zeroizing` 持有。
fn base64_standard(raw: &[u8]) -> Zeroizing<Vec<u8>> {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Zeroizing::new(Vec::with_capacity(raw.len().saturating_add(3) / 3 * 4));
    let mut chunks = raw.chunks(3);
    for chunk in &mut chunks {
        let b0 = chunk.first().copied().unwrap_or(0);
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);
        let n = chunk.len();
        let triple = (u32::from(b0) << 16) | (u32::from(b1) << 8) | u32::from(b2);
        let i0 = ((triple >> 18) & 0x3f) as usize;
        let i1 = ((triple >> 12) & 0x3f) as usize;
        let i2 = ((triple >> 6) & 0x3f) as usize;
        let i3 = (triple & 0x3f) as usize;
        out.push(ALPHABET.get(i0).copied().unwrap_or(b'A'));
        out.push(ALPHABET.get(i1).copied().unwrap_or(b'A'));
        if n >= 2 {
            out.push(ALPHABET.get(i2).copied().unwrap_or(b'A'));
        } else {
            out.push(b'=');
        }
        if n >= 3 {
            out.push(ALPHABET.get(i3).copied().unwrap_or(b'A'));
        } else {
            out.push(b'=');
        }
    }
    out
}
