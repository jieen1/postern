//! Behavior tests for the domain_vocab unit: the domain / request / decision
//! vocabulary (module design 3.1/5.1/7, acceptance item F-1; detailed design
//! 4.1 core types, 7.1 secret-type discipline, 8.1).
//!
//! Every test pins exactly one behavior and is traceable to the assigned
//! acceptance entry via a `// §8-一F-1` comment. Compile-level pins
//! (exhaustive matches without wildcard arms, exhaustive struct
//! destructuring without `..`) make this file fail to COMPILE when a
//! variant or field is added or removed - the strongest no-dependency
//! guard available for "exactly these variants / exactly these fields".
//!
//! Structural `test_source_*` checks scan the unit's implementation files
//! the same way the contract scanner does (line comments stripped, needles
//! assembled at runtime so this file never contains forbidden text). The
//! hardened variants additionally normalize whitespace, respect string
//! literals and audit whole impl blocks, so brace-on-next-line layout,
//! path-qualified trait spellings and `//`-in-string hiding cannot dodge
//! them. The `crate_wide` checks additionally walk EVERY `src/` file at
//! runtime and pin the alias discipline (no `type` alias, no `use ... as`
//! rename of a zero-construction name, payload field names only at their
//! declarations), because an aliased struct literal would compile in any
//! crate file while matching none of the literal needles.
//!
//! NOT covered here, by design: constructing the two zero-construction
//! secret types (impossible in this crate - that is contract B-3's job);
//! their non-Clone/non-Serialize/non-Display/non-Default nature is pinned
//! at COMPILE level by the negative impl assertions below (plus the
//! structural scans and the Stele contracts), not by runtime assertions.

use std::collections::BTreeMap;
use std::net::SocketAddr;

use serde_json::{json, Value};

use postern_core::decision::{Decision, DeniedFacts, DenyResponse, EvalTrace, TraceStep};
use postern_core::domain::{
    Capability, ConditionSpec, ConstraintSpec, CredentialMeta, CredentialTier, CredentialView,
    EvalContext, GrantAction, GrantCell, MatchedGrant, Mode, PolicySnapshot, PresentedCredential,
    PrincipalId, ResolvedTarget, ResourceCode, ResourceCredential, Role, Scope, TierDecl,
    Timestamp,
};
use postern_core::error::Stage;
use postern_core::id::SnowflakeId;
use postern_core::request::ConnOrigin as Origin;
use postern_core::request::{ClassifiedIntent, Intent, NormalizedRequest, ObjectRef};

// ---------------------------------------------------------------------------
// Implementation sources for the structural checks.
// ---------------------------------------------------------------------------

const DOMAIN_SRC: &str = include_str!("../src/domain/mod.rs");
const CAPABILITY_SRC: &str = include_str!("../src/domain/capability.rs");
const SNAPSHOT_SRC: &str = include_str!("../src/domain/snapshot.rs");
const SECRET_SRC: &str = include_str!("../src/domain/secret.rs");
const REQUEST_SRC: &str = include_str!("../src/request/mod.rs");
const DECISION_SRC: &str = include_str!("../src/decision/mod.rs");

const UNIT_SOURCES: [(&str, &str); 6] = [
    ("domain/mod.rs", DOMAIN_SRC),
    ("domain/capability.rs", CAPABILITY_SRC),
    ("domain/snapshot.rs", SNAPSHOT_SRC),
    ("domain/secret.rs", SECRET_SRC),
    ("request/mod.rs", REQUEST_SRC),
    ("decision/mod.rs", DECISION_SRC),
];

// ---------------------------------------------------------------------------
// Helpers.
// ---------------------------------------------------------------------------

