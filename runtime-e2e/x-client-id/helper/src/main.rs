//! Real-wire test of the SDK's v9 X-Client-ID + ADR-050 §4 X-Axonflow-Client
//! header emission against a real running AxonFlow agent.
//!
//! Mirrors the in-process forwarding-proxy approach used by the other 4
//! SDKs' runtime-e2e/x-client-id/ runners (Go: httputil.ReverseProxy,
//! Java: HttpServer + HttpClient, Python/TS: httpx/fetch monkey-patch).
//!
//! Flow:
//!   1. Bind a tokio TcpListener on 127.0.0.1:0.
//!   2. Construct the SDK with that listener's URL as `agent_url` and the
//!      caller-supplied AXONFLOW_TENANT_ID / AXONFLOW_TENANT_SECRET.
//!   3. Issue one `proxy_llm_call`.
//!   4. The listener accepts the connection, parses the request headers
//!      off the wire (captures the four headers we care about), forwards
//!      the request to the real agent at AXONFLOW_AGENT_URL via reqwest,
//!      and writes the agent's response back to the SDK.
//!   5. After the call completes, assert:
//!        - `X-Client-ID`       equals AXONFLOW_TENANT_ID
//!        - `X-Axonflow-Client` starts with `sdk-rust/`
//!        - `Authorization`     starts with `Basic `
//!        - `X-Tenant-ID`       absent (agent still accepts as an alias for
//!          back-compat through v9, but the SDK standardizes on X-Client-ID)

use axonflow_sdk_rust::{AxonFlowClient, AxonFlowConfig};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let tenant =
        std::env::var("AXONFLOW_TENANT_ID").map_err(|_| "AXONFLOW_TENANT_ID must be set")?;
    let secret = std::env::var("AXONFLOW_TENANT_SECRET")
        .map_err(|_| "AXONFLOW_TENANT_SECRET must be set")?;
    let upstream =
        std::env::var("AXONFLOW_AGENT_URL").unwrap_or_else(|_| "http://localhost:8080".to_string());

    let captured: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(Vec::new()));
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let proxy_url = format!("http://{}", listener.local_addr()?);

    let captured_for_server = captured.clone();
    let upstream_for_server = upstream.clone();
    tokio::spawn(async move {
        let client = reqwest::Client::new();
        loop {
            let (mut conn, _) = match listener.accept().await {
                Ok(x) => x,
                Err(_) => break,
            };
            let captured = captured_for_server.clone();
            let upstream = upstream_for_server.clone();
            let client = client.clone();
            tokio::spawn(async move {
                let _ = handle(&mut conn, &upstream, &client, captured).await;
            });
        }
    });

    let cfg = AxonFlowConfig::new(proxy_url).with_auth(tenant.clone(), secret);
    let client = AxonFlowClient::new(cfg)?;
    // outcome of the call doesn't matter; only the captured headers.
    let _ = client
        .proxy_llm_call("", "ping", "chat", HashMap::new())
        .await;

    // small grace period so the spawned task observes the connection close
    // and stores its headers into `captured` before we read.
    tokio::time::sleep(Duration::from_millis(200)).await;

    let cap = captured.lock().unwrap();
    let lookup = |name: &str| -> Option<String> {
        cap.iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.clone())
    };

    let xcid = lookup("X-Client-ID");
    let xac = lookup("X-Axonflow-Client");
    let auth = lookup("Authorization");
    let xtenant = lookup("X-Tenant-ID");

    let mut failed = Vec::<String>::new();
    if xcid.as_deref() != Some(tenant.as_str()) {
        failed.push(format!("X-Client-ID: want {:?}, got {:?}", tenant, xcid));
    }
    if !xac.as_deref().is_some_and(|v| v.starts_with("sdk-rust/")) {
        failed.push(format!(
            "X-Axonflow-Client: want starts-with 'sdk-rust/', got {:?}",
            xac
        ));
    }
    if !auth.as_deref().is_some_and(|v| v.starts_with("Basic ")) {
        failed.push(format!(
            "Authorization: want starts-with 'Basic ', got {:?}",
            auth
        ));
    }
    if let Some(v) = xtenant.as_ref() {
        failed.push(format!("X-Tenant-ID: should be ABSENT, got {:?}", v));
    }

    if !failed.is_empty() {
        for f in &failed {
            eprintln!("FAIL: {}", f);
        }
        std::process::exit(1);
    }

    println!("PASS: 4/4 header assertions");
    println!("  X-Client-ID:       {}", xcid.unwrap());
    println!("  X-Axonflow-Client: {}", xac.unwrap());
    println!("  Authorization:     Basic <redacted base64>");
    println!("  X-Tenant-ID:       <absent (✓)>");
    Ok(())
}

