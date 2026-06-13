//! 出口脱敏器（`daemon::sanitize`）行为测试 —— core 的 `Sanitizer` + `StreamScrubber`
//! 在 daemon（出口执行器）侧的实现。
//!
//! 驱动方式：用机密面公开 API 构造**真实** `ScrubSet` 句柄（`Payload::from_plaintext`
//! → `ScrubSet::from_payload`），按值（`Arc`）交给 `DaemonSanitizer`。daemon 只持句柄、
//! 只调 `scrub`/`scrub_stream`，绝不在测试或实现里构造 ScrubSet、绝不枚举/序列化句柄。
//!
//! §8 逐条覆盖（见各 `// §8` 注释）：签名与 core 一致、ScrubSet 先于 MaskRule、跨 chunk
//! 边界敏感串被擦除、句柄只 match-and-erase 无枚举、流式缓冲有界（carry ≤ N-1）。
//!
//! 失败路径一等公民：畸形/空字节绝不 panic（fail-closed）；未命中即原样直出（L-4 诚实度，
//! 绝无「放行」语义退化）。
//!
//! 雷区纪律：本文件零 SQL 标记、零非-shells 的 `ConnOrigin` 字面、不构造机密类型。

use std::sync::Arc;

use zeroize::Zeroizing;

use postern_core::plugin::channel::RawResponse;
use postern_core::plugin::sanitize::{MaskRule, SanitizedResponse, Sanitizer, StreamScrubber};

use postern_daemon::sanitize::{DaemonSanitizer, DaemonStreamScrubber};

use postern_secrets::scrubset::{ScrubSet, SCRUB_MASK};
use postern_secrets::vault::payload::Payload;

// ───────────────────────────── 测试夹具 ─────────────────────────────

/// 唯一的 ScrubSet-覆盖凭据 token（全 unreserved 字符：原文/URL-encode 同形，18 字节）。
const SECRET_TOKEN: &str = "S3CR3T-TOKEN-VALUE";

/// 该 token 的标准 base64 编码形态（机密面 build 会一并纳入匹配集；24 字节，是最长模式）。
/// 由此 N = 最长模式上界 = 24，流式 carry 上界 = N-1 = 23。
const SECRET_TOKEN_B64: &str = "UzNDUjNULVRPS0VOLVZBTFVF";

/// 真实地址 token（也被 ScrubSet 覆盖；测「整段先过 ScrubSet」时用）。
const TARGET_HOST: &str = "10.0.0.5";

/// 由机密面公开 API 构造真实 ScrubSet 句柄 —— daemon 只接管，绝不在此构造。
///
/// 经 `Payload::from_plaintext`（解析两段 JSON 明文）→ `ScrubSet::from_payload`（派生匹配集，
/// 含原文/URL-encode/base64 形态）。键名非敏感，叶子值（token / host）即匹配项。
fn real_scrubset() -> Arc<ScrubSet> {
    let json = format!(
        r#"{{"secrets":{{"db/ro":{{"password":"{SECRET_TOKEN}"}}}},"targets":{{"db":{{"host":"{TARGET_HOST}"}}}}}}"#
    );
    let plaintext = Zeroizing::new(json.into_bytes());
    let payload = Payload::from_plaintext(&plaintext).expect("两段 JSON payload 应解析成功");
    Arc::new(ScrubSet::from_payload(&payload))
}

/// 装配出口脱敏器（持真实 ScrubSet 句柄）。
fn sanitizer() -> DaemonSanitizer {
    DaemonSanitizer::new(real_scrubset())
}

/// 一条 `mask_fields` 声明式规则：spec 形态与详细设计 5.2 一致（`{"fields":[...]}`）。
fn mask_rule(field: &str) -> MaskRule {
    MaskRule {
        field: field.to_string(),
        spec: format!(r#"{{"fields":["{field}"]}}"#),
    }
}

/// 子串是否在字节序列里出现（断言「敏感原文已不在输出」用）。
fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || needle.len() > haystack.len() {
        return false;
    }
    haystack.windows(needle.len()).any(|w| w == needle)
}

// ─────────────────────── §8：签名与 core 一致 ───────────────────────