/// Strips `//` line comments exactly like the contract scanner does, so the
/// structural assertions below judge the same text the scanner judges.
fn strip_line_comments(s: &str) -> String {
    s.lines()
        .map(|l| match l.find("//") {
            Some(idx) => &l[..idx],
            None => l,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn is_ident_byte(b: u8) -> bool {
    b == b'_' || b.is_ascii_alphanumeric()
}

/// Word-boundary occurrence check (mirrors the scanner's word matching).
fn word_present(text: &str, word: &str) -> bool {
    let bytes = text.as_bytes();
    text.match_indices(word).any(|(i, _)| {
        let before_clear = i == 0 || !is_ident_byte(bytes[i - 1]);
        let after = i + word.len();
        let after_clear = after >= bytes.len() || !is_ident_byte(bytes[after]);
        before_clear && after_clear
    })
}

/// Word-boundary occurrence count (same matching as `word_present`).
fn word_count(text: &str, word: &str) -> usize {
    let bytes = text.as_bytes();
    text.match_indices(word)
        .filter(|&(i, _)| {
            let before_clear = i == 0 || !is_ident_byte(bytes[i - 1]);
            let after = i + word.len();
            let after_clear = after >= bytes.len() || !is_ident_byte(bytes[after]);
            before_clear && after_clear
        })
        .count()
}

/// Every `.rs` file under the crate's `src/` tree, read at runtime. The
/// alias-discipline checks must cover files OUTSIDE this unit's six
/// `include_str!` sources too: the secret payload fields are `pub` (solely
/// for the secrets crate), so an aliased struct literal would compile in
/// ANY file of this crate.
fn crate_src_files() -> Vec<(String, String)> {
    fn walk(dir: &std::path::Path, out: &mut Vec<(String, String)>) {
        for entry in std::fs::read_dir(dir).expect("crate src directory is readable") {
            let path = entry.expect("crate src entry is readable").path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                let text =
                    std::fs::read_to_string(&path).expect("crate source file is readable");
                out.push((path.display().to_string(), text));
            }
        }
    }
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    walk(&root, &mut files);
    files.sort();
    assert!(
        files
            .iter()
            .any(|(path, _)| path.replace('\\', "/").ends_with("domain/secret.rs")),
        "the crate source walk must reach domain/secret.rs"
    );
    assert!(
        files.len() >= UNIT_SOURCES.len(),
        "the crate source walk found fewer files than this unit's own sources"
    );
    files
}

/// Like `strip_line_comments`, but string-literal aware: a `//` INSIDE a
/// string literal (e.g. a URL) does not truncate the line, so no code can
/// hide behind one. Used by the hardened structural checks; the
/// scanner-mirroring checks keep the scanner's exact semantics.
fn strip_line_comments_string_aware(s: &str) -> String {
    s.lines()
        .map(|line| {
            let bytes = line.as_bytes();
            let mut in_string = false;
            let mut escaped = false;
            let mut cut = line.len();
            for (i, &b) in bytes.iter().enumerate() {
                if in_string {
                    if escaped {
                        escaped = false;
                    } else if b == b'\\' {
                        escaped = true;
                    } else if b == b'"' {
                        in_string = false;
                    }
                } else if b == b'"' {
                    in_string = true;
                } else if b == b'/' && bytes.get(i + 1) == Some(&b'/') {
                    cut = i;
                    break;
                }
            }
            &line[..cut]
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Collapses every whitespace run to a single space, so brace-on-next-line
/// layout or extra spacing cannot dodge a textual needle.
fn normalize_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Removes ALL whitespace, for needles that must match regardless of any
/// spacing at all.
fn remove_ws(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace()).collect()
}

/// True when `text`, ignoring trailing whitespace, ends with `word` at a
/// word boundary.
fn ends_with_word(text: &str, word: &str) -> bool {
    let trimmed = text.trim_end();
    trimmed.ends_with(word) && {
        let start = trimmed.len() - word.len();
        start == 0 || !is_ident_byte(trimmed.as_bytes()[start - 1])
    }
}

/// Every `impl` block of the (comment-stripped) source as (header, body):
/// the header is the text from the `impl` keyword to its opening brace,
/// the body the brace-matched contents.
fn impl_blocks(stripped: &str) -> Vec<(String, String)> {
    let bytes = stripped.as_bytes();
    let mut blocks = Vec::new();
    let mut from = 0usize;
    while let Some(found) = stripped[from..].find("impl") {
        let at = from + found;
        from = at + 1;
        let end = at + "impl".len();
        let bounded = (at == 0 || !is_ident_byte(bytes[at - 1]))
            && (end >= bytes.len() || !is_ident_byte(bytes[end]));
        if !bounded {
            continue;
        }
        let open = match stripped[at..].find('{') {
            Some(i) => at + i,
            None => break,
        };
        let mut depth = 0usize;
        let mut close = None;
        for (i, &b) in bytes.iter().enumerate().skip(open) {
            match b {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        close = Some(i);
                        break;
                    }
                }
                _ => {}
            }
        }
        let close = close.expect("impl block never closes");
        blocks.push((
            stripped[at..open].to_string(),
            stripped[open + 1..close].to_string(),
        ));
        from = open + 1;
    }
    blocks
}

/// Raw text between the braces of `enum <name>` - comments INCLUDED, which
/// is exactly the slice the contract scanner extracts and judges.
fn enum_body_of(src: &str, name: &str) -> String {
    let needle = format!("enum {name}");
    let start = src
        .find(&needle)
        .unwrap_or_else(|| panic!("definition `{needle}` not found in source"));
    let open = src[start..]
        .find('{')
        .map(|i| start + i)
        .unwrap_or_else(|| panic!("`{needle}` has no body"));
    let bytes = src.as_bytes();
    let mut depth = 0usize;
    for (i, &b) in bytes.iter().enumerate().skip(open) {
        if b == b'{' {
            depth += 1;
        } else if b == b'}' {
            depth -= 1;
            if depth == 0 {
                return src[open + 1..i].to_string();
            }
        }
    }
    panic!("enum body of `{name}` never closes");
}

/// Variant identifiers at depth 0 of an enum body (comments stripped).
fn top_level_variants(body: &str) -> Vec<String> {
    let stripped = strip_line_comments(body);
    let mut variants = Vec::new();
    let mut depth = 0usize;
    let mut expecting = true;
    let mut current = String::new();
    for ch in stripped.chars() {
        match ch {
            '{' | '(' | '[' => {
                if depth == 0 && !current.is_empty() {
                    variants.push(std::mem::take(&mut current));
                    expecting = false;
                }
                depth += 1;
            }
            '}' | ')' | ']' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                if !current.is_empty() {
                    variants.push(std::mem::take(&mut current));
                }
                expecting = true;
            }
            c if depth == 0 && expecting && (c.is_ascii_alphanumeric() || c == '_') => {
                current.push(c);
            }
            c if depth == 0 && !current.is_empty() && c.is_whitespace() => {
                variants.push(std::mem::take(&mut current));
                expecting = false;
            }
            _ => {}
        }
    }
    if !current.is_empty() {
        variants.push(current);
    }
    variants
}

fn rc(code: &str) -> ResourceCode {
    ResourceCode::new(code)
}

fn obj(reference: &str) -> ObjectRef {
    ObjectRef::new(reference)
}

fn pid(raw: u64) -> PrincipalId {
    PrincipalId::new(SnowflakeId::from_raw(raw))
}

fn jval<T: serde::Serialize>(value: &T) -> Value {
    serde_json::to_value(value).expect("vocabulary type serializes to JSON")
}

/// The documented deny-response example (scenario spec 01, trace 3).
fn sample_deny_response(
    request_hint: Option<&str>,
    operator_note: Option<&str>,
) -> DenyResponse {
    DenyResponse {
        decision: "deny",
        denied: DeniedFacts {
            resource: rc("docker-A"),
            capability: Capability::Manage,
            objects: vec![obj("container:app-order")],
        },
        reason: "role=observer does not include docker-A:manage".to_string(),
        your_grants: BTreeMap::from([
            (rc("db-main"), vec!["observe".to_string(), "query".to_string()]),
            (rc("docker-A"), vec!["observe".to_string()]),
        ]),
        request_hint: request_hint.map(str::to_string),
        operator_note: operator_note.map(str::to_string),
    }
}

// ---------------------------------------------------------------------------
// Compile-level existence pins. The two opaque secret types cannot be
// constructed in this crate, so their existence (and their hand-written
// Debug impls) is pinned by reference only.
// ---------------------------------------------------------------------------

// §8-一F-1 - the opaque secret types are declared in core (by-reference pin).
fn _pin_opaque_secret_types(target: &ResolvedTarget, credential: &ResourceCredential) {
    let _ = (target, credential);
}

fn _requires_debug<T: std::fmt::Debug>() {}

// §8-一F-1 - the whole redacted family implements Debug (hand-written).
fn _pin_redacted_family_has_debug() {
    _requires_debug::<ResolvedTarget>();
    _requires_debug::<ResourceCredential>();
    _requires_debug::<PresentedCredential>();
    _requires_debug::<Intent>();
}

/// Compile-level NEGATIVE impl pin (the classic one-candidate ambiguity
/// trick, inlined so no dependency is added): if the type implements any
/// listed trait - derived, hand-written, path-qualified or renamed - the
/// `AmbiguousIfImpl` parameter gains a second candidate and this file
/// fails to COMPILE.
macro_rules! assert_never_implements {
    ($ty:ty: $($tr:path),+ $(,)?) => {
        const _: fn() = || {
            trait AmbiguousIfImpl<A> {
                fn some_item() {}
            }
            impl<T: ?Sized> AmbiguousIfImpl<()> for T {}
            $({
                #[allow(dead_code)]
                struct Invalid;
                impl<T: ?Sized + $tr> AmbiguousIfImpl<Invalid> for T {}
            })+
            let _ = <$ty as AmbiguousIfImpl<_>>::some_item;
        };
    };
}

// §8-一F-1 (log red line 7.5, compile-level): no member of the secret
// family - the two zero-construction types, the presented credential and
// the raw intent - implements Clone, Default, Display or any serde trait,
// in ANY spelling (derive, hand-written, path-qualified or renamed).
assert_never_implements!(
    ResolvedTarget:
        Clone, Default, std::fmt::Display, serde::Serialize, serde::de::DeserializeOwned
);
assert_never_implements!(
    ResourceCredential:
        Clone, Default, std::fmt::Display, serde::Serialize, serde::de::DeserializeOwned
);
assert_never_implements!(
    PresentedCredential:
        Clone, Default, std::fmt::Display, serde::Serialize, serde::de::DeserializeOwned
);
assert_never_implements!(
    Intent:
        Clone, Default, std::fmt::Display, serde::Serialize, serde::de::DeserializeOwned
);

/// Successor chain over the verb enum: an exhaustive match with no `_ =>`
/// arm, so adding or removing a variant makes this test file fail to
/// compile until the chain (and every assertion fed by it) is updated.
fn next_capability(prev: Option<Capability>) -> Option<Capability> {
    match prev {
        None => Some(Capability::Observe),
        Some(Capability::Observe) => Some(Capability::Query),
        Some(Capability::Query) => Some(Capability::Mutate),
        Some(Capability::Mutate) => Some(Capability::Execute),
        Some(Capability::Execute) => Some(Capability::Manage),
        Some(Capability::Manage) => Some(Capability::Destroy),
        Some(Capability::Destroy) => None,
    }
}

// ---------------------------------------------------------------------------
// Capability: exactly six orthogonal verbs.
// ---------------------------------------------------------------------------

#[test]
fn test_capability_is_exactly_the_six_orthogonal_verbs_in_design_order() {
    // §8-一F-1: six variants, design order, nothing else (compile-pinned).
    let mut verbs: Vec<Capability> = Vec::new();
    while let Some(verb) = next_capability(verbs.last().copied()) {
        assert!(
            !verbs.contains(&verb),
            "successor chain revisits {verb:?}; every variant must appear exactly once"
        );
        verbs.push(verb);
    }
    assert_eq!(
        verbs,
        [
            Capability::Observe,
            Capability::Query,
            Capability::Mutate,
            Capability::Execute,
            Capability::Manage,
            Capability::Destroy,
        ]
    );
    assert_eq!(verbs.len(), 6);
}

#[test]
fn test_capability_enum_body_holds_six_variants_and_never_the_banned_admin_word() {
    // §8-一F-1: the raw enum body (comments included, exactly what the
    // contract scanner judges) carries the six verbs and never the banned
    // word - so "granting it" stays unrepresentable at the type level.
    let body = enum_body_of(CAPABILITY_SRC, "Capability");
    assert_eq!(
        top_level_variants(&body),
        ["Observe", "Query", "Mutate", "Execute", "Manage", "Destroy"]
    );
    let banned = ["Ad", "min"].concat();
    assert!(
        !word_present(&body, &banned),
        "the verb enum body must never contain the word `{banned}` (axiom three)"
    );
}

#[test]
fn test_capability_serializes_as_lowercase_verb_names() {
    // §8-一F-1: serde form matches the documented JSON ("manage" etc.).
    let pairs = [
        (Capability::Observe, "observe"),
        (Capability::Query, "query"),
        (Capability::Mutate, "mutate"),
        (Capability::Execute, "execute"),
        (Capability::Manage, "manage"),
        (Capability::Destroy, "destroy"),
    ];
    for (verb, name) in pairs {
        assert_eq!(jval(&verb), json!(name), "serde form of {verb:?}");
    }
}

#[test]
fn test_capability_as_str_yields_the_lowercase_verb_names() {
    // §8-一F-1: as_str is the same canonical text as the serde form (the
    // `your_grants` capability-name source).
    let pairs = [
        (Capability::Observe, "observe"),
        (Capability::Query, "query"),
        (Capability::Mutate, "mutate"),
        (Capability::Execute, "execute"),
        (Capability::Manage, "manage"),
        (Capability::Destroy, "destroy"),
    ];
    for (verb, name) in pairs {
        assert_eq!(verb.as_str(), name, "canonical name of {verb:?}");
    }
}

// ---------------------------------------------------------------------------
// ConnOrigin: exactly two states, minimal trustworthy fields.
// ---------------------------------------------------------------------------

#[test]
fn test_conn_origin_unix_peer_state_carries_exactly_uid_and_gid() {
    // §8-一F-1: UnixPeer carries uid and gid, nothing else (the exhaustive
    // match with no wildcard compile-pins the two-state shape).
    let origin = Origin::UnixPeer {
        uid: 1000,
        gid: 1001,
    };
    match origin {
        Origin::UnixPeer { uid, gid } => {
            assert_eq!(uid, 1000);
            assert_eq!(gid, 1001);
        }
        Origin::Tcp { remote } => panic!("unix peer expected, got tcp from {remote}"),
    }
}

#[test]
fn test_conn_origin_tcp_state_carries_exactly_the_remote_socket_addr() {
    // §8-一F-1: Tcp carries the observed remote address, nothing else.
    let addr: SocketAddr = "127.0.0.1:5432".parse().expect("literal socket addr");
    let origin = Origin::Tcp { remote: addr };
    match origin {
        Origin::Tcp { remote } => assert_eq!(remote, addr),
        Origin::UnixPeer { uid, gid } => {
            panic!("tcp expected, got unix peer uid={uid} gid={gid}")
        }
    }
}

#[test]
fn test_conn_origin_enum_body_holds_exactly_the_two_designed_states() {
    // §8-一F-1: exactly UnixPeer and Tcp in the source enum body - no third
    // state, and no forgeable pid/exe field can hide in an extra variant.
    let body = enum_body_of(REQUEST_SRC, "ConnOrigin");
    assert_eq!(top_level_variants(&body), ["UnixPeer", "Tcp"]);
}

// ---------------------------------------------------------------------------
// DenyResponse / DeniedFacts: the structured refusal.
// ---------------------------------------------------------------------------

#[test]
fn test_deny_response_field_set_is_exactly_the_six_design_fields() {
    // §8-一F-1: exhaustive destructuring (no `..`) compile-pins the exact
    // field set decision/denied/reason/your_grants/request_hint/operator_note.
    let DenyResponse {
        decision,
        denied,
        reason,
        your_grants,
        request_hint,
        operator_note,
    } = sample_deny_response(
        Some("postern elevate agent1 --cap docker-A:manage --ttl 30m"),
        None,
    );
    assert_eq!(decision, "deny");
    assert_eq!(denied.resource, rc("docker-A"));
    assert_eq!(denied.capability, Capability::Manage);
    assert_eq!(denied.objects, vec![obj("container:app-order")]);
    assert_eq!(reason, "role=observer does not include docker-A:manage");
    assert_eq!(
        your_grants.get(&rc("db-main")),
        Some(&vec!["observe".to_string(), "query".to_string()])
    );
    assert_eq!(
        request_hint.as_deref(),
        Some("postern elevate agent1 --cap docker-A:manage --ttl 30m")
    );
    assert_eq!(operator_note, None);
}

#[test]
fn test_deny_response_json_omits_unset_operator_note_but_keeps_request_hint_null() {
    // §8-一F-1: operator_note has skip_serializing_if (absent when no human
    // prewrote it); request_hint stays present as null for ungrantable verbs.
    let value = jval(&sample_deny_response(None, None));
    let object = value.as_object().expect("deny response is a JSON object");
    let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        ["decision", "denied", "reason", "request_hint", "your_grants"]
    );
    assert_eq!(value["request_hint"], Value::Null);
}

