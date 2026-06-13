//! 系统级擦除集 ScrubSet 的不透明 match-and-erase 句柄。
//!
//! 单向性（§5.4 / §7-5 / §8 L-12，详细设计 6.4 / 8.8）：句柄**只**暴露两个入口——
//! `scrub`（整段字节 → 已擦除字节）与 `scrub_stream`（流式分块脱敏，跨 chunk 边界安全）。
//! 句柄**不暴露任何枚举/读出/序列化匹配集的方法**：无 `iter`/`keys`/`len`/`contains`/
//! `as_slice`，类型层不可 `Serialize`（不 derive、不手写）、不可 `Clone`。句柄持有者
//! （daemon）即便有代码亦无法把集合内容读出（L-12）。
//!
//! 红线（§8 L-12）：脱敏是单向的——`scrub`/`scrub_stream` **无「放行」语义分支**，
//! 失败不退化为「未擦除直出」。匹配不到就原样过（黑名单诚实度边界，§5.4），但绝不
//! 因匹配/编码异常而 panic 或抛出致放行（B-6：无 unwrap/expect/panic/indexing_slicing）。
//!
//! 内存纪律（§7-1）：匹配集字段私有、明文匹配项与派生中间态全程 `Zeroizing`；命中段
//! 替换为固定掩码标记（本单元独立常量，勿跨单元改文件）；输出为已擦除字节，绝不在
//! 日志/错误中泄露原文。

use zeroize::Zeroizing;

/// 命中段替换的固定掩码标记（本单元独立常量；与 vault payload 的掩码风格同类但不共享，
/// 勿跨单元改文件）。任何命中的真实地址/凭据串在输出里一律替换为此。
pub const SCRUB_MASK: &[u8] = b"[REDACTED]";

/// 系统级擦除集 ScrubSet 的不透明 match-and-erase 句柄。
///
/// 内部持有由 `targets`/`secrets` 全段派生的多模式匹配集（预编译、私有、`Zeroizing` 中间态）。
/// **不 derive / 不手写 `Clone`、`Serialize`、`Debug`**——句柄内容不可枚举、不可序列化、
/// 不进任何输出路径（§7-5 / L-12）。对外只暴露下列两个入口。
pub struct ScrubSet {
    // 匹配集字段私有，类型层不可读出（无 getter / iter / len / contains）。
    // 手写多模式扫描器的预编译串集合（不引 aho-corasick / regex）；明文匹配项 Zeroizing。
    // 不变量：每个模式非空（空模式在构造期被丢弃），故匹配前进步长 >= 1。
    patterns: Zeroizing<Vec<Zeroizing<Vec<u8>>>>,
}

impl ScrubSet {
    /// 由构造路径（`build`）填入预编译模式集合，产出不透明句柄。
    ///
    /// crate 内可见：仅供同 crate 的 `build` 单元在 `from_payload` 里调用，对外不暴露。
    /// 入参模式集合已是去空、`Zeroizing` 持有的明文串；本构造只搬运、不复制明文到额外位置。
    pub(crate) fn from_patterns(patterns: Zeroizing<Vec<Zeroizing<Vec<u8>>>>) -> Self {
        Self { patterns }
    }

    /// 整段脱敏：对 `input` 字节流单遍线性扫描，命中段替换为 `SCRUB_MASK`，返回已擦除字节。
    ///
    /// 复杂度随字节长度线性、不随匹配项数放大（§3.1 脱敏热路径）。无「放行」分支：匹配不到
    /// 的字节原样保留，但绝不因异常而 panic 或外抛原文（§8 L-12、B-6）。
    pub fn scrub(&self, input: &[u8]) -> Vec<u8> {
        // 整段模式：全缓冲一次扫完，无未决尾部留存（`hold_prefix = false`）。
        let (out, _carry_from) = self.scan(input, false);
        out
    }

