//! 系统级擦除集 ScrubSet 的行为测试（RED）。
//!
//! 被测对象：`postern_secrets::scrubset`——由已解锁 `Payload` 的 `targets`/`secrets`
//! 全段叶子明文派生的不透明 match-and-erase 句柄（`ScrubSet::from_payload`），及其
//! 仅有的两个入口 `scrub`（整段）与 `scrub_stream`（流式分块）。
//!
//! 覆盖 §8 条目（逐条加 `// §8 …` 注释）：
//! - F-6（ScrubSet 句柄签发）：由 `targets`/`secrets` 派生句柄；句柄类型层只暴露
//!   match-and-erase；含真实地址/凭据样本的字节流经句柄 → 命中段被擦除（行为观察）。
//! - L-12（ScrubSet 单向、内容不外泄）：含样本字节流 → 命中段被擦除、内容不进任何
//!   输出路径；脱敏无「放行」语义分支——匹配不到原样过（诚实度边界，§5.4），但绝不
//!   因匹配/编码异常 panic 或致放行；句柄不可枚举/序列化（类型层不变量见 type_level_notes）。
//!
//! 测试策略（§3.1）：纯内存构造——夹具直接 `Payload::from_sections` 构造，避开
//! vault::unlock / argon2 KDF（本单元测试不裹内存上限）。
//!
//! 雷区纪律：本文件含注释 / 字符串均**不含任何裸数据库写标记**；payload 夹具 `secrets`
//! 段键形如 `cache-main/readonly`、字段名 / 值均不触 SQL 标记；不构造 `ResolvedTarget`/
//! `ResourceCredential`（ScrubSet 单元只读 payload 叶子明文派生匹配集）。

use std::collections::BTreeMap;

use postern_secrets::scrubset::{ScrubSet, SCRUB_MASK};
use postern_secrets::vault::payload::Payload;
use zeroize::Zeroizing;

// ── 固定测试材料（可控、确定，不碰真实来源） ──────────────────────────────

/// payload `targets` 真实地址字段值样本（私网 IP 明文）——必须被 ScrubSet 命中擦除。
const TARGET_HOST: &str = "10.0.3.17";
/// payload `targets` 第二条真实地址样本（不同代号的另一私网 IP）。
const TARGET_HOST_2: &str = "192.168.41.9";
/// payload `secrets` 凭据值样本（账号口令明文）——必须被 ScrubSet 命中擦除。
const SECRET_VALUE: &str = "s3cr3t-ro-pw";
/// payload `secrets` 第二条凭据值样本（API token 明文）。
const SECRET_TOKEN: &str = "tok-9f2a7c41bd";

/// payload `secrets` 含 URL 保留字符的凭据值样本（口令含 `@ : / +`）——其 URL-encode 形态
/// 与原文字节**严格不同**，故能区分「只匹配原文」与「匹配编码变体」两种实现（§5.4 编码兜底）。
const ENC_SECRET: &str = "p@ss:w/rd+1";
/// payload `targets` 含 URL 保留字符的真实地址样本（连接串内嵌 host:port，含 `:`）——
/// 其 URL-encode / base64 形态均与原文不同，用于编码形态擦除取证。
const ENC_HOST: &str = "db.internal:5432";

/// 构造一个最小两段 payload：两条 `targets`（私网 IP）+ 两条 `secrets`（口令 / token）。
/// 字段名 / 值均不触任何裸数据库写标记；代号用 `cache-main` 一类非数据库写语义代号。
fn sample_payload() -> Payload {
    let mut targets: BTreeMap<String, BTreeMap<String, Zeroizing<String>>> = BTreeMap::new();
    let mut t1 = BTreeMap::new();
    t1.insert("host".to_string(), Zeroizing::new(TARGET_HOST.to_string()));
    t1.insert("port".to_string(), Zeroizing::new("6379".to_string()));
    targets.insert("cache-main".to_string(), t1);
    let mut t2 = BTreeMap::new();
    t2.insert(
        "host".to_string(),
        Zeroizing::new(TARGET_HOST_2.to_string()),
    );
    targets.insert("api-edge".to_string(), t2);

    let mut secrets: BTreeMap<String, BTreeMap<String, Zeroizing<String>>> = BTreeMap::new();
    let mut ro = BTreeMap::new();
    ro.insert("user".to_string(), Zeroizing::new("ro".to_string()));
    ro.insert(
        "password".to_string(),
        Zeroizing::new(SECRET_VALUE.to_string()),
    );
    secrets.insert("cache-main/readonly".to_string(), ro);
    let mut tok = BTreeMap::new();
    tok.insert(
        "token".to_string(),
        Zeroizing::new(SECRET_TOKEN.to_string()),
    );
    secrets.insert("api-edge/default".to_string(), tok);

    Payload::from_sections(secrets, targets)
}

