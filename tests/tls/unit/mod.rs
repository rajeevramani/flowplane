mod test_certificate_authority;
// Note: test_certificate_parsing and test_tls_config require #[cfg(test)] functions
// from the library that aren't available in integration tests. They need to be
// converted to library tests or the functions need to be exposed via a feature flag.