/// §8：daemon Sanitizer 实现的签名与 core trait 一致 ——
/// `scrub(payload: RawResponse, declared: &[MaskRule]) -> SanitizedResponse`，
/// `scrub_stream(declared: &[MaskRule]) -> Box<dyn StreamScrubber>`。
/// 经 `&dyn Sanitizer` 调用即证明 DaemonSanitizer 满足 core trait 形状（dyn-safe）。
#[test]
fn impl_signature_matches_core_sanitizer_trait() {
    let s = sanitizer();
    let dyn_s: &dyn Sanitizer = &s;

    let declared: Vec<MaskRule> = Vec::new();
    let out: SanitizedResponse = dyn_s.scrub(
        RawResponse {
            payload: b"nothing-sensitive-here".to_vec(),
        },
        &declared,
    );
    // 无敏感串、无 mask：原样直出（L-4 诚实度——未命中即 verbatim，绝不丢弃/改写）。
    assert_eq!(
        out.payload, b"nothing-sensitive-here",
        "无命中无规则时应原样直出（诚实度边界）"
    );

    let mut stream: Box<dyn StreamScrubber> = dyn_s.scrub_stream(&declared);
    let mut got = stream.push(b"plain-bytes");
    got.extend_from_slice(&stream.finish());
    assert_eq!(got, b"plain-bytes", "无命中流式也应原样直出");
}

// ───────────── §8：ScrubSet 先于 MaskRule（小载荷 token→SCRUB_MASK）─────────────

/// §8：`scrub` 对小载荷先整段过 ScrubSet —— 含 ScrubSet-覆盖 token 的输出里，
/// 该 token 被替换为 `SCRUB_MASK`（`b"[REDACTED]"`），原文绝不残留。
#[test]
fn scrub_replaces_scrubset_token_with_mask() {
    let s = sanitizer();
    let payload = format!("prefix {SECRET_TOKEN} suffix").into_bytes();

    let out = s.scrub(RawResponse { payload }, &[]);

    let expected = format!("prefix {} suffix", String::from_utf8_lossy(SCRUB_MASK)).into_bytes();
    assert_eq!(
        out.payload, expected,
        "ScrubSet-覆盖 token 应整段替换为 SCRUB_MASK"
    );
    assert!(
        !contains(&out.payload, SECRET_TOKEN.as_bytes()),
        "凭据原文绝不残留在输出"
    );
}