#[test]
fn test_deny_response_json_carries_operator_note_verbatim_when_prewritten() {
    // §8-一F-1: a prewritten note is relayed verbatim and only then appears.
    let value = jval(&sample_deny_response(
        None,
        Some("ask in #db-owners before requesting manage"),
    ));
    let object = value.as_object().expect("deny response is a JSON object");
    let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "decision",
            "denied",
            "operator_note",
            "reason",
            "request_hint",
            "your_grants"
        ]
    );
    assert_eq!(
        value["operator_note"],
        json!("ask in #db-owners before requesting manage")
    );
}

#[test]
fn test_your_grants_map_keys_serialize_as_plain_resource_code_strings() {
    // §8-一F-1: ResourceCode keys a BTreeMap and serializes as the plain
    // code string (deterministic key order included).
    let value = jval(&sample_deny_response(None, None));
    assert_eq!(
        value["your_grants"],
        json!({
            "db-main": ["observe", "query"],
            "docker-A": ["observe"],
        })
    );
}

#[test]
fn test_denied_facts_serialize_to_the_documented_resource_capability_objects_shape() {
    // §8-一F-1: byte-for-byte the documented denied facts shape
    // (scenario spec 01, trace 3).
    let facts = DeniedFacts {
        resource: rc("docker-A"),
        capability: Capability::Manage,
        objects: vec![obj("container:app-order")],
    };
    assert_eq!(
        jval(&facts),
        json!({
            "resource": "docker-A",
            "capability": "manage",
            "objects": ["container:app-order"],
        })
    );
}

