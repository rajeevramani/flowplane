use chrono::{Duration, Utc};
use flowplane::utils::certificates::{load_certificate_bundle, set_mock_time};

fn fixture(path: &str) -> std::path::PathBuf {
    std::path::Path::new("tests/fixtures").join(path)
}

#[test]
fn load_certificate_bundle_success() {
    set_mock_time(None);
    let cert_path = fixture("valid_cert.pem");
    let key_path = fixture("valid_key.pem");

    let bundle = load_certificate_bundle(&cert_path, &key_path, None).expect("load bundle");
    assert!(bundle.info.subject.contains("CN=Flowplane Test"));
    assert!(bundle.info.not_after > Utc::now());
    assert!(bundle.intermediates.is_empty());

    set_mock_time(None);
}

#[test]
fn load_certificate_bundle_rejects_expired() {
    let cert_path = fixture("valid_cert.pem");
    let key_path = fixture("valid_key.pem");

    set_mock_time(Some(Utc::now() + Duration::days(35)));
    let err = load_certificate_bundle(&cert_path, &key_path, None).expect_err("expired");
    assert!(format!("{err}").contains("expired"));
    set_mock_time(None);
}

#[test]
fn load_certificate_bundle_rejects_mismatched_key() {
    set_mock_time(None);
    let cert_path = fixture("valid_cert.pem");
    let key_path = fixture("mismatched_key.pem");

    let err = load_certificate_bundle(&cert_path, &key_path, None).expect_err("mismatch");
    assert!(format!("{err}").contains("do not match"));
}