/// §8：先 ScrubSet 后 MaskRule —— 用一个**两次序输出可逐字节区分**的载荷钉死次序不变量。
///
/// 区分原理（关键）：把一个 **ScrubSet-覆盖的真实地址**（host `10.0.0.5`）同时摆在 *JSON 键名*
/// 上，并对该键名声明 `mask_fields`。两路操作在该键值对上**真实交互**，输出随次序而异。
/// 正确次序（先 ScrubSet 后 MaskRule）下，ScrubSet 整段先扫，把键名 host 与值 token 各自就地
/// 替换为 `SCRUB_MASK`（**保留值两侧引号**）；随后 MaskRule 再去找键 `"10.0.0.5"` —— 已被擦成
/// `"[REDACTED]"`、原键名不复存在，故 MaskRule 对该字段**无从匹配**，值的引号得以保留，该键值
/// 对输出为 `"[REDACTED]":"[REDACTED]"`（**值带引号**）。
/// 反向次序（先 MaskRule 后 ScrubSet）下，MaskRule 先匹配到键 `"10.0.0.5"`，把其字符串值整体
/// （**连同两侧引号**）换成裸 `SCRUB_MASK`；ScrubSet 再擦键名，该键值对输出为
/// `"[REDACTED]":[REDACTED]`（**值不带引号**）。
/// 二者仅差「值是否仍带一对引号」这一可逐字节判定的结构特征 —— 据此钉死次序，反转实现即红。
/// 两路次序**都不泄露**任何 token/host/PII（断言一并守住安全面），区分点纯属结构，不是泄漏。
///
/// 同载荷另带一个普通 `email` 字段（值为明文 PII），保留「MaskRule 命名字段值消失」这条断言。
#[test]
fn scrub_applies_scrubset_before_mask_rule() {
    let s = sanitizer();
    // JSON 响应体：键名是 ScrubSet-覆盖 host、值是 ScrubSet-覆盖 token（两者皆系统级擦除目标）；
    // 另一字段 email 是要 mask 的明文 PII。对 host 键名与 email 同时声明 mask_fields。
    let payload = br#"{"10.0.0.5":"S3CR3T-TOKEN-VALUE","email":"alice@example.com"}"#.to_vec();

    let out = s.scrub(
        RawResponse { payload },
        &[mask_rule(TARGET_HOST), mask_rule("email")],
    );

    // (a) ScrubSet 已整段先跑：凭据 token 与真实地址原文都不在输出，命中段为 SCRUB_MASK。
    assert!(
        contains(&out.payload, SCRUB_MASK),
        "ScrubSet 应已把凭据/地址替换为 SCRUB_MASK"
    );
    assert!(
        !contains(&out.payload, SECRET_TOKEN.as_bytes()),
        "凭据原文绝不残留"
    );
    assert!(
        !contains(&out.payload, TARGET_HOST.as_bytes()),
        "真实地址原文绝不残留"
    );
    // (b) MaskRule 对普通 email 字段生效：被掩字段的原始明文值从输出消失。
    assert!(
        !contains(&out.payload, b"alice@example.com"),
        "mask_fields 命名字段的明文值应从输出消失"
    );
    // (c) **次序钉死（证伪反转的唯一可观察特征）**：正确次序下，host 键名先被 ScrubSet 擦成
    // `"[REDACTED]"`，使 MaskRule 再也匹配不到该键 —— 故其值的两侧引号**得以保留**，输出里
    // 必出现「冒号 + 带引号的掩码值」`:"[REDACTED]"`。若实现反转为先 MaskRule，MaskRule 会先
    // 匹配到 `"10.0.0.5"` 并把值连引号一并替换为裸掩码 → 该子串不复存在，此断言转红。
    let quoted_masked_value = {
        let mut v = Vec::new();
        v.push(b':');
        v.push(b'"');
        v.extend_from_slice(SCRUB_MASK);
        v.push(b'"');
        v
    };
    assert!(
        contains(&out.payload, &quoted_masked_value),
        "次序不变量：ScrubSet 须先于 MaskRule —— host 键名被先擦除使 MaskRule 失配，\
         其值引号应保留为 `:\"[REDACTED]\"`；若实现先 MaskRule 则引号被剥离，此断言失败"
    );
    // 并显式排除反向次序的特征子串：值被剥引号的裸掩码 `:[REDACTED]`（紧跟逗号，非 email 那处）。
    // email 字段的值本就被 MaskRule 掩成裸 `[REDACTED]}`（结尾），故此处专挑 host 键值对的形态：
    // 裸掩码值后紧跟逗号 `,`。正确次序下不出现（host 值带引号），反向次序下出现。
    let bare_masked_then_comma = {
        let mut v = Vec::new();
        v.push(b':');
        v.extend_from_slice(SCRUB_MASK);
        v.push(b',');
        v
    };
    assert!(
        !contains(&out.payload, &bare_masked_then_comma),
        "反向次序特征（host 值被剥引号 `:[REDACTED],`）绝不应出现：MaskRule 不得先于 ScrubSet"
    );
}

/// §8（整段先过 ScrubSet）：真实地址同样被整段擦除（不限于凭据）。
#[test]
fn scrub_erases_target_address_too() {
    let s = sanitizer();
    let payload = format!("connect to {TARGET_HOST} now").into_bytes();

    let out = s.scrub(RawResponse { payload }, &[]);

    assert!(
        !contains(&out.payload, TARGET_HOST.as_bytes()),
        "真实地址应被 ScrubSet 擦除"
    );
    assert!(
        contains(&out.payload, SCRUB_MASK),
        "命中段应替换为 SCRUB_MASK"
    );
}

// ─────────────── §8：跨 chunk 边界的敏感串被擦除 ───────────────