// ---------------------------------------------------------------------------
// Decision: three values, never a bare boolean.
// ---------------------------------------------------------------------------

#[test]
fn test_decision_allow_state_carries_matched_grant_and_selected_tier() {
    // §8-一F-1: Allow cannot exist without the granting facts (the
    // wildcard-free match compile-pins the three-valued shape).
    let decision = Decision::Allow {
        grant: MatchedGrant {
            resource: rc("db-main"),
            capability: Capability::Query,
            role: Role::new("observer"),
        },
        tier: CredentialTier::new("readonly"),
    };
    match decision {
        Decision::Allow { grant, tier } => {
            assert_eq!(grant.resource, rc("db-main"));
            assert_eq!(grant.capability, Capability::Query);
            assert_eq!(grant.role, Role::new("observer"));
            assert_eq!(tier, CredentialTier::new("readonly"));
        }
        Decision::Deny(response) => panic!("allow expected, got deny: {response:?}"),
        Decision::Escalate { fallback } => {
            panic!("allow expected, got escalate: {fallback:?}")
        }
    }
}

#[test]
fn test_decision_deny_state_wraps_the_structured_response() {
    // §8-一F-1: Deny carries the full structured response, not a flag.
    let decision = Decision::Deny(sample_deny_response(None, None));
    match decision {
        Decision::Deny(response) => {
            assert_eq!(response.decision, "deny");
            assert_eq!(
                response.reason,
                "role=observer does not include docker-A:manage"
            );
        }
        Decision::Allow { grant, tier } => {
            panic!("deny expected, got allow: {grant:?} via {tier:?}")
        }
        Decision::Escalate { fallback } => {
            panic!("deny expected, got escalate: {fallback:?}")
        }
    }
}

#[test]
fn test_decision_escalate_state_carries_a_fallback_deny_response() {
    // §8-一F-1: Escalate embeds its fold target - a complete deny response,
    // so a closed approval lane never needs to invent one.
    let decision = Decision::Escalate {
        fallback: sample_deny_response(None, None),
    };
    match decision {
        Decision::Escalate { fallback } => assert_eq!(fallback.decision, "deny"),
        Decision::Allow { grant, tier } => {
            panic!("escalate expected, got allow: {grant:?} via {tier:?}")
        }
        Decision::Deny(response) => panic!("escalate expected, got deny: {response:?}"),
    }
}

// ---------------------------------------------------------------------------
// EvalTrace: stepwise Vec records referencing error::Stage.
// ---------------------------------------------------------------------------

#[test]
fn test_eval_trace_final_stage_is_the_stage_of_the_last_recorded_step() {
    // §8-一F-1: on a short-circuit the trace ends at the deciding step and
    // that step's stage IS the deny stage.
    let trace = EvalTrace {
        steps: vec![
            TraceStep {
                stage: Stage::Auth,
                detail: "principal authenticated".to_string(),
            },
            TraceStep {
                stage: Stage::Rbac,
                detail: "grant cell hit".to_string(),
            },
            TraceStep {
                stage: Stage::Condition,
                detail: "predicate time_window=false".to_string(),
            },
        ],
    };
    assert_eq!(trace.final_stage(), Some(Stage::Condition));
}

#[test]
fn test_eval_trace_final_stage_of_an_empty_trace_is_none() {
    // §8-一F-1: boundary - an empty trace attributes no stage (None), it
    // never invents one.
    let trace = EvalTrace { steps: Vec::new() };
    assert_eq!(trace.final_stage(), None);
}