/// 由样本 payload 派生一个 ScrubSet 句柄。
fn sample_scrubset() -> ScrubSet {
    ScrubSet::from_payload(&sample_payload())
}

/// 构造一个含 URL 保留字符叶子的 payload：`secrets` 一条口令 `ENC_SECRET`、`targets`
/// 一条 host `ENC_HOST`——二者的 URL-encode / base64 形态均与原文字节不同，专供编码形态擦除取证。
fn enc_variant_payload() -> Payload {
    let mut targets: BTreeMap<String, BTreeMap<String, Zeroizing<String>>> = BTreeMap::new();
    let mut t = BTreeMap::new();
    t.insert("host".to_string(), Zeroizing::new(ENC_HOST.to_string()));
    targets.insert("cache-main".to_string(), t);

    let mut secrets: BTreeMap<String, BTreeMap<String, Zeroizing<String>>> = BTreeMap::new();
    let mut ro = BTreeMap::new();
    ro.insert(
        "password".to_string(),
        Zeroizing::new(ENC_SECRET.to_string()),
    );
    secrets.insert("cache-main/readonly".to_string(), ro);

    Payload::from_sections(secrets, targets)
}

/// 由含编码保留字符的 payload 派生一个 ScrubSet 句柄（用于编码形态擦除取证）。
fn enc_variant_scrubset() -> ScrubSet {
    ScrubSet::from_payload(&enc_variant_payload())
}

/// 测试侧独立 URL 百分号编码（与被测实现各自实现，互为参照）：未保留字符
/// （`A-Z a-z 0-9 - _ . ~`）原样，其余转大写 `%XX`。用于构造「编码形态」输入字节。
fn url_encode(raw: &str) -> Vec<u8> {
    let mut out = Vec::new();
    for &b in raw.as_bytes() {
        let unreserved = b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~');
        if unreserved {
            out.push(b);
        } else {
            out.push(b'%');
            let hi = b >> 4;
            let lo = b & 0x0f;
            out.push(if hi < 10 { b'0' + hi } else { b'A' + (hi - 10) });
            out.push(if lo < 10 { b'0' + lo } else { b'A' + (lo - 10) });
        }
    }
    out
}

/// 测试侧独立标准 base64 编码（`+`/`/` 字母表、`=` 填充）：与被测实现各自实现、互为参照。
/// 用于构造「base64 形态」输入字节，钉住命中段被擦除的覆盖面含 base64 内嵌。
fn base64_standard(raw: &str) -> Vec<u8> {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes = raw.as_bytes();
    let mut out = Vec::new();
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[((triple >> 18) & 0x3f) as usize]);
        out.push(ALPHABET[((triple >> 12) & 0x3f) as usize]);
        if chunk.len() >= 2 {
            out.push(ALPHABET[((triple >> 6) & 0x3f) as usize]);
        } else {
            out.push(b'=');
        }
        if chunk.len() >= 3 {
            out.push(ALPHABET[(triple & 0x3f) as usize]);
        } else {
            out.push(b'=');
        }
    }
    out
}

/// 断言一段已擦除字节里不含某敏感子串（内容不进输出路径，L-12）。
fn assert_absent(haystack: &[u8], needle: &str) {
    let needle_bytes = needle.as_bytes();
    let found = haystack
        .windows(needle_bytes.len().max(1))
        .any(|w| w == needle_bytes);
    assert!(
        !found,
        "scrubbed output must NOT contain the sensitive substring {needle:?}"
    );
}

