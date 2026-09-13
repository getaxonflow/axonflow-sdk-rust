//! The PEP capability handshake on the wire: which requests carry it, whose
//! declaration they carry, and which never carry it.
//!
//! The platform reads `X-Axonflow-PEP-Handshake` on `/api/v1/decide`, the
//! AuthZEN evaluation route (single and bulk) and the MCP check routes. This
//! SDK reaches check-input only as the engine round-trip of `fulfill_request`
//! and `decide_and_fulfill`, and has no call that sends check-output. Every
//! other route, `/api/request` above all, must never see the header.
//!
//! These pin what the CLIENT sends. The proof that the platform READ it lives
//! in `runtime-e2e/pep_handshake_planes/`, which counts the agent's own
//! `axonflow_pep_handshake_total`.

use axonflow_sdk_rust::authzen::{
    AuthZenAction, AuthZenBulk, AuthZenRequest, AuthZenResource, AuthZenSubject, AUTHZEN_PATH,
    AUTHZEN_PROFILE_V1,
};
use axonflow_sdk_rust::{
    AxonFlowClient, AxonFlowConfig, DecideRequest, DecideResponse, ListDecisionsOptions,
    Obligation, ObligationFulfillment, PEPCapability, PEPHandshake, CONTENT_TYPE_TEXT, DECIDE_PATH,
    HEADER_USER_TOKEN, OBLIGATION_REDACT_PII, PEP_HANDSHAKE_HEADER, PHASE_REQUEST,
    REQUEST_REDACTION_PATH,
};
use serde_json::json;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use wiremock::matchers::{any, method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

fn declared() -> PEPHandshake {
    PEPHandshake::new(
        "sdk-rust-test",
        "https://pep.example.test",
        [PEPCapability::new("field_redact", 1)],
    )
    .expect("a valid declaration")
}

fn other() -> PEPHandshake {
    PEPHandshake::new(
        "sdk-rust-other",
        "https://pep.example.test",
        [PEPCapability::new("field_mask", 1)],
    )
    .expect("a valid declaration")
}

fn client(server: &MockServer, handshake: Option<PEPHandshake>) -> AxonFlowClient {
    AxonFlowClient::new(AxonFlowConfig {
        endpoint: server.uri(),
        pep_handshake: handshake,
        ..Default::default()
    })
    .expect("the client builds")
}

fn authzen_allow() -> serde_json::Value {
    json!({
        "decision": true,
        "context": {
            "profile": AUTHZEN_PROFILE_V1,
            "state": "ALLOW",
            "category": "allowed",
            "reason": "permitted",
            "decision_id": "dec-1",
            "schema_version": "2026-08-29"
        }
    })
}

fn an_evaluation() -> AuthZenRequest {
    AuthZenRequest::evaluating(
        AuthZenSubject::new("gateway", "sdk-rust-test"),
        AuthZenAction::new("llm.completion"),
        AuthZenResource::new("llm", "llm"),
    )
}

fn redact_obligation() -> Obligation {
    Obligation {
        r#type: OBLIGATION_REDACT_PII.into(),
        detail: None,
        fulfillment: Some(ObligationFulfillment {
            endpoint: REQUEST_REDACTION_PATH.into(),
            method: "POST".into(),
            phase: PHASE_REQUEST.into(),
            content_types: Some(vec![CONTENT_TYPE_TEXT.into()]),
        }),
    }
}

/// A decision carrying a request-phase redaction obligation, so fulfillment
/// makes its engine round-trip.
fn redacting_decision() -> DecideResponse {
    DecideResponse {
        verdict: "allow".into(),
        obligations: vec![redact_obligation()],
        ..Default::default()
    }
}

/// A stub platform. `/decide` answers `decide_body`; the engine, the AuthZEN
/// route and `/api/request` answer successfully; anything else answers `{}`.
async fn platform(decide_body: serde_json::Value) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(DECIDE_PATH))
        .respond_with(ResponseTemplate::new(200).set_body_json(decide_body))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(REQUEST_REDACTION_PATH))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "allowed": true,
            "redacted": false,
            "redaction_evaluated": true,
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(AUTHZEN_PATH))
        .respond_with(ResponseTemplate::new(200).set_body_json(authzen_allow()))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/request"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "success": true,
            "request_id": "rid",
            "data": {},
        })))
        .mount(&server)
        .await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .with_priority(10)
        .mount(&server)
        .await;
    server
}

async fn allowing_platform() -> MockServer {
    platform(json!({"verdict": "allow"})).await
}