/// §8：一个敏感串恰好被切在 chunk 边界两侧、经 `scrub_stream` 喂入后，在输出里被擦除
/// （上一 chunk 的 N-1 carry 尾巴参与下一 chunk 的匹配）。
#[test]
fn stream_erases_secret_split_across_chunk_boundary() {
    let s = sanitizer();
    let mut stream = s.scrub_stream(&[]);

    // 把 token 从中间切开：前半进第一个 chunk，后半进第二个 chunk。
    let (head, tail) = SECRET_TOKEN.split_at(7);
    let mut out = stream.push(format!("lead-{head}").as_bytes());
    out.extend_from_slice(&stream.push(format!("{tail}-trail").as_bytes()));
    out.extend_from_slice(&stream.finish());

    assert!(
        !contains(&out, SECRET_TOKEN.as_bytes()),
        "跨 chunk 边界的敏感串必须被擦除，绝不逃逸"
    );
    assert!(
        contains(&out, SCRUB_MASK),
        "跨界命中应在输出里以 SCRUB_MASK 呈现"
    );
    // 边界外的非敏感字节应保留（诚实度：只擦命中，不丢无辜字节）。
    assert!(contains(&out, b"lead-"), "前缀非敏感字节应保留");
    assert!(contains(&out, b"-trail"), "后缀非敏感字节应保留");
}

/// §8（跨界擦除）：base64 编码形态的敏感串跨 chunk 切开同样被擦除。
#[test]
fn stream_erases_base64_secret_split_across_boundary() {
    let s = sanitizer();
    let mut stream = s.scrub_stream(&[]);

    let (head, tail) = SECRET_TOKEN_B64.split_at(10);
    let mut out = stream.push(head.as_bytes());
    out.extend_from_slice(&stream.push(tail.as_bytes()));
    out.extend_from_slice(&stream.finish());

    assert!(
        !contains(&out, SECRET_TOKEN_B64.as_bytes()),
        "base64 形态敏感串跨界也必须被擦除"
    );
}

/// §8（跨界擦除）：敏感串被切成逐字节多个微 chunk，仍被擦除（carry 跨任意切分有效）。
#[test]
fn stream_erases_secret_fed_one_byte_per_chunk() {
    let s = sanitizer();
    let mut stream = s.scrub_stream(&[]);

    let mut out = Vec::new();
    let feed = format!("x{SECRET_TOKEN}y");
    for b in feed.as_bytes() {
        out.extend_from_slice(&stream.push(&[*b]));
    }
    out.extend_from_slice(&stream.finish());

    assert!(
        !contains(&out, SECRET_TOKEN.as_bytes()),
        "逐字节喂入的敏感串必须被擦除"
    );
    assert!(contains(&out, b"x"), "前导非敏感字节保留");
    assert!(contains(&out, b"y"), "尾随非敏感字节保留");
}

// ─────────────── §8：流式缓冲有界（carry ≤ N-1）───────────────

/// §8：流式缓冲有界 —— 任一时刻被扣留（未提交）的字节数不超过最长模式长度 N。
/// 喂入一大段**纯非匹配**字节（不是任何模式前缀），输出应即时跟上、几乎不扣留，
/// 证明 carry 不无界增长。
#[test]
fn stream_buffer_is_bounded_not_unbounded() {
    const LONGEST_PATTERN_N: usize = SECRET_TOKEN_B64.len(); // 24，最长模式上界

    let s = sanitizer();
    let mut stream = s.scrub_stream(&[]);

    // 纯非匹配填充（句点既非凭据/地址原文，也非任一模式前缀）。
    let filler = vec![b'.'; 100_000];

    let mut emitted = 0usize;
    let mut fed = 0usize;
    // 分块喂入，每步断言「已喂 - 已出」（即被扣留量）不超过 N。
    for chunk in filler.chunks(997) {
        fed += chunk.len();
        emitted += stream.push(chunk).len();
        assert!(
            fed - emitted <= LONGEST_PATTERN_N,
            "流式扣留量必须有界（≤N={LONGEST_PATTERN_N}）；已喂 {fed} 已出 {emitted}"
        );
    }
    emitted += stream.finish().len();
    assert_eq!(emitted, fed, "流末收尾后输出总量应等于输入总量（无丢无吞）");
}