/// 断言一段字节里**含**某子串。
fn assert_present(haystack: &[u8], needle: &[u8]) {
    let found = haystack.windows(needle.len().max(1)).any(|w| w == needle);
    assert!(
        found,
        "output must contain the expected substring {:?}",
        String::from_utf8_lossy(needle)
    );
}

// ── F-6 / L-12：由 targets/secrets 派生句柄，命中段被擦除 ──────────────────

/// §8 F-6：含 `targets` 真实地址样本的字节流经句柄 → 该真实地址被擦除、不进输出。
#[test]
fn scrub_erases_target_real_address_from_targets_section() {
    let ss = sample_scrubset();
    let input = format!("upstream connect to {TARGET_HOST} ok").into_bytes();
    let out = ss.scrub(&input);
    // §8 F-6：命中的真实地址段被擦除。
    assert_absent(&out, TARGET_HOST);
    // §8 L-12（定向擦除取证）：擦除是定向的——只吃命中段，周边非敏感字节无损存活，
    // 命中处出现掩码。钉死「凡命中即吞掉周边」的退化实现（assert_absent 单边断言放不住）。
    assert_present(&out, b"upstream connect to ");
    assert_present(&out, b" ok");
    assert_present(&out, SCRUB_MASK);
}

/// §8 F-6：含 `secrets` 凭据值样本的字节流经句柄 → 该凭据值被擦除、不进输出。
#[test]
fn scrub_erases_credential_value_from_secrets_section() {
    let ss = sample_scrubset();
    let input = format!("auth header carried {SECRET_VALUE} downstream").into_bytes();
    let out = ss.scrub(&input);
    // §8 F-6：命中的凭据值段被擦除。
    assert_absent(&out, SECRET_VALUE);
    // §8 L-12（定向擦除取证）：周边非敏感字节无损存活、命中处出现掩码（非整段吞没）。
    assert_present(&out, b"auth header carried ");
    assert_present(&out, b" downstream");
    assert_present(&out, SCRUB_MASK);
}

/// §8 L-12：命中段替换为固定掩码标记 `SCRUB_MASK`（命中 → 有掩码出现，非原文）。
#[test]
fn scrub_replaces_matched_segment_with_fixed_mask_marker() {
    let ss = sample_scrubset();
    let input = format!("value={SECRET_VALUE};").into_bytes();
    let out = ss.scrub(&input);
    // §8 L-12：原文不在输出，且命中处出现固定掩码标记。
    assert_absent(&out, SECRET_VALUE);
    assert_present(&out, SCRUB_MASK);
}

/// §8 F-6：单遍扫描同时擦除来自不同段的多个匹配项（targets 地址 + secrets 凭据）。
#[test]
fn scrub_erases_multiple_distinct_matches_in_single_pass() {
    let ss = sample_scrubset();
    let input =
        format!("host={TARGET_HOST} token={SECRET_TOKEN} host2={TARGET_HOST_2}").into_bytes();
    let out = ss.scrub(&input);
    // §8 F-6：四类样本（两地址 + 一 token，及第二地址）全部命中擦除。
    assert_absent(&out, TARGET_HOST);
    assert_absent(&out, TARGET_HOST_2);
    assert_absent(&out, SECRET_TOKEN);
    // §8 L-12（定向擦除取证）：多匹配间的非敏感分隔文字逐一存活——擦除只吃命中段，
    // 不连周边一并吞掉。钉死「命中即裁掉大段字节」的退化实现。
    assert_present(&out, b"host=");
    assert_present(&out, b" token=");
    assert_present(&out, b" host2=");
    assert_present(&out, SCRUB_MASK);
}