/// Every request the stub received, in order.
async fn received(server: &MockServer) -> Vec<Request> {
    server
        .received_requests()
        .await
        .expect("request recording is on")
}

/// The handshake header values on every request to `route`, one entry per
/// request: `None` where the request carried none.
async fn handshakes_at(server: &MockServer, route: &str) -> Vec<Option<String>> {
    received(server)
        .await
        .iter()
        .filter(|r| r.url.path() == route)
        .map(|r| {
            let values: Vec<_> = r.headers.get_all(PEP_HANDSHAKE_HEADER).iter().collect();
            // The platform refuses a repeated header with a 400.
            assert!(
                values.len() <= 1,
                "{route} carried the header {} times",
                values.len()
            );
            values
                .first()
                .map(|v| v.to_str().expect("base64url is ASCII").to_string())
        })
        .collect()
}

fn value_of(h: &PEPHandshake) -> Option<String> {
    Some(h.header_value().to_string())
}

// ---------------------------------------------------------------------------
// Present on the three planes
// ---------------------------------------------------------------------------

#[tokio::test]
async fn decide_presents_the_client_declaration() {
    let server = allowing_platform().await;
    client(&server, Some(declared()))
        .decide(DecideRequest::new("tool", "look up the weather"))
        .await
        .expect("allow");
    assert_eq!(
        handshakes_at(&server, DECIDE_PATH).await,
        vec![value_of(&declared())]
    );
}

#[tokio::test]
async fn evaluate_presents_the_client_declaration() {
    let server = allowing_platform().await;
    client(&server, Some(declared()))
        .evaluate(an_evaluation())
        .await
        .expect("allow");
    assert_eq!(
        handshakes_at(&server, AUTHZEN_PATH).await,
        vec![value_of(&declared())]
    );
}

#[tokio::test]
async fn evaluate_all_presents_the_client_declaration() {
    let server = allowing_platform().await;
    client(&server, Some(declared()))
        .evaluate_all(AuthZenBulk::over([an_evaluation()]))
        .await
        .expect("allow");
    assert_eq!(
        handshakes_at(&server, AUTHZEN_PATH).await,
        vec![value_of(&declared())]
    );
}

#[tokio::test]
async fn the_authzen_profile_header_is_still_sent_beside_it() {
    let server = allowing_platform().await;
    client(&server, Some(declared()))
        .evaluate(an_evaluation())
        .await
        .expect("allow");
    let requests = received(&server).await;
    let sent = requests
        .iter()
        .find(|r| r.url.path() == AUTHZEN_PATH)
        .expect("an evaluation");
    assert_eq!(
        sent.headers
            .get(axonflow_sdk_rust::AUTHZEN_PROFILE_HEADER)
            .and_then(|v| v.to_str().ok()),
        Some(AUTHZEN_PROFILE_V1)
    );
}

#[tokio::test]
async fn fulfill_request_presents_it_on_the_check_input_round_trip() {
    let server = allowing_platform().await;
    client(&server, Some(declared()))
        .fulfill_request(&redacting_decision(), "no pii here")
        .await
        .expect("the engine ran and found nothing");
    assert_eq!(
        handshakes_at(&server, REQUEST_REDACTION_PATH).await,
        vec![value_of(&declared())]
    );
}

#[tokio::test]
async fn decide_and_fulfill_presents_it_on_both_requests() {
    let server = platform(json!({
        "verdict": "allow",
        "obligations": [serde_json::to_value(redact_obligation()).unwrap()],
    }))
    .await;
    client(&server, Some(declared()))
        .decide_and_fulfill(DecideRequest::new("tool", "no pii here"))
        .await
        .expect("allowed and fulfilled");
    assert_eq!(
        handshakes_at(&server, DECIDE_PATH).await,
        vec![value_of(&declared())]
    );
    assert_eq!(
        handshakes_at(&server, REQUEST_REDACTION_PATH).await,
        vec![value_of(&declared())]
    );
}

// ---------------------------------------------------------------------------
// Absent: no declaration, and the routes that do not read it
// ---------------------------------------------------------------------------

