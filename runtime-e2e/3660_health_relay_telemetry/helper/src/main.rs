//! Runtime proof — the Rust SDK relays `/health` onto its telemetry ping.
//!
//! Sister proof to the Go / Python / TypeScript / Java SDKs, which have read
//! `/health` on the telemetry path since enterprise#3619. Issue
//! axonflow-sdk-rust#88; platform contract enterprise#3660.
//!
//! This exercises the REAL client on the REAL wire: it stands up one TCP
//! listener that answers BOTH `GET /health` and `POST /v1/ping`, constructs an
//! ordinary [`AxonFlowClient`] pointed at it, and asserts on the bytes the SDK
//! actually sent. Nothing is hand-crafted and nothing is mocked — the only
//! thing this file supplies is the platform's side of the conversation.
//!
//! One scenario per process, because the SDK's 1-hour in-process guard is
//! process-wide by design: a second heartbeat in the same process is exactly
//! what it exists to suppress. `test.sh` runs the matrix.
//!
//! ```sh
//! cd runtime-e2e/3660_health_relay_telemetry/helper
//! SCENARIO=full cargo run
//!
//! # Against a live agent: /health is answered by the real platform, the ping
//! # is still captured locally so it can be asserted on.
//! AXONFLOW_LIVE_HEALTH_URL=http://localhost:8080 SCENARIO=live cargo run
//! ```

