use std::fs;
use std::path::{Path, PathBuf};

pub fn collect_envoy_admin_logs(out_dir: &Path) {
    let _ = fs::create_dir_all(out_dir);
    for entry in ["/tmp/envoy_admin.log", "/tmp/envoy_admin_tls.log", "/tmp/envoy_admin_meta.log", "/tmp/envoy_admin_mtls.log"] {
        let from = PathBuf::from(entry);
        if from.exists() {
            let to = out_dir.join(from.file_name().unwrap());
            let _ = fs::copy(&from, &to);
        }
    }
}

