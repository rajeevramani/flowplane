use chrono::Utc;
use flowplane::utils::certificates::{load_certificate_bundle, set_mock_time};
use time::Duration;

#[path = "../support.rs"]
mod support;

use support::TestCertificateFiles;

fn offset_to_chrono(dt: time::OffsetDateTime) -> chrono::DateTime<chrono::Utc> {
    let seconds = dt.unix_timestamp();
    let nanos = dt.nanosecond();
    let naive = chrono::NaiveDateTime::from_timestamp_opt(seconds, nanos)
        .expect("valid timestamp for chrono conversion");
    chrono::DateTime::<chrono::Utc>::from_utc(naive, chrono::Utc)
}

#[test]
fn load_certificate_bundle_success() {
    set_mock_time(None);
    let files = TestCertificateFiles::localhost(Duration::days(60)).expect("create cert");

    let bundle =
        load_certificate_bundle(&files.cert_path, &files.key_path, None).expect("load bundle");
    assert!(bundle.info.subject.contains("CN=Flowplane Test"));
    assert!(bundle.info.not_after > Utc::now());
    assert!(bundle.intermediates.is_empty());

    set_mock_time(None);
}

#[test]
fn load_certificate_bundle_rejects_expired() {
    let expiry = time::OffsetDateTime::now_utc() + Duration::days(30);
    let files = TestCertificateFiles::with_expiration(expiry).expect("create cert");

    set_mock_time(Some(offset_to_chrono(expiry + Duration::days(1))));
    let err =
        load_certificate_bundle(&files.cert_path, &files.key_path, None).expect_err("expired");
    assert!(format!("{err}").contains("expired"));
    set_mock_time(None);
}

#[test]
fn load_certificate_bundle_rejects_mismatched_key() {
    set_mock_time(None);
    let files = TestCertificateFiles::localhost(Duration::days(60)).expect("create cert");
    let mismatched_key = files.mismatched_key().expect("create mismatched key");

    let err =
        load_certificate_bundle(&files.cert_path, &mismatched_key, None).expect_err("mismatch");
    assert!(format!("{err}").contains("do not match"));
}
