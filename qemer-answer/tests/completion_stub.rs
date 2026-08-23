//! A one-shot HTTP stand-in for `llama-server`'s streaming chat endpoint.
//!
//! Writes the supplied SSE frames and closes. Frames are written in two
//! deliberately misaligned pieces so the client's line reassembly is
//! exercised rather than merely present.

#![allow(dead_code)]

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// Start the stub and return its base URL.
pub async fn start(frames: Vec<String>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        let Ok((mut socket, _)) = listener.accept().await else {
            return;
        };
        // Not a real HTTP parser: read once and assume the request arrived in
        // a single packet, which holds on loopback for a small JSON POST.
        let mut scratch = vec![0u8; 16384];
        let _ = socket.read(&mut scratch).await;

        let body: String = frames.iter().map(|f| format!("data: {f}\n\n")).collect();
        let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n";
        let _ = socket.write_all(head.as_bytes()).await;

        // Split mid-body so at least one SSE frame straddles two writes.
        let split = body.len() / 2;
        let split = body
            .char_indices()
            .map(|(i, _)| i)
            .find(|i| *i >= split)
            .unwrap_or(0);
        let _ = socket.write_all(&body.as_bytes()[..split]).await;
        let _ = socket.flush().await;
        let _ = socket.write_all(&body.as_bytes()[split..]).await;
        let _ = socket.shutdown().await;
    });

    format!("http://{addr}")
}
