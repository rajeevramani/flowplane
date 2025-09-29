use axum::{routing::any, Router};
use std::net::SocketAddr;
use tokio::sync::oneshot;
use tracing::info;

#[derive(Debug)]

pub struct EchoServerHandle {
    shutdown: Option<oneshot::Sender<()>>,
    #[allow(dead_code)]
    pub addr: SocketAddr,
}

#[allow(dead_code)]
impl EchoServerHandle {
    pub async fn start(addr: SocketAddr) -> Self {
        let (tx, rx) = oneshot::channel::<()>();
        // axum 0.7 path wildcard syntax changed to use `{*var}`
        let app = Router::new().route("/{*path}", any(echo_handler));
        let server = axum::serve(tokio::net::TcpListener::bind(addr).await.unwrap(), app)
            .with_graceful_shutdown(async move {
                let _ = rx.await;
            });
        let local = server.local_addr().expect("local addr");
        tokio::spawn(async move {
            let _ = server.await;
        });
        info!(%local, "Started echo server");
        Self { shutdown: Some(tx), addr: local }
    }

    pub async fn stop(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
    }
}

async fn echo_handler(axum::extract::Path(path): axum::extract::Path<String>) -> String {
    format!("echo:{}", path)
}