#[test]
fn test_eval_trace_serializes_steps_as_an_ordered_array_with_lowercase_stage_names() {
    // §8-一F-1: Vec order is pipeline order and stages reference the closed
    // error::Stage vocabulary (lowercase serde form).
    let trace = EvalTrace {
        steps: vec![
            TraceStep {
                stage: Stage::Auth,
                detail: "principal authenticated".to_string(),
            },
            TraceStep {
                stage: Stage::Rbac,
                detail: "grant cell hit".to_string(),
            },
        ],
    };
    assert_eq!(
        jval(&trace),
        json!({
            "steps": [
                { "stage": "auth", "detail": "principal authenticated" },
                { "stage": "rbac", "detail": "grant cell hit" },
            ]
        })
    );
}

// ---------------------------------------------------------------------------
// Snowflake-backed ids and plain-string codes.
// ---------------------------------------------------------------------------

#[test]
fn test_principal_id_serializes_as_a_decimal_string() {
    // §8-一F-1: snowflake-backed id types serialize as decimal strings,
    // never as JSON numbers (53-bit JS safety).
    assert_eq!(jval(&pid(123_456_789)), json!("123456789"));
}

#[test]
fn test_principal_id_deserialization_rejects_a_json_number() {
    // §8-一F-1 (fail-closed boundary): a numeric id is refused outright,
    // never silently accepted with possible precision loss.
    let error = serde_json::from_str::<PrincipalId>("42")
        .expect_err("a JSON number must not deserialize into a principal id");
    let message = error.to_string();
    assert!(
        message.contains("invalid type: integer"),
        "rejection names the offending type: {message}"
    );
    assert!(
        message.contains("decimal string"),
        "rejection names the expected form: {message}"
    );
}

#[test]
fn test_resource_code_serializes_as_its_plain_code_string() {
    // §8-一F-1: a resource code is a plain JSON string.
    assert_eq!(jval(&rc("db-main")), json!("db-main"));
}

#[test]
fn test_resource_code_round_trips_its_code_text() {
    // §8-一F-1: the code text survives wrapping unchanged.
    assert_eq!(rc("db-main").as_str(), "db-main");
}

#[test]
fn test_object_ref_is_a_plain_reference_string_view() {
    // §8-一F-1: object references render and serialize as the plain
    // reference text (e.g. "container:app-order").
    let reference = obj("container:app-order");
    assert_eq!(reference.as_str(), "container:app-order");
    assert_eq!(jval(&reference), json!("container:app-order"));
}

// ---------------------------------------------------------------------------
// Secret family and Intent: REDACTED debug, boxed payloads.
// ---------------------------------------------------------------------------

#[test]
fn test_presented_credential_boxes_kind_and_secret_for_authenticators() {
    // §8-一F-1: the public constructor (shells need it) boxes the
    // registry-selection kind plus the secret bytes.
    let credential = PresentedCredential::new("api_key", b"k-12345".to_vec());
    assert_eq!(credential.kind(), "api_key");
    assert_eq!(credential.secret(), b"k-12345");
}

#[test]
fn test_presented_credential_debug_is_always_redacted() {
    // §8-一F-1 (secret-type discipline): Debug output is the constant
    // REDACTED, independent of the payload.
    let credential = PresentedCredential::new("api_key", b"k-12345".to_vec());
    assert_eq!(format!("{credential:?}"), "REDACTED");
}

#[test]
fn test_intent_debug_is_always_redacted() {
    // §8-一F-1 (log red line 7.5): intent payloads can carry business
    // data; Debug output is the constant REDACTED.
    let intent = Intent::new(b"fetch-rows t.secret_col".to_vec());
    assert_eq!(format!("{intent:?}"), "REDACTED");
}

#[test]
fn test_intent_boxes_the_raw_payload_without_interpreting_it() {
    // §8-一F-1: core boxes, never interprets - the bytes come back
    // unchanged for the adapter, the sole interpreter.
    let intent = Intent::new(b"raw-protocol-bytes".to_vec());
    assert_eq!(intent.payload(), b"raw-protocol-bytes");
}

#[test]
fn test_normalized_request_field_set_is_presented_origin_resource_intent() {
    // §8-一F-1: exhaustive destructuring compile-pins the exact field set
    // of the step [0] product.
    let request = NormalizedRequest {
        presented: PresentedCredential::new("api_key", b"hunter2-secret".to_vec()),
        origin: Origin::UnixPeer {
            uid: 1000,
            gid: 1000,
        },
        resource: rc("db-main"),
        intent: Intent::new(b"raw-protocol-bytes".to_vec()),
    };
    let NormalizedRequest {
        presented,
        origin,
        resource,
        intent,
    } = request;
    assert_eq!(presented.kind(), "api_key");
    match origin {
        Origin::UnixPeer { uid, gid } => {
            assert_eq!(uid, 1000);
            assert_eq!(gid, 1000);
        }
        Origin::Tcp { remote } => panic!("unix peer expected, got tcp from {remote}"),
    }
    assert_eq!(resource, rc("db-main"));
    assert_eq!(intent.payload(), b"raw-protocol-bytes");
}

#[test]
fn test_normalized_request_debug_redacts_credential_and_intent_payloads() {
    // §8-一F-1: rendering a whole request leaks neither the presented
    // secret nor the intent text - exactly two REDACTED markers stand in;
    // the resource code stays visible (codes are public facts).
    let request = NormalizedRequest {
        presented: PresentedCredential::new("api_key", b"hunter2-secret".to_vec()),
        origin: Origin::UnixPeer {
            uid: 1000,
            gid: 1000,
        },
        resource: rc("db-main"),
        intent: Intent::new(b"fetch-rows users.password".to_vec()),
    };
    let rendered = format!("{request:?}");
    assert!(
        !rendered.contains("hunter2"),
        "presented secret leaked: {rendered}"
    );
    assert!(
        !rendered.contains("users.password"),
        "intent payload leaked: {rendered}"
    );
    assert_eq!(
        rendered.matches("REDACTED").count(),
        2,
        "one marker per redacted member: {rendered}"
    );
    assert!(
        rendered.contains("db-main"),
        "resource codes are public facts and stay visible: {rendered}"
    );
}