/// §8 L-12：脱敏无「放行」误擦——不含任何敏感样本的字节流原样保留（非敏感字节不被吞）。
#[test]
fn scrub_passes_through_bytes_with_no_sensitive_match_verbatim() {
    let ss = sample_scrubset();
    let clean = b"GET /v1/items HTTP/1.1\r\ncontent-type: application/json\r\n\r\n{\"ok\":true}";
    let out = ss.scrub(clean);
    // §8 L-12：黑名单匹配不到 → 原样过，输出与输入逐字节相等（不放行=不误擦、不漏过明文）。
    assert_eq!(
        out.as_slice(),
        clean.as_slice(),
        "non-sensitive bytes must pass through unchanged"
    );
}

/// §8 L-12：擦除是单向 best-effort——空输入不 panic、不放行，返回空（无致放行异常分支）。
#[test]
fn scrub_on_empty_input_returns_empty_without_panic() {
    let ss = sample_scrubset();
    let out = ss.scrub(&[]);
    // §8 L-12：边界输入不 panic（B-6），空进空出。
    assert!(out.is_empty(), "scrubbing empty input yields empty output");
}

/// §8 L-12：含非 UTF-8 二进制字节且无敏感匹配 → 不 panic、原样过（编码异常不致放行/不抛原文）。
#[test]
fn scrub_on_binary_bytes_without_match_does_not_panic_and_preserves_bytes() {
    let ss = sample_scrubset();
    let input: Vec<u8> = vec![0x00, 0xff, 0x10, 0x80, 0xfe, 0x01, 0x7f, 0xc0];
    let out = ss.scrub(&input);
    // §8 L-12：二进制无匹配 → 原样保留，绝不因编码异常 panic 或致放行。
    assert_eq!(out, input, "unmatched binary bytes pass through unchanged");
}

/// §8 L-12：敏感样本出现在边缘（流首/流尾）亦被擦除（扫描覆盖整段，无边界漏匹配）。
#[test]
fn scrub_erases_match_at_stream_head_and_tail() {
    let ss = sample_scrubset();
    let head = format!("{SECRET_VALUE} trailing-text").into_bytes();
    let tail = format!("leading-text {TARGET_HOST}").into_bytes();
    let head_out = ss.scrub(&head);
    let tail_out = ss.scrub(&tail);
    // §8 L-12：首部命中样本被擦除，且其后的非敏感文字无损存活（定向擦除取证）。
    assert_absent(&head_out, SECRET_VALUE);
    assert_present(&head_out, b" trailing-text");
    assert_present(&head_out, SCRUB_MASK);
    // §8 L-12：尾部命中样本被擦除，且其前的非敏感文字无损存活（定向擦除取证）。
    assert_absent(&tail_out, TARGET_HOST);
    assert_present(&tail_out, b"leading-text ");
    assert_present(&tail_out, SCRUB_MASK);
}

/// §8 L-12：同一敏感样本多次出现 → 每次出现均被擦除（不止首个命中）。
#[test]
fn scrub_erases_all_occurrences_of_a_repeated_match() {
    let ss = sample_scrubset();
    let input = format!("{SECRET_TOKEN} | {SECRET_TOKEN} | {SECRET_TOKEN}").into_bytes();
    let out = ss.scrub(&input);
    // §8 L-12：重复命中样本全部擦除，输出不含任何一处原文。
    assert_absent(&out, SECRET_TOKEN);
    // §8 L-12（定向擦除取证）：命中间的非敏感分隔字节 ` | ` 存活，命中处出现掩码——
    // 三处命中被各自定向替换为掩码，而非把整段连周边一并吞没。
    assert_present(&out, b" | ");
    assert_present(&out, SCRUB_MASK);
}

// ── L-12：scrub_stream 流式分块脱敏，跨 chunk 边界安全 ─────────────────────

