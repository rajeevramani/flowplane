//! Architecture guard for fpv2-7f3.3's injected legacy-certificate pin boundary.

#![allow(clippy::expect_used)]

#[test]
fn fp_xds_owns_the_interface_and_the_binary_composes_fp_core_without_a_reverse_dependency() {
    let xds_manifest = include_str!("../../fp-xds/Cargo.toml");
    let production_dependencies = xds_manifest
        .split_once("[dependencies]")
        .expect("fp-xds production dependencies")
        .1
        .split_once("[dev-dependencies]")
        .expect("fp-xds dev dependencies")
        .0;
    assert!(
        !production_dependencies.lines().any(|line| {
            line.split_once('=')
                .is_some_and(|(name, _)| name.trim() == "fp-core")
        }),
        "fp-xds must not declare a production dependency on fp-core"
    );

    let xds_ads = include_str!("../../fp-xds/src/ads.rs");
    assert!(xds_ads.contains("pub trait LegacyCertificateFingerprintPinner"));

    let composition_root = include_str!("../src/serve.rs");
    assert!(composition_root.contains(
        "impl fp_xds::ads::LegacyCertificateFingerprintPinner for CoreLegacyCertificateFingerprintPinner"
    ));
    assert!(composition_root
        .contains("fp_core::services::dataplanes::pin_legacy_certificate_fingerprint"));
}
