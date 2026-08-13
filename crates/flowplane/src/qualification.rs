use anyhow::{bail, Context, Result};
use clap::Subcommand;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

const CLASSIFICATIONS: &[&str] = &[
    "supported-core",
    "supported-with-dependency",
    "development-only",
    "evolving/experimental",
    "incomplete",
    "not-applicable-to-topology",
];
const SUPPORTED: &[&str] = &["supported-core", "supported-with-dependency"];
const EXECUTABLE_SURFACES: &[&str] = &["openapi", "cli-schema", "dashboard-routes"];
const DECLARATION_SURFACES: &[&str] = &["cli-help", "stable-docs", "config"];
const TRIAGE_CLASSES: &[&str] = &[
    "platform",
    "harness",
    "docs",
    "product",
    "unsupported",
    "shared-responsibility",
];
const EVIDENCE_DIMENSIONS: &[&str] = &[
    "configuration",
    "runtime",
    "isolation",
    "diagnostics",
    "presentation",
    "cleanup",
];
const SEVERITY_LABELS: &[&str] = &["severity:blocker", "severity:major", "severity:minor"];
const GATE_LABELS: &[&str] = &[
    "gate:0", "gate:1", "gate:2", "gate:3", "gate:4", "gate:5", "gate:6",
];
const CAPABILITY_LABELS: &[&str] = &[
    "capability:auth",
    "capability:tenancy",
    "capability:xds",
    "capability:gateway",
    "capability:learning",
    "capability:ai",
    "capability:mcp",
    "capability:dashboard",
    "capability:operations",
    "capability:packaging",
];
const REPOSITORY_URL: &str = "https://github.com/rajeevramani/flowplane";
const MILESTONE_URL: &str = "https://github.com/rajeevramani/flowplane/milestone/1";

#[derive(Debug, Subcommand)]
pub(crate) enum QualificationCommand {
    /// Reconcile exact-artifact surfaces into a deterministic capability inventory.
    Inventory {
        /// JSON document containing artifact, surfaces, and explicit classifications.
        #[arg(long)]
        input: PathBuf,
        /// Canonical generated inventory JSON.
        #[arg(long)]
        output_path: PathBuf,
    },
    /// Validate a qualification inventory, scenario, evidence, and triage contract.
    Validate {
        /// Qualification contract JSON.
        #[arg(long)]
        input: PathBuf,
    },
    /// Redact raw result evidence before it enters a publication tree.
    Assemble {
        /// Raw evidence JSON.
        #[arg(long)]
        input: PathBuf,
        /// Redacted public evidence JSON.
        #[arg(long)]
        output_path: PathBuf,
    },
}

pub(crate) fn run(command: QualificationCommand) -> Result<()> {
    match command {
        QualificationCommand::Inventory { input, output_path } => {
            let document = read_json(&input)?;
            let inventory = generate_inventory(&document)?;
            write_json(&output_path, &inventory)
        }
        QualificationCommand::Validate { input } => {
            let document = read_json(&input)?;
            let errors = validate_contract(&document);
            if errors.is_empty() {
                println!("{{\"valid\":true}}");
                Ok(())
            } else {
                bail!(errors.join("; "))
            }
        }
        QualificationCommand::Assemble { input, output_path } => {
            let mut document = read_json(&input)?;
            let errors = validate_result_classification(&document);
            if !errors.is_empty() {
                bail!(errors.join("; "))
            }
            redact(&mut document, None);
            write_json(&output_path, &document)
        }
    }
}

fn read_json(path: &Path) -> Result<Value> {
    let raw = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_str(&raw).with_context(|| format!("parse JSON from {}", path.display()))
}

fn write_json(path: &Path, document: &Value) -> Result<()> {
    let mut bytes = serde_json::to_vec_pretty(document).context("serialize qualification JSON")?;
    bytes.push(b'\n');
    fs::write(path, bytes).with_context(|| format!("write {}", path.display()))
}