/// §8 L-12：scrub_stream 对完整落于单 chunk 的敏感样本擦除（流式入口同样单向擦除）。
#[test]
fn scrub_stream_erases_match_within_a_single_chunk() {
    let ss = sample_scrubset();
    let mut carry: Vec<u8> = Vec::new();
    let chunk = format!("payload {SECRET_VALUE} end").into_bytes();
    let mut out = ss.scrub_stream(&chunk, &mut carry);
    // 末尾 flush：把 carry 残留并入（最后一块无后继 chunk）。
    out.extend_from_slice(&carry);
    // §8 L-12：单 chunk 内命中样本被擦除。
    assert_absent(&out, SECRET_VALUE);
    // §8 L-12（定向擦除取证）：流式入口同样只吃命中段——周边非敏感字节无损存活、命中处出现掩码。
    assert_present(&out, b"payload ");
    assert_present(&out, b" end");
    assert_present(&out, SCRUB_MASK);
}

/// §8 L-12：敏感样本恰好跨 chunk 边界被切开 → 经 carry 滑动重叠窗口仍被擦除（不逃逸）。
#[test]
fn scrub_stream_erases_match_split_across_chunk_boundary() {
    let ss = sample_scrubset();
    let full = SECRET_VALUE.as_bytes();
    let cut = full.len() / 2;
    let (left, right) = full.split_at(cut);

    let mut carry: Vec<u8> = Vec::new();
    let mut emitted: Vec<u8> = Vec::new();
    // chunk1 = 前缀文字 + 敏感样本左半；chunk2 = 敏感样本右半 + 后缀文字。
    let mut c1 = b"begin ".to_vec();
    c1.extend_from_slice(left);
    let mut c2 = right.to_vec();
    c2.extend_from_slice(b" finish");

    emitted.extend_from_slice(&ss.scrub_stream(&c1, &mut carry));
    emitted.extend_from_slice(&ss.scrub_stream(&c2, &mut carry));
    // 末尾 flush carry。
    emitted.extend_from_slice(&carry);

    // §8 L-12：跨 chunk 边界切开的敏感样本不得逃逸——完整原文不在拼接后的输出里。
    assert_absent(&emitted, SECRET_VALUE);
    // §8 L-12（定向擦除取证）：跨界擦除亦是定向的——命中两侧的非敏感文字无损存活、出现掩码，
    // 而非把跨界附近字节一并吞没。
    assert_present(&emitted, b"begin ");
    assert_present(&emitted, b" finish");
    assert_present(&emitted, SCRUB_MASK);
}

/// §8 L-12：scrub_stream 对全无敏感匹配的连续 chunk 不误擦——拼接输出含原文非敏感内容。
#[test]
fn scrub_stream_passes_through_clean_chunks_without_spurious_erase() {
    let ss = sample_scrubset();
    let mut carry: Vec<u8> = Vec::new();
    let mut emitted: Vec<u8> = Vec::new();
    let c1 = b"HTTP/1.1 200 OK\r\ncontent-length: 13\r\n".to_vec();
    let c2 = b"\r\nhello world\n".to_vec();
    emitted.extend_from_slice(&ss.scrub_stream(&c1, &mut carry));
    emitted.extend_from_slice(&ss.scrub_stream(&c2, &mut carry));
    emitted.extend_from_slice(&carry);
    // §8 L-12：无敏感匹配 → 非敏感内容原样过（不放行=不误擦），可见文字保留。
    assert_present(&emitted, b"hello world");
    assert_present(&emitted, b"200 OK");
}

/// §8 F-6：据**新** targets/secrets 重新 build 的新版本句柄擦除新样本（更新语义=纯重构造）。
#[test]
fn rebuilt_scrubset_from_new_payload_erases_the_new_samples() {
    // 一个仅含新样本的 payload（模拟保险箱写入后据新内容重新派生新版本句柄）。
    const NEW_HOST: &str = "10.55.0.2";
    const NEW_SECRET: &str = "rotated-pw-7788";

    let mut targets: BTreeMap<String, BTreeMap<String, Zeroizing<String>>> = BTreeMap::new();
    let mut t = BTreeMap::new();
    t.insert("host".to_string(), Zeroizing::new(NEW_HOST.to_string()));
    targets.insert("cache-main".to_string(), t);

    let mut secrets: BTreeMap<String, BTreeMap<String, Zeroizing<String>>> = BTreeMap::new();
    let mut ro = BTreeMap::new();
    ro.insert(
        "password".to_string(),
        Zeroizing::new(NEW_SECRET.to_string()),
    );
    secrets.insert("cache-main/readonly".to_string(), ro);

    let ss = ScrubSet::from_payload(&Payload::from_sections(secrets, targets));
    let input = format!("conn {NEW_HOST} pw {NEW_SECRET}").into_bytes();
    let out = ss.scrub(&input);
    // §8 F-6：新版本句柄据新 targets/secrets 擦除新样本。
    assert_absent(&out, NEW_HOST);
    assert_absent(&out, NEW_SECRET);
}

