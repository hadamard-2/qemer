//! A one-shot HTTP stand-in for `llama-server`'s embeddings endpoint.
//!
//! Answers exactly one request with a fixed vector, ignoring the request
//! body entirely. Enough to drive `EmbedClient` deterministically, without a
//! running model or a fixture that only `qemer-ingest` could produce.

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// Start the stub and return its base URL.
pub async fn start(vector: Vec<f32>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        let Ok((mut socket, _)) = listener.accept().await else {
            return;
        };
        // Not a real HTTP parser: read once and assume the request (a small
        // JSON POST) arrived in a single packet, which is true on loopback.
        let mut buf = vec![0u8; 8192];
        let _ = socket.read(&mut buf).await;

        let body = serde_json::json!({
            "data": [{ "embedding": vector, "index": 0 }]
        })
        .to_string();
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        let _ = socket.write_all(response.as_bytes()).await;
        let _ = socket.shutdown().await;
    });

    format!("http://{addr}")
}
