//! The PEP capability handshake on the wire: which requests carry it, whose
//! declaration they carry, and which never carry it.
//!
//! The platform reads `X-Axonflow-PEP-Handshake` on `/api/v1/decide`, the
//! AuthZEN evaluation route (single and bulk) and the MCP check routes, and on
//! the gateway pre-check and MCP `tools/call`. This SDK reaches check-input
//! only as the engine round-trip of `fulfill_request` and `decide_and_fulfill`,
//! and calls neither check-output, the pre-check nor `tools/call`. Every other
//! route, `/api/request` above all, must never see the header.
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
    let _ = c
        .proxy_llm_call("", "hello", "chat", HashMap::new())
        .await
        .expect("a 200 envelope");
    let _ = c
        .query_connector("", "postgres", "SELECT 1", HashMap::new())
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

/// There is no default declaration. A client built the way a caller builds one,
/// from `AxonFlowConfig::new` or `Default`, never naming the field, sends no
/// header. The other no-declaration test names `pep_handshake: None` in its
/// struct literal, so it cannot see what `Default` supplies; this one can.
#[tokio::test]
async fn the_default_config_declares_nothing() {
    assert!(AxonFlowConfig::default().pep_handshake.is_none());
    let server = allowing_platform().await;
    AxonFlowClient::new(AxonFlowConfig::new(server.uri()))
        .expect("the client builds")
        .decide(DecideRequest::new("tool", "hi"))
        .await
        .expect("allow");
    assert_eq!(handshakes_at(&server, DECIDE_PATH).await, vec![None]);
}

/// The declaration's accessors return what was declared, the capabilities in
/// canonical order and the header value non-empty.
#[test]
fn the_accessors_return_what_was_declared() {
    let declared = PEPHandshake::new(
        "gw.request-1",
        "https://pep.example.test",
        [
            PEPCapability::new("field_redact", 2),
            PEPCapability::new("field_mask", 1),
        ],
    )
    .expect("a valid declaration");
    assert_eq!(declared.pep_id(), "gw.request-1");
    assert_eq!(declared.audience(), "https://pep.example.test");
    assert_eq!(
        declared.capabilities(),
        &[
            PEPCapability::new("field_mask", 1),
            PEPCapability::new("field_redact", 2)
        ]
    );
    assert!(!declared.header_value().is_empty());
}

// ---------------------------------------------------------------------------
// Structure: where the header CAN be attached
// ---------------------------------------------------------------------------

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("readable src dir") {
        let p = entry.expect("dir entry").path();
        let name = p.file_name().map(|n| n.to_string_lossy().to_string());
        if p.is_dir() {
            rust_files(&p, out);
        } else if p.extension().is_some_and(|e| e == "rs")
            && !name.is_some_and(|n| n.ends_with("_tests.rs"))
        {
            out.push(p);
        }
    }
}

/// What one source file's SHIPPED code names, read as syntax rather than text,
/// so whitespace, a line break or a fully-qualified call cannot hide a use.
///
/// Every identifier counts, including those in a macro body, where URLs and
/// header names are formatted (`format!("{}{}", base, PATH)`), and so does
/// every string literal. Attributes (doc comments, `cfg`s) are not code and are
/// skipped, and so is every item marked `#[cfg(test)]`, since a test is where a
/// route is named without being called.
#[derive(Default)]
struct Census {
    idents: std::collections::BTreeMap<String, usize>,
    strings: Vec<String>,
}

fn is_cfg_test(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|a| {
        a.path().is_ident("cfg") && a.parse_args::<syn::Ident>().is_ok_and(|i| i == "test")
    })
}

