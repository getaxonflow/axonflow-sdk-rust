//! Real-wire test of the SDK's v9.1 org_id telemetry field (#2277).
//!
//! Sister proof to the Go / Python / TS / Java SDK runtime-e2e tests
//! under the same `v91_org_id_telemetry/` subdirectory. Stands up a
//! tokio TcpListener that pretends to be the checkpoint receiver,
//! invokes the SDK's `maybe_send_heartbeat` path, captures the wire
//! body, and asserts the org_id field.
//!
//! Run via:
//!
//!   # ORG_ID set — operator-supplied or cs_<uuid>:
//!   cd runtime-e2e/v91_org_id_telemetry/helper && ORG_ID=acme-corp cargo run
//!
//!   # ORG_ID unset — sentinel:
//!   cd runtime-e2e/v91_org_id_telemetry/helper && unset ORG_ID && cargo run

use axonflow_sdk_rust::heartbeat::{maybe_send_heartbeat, ORG_ID_LOCAL_DEV_SENTINEL};
use axonflow_sdk_rust::Mode;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let expected = std::env::var("ORG_ID")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| ORG_ID_LOCAL_DEV_SENTINEL.to_string());

    // Nuke the SDK's heartbeat stamp file at the platform-canonical
    // location so the 7-day gate doesn't suppress our ping. Mirrors the
    // Go helper's `os.Remove(stampPath)` pattern.
    if let Some(cache_dir) = dirs::cache_dir() {
        let stamp = cache_dir.join("axonflow").join("rust-telemetry-last-sent");
        let _ = std::fs::remove_file(&stamp);
    }
    // Also: scrub a likely XDG fallback so containerized envs don't keep a
    // stamp around between runs.
    let _ = std::fs::remove_file(PathBuf::from("/tmp/axonflow-rust-telemetry-last-sent"));

    let captured: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));

    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();

    let captured_clone = Arc::clone(&captured);
    tokio::spawn(async move {
        loop {
            if let Ok((mut socket, _)) = listener.accept().await {
                let captured = Arc::clone(&captured_clone);
                tokio::spawn(async move {
                    let mut buf = Vec::with_capacity(8192);
                    let mut chunk = [0u8; 4096];
                    // Read until headers complete (then read body bytes per
                    // Content-Length if we want to be careful — for v9.1 we
                    // just keep reading until socket idle).
                    loop {
                        match tokio::time::timeout(
                            Duration::from_millis(250),
                            socket.read(&mut chunk),
                        )
                        .await
                        {
                            Ok(Ok(0)) => break,
                            Ok(Ok(n)) => buf.extend_from_slice(&chunk[..n]),
                            Ok(Err(_)) => break,
                            Err(_) => break, // idle, body fully drained
                        }
                    }
                    let raw = String::from_utf8_lossy(&buf).to_string();
                    let is_post = raw.starts_with("POST ");
                    let body = raw.splitn(2, "\r\n\r\n").nth(1).unwrap_or("").to_string();
                    if is_post {
                        *captured.lock().unwrap() = Some(body);
                    }
                    let resp = if is_post {
                        b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 35\r\n\r\n{\"latest_version\":null,\"alerts\":[]}".to_vec()
                    } else {
                        b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 29\r\n\r\n{\"version\":\"8.0.0-rt-e2e\"}".to_vec()
                    };
                    let _ = socket.write_all(&resp).await;
                });
            }
        }
    });

    // Point the SDK at our local listener for both /health and /v1/ping.
    let agent_url = format!("http://127.0.0.1:{port}");
    std::env::set_var(
        "AXONFLOW_CHECKPOINT_URL",
        format!("http://127.0.0.1:{port}/v1/ping"),
    );
    std::env::remove_var("AXONFLOW_TELEMETRY");

    // Fire the heartbeat path. HEARTBEAT_ONCE is a per-process gate that's
    // fresh on first invocation; the stamp file is gone so the 7-day gate
    // won't suppress.
    maybe_send_heartbeat(&agent_url, &Mode::Production);

    // Give the spawned async task time to land the POST.
    tokio::time::sleep(Duration::from_secs(2)).await;

    let body_opt = captured.lock().unwrap().clone();
    let body = match body_opt {
        Some(b) => b,
        None => {
            eprintln!("FAIL: no telemetry body captured");
            std::process::exit(1);
        }
    };
    let parsed: serde_json::Value = match serde_json::from_str(body.trim_end_matches('\0')) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("FAIL: body is not valid JSON: {e}\nBody: {body}");
            std::process::exit(1);
        }
    };
    let got = parsed
        .get("org_id")
        .and_then(|v| v.as_str())
        .unwrap_or("<MISSING>");
    if got != expected {
        eprintln!("FAIL: wire org_id = {got:?}, want {expected:?}\nBody: {body}");
        std::process::exit(1);
    }
    println!("PASS: telemetry wire payload carries org_id={got:?} (expected={expected:?})");
    println!("Wire body: {body}");
    Ok(())
}