/// Read the HTTP/1.1 request from `conn`, capture its headers into
/// `captured`, forward the request to the real agent at `upstream`, and
/// stream the agent's response back to `conn`.
async fn handle(
    conn: &mut TcpStream,
    upstream: &str,
    client: &reqwest::Client,
    captured: Arc<Mutex<Vec<(String, String)>>>,
) -> std::io::Result<()> {
    // Read until we see the header terminator \r\n\r\n.
    let mut buf = Vec::with_capacity(8192);
    let mut tmp = [0u8; 4096];
    let header_end = loop {
        let n = conn.read(&mut tmp).await?;
        if n == 0 {
            return Ok(());
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(idx) = find_double_crlf(&buf) {
            break idx;
        }
        if buf.len() > 64 * 1024 {
            // request headers too large for this minimal forwarder
            return Ok(());
        }
    };

    let header_str = std::str::from_utf8(&buf[..header_end])
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let mut lines = header_str.split("\r\n");
    let request_line = lines.next().unwrap_or("");
    let parts: Vec<&str> = request_line.split_whitespace().collect();
    let method = parts.first().copied().unwrap_or("GET");
    let path = parts.get(1).copied().unwrap_or("/");

    let mut local_headers: Vec<(String, String)> = Vec::new();
    let mut content_length: usize = 0;
    for line in lines {
        if let Some((k, v)) = line.split_once(": ") {
            if k.eq_ignore_ascii_case("content-length") {
                content_length = v.parse().unwrap_or(0);
            }
            local_headers.push((k.to_string(), v.to_string()));
        }
    }
    // Record the captured headers so the test runner can assert against them.
    {
        let mut cap = captured.lock().unwrap();
        cap.extend(local_headers.iter().cloned());
    }

    // Read the request body (already partially in `buf`).
    let body_start = header_end + 4;
    let mut body: Vec<u8> = buf[body_start..].to_vec();
    let remaining = content_length.saturating_sub(body.len());
    if remaining > 0 {
        let mut rest = vec![0u8; remaining];
        conn.read_exact(&mut rest).await?;
        body.extend(rest);
    }

    // Forward to the real agent.
    let target = format!("{}{}", upstream, path);
    let method_typed: reqwest::Method = method.parse().unwrap_or(reqwest::Method::POST);
    let mut req = client.request(method_typed, &target);
    for (k, v) in &local_headers {
        if k.eq_ignore_ascii_case("host") || k.eq_ignore_ascii_case("content-length") {
            continue;
        }
        req = req.header(k, v);
    }
    req = req.body(body);

    match req.send().await {
        Ok(resp) => {
            let status = resp.status();
            let headers = resp.headers().clone();
            let resp_body = resp.bytes().await.unwrap_or_default();
            let mut head = format!(
                "HTTP/1.1 {} {}\r\n",
                status.as_u16(),
                status.canonical_reason().unwrap_or("")
            );
            for (k, v) in headers.iter() {
                let kn = k.as_str();
                if kn.eq_ignore_ascii_case("transfer-encoding")
                    || kn.eq_ignore_ascii_case("content-length")
                {
                    continue;
                }
                head.push_str(&format!("{}: {}\r\n", kn, v.to_str().unwrap_or("")));
            }
            head.push_str(&format!("Content-Length: {}\r\n\r\n", resp_body.len()));
            conn.write_all(head.as_bytes()).await?;
            conn.write_all(&resp_body).await?;
        }
        Err(_) => {
            // upstream is unreachable — the SDK call will fail but the
            // headers we wanted to capture were already on the wire.
            let resp = b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\n\r\n";
            conn.write_all(resp).await?;
        }
    }
    conn.flush().await?;
    Ok(())
}

fn find_double_crlf(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}
