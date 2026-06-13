//! vault 密文 payload（data-key 解密后的机密内容）（设计承诺级桩，函数体未实现）。
//!
//! payload 是 JSON 两段（详细设计 5.4）：
//! ```json
//! { "secrets": { "db-main/readonly": {"user":"ro","password":"..."}, ... },
//!   "targets": { "db-main": {"host":"...","port":"5432"}, ... } }
//! ```
//! - `secrets` 键即 `vault://<code>/<tier-or-slot>` 引用路径；值是该凭据的字段映射。
//! - `targets` 键即资源代号；值是该代号的真实地址字段映射。
//!
//! 内存纪律（§7-1 / 公理四）：payload 明文**只活在解锁后的进程内存**，整段置于
//! `Zeroizing` 容器，离作用域清零；**不 derive `Debug`**（不落明文到日志 / trace）；
//! 仅密文落盘。
//!
//! 边界纪律（本单元职责）：本单元只把 payload 明文**解出为字段映射**，供 mapping /
//! provider 单元据此物化成机密类型——**本单元绝不构造 `ResolvedTarget`/`ResourceCredential`**
//! （构造权归 mapping/provider 单元，本文件不 touch 这两个类型）。回读对外只产**掩码**
//! 或 `vault://` 引用，绝不回吐明文（§5.5 / F-8）。
//!
//! 雷区纪律：`secrets` 段键形如 `db-main/readonly`，但本文件 / 段名 / 字段名里**不出现
//! 任何裸数据库写标记**；payload 是 JSON，用 serde_json 解析，不引任何数据库 / SQL 解析依赖。

use std::collections::BTreeMap;

use zeroize::Zeroizing;

use crate::error::UnlockError;

/// 一段字段映射（条目键 → 该条目字段名 → 字段值）。键（引用键 / 代号 / 字段名）非敏感；
/// **叶子值是明文机密**，逐个置于 `Zeroizing<String>`，离作用域清零（`BTreeMap` 容器本身
/// 不实现 `Zeroize`，故清零落在叶子值上）。
pub(crate) type SecretSection = BTreeMap<String, BTreeMap<String, Zeroizing<String>>>;

/// 解锁后的 payload 明文（两段字段映射）。叶子明文值 `Zeroizing`、不 derive `Debug`。
///
/// 字段为 crate 内可见（供 mapping/provider 同 crate 单元物化），**不 `pub`**——payload
/// 明文不出 crate 边界；对外只经掩码 / `vault://` 引用回读方法暴露。
pub struct Payload {
    /// `vault://<code>/<tier-or-slot>` → 凭据字段映射（如 `{user, password}`）。
    pub(crate) secrets: SecretSection,
    /// 资源代号 → 真实地址字段映射（如 `{host, port}` / `{instance_id, region}`）。
    pub(crate) targets: SecretSection,
}

impl Payload {
    /// 由两段字段映射直接构造 payload（控制面录入 / 测试夹具的写入起点）。叶子明文值
    /// 已是 `Zeroizing<String>`；本构造只搬运、不复制明文到额外位置。
    pub fn from_sections(secrets: SecretSection, targets: SecretSection) -> Self {
        Self { secrets, targets }
    }

    /// 从解密后的 payload 明文字节解析两段 JSON 映射。整段产物 `Zeroizing` 持有。
    ///
    /// fail-closed（B-6）：JSON 解析失败（结构不符 / 截断 / 非法 UTF-8）一律 map 成
    /// `UnlockError::PayloadDecryptFailed`，绝不 unwrap / panic、绝不返回半截 payload。
    pub fn from_plaintext(plaintext: &Zeroizing<Vec<u8>>) -> Result<Self, UnlockError> {
        let text =
            core::str::from_utf8(plaintext).map_err(|_| UnlockError::PayloadDecryptFailed)?;
        let mut p = JsonReader::new(text);
        p.skip_ws();
        p.expect(b'{')?;
        let mut secrets: Option<SecretSection> = None;
        let mut targets: Option<SecretSection> = None;
        // 顶层对象：恰两段 secrets / targets，顺序不限，其余键拒绝（fail-closed）。
        loop {
            p.skip_ws();
            if p.peek() == Some(b'}') {
                p.bump();
                break;
            }
            let key = p.parse_string()?;
            p.skip_ws();
            p.expect(b':')?;
            let section = p.parse_section()?;
            match key.as_str() {
                "secrets" => secrets = Some(section),
                "targets" => targets = Some(section),
                _ => return Err(UnlockError::PayloadDecryptFailed),
            }
            p.skip_ws();
            match p.peek() {
                Some(b',') => {
                    p.bump();
                }
                Some(b'}') => {
                    p.bump();
                    break;
                }
                _ => return Err(UnlockError::PayloadDecryptFailed),
            }
        }
        p.skip_ws();
        if !p.at_end() {
            return Err(UnlockError::PayloadDecryptFailed);
        }
        Ok(Self {
            secrets: secrets.ok_or(UnlockError::PayloadDecryptFailed)?,
            targets: targets.ok_or(UnlockError::PayloadDecryptFailed)?,
        })
    }