#[test]
fn test_source_secret_family_declares_no_clone_serde_display_or_default() {
    // §8-一F-1 (secret-type discipline, type-level): the secret module
    // carries no derive at all and no hand-written Clone/Serialize/
    // Display/Default impl - judged on comment-stripped source like the
    // contract scanner does.
    let stripped = strip_line_comments(SECRET_SRC);
    for forbidden in [
        "#[derive",
        "impl Clone for",
        "impl Serialize for",
        "impl serde::Serialize for",
        "Display for",
        "impl Default for",
    ] {
        assert!(
            !stripped.contains(forbidden),
            "domain/secret.rs must not contain `{forbidden}`"
        );
    }
    // Layout/path-robust form: whitespace collapsed, string-aware comment
    // stripping, needles anchored on the trait's TERMINAL path segment -
    // so `impl core::default::Default for ...`, `impl ::serde::Serialize
    // for ...` and brace/newline layout tricks all stay caught.
    let normalized = normalize_ws(&strip_line_comments_string_aware(SECRET_SRC));
    for forbidden in ["Clone for", "Serialize for", "Display for", "Default for"] {
        assert!(
            !normalized.contains(forbidden),
            "domain/secret.rs must not contain `{forbidden}` in any path qualification or layout"
        );
    }
}

#[test]
fn test_source_zero_construction_secret_types_have_only_redacted_debug_impls() {
    // §8-一F-1 (log red line 7.5 + zero construction points): across every
    // unit file, the ONLY impl block naming a zero-construction secret
    // type is the hand-written Debug impl in domain/secret.rs, and its
    // body is exactly one write of the REDACTED constant - no `self.`
    // field read, no `Self` construction. A leaking Debug body, a forged
    // inherent constructor or any extra trait impl (whatever its path
    // spelling) turns this red.
    let mut debug_impls_seen: BTreeMap<&str, usize> = BTreeMap::new();
    for (label, src) in UNIT_SOURCES {
        let stripped = strip_line_comments_string_aware(src);
        for (header, body) in impl_blocks(&stripped) {
            for name in ["ResolvedTarget", "ResourceCredential"] {
                if !word_present(&header, name) {
                    continue;
                }
                assert_eq!(
                    label, "domain/secret.rs",
                    "only domain/secret.rs may hold an impl naming {name}, found one in {label}"
                );
                assert_eq!(
                    normalize_ws(&header),
                    format!("impl fmt::Debug for {name}"),
                    "the only impl allowed for {name} is its hand-written Debug"
                );
                let compact = remove_ws(&body);
                assert!(
                    compact.contains("f.write_str(\"REDACTED\")"),
                    "Debug for {name} must write the REDACTED constant, body: {body}"
                );
                assert_eq!(
                    compact.matches("write_str").count(),
                    1,
                    "Debug for {name} writes exactly once, body: {body}"
                );
                assert!(
                    !compact.contains("self."),
                    "Debug for {name} must not read any field, body: {body}"
                );
                assert!(
                    !word_present(&body, "Self"),
                    "Debug for {name} must not construct Self, body: {body}"
                );
                *debug_impls_seen.entry(name).or_insert(0) += 1;
            }
        }
    }
    assert_eq!(
        debug_impls_seen.get("ResolvedTarget"),
        Some(&1),
        "exactly one (Debug) impl for the resolved-target type"
    );
    assert_eq!(
        debug_impls_seen.get("ResourceCredential"),
        Some(&1),
        "exactly one (Debug) impl for the resource-credential type"
    );
}

#[test]
fn test_source_unit_files_contain_no_secret_construction_text_in_any_layout() {
    // §8-一F-1 (zero construction points, layout-robust complement to the
    // scanner-mirror check below): every word-boundary occurrence of a
    // zero-construction type name followed - across ANY whitespace,
    // including a newline - by `{` must be the struct definition or an
    // impl header (both policed by the impl audit above), and `<name>::`
    // associated paths are forbidden outright, so brace-on-next-line
    // literals and constructor names other than `new` stay caught.
    for (label, src) in UNIT_SOURCES {
        let stripped = strip_line_comments_string_aware(src);
        let bytes = stripped.as_bytes();
        for name in ["ResolvedTarget", "ResourceCredential"] {
            for (at, _) in stripped.match_indices(name) {
                let end = at + name.len();
                let bounded = (at == 0 || !is_ident_byte(bytes[at - 1]))
                    && (end >= bytes.len() || !is_ident_byte(bytes[end]));
                if !bounded {
                    continue;
                }
                let rest = stripped[end..].trim_start();
                assert!(
                    !rest.starts_with("::"),
                    "{label} contains a `{name}::` associated path (construction-adjacent text)"
                );
                if rest.starts_with('{') {
                    let before = &stripped[..at];
                    assert!(
                        ends_with_word(before, "struct") || ends_with_word(before, "for"),
                        "{label} contains `{name}` followed by a brace outside its \
                         definition or impl header - construction text"
                    );
                }
            }
        }
    }
}

#[test]
fn test_source_intent_carries_no_derive_and_only_boxing_and_debug_impls() {
    // §8-一F-1 (log red line 7.5): Intent's definition carries no derive
    // attribute at all, and the only impl blocks naming Intent are the
    // inherent boxing impl and the hand-written Debug impl - so no Clone,
    // serde or Display can attach to it in this file, in any spelling.
    let stripped = strip_line_comments_string_aware(REQUEST_SRC);
    let def = stripped
        .find("struct Intent")
        .expect("Intent struct defined in request/mod.rs");
    let def_end = def + "struct Intent".len();
    assert!(
        def_end >= stripped.len() || !is_ident_byte(stripped.as_bytes()[def_end]),
        "found the Intent definition itself, not a longer identifier"
    );
    // The span between the previous item's end and the definition holds
    // exactly the attributes attached to it - it must carry no derive.
    let attrs_start = stripped[..def].rfind(['}', ';']).map_or(0, |i| i + 1);
    let attrs = &stripped[attrs_start..def];
    assert!(
        !attrs.contains("derive"),
        "Intent must carry no derive attribute, found: {attrs}"
    );
    let mut headers: Vec<String> = impl_blocks(&stripped)
        .into_iter()
        .filter(|(header, _)| word_present(header, "Intent"))
        .map(|(header, _)| normalize_ws(&header))
        .collect();
    headers.sort();
    assert_eq!(
        headers,
        ["impl Intent", "impl fmt::Debug for Intent"],
        "Intent allows exactly its boxing impl and its hand-written Debug impl"
    );
}

