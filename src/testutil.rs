//! Helpers for tests that need real HTTP servers on the loopback interface.

use axum::Router;
use std::net::SocketAddr;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

/// A server on an ephemeral local port that can be shut down on demand.
pub struct TestServer {
    pub addr: SocketAddr,
    stop: oneshot::Sender<()>,
    task: JoinHandle<()>,
}

impl TestServer {
    pub async fn start(router: Router) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (stop, stopped) = oneshot::channel::<()>();
        let task = tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(async {
                    let _ = stopped.await;
                })
                .await
                .unwrap();
        });
        Self { addr, stop, task }
    }

    pub fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }

    /// Stop listening and close idle connections, as when the process exits.
    pub async fn shutdown(self) {
        let _ = self.stop.send(());
        let _ = self.task.await;
    }
}