fn item_attrs(item: &syn::Item) -> &[syn::Attribute] {
    match item {
        syn::Item::Const(i) => &i.attrs,
        syn::Item::Enum(i) => &i.attrs,
        syn::Item::Fn(i) => &i.attrs,
        syn::Item::Impl(i) => &i.attrs,
        syn::Item::Macro(i) => &i.attrs,
        syn::Item::Mod(i) => &i.attrs,
        syn::Item::Static(i) => &i.attrs,
        syn::Item::Struct(i) => &i.attrs,
        syn::Item::Trait(i) => &i.attrs,
        syn::Item::Type(i) => &i.attrs,
        syn::Item::Use(i) => &i.attrs,
        _ => &[],
    }
}

impl<'ast> syn::visit::Visit<'ast> for Census {
    fn visit_attribute(&mut self, _: &'ast syn::Attribute) {}

    fn visit_item(&mut self, item: &'ast syn::Item) {
        if !is_cfg_test(item_attrs(item)) {
            syn::visit::visit_item(self, item);
        }
    }

    fn visit_impl_item_fn(&mut self, f: &'ast syn::ImplItemFn) {
        if !is_cfg_test(&f.attrs) {
            syn::visit::visit_impl_item_fn(self, f);
        }
    }

    fn visit_ident(&mut self, ident: &'ast syn::Ident) {
        *self.idents.entry(ident.to_string()).or_default() += 1;
    }

    fn visit_lit_str(&mut self, s: &'ast syn::LitStr) {
        self.strings.push(s.value());
    }

    fn visit_lit_byte_str(&mut self, s: &'ast syn::LitByteStr) {
        self.strings
            .push(String::from_utf8_lossy(&s.value()).into_owned());
    }

    fn visit_macro(&mut self, m: &'ast syn::Macro) {
        syn::visit::visit_macro(self, m);
        let text = m.tokens.to_string();
        for word in text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')) {
            if !word.is_empty() {
                *self.idents.entry(word.to_string()).or_default() += 1;
            }
        }
        self.strings.push(text);
    }
}

/// `(path relative to the crate, count)` for every shipped source file where
/// `pick` counts something.
fn census_of(pick: impl Fn(&Census) -> usize) -> Vec<(String, usize)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    rust_files(&root.join("src"), &mut files);
    assert!(files.len() > 5, "the source walk found suspiciously little");
    let mut hits = Vec::new();
    for f in &files {
        let text = std::fs::read_to_string(f).expect("readable source");
        let parsed = syn::parse_file(&text).unwrap_or_else(|e| panic!("{}: {e}", f.display()));
        let mut census = Census::default();
        syn::visit::Visit::visit_file(&mut census, &parsed);
        let n = pick(&census);
        if n > 0 {
            let rel = f
                .strip_prefix(root)
                .expect("under the crate")
                .to_string_lossy()
                .replace('\\', "/");
            hits.push((rel, n));
        }
    }
    hits.sort();
    hits
}

fn ident(name: &'static str) -> impl Fn(&Census) -> usize {
    move |c| c.idents.get(name).copied().unwrap_or(0)
}

fn literal(fragment: &'static str) -> impl Fn(&Census) -> usize {
    move |c| {
        c.strings
            .iter()
            .filter(|s| s.to_ascii_lowercase().contains(fragment))
            .count()
    }
}

/// Letters and digits only, lower-cased, so `"X-Axonflow-PEP-Handshake"`,
/// `b"x_axonflow_pep_handshake"` and a `concat!` of its halves all contain
/// `fragment` when it is written the same way.
fn normalised(fragment: &'static str) -> impl Fn(&Census) -> usize {
    move |c| {
        c.strings
            .iter()
            .filter(|s| {
                s.chars()
                    .filter(|ch| ch.is_ascii_alphanumeric())
                    .collect::<String>()
                    .to_ascii_lowercase()
                    .contains(fragment)
            })
            .count()
    }
}

fn pins(expected: &[(&str, usize)]) -> Vec<(String, usize)> {
    expected.iter().map(|(f, n)| (f.to_string(), *n)).collect()
}

