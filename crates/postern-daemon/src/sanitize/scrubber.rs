//! 出口脱敏器：内核出口处对出站载荷施加 ScrubSet 全段擦除 + 声明式 MaskRule。
//!
//! 用于两处：数据面出口统一脱敏（步骤 [9]），以及连接归池前的会话净化。daemon 是
//! 出口执行器：它**持有**机密面签发的不透明 `ScrubSet` 句柄（`postern_secrets::scrubset::
//! ScrubSet`，只 match-and-erase、不可枚举/序列化），外加来自 `grant_constraints` 的声明式
//! `MaskRule` 列表，按「先 ScrubSet 整段、后 MaskRule」的次序施加。
//!
//! 单向纪律（L-4 诚实度）：脱敏是单向的——命中即擦除，未命中即原样直出（match-failure
//! 不退化为「放行」或「丢弃」）。`ScrubSet` 句柄只被 hold，**绝不在本模块枚举/序列化**；
//! 其构造（`from_payload`）只发生在机密面，daemon 不提供任何 `ScrubSet` 构造路径。
//!
//! fail-closed（公理二）：对畸形字节绝不 panic（本模块非 `src/kernel/` 下，但是内核出口
//! 依赖，保持永不崩）。流式脱敏维护一段长度有界（N-1，N=ScrubSet 最长模式上界）的 carry
//! 尾巴，使恰好跨 chunk 边界被切开的敏感串仍被擦除；缓冲有界、不无界增长。

use std::sync::Arc;

use postern_core::plugin::channel::RawResponse;
use postern_core::plugin::sanitize::{MaskRule, SanitizedResponse, Sanitizer, StreamScrubber};
use postern_secrets::scrubset::{ScrubSet, SCRUB_MASK};

/// 出口脱敏器（core `Sanitizer` 的 daemon 实现）。
///
/// 持有机密面签发的不透明 `ScrubSet` 句柄（`Arc` 共享给流式脱敏器；句柄不 `Clone`、
/// 不可枚举、不可序列化，只 match-and-erase）。`scrub` 对整段先过 ScrubSet 再过 MaskRule；
/// `scrub_stream` 产出一个持有 carry 尾巴的有界流式脱敏器。
pub struct DaemonSanitizer {
    /// 机密面签发的系统级擦除集句柄；只被持有与调用 `scrub`/`scrub_stream`，绝不枚举/序列化。
    scrubset: Arc<ScrubSet>,
}

impl DaemonSanitizer {
    /// 由机密面**已构造**的 `ScrubSet` 句柄装配出口脱敏器。daemon 只接管句柄，
    /// **绝不在此构造 ScrubSet**（构造权归 `postern_secrets::scrubset::ScrubSet::from_payload`）。
    pub fn new(scrubset: Arc<ScrubSet>) -> Self {
        Self { scrubset }
    }
}

impl Sanitizer for DaemonSanitizer {
    /// 整段脱敏：先对整个 payload 过一遍 ScrubSet（命中真实地址/凭据串 → `SCRUB_MASK`），
    /// 再按声明式 `MaskRule` 抹掉/掩码命名字段，产出可安全出网的 `SanitizedResponse`。
    fn scrub(&self, payload: RawResponse, declared: &[MaskRule]) -> SanitizedResponse {
        // 次序：先 ScrubSet 整段擦除（系统级、不可绕过），后声明式字段掩码。
        let scrubbed = self.scrubset.scrub(&payload.payload);
        let masked = mask_named_fields(scrubbed, &resolve_mask_fields(declared));
        SanitizedResponse { payload: masked }
    }

    /// 开启流式脱敏：返回一个持有 carry 尾巴的有界流式脱敏器；逐 chunk 处理，保留上一
    /// chunk 末尾不超过 N-1 字节参与下一 chunk 匹配，消除跨界逃逸。
    fn scrub_stream(&self, declared: &[MaskRule]) -> Box<dyn StreamScrubber> {
        Box::new(DaemonStreamScrubber {
            scrubset: Arc::clone(&self.scrubset),
            carry: Vec::new(),
            // core 的 `MaskRule` 不 `Clone`；流式器只需字段名集合，构造时一次性解析为拥有所有权
            // 的 `Vec<String>`，避免持有不可克隆的规则结构。
            mask_fields: resolve_mask_fields(declared),
        })
    }
}

