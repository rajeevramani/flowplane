//! Guard test: no unaudited SQL deletion path may exist for identity tables whose rows
//! carry or cascade authorization (`teams`, `org_memberships`, `agents`, `users`, `grants`,
//! `user_grants`, `agent_grants`).
//!
//! Deleting any of those rows revokes or re-parents authority, so it must happen inside an
//! audited `fp-core::services` transaction that writes an audit record alongside the delete.
//!
//! Two properties are enforced, and the second is the one with teeth:
//!
//! 1. Only the storage layer's identity repository may *author* the SQL — a handler, service,
//!    worker or CLI writing `DELETE FROM grants` directly bypasses the audit.
//! 2. Within that repository, each such delete must sit in a function that takes the caller's
//!    `Transaction` and does not open its own from a `&PgPool`. A file-level allow-list alone
//!    would be near-vacuous: it would let the three unaudited pool-taking wrappers this slice
//!    removed (`remove_org_membership`, `delete_team`, `delete_grant`) be reinstated silently,
//!    which is precisely the regression this guard exists to prevent.
//!
//! A companion test pins that each of those transaction-taking deletes has exactly ONE
//! production caller — the audited service function — since a second caller would be a second
//! path by which authority is revoked, uncovered by the audit evidence the design rests on.
//!
//! This is a static source scan: no database, no network. It runs (and must not skip) with
//! `FLOWPLANE_TEST_DATABASE_URL` unset.
//!
//! Scope is PRODUCTION code only — `crates/*/src/**/*.rs`. The `crates/*/tests/` trees are
//! never entered, structurally rather than by a name filter. That exemption is load-bearing:
//! a later slice's acceptance criterion *requires* a storage test to issue a direct
//! `DELETE FROM agents` (no production agent-deletion path exists, so the FK cascade can only
//! be proven by direct SQL), and a scan covering test code would contradict it.

#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};

/// Identity tables whose deletion changes who can do what.
const GUARDED_TABLES: &[&str] = &[
    "teams",
    "org_memberships",
    "agents",
    "users",
    "grants",
    "user_grants",
    "agent_grants",
];

/// The single file allowed to author deletes against `GUARDED_TABLES`.
const ALLOWED_FILE: &str = "crates/fp-storage/src/repos/identity.rs";

fn workspace_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is <workspace>/crates/fp-storage.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root two levels above crates/fp-storage")
        .to_path_buf()
}

/// Every `.rs` file under `crates/*/src/`. The `crates/*/tests/` trees are never entered.
fn production_sources(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let crates_dir = root.join("crates");
    let entries = std::fs::read_dir(&crates_dir)
        .unwrap_or_else(|e| panic!("read {}: {e}", crates_dir.display()));
    for entry in entries {
        let krate = entry.expect("crate dir entry").path();
        let src = krate.join("src");
        if src.is_dir() {
            collect_rs(&src, &mut out);
        }
    }
    out.sort();
    out
}

