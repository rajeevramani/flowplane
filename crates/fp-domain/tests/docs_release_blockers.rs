#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use fp_domain::gateway::filters::GlobalRateLimitConfig;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

const EDITED_DOCS: &[&str] = &[
    "docs/reference/filters.md",
    "docs/concepts/global-rate-limiting.md",
    "docs/how-to/global-rate-limit.md",
    "docs/tutorials/getting-started.md",
    "docs/reference/configuration.md",
];

#[test]
fn global_rate_limit_failure_mode_deny_docs_match_domain_default() {
    let cfg: GlobalRateLimitConfig = serde_json::from_value(serde_json::json!({
        "domain": "checkout"
    }))
    .unwrap();
    assert!(cfg.failure_mode_deny);

    let filters = read_repo("docs/reference/filters.md");
    assert!(filters.contains(
        "| `failure_mode_deny` | `bool` | optional (default `true`) | On RLS error/unreachable: `true` fails **closed**"
    ));

    let how_to = read_repo("docs/how-to/global-rate-limit.md");
    assert!(how_to
        .contains("`failure_mode_deny` defaults to `true` (fail **closed**), so the explicit"));
    assert!(how_to.contains(
        "`failure_mode_deny: false` above intentionally overrides the safer default to fail **open**"
    ));

    let concept = read_repo("docs/concepts/global-rate-limiting.md");
    assert!(concept.contains("it defaults to `true`, which"));
    assert!(concept.contains("Setting `failure_mode_deny: false`"));
    assert!(concept.contains("intentionally overrides that safer default to fail **open**"));
}

#[test]
fn release_blocker_flow_variables_are_documented_once_and_match_sources() {
    let configuration = read_repo("docs/reference/configuration.md");
    let server_config = read_repo("crates/fp-core/src/config.rs");
    let rls_config = read_repo("crates/flowplane-rls/src/config.rs");

    let server_vars = [
        "FLOWPLANE_EGRESS_ALLOWED_DESTINATIONS",
        "FLOWPLANE_DISCOVERY_ALLOWED_DESTINATIONS",
        "FLOWPLANE_RLS_GRPC_URL",
        "FLOWPLANE_RLS_GRPC_ALLOW_PRODUCTION_PLAINTEXT",
        "FLOWPLANE_RLS_ADMIN_URL",
        "FLOWPLANE_RLS_ADMIN_TOKEN_FILE",
        "FLOWPLANE_RLS_RECONCILE_SECS",
        "FLOWPLANE_DATAPLANE_TLS_CERT",
        "FLOWPLANE_DATAPLANE_TLS_KEY",
        "FLOWPLANE_DATAPLANE_TLS_CLIENT_CA",
    ];
    for var in server_vars {
        assert!(
            server_config.contains(var),
            "server config source no longer reads {var}"
        );
        assert_eq!(config_table_rows_for(&configuration, var), 1, "{var}");
    }

    let rls_vars = [
        "FLOWPLANE_RLS_GRPC_LISTEN",
        "FLOWPLANE_RLS_GRPC_TLS_CERT",
        "FLOWPLANE_RLS_GRPC_TLS_KEY",
        "FLOWPLANE_RLS_GRPC_TLS_CLIENT_CA",
        "FLOWPLANE_RLS_ALLOW_INSECURE_GRPC",
        "FLOWPLANE_RLS_ADMIN_LISTEN",
        "FLOWPLANE_RLS_ADMIN_TOKEN_FILE",
        "FLOWPLANE_RLS_ALLOW_UNAUTH_ADMIN",
    ];
    for var in rls_vars {
        assert!(rls_config.contains(var), "RLS source no longer reads {var}");
        assert_eq!(config_table_rows_for(&configuration, var), 1, "{var}");
    }
}

#[test]
fn getting_started_allowlists_loopback_upstream_before_expose() {
    let doc = read_repo("docs/tutorials/getting-started.md");
    let allowlist = doc
        .find("FLOWPLANE_EGRESS_ALLOWED_DESTINATIONS=127.0.0.1:3001")
        .unwrap();
    let expose = doc
        .find("./target/debug/flowplane expose http://127.0.0.1:3001")
        .unwrap();
    assert!(allowlist < expose);
}

#[test]
fn edited_docs_have_no_dangling_local_markdown_links() {
    let repo = repo_root();
    for doc in EDITED_DOCS {
        let doc_path = repo.join(doc);
        let body = fs::read_to_string(&doc_path).unwrap();
        let headings = markdown_headings(&body);
        for link in markdown_links(&body) {
            if link.starts_with("http://")
                || link.starts_with("https://")
                || link.starts_with("mailto:")
            {
                continue;
            }
            let (path_part, anchor_part) = link.split_once('#').unwrap_or((&link, ""));
            let target = if path_part.is_empty() {
                doc_path.clone()
            } else {
                doc_path.parent().unwrap().join(path_part)
            };
            assert!(
                target.exists(),
                "{} links to missing target {}",
                doc,
                target.display()
            );
            if !anchor_part.is_empty() {
                let target_body = if path_part.is_empty() {
                    body.clone()
                } else {
                    fs::read_to_string(&target).unwrap()
                };
                let target_headings = if path_part.is_empty() {
                    headings.clone()
                } else {
                    markdown_headings(&target_body)
                };
                assert!(
                    target_headings.contains(anchor_part),
                    "{} links to missing anchor #{} in {}",
                    doc,
                    anchor_part,
                    target.display()
                );
            }
        }
    }
}

fn config_table_rows_for(doc: &str, var: &str) -> usize {
    let needle = format!("| `{var}` |");
    doc.lines().filter(|line| line.starts_with(&needle)).count()
}

fn markdown_links(body: &str) -> Vec<String> {
    let mut links = Vec::new();
    let mut rest = body;
    while let Some(label_end) = rest.find("](") {
        rest = &rest[label_end + 2..];
        let Some(link_end) = rest.find(')') else {
            break;
        };
        let link = &rest[..link_end];
        if !link.starts_with("app://") {
            links.push(link.to_string());
        }
        rest = &rest[link_end + 1..];
    }
    links
}

fn markdown_headings(body: &str) -> HashSet<String> {
    body.lines()
        .filter_map(|line| {
            let trimmed = line.trim_start();
            let hashes = trimmed.chars().take_while(|ch| *ch == '#').count();
            if hashes == 0 || hashes > 6 {
                return None;
            }
            let heading = trimmed[hashes..].trim();
            if heading.is_empty() {
                return None;
            }
            Some(slugify_heading(heading))
        })
        .collect()
}

fn slugify_heading(heading: &str) -> String {
    let mut slug = String::new();
    let mut last_dash = false;
    for ch in heading.chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch);
            last_dash = false;
        } else if (ch.is_whitespace() || ch == '-') && !last_dash && !slug.is_empty() {
            slug.push('-');
            last_dash = true;
        }
    }
    if slug.ends_with('-') {
        slug.pop();
    }
    slug
}

fn read_repo(relative: &str) -> String {
    fs::read_to_string(repo_root().join(relative)).unwrap()
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}