#[test]
fn test_source_unit_files_contain_no_secret_construction_point_text() {
    // §8-一F-1 (zero construction points): no unit file contains even the
    // text of a struct literal or constructor call for the two opaque
    // types (needles assembled at runtime; comment-stripped like the
    // scanner).
    for (label, src) in UNIT_SOURCES {
        let stripped = strip_line_comments(src);
        for name in ["ResolvedTarget", "ResourceCredential"] {
            for needle in [format!("{name} {{"), format!("{name}::new")] {
                assert!(
                    !stripped.contains(&needle),
                    "{label} contains construction text `{needle}`"
                );
            }
        }
    }
}

#[test]
fn test_source_crate_wide_zero_construction_names_are_never_aliased_or_renamed() {
    // §8-一F-1 (zero construction points, alias-proof): `type Forged =
    // ResourceCredential;` plus a struct literal of the ALIAS is a real
    // construction point that contains none of the literal needles above,
    // and it compiles in ANY crate file because the payload fields are
    // `pub` for the secrets crate. So across EVERY `src/` file: no `type`
    // item may mention a zero-construction name in its declaration span
    // (any path qualification included), no occurrence may be renamed with
    // `use ... as`, and the construction-text discipline (`::` associated
    // paths, braces outside the definition / Debug impl header) holds
    // crate-wide, not just in this unit's six files.
    for (label, src) in crate_src_files() {
        let stripped = strip_line_comments_string_aware(&src);
        let bytes = stripped.as_bytes();
        for (at, _) in stripped.match_indices("type") {
            let end = at + "type".len();
            let bounded = (at == 0 || !is_ident_byte(bytes[at - 1]))
                && (end >= bytes.len() || !is_ident_byte(bytes[end]));
            if !bounded {
                continue;
            }
            let span_end = stripped[end..].find(';').map_or(stripped.len(), |i| end + i);
            let span = &stripped[end..span_end];
            for name in ["ResolvedTarget", "ResourceCredential"] {
                assert!(
                    !word_present(span, name),
                    "{label} declares a `type` item mentioning {name} - an alias \
                     reopens a construction point:{span}"
                );
            }
        }
        for name in ["ResolvedTarget", "ResourceCredential"] {
            for (at, _) in stripped.match_indices(name) {
                let end = at + name.len();
                let bounded = (at == 0 || !is_ident_byte(bytes[at - 1]))
                    && (end >= bytes.len() || !is_ident_byte(bytes[end]));
                if !bounded {
                    continue;
                }
                let rest = stripped[end..].trim_start();
                let renamed = rest.strip_prefix("as").is_some_and(|tail| {
                    tail.as_bytes().first().is_none_or(|&b| !is_ident_byte(b))
                });
                assert!(
                    !renamed,
                    "{label} renames {name} with `as` - an alias reopens a \
                     construction point"
                );
                assert!(
                    !rest.starts_with("::"),
                    "{label} contains a `{name}::` associated path (construction-adjacent text)"
                );
                if rest.starts_with('{') {
                    let before = &stripped[..at];
                    assert!(
                        ends_with_word(before, "struct") || ends_with_word(before, "for"),
                        "{label} contains `{name}` followed by a brace outside its \
                         definition or impl header - construction text"
                    );
                }
            }
        }
    }
}

#[test]
fn test_source_crate_wide_secret_payload_field_names_live_only_in_their_declarations() {
    // §8-一F-1 (zero construction points, field-name line of defense): any
    // struct literal of the two opaque types must spell their payload field
    // names - including the shorthand `Forged { material }` form that
    // names no type at all - and any forged accessor must read them. So
    // crate-wide, each identifier appears exactly once: as its
    // `pub <field>: String` declaration in domain/secret.rs.
    let mut declarations_seen = 0usize;
    for (label, src) in crate_src_files() {
        let stripped = strip_line_comments_string_aware(&src);
        let is_secret_module = label.replace('\\', "/").ends_with("domain/secret.rs");
        for field in ["endpoint", "material"] {
            let count = word_count(&stripped, field);
            if is_secret_module {
                assert_eq!(
                    count, 1,
                    "{label}: `{field}` appears exactly once - its declaration"
                );
                let declaration = format!("pub {field}: String");
                assert!(
                    normalize_ws(&stripped).contains(&declaration),
                    "{label}: the single `{field}` occurrence is `{declaration}`"
                );
                declarations_seen += 1;
            } else {
                assert_eq!(
                    count, 0,
                    "{label}: secret payload field name `{field}` may appear only in \
                     domain/secret.rs - a struct literal (even via a type alias, even \
                     in shorthand form) has to spell it"
                );
            }
        }
    }
    assert_eq!(
        declarations_seen, 2,
        "the walk judged both payload-field declarations in domain/secret.rs"
    );
}

// ---------------------------------------------------------------------------
// Authorization lattice vocabulary.
// ---------------------------------------------------------------------------

#[test]
fn test_matched_grant_pins_resource_capability_and_role_provenance() {
    // §8-一F-1: the matched cell carries its lattice coordinates plus the
    // role provenance (deny reasons cite policy facts like the role name).
    let MatchedGrant {
        resource,
        capability,
        role,
    } = MatchedGrant {
        resource: rc("db-main"),
        capability: Capability::Query,
        role: Role::new("observer"),
    };
    assert_eq!(resource, rc("db-main"));
    assert_eq!(capability, Capability::Query);
    assert_eq!(role, Role::new("observer"));
}

