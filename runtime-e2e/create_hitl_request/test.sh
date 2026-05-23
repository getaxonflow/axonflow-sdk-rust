#!/usr/bin/env bash
# Runtime proof — Rust SDK v0.4 create_hitl_request through real reqwest
# transport against a localhost HTTP server that mimics the platform
# handler at platform/agent/hitl/handler.go:177.
#
# Drives the SDK's create_hitl_request method via a tiny temp-cargo
# binary that uses the LOCAL SDK (via [path = "../.."] in a temporary
# Cargo.toml). Asserts the captured wire body carries every required
# field plus the new notify_url surface added in
# getaxonflow/axonflow-enterprise#2419, and that the SDK parses the
# platform's APIResponse{success,data} envelope back into a populated
# HITLApprovalRequest.
#
# No wiremock — uses tokio's hyper to bind a real socket. Satisfies the
# runtime-e2e/ DoD gate that the in-process wiremock-based unit tests
# in src/hitl.rs do not.
#
# Issue: getaxonflow/axonflow-enterprise#2421
#
# Usage:
#   ./test.sh

set -uo pipefail

SDK_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
RUN_TAG=$(date -u +%s)
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

red()   { printf '\033[31m%s\033[0m\n' "$*"; }
green() { printf '\033[32m%s\033[0m\n' "$*"; }

cat > "$WORK/Cargo.toml" <<EOF
[package]
name = "create-hitl-rt-${RUN_TAG}"
version = "0.0.0"
edition = "2021"

[dependencies]
axonflow-sdk-rust = { path = "${SDK_ROOT}" }
hyper = { version = "1", features = ["http1", "server"] }
hyper-util = { version = "0.1", features = ["tokio"] }
http-body-util = "0.1"
bytes = "1"
serde_json = "1"
tokio = { version = "1", features = ["macros", "rt-multi-thread", "net", "sync"] }
EOF

mkdir -p "$WORK/src"
cat > "$WORK/src/main.rs" <<'EOF'
use axonflow_sdk_rust::{AxonFlowClient, AxonFlowConfig, HITLCreateInput};
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::Mutex;

const NOTIFY_URL: &str = "https://workflows.example.com/hooks/runtime-e2e";

type CapturedBody = Arc<Mutex<Vec<u8>>>;

async fn handle(
    req: Request<Incoming>,
    captured: CapturedBody,
) -> Result<Response<Full<Bytes>>, hyper::Error> {
    if req.method() != hyper::Method::POST || req.uri().path() != "/api/v1/hitl/queue" {
        return Ok(Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Full::new(Bytes::new()))
            .unwrap());
    }
    let bytes = req.collect().await?.to_bytes().to_vec();
    let wire: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    *captured.lock().await = bytes;

    let resp_body = serde_json::json!({
        "success": true,
        "data": {
            "request_id": "hitl-req-runtime-e2e-001",
            "org_id": "org-runtime-e2e",
            "tenant_id": "tenant-runtime-e2e",
            "client_id": wire["client_id"],
            "user_id": wire["user_id"],
            "original_query": wire["original_query"],
            "request_type": wire["request_type"],
            "request_context": wire.get("request_context").cloned().unwrap_or(serde_json::Value::Null),
            "triggered_policy_id": wire["triggered_policy_id"],
            "triggered_policy_name": wire["triggered_policy_name"],
            "trigger_reason": wire["trigger_reason"],
            "severity": wire["severity"],
            "notify_url": wire["notify_url"],
            "status": "pending",
            "expires_at": "2026-05-23T11:00:00Z",
            "created_at": "2026-05-23T10:00:00Z",
            "updated_at": "2026-05-23T10:00:00Z",
        },
    });
    Ok(Response::builder()
        .status(StatusCode::CREATED)
        .header("content-type", "application/json")
        .body(Full::new(Bytes::from(resp_body.to_string())))
        .unwrap())
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let endpoint = format!("http://127.0.0.1:{port}");

    let captured: CapturedBody = Arc::new(Mutex::new(Vec::new()));
    let captured_for_server = captured.clone();

    tokio::spawn(async move {
        loop {
            let (stream, _) = match listener.accept().await {
                Ok(v) => v,
                Err(_) => break,
            };
            let cap = captured_for_server.clone();
            tokio::spawn(async move {
                let _ = http1::Builder::new()
                    .serve_connection(
                        TokioIo::new(stream),
                        service_fn(move |req| handle(req, cap.clone())),
                    )
                    .await;
            });
        }
    });

    let client = AxonFlowClient::new(AxonFlowConfig::new(&endpoint))?;

    let mut request_context = HashMap::new();
    request_context.insert(
        "tool_name".to_string(),
        serde_json::Value::String("disburse_payment".to_string()),
    );

    let req = client
        .create_hitl_request(HITLCreateInput {
            client_id: "runtime-e2e-client".into(),
            user_id: Some("runtime-e2e-user".into()),
            original_query: "disburse $50000 to cust-runtime-e2e".into(),
            request_type: "adk-tool".into(),
            request_context: Some(request_context),
            triggered_policy_id: Some("loan-amount-cap".into()),
            triggered_policy_name: Some("Loan amount cap".into()),
            trigger_reason: Some(
                "Disbursement above $10k requires manager approval".into(),
            ),
            severity: Some("high".into()),
            notify_url: Some(NOTIFY_URL.into()),
            ..Default::default()
        })
        .await?;

    let body = captured.lock().await.clone();
    if body.is_empty() {
        eprintln!("FAIL: server captured no body");
        std::process::exit(1);
    }
    let wire: serde_json::Value = serde_json::from_slice(&body)?;
    let expected: &[(&str, &str)] = &[
        ("client_id", "runtime-e2e-client"),
        ("user_id", "runtime-e2e-user"),
        ("original_query", "disburse $50000 to cust-runtime-e2e"),
        ("request_type", "adk-tool"),
        ("triggered_policy_id", "loan-amount-cap"),
        ("triggered_policy_name", "Loan amount cap"),
        ("trigger_reason", "Disbursement above $10k requires manager approval"),
        ("severity", "high"),
        ("notify_url", NOTIFY_URL),
    ];
    for (field, want) in expected {
        let got = wire[*field].as_str().unwrap_or("");
        if got != *want {
            eprintln!(
                "FAIL: wire body field {field} = {got:?}, want {want:?}\nFull body: {}",
                String::from_utf8_lossy(&body)
            );
            std::process::exit(1);
        }
    }
    if req.request_id != "hitl-req-runtime-e2e-001" {
        eprintln!("FAIL: parsed request_id = {}", req.request_id);
        std::process::exit(1);
    }
    if req.notify_url.as_deref() != Some(NOTIFY_URL) {
        eprintln!(
            "FAIL: parsed notify_url = {:?}, want {}",
            req.notify_url, NOTIFY_URL
        );
        std::process::exit(1);
    }

    println!("PASS: create_hitl_request wire payload + response parsing round-trip OK");
    println!("Wire body: {}", String::from_utf8_lossy(&body));
    println!(
        "Parsed request_id={} notify_url={:?}",
        req.request_id, req.notify_url
    );
    Ok(())
}
EOF

cd "$WORK"
if ! cargo build --quiet 2>&1; then
    red "FAIL: cargo build failed"
    exit 1
fi
if ! ./target/debug/create-hitl-rt-${RUN_TAG}; then
    red "FAIL: runtime probe exited non-zero"
    exit 1
fi
green "PASS: runtime-e2e create_hitl_request"
