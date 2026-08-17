//! Secret-adjacent OIDC subject input for offline platform-admin recovery.
//!
//! Subjects never enter argv, environment variables, logs, errors, output, or debug formatting.

use anyhow::Context;
use std::io::Read;
use std::path::PathBuf;

const MAX_SUBJECT_BYTES: u64 = 4096;

/// Immutable OIDC subject whose formatting is always redacted.
pub(crate) struct RecoverySubject(String);

impl RecoverySubject {
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for RecoverySubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RecoverySubject(<redacted>)")
    }
}

impl std::fmt::Display for RecoverySubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("<redacted>")
    }
}

pub(crate) fn resolve_subject(
    subject_stdin: bool,
    subject_file: Option<PathBuf>,
) -> anyhow::Result<RecoverySubject> {
    let raw = match (subject_stdin, subject_file) {
        (true, None) => read_bounded(std::io::stdin().lock())
            .context("could not read the replacement subject from standard input")?,
        (false, Some(path)) => read_subject_file(path)?,
        _ => anyhow::bail!(
            "supply exactly one private replacement-subject source: --subject-stdin or --subject-file"
        ),
    };
    validate_subject(raw)
}

fn read_bounded(reader: impl Read) -> anyhow::Result<String> {
    let mut raw = String::new();
    reader
        .take(MAX_SUBJECT_BYTES + 1)
        .read_to_string(&mut raw)
        .context("replacement-subject input is not valid UTF-8")?;
    if raw.len() as u64 > MAX_SUBJECT_BYTES {
        anyhow::bail!("replacement-subject input exceeds the 4096-byte limit");
    }
    Ok(raw)
}

fn validate_subject(raw: String) -> anyhow::Result<RecoverySubject> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        anyhow::bail!("replacement-subject input must not be empty");
    }
    Ok(RecoverySubject(trimmed.to_string()))
}

#[cfg(unix)]
fn validate_file_metadata(
    is_regular: bool,
    owner_uid: u32,
    mode: u32,
    effective_uid: u32,
) -> anyhow::Result<()> {
    if !is_regular {
        anyhow::bail!("the private replacement-subject input must be a regular file");
    }
    if owner_uid != effective_uid {
        anyhow::bail!("the private replacement-subject file must be owned by the effective user");
    }
    if mode & 0o077 != 0 {
        anyhow::bail!("the private replacement-subject file must not grant group or other access");
    }
    Ok(())
}

#[cfg(unix)]
fn read_subject_file(path: PathBuf) -> anyhow::Result<String> {
    use rustix::fs::{open, Mode, OFlags};
    use std::os::unix::fs::MetadataExt;

    // O_NOFOLLOW protects the final component. All trust decisions below use metadata from the
    // already-open descriptor, so a path replacement cannot swap the validated file.
    let descriptor = open(
        &path,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    )
    .map_err(|_| anyhow::anyhow!("could not open the private replacement-subject file"))?;
    let file = std::fs::File::from(descriptor);
    let metadata = file
        .metadata()
        .map_err(|_| anyhow::anyhow!("could not inspect the private replacement-subject file"))?;
    validate_file_metadata(
        metadata.is_file(),
        metadata.uid(),
        metadata.mode(),
        rustix::process::geteuid().as_raw(),
    )?;
    read_bounded(file).context("could not read the private replacement-subject file")
}

#[cfg(not(unix))]
fn read_subject_file(_path: PathBuf) -> anyhow::Result<String> {
    anyhow::bail!(
        "--subject-file is unavailable on this platform; use --subject-stdin for private input"
    )
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn subject_formatting_is_redacted() {
        let subject = validate_subject("private-canary".to_string()).expect("valid subject");
        assert_eq!(subject.to_string(), "<redacted>");
        assert_eq!(format!("{subject:?}"), "RecoverySubject(<redacted>)");
    }

    #[test]
    fn bounded_reader_rejects_oversized_input() {
        let raw = vec![b'x'; MAX_SUBJECT_BYTES as usize + 1];
        let error = read_bounded(raw.as_slice()).expect_err("oversized input");
        assert!(!error
            .to_string()
            .contains(String::from_utf8_lossy(&raw).as_ref()));
    }

    #[test]
    fn empty_subject_is_rejected() {
        let error = validate_subject(" \n\t".to_string()).expect_err("empty subject");
        assert_eq!(
            error.to_string(),
            "replacement-subject input must not be empty"
        );
    }

    #[cfg(unix)]
    #[test]
    fn descriptor_metadata_requires_regular_effective_user_owned_owner_only_file() {
        assert!(validate_file_metadata(false, 1000, 0o600, 1000).is_err());
        assert!(validate_file_metadata(true, 1001, 0o600, 1000).is_err());
        assert!(validate_file_metadata(true, 1000, 0o640, 1000).is_err());
        assert!(validate_file_metadata(true, 1000, 0o604, 1000).is_err());
        assert!(validate_file_metadata(true, 1000, 0o600, 1000).is_ok());
    }
}
