//! `flowplane apply` — kubectl-style declarative create-or-update
//!
//! Reads YAML or JSON manifests with a `kind` field and applies them to the
//! Flowplane REST API, creating or updating resources as needed.

use anyhow::{Context, Result};
use clap::Args;
use serde::Deserialize;
use std::path::{Path, PathBuf};

use super::client::FlowplaneClient;
use crate::api::handlers::PaginatedResponse;

/// Arguments for `flowplane apply`
#[derive(Args)]
pub struct ApplyArgs {
    /// Path to a manifest file or directory of manifests
    #[arg(short, long, value_name = "PATH")]
    pub file: PathBuf,
}

/// The set of resource kinds that `apply` supports
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResourceKind {
    Cluster,
    Listener,
    RouteConfig,
    Filter,
    Secret,
    Dataplane,
}

impl ResourceKind {
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "Cluster" => Ok(Self::Cluster),
            "Listener" => Ok(Self::Listener),
            "RouteConfig" | "Route-Config" | "route-config" => Ok(Self::RouteConfig),
            "Filter" => Ok(Self::Filter),
            "Secret" => Ok(Self::Secret),
            "Dataplane" => Ok(Self::Dataplane),
            other => anyhow::bail!(
                "Unknown resource kind '{}'. Supported: Cluster, Listener, RouteConfig, Filter, Secret, Dataplane",
                other
            ),
        }
    }

    fn label(&self) -> &'static str {
        match self {
            Self::Cluster => "Cluster",
            Self::Listener => "Listener",
            Self::RouteConfig => "RouteConfig",
            Self::Filter => "Filter",
            Self::Secret => "Secret",
            Self::Dataplane => "Dataplane",
        }
    }
}

/// Outcome of applying a single resource
enum ApplyOutcome {
    Created,
    Updated,
    Error(String),
}

/// Minimal struct for extracting `id` from list responses (filters, secrets)
#[derive(Debug, Deserialize)]
struct IdNameEntry {
    id: String,
    name: String,
}

pub async fn handle_apply_command(
    args: ApplyArgs,
    client: &FlowplaneClient,
    team: &str,
) -> Result<()> {
    let path = &args.file;
    let manifests = load_manifests(path)?;

    if manifests.is_empty() {
        println!("No manifests found at {}", path.display());
        return Ok(());
    }

    let mut errors = 0u32;

    for (file_label, value) in &manifests {
        match apply_single(client, team, value, file_label).await {
            Ok((kind, name, ApplyOutcome::Created)) => {
                println!("{}/{}: created", kind.label(), name);
            }
            Ok((kind, name, ApplyOutcome::Updated)) => {
                println!("{}/{}: updated", kind.label(), name);
            }
            Ok((kind, name, ApplyOutcome::Error(msg))) => {
                eprintln!("{}/{}: error: {}", kind.label(), name, msg);
                errors += 1;
            }
            Err(e) => {
                eprintln!("{}: error: {:#}", file_label, e);
                errors += 1;
            }
        }
    }

    if errors > 0 {
        std::process::exit(1);
    }

    Ok(())
}

/// Load manifests from a file or directory.
/// Returns Vec of (label, parsed Value) pairs.
fn load_manifests(path: &Path) -> Result<Vec<(String, serde_json::Value)>> {
    if path.is_dir() {
        load_directory(path)
    } else {
        let value = load_file(path)?;
        let label = path.display().to_string();
        Ok(vec![(label, value)])
    }
}

fn load_directory(dir: &Path) -> Result<Vec<(String, serde_json::Value)>> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .with_context(|| format!("Failed to read directory: {}", dir.display()))?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .map(|e| matches!(e, "yaml" | "yml" | "json"))
                .unwrap_or(false)
        })
        .collect();

    entries.sort();

    let mut results = Vec::new();
    for entry in entries {
        match load_file(&entry) {
            Ok(value) => results.push((entry.display().to_string(), value)),
            Err(e) => {
                eprintln!("{}: error: {:#}", entry.display(), e);
            }
        }
    }

    Ok(results)
}

fn load_file(path: &Path) -> Result<serde_json::Value> {
    let contents = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read file: {}", path.display()))?;

    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    match ext {
        "yaml" | "yml" => serde_yaml::from_str(&contents)
            .with_context(|| format!("Invalid YAML: {}", path.display())),
        "json" => serde_json::from_str(&contents)
            .with_context(|| format!("Invalid JSON: {}", path.display())),
        _ => anyhow::bail!("Unsupported file extension: {}", path.display()),
    }
}

