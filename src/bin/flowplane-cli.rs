use flowplane::cli;

fn install_rustls_provider() {
    use rustls::crypto::{ring, CryptoProvider};

    if CryptoProvider::get_default().is_none() {
        ring::default_provider().install_default().expect("install ring crypto provider");
    }
}

#[tokio::main]
async fn main() -> flowplane::Result<()> {
    install_rustls_provider();

    cli::run_cli().await
}
