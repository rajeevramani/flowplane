use std::fs;
use std::path::{Path, PathBuf};

/// When dropped, attempts to cleanup registered paths (files/dirs).
/// Intended to be extended to stop processes/containers as the harness evolves.
#[derive(Debug)]
pub struct TeardownGuard {
    artifacts_dir: Option<PathBuf>,
    cleanup_paths: Vec<PathBuf>,
    mode: ArtifactMode,
    finished_successfully: bool,
}

#[derive(Clone, Copy, Debug)]
#[allow(dead_code)]
pub enum ArtifactMode {
    OnFailure,
    Always,
    Never,
}

impl TeardownGuard {
    pub fn new(artifacts_dir: impl AsRef<Path>, mode: ArtifactMode) -> Self {
        Self {
            artifacts_dir: Some(artifacts_dir.as_ref().to_path_buf()),
            cleanup_paths: vec![],
            mode,
            finished_successfully: false,
        }
    }

    /// Register a path to be removed on drop (file or directory).
    pub fn track_path(&mut self, path: impl AsRef<Path>) {
        self.cleanup_paths.push(path.as_ref().to_path_buf());
    }

    /// Signal test completion. If `success` is true and mode was OnFailure, artifacts are removed.
    #[allow(dead_code)]
    pub fn finish(&mut self, success: bool) {
        self.finished_successfully = success;
        if success && !matches!(self.mode, ArtifactMode::Always) {
            if let Some(dir) = &self.artifacts_dir {
                let _ = fs::remove_dir_all(dir);
            }
            self.artifacts_dir = None;
        }
    }
}

impl Drop for TeardownGuard {
    fn drop(&mut self) {
        let keep_for_failure =
            matches!(self.mode, ArtifactMode::OnFailure) && !self.finished_successfully;
        if !keep_for_failure {
            for p in self.cleanup_paths.drain(..) {
                let _ = if p.is_dir() { fs::remove_dir_all(&p) } else { fs::remove_file(&p) };
            }
            if !matches!(self.mode, ArtifactMode::Always) {
                if let Some(dir) = &self.artifacts_dir {
                    let _ = fs::remove_dir_all(dir);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn cleanup_removes_tracked_paths() {
        let tmp = tempdir().unwrap();
        let f = tmp.path().join("file.txt");
        fs::write(&f, b"hi").unwrap();
        {
            let mut guard = TeardownGuard::new(tmp.path(), ArtifactMode::Never);
            guard.track_path(&f);
        }
        assert!(!f.exists());
    }
}