/// A positive control for the census itself: it sees shipped code, and it does
/// not see a `#[cfg(test)]` module. `pep.rs` names the check-output path in its
/// test module, so a census that read test code would count it there.
#[test]
fn the_census_sees_shipped_code_and_skips_test_modules() {
    assert!(
        census_of(ident("dispatch"))
            .iter()
            .any(|(f, n)| f == "src/client.rs" && *n > 0),
        "the census must see client.rs's dispatch funnel"
    );
    let in_pep_tests = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/pep.rs"))
        .expect("pep.rs")
        .matches("/api/v1/mcp/check-output")
        .count();
    assert!(
        in_pep_tests >= 2,
        "the control needs pep.rs to name the path in its tests too"
    );
    assert_eq!(
        census_of(literal("check-output")),
        pins(&[("src/pep.rs", 1)])
    );
}

/// The census skips files named `*_tests.rs`, which today is only the
/// heartbeat's `#[cfg(test)] #[path]` module. A shipped file with that suffix
/// would be invisible to every pin, so the skipped set is pinned too.
#[test]
fn the_only_file_the_census_skips_is_the_heartbeat_test_module() {
    fn walk(dir: &Path, root: &Path, out: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).expect("readable src dir") {
            let p = entry.expect("dir entry").path();
            if p.is_dir() {
                walk(&p, root, out);
            } else if p.to_string_lossy().ends_with("_tests.rs") {
                let rel = p.strip_prefix(root).expect("under the crate");
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut skipped = Vec::new();
    walk(&root.join("src"), root, &mut skipped);
    skipped.sort();
    assert_eq!(skipped, vec!["src/heartbeat_tests.rs".to_string()]);
}

/// The declaration is attached through ONE accessor, `pep_handshake_header`,
/// called by the one declaring POST (used by `decide` and the check-input
/// fulfillment) and by the AuthZEN evaluation's transport path. A new call site
/// (a typed-policy route, `/api/request`) fails this, in any spelling, until
/// someone decides the platform reads the declaration there.
#[test]
fn the_declaration_is_attached_through_one_accessor_and_one_declaring_post() {
    assert_eq!(
        census_of(ident("pep_handshake_header")),
        pins(&[("src/authzen/mod.rs", 1), ("src/client.rs", 2)])
    );
    assert_eq!(
        census_of(ident("checked_post_json_declaring")),
        pins(&[("src/client.rs", 1), ("src/pep.rs", 2)])
    );
}

/// Nothing but the accessor names the header, so a new site cannot attach it
/// by writing the header name itself.
#[test]
fn only_the_accessor_names_the_header() {
    assert_eq!(
        census_of(ident("PEP_HANDSHAKE_HEADER")),
        pins(&[
            ("src/client.rs", 2),
            ("src/lib.rs", 1),
            ("src/pep_handshake.rs", 3)
        ])
    );
    // Any spelling of the header name in a string, a byte string or a macro
    // body, including one split across a concat!, is the constant's alone.
    assert_eq!(
        census_of(normalised("xaxonflowpephandshake")),
        pins(&[("src/pep_handshake.rs", 1)])
    );
}

/// This SDK has no call that sends MCP check-output: the path is named once,
/// by its constant, and the constant is only defined and re-exported. If a
/// call is added, the declaration must be attached to it too.
#[test]
fn no_call_site_sends_mcp_check_output() {
    assert_eq!(
        census_of(literal("check-output")),
        pins(&[("src/pep.rs", 1)])
    );
    assert_eq!(
        census_of(ident("RESPONSE_REDACTION_PATH")),
        pins(&[("src/lib.rs", 1), ("src/pep.rs", 1)])
    );
}

/// The platform also reads the declaration on the gateway pre-check and on MCP
/// `tools/call`. This SDK calls neither; if a call is added, the declaration
/// must be attached to it too.
#[test]
fn no_call_site_reaches_the_other_planes_that_read_it() {
    for route in ["/api/policy/pre-check", "/api/v1/mcp-server", "tools/call"] {
        assert_eq!(census_of(literal(route)), pins(&[]), "{route}");
    }
}