/// There is no default declaration: a client given none sends none, on every
/// plane that would read one.
#[tokio::test]
async fn a_client_with_no_declaration_sends_no_header_on_any_plane() {
    let server = platform(json!({
        "verdict": "allow",
        "obligations": [serde_json::to_value(redact_obligation()).unwrap()],
    }))
    .await;
    let c = client(&server, None);
    c.decide_and_fulfill(DecideRequest::new("tool", "no pii here"))
        .await
        .expect("allowed and fulfilled");
    c.evaluate(an_evaluation()).await.expect("allow");
    c.evaluate_all(AuthZenBulk::over([an_evaluation()]))
        .await
        .expect("allow");
    assert_eq!(handshakes_at(&server, DECIDE_PATH).await, vec![None]);
    assert_eq!(
        handshakes_at(&server, REQUEST_REDACTION_PATH).await,
        vec![None]
    );
    assert_eq!(handshakes_at(&server, AUTHZEN_PATH).await, vec![None, None]);
}

/// `/api/request` does not read the declaration, so a declaring client must
/// not send it there: not on `proxy_llm_call`, and not on `query_connector`,
/// which is dispatched through the same route.
#[tokio::test]
async fn the_declaration_never_reaches_api_request() {
    let server = allowing_platform().await;
    let c = client(&server, Some(declared()));
    c.proxy_llm_call("", "hello", "chat", HashMap::new())
        .await
        .expect("a 200 envelope");
    c.query_connector("", "postgres", "SELECT 1", HashMap::new())
        .await
        .expect("a 200 envelope");
    let at_request = handshakes_at(&server, "/api/request").await;
    assert_eq!(
        at_request,
        vec![None, None],
        "two requests, neither carrying it"
    );
}

/// Every other route this client reaches, driven with a declaring client:
/// nothing but the three planes may carry the header. Results are ignored;
/// what is asserted is what was SENT.
#[tokio::test]
async fn the_declaration_never_reaches_a_route_that_does_not_read_it() {
    let server = allowing_platform().await;
    let c = client(&server, Some(declared()));
    let _ = c.list_connectors().await;
    let _ = c.get_connector("postgres").await;
    let _ = c.get_connector_health("postgres").await;
    let _ = c.get_plan_status("plan-1").await;
    let _ = c.cancel_plan("plan-1", None).await;
    let _ = c.explain_decision("dec-1").await;
    let _ = c.list_decisions(ListDecisionsOptions::default()).await;
    let _ = c.list_hitl_queue(Default::default()).await;
    let _ = c.get_hitl_stats().await;

    let requests = received(&server).await;
    let routes: Vec<&str> = requests.iter().map(|r| r.url.path()).collect();
    assert!(
        routes.len() >= 9,
        "positive control: every call must have reached the stub, got {routes:?}"
    );
    for r in &requests {
        assert!(
            r.headers.get(PEP_HANDSHAKE_HEADER).is_none(),
            "{} {} carried the declaration",
            r.method,
            r.url.path()
        );
    }
}

// ---------------------------------------------------------------------------
// Whose declaration: the derived client
// ---------------------------------------------------------------------------

/// The per-call form: a derived client presents its own declaration, and the
/// client it was derived from keeps presenting its own.
#[tokio::test]
async fn with_pep_handshake_replaces_the_declaration_on_the_derived_client_only() {
    let server = allowing_platform().await;
    let base = client(&server, Some(declared()));
    base.with_pep_handshake(other())
        .decide(DecideRequest::new("tool", "one"))
        .await
        .expect("allow");
    base.decide(DecideRequest::new("tool", "two"))
        .await
        .expect("allow");
    assert_eq!(
        handshakes_at(&server, DECIDE_PATH).await,
        vec![value_of(&other()), value_of(&declared())]
    );
}

#[tokio::test]
async fn with_pep_handshake_declares_on_a_client_that_declared_nothing() {
    let server = allowing_platform().await;
    let base = client(&server, None);
    base.with_pep_handshake(declared())
        .evaluate(an_evaluation())
        .await
        .expect("allow");
    base.evaluate(an_evaluation()).await.expect("allow");
    assert_eq!(
        handshakes_at(&server, AUTHZEN_PATH).await,
        vec![value_of(&declared()), None]
    );
}

/// The derived declaration flows into the engine round-trip of the derived
/// client's fulfillment, not the base client's.
#[tokio::test]
async fn decide_and_fulfill_on_a_derived_client_carries_its_declaration_into_the_engine() {
    let server = platform(json!({
        "verdict": "allow",
        "obligations": [serde_json::to_value(redact_obligation()).unwrap()],
    }))
    .await;
    client(&server, Some(declared()))
        .with_pep_handshake(other())
        .decide_and_fulfill(DecideRequest::new("tool", "no pii here"))
        .await
        .expect("allowed and fulfilled");
    assert_eq!(
        handshakes_at(&server, DECIDE_PATH).await,
        vec![value_of(&other())]
    );
    assert_eq!(
        handshakes_at(&server, REQUEST_REDACTION_PATH).await,
        vec![value_of(&other())]
    );
}