use axonflow_sdk_rust::{AxonFlowClient, AxonFlowConfig};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// What the fake platform answers on `GET /health`, and what the ping is then
/// required to carry.
struct Scenario {
    name: &'static str,
    /// Raw `/health` response body. `None` means "answer 503 with no body".
    health_body: Option<String>,
    health_status: u16,
    /// Keys required to be PRESENT on the ping, with their exact values.
    expect_present: Vec<(&'static str, String)>,
    /// Keys required to be ABSENT from the ping — asked as `has(key)`, so a
    /// JSON `null` fails just as loudly as a substituted default.
    expect_absent: Vec<&'static str>,
}

fn scenario(name: &str) -> Scenario {
    let all_four = |v: &str, t: &str, e: &str, d: &str| {
        vec![
            ("platform_version", v.to_string()),
            ("license_tier", t.to_string()),
            ("edition", e.to_string()),
            ("platform_deployment_mode", d.to_string()),
        ]
    };
    let relayed = [
        "platform_version",
        "license_tier",
        "edition",
        "platform_deployment_mode",
    ];

    match name {
        // The post-#3660 platform: every relay present.
        "full" => Scenario {
            name: "full",
            health_body: Some(
                r#"{"status":"healthy","version":"10.4.0","tier":"Enterprise","edition":"enterprise","deployment_mode":"self_hosted"}"#
                    .to_string(),
            ),
            health_status: 200,
            expect_present: all_four("10.4.0", "Enterprise", "enterprise", "self_hosted"),
            expect_absent: vec![],
        },
        // Any platform released BEFORE #3660: tier and version only. The two
        // new relays must be absent, not defaulted.
        "pre_3660" => Scenario {
            name: "pre_3660",
            health_body: Some(
                r#"{"status":"healthy","version":"10.3.0","tier":"Community"}"#.to_string(),
            ),
            health_status: 200,
            expect_present: vec![
                ("platform_version", "10.3.0".to_string()),
                ("license_tier", "Community".to_string()),
            ],
            expect_absent: vec!["edition", "platform_deployment_mode"],
        },
        // An agent caught inside its pre-init window. Forwarded verbatim.
        "starting" => Scenario {
            name: "starting",
            health_body: Some(r#"{"status":"starting","tier":"starting"}"#.to_string()),
            health_status: 200,
            expect_present: vec![("license_tier", "starting".to_string())],
            expect_absent: vec!["platform_version", "edition", "platform_deployment_mode"],
        },
        // /health errors. The ping must still be delivered, without the keys.
        "http_error" => Scenario {
            name: "http_error",
            health_body: None,
            health_status: 503,
            expect_present: vec![],
            expect_absent: relayed.to_vec(),
        },
        // /health answers 200 with something that is not JSON.
        "not_json" => Scenario {
            name: "not_json",
            health_body: Some("<html>upstream proxy error</html>".to_string()),
            health_status: 200,
            expect_present: vec![],
            expect_absent: relayed.to_vec(),
        },
        // A probe that SUCCEEDS with a hostile value. The dangerous case: the
        // value must arrive as a VALUE, escaped by the serializer, without
        // injecting a key or corrupting the payload.
        "hostile" => Scenario {
            name: "hostile",
            health_body: Some(
                // A quote, an injected key, a backslash and a newline.
                "{\"version\":\"10.4.0\\\", \\\"org_id\\\": \\\"pwned\\\", \\\"x\\\": \\\"\\\\\\ntail\",\"tier\":\"Community\"}"
                    .to_string(),
            ),
            health_status: 200,
            expect_present: vec![
                (
                    "platform_version",
                    "10.4.0\", \"org_id\": \"pwned\", \"x\": \"\\\ntail".to_string(),
                ),
                ("license_tier", "Community".to_string()),
            ],
            expect_absent: vec!["x", "edition", "platform_deployment_mode"],
        },
        // A 10 KB tier. Dropped whole; everything else survives, and the ping
        // stays far inside the checkpoint service's 64 KiB body cap.
        "oversized_value" => Scenario {
            name: "oversized_value",
            health_body: Some(format!(
                r#"{{"version":"10.4.0","tier":"{}"}}"#,
                "T".repeat(10 * 1024)
            )),
            health_status: 200,
            expect_present: vec![("platform_version", "10.4.0".to_string())],
            expect_absent: vec!["license_tier", "edition", "platform_deployment_mode"],
        },
        // Values relayed from a LIVE agent. Asserted structurally (the tier
        // and version depend on the stack under test), so this proves the
        // real platform's real response reaches the wire.
        "live" => Scenario {
            name: "live",
            health_body: None,
            health_status: 0,
            expect_present: vec![],
            expect_absent: vec![],
        },
        other => {
            eprintln!("unknown SCENARIO {other:?}");
            std::process::exit(2);
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let name = std::env::var("SCENARIO").unwrap_or_else(|_| "full".to_string());
    let sc = scenario(&name);
    let live_health = std::env::var("AXONFLOW_LIVE_HEALTH_URL").ok();

    if sc.name == "live" && live_health.is_none() {
        eprintln!("SCENARIO=live requires AXONFLOW_LIVE_HEALTH_URL=<agent base url>");
        std::process::exit(2);
    }

    // The 7-day stamp would otherwise suppress the ping on a machine that has
    // pinged recently. Same scrub as the v9.1 helper.
    if let Some(cache_dir) = dirs::cache_dir() {
        let _ = std::fs::remove_file(
            cache_dir
                .join("axonflow")
                .join("rust-telemetry-last-sent"),
        );
    }

    // Telemetry must be ON for this proof; make it explicit rather than
    // inheriting whatever the shell had.
    std::env::remove_var("AXONFLOW_TELEMETRY");
    std::env::remove_var("AXONFLOW_TRY");
    std::env::set_var("ORG_ID", "runtime-e2e-org");

    let captured: Arc<Mutex<Option<Vec<u8>>>> = Arc::new(Mutex::new(None));
    let health_hits = Arc::new(Mutex::new(0usize));

    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let base = format!("http://127.0.0.1:{port}");

    {
        let captured = Arc::clone(&captured);
        let health_hits = Arc::clone(&health_hits);
        let health_body = sc.health_body.clone();
        let health_status = sc.health_status;
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    continue;
                };
                let captured = Arc::clone(&captured);
                let health_hits = Arc::clone(&health_hits);
                let health_body = health_body.clone();
                tokio::spawn(async move {
                    let Some((head, body)) = read_request(&mut socket).await else {
                        return;
                    };
                    let is_ping = head.starts_with("POST ");
                    let response = if is_ping {
                        *captured.lock().unwrap() = Some(body);
                        http_response(200, Some("{\"latest_version\":null}"))
                    } else {
                        *health_hits.lock().unwrap() += 1;
                        match &health_body {
                            Some(b) => http_response(health_status, Some(b)),
                            None => http_response(health_status, None),
                        }
                    };
                    let _ = socket.write_all(&response).await;
                    let _ = socket.flush().await;
                });
            }
        });
    }

    std::env::set_var("AXONFLOW_CHECKPOINT_URL", format!("{base}/v1/ping"));

    // The endpoint the SDK will probe. In `live` mode that is the real agent,
    // so the /health half of this proof runs against the platform itself.
    let endpoint = live_health.clone().unwrap_or_else(|| base.clone());

    println!("[{}] listener on {base}", sc.name);
    println!("[{}] SDK endpoint {endpoint}", sc.name);

    // The real public entry point. Construction is one of the two trigger
    // sites; nothing here reaches into the heartbeat module.
    let _client = AxonFlowClient::new(AxonFlowConfig::new(&endpoint))?;