fn string_set(value: Option<&Value>) -> BTreeSet<String> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(ToOwned::to_owned)
        .collect()
}

fn expected_set(values: &[&str]) -> BTreeSet<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

fn generate_inventory(document: &Value) -> Result<Value> {
    let artifact = document
        .get("artifact")
        .cloned()
        .context("artifact is required")?;
    validate_artifact(&artifact).map_err(|errors| anyhow::anyhow!(errors.join("; ")))?;
    let surfaces = document
        .get("surfaces")
        .and_then(Value::as_object)
        .context("surfaces must be an object")?;
    let classifications = document
        .get("classifications")
        .and_then(Value::as_object)
        .context("classifications must be an object")?;

    let mut observed: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (surface, rows) in surfaces {
        let rows = rows
            .as_array()
            .with_context(|| format!("surfaces.{surface} must be an array"))?;
        for row in rows {
            let id = row
                .get("id")
                .and_then(Value::as_str)
                .with_context(|| format!("surfaces.{surface}[].id is required"))?;
            observed
                .entry(id.to_owned())
                .or_default()
                .insert(surface.replace('_', "-"));
        }
    }

    for id in classifications.keys() {
        if !observed.contains_key(id) {
            bail!("classifications.{id} does not match an observed capability")
        }
    }

    let mut inventory = Vec::with_capacity(observed.len());
    for (id, surface_names) in observed {
        let classification = classifications
            .get(&id)
            .and_then(Value::as_object)
            .with_context(|| format!("classifications.{id} is required"))?;
        let class = classification
            .get("classification")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .with_context(|| format!("classifications.{id}.classification is required"))?;
        let rationale = classification
            .get("rationale")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .with_context(|| format!("classifications.{id}.rationale is required"))?;
        if !CLASSIFICATIONS.contains(&class) {
            bail!("classifications.{id}.classification is not allowed")
        }
        if SUPPORTED.contains(&class) && !support_is_corroborated(&id, &surface_names) {
            bail!(
                "classifications.{id}: support requires executable and independent declaration surfaces"
            )
        }
        inventory.push(serde_json::json!({
            "id": id,
            "classification": class,
            "classification_rationale": rationale,
            "observed_in": surface_names,
        }));
    }
    Ok(serde_json::json!({
        "schema_version": 1,
        "artifact": artifact,
        "inventory": inventory,
    }))
}

fn support_is_corroborated(id: &str, observed: &BTreeSet<String>) -> bool {
    if id.starts_with("binary:") || id.starts_with("binary.") {
        return observed == &expected_set(&["binaries"]);
    }
    observed
        .iter()
        .any(|surface| EXECUTABLE_SURFACES.contains(&surface.as_str()))
        && observed
            .iter()
            .any(|surface| DECLARATION_SURFACES.contains(&surface.as_str()))
}