/// 流式出口脱敏器（core `StreamScrubber` 的 daemon 实现）。
///
/// 持有共享的 `ScrubSet` 句柄、一段长度有界（N-1）的 carry 尾巴，以及声明式 `MaskRule`。
/// `push` 把本 chunk 接在 carry 之后过 ScrubSet 流式脱敏，提交可安全输出的字节、把可能
/// 跨界的尾部留作新 carry；`finish` 在流末把残余 carry 收尾。缓冲有界、不无界增长。
pub struct DaemonStreamScrubber {
    /// 共享的系统级擦除集句柄（与父 `DaemonSanitizer` 同一份；只 match-and-erase）。
    scrubset: Arc<ScrubSet>,
    /// 跨 chunk 滑动重叠窗口的留存尾巴——长度有界为 N-1（只可能是某模式的严格前缀）。
    carry: Vec<u8>,
    /// 声明式字段掩码的字段名集合（构造时由 `MaskRule` 列表解析定型；流式逐 chunk 适用）。
    mask_fields: Vec<String>,
}

impl StreamScrubber for DaemonStreamScrubber {
    /// 脱敏一个 chunk：把上一 chunk 留存的 carry 接在本 chunk 前过 ScrubSet 流式脱敏，
    /// 返回本 chunk 可安全提交的已擦除字节；可能跨界的尾部留作新 carry，不提交。
    fn push(&mut self, chunk: &[u8]) -> Vec<u8> {
        // ScrubSet 自持碳带语义：把本 chunk 接 carry 后扫描，提交已擦除前段，未决前缀回填 carry。
        let scrubbed = self.scrubset.scrub_stream(chunk, &mut self.carry);
        // 已提交字节再过声明式字段掩码（流式按 chunk 适用；命名字段值消失）。
        mask_named_fields(scrubbed, &self.mask_fields)
    }

    /// 流末收尾：把残余 carry（只可能是某模式的严格前缀，绝不含完整命中）收口并清空。
    fn finish(&mut self) -> Vec<u8> {
        // carry 只可能是某模式的严格前缀（绝无完整命中），原样收口即可——单向脱敏无放行分支。
        let tail = std::mem::take(&mut self.carry);
        mask_named_fields(tail, &self.mask_fields)
    }
}

/// 把声明式 `MaskRule` 列表解析为要掩码的字段名集合（去重保序）。
///
/// 每条规则的 spec 形态（详细设计 5.2）：`{"fields":["a","b"]}`。spec 解析失败/无 `fields`
/// 数组时，回退到该规则自身的 `field` 名（fail-closed：宁可仍尝试掩码也不放弃该字段）。
fn resolve_mask_fields(declared: &[MaskRule]) -> Vec<String> {
    let mut fields: Vec<String> = Vec::new();
    for rule in declared {
        for field in spec_fields(rule) {
            if !fields.contains(&field) {
                fields.push(field);
            }
        }
    }
    fields
}

/// 依次对每个命名字段把其 JSON 字符串值擦为 `SCRUB_MASK`。
///
/// 单向纪律：字段不在载荷里即原样直出，绝不退化为放行/丢弃。畸形字节按字节处理、绝不
/// panic（fail-closed）。
fn mask_named_fields(payload: Vec<u8>, fields: &[String]) -> Vec<u8> {
    let mut out = payload;
    for field in fields {
        out = mask_json_string_field(out, field.as_bytes());
    }
    out
}

/// 从一条 `MaskRule` 取出要掩码的字段名集合。
///
/// spec 形态（详细设计 5.2）：`{"fields":["a","b"]}`。spec 解析失败/无 `fields` 数组时，
/// 回退到该规则自身的 `field` 名（fail-closed：宁可仍尝试掩码也不放弃该字段）。
fn spec_fields(rule: &MaskRule) -> Vec<String> {
    match serde_json::from_str::<serde_json::Value>(&rule.spec) {
        Ok(value) => match value.get("fields").and_then(|f| f.as_array()) {
            Some(arr) => {
                let fields: Vec<String> = arr
                    .iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect();
                if fields.is_empty() {
                    vec![rule.field.clone()]
                } else {
                    fields
                }
            }
            None => vec![rule.field.clone()],
        },
        Err(_) => vec![rule.field.clone()],
    }
}

