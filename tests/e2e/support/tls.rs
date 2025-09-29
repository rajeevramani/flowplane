use std::path::PathBuf;

#[allow(dead_code)]
pub struct TlsFixtures {
    pub ca: PathBuf,
    pub server_cert: PathBuf,
    pub server_key: PathBuf,
    pub client_cert: PathBuf,
    pub client_key: PathBuf,
}

#[allow(dead_code)]
impl TlsFixtures {
    pub fn load() -> Option<Self> {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/e2e/fixtures/tls");
        let ca = root.join("ca.pem");
        let server_cert = root.join("server.pem");
        let server_key = root.join("server.key");
        let client_cert = root.join("client.pem");
        let client_key = root.join("client.key");
        // Require at least CA + server cert/key for edge TLS tests.
        if ca.exists() && server_cert.exists() && server_key.exists() {
            Some(Self { ca, server_cert, server_key, client_cert, client_key })
        } else {
            None
        }
    }
}