/// §8 F-6：由**空** payload（无 targets / 无 secrets）派生的句柄不擦任何内容（无匹配项=全过）。
#[test]
fn scrubset_from_empty_payload_erases_nothing() {
    let empty = Payload::from_sections(BTreeMap::new(), BTreeMap::new());
    let ss = ScrubSet::from_payload(&empty);
    let input = b"arbitrary 10.0.3.17 text s3cr3t-ro-pw here".to_vec();
    let out = ss.scrub(&input);
    // §8 F-6：匹配集为空 → 字节流原样过（黑名单兜底诚实度，§5.4），不误擦、不 panic。
    assert_eq!(
        out, input,
        "empty match set must leave all bytes unchanged (honesty boundary)"
    );
}

// ── F-6 / §5.4：编码形态（URL-encode / base64）擦除取证 ─────────────────────
//
// 设计承诺（§5.4 / F-6）：ScrubSet 覆盖面含机密叶子的**常见编码形态**（URL-encode /
// base64 / 连接串内嵌），而非只擦原文裸串。以下用例把机密的**编码形态字节**喂进 scrub
// 并断言被擦除——这些断言只有当 build.rs 为每个叶子额外注入 url_encode / base64 变体模式时
// 才成立。若退化为「只匹配原文」（删去那两个变体注入），机密的 base64 / URL-encode 形态将
// 在出口逃逸擦除，本组用例随即变红，守住 L-12「内容不进任何输出路径」对编码形态的覆盖面。

/// 编码前置健全性：所选样本的 URL-encode / base64 形态与原文字节**严格不同**，否则
/// 「擦除编码形态」与「擦除原文」无从区分（测试自证有效，非空转）。
#[test]
fn encoding_variant_samples_differ_from_raw_bytes() {
    // ENC_SECRET / ENC_HOST 含 URL 保留字符，url_encode 必引入 `%XX`，与原文不等。
    assert_ne!(
        url_encode(ENC_SECRET).as_slice(),
        ENC_SECRET.as_bytes(),
        "url-encoded form must differ from raw, else the variant test is vacuous"
    );
    assert_ne!(
        url_encode(ENC_HOST).as_slice(),
        ENC_HOST.as_bytes(),
        "url-encoded form must differ from raw, else the variant test is vacuous"
    );
    // base64 字母表与原文 ASCII 不同，编码形态必与原文不等。
    assert_ne!(
        base64_standard(ENC_SECRET).as_slice(),
        ENC_SECRET.as_bytes(),
        "base64 form must differ from raw, else the variant test is vacuous"
    );
}

/// §8 F-6 / §5.4：机密凭据值以 **base64 形态**出现在字节流里 → 被擦除（覆盖面含 base64 内嵌）。
/// 删去 build.rs 的 `base64_standard` 变体注入即令本用例变红（base64 形态逃逸擦除）。
#[test]
fn scrub_erases_credential_base64_encoded_form() {
    let ss = enc_variant_scrubset();
    let b64 = base64_standard(ENC_SECRET);
    let mut input = b"Authorization: Basic ".to_vec();
    input.extend_from_slice(&b64);
    input.extend_from_slice(b" trailer");
    let out = ss.scrub(&input);
    // §8 F-6：机密的 base64 形态被命中擦除，base64 串不在输出。
    assert_absent(&out, &String::from_utf8_lossy(&b64));
    // 定向擦除取证：周边非敏感字节存活、命中处出现掩码（不连周边一并吞没）。
    assert_present(&out, b"Authorization: Basic ");
    assert_present(&out, b" trailer");
    assert_present(&out, SCRUB_MASK);
}