fn validate_artifact(artifact: &Value) -> std::result::Result<(), Vec<String>> {
    let mut errors = Vec::new();
    for field in ["release", "digest"] {
        if artifact
            .get(field)
            .and_then(Value::as_str)
            .is_none_or(|value| value.trim().is_empty())
        {
            errors.push(format!("artifact.{field} is required"));
        }
    }
    if artifact
        .get("digest")
        .and_then(Value::as_str)
        .is_some_and(|digest| {
            digest.len() != 71
                || !digest.starts_with("sha256:")
                || !digest[7..]
                    .chars()
                    .all(|character| character.is_ascii_hexdigit())
        })
    {
        errors.push("artifact.digest must be sha256:<64 hex characters>".to_owned());
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

fn validate_contract(document: &Value) -> Vec<String> {
    let mut errors = Vec::new();
    match document.get("artifact") {
        Some(artifact) => {
            if let Err(artifact_errors) = validate_artifact(artifact) {
                errors.extend(artifact_errors);
            }
        }
        None => errors.push("artifact is required".to_owned()),
    }

    let inventory = document.get("inventory").and_then(Value::as_array);
    if inventory.is_none_or(Vec::is_empty) {
        errors.push("inventory must not be empty".to_owned());
    }
    for (index, row) in inventory.into_iter().flatten().enumerate() {
        let prefix = format!("inventory[{index}]");
        let classification = row
            .get("classification")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty());
        if classification.is_none_or(|value| !CLASSIFICATIONS.contains(&value)) {
            errors.push(format!("{prefix}.classification is invalid"));
        }
        let observed = string_set(row.get("observed_in"));
        let id = row.get("id").and_then(Value::as_str).unwrap_or_default();
        if classification.is_some_and(|value| SUPPORTED.contains(&value))
            && !support_is_corroborated(id, &observed)
        {
            errors.push(format!(
                "{prefix}.observed_in must corroborate support across executable and declaration surfaces"
            ));
        }
        validate_dimensions(row.get("evidence_dimensions"), &prefix, &mut errors);
    }

    let scenarios = document.get("scenarios").and_then(Value::as_array);
    if scenarios.is_none_or(Vec::is_empty) {
        errors.push("scenarios must not be empty".to_owned());
    }
    for (index, scenario) in scenarios.into_iter().flatten().enumerate() {
        let prefix = format!("scenarios[{index}]");
        for field in ["probe_origin", "cleanup", "rerun"] {
            if scenario.get(field).is_none_or(Value::is_null) {
                errors.push(format!("{prefix}.{field} is required"));
            }
        }
        validate_dimensions(scenario.get("evidence_dimensions"), &prefix, &mut errors);
    }

    if string_set(document.get("triage_classes")) != expected_set(TRIAGE_CLASSES) {
        errors
            .push("triage_classes must contain exactly the six responsibility classes".to_owned());
    }
    validate_github(document.get("github"), &mut errors);
    validate_beads(document.get("scheduled_beads"), &mut errors);
    errors
}

fn validate_dimensions(value: Option<&Value>, prefix: &str, errors: &mut Vec<String>) {
    let dimensions = value.and_then(Value::as_object);
    for dimension in EVIDENCE_DIMENSIONS {
        let present = dimensions
            .and_then(|items| items.get(*dimension))
            .and_then(Value::as_array)
            .is_some_and(|items| !items.is_empty());
        if !present {
            errors.push(format!(
                "{prefix}.evidence_dimensions.{dimension} is required"
            ));
        }
    }
}

fn validate_github(value: Option<&Value>, errors: &mut Vec<String>) {
    let github = value.and_then(Value::as_object);
    if github
        .and_then(|value| value.get("repository_url"))
        .and_then(Value::as_str)
        != Some(REPOSITORY_URL)
    {
        errors.push("github.repository_url must match the pinned repository".to_owned());
    }
    let milestone = github
        .and_then(|value| value.get("milestone"))
        .and_then(Value::as_object);
    if milestone
        .and_then(|value| value.get("title"))
        .and_then(Value::as_str)
        != Some("3.1.3")
    {
        errors.push("github.milestone.title must equal 3.1.3".to_owned());
    }
    if milestone
        .and_then(|value| value.get("number"))
        .and_then(Value::as_u64)
        != Some(1)
    {
        errors.push("github.milestone.number must equal 1".to_owned());
    }
    if milestone
        .and_then(|value| value.get("url"))
        .and_then(Value::as_str)
        != Some(MILESTONE_URL)
    {
        errors.push("github.milestone.url must match milestone.number".to_owned());
    }
    let labels = github
        .and_then(|value| value.get("labels"))
        .and_then(Value::as_object);
    for (family, expected) in [
        ("severity", SEVERITY_LABELS),
        ("gate", GATE_LABELS),
        ("capability", CAPABILITY_LABELS),
    ] {
        let actual = labels.and_then(|value| value.get(family));
        if string_set(actual) != expected_set(expected) {
            errors.push(format!(
                "github.labels.{family} must match the pinned taxonomy"
            ));
        }
    }
}

fn validate_beads(value: Option<&Value>, errors: &mut Vec<String>) {
    let Some(beads) = value.and_then(Value::as_array) else {
        errors.push("scheduled_beads must be an array".to_owned());
        return;
    };
    for (index, bead) in beads.iter().enumerate() {
        let prefix = format!("scheduled_beads[{index}]");
        let number = bead.get("github_issue_number").and_then(Value::as_u64);
        if number.is_none() {
            errors.push(format!("{prefix}.github_issue_number is required"));
        }
        let url = bead.get("github_issue_url").and_then(Value::as_str);
        if url.is_none() {
            errors.push(format!("{prefix}.github_issue_url is required"));
        } else if number
            .is_some_and(|number| url != Some(format!("{REPOSITORY_URL}/issues/{number}").as_str()))
        {
            errors.push(format!(
                "{prefix}.github_issue_url must match github_issue_number"
            ));
        }
    }
}

fn validate_result_classification(document: &Value) -> Vec<String> {
    let Some(result) = document.get("result") else {
        return Vec::new();
    };
    let Some(exit_code) = result.get("exit_code").and_then(Value::as_i64) else {
        return vec!["result.exit_code is required".to_owned()];
    };
    let failure_class = result.get("failure_class").and_then(Value::as_str);
    if exit_code == 0 {
        if failure_class != Some("none") {
            return vec!["result.failure_class must equal none for a successful exit".to_owned()];
        }
    } else if failure_class.is_none_or(|class| !TRIAGE_CLASSES.contains(&class)) {
        return vec!["result.failure_class is required for a nonzero exit".to_owned()];
    }
    Vec::new()
}

fn redact(value: &mut Value, key: Option<&str>) {
    const SECRET_KEYS: &[&str] = &[
        "authorization",
        "access_token",
        "refresh_token",
        "id_token",
        "client_secret",
        "password",
        "private_key",
        "token",
        "certificate",
    ];
    if key.is_some_and(|key| SECRET_KEYS.contains(&key.to_ascii_lowercase().as_str())) {
        *value = Value::String("[REDACTED]".to_owned());
        return;
    }
    match value {
        Value::Object(object) => {
            for (child_key, child) in object {
                redact(child, Some(child_key));
            }
        }
        Value::Array(items) => {
            for item in items {
                redact(item, None);
            }
        }
        Value::String(text) => redact_string(text),
        _ => {}
    }
}

fn redact_string(text: &mut String) {
    let sensitive = text.contains("fp-secret-canary")
        || text.contains("fpq-canary")
        || text.contains("FLOWPLANE_TOKEN=")
        || text.contains("Bearer ")
        || text.contains("-----BEGIN PRIVATE KEY-----")
        || text.contains("tskey-");
    if sensitive {
        *text = "[REDACTED]".to_owned();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structured_failure_requires_explicit_triage_class() {
        let document = serde_json::json!({"result": {"exit_code": 1}});
        let errors = validate_result_classification(&document);
        assert_eq!(
            errors,
            ["result.failure_class is required for a nonzero exit"]
        );
    }

    #[test]
    fn harness_and_product_failures_remain_distinct() {
        let harness = serde_json::json!({"result": {"exit_code": 2, "failure_class": "harness"}});
        let product = serde_json::json!({"result": {"exit_code": 1, "failure_class": "product"}});
        assert!(validate_result_classification(&harness).is_empty());
        assert!(validate_result_classification(&product).is_empty());
        assert_ne!(
            harness["result"]["failure_class"],
            product["result"]["failure_class"]
        );
    }

    #[test]
    fn structured_success_must_use_none_classification() {
        let invalid = serde_json::json!({"result": {"exit_code": 0, "failure_class": "product"}});
        assert_eq!(
            validate_result_classification(&invalid),
            ["result.failure_class must equal none for a successful exit"]
        );
    }
}