/// A client derived for a person keeps the enforcement point's declaration.
#[tokio::test]
async fn as_user_keeps_the_declaration() {
    let server = allowing_platform().await;
    client(&server, Some(declared()))
        .as_user("alice-token")
        .decide(DecideRequest::new("tool", "hi"))
        .await
        .expect("allow");
    assert_eq!(
        handshakes_at(&server, DECIDE_PATH).await,
        vec![value_of(&declared())]
    );
}

/// A client derived for a declaration keeps the person's identity.
#[tokio::test]
async fn with_pep_handshake_keeps_the_read_identity() {
    let server = allowing_platform().await;
    client(&server, None)
        .as_user("alice-token")
        .with_pep_handshake(declared())
        .decide(DecideRequest::new("tool", "hi"))
        .await
        .expect("allow");
    let requests = received(&server).await;
    let sent = requests
        .iter()
        .find(|r| r.url.path() == DECIDE_PATH)
        .expect("a decide");
    assert_eq!(
        sent.headers
            .get(HEADER_USER_TOKEN)
            .and_then(|v| v.to_str().ok()),
        Some("alice-token")
    );
    assert_eq!(
        sent.headers
            .get(PEP_HANDSHAKE_HEADER)
            .and_then(|v| v.to_str().ok()),
        Some(declared().header_value())
    );
}

#[tokio::test]
async fn the_config_builder_declares_it() {
    let server = allowing_platform().await;
    AxonFlowClient::new(AxonFlowConfig::new(server.uri()).with_pep_handshake(declared()))
        .expect("the client builds")
        .decide(DecideRequest::new("tool", "hi"))
        .await
        .expect("allow");
    assert_eq!(
        handshakes_at(&server, DECIDE_PATH).await,
        vec![value_of(&declared())]
    );
}

// ---------------------------------------------------------------------------
// Structure: where the header CAN be attached
// ---------------------------------------------------------------------------

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("readable src dir") {
        let p = entry.expect("dir entry").path();
        if p.is_dir() {
            rust_files(&p, out);
        } else if p.extension().is_some_and(|e| e == "rs") {
            out.push(p);
        }
    }
}

/// A source file's shipped code: comment lines dropped, and everything from a
/// `#[cfg(test)]` module on cut, since a test module is where a route is named
/// without being called.
fn shipped_code(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut kept = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let next = lines.get(i + 1).map_or("", |l| l.trim_start());
        if line.trim() == "#[cfg(test)]" && next.starts_with("mod ") {
            break;
        }
        if !line.trim_start().starts_with("//") {
            kept.push(*line);
        }
    }
    kept.join("\n")
}

/// `(path relative to the crate, occurrences of needle in shipped code)` for
/// every source file where it occurs.
fn census(needle: &str) -> Vec<(String, usize)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    rust_files(&root.join("src"), &mut files);
    let mut hits: Vec<(String, usize)> = files
        .iter()
        .filter_map(|f| {
            let text = std::fs::read_to_string(f).expect("readable source");
            let n = shipped_code(&text).matches(needle).count();
            let rel = f
                .strip_prefix(root)
                .expect("under the crate")
                .to_string_lossy()
                .replace('\\', "/");
            (n > 0).then_some((rel, n))
        })
        .collect();
    hits.sort();
    hits
}

/// The declaration is attached by exactly three call sites: `decide` and the
/// check-input fulfillment in `pep.rs`, and the AuthZEN evaluation's one
/// transport path. A new call site (a typed-policy route, `/api/request`) must
/// fail this until someone decides the platform reads it there.
#[tokio::test]
async fn the_declaration_is_attached_at_exactly_the_three_resolving_call_sites() {
    assert_eq!(
        census(".pep_handshake_header()"),
        vec![
            ("src/authzen/mod.rs".to_string(), 1),
            ("src/pep.rs".to_string(), 2)
        ]
    );
}

/// This SDK has no call that sends MCP check-output: the path is named once,
/// by its constant, and the constant is only defined and re-exported. If a
/// call is added, the declaration must be attached to it too.
#[tokio::test]
async fn no_call_site_sends_mcp_check_output() {
    assert_eq!(
        census("/api/v1/mcp/check-output"),
        vec![("src/pep.rs".to_string(), 1)]
    );
    assert_eq!(
        census("RESPONSE_REDACTION_PATH"),
        vec![("src/lib.rs".to_string(), 1), ("src/pep.rs".to_string(), 1)]
    );
}
