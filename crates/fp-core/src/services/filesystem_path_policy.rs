//! Service-layer validation for tenant-authored Envoy filesystem paths.

use fp_domain::{DomainError, DomainResult};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilesystemPathPolicy {
    dev_mode: bool,
    local_file_opt_in: bool,
    base_dirs: Vec<PathBuf>,
}

impl FilesystemPathPolicy {
    pub fn disabled() -> Self {
        Self {
            dev_mode: false,
            local_file_opt_in: false,
            base_dirs: Vec::new(),
        }
    }

    pub fn new(
        dev_mode: bool,
        local_file_opt_in: bool,
        base_dirs: Vec<PathBuf>,
    ) -> DomainResult<Self> {
        let mut canonical_bases = Vec::with_capacity(base_dirs.len());
        for base in base_dirs {
            let canonical = base.canonicalize().map_err(|e| {
                DomainError::invalid_config(format!(
                    "tenant local-file base dir {} cannot be canonicalized: {e}",
                    base.display()
                ))
            })?;
            if !canonical.is_dir() {
                return Err(DomainError::invalid_config(format!(
                    "tenant local-file base dir {} is not a directory",
                    canonical.display()
                )));
            }
            canonical_bases.push(canonical);
        }
        Ok(Self {
            dev_mode,
            local_file_opt_in,
            base_dirs: canonical_bases,
        })
    }

    pub fn from_server_config(config: &crate::config::ServerConfig) -> DomainResult<Self> {
        Self::new(
            config.dev_mode,
            config.dev_allow_tenant_file_paths,
            config.dev_tenant_file_base_dirs.clone(),
        )
    }

    pub fn from_process_config() -> DomainResult<Self> {
        let dev_mode = bool_env("FLOWPLANE_DEV_MODE");
        let local_file_opt_in = std::env::var("FLOWPLANE_DEV_ALLOW_TENANT_FILE_PATHS")
            .ok()
            .as_deref()
            == Some("yes-this-is-local-only");
        let base_dirs = parse_path_list(std::env::var("FLOWPLANE_DEV_TENANT_FILE_BASE_DIRS").ok());
        Self::new(dev_mode, local_file_opt_in, base_dirs)
    }

    pub fn validate_optional_path(
        &self,
        field: &'static str,
        value: Option<&str>,
    ) -> DomainResult<Option<PathBuf>> {
        match value {
            Some(path) => self.validate_path(field, path).map(Some),
            None => Ok(None),
        }
    }

    pub fn validate_path(&self, field: &'static str, value: &str) -> DomainResult<PathBuf> {
        if !self.dev_mode || !self.local_file_opt_in {
            return Err(DomainError::validation(format!(
                "{field} uses a tenant-authored filesystem path; use an SDS secret reference or enable dev local-file mode explicitly"
            ))
            .with_hint(
                "non-dev/default mode rejects tenant-authored Envoy filename fields; dev local-file mode requires FLOWPLANE_DEV_MODE=true and FLOWPLANE_DEV_ALLOW_TENANT_FILE_PATHS=yes-this-is-local-only",
            ));
        }
        if self.base_dirs.is_empty() {
            return Err(DomainError::validation(format!(
                "{field} requires at least one configured dev local-file base dir"
            ))
            .with_hint("set FLOWPLANE_DEV_TENANT_FILE_BASE_DIRS to one or more directories"));
        }
        if value.trim().is_empty() {
            return Err(DomainError::validation(format!(
                "{field} must not be empty"
            )));
        }
        if value.chars().any(char::is_control) {
            return Err(DomainError::validation(format!(
                "{field} must not contain control characters"
            )));
        }
        let raw = Path::new(value);
        if !raw.is_absolute() {
            return Err(DomainError::validation(format!(
                "{field} must be an absolute canonical path"
            )));
        }
        let canonical = raw.canonicalize().map_err(|e| {
            DomainError::validation(format!("{field} cannot be canonicalized: {e}"))
        })?;
        if canonical != raw {
            return Err(DomainError::validation(format!(
                "{field} must be canonical and must not contain .. or symlink escapes"
            )));
        }
        if !self
            .base_dirs
            .iter()
            .any(|base| canonical.starts_with(base))
        {
            return Err(DomainError::validation(format!(
                "{field} is outside the configured dev local-file base dirs"
            )));
        }
        Ok(canonical)
    }
}

fn bool_env(key: &str) -> bool {
    matches!(
        std::env::var(key).ok().as_deref(),
        Some("true" | "1" | "yes")
    )
}

pub(crate) fn parse_path_list(raw: Option<String>) -> Vec<PathBuf> {
    raw.map(|raw| {
        raw.split(',')
            .map(str::trim)
            .filter(|entry| !entry.is_empty())
            .map(PathBuf::from)
            .collect()
    })
    .unwrap_or_default()
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_root() -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "flowplane-file-policy-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).expect("mkdir");
        root.canonicalize().expect("canonical root")
    }

    #[test]
    fn non_dev_rejects_tenant_paths() {
        let policy = FilesystemPathPolicy::disabled();
        let err = policy
            .validate_path("listener.tls.cert_chain_file", "/etc/cert.pem")
            .expect_err("non-dev path rejected");
        assert_eq!(err.code, fp_domain::ErrorCode::ValidationFailed);
        assert!(err.message.contains("tenant-authored filesystem path"));
    }

    #[test]
    fn dev_mode_still_requires_explicit_file_opt_in() {
        let root = temp_root();
        let file = root.join("cert.pem");
        std::fs::write(&file, "cert").expect("write");
        std::fs::create_dir_all(root.join("nested")).expect("nested dir");
        let policy =
            FilesystemPathPolicy::new(true, false, vec![root]).expect("policy construction");
        let err = policy
            .validate_path("listener.tls.cert_chain_file", file.to_str().expect("utf8"))
            .expect_err("missing file opt-in rejected");
        assert!(err.message.contains("tenant-authored filesystem path"));
    }

    #[test]
    fn dev_local_file_mode_accepts_canonical_paths_under_base_dir() {
        let root = temp_root();
        let file = root.join("cert.pem");
        std::fs::write(&file, "cert").expect("write");
        let policy = FilesystemPathPolicy::new(true, true, vec![root]).expect("policy");
        assert_eq!(
            policy
                .validate_path("listener.tls.cert_chain_file", file.to_str().expect("utf8"))
                .expect("accepted"),
            file
        );
    }

    #[test]
    fn dev_local_file_mode_rejects_bad_paths() {
        let root = temp_root();
        let file = root.join("cert.pem");
        std::fs::write(&file, "cert").expect("write");
        let outside_root = temp_root();
        let outside = outside_root.join("cert.pem");
        std::fs::write(&outside, "cert").expect("write outside");
        let policy = FilesystemPathPolicy::new(true, true, vec![root.clone()]).expect("policy");

        for (label, value) in [
            ("relative", "cert.pem".to_string()),
            (
                "missing",
                root.join("missing.pem").to_string_lossy().into_owned(),
            ),
            (
                "dotdot",
                root.join("nested/../cert.pem")
                    .to_string_lossy()
                    .into_owned(),
            ),
            ("control", format!("{}\n", file.to_string_lossy())),
            ("outside", outside.to_string_lossy().into_owned()),
        ] {
            assert!(
                policy
                    .validate_path("listener.tls.cert_chain_file", &value)
                    .is_err(),
                "{label} path must be rejected"
            );
        }
    }
}