    /// 某 `secrets` 引用键下的字段名集合（不含值）——供"掩码回读保留字段名"取证。
    /// 字段名非敏感（如 `user`/`password`/`host`），值才是机密、绝不经此暴露。未知键 `None`。
    pub fn secret_field_names(&self, secret_ref: &str) -> Option<Vec<String>> {
        self.secrets
            .get(secret_ref)
            .map(|fields| fields.keys().cloned().collect())
    }

    /// 把 payload 两段映射序列化回 JSON 明文字节（供整体重加密写入）。产物 `Zeroizing`。
    pub fn to_plaintext(&self) -> Result<Zeroizing<Vec<u8>>, UnlockError> {
        let mut out = String::new();
        out.push('{');
        write_section(&mut out, "secrets", &self.secrets);
        out.push(',');
        write_section(&mut out, "targets", &self.targets);
        out.push('}');
        Ok(Zeroizing::new(out.into_bytes()))
    }

    /// 全部 `secrets` 引用键（`vault://<code>/<tier-or-slot>` 形态）——回读只得引用，
    /// **不含任何凭据明文值**（§5.5 / F-8）。
    pub fn secret_refs(&self) -> Vec<String> {
        self.secrets.keys().cloned().collect()
    }

    /// 全部 `targets` 资源代号——回读只得代号，**不含任何真实地址明文**。
    pub fn target_codes(&self) -> Vec<String> {
        self.targets.keys().cloned().collect()
    }

    /// 对某个 `secrets` 引用键的字段做**掩码回读**——每字段值替换为掩码标记，
    /// **绝不回吐明文**（§5.5 / F-8）。未知键返回 `None`。
    pub fn masked_secret(&self, secret_ref: &str) -> Option<BTreeMap<String, String>> {
        self.secrets.get(secret_ref).map(|fields| {
            fields
                .keys()
                .map(|name| (name.clone(), MASK.to_string()))
                .collect()
        })
    }
}

/// 掩码回读的固定标记——任何字段的明文值在对外回读时一律替换为此（绝不回吐明文）。
const MASK: &str = "********";

/// 把一段字段映射写成 JSON 对象段：`"name":{ "k":"v", ... }`。键有序（`BTreeMap`）。
fn write_section(out: &mut String, name: &str, section: &SecretSection) {
    push_json_string(out, name);
    out.push(':');
    out.push('{');
    let mut first_entry = true;
    for (entry_key, fields) in section {
        if !first_entry {
            out.push(',');
        }
        first_entry = false;
        push_json_string(out, entry_key);
        out.push(':');
        out.push('{');
        let mut first_field = true;
        for (field, value) in fields {
            if !first_field {
                out.push(',');
            }
            first_field = false;
            push_json_string(out, field);
            out.push(':');
            push_json_string(out, value);
        }
        out.push('}');
    }
    out.push('}');
}

