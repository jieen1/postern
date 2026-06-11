//! Contract-wiring build script.
//!
//! Computes the violation counts that the generated Stele tests
//! (tests/contract/test_contract.rs) read via `.stele_fixture.json`.
//! Counts are recomputed from the real workspace on every build — never
//! hard-coded — and every scanner is also run against embedded
//! counterexamples (including known bypass shapes) so the *_TEETH invariants
//! prove the scanner still detects violations.
//!
//! Std-only on purpose: build scripts cannot use dev-dependencies.
//!
//! Hardening notes (text-level scanning is conservative — over-report is
//! acceptable, under-report is not; the exit for a false positive is to fix
//! the code or file a protected exemption, never to loosen a scanner):
//!   - SQL matching folds whitespace, strips comments, and is case-insensitive.
//!   - Path predicates compare anchored, slash-bounded segments, not bare substrings.
//!   - Dependency scanning is TOML section-aware (handles [dependencies.x] table
//!     headers, `package = "x"` renames, and the workspace root manifest).

use std::fs;
use std::path::Path;

const BASE_COLS: [&str; 8] = [
    "id", "version", "created_at", "created_by", "updated_at", "updated_by", "delete_flag",
    "enable_flag",
];

/// id libraries banned in favour of the unified snowflake IdGen (5.1-⑥/D8).
const BANNED_ID_CRATES: [&str; 8] =
    ["uuid", "ulid", "nanoid", "cuid", "xid", "sqids", "ksuid", "scru128"];

/// Forbidden workspace dependency edges (detailed design 3.2). Each entry:
/// (consumer crate, [forbidden dependency crates]).
const FORBIDDEN_EDGES: [(&str, &[&str]); 4] = [
    ("postern-adapters", &["postern-secrets", "postern-transports", "postern-store"]),
    ("postern-transports", &["postern-store"]),
    ("postern-cli", &["postern-store", "postern-secrets"]),
    ("postern-core", &[
        "postern-store",
        "postern-secrets",
        "postern-transports",
        "postern-adapters",
        "postern-daemon",
    ]),
];