/// §8（缓冲有界，压力面）：持续喂入「某模式严格前缀、永不补全」的字节，
/// carry 始终被钳在 ≤ N-1，绝不随输入长度线性膨胀。
#[test]
fn stream_carry_stays_bounded_under_perpetual_prefix() {
    const N_MINUS_1: usize = SECRET_TOKEN_B64.len() - 1; // 23 = carry 上界

    let s = sanitizer();
    let mut stream = s.scrub_stream(&[]);

    // base64 模式的一个严格前缀（短于 24，必为未决前缀，会进 carry 但永不补全）。
    let prefix = &SECRET_TOKEN_B64.as_bytes()[..N_MINUS_1];

    let mut fed = 0usize;
    let mut emitted = 0usize;
    // 反复喂同一前缀很多遍；任一时刻扣留量 ≤ N-1。
    for _ in 0..5_000 {
        fed += prefix.len();
        emitted += stream.push(prefix).len();
        assert!(
            fed - emitted <= N_MINUS_1,
            "永不补全的前缀流里 carry 必须钳在 ≤N-1={N_MINUS_1}；扣留 {}",
            fed - emitted
        );
    }
}

// ─────────────── §8：句柄只 match-and-erase，无枚举/序列化 ───────────────

/// §8：本模块持有的 ScrubSet 句柄只暴露 match-and-erase ——
/// `DaemonSanitizer`/`DaemonStreamScrubber` 不向外提供任何枚举/读出/序列化匹配集的入口。
/// 此处以「输出里只见 SCRUB_MASK、绝不见任何匹配项明文」从行为面佐证句柄不外泄内容；
/// 类型层「无 iter/len/Serialize」由编译期保证（见 type_level_notes，扫描器/审查另核）。
#[test]
fn handle_only_matches_and_erases_never_leaks_patterns() {
    let s = sanitizer();
    // 一段完全不含任何敏感串的输入：输出绝不凭空出现匹配集里的任何 token
    // （句柄无法被反向枚举出内容）。
    let out = s.scrub(
        RawResponse {
            payload: b"hello world, no secrets at all".to_vec(),
        },
        &[],
    );
    assert!(
        !contains(&out.payload, SECRET_TOKEN.as_bytes()),
        "句柄绝不把匹配项明文写进输出（不可枚举/读出）"
    );
    assert!(
        !contains(&out.payload, TARGET_HOST.as_bytes()),
        "句柄绝不把真实地址写进输出"
    );
    assert_eq!(
        out.payload, b"hello world, no secrets at all",
        "无命中输入应字节级原样直出"
    );
}

// ─────────────── 失败路径：畸形/空字节 fail-closed 不 panic ───────────────

/// fail-closed：空载荷与非法 UTF-8 字节经 `scrub` 绝不 panic（出口依赖永不崩，公理二）。
#[test]
fn scrub_never_panics_on_empty_or_malformed_bytes() {
    let s = sanitizer();

    let empty = s.scrub(
        RawResponse {
            payload: Vec::new(),
        },
        &[],
    );
    assert_eq!(empty.payload, b"", "空载荷脱敏仍是空，且不 panic");

    // 非法 UTF-8（孤立续字节 / 截断多字节序列）—— 仍按字节脱敏，不崩。
    let malformed = vec![0xFF, 0xFE, 0x80, 0x00, 0xC3, 0x28];
    let out = s.scrub(
        RawResponse {
            payload: malformed.clone(),
        },
        &[],
    );
    assert!(
        !out.payload.is_empty() || malformed.is_empty(),
        "畸形字节脱敏不 panic（fail-closed，永不崩）"
    );
}

/// fail-closed：流式脱敏对空 chunk 与畸形字节绝不 panic；`finish` 幂等收尾。
#[test]
fn stream_never_panics_on_empty_chunk_and_finish() {
    let s = sanitizer();
    let mut stream: Box<dyn StreamScrubber> = s.scrub_stream(&[]);

    let _ = stream.push(b""); // 空 chunk
    let _ = stream.push(&[0xFF, 0x00, 0xC3]); // 畸形字节
    let tail = stream.finish();
    let _ = tail; // 仅断言不 panic 即可达成
}

// ─────────────── 用得到 DaemonStreamScrubber 命名（类型存在性） ───────────────

/// `DaemonStreamScrubber` 是 `scrub_stream` 的具体回类型；此处确保该公开类型名被引用到，
/// 并经 `Box<dyn StreamScrubber>` 抽象使用（具体类型对调用方不可见但必须存在）。
#[test]
fn daemon_stream_scrubber_type_is_exposed() {
    fn assert_is_stream_scrubber<T: StreamScrubber>() {}
    assert_is_stream_scrubber::<DaemonStreamScrubber>();
}