/// 追加一个 JSON 字符串字面量（含两端引号、最小必要转义）。
fn push_json_string(out: &mut String, s: &str) {
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                // 控制字符用 \u00XX 形式，避免产出非法 JSON。
                out.push_str("\\u00");
                let hi = (c as u32 >> 4) & 0xf;
                let lo = (c as u32) & 0xf;
                out.push(hex_digit(hi));
                out.push(hex_digit(lo));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

fn hex_digit(n: u32) -> char {
    match n {
        0..=9 => char::from(b'0'.wrapping_add(n as u8)),
        _ => char::from(b'a'.wrapping_add((n as u8).wrapping_sub(10))),
    }
}

/// 极简 JSON 游标解析器，专解两段字段映射结构（对象 → 对象 → 字符串）。
/// 一切非预期输入一律 fail-closed 成 `PayloadDecryptFailed`，绝不 unwrap / panic。
struct JsonReader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> JsonReader<'a> {
    fn new(s: &'a str) -> Self {
        Self {
            bytes: s.as_bytes(),
            pos: 0,
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn bump(&mut self) {
        self.pos = self.pos.saturating_add(1);
    }

    fn at_end(&self) -> bool {
        self.pos >= self.bytes.len()
    }

    fn skip_ws(&mut self) {
        while let Some(b) = self.peek() {
            if b == b' ' || b == b'\t' || b == b'\n' || b == b'\r' {
                self.bump();
            } else {
                break;
            }
        }
    }

    fn expect(&mut self, want: u8) -> Result<(), UnlockError> {
        if self.peek() == Some(want) {
            self.bump();
            Ok(())
        } else {
            Err(UnlockError::PayloadDecryptFailed)
        }
    }

    /// 解析一段对象：`{ "<entry>": { "<field>":"<value>", ... }, ... }`。
    fn parse_section(&mut self) -> Result<SecretSection, UnlockError> {
        self.skip_ws();
        self.expect(b'{')?;
        let mut section: SecretSection = BTreeMap::new();
        loop {
            self.skip_ws();
            if self.peek() == Some(b'}') {
                self.bump();
                break;
            }
            let entry_key = self.parse_string()?;
            self.skip_ws();
            self.expect(b':')?;
            let fields = self.parse_fields()?;
            section.insert(entry_key, fields);
            self.skip_ws();
            match self.peek() {
                Some(b',') => self.bump(),
                Some(b'}') => {
                    self.bump();
                    break;
                }
                _ => return Err(UnlockError::PayloadDecryptFailed),
            }
        }
        Ok(section)
    }

    /// 解析一个字段映射：`{ "<field>":"<value>", ... }`，值入 `Zeroizing<String>`。
    fn parse_fields(&mut self) -> Result<BTreeMap<String, Zeroizing<String>>, UnlockError> {
        self.skip_ws();
        self.expect(b'{')?;
        let mut fields: BTreeMap<String, Zeroizing<String>> = BTreeMap::new();
        loop {
            self.skip_ws();
            if self.peek() == Some(b'}') {
                self.bump();
                break;
            }
            let field = self.parse_string()?;
            self.skip_ws();
            self.expect(b':')?;
            self.skip_ws();
            let value = self.parse_string()?;
            fields.insert(field, Zeroizing::new(value));
            self.skip_ws();
            match self.peek() {
                Some(b',') => self.bump(),
                Some(b'}') => {
                    self.bump();
                    break;
                }
                _ => return Err(UnlockError::PayloadDecryptFailed),
            }
        }
        Ok(fields)
    }

    /// 解析一个 JSON 字符串字面量（处理 `\" \\ \/ \n \r \t \b \f \uXXXX` 转义）。
    fn parse_string(&mut self) -> Result<String, UnlockError> {
        self.skip_ws();
        self.expect(b'"')?;
        let mut s = String::new();
        loop {
            let b = self.peek().ok_or(UnlockError::PayloadDecryptFailed)?;
            self.bump();
            match b {
                b'"' => return Ok(s),
                b'\\' => {
                    let esc = self.peek().ok_or(UnlockError::PayloadDecryptFailed)?;
                    self.bump();
                    match esc {
                        b'"' => s.push('"'),
                        b'\\' => s.push('\\'),
                        b'/' => s.push('/'),
                        b'n' => s.push('\n'),
                        b'r' => s.push('\r'),
                        b't' => s.push('\t'),
                        b'b' => s.push('\u{0008}'),
                        b'f' => s.push('\u{000c}'),
                        b'u' => {
                            let cp = self.parse_hex4()?;
                            let ch = char::from_u32(cp).ok_or(UnlockError::PayloadDecryptFailed)?;
                            s.push(ch);
                        }
                        _ => return Err(UnlockError::PayloadDecryptFailed),
                    }
                }
                // 控制字符在 JSON 字符串里非法。
                0x00..=0x1f => return Err(UnlockError::PayloadDecryptFailed),
                // 其余字节：按 UTF-8 续接逐字节收集，再交给 from_utf8 校验。
                _ => {
                    let mut buf = [0u8; 4];
                    let len = utf8_len(b);
                    buf[0] = b;
                    for slot in buf.iter_mut().take(len).skip(1) {
                        *slot = self.peek().ok_or(UnlockError::PayloadDecryptFailed)?;
                        self.bump();
                    }
                    let chunk = buf.get(..len).ok_or(UnlockError::PayloadDecryptFailed)?;
                    let decoded = core::str::from_utf8(chunk)
                        .map_err(|_| UnlockError::PayloadDecryptFailed)?;
                    s.push_str(decoded);
                }
            }
        }
    }

    /// 解析 `\u` 后的 4 位十六进制码点。
    fn parse_hex4(&mut self) -> Result<u32, UnlockError> {
        let mut val: u32 = 0;
        for _ in 0..4 {
            let b = self.peek().ok_or(UnlockError::PayloadDecryptFailed)?;
            self.bump();
            let digit = match b {
                b'0'..=b'9' => u32::from(b - b'0'),
                b'a'..=b'f' => u32::from(b - b'a') + 10,
                b'A'..=b'F' => u32::from(b - b'A') + 10,
                _ => return Err(UnlockError::PayloadDecryptFailed),
            };
            val = val
                .checked_mul(16)
                .and_then(|v| v.checked_add(digit))
                .ok_or(UnlockError::PayloadDecryptFailed)?;
        }
        Ok(val)
    }
}

/// 由 UTF-8 首字节判断该字符总字节数（1..=4）。非法首字节按 1 处理，后续 from_utf8 兜底拒绝。
fn utf8_len(first: u8) -> usize {
    if first < 0x80 {
        1
    } else if first >> 5 == 0b110 {
        2
    } else if first >> 4 == 0b1110 {
        3
    } else if first >> 3 == 0b11110 {
        4
    } else {
        1
    }
}