type SrcFile = (String, String); // (normalized path, content)

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-changed=contract/sql-exceptions.json");
    // Watch crates/ and every subdirectory so a new nested source file retriggers
    // the scan (a bare directory watch misses deep additions — contracts SEC-7).
    rerun_dirs(Path::new("crates"));

    let sql = collect("crates", &|p| p.ends_with(".sql"));
    let store_sql: Vec<SrcFile> =
        sql.iter().filter(|(p, _)| in_store(p)).cloned().collect();
    let rs = collect("crates", &|p| p.ends_with(".rs"));
    // Manifests include the workspace root Cargo.toml (workspace.dependencies live
    // there) plus every crate manifest (contracts SEC-6).
    let mut manifests = collect("crates", &|p| p.ends_with("Cargo.toml"));
    if let Ok(root) = fs::read_to_string("Cargo.toml") {
        manifests.push(("Cargo.toml".to_string(), root));
    }
    let exceptions = exception_files("contract/sql-exceptions.json");

    let rule = |violations: usize, selftest: usize| {
        format!(
            r#"{{"Map":{{"violation_count":{{"Int":{violations}}},"selftest_violation_count":{{"Int":{selftest}}}}}}}"#
        )
    };

    let rules: Vec<(&str, String)> = vec![
        // ---- DB-layer rules (7) ----
        ("base_fields", rule(
            scan_base_fields(&store_sql, &rs),
            scan_base_fields(&fix(&[("crates/postern-store/src/schema.sql", FIX_BAD_TABLE)]), &[]),
        )),
        ("write_path", rule(
            scan_write_path(&rs),
            scan_write_path(&fix_write_path()),
        )),
        ("logical_delete", rule(
            scan_logical_delete(&rs, &sql),
            scan_logical_delete(&fix_delete(), &[]),
        )),
        ("default_scope", rule(
            scan_default_scope(&rs, &sql),
            scan_default_scope(&fix_unscoped(), &[]),
        )),
        ("raw_sql", rule(
            scan_raw_sql(&rs, &exceptions),
            scan_raw_sql(&fix_raw_sql(), &exceptions),
        )),
        ("pagination", rule(
            scan_pagination(&rs, &sql),
            scan_pagination(&fix_unbounded(), &[]),
        )),
        ("id_generator", rule(
            scan_id_generator(&manifests),
            scan_id_generator(&fix_id_dep()),
        )),
        // ---- Security-core rules (5, SUG-1) ----
        ("admin_not_grantable", rule(
            scan_admin(&rs, &sql),
            scan_admin(&fix_admin(), &[]),
        )),
        ("secret_type_discipline", rule(
            scan_secret_derives(&rs),
            scan_secret_derives(&fix_secret_derive()),
        )),
        ("construction_sites", rule(
            scan_construction_sites(&rs),
            scan_construction_sites(&fix_construction()),
        )),
        ("forbidden_edges", rule(
            scan_forbidden_edges(&manifests),
            scan_forbidden_edges(&fix_forbidden_edge()),
        )),
        ("error_swallowing", rule(
            scan_error_swallow(&rs),
            scan_error_swallow(&fix_error_swallow()),
        )),
    ];

    let db_layer = rules
        .iter()
        .map(|(k, v)| format!(r#""{k}":{v}"#))
        .collect::<Vec<_>>()
        .join(",");
    let fixture = format!(r#"{{"Map":{{"db_layer":{{"Map":{{{db_layer}}}}}}}}}"#);
    fs::write(".stele_fixture.json", fixture).expect("write .stele_fixture.json");

    // Meta-gate (ENG-1): if the generated test file exists, every (assert ...)
    // invariant must translate to a real assert!(...). A Stele backend that
    // silently regresses to `let _ = ...;` makes all assert invariants vacuous;
    // fail the build closed rather than rely on a human noticing.
    meta_gate_assert_count();
}

// ============================================================ SQL scanners

/// DB_BASE_FIELDS_REQUIRED: every CREATE TABLE (in .sql or in-store Rust string)
/// declares all 8 base columns as word-bounded tokens.
fn scan_base_fields(store_sql: &[SrcFile], rs: &[SrcFile]) -> usize {
    let mut blocks: Vec<String> = Vec::new();
    for (_, c) in store_sql {
        blocks.extend(create_table_blocks(c));
    }
    // Rust-string CREATE TABLE inside the store crate must also comply (SEC-9).
    for (p, c) in rs.iter().filter(|(p, _)| in_store(p)) {
        let _ = p;
        blocks.extend(create_table_blocks(c));
    }
    blocks
        .iter()
        .filter(|b| BASE_COLS.iter().any(|col| !has_column(b, col)))
        .count()
}

/// DB_WRITE_PATH_CENTRALIZED: INSERT/UPDATE SQL only inside postern-store/src/base/.
fn scan_write_path(files: &[SrcFile]) -> usize {
    files
        .iter()
        .filter(|(p, c)| {
            !in_store_base(p) && {
                let sql = sql_norm(c);
                sql.contains("INSERT INTO") || (sql.contains("UPDATE ") && sql.contains(" SET "))
            }
        })
        .count()
}

/// DB_LOGICAL_DELETE_ONLY: "DELETE FROM" is forbidden everywhere, no exceptions.
fn scan_logical_delete(rs: &[SrcFile], sql: &[SrcFile]) -> usize {
    rs.iter()
        .chain(sql.iter())
        .filter(|(_, c)| sql_norm(c).contains("DELETE FROM"))
        .count()
}

/// DB_DEFAULT_SCOPE_EXCLUDES_DELETED: SELECT statements in store (outside base/)
/// must carry an explicit `delete_flag = 0` predicate.
fn scan_default_scope(rs: &[SrcFile], sql: &[SrcFile]) -> usize {
    let mut count = 0;
    for (p, c) in rs.iter().chain(sql.iter()) {
        if !in_store(p) || in_store_base(p) {
            continue;
        }
        for stmt in sql_statements(c) {
            if stmt.contains("SELECT ") && !stmt.contains("DELETE_FLAG = 0") {
                count += 1;
            }
        }
    }
    count
}

/// DB_NO_RAW_SQL_OUTSIDE_STORE: SQL markers / rusqlite outside postern-store,
/// unless the file is registered (by exact normalized path) in
/// contract/sql-exceptions.json.
fn scan_raw_sql(files: &[SrcFile], exceptions: &[String]) -> usize {
    const MARKERS: [&str; 6] =
        ["SELECT ", "INSERT INTO", "UPDATE ", "DELETE FROM", "CREATE TABLE", "RUSQLITE"];
    files
        .iter()
        .filter(|(p, c)| {
            !in_store(p)
                && !exceptions.iter().any(|e| e == p.as_str())
                && {
                    let sql = sql_norm(c);
                    MARKERS.iter().any(|m| sql.contains(m))
                }
        })
        .count()
}

/// DB_PAGINATION_MANDATORY: collection SELECT statements (not single-row by id,
/// not COUNT) must be LIMIT-bounded.
fn scan_pagination(rs: &[SrcFile], sql: &[SrcFile]) -> usize {
    let mut count = 0;
    for (_, c) in rs.iter().chain(sql.iter()) {
        for stmt in sql_statements(c) {
            if stmt.contains("SELECT ")
                && !stmt.contains("LIMIT")
                && !stmt.contains("COUNT(")
                && !stmt.contains("WHERE ID = ?")
            {
                count += 1;
            }
        }
    }
    count
}

// ============================================================ dependency scanners

/// DB_UNIFIED_ID_GENERATOR: no banned id-crate dependency, in any manifest,
/// in any form (key, [dependencies.<name>] header, or `package = "<name>"` rename).
fn scan_id_generator(manifests: &[SrcFile]) -> usize {
    let mut count = 0;
    for (_, c) in manifests {
        for dep in toml_dependencies(c) {
            if BANNED_ID_CRATES.contains(&dep.as_str()) {
                count += 1;
            }
        }
    }
    count
}

/// ARCH_FORBIDDEN_EDGES: a crate must not declare a dependency the architecture
/// forbids (detailed design 3.2).
fn scan_forbidden_edges(manifests: &[SrcFile]) -> usize {
    let mut count = 0;
    for (path, content) in manifests {
        let Some(consumer) = crate_name_of(path, content) else { continue };
        let Some((_, forbidden)) = FORBIDDEN_EDGES.iter().find(|(c, _)| *c == consumer) else {
            continue;
        };
        for dep in toml_dependencies(content) {
            if forbidden.contains(&dep.as_str()) {
                count += 1;
            }
        }
    }
    count
}

// ============================================================ Rust source scanners

/// ADMIN_NOT_GRANTABLE: the Capability enum has no Admin variant, and the roles
/// table pins a CHECK forbidding the admin role name.
fn scan_admin(rs: &[SrcFile], sql: &[SrcFile]) -> usize {
    let mut count = 0;
    // (a) Capability enum must not contain an Admin variant.
    for (_, c) in rs {
        if let Some(body) = enum_body(c, "Capability") {
            if word_present(&body, "Admin") {
                count += 1;
            }
        }
    }
    // (b) roles CREATE TABLE must carry a CHECK that bans the admin name.
    for (_, c) in rs.iter().chain(sql.iter()) {
        for block in create_table_blocks(c) {
            if block.contains("CREATE TABLE ROLES")
                && !(block.contains("CHECK") && block.contains("ADMIN"))
            {
                count += 1;
            }
        }
    }
    count
}

/// SECRET_TYPE_DISCIPLINE: ResolvedTarget / ResourceCredential must not derive
/// Clone or Serialize (their values must never be copied or serialized).
fn scan_secret_derives(rs: &[SrcFile]) -> usize {
    const SECRET_TYPES: [&str; 2] = ["ResolvedTarget", "ResourceCredential"];
    let mut count = 0;
    for (_, c) in rs {
        let lines: Vec<&str> = c.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            let def = line.trim_start();
            let is_def = SECRET_TYPES.iter().any(|t| {
                def.starts_with(&format!("struct {t}"))
                    || def.starts_with(&format!("pub struct {t}"))
                    || def.starts_with(&format!("enum {t}"))
                    || def.starts_with(&format!("pub enum {t}"))
            });
            if !is_def {
                continue;
            }
            // Inspect the few lines above for a derive attribute.
            let start = i.saturating_sub(4);
            let preamble = lines[start..i].join(" ");
            if preamble.contains("derive")
                && (preamble.contains("Clone") || preamble.contains("Serialize"))
            {
                count += 1;
            }
        }
    }
    count
}

/// CONSTRUCTION_SITES: ConnOrigin is constructed only in daemon shells (listener);
/// ResolvedTarget / ResourceCredential are constructed only in postern-secrets.
fn scan_construction_sites(rs: &[SrcFile]) -> usize {
    let mut count = 0;
    for (p, c) in rs {
        let body = strip_line_comments_rs(c);
        // ConnOrigin: only the listener layer may construct it.
        if !in_daemon_shells(p)
            && (body.contains("ConnOrigin::UnixPeer") || body.contains("ConnOrigin::Tcp"))
        {
            count += 1;
        }
        // ResolvedTarget / ResourceCredential: only postern-secrets may construct.
        if !in_secrets(p) {
            for t in ["ResolvedTarget", "ResourceCredential"] {
                if body.contains(&format!("{t} {{")) || body.contains(&format!("{t}::new")) {
                    count += 1;
                }
            }
        }
    }
    count
}

/// EVAL_NO_ERROR_SWALLOWING: the evaluation path (core::eval, daemon::kernel)
/// must not swallow errors into an allow/default.
fn scan_error_swallow(rs: &[SrcFile]) -> usize {
    const BAD: [&str; 3] = [".unwrap_or(true)", ".ok()", ".unwrap_or_default()"];
    let mut count = 0;
    for (p, c) in rs {
        if !on_eval_path(p) {
            continue;
        }
        let body = strip_line_comments_rs(c);
        for pat in BAD {
            count += body.matches(pat).count();
        }
    }
    count
}

// ============================================================ meta-gate

fn meta_gate_assert_count() {
    let gen = match fs::read_to_string("tests/contract/test_contract.rs") {
        Ok(s) => s,
        Err(_) => return, // not generated yet — nothing to verify
    };
    let mut declared = 0usize;
    for src in ["contract/main.stele", "contract/proposals/agent-additions.stele"] {
        if let Ok(s) = fs::read_to_string(src) {
            // Strip CDL comments (';' to end of line) so commented example
            // invariants are not miscounted as declared assertions.
            for line in s.lines() {
                let code = match line.find(';') {
                    Some(idx) => &line[..idx],
                    None => line,
                };
                declared += code.matches("(assert ").count();
            }
        }
    }
    let emitted = gen.matches("assert!(").count();
    if declared > 0 && emitted < declared {
        panic!(
            "Stele backend regression: {declared} (assert ...) invariants but only {emitted} \
             assert!(...) in generated tests — generator is discarding assertions (vacuous pass). \
             Upgrade/patch the stele backend-rust translator before trusting these contracts."
        );
    }
}

// ============================================================ counterexamples
// Permanently embedded known-bad inputs, including bypass shapes the hardened
// scanners must still catch (contracts SUG-2). Each *_TEETH invariant asserts
// its scanner flags these; a scanner that stops detecting them turns red.

const FIX_BAD_TABLE: &str = "CREATE TABLE bad (id INTEGER PRIMARY KEY, name TEXT);\n\
    create table sneaky ( -- id, version, created_at, created_by, updated_at, updated_by, delete_flag, enable_flag\n  who TEXT\n);";

fn fix(pairs: &[(&str, &str)]) -> Vec<SrcFile> {
    pairs.iter().map(|(p, c)| (p.to_string(), c.to_string())).collect()
}
fn fix_write_path() -> Vec<SrcFile> {
    fix(&[
        // plain, lowercase, and prefix-path (baseball) bypass attempts
        ("crates/postern-daemon/src/x.rs", r#"q("INSERT INTO roles VALUES (?1)")"#),
        ("crates/postern-cli/src/y.rs", r#"q("insert into roles values (?1)")"#),
        ("crates/postern-store/src/baseball/z.rs", r#"q("INSERT INTO roles VALUES (?1)")"#),
    ])
}
fn fix_delete() -> Vec<SrcFile> {
    fix(&[
        ("crates/postern-store/src/base/a.rs", r#"q("DELETE FROM principals WHERE id = ?1")"#),
        ("crates/postern-store/src/base/b.rs", "q(\"delete\nfrom principals\")"), // lowercase + newline
    ])
}
fn fix_unscoped() -> Vec<SrcFile> {
    fix(&[(
        "crates/postern-store/src/policy/p.rs",
        "q(\"SELECT id, name FROM principals WHERE kind = ?1\") // delete_flag handled elsewhere",
    )])
}
fn fix_raw_sql() -> Vec<SrcFile> {
    fix(&[
        ("crates/postern-cli/src/c.rs", r#"q("select value from settings")"#),
        ("crates/postern-adapters/src/d.rs", "use rusqlite::Connection;"),
    ])
}
fn fix_unbounded() -> Vec<SrcFile> {
    fix(&[(
        "crates/postern-store/src/base/u.rs",
        r#"q("SELECT id FROM temp_grants WHERE principal_id = ?1")"#,
    )])
}
fn fix_id_dep() -> Vec<SrcFile> {
    fix(&[
        ("crates/postern-core/Cargo.toml", "[dependencies.uuid]\nversion = \"1\""), // table header
        ("crates/postern-x/Cargo.toml", "[dependencies]\nmyid = { package = \"ulid\" }"), // rename
        ("Cargo.toml", "[workspace.dependencies]\nnanoid = \"0.4\""), // root workspace
    ])
}
fn fix_admin() -> Vec<SrcFile> {
    fix(&[
        ("crates/postern-core/src/cap.rs", "pub enum Capability { Observe, Admin, Destroy }"),
        ("crates/postern-store/src/schema.sql", "CREATE TABLE roles (id INTEGER, name TEXT);"),
    ])
}
fn fix_secret_derive() -> Vec<SrcFile> {
    fix(&[(
        "crates/postern-secrets/src/types.rs",
        "#[derive(Clone, Debug)]\npub struct ResolvedTarget { host: String }",
    )])
}
fn fix_construction() -> Vec<SrcFile> {
    fix(&[
        ("crates/postern-adapters/src/leak.rs", "let t = ResolvedTarget { host: h };"),
        ("crates/postern-daemon/src/kernel/k.rs", "let o = ConnOrigin::Tcp { remote };"),
    ])
}
fn fix_forbidden_edge() -> Vec<SrcFile> {
    fix(&[(
        "crates/postern-adapters/Cargo.toml",
        "[package]\nname = \"postern-adapters\"\n[dependencies]\npostern-secrets = { path = \"../postern-secrets\" }",
    )])
}
fn fix_error_swallow() -> Vec<SrcFile> {
    fix(&[(
        "crates/postern-core/src/eval/e.rs",
        "let ok = check(req).unwrap_or(true);",
    )])
}

// ============================================================ text helpers

/// Uppercase + fold whitespace + strip SQL comments, for case-insensitive,
/// layout-insensitive keyword matching.
fn sql_norm(s: &str) -> String {
    fold_ws(&strip_sql_comments(s)).to_ascii_uppercase()
}

/// Split normalized SQL into statements on ';' for per-statement predicates.
fn sql_statements(s: &str) -> Vec<String> {
    sql_norm(s).split(';').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect()
}

fn strip_sql_comments(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if i + 1 < b.len() && b[i] == b'-' && b[i + 1] == b'-' {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
        } else if i + 1 < b.len() && b[i] == b'/' && b[i + 1] == b'*' {
            i += 2;
            while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                i += 1;
            }
            i += 2;
        } else {
            out.push(b[i] as char);
            i += 1;
        }
    }
    out
}

fn strip_line_comments_rs(s: &str) -> String {
    s.lines()
        .map(|l| match l.find("//") {
            Some(idx) => &l[..idx],
            None => l,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn fold_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Extract each `CREATE TABLE ...(...)` block as normalized SQL.
fn create_table_blocks(s: &str) -> Vec<String> {
    let norm = sql_norm(s);
    let mut blocks = Vec::new();
    let mut rest = norm.as_str();
    while let Some(start) = rest.find("CREATE TABLE") {
        // block ends at the next CREATE TABLE or end of string; bound the column
        // list at the matching ");" if present, else take the remainder.
        let after = &rest[start..];
        let next = after[1..].find("CREATE TABLE").map(|x| x + 1).unwrap_or(after.len());
        let segment = &after[..next];
        let end = segment.find(");").map(|x| x + 1).unwrap_or(segment.len());
        blocks.push(segment[..end].to_string());
        rest = &after[next..];
    }
    blocks
}

/// Word-bounded column presence inside an (already uppercased) CREATE TABLE block,
/// ignoring the parenthesized comment-free body. `col` is lowercase.
fn has_column(block: &str, col: &str) -> bool {
    word_present(block, &col.to_ascii_uppercase())
}

/// True if `word` appears in `hay` with non-alphanumeric/underscore boundaries.
fn word_present(hay: &str, word: &str) -> bool {
    let hb = hay.as_bytes();
    let wb = word.as_bytes();
    if wb.is_empty() {
        return false;
    }
    let mut i = 0;
    while let Some(pos) = hay[i..].find(word) {
        let at = i + pos;
        let before_ok = at == 0 || !is_ident(hb[at - 1]);
        let after = at + wb.len();
        let after_ok = after >= hb.len() || !is_ident(hb[after]);
        if before_ok && after_ok {
            return true;
        }
        i = at + 1;
        if i >= hay.len() {
            break;
        }
    }
    false
}

fn is_ident(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Body of a Rust `enum <name> { ... }` (best-effort, first match).
fn enum_body(s: &str, name: &str) -> Option<String> {
    let needle = format!("enum {name}");
    let start = s.find(&needle)?;
    let brace = s[start..].find('{')? + start;
    let mut depth = 0i32;
    let bytes = s.as_bytes();
    let mut i = brace;
    while i < bytes.len() {
        match bytes[i] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(s[brace + 1..i].to_string());
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

// ============================================================ path helpers
// Anchored, slash-bounded segment checks (contracts SEC-8) — never bare contains.

fn in_store(p: &str) -> bool {
    p.contains("crates/postern-store/")
}
fn in_store_base(p: &str) -> bool {
    p.contains("crates/postern-store/src/base/")
}
fn in_secrets(p: &str) -> bool {
    p.contains("crates/postern-secrets/")
}
fn in_daemon_shells(p: &str) -> bool {
    p.contains("crates/postern-daemon/src/shells/")
}
fn on_eval_path(p: &str) -> bool {
    p.contains("crates/postern-core/src/eval/") || p.contains("crates/postern-daemon/src/kernel/")
}

// ============================================================ TOML helpers

/// Crate name declared in a `[package] name = "..."` (or workspace root → None).
fn crate_name_of(path: &str, content: &str) -> Option<String> {
    let mut in_package = false;
    for line in content.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            in_package = t == "[package]";
            continue;
        }
        if in_package && t.starts_with("name") {
            if let Some(v) = toml_string_value(t) {
                return Some(v);
            }
        }
    }
    // Fall back to the directory name for crate manifests.
    if path.ends_with("Cargo.toml") && path.contains("crates/") {
        return path
            .strip_suffix("/Cargo.toml")
            .and_then(|d| d.rsplit('/').next())
            .map(|s| s.to_string());
    }
    None
}

/// All dependency crate names declared in a manifest, across [dependencies],
/// [dev-dependencies], [build-dependencies], [workspace.dependencies], and
/// [<...>.dependencies.<name>] table headers, resolving `package = "x"` renames.
fn toml_dependencies(content: &str) -> Vec<String> {
    let mut deps = Vec::new();
    let mut section = String::new();
    let mut header_dep: Option<String> = None; // name from [..dependencies.<name>]
    for line in content.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        if let Some(inner) = t.strip_prefix('[').and_then(|x| x.strip_suffix(']')) {
            section = inner.to_string();
            header_dep = None;
            // [dependencies.foo] / [workspace.dependencies.foo] / [target.'..'.dependencies.foo]
            if let Some(idx) = inner.find("dependencies.") {
                let name = &inner[idx + "dependencies.".len()..];
                let name = name.trim_matches('"').trim_matches('\'');
                if !name.is_empty() {
                    header_dep = Some(name.to_string());
                    // a rename inside the table body is handled below
                    deps.push(name.to_string());
                }
            }
            continue;
        }
        let is_dep_section = section.ends_with("dependencies");
        if is_dep_section && header_dep.is_none() {
            // `name = ...` or `name.workspace = true`
            if let Some(key) = t.split(['=', '.']).next() {
                let key = key.trim();
                if !key.is_empty() && !key.starts_with('#') {
                    deps.push(key.to_string());
                }
            }
        }
        // `package = "real"` rename target, in either inline or table form.
        if t.starts_with("package") {
            if let Some(v) = toml_string_value(t) {
                deps.push(v);
            }
        }
        // inline rename: `myid = { package = "uuid", ... }`
        if let Some(pkg) = inline_package_rename(t) {
            deps.push(pkg);
        }
    }
    deps
}

fn toml_string_value(line: &str) -> Option<String> {
    let after = line.split('=').nth(1)?;
    let s = after.trim().trim_matches(',').trim();
    let s = s.trim_matches('"').trim_matches('\'');
    if s.is_empty() {
        None
    } else {
        Some(s.to_string())
    }
}

fn inline_package_rename(line: &str) -> Option<String> {
    let idx = line.find("package")?;
    let after = &line[idx..];
    let eq = after.find('=')?;
    let rest = after[eq + 1..].trim_start();
    let rest = rest.strip_prefix('"').or_else(|| rest.strip_prefix('\''))?;
    let end = rest.find(['"', '\''])?;
    Some(rest[..end].to_string())
}

/// Extract the "file" string values from contract/sql-exceptions.json.
/// Std-only; only reads values whose key is exactly "file".
fn exception_files(path: &str) -> Vec<String> {
    let Ok(raw) = fs::read_to_string(path) else { return Vec::new() };
    let mut out = Vec::new();
    let mut rest = raw.as_str();
    while let Some(k) = rest.find("\"file\"") {
        rest = &rest[k + 6..];
        let Some(colon) = rest.find(':') else { break };
        rest = &rest[colon + 1..];
        let Some(q1) = rest.find('"') else { break };
        rest = &rest[q1 + 1..];
        let Some(q2) = rest.find('"') else { break };
        out.push(rest[..q2].to_string());
        rest = &rest[q2 + 1..];
    }
    out
}

// ============================================================ fs helpers

fn rerun_dirs(dir: &Path) {
    if dir.is_dir() {
        println!("cargo:rerun-if-changed={}", dir.to_string_lossy());
        if let Ok(entries) = fs::read_dir(dir) {
            for e in entries.flatten() {
                let p = e.path();
                let name = e.file_name().to_string_lossy().into_owned();
                if p.is_dir() && name != "target" && !name.starts_with('.') {
                    rerun_dirs(&p);
                }
            }
        }
    }
}

fn collect(root: &str, keep: &dyn Fn(&str) -> bool) -> Vec<SrcFile> {
    let mut out = Vec::new();
    walk(Path::new(root), keep, &mut out);
    out.sort();
    out
}

fn walk(dir: &Path, keep: &dyn Fn(&str) -> bool, out: &mut Vec<SrcFile>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if path.is_dir() {
            if name != "target" && !name.starts_with('.') {
                walk(&path, keep, out);
            }
        } else {
            let p = path.to_string_lossy().replace('\\', "/");
            if keep(&p) {
                if let Ok(content) = fs::read_to_string(&path) {
                    out.push((p, content));
                }
            }
        }
    }
}