    let deadline = Instant::now() + Duration::from_secs(20);
    let body = loop {
        if let Some(b) = captured.lock().unwrap().clone() {
            break b;
        }
        if Instant::now() > deadline {
            eprintln!("[{}] FAIL: no ping reached the listener in 20s", sc.name);
            std::process::exit(1);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };

    let ping: serde_json::Value = serde_json::from_slice(&body).map_err(|e| {
        format!(
            "ping body was not valid JSON ({e}); a relayed value corrupted the payload: {}",
            String::from_utf8_lossy(&body)
        )
    })?;
    println!(
        "[{}] ping: {}",
        sc.name,
        serde_json::to_string_pretty(&ping)?
    );

    let mut failures: Vec<String> = Vec::new();

    // Shape that must hold in every scenario — a failed probe costs
    // dimensions, never the ping.
    for (key, want) in [
        ("telemetry_type", "sdk"),
        ("sdk", "rust"),
        ("org_id", "runtime-e2e-org"),
    ] {
        if ping.get(key).and_then(|v| v.as_str()) != Some(want) {
            failures.push(format!("{key}: expected {want:?}, got {:?}", ping.get(key)));
        }
    }
    match ping.get("runtime_version").and_then(|v| v.as_str()) {
        Some(v) if v.starts_with("rustc ") => {}
        other => failures.push(format!(
            "runtime_version: expected a real toolchain, got {other:?}"
        )),
    }

    if sc.name == "live" {
        // A live agent always answers `tier` and `version`; `edition` and
        // `deployment_mode` only once enterprise#3660 has shipped, so they are
        // reported rather than required.
        for key in ["platform_version", "license_tier"] {
            match ping.get(key).and_then(|v| v.as_str()) {
                Some(v) if !v.is_empty() => println!("[live] relayed {key} = {v:?}"),
                other => failures.push(format!(
                    "{key}: live /health should have supplied this, got {other:?}"
                )),
            }
        }
        for key in ["edition", "platform_deployment_mode"] {
            match ping.get(key).and_then(|v| v.as_str()) {
                Some(v) => println!("[live] relayed {key} = {v:?} (platform has #3660)"),
                None => println!("[live] {key} absent — platform predates #3660, as expected"),
            }
        }
    } else {
        for (key, want) in &sc.expect_present {
            match ping.get(*key).and_then(|v| v.as_str()) {
                Some(got) if got == want => {}
                other => failures.push(format!("{key}: expected {want:?}, got {other:?}")),
            }
        }
        for key in &sc.expect_absent {
            if ping.get(*key).is_some() {
                failures.push(format!(
                    "{key}: must be ABSENT (not null, not a default), found {:?}",
                    ping.get(*key)
                ));
            }
        }
    }

    // Exactly one /health fetch per heartbeat. A second would double the
    // telemetry path's blocking budget and its failure surface.
    let hits = *health_hits.lock().unwrap();
    let expected_hits = if live_health.is_some() { 0 } else { 1 };
    if hits != expected_hits {
        failures.push(format!(
            "expected {expected_hits} local /health fetch(es), saw {hits}"
        ));
    }

    // The ping must stay well inside the checkpoint service's 64 KiB cap —
    // the bound an uncapped relay would blow through.
    if body.len() >= 64 * 1024 {
        failures.push(format!(
            "ping body is {} bytes, at or over the receiver's 64 KiB limit",
            body.len()
        ));
    }

    if failures.is_empty() {
        println!("[{}] PASS ({} bytes on the wire)", sc.name, body.len());
        Ok(())
    } else {
        for f in &failures {
            eprintln!("[{}] FAIL: {f}", sc.name);
        }
        std::process::exit(1);
    }
}

/// Read one HTTP request: the head, then exactly `Content-Length` body bytes.
/// Reading to idle instead would race a fast client's connection reuse.
async fn read_request(socket: &mut tokio::net::TcpStream) -> Option<(String, Vec<u8>)> {
    let mut buf: Vec<u8> = Vec::with_capacity(8192);
    let mut chunk = [0u8; 4096];

    let head_end = loop {
        if let Some(i) = find_subslice(&buf, b"\r\n\r\n") {
            break i;
        }
        let n = tokio::time::timeout(Duration::from_secs(5), socket.read(&mut chunk))
            .await
            .ok()?
            .ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
    };

    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let content_length = head
        .lines()
        .find(|l| l.to_ascii_lowercase().starts_with("content-length:"))
        .and_then(|l| l.split(':').nth(1))
        .and_then(|v| v.trim().parse::<usize>().ok())
        .unwrap_or(0);

    let body_start = head_end + 4;
    let mut body = buf[body_start..].to_vec();
    while body.len() < content_length {
        let n = tokio::time::timeout(Duration::from_secs(5), socket.read(&mut chunk))
            .await
            .ok()?
            .ok()?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    body.truncate(content_length);
    Some((head, body))
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|w| w == needle)
}

fn http_response(status: u16, body: Option<&str>) -> Vec<u8> {
    let reason = match status {
        200 => "OK",
        503 => "Service Unavailable",
        _ => "Status",
    };
    let body = body.unwrap_or("");
    format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}