/// 在 JSON 文本字节里把 `"<field>"` 后紧跟的字符串值替换为 `SCRUB_MASK`。
///
/// 纯字节扫描、不假设载荷是合法 JSON（fail-closed：畸形即原样过、不 panic）。逐处查找
/// `"<field>"`，跳过其后的空白与冒号，若随后是一个 JSON 字符串字面（以 `"` 起、含转义、
/// 以 `"` 止），就把该字符串字面（含两侧引号）替换为掩码。未命中即原样保留。
fn mask_json_string_field(input: Vec<u8>, field: &[u8]) -> Vec<u8> {
    // 待匹配的键 token：`"field"`。
    let mut key = Vec::with_capacity(field.len().saturating_add(2));
    key.push(b'"');
    key.extend_from_slice(field);
    key.push(b'"');

    let mut out: Vec<u8> = Vec::with_capacity(input.len());
    let mut pos = 0usize;
    let len = input.len();
    while pos < len {
        // 尝试在 pos 处匹配键 token。
        if let Some(after_key) = match_key_at(&input, pos, &key) {
            // 跳过键名后的空白与一个冒号。
            if let Some(value_start) = skip_to_value(&input, after_key) {
                if let Some(value_end) = json_string_end(&input, value_start) {
                    // 命中：把键 token 原样保留、字符串值（含两侧引号）整体换为掩码。
                    out.extend_from_slice(&key);
                    // 保留键与值之间的原始分隔字节（空白 + 冒号），以免改动结构形态。
                    if let Some(sep) = input.get(after_key..value_start) {
                        out.extend_from_slice(sep);
                    }
                    out.extend_from_slice(SCRUB_MASK);
                    pos = value_end;
                    continue;
                }
            }
        }
        if let Some(&b) = input.get(pos) {
            out.push(b);
        }
        pos = pos.saturating_add(1);
    }
    out
}

/// 若 `input[pos..]` 以键 token `key` 起，返回键之后的下标；否则 `None`。纯比较、不分配。
fn match_key_at(input: &[u8], pos: usize, key: &[u8]) -> Option<usize> {
    let end = pos.checked_add(key.len())?;
    let window = input.get(pos..end)?;
    if window == key {
        Some(end)
    } else {
        None
    }
}

/// 从键之后的位置跳过空白，要求随后是一个 `:`，再跳过冒号后的空白，返回值起始下标。
/// 形态不符（无冒号）返回 `None`。
fn skip_to_value(input: &[u8], mut pos: usize) -> Option<usize> {
    while let Some(&b) = input.get(pos) {
        if b.is_ascii_whitespace() {
            pos = pos.checked_add(1)?;
        } else {
            break;
        }
    }
    if input.get(pos) != Some(&b':') {
        return None;
    }
    pos = pos.checked_add(1)?;
    while let Some(&b) = input.get(pos) {
        if b.is_ascii_whitespace() {
            pos = pos.checked_add(1)?;
        } else {
            break;
        }
    }
    Some(pos)
}

/// 若 `input[value_start]` 是 JSON 字符串字面的起始 `"`，返回该字符串字面结束 `"` 之后的
/// 下标（即整个 `"..."` 含两侧引号的右开端点）；非字符串值或未闭合返回 `None`。处理 `\"`
/// 与 `\\` 转义。纯扫描、不分配。
fn json_string_end(input: &[u8], value_start: usize) -> Option<usize> {
    if input.get(value_start) != Some(&b'"') {
        return None;
    }
    let mut pos = value_start.checked_add(1)?;
    while let Some(&b) = input.get(pos) {
        match b {
            b'\\' => {
                // 跳过转义符与被转义的那个字节。
                pos = pos.checked_add(2)?;
            }
            b'"' => {
                // 闭合引号：返回其后一位（右开端点）。
                return pos.checked_add(1);
            }
            _ => {
                pos = pos.checked_add(1)?;
            }
        }
    }
    None
}