#[test]
fn test_grant_cell_carries_action_with_constraint_and_condition_declarations() {
    // §8-一F-1: an expanded cell bundles routing action plus the attached
    // constraint (step [4]) and condition (step [5]) declarations.
    let cell = GrantCell {
        resource: rc("db-main"),
        capability: Capability::Query,
        role: Role::new("observer"),
        action: GrantAction::Escalate,
        constraints: vec![ConstraintSpec {
            kind: "table_allow".to_string(),
            spec: r#"{"tables":["orders"]}"#.to_string(),
        }],
        conditions: vec![ConditionSpec {
            kind: "time_window".to_string(),
            spec: r#"{"window":"09:00-18:00"}"#.to_string(),
        }],
    };
    let GrantCell {
        resource,
        capability,
        role,
        action,
        constraints,
        conditions,
    } = cell;
    assert_eq!(resource, rc("db-main"));
    assert_eq!(capability, Capability::Query);
    assert_eq!(role, Role::new("observer"));
    match action {
        GrantAction::Escalate => {}
        GrantAction::Allow => panic!("constructed an escalate cell, read back allow"),
    }
    let constraint = constraints.first().expect("one constraint declared");
    assert_eq!(constraint.kind, "table_allow");
    assert_eq!(constraint.spec, r#"{"tables":["orders"]}"#);
    let condition = conditions.first().expect("one condition declared");
    assert_eq!(condition.kind, "time_window");
    assert_eq!(condition.spec, r#"{"window":"09:00-18:00"}"#);
}

#[test]
fn test_classified_intent_field_set_is_capability_plus_objects() {
    // §8-一F-1: the step [2] product is exactly verb + objects.
    let ClassifiedIntent {
        capability,
        objects,
    } = ClassifiedIntent {
        capability: Capability::Manage,
        objects: vec![obj("container:app-order")],
    };
    assert_eq!(capability, Capability::Manage);
    assert_eq!(objects, vec![obj("container:app-order")]);
}

#[test]
fn test_scope_states_distinguish_enumerated_resources_from_label_selector() {
    // §8-一F-1: the two jurisdiction shapes are distinct states (wildcard-
    // free matches compile-pin the two-state shape).
    let enumerated = Scope::Resources(vec![rc("db-main"), rc("redis-main")]);
    match enumerated {
        Scope::Resources(codes) => {
            assert_eq!(codes, vec![rc("db-main"), rc("redis-main")]);
        }
        Scope::Selector(spec) => panic!("enumerated scope expected, got selector {spec}"),
    }
    let selector = Scope::Selector("env=prod".to_string());
    match selector {
        Scope::Selector(spec) => assert_eq!(spec, "env=prod"),
        Scope::Resources(codes) => {
            panic!("selector scope expected, got {} resources", codes.len())
        }
    }
}

#[test]
fn test_role_round_trips_its_declared_name() {
    // §8-一F-1: the role name survives wrapping unchanged.
    assert_eq!(Role::new("observer").as_str(), "observer");
}

#[test]
fn test_credential_tier_round_trips_its_declared_name() {
    // §8-一F-1: the tier name survives wrapping unchanged.
    assert_eq!(CredentialTier::new("readonly").as_str(), "readonly");
}

#[test]
fn test_timestamp_round_trips_unix_milliseconds() {
    // §8-一F-1: a wall-clock instant is exactly its Unix milliseconds.
    assert_eq!(
        Timestamp::from_unix_ms(1_767_225_600_000).as_unix_ms(),
        1_767_225_600_000
    );
}

#[test]
fn test_mode_orders_strictness_freeze_above_maintain_above_observe_above_normal() {
    // §8-一F-1: Ord IS strictness, so "take the maximum" always takes the
    // strictest mode, never the loosest.
    assert!(Mode::Freeze > Mode::Maintain);
    assert!(Mode::Maintain > Mode::Observe);
    assert!(Mode::Observe > Mode::Normal);
    let strictest = [Mode::Normal, Mode::Freeze, Mode::Observe, Mode::Maintain]
        .into_iter()
        .max();
    assert_eq!(strictest, Some(Mode::Freeze));
}

#[test]
fn test_eval_context_field_set_pins_the_explicit_inputs_only() {
    // §8-一F-1: the predicate context is assembled from explicit inputs
    // only - exhaustive destructuring compile-pins that no implicit
    // source can hide in an extra field.
    let EvalContext {
        principal,
        resource,
        capability,
        objects,
        now,
        mode,
    } = EvalContext {
        principal: pid(7),
        resource: rc("db-main"),
        capability: Capability::Query,
        objects: vec![obj("table:orders")],
        now: Timestamp::from_unix_ms(1_767_225_600_000),
        mode: Mode::Normal,
    };
    assert_eq!(principal, pid(7));
    assert_eq!(resource, rc("db-main"));
    assert_eq!(capability, Capability::Query);
    assert_eq!(objects, vec![obj("table:orders")]);
    assert_eq!(now.as_unix_ms(), 1_767_225_600_000);
    assert_eq!(mode, Mode::Normal);
}

// ---------------------------------------------------------------------------
// PolicySnapshot / CredentialView.
// ---------------------------------------------------------------------------

#[test]
fn test_policy_snapshot_default_is_the_empty_deny_world_with_the_design_field_set() {
    // §8-一F-1: exhaustive destructuring compile-pins the snapshot field
    // set (grant index, tier declarations, credential view, deny notes,
    // grantable set, policy_rev); the default value grants nothing.
    let PolicySnapshot {
        policy_rev,
        grants,
        tiers,
        credentials,
        deny_notes,
        grantable,
    } = PolicySnapshot::default();
    assert_eq!(policy_rev, 0);
    assert_eq!(grants.len(), 0, "empty snapshot grants nothing (axiom one)");
    assert_eq!(tiers.len(), 0);
    assert_eq!(credentials, CredentialView::default());
    assert_eq!(credentials.credentials.len(), 0);
    assert_eq!(deny_notes.len(), 0);
    assert_eq!(grantable.len(), 0);
}

#[test]
fn test_credential_meta_field_set_is_metadata_plus_secret_hash_only() {
    // §8-一F-1: the credential view holds metadata and the secret HASH -
    // exhaustive destructuring compile-pins that no plaintext field exists.
    let CredentialMeta {
        principal,
        kind,
        secret_hash,
        expires_at,
        revoked_at,
    } = CredentialMeta {
        principal: pid(11),
        kind: "api_key".to_string(),
        secret_hash: "sha256:2c26b46b68ffc68ff99b453c1d304134".to_string(),
        expires_at: Some(Timestamp::from_unix_ms(1_767_225_600_000)),
        revoked_at: None,
    };
    assert_eq!(principal, pid(11));
    assert_eq!(kind, "api_key");
    assert_eq!(secret_hash, "sha256:2c26b46b68ffc68ff99b453c1d304134");
    assert_eq!(
        expires_at,
        Some(Timestamp::from_unix_ms(1_767_225_600_000))
    );
    assert_eq!(revoked_at, None);
}

#[test]
fn test_tier_decl_field_set_pins_tier_name_and_carried_verbs() {
    // §8-一F-1: a tier declaration is exactly "which tier" plus "which
    // verbs it carries" (the step [6] selection source).
    let TierDecl { tier, carries } = TierDecl {
        tier: CredentialTier::new("readonly"),
        carries: vec![Capability::Observe, Capability::Query],
    };
    assert_eq!(tier, CredentialTier::new("readonly"));
    assert_eq!(carries, vec![Capability::Observe, Capability::Query]);
}

#[test]
fn test_source_snapshot_and_decision_containers_never_use_a_hash_map() {
    // §8-一F-1 (determinism): hashed iteration order would break
    // byte-identical traces and serializations - BTreeMap/Vec only.
    for (label, src) in UNIT_SOURCES {
        let stripped = strip_line_comments(src);
        assert!(
            !stripped.contains("HashMap"),
            "{label} must use deterministic containers, found a hash map"
        );
    }
}