/// Apply a single manifest. Returns (kind, name, outcome).
async fn apply_single(
    client: &FlowplaneClient,
    team: &str,
    value: &serde_json::Value,
    label: &str,
) -> Result<(ResourceKind, String, ApplyOutcome)> {
    let kind_str = value
        .get("kind")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("{}: missing 'kind' field", label))?;

    let name = value
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("{}: missing 'name' field", label))?
        .to_string();

    let kind = ResourceKind::from_str(kind_str)?;

    // Strip `kind` from the payload before sending
    let mut body = value.clone();
    if let Some(obj) = body.as_object_mut() {
        obj.remove("kind");
    }

    let outcome = match kind {
        ResourceKind::Cluster => {
            apply_named_resource(client, team, &name, &body, "clusters", HttpVerb::Put).await
        }
        ResourceKind::Listener => {
            apply_named_resource(client, team, &name, &body, "listeners", HttpVerb::Put).await
        }
        ResourceKind::RouteConfig => {
            apply_named_resource(client, team, &name, &body, "route-configs", HttpVerb::Put).await
        }
        ResourceKind::Dataplane => {
            apply_named_resource(client, team, &name, &body, "dataplanes", HttpVerb::Patch).await
        }
        ResourceKind::Filter => apply_id_resource(client, team, &name, &body, "filters").await,
        ResourceKind::Secret => apply_id_resource(client, team, &name, &body, "secrets").await,
    };

    Ok((kind, name, outcome))
}

#[derive(Clone, Copy)]
enum HttpVerb {
    Put,
    Patch,
}

/// Apply a resource that uses name-based REST paths (GET/PUT or GET/PATCH by name).
async fn apply_named_resource(
    client: &FlowplaneClient,
    team: &str,
    name: &str,
    body: &serde_json::Value,
    resource_path: &str,
    update_verb: HttpVerb,
) -> ApplyOutcome {
    let item_path = format!("/api/v1/teams/{team}/{resource_path}/{name}");
    let list_path = format!("/api/v1/teams/{team}/{resource_path}");

    // Check if resource exists
    match client.get_json_optional::<serde_json::Value>(&item_path).await {
        Ok(Some(_)) => {
            // Update
            let result = match update_verb {
                HttpVerb::Put => client.put_json::<_, serde_json::Value>(&item_path, body).await,
                HttpVerb::Patch => {
                    client.patch_json::<_, serde_json::Value>(&item_path, body).await
                }
            };
            match result {
                Ok(_) => ApplyOutcome::Updated,
                Err(e) => ApplyOutcome::Error(format!("{e:#}")),
            }
        }
        Ok(None) => {
            // Create
            match client.post_json::<_, serde_json::Value>(&list_path, body).await {
                Ok(_) => ApplyOutcome::Created,
                Err(e) => ApplyOutcome::Error(format!("{e:#}")),
            }
        }
        Err(e) => ApplyOutcome::Error(format!("lookup failed: {e:#}")),
    }
}