fn collect_rs(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = std::fs::read_dir(dir).unwrap_or_else(|e| panic!("read {}: {e}", dir.display()));
    for entry in entries {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            collect_rs(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// A `DELETE FROM <table>` hit against a guarded table.
struct Hit {
    table: String,
    line_no: usize,
    line: String,
}

/// Scan for `DELETE` <sep> `FROM` <sep> `<table>`, case-insensitively, where <sep> tolerates
/// arbitrary whitespace/newlines and the `\` line-continuations used in multi-line SQL
/// string literals.
fn find_guarded_deletes(text: &str) -> Vec<Hit> {
    let lower = text.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    let mut hits = Vec::new();
    let mut idx = 0usize;

    while let Some(rel) = lower[idx..].find("delete") {
        let start = idx + rel;
        idx = start + "delete".len();

        let mut i = idx;
        if !skip_sep(bytes, &mut i, true) {
            continue;
        }
        if !lower[i..].starts_with("from") {
            continue;
        }
        i += "from".len();
        if !skip_sep(bytes, &mut i, true) {
            continue;
        }
        let Some(table) = read_table(bytes, &mut i) else {
            continue;
        };
        if !GUARDED_TABLES.contains(&table.as_str()) {
            continue;
        }
        let line_no = text[..start].chars().filter(|c| *c == '\n').count() + 1;
        let line = text.lines().nth(line_no - 1).unwrap_or_default().trim();
        hits.push(Hit {
            table,
            line_no,
            line: line.to_string(),
        });
    }
    hits
}

/// Advance past whitespace, `\` continuations and SQL string-literal quote punctuation.
/// Returns false if `require_one` and nothing was skipped (so `deleted`/`fromage` don't match).
fn skip_sep(bytes: &[u8], i: &mut usize, require_one: bool) -> bool {
    let start = *i;
    while *i < bytes.len() && matches!(bytes[*i], b' ' | b'\t' | b'\r' | b'\n' | b'\\') {
        *i += 1;
    }
    !require_one || *i > start
}

/// Read an SQL identifier, tolerating optional double quotes and an optional schema prefix
/// (`public.grants` -> `grants`).
fn read_table(bytes: &[u8], i: &mut usize) -> Option<String> {
    let mut ident = read_ident(bytes, i)?;
    if *i < bytes.len() && bytes[*i] == b'.' {
        *i += 1;
        ident = read_ident(bytes, i)?;
    }
    Some(ident)
}

fn read_ident(bytes: &[u8], i: &mut usize) -> Option<String> {
    if *i < bytes.len() && bytes[*i] == b'"' {
        *i += 1;
    }
    let start = *i;
    while *i < bytes.len() && (bytes[*i].is_ascii_alphanumeric() || bytes[*i] == b'_') {
        *i += 1;
    }
    if *i == start {
        return None;
    }
    let ident = String::from_utf8(bytes[start..*i].to_vec()).ok()?;
    if *i < bytes.len() && bytes[*i] == b'"' {
        *i += 1;
    }
    Some(ident)
}

#[test]
fn identity_deletes_live_only_in_the_audited_storage_repo() {
    let root = workspace_root();
    let sources = production_sources(&root);
    assert!(
        sources.len() > 50,
        "source scan found only {} files under crates/*/src — the walk is broken, not the code",
        sources.len()
    );

    let mut scanned_allowed_file = false;
    let mut violations: Vec<String> = Vec::new();

    for path in &sources {
        let rel = path
            .strip_prefix(&root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        let src = std::fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));

        // NOTE: `#[cfg(test)]` regions are deliberately NOT stripped. An earlier version
        // blanked them by brace counting, which could silently blank *production* code and
        // hide a real violation — a false negative, the one failure direction a security
        // guard must not have. Today no guarded delete sits inside a `#[cfg(test)]` region
        // in any production file (the sole such attribute in a file with guarded deletes is
        // `identity.rs:1009`, after all of them), so scanning raw source costs nothing. If
        // that ever changes, this test fails loudly and a human decides — fail-closed.
        for hit in find_guarded_deletes(&src) {
            if rel != ALLOWED_FILE {
                violations.push(format!(
                    "  {}:{}\n    table:    {}\n    line:     {}\n    problem:  delete authored outside the storage identity repository",
                    rel, hit.line_no, hit.table, hit.line
                ));
                continue;
            }
            scanned_allowed_file = true;

            // Being in the right FILE is not enough. The invariant S1 exists to protect is
            // that these deletes are only reachable through an audited service transaction,
            // so the enclosing function must take a `Transaction` and must NOT take a `&PgPool`
            // of its own. A pool-taking wrapper opens and commits its own transaction, which
            // is exactly how the three functions deleted in this slice revoked authority with
            // no audit row. A file-level allow-list would let them be reinstated silently.
            let Some(func) = enclosing_fn(&src, hit.line_no) else {
                violations.push(format!(
                    "  {}:{}\n    table:    {}\n    problem:  could not determine the enclosing function; \
                     this guard cannot vouch for it",
                    rel, hit.line_no, hit.table
                ));
                continue;
            };
            if !func.takes_transaction || func.takes_pool {
                violations.push(format!(
                    "  {}:{}\n    table:    {}\n    function: {}\n    problem:  deletes a guarded table from a \
                     function that opens its own transaction (takes `&PgPool`) instead of joining the caller's",
                    rel, hit.line_no, hit.table, func.name
                ));
            }
        }
    }

    assert!(
        scanned_allowed_file,
        "expected {ALLOWED_FILE} to contain the guarded deletes — the allow-list target moved, update this test"
    );

    assert!(
        violations.is_empty(),
        "unaudited SQL deletion path(s) found for authorization-bearing identity tables \
         ({}).\n\n{}\n\nDeleting these rows revokes or re-parents authority, so it must happen \
         inside an audited `fp-core::services` transaction that records the revocation alongside \
         the delete. Author the SQL in {ALLOWED_FILE} in a function that takes the caller's \
         `Transaction`, and call it from the audited service path.",
        GUARDED_TABLES.join(", "),
        violations.join("\n\n"),
    );
}

/// The three transaction-taking deletes, and the single audited service function allowed to
/// call each. Pinning the caller COUNT is what stops a second, unaudited call site appearing.
const SOLE_CALLERS: &[(&str, &str)] = &[
    (
        "remove_org_membership_in_tx",
        "crates/fp-core/src/services/orgs.rs",
    ),
    ("delete_team_tx", "crates/fp-core/src/services/teams.rs"),
    (
        "delete_user_grant_in_tx",
        "crates/fp-core/src/services/teams.rs",
    ),
    (
        "delete_agent_grant_in_tx",
        "crates/fp-core/src/services/teams.rs",
    ),
];

#[test]
fn guarded_deletes_have_exactly_one_audited_caller_each() {
    let root = workspace_root();
    let sources = production_sources(&root);

    for (func, expected_file) in SOLE_CALLERS {
        let mut references: Vec<String> = Vec::new();
        for path in &sources {
            let rel = path
                .strip_prefix(&root)
                .unwrap_or(path)
                .to_string_lossy()
                .replace('\\', "/");
            let src = std::fs::read_to_string(path)
                .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
            for (idx, line) in src.lines().enumerate() {
                // Exclude ONLY the function's own definition line — not the whole defining
                // file. The three wrappers this slice removed authored no SQL themselves;
                // they *delegated* to the `_tx` function. Skipping all of `identity.rs` would
                // therefore let any of them be reinstated in their original delegating shape
                // and stay invisible: the SQL scanner attributes the DELETE to the (legitimate)
                // `_tx` function, and the reinstated wrapper's reference would go uncounted.
                if is_definition_line(line, func) {
                    continue;
                }
                // Comment text is not a call path — a doc comment cross-referencing a sibling
                // function (`see [\`delete_agent_grant_in_tx\`]`) must not read as a second
                // caller. Only the code part of the line is considered. This narrows the
                // false-alarm surface without weakening the guard: anything that can actually
                // invoke the function is still code.
                if line_references_ident(code_part(line), func) {
                    references.push(format!("{rel}:{}", idx + 1));
                }
            }
        }
        assert_eq!(
            references.len(),
            1,
            "`{func}` must have exactly one production reference (the audited service function in \
             {expected_file}); found {}: {:?}. A second reference is a second path by which authority \
             is revoked, and it is not covered by the audit evidence this feature's design rests on.",
            references.len(),
            references,
        );
        let (file, _line) = references[0]
            .rsplit_once(':')
            .expect("reference is formatted as path:line");
        assert_eq!(
            file, *expected_file,
            "`{func}`'s only reference must live in {expected_file}, found {}",
            references[0]
        );
    }
}

/// True if `line` mentions `ident` as a whole word.
///
/// Deliberately counts *any* identifier reference, not just a call-shaped `ident(`. A second
/// caller can reach the function without ever writing `ident(` next to it:
///
/// ```ignore
/// let delete = identity::delete_team_tx;   // function item, no parenthesis
/// delete(&mut tx, team_id).await?;         // invoked through the binding
/// ```
///
/// Counting call shapes would leave that second, unaudited path invisible while the audited
/// call still supplied the single expected match. Counting references also catches a call whose
/// `(` lands on the next line.
///
/// The trade-off is the safe direction: a mention in a comment or doc string counts as a
/// reference and fails the test. That is a false *alarm*, resolved by a human looking at it —
/// never a false pass that hides a real revocation path.
fn line_references_ident(line: &str, ident: &str) -> bool {
    let mut from = 0usize;
    while let Some(rel) = line[from..].find(ident) {
        let start = from + rel;
        let end = start + ident.len();
        let before_ok = start == 0 || !is_ident_byte(line.as_bytes()[start - 1]);
        let after_ok = end == line.len() || !is_ident_byte(line.as_bytes()[end]);
        if before_ok && after_ok {
            return true;
        }
        from = end;
    }
    false
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// The code portion of a line: everything before a `//` line comment, and nothing at all for a
/// block-comment continuation (`* ...`). Deliberately simple — it only has to stop doc-comment
/// prose from reading as a call, and anything it misclassifies as code merely raises a false
/// alarm a human resolves.
fn code_part(line: &str) -> &str {
    let trimmed = line.trim_start();
    if trimmed.starts_with("//") || trimmed.starts_with('*') {
        return "";
    }
    match line.find("//") {
        Some(i) => &line[..i],
        None => line,
    }
}

/// True if `line` is the `fn` declaration for `ident` — the one occurrence that is a
/// definition rather than a use. Everything else in the defining file counts as a reference.
fn is_definition_line(line: &str, ident: &str) -> bool {
    let t = line.trim_start();
    for prefix in [
        "pub async fn ",
        "async fn ",
        "pub(crate) async fn ",
        "pub fn ",
        "fn ",
        "pub(crate) fn ",
    ] {
        if let Some(rest) = t.strip_prefix(prefix) {
            let name_end = rest
                .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                .unwrap_or(rest.len());
            if &rest[..name_end] == ident {
                return true;
            }
        }
    }
    false
}

struct EnclosingFn {
    name: String,
    takes_transaction: bool,
    takes_pool: bool,
}

/// Walk backwards from `line_no` to the nearest preceding top-level `fn` declaration and read
/// its parameter list. Deliberately simple: it only has to classify the handful of repository
/// functions that author guarded deletes, and an unrecognised shape is reported as a violation
/// rather than assumed innocent.
fn enclosing_fn(src: &str, line_no: usize) -> Option<EnclosingFn> {
    let lines: Vec<&str> = src.lines().collect();
    let start = lines[..line_no.min(lines.len())].iter().rposition(|l| {
        l.starts_with("pub async fn ")
            || l.starts_with("async fn ")
            || l.starts_with("fn ")
            || l.starts_with("pub fn ")
    })?;
    // The signature runs from `fn` to the opening brace of the body.
    let mut sig = String::new();
    for line in &lines[start..] {
        sig.push_str(line);
        sig.push('\n');
        if line.contains('{') {
            break;
        }
    }
    let name = sig
        .split("fn ")
        .nth(1)?
        .split(['(', '<'])
        .next()?
        .trim()
        .to_string();
    Some(EnclosingFn {
        takes_transaction: sig.contains("Transaction<"),
        takes_pool: sig.contains("&PgPool"),
        name,
    })
}

#[test]
fn scanner_mechanics_behave_as_documented() {
    // Multi-line SQL with `\` continuation, mixed case, and a schema prefix all match.
    let sql = "let q = \"DELETE\\\n  FROM   user_grants WHERE id = $1\";";
    let hits = find_guarded_deletes(sql);
    assert_eq!(
        hits.len(),
        1,
        "continuation-separated DELETE FROM must match"
    );
    assert_eq!(hits[0].table, "user_grants");

    // Non-guarded tables and near-miss identifiers are ignored.
    assert!(find_guarded_deletes("DELETE FROM clusters").is_empty());
    assert!(find_guarded_deletes("DELETE FROM grants_archive").is_empty());
    assert!(find_guarded_deletes("deleted_from_teams").is_empty());

    // Line numbers are reported against the real file, so a violation is navigable.
    let src = "fn a() {}\nfn b() { q(\"DELETE FROM agents\"); }\n";
    let hits = find_guarded_deletes(src);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].line_no, 2);

    // The transaction-vs-pool classification is what gives the guard its teeth, so pin both
    // directions on shapes matching the real repository signatures.
    let tx_fn = "pub async fn delete_x_in_tx(\n    tx: &mut Transaction<'_, Postgres>,\n    id: Uuid,\n) -> DomainResult<bool> {\n    q(\"DELETE FROM grants WHERE id = $1\");\n}\n";
    let f = enclosing_fn(tx_fn, 5).expect("signature should be recognised");
    assert_eq!(f.name, "delete_x_in_tx");
    assert!(f.takes_transaction && !f.takes_pool);

    let pool_fn = "pub async fn delete_x(pool: &PgPool, id: Uuid) -> DomainResult<bool> {\n    q(\"DELETE FROM grants WHERE id = $1\");\n}\n";
    let f = enclosing_fn(pool_fn, 2).expect("signature should be recognised");
    assert!(
        f.takes_pool,
        "a pool-taking wrapper must be classified as opening its own transaction — this is the \
         exact shape of the three unaudited functions removed in this slice"
    );

    // Reference counting must see a function ITEM, not just a call — that is the evasion a
    // call-shaped match would miss.
    assert!(line_references_ident(
        "    let delete = identity::delete_team_tx;",
        "delete_team_tx"
    ));
    assert!(line_references_ident(
        "    identity::delete_team_tx(&mut tx, id).await?;",
        "delete_team_tx"
    ));
    // ...and must not fire on a longer identifier that merely contains the name.
    assert!(!line_references_ident(
        "    delete_team_tx_helper(&mut tx);",
        "delete_team_tx"
    ));
    assert!(!line_references_ident(
        "    wrap_delete_team_tx(&mut tx);",
        "delete_team_tx"
    ));

    // Only the definition line is exempt from reference counting; a delegating wrapper in the
    // same file is a reference. This is what makes reinstating one of the removed wrappers in
    // its ORIGINAL shape (delegate to `_tx`, author no SQL) visible to the guard.
    assert!(is_definition_line(
        "pub async fn delete_team_tx(",
        "delete_team_tx"
    ));
    assert!(!is_definition_line(
        "    delete_team_tx(&mut tx, team_id).await?;",
        "delete_team_tx"
    ));
    assert!(
        !is_definition_line("pub async fn delete_team(", "delete_team_tx"),
        "a different function whose name is a prefix must not be mistaken for the definition"
    );

    // Comment text is not a call path, but real code on the line still is.
    assert_eq!(code_part("/// see [`delete_team_tx`] for the tx form"), "");
    assert_eq!(code_part("    // delete_team_tx(&mut tx, id);"), "");
    assert!(!line_references_ident(
        code_part("/// see [`delete_team_tx`]"),
        "delete_team_tx"
    ));
    assert!(
        line_references_ident(
            code_part("    delete_team_tx(&mut tx, id).await?; // revoke"),
            "delete_team_tx"
        ),
        "a trailing comment must not hide the call before it"
    );
}