    /// 流式分块脱敏：对一个 chunk 脱敏并维护跨 chunk 滑动重叠窗口（`carry`），消除敏感串
    /// 恰好跨 chunk 边界被切开而逃逸匹配的分块逃逸（详细设计 6.4 流式脱敏模型）。
    ///
    /// `carry` 持有上一 chunk 未决尾部——其内容**只可能是某模式的一个严格前缀**（绝不含
    /// 完整命中），故流末把 `carry` 原样并入也绝不外泄完整敏感串。调用方按序喂入各 chunk，
    /// 返回本 chunk 可安全提交的已擦除字节。`carry` 不泄露已脱敏内容。无「放行」分支（§8 L-12）。
    pub fn scrub_stream(&self, chunk: &[u8], carry: &mut Vec<u8>) -> Vec<u8> {
        // 把上一 chunk 留存的未决尾部接在本 chunk 前，形成连续待扫描缓冲。
        let mut buf = Zeroizing::new(Vec::with_capacity(carry.len().saturating_add(chunk.len())));
        buf.extend_from_slice(carry);
        buf.extend_from_slice(chunk);

        // 流式：扫到末尾时，若尾部恰是某模式的严格前缀（可能跨界续到下一 chunk），留作 carry。
        let (out, carry_from) = self.scan(&buf, true);

        // 把留存点之后的原始字节作为新的 carry（仅可能是模式前缀，下一 chunk 再判）。
        carry.clear();
        if let Some(rest) = buf.get(carry_from..) {
            carry.extend_from_slice(rest);
        }
        out
    }

    /// 单遍线性多模式扫描器：从头逐字节推进，每个位置取该处**最长**命中模式，命中段写入
    /// `SCRUB_MASK` 并跳过整段，未命中字节原样写出。复杂度随字节长度线性（§3.1 热路径）。
    ///
    /// `hold_prefix`：流式模式置 `true`——扫到缓冲末尾时，若一段尾部字节恰是某模式的**严格
    /// 前缀**（可能在下一 chunk 续成完整命中），就在该处停住、把这段尾部交回调用方留作 carry
    /// （绝不提交、绝不当已脱敏放出）。整段模式置 `false`，扫到底。
    /// 返回 `(已擦除输出, 留存起点下标)`；非流式时留存起点 = 缓冲长度（无留存）。
    ///
    /// 无 panic / 无裸索引：全程用 `get(..)` 切片与迭代，越界一律按「无匹配」处理（fail-closed
    /// 不致放行——匹配不到原样过是诚实度边界，绝无「未擦除却当已脱敏」的分支）。
    fn scan(&self, buf: &[u8], hold_prefix: bool) -> (Vec<u8>, usize) {
        let mut out = Vec::with_capacity(buf.len());
        let mut pos = 0usize;
        let len = buf.len();
        while pos < len {
            if let Some(match_len) = self.match_at(buf, pos) {
                // 完整命中：整段擦除、跳过。命中始终提交（哪怕落在尾部）。
                out.extend_from_slice(SCRUB_MASK);
                pos = pos.saturating_add(match_len);
                continue;
            }
            // 无完整命中。流式模式下，若从此处到缓冲末尾恰是某模式的严格前缀，留作 carry。
            if hold_prefix && self.is_pending_prefix(buf, pos) {
                break;
            }
            if let Some(&b) = buf.get(pos) {
                out.push(b);
            }
            pos = pos.saturating_add(1);
        }
        (out, pos)
    }

    /// 流式判定：`buf[pos..]`（一直到缓冲末尾、非空）是否为某模式的**严格前缀**——即该段
    /// 字节与某模式逐字节相等且短于该模式（说明这是一个可能在下一 chunk 续全的未决匹配头）。
    /// 是则该段须留作 carry，不提交。纯比较、不分配。
    fn is_pending_prefix(&self, buf: &[u8], pos: usize) -> bool {
        let tail = match buf.get(pos..) {
            Some(t) if !t.is_empty() => t,
            _ => return false,
        };
        for pat in self.patterns.iter() {
            // 严格前缀：尾部短于模式，且是模式的起始片段。
            if tail.len() < pat.len() {
                if let Some(head) = pat.get(..tail.len()) {
                    if head == tail {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// 在 `buf` 的 `pos` 处尝试匹配任一模式；返回首个命中的字节长度（取最长命中以免漏掉
    /// 被短模式抢先截断的长敏感串）。无命中返回 `None`。纯比较、无副作用、不分配。
    fn match_at(&self, buf: &[u8], pos: usize) -> Option<usize> {
        let mut best: Option<usize> = None;
        for pat in self.patterns.iter() {
            let plen = pat.len();
            if plen == 0 {
                continue;
            }
            let end = pos.checked_add(plen)?;
            if let Some(window) = buf.get(pos..end) {
                if window == pat.as_slice() {
                    best = Some(match best {
                        Some(cur) if cur >= plen => cur,
                        _ => plen,
                    });
                }
            }
        }
        best
    }
}