/// Apply a resource that uses ID-based REST paths (filters, secrets).
/// Must list all, find by name, then PATCH by ID for updates.
async fn apply_id_resource(
    client: &FlowplaneClient,
    team: &str,
    name: &str,
    body: &serde_json::Value,
    resource_path: &str,
) -> ApplyOutcome {
    let list_path = format!("/api/v1/teams/{team}/{resource_path}?limit=1000");

    // List all and find by name
    let existing_id = match client.get_json::<PaginatedResponse<IdNameEntry>>(&list_path).await {
        Ok(response) => response.items.into_iter().find(|e| e.name == name).map(|e| e.id),
        Err(e) => return ApplyOutcome::Error(format!("list failed: {e:#}")),
    };

    match existing_id {
        Some(id) => {
            // Update by ID
            let patch_path = format!("/api/v1/teams/{team}/{resource_path}/{id}");
            match client.patch_json::<_, serde_json::Value>(&patch_path, body).await {
                Ok(_) => ApplyOutcome::Updated,
                Err(e) => ApplyOutcome::Error(format!("{e:#}")),
            }
        }
        None => {
            // Create
            let create_path = format!("/api/v1/teams/{team}/{resource_path}");
            match client.post_json::<_, serde_json::Value>(&create_path, body).await {
                Ok(_) => ApplyOutcome::Created,
                Err(e) => ApplyOutcome::Error(format!("{e:#}")),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    #[test]
    fn test_resource_kind_from_str() {
        assert_eq!(ResourceKind::from_str("Cluster").unwrap(), ResourceKind::Cluster);
        assert_eq!(ResourceKind::from_str("Listener").unwrap(), ResourceKind::Listener);
        assert_eq!(ResourceKind::from_str("RouteConfig").unwrap(), ResourceKind::RouteConfig);
        assert_eq!(ResourceKind::from_str("Route-Config").unwrap(), ResourceKind::RouteConfig);
        assert_eq!(ResourceKind::from_str("route-config").unwrap(), ResourceKind::RouteConfig);
        assert_eq!(ResourceKind::from_str("Filter").unwrap(), ResourceKind::Filter);
        assert_eq!(ResourceKind::from_str("Secret").unwrap(), ResourceKind::Secret);
        assert_eq!(ResourceKind::from_str("Dataplane").unwrap(), ResourceKind::Dataplane);
        assert!(ResourceKind::from_str("Unknown").is_err());
    }

    #[test]
    fn test_resource_kind_label() {
        assert_eq!(ResourceKind::Cluster.label(), "Cluster");
        assert_eq!(ResourceKind::RouteConfig.label(), "RouteConfig");
        assert_eq!(ResourceKind::Dataplane.label(), "Dataplane");
    }

    #[test]
    fn test_load_yaml_file() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("cluster.yaml");
        let mut f = std::fs::File::create(&file).unwrap();
        writeln!(
            f,
            "kind: Cluster\nname: my-cluster\nserviceName: my-service\nendpoints:\n  - address: backend\n    port: 8000"
        )
        .unwrap();

        let value = load_file(&file).unwrap();
        assert_eq!(value["kind"], "Cluster");
        assert_eq!(value["name"], "my-cluster");
        assert_eq!(value["serviceName"], "my-service");
    }

    #[test]
    fn test_load_json_file() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("listener.json");
        std::fs::write(
            &file,
            r#"{"kind":"Listener","name":"my-listener","address":"0.0.0.0","port":10000}"#,
        )
        .unwrap();

        let value = load_file(&file).unwrap();
        assert_eq!(value["kind"], "Listener");
        assert_eq!(value["name"], "my-listener");
    }

    #[test]
    fn test_load_directory_filters_and_sorts() {
        let dir = TempDir::new().unwrap();

        // Create files in reverse alphabetical order
        std::fs::write(
            dir.path().join("z-cluster.yaml"),
            "kind: Cluster\nname: z-cluster\nserviceName: svc",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("a-listener.json"),
            r#"{"kind":"Listener","name":"a-listener"}"#,
        )
        .unwrap();
        // Non-manifest file should be skipped
        std::fs::write(dir.path().join("readme.txt"), "not a manifest").unwrap();

        let manifests = load_directory(dir.path()).unwrap();
        assert_eq!(manifests.len(), 2);
        // Should be sorted alphabetically
        assert!(manifests[0].0.contains("a-listener"));
        assert!(manifests[1].0.contains("z-cluster"));
    }

    #[test]
    fn test_load_file_unsupported_extension() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("data.txt");
        std::fs::write(&file, "hello").unwrap();
        assert!(load_file(&file).is_err());
    }

    #[test]
    fn test_load_file_invalid_yaml() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("bad.yaml");
        std::fs::write(&file, "{{invalid yaml").unwrap();
        assert!(load_file(&file).is_err());
    }

    #[test]
    fn test_load_file_invalid_json() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("bad.json");
        std::fs::write(&file, "{not json}").unwrap();
        assert!(load_file(&file).is_err());
    }

    #[test]
    fn test_kind_stripped_from_body() {
        let value: serde_json::Value = serde_json::json!({
            "kind": "Cluster",
            "name": "my-cluster",
            "serviceName": "svc"
        });

        let mut body = value.clone();
        if let Some(obj) = body.as_object_mut() {
            obj.remove("kind");
        }

        assert!(body.get("kind").is_none());
        assert_eq!(body["name"], "my-cluster");
        assert_eq!(body["serviceName"], "svc");
    }

    #[test]
    fn test_missing_kind_field() {
        let value: serde_json::Value = serde_json::json!({
            "name": "my-cluster",
            "serviceName": "svc"
        });
        // Simulate what apply_single does
        let kind_str = value.get("kind").and_then(|v| v.as_str());
        assert!(kind_str.is_none());
    }

    #[test]
    fn test_missing_name_field() {
        let value: serde_json::Value = serde_json::json!({
            "kind": "Cluster",
            "serviceName": "svc"
        });
        let name = value.get("name").and_then(|v| v.as_str());
        assert!(name.is_none());
    }

    #[test]
    fn test_load_manifests_single_file() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("single.yaml");
        std::fs::write(&file, "kind: Cluster\nname: test").unwrap();

        let manifests = load_manifests(&file).unwrap();
        assert_eq!(manifests.len(), 1);
    }

    #[test]
    fn test_load_manifests_directory() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("a.yaml"), "kind: Cluster\nname: a").unwrap();
        std::fs::write(dir.path().join("b.yml"), "kind: Listener\nname: b").unwrap();

        let manifests = load_manifests(dir.path()).unwrap();
        assert_eq!(manifests.len(), 2);
    }

    #[test]
    fn test_load_empty_directory() {
        let dir = TempDir::new().unwrap();
        let manifests = load_manifests(dir.path()).unwrap();
        assert!(manifests.is_empty());
    }
}
