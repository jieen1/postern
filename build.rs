//! Contract-wiring build script.
//!
//! Computes the db-layer violation counts that the generated Stele tests
//! (tests/contract/test_contract.rs) read via `.stele_fixture.json`.
//! Counts are recomputed from the real workspace on every build — never
//! hard-coded — and every scanner is also run against an embedded
//! counterexample fixture so the *_TEETH invariants prove the scanner
//! itself still detects violations.
//!
//! Std-only on purpose: build scripts cannot use dev-dependencies.

use std::fs;
use std::path::Path;

const BASE_COLS: [&str; 8] = [
    "id", "version", "created_at", "created_by", "updated_at", "updated_by", "delete_flag",
    "enable_flag",
];

type SrcFile = (String, String); // (normalized path, content)

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=crates");
    println!("cargo:rerun-if-changed=contract/sql-exceptions.json");

    let sql = collect("crates/postern-store/src", &|p| p.ends_with(".sql"));
    let rs = collect("crates", &|p| p.ends_with(".rs"));
    let manifests = collect("crates", &|p| p.ends_with("Cargo.toml"));
    for (p, _) in sql.iter().chain(rs.iter()).chain(manifests.iter()) {
        println!("cargo:rerun-if-changed={p}");
    }
    let exceptions = exception_files("contract/sql-exceptions.json");

    let schema_all = sql.iter().map(|(_, c)| c.as_str()).collect::<Vec<_>>().join("\n");

    // SteleValue derives a plain (externally tagged) serde::Deserialize,
    // so the fixture must use the tagged representation: {"Map":{...}}, {"Int":n}.
    let rule = |violations: usize, selftest: usize| {
        format!(
            r#"{{"Map":{{"violation_count":{{"Int":{violations}}},"selftest_violation_count":{{"Int":{selftest}}}}}}}"#
        )
    };
    let rules = [
        ("base_fields", rule(scan_base_fields(&schema_all), scan_base_fields(FIX_BAD_TABLE))),
        ("write_path", rule(scan_write_path(&rs), scan_write_path(&fix_write_path()))),
        ("logical_delete", rule(scan_logical_delete(&rs, &sql), scan_logical_delete(&fix_delete(), &[]))),
        ("default_scope", rule(scan_default_scope(&rs), scan_default_scope(&fix_unscoped()))),
        ("raw_sql", rule(scan_raw_sql(&rs, &exceptions), scan_raw_sql(&fix_raw_sql(), &exceptions))),
        ("pagination", rule(scan_pagination(&rs), scan_pagination(&fix_unbounded()))),
        ("id_generator", rule(scan_id_generator(&manifests), scan_id_generator(&fix_uuid_dep()))),
    ];
    let db_layer = rules
        .iter()
        .map(|(k, v)| format!(r#""{k}":{v}"#))
        .collect::<Vec<_>>()
        .join(",");
    let fixture = format!(r#"{{"Map":{{"db_layer":{{"Map":{{{db_layer}}}}}}}}}"#);
    fs::write(".stele_fixture.json", fixture).expect("write .stele_fixture.json");
}

// ---------------------------------------------------------------- scanners
// One scanner per rule; the same function runs on the real workspace AND on
// the embedded counterexample, so the rule and its teeth cannot diverge.
// Text-level and deliberately conservative: false positives are acceptable,
// missed violations are not (fail-closed). Exemptions only via the protected
// contract/sql-exceptions.json.

/// DB_BASE_FIELDS_REQUIRED: every CREATE TABLE block declares all 8 base columns.
fn scan_base_fields(sql: &str) -> usize {
    let mut violations = 0;
    let mut rest = sql;
    while let Some(start) = rest.find("CREATE TABLE") {
        let block_end = rest[start..].find(");").map(|e| start + e).unwrap_or(rest.len());
        let block = &rest[start..block_end];
        if BASE_COLS.iter().any(|col| !block.contains(col)) {
            violations += 1;
        }
        rest = &rest[block_end..];
        if rest.len() <= 2 {
            break;
        }
        rest = &rest[2..];
    }
    violations
}

/// DB_WRITE_PATH_CENTRALIZED: INSERT/UPDATE SQL only inside postern-store/src/base/.
fn scan_write_path(files: &[SrcFile]) -> usize {
    files
        .iter()
        .filter(|(p, c)| {
            !p.contains("postern-store/src/base")
                && (c.contains("INSERT INTO") || (c.contains("UPDATE ") && c.contains(" SET ")))
        })
        .count()
}

/// DB_LOGICAL_DELETE_ONLY: "DELETE FROM" is forbidden everywhere, no exceptions.
fn scan_logical_delete(rs: &[SrcFile], sql: &[SrcFile]) -> usize {
    rs.iter().chain(sql.iter()).filter(|(_, c)| c.contains("DELETE FROM")).count()
}

/// DB_DEFAULT_SCOPE_EXCLUDES_DELETED: SELECTs in store (outside base/) must
/// carry a delete_flag predicate on the same line.
fn scan_default_scope(files: &[SrcFile]) -> usize {
    files
        .iter()
        .filter(|(p, _)| p.contains("postern-store/src") && !p.contains("postern-store/src/base"))
        .flat_map(|(_, c)| c.lines())
        .filter(|l| l.contains("SELECT ") && !l.contains("delete_flag"))
        .count()
}

/// DB_NO_RAW_SQL_OUTSIDE_STORE: SQL markers / rusqlite outside postern-store,
/// unless the file is registered in contract/sql-exceptions.json.
fn scan_raw_sql(files: &[SrcFile], exceptions: &[String]) -> usize {
    const MARKERS: [&str; 6] =
        ["SELECT ", "INSERT INTO", "UPDATE ", "DELETE FROM", "CREATE TABLE", "rusqlite"];
    files
        .iter()
        .filter(|(p, c)| {
            !p.contains("crates/postern-store/")
                && MARKERS.iter().any(|m| c.contains(m))
                && !exceptions.iter().any(|e| p.ends_with(e.as_str()))
        })
        .count()
}

/// DB_PAGINATION_MANDATORY: collection SELECTs (not single-row by id) must be
/// LIMIT-bounded on the same line.
fn scan_pagination(files: &[SrcFile]) -> usize {
    files
        .iter()
        .flat_map(|(_, c)| c.lines())
        .filter(|l| {
            l.contains("SELECT ")
                && !l.contains("LIMIT")
                && !l.contains("COUNT(")
                && !l.contains("WHERE id = ?")
                && !l.contains("WHERE id=?")
        })
        .count()
}

/// DB_UNIFIED_ID_GENERATOR: no uuid/ulid/nanoid dependency declarations.
fn scan_id_generator(manifests: &[SrcFile]) -> usize {
    manifests
        .iter()
        .flat_map(|(_, c)| c.lines())
        .map(str::trim)
        .filter(|l| {
            l.starts_with("uuid") || l.starts_with("ulid") || l.starts_with("nanoid")
        })
        .count()
}

// ------------------------------------------------- counterexample fixtures
// Permanently embedded known-bad inputs. The *_TEETH invariants assert the
// scanners flag these; a scanner that stops detecting them turns the contract
// test red immediately.

const FIX_BAD_TABLE: &str = "CREATE TABLE bad (id INTEGER PRIMARY KEY, name TEXT);";

fn fix_write_path() -> Vec<SrcFile> {
    vec![(
        "crates/postern-daemon/src/fixture.rs".into(),
        r#"conn.execute("INSERT INTO roles (name) VALUES (?1)")"#.into(),
    )]
}
fn fix_delete() -> Vec<SrcFile> {
    vec![(
        "crates/postern-store/src/base/fixture.rs".into(),
        r#"conn.execute("DELETE FROM principals WHERE id = ?1")"#.into(),
    )]
}
fn fix_unscoped() -> Vec<SrcFile> {
    vec![(
        "crates/postern-store/src/policy/fixture.rs".into(),
        r#""SELECT id, name FROM principals WHERE kind = ?1 AND delete_flag = 0"
"SELECT id, name FROM principals WHERE kind = ?1""#
            .into(),
    )]
}
fn fix_raw_sql() -> Vec<SrcFile> {
    vec![(
        "crates/postern-cli/src/fixture.rs".into(),
        r#""SELECT value FROM settings""#.into(),
    )]
}
fn fix_unbounded() -> Vec<SrcFile> {
    vec![(
        "crates/postern-store/src/policy/fixture.rs".into(),
        r#""SELECT id FROM temp_grants WHERE principal_id = ?1""#.into(),
    )]
}
fn fix_uuid_dep() -> Vec<SrcFile> {
    vec![("crates/postern-x/Cargo.toml".into(), "[dependencies]\nuuid = \"1\"".into())]
}

// ------------------------------------------------------------------ helpers

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

/// Minimal extractor for the "file" fields of contract/sql-exceptions.json.
/// Std-only by necessity; the file is human-maintained under Stele protection.
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