/// §8 F-6 / §5.4：真实地址以 **base64 形态**出现（如连接串内嵌）→ 被擦除。
#[test]
fn scrub_erases_target_address_base64_encoded_form() {
    let ss = enc_variant_scrubset();
    let b64 = base64_standard(ENC_HOST);
    let mut input = b"endpoint(".to_vec();
    input.extend_from_slice(&b64);
    input.extend_from_slice(b")");
    let out = ss.scrub(&input);
    // §8 F-6：真实地址的 base64 形态被擦除。
    assert_absent(&out, &String::from_utf8_lossy(&b64));
    assert_present(&out, b"endpoint(");
    assert_present(&out, SCRUB_MASK);
}

/// §8 F-6 / §5.4：机密凭据值以 **URL-encode 形态**出现（连接串里 `@:/+` 被百分号转义）
/// → 被擦除。删去 build.rs 的 `url_encode` 变体注入即令本用例变红（URL-encode 形态逃逸擦除）。
#[test]
fn scrub_erases_credential_url_encoded_form() {
    let ss = enc_variant_scrubset();
    let enc = url_encode(ENC_SECRET);
    let mut input = b"pgsql://ro:".to_vec();
    input.extend_from_slice(&enc);
    input.extend_from_slice(b"@host/db");
    let out = ss.scrub(&input);
    // §8 F-6：机密的 URL-encode 形态被命中擦除，编码串不在输出。
    assert_absent(&out, &String::from_utf8_lossy(&enc));
    // 同时原文裸串本就不应出现（编码形态输入里无原文，断言无回吐）。
    assert_absent(&out, ENC_SECRET);
    // 定向擦除取证：连接串骨架（非敏感）存活、命中处出现掩码。
    assert_present(&out, b"pgsql://ro:");
    assert_present(&out, b"@host/db");
    assert_present(&out, SCRUB_MASK);
}

/// §8 F-6 / §5.4：真实地址以 **URL-encode 形态**出现（host:port 的 `:` 被转义）→ 被擦除。
#[test]
fn scrub_erases_target_address_url_encoded_form() {
    let ss = enc_variant_scrubset();
    let enc = url_encode(ENC_HOST);
    let mut input = b"target=".to_vec();
    input.extend_from_slice(&enc);
    input.extend_from_slice(b";next");
    let out = ss.scrub(&input);
    // §8 F-6：真实地址的 URL-encode 形态被擦除。
    assert_absent(&out, &String::from_utf8_lossy(&enc));
    assert_present(&out, b"target=");
    assert_present(&out, b";next");
    assert_present(&out, SCRUB_MASK);
}

/// §8 F-6 / §5.4：同一机密的**原文、URL-encode、base64 三形态同时出现**在一段字节里 →
/// 三形态各自被命中擦除（一遍扫描覆盖原文与两种编码变体，无一逃逸）。
#[test]
fn scrub_erases_raw_url_and_base64_forms_of_same_secret_in_one_pass() {
    let ss = enc_variant_scrubset();
    let url = url_encode(ENC_SECRET);
    let b64 = base64_standard(ENC_SECRET);
    let mut input = b"raw=".to_vec();
    input.extend_from_slice(ENC_SECRET.as_bytes());
    input.extend_from_slice(b" url=");
    input.extend_from_slice(&url);
    input.extend_from_slice(b" b64=");
    input.extend_from_slice(&b64);
    let out = ss.scrub(&input);
    // §8 F-6：原文与两种编码形态全部被擦除，无一形态逃逸。
    assert_absent(&out, ENC_SECRET);
    assert_absent(&out, &String::from_utf8_lossy(&url));
    assert_absent(&out, &String::from_utf8_lossy(&b64));
    // 定向擦除取证：三个非敏感标签字节存活、出现掩码。
    assert_present(&out, b"raw=");
    assert_present(&out, b" url=");
    assert_present(&out, b" b64=");
    assert_present(&out, SCRUB_MASK);
}
