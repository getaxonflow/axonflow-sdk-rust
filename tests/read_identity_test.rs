//! Read-path per-user identity (`X-User-Token`) and the read-scope contract
//! (platform #2922).
//!
//! Companion to `src/read_identity.rs`.

use axonflow_sdk_rust::{
    AxonFlowClient, AxonFlowConfig, AxonFlowError, ListDecisionsOptions, ReadScope,
    HEADER_READ_SCOPE, HEADER_USER_TOKEN,
};
use serde_json::json;
use std::fs;
use std::path::Path;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

/// Distinctive on purpose: the leak assertions grep whole strings for it, and a
/// value like "tok" would match by accident.
const TEST_TOKEN: &str = "eyJhbGciOiJIUzI1NiJ9.SENTINEL-USER-TOKEN-a7f3c91e.sig";

fn client(uri: &str, user_token: Option<&str>) -> AxonFlowClient {
    AxonFlowClient::new(AxonFlowConfig {
        endpoint: uri.to_string(),
        client_id: Some("org".into()),
        client_secret: Some("secret".into()),
        user_token: user_token.map(str::to_string),
        ..AxonFlowConfig::new(uri)
    })
    .expect("client")
}

fn row_page() -> serde_json::Value {
    json!({"decisions": [{
        "decision_id": "d1",
        "timestamp": "2026-04-17T12:00:00Z",
        "decision": "blocked"
    }]})
}

fn explanation() -> serde_json::Value {
    json!({
        "decision_id": "d1",
        "timestamp": "2026-04-17T12:00:00Z",
        "decision": "blocked",
        "policy_matches": [],
        "reason": "",
        "override_available": false,
        "historical_hit_count_session": 0
    })
}

// ==========================================================================
// Option plumbing: present when configured, absent when not
// ==========================================================================

#[tokio::test]
async fn no_identity_header_when_none_configured() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/decisions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(row_page()))
        .mount(&server)
        .await;

    client(&server.uri(), None)
        .list_decisions(ListDecisionsOptions::default())
        .await
        .expect("list");

    let requests = server.received_requests().await.unwrap();
    assert!(
        requests[0].headers.get(HEADER_USER_TOKEN).is_none(),
        "a client with no identity configured must send no identity header at all, not an empty one"
    );
}

#[tokio::test]
async fn client_level_identity_travels_on_every_read() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/decisions"))
        .and(header(HEADER_USER_TOKEN, TEST_TOKEN))
        .respond_with(ResponseTemplate::new(200).set_body_json(row_page()))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/decisions/d1/explain"))
        .and(header(HEADER_USER_TOKEN, TEST_TOKEN))
        .respond_with(ResponseTemplate::new(200).set_body_json(explanation()))
        .expect(1)
        .mount(&server)
        .await;

    let axonflow = client(&server.uri(), Some(TEST_TOKEN));
    axonflow
        .list_decisions(ListDecisionsOptions::default())
        .await
        .expect("list");
    let _ = axonflow.explain_decision("d1").await.expect("explain");
    // The `.expect(1)` mounts assert the header matched; wiremock fails on drop
    // if either was not called exactly once.
}

#[tokio::test]
async fn per_call_identity_overrides_the_client_level_one() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/decisions/d1/explain"))
        .and(header(HEADER_USER_TOKEN, TEST_TOKEN))
        .respond_with(ResponseTemplate::new(200).set_body_json(explanation()))
        .expect(1)
        .mount(&server)
        .await;

    client(&server.uri(), Some("client-level"))
        .explain_decision_as("d1", Some(TEST_TOKEN))
        .await
        .map(|_| ())
        .expect("explain");
}

#[tokio::test]
async fn an_explicitly_empty_per_call_identity_clears_the_client_level_one() {
    // Falling back would make the option unable to express the very state the
    // platform treats as distinct (ReadScope::None).
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/decisions"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"decisions": []}))
                .insert_header(HEADER_READ_SCOPE, "own-rows"),
        )
        .mount(&server)
        .await;

    client(&server.uri(), Some(TEST_TOKEN))
        .list_decisions_as(ListDecisionsOptions::default(), Some("   "))
        .await
        .expect("list");

    let requests = server.received_requests().await.unwrap();
    assert!(requests[0].headers.get(HEADER_USER_TOKEN).is_none());
}

#[tokio::test]
async fn a_per_call_identity_does_not_become_client_state() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/decisions/d1/explain"))
        .respond_with(ResponseTemplate::new(200).set_body_json(explanation()))
        .mount(&server)
        .await;

    let axonflow = client(&server.uri(), None);
    axonflow
        .explain_decision_as("d1", Some(TEST_TOKEN))
        .await
        .map(|_| ())
        .expect("first");
    let _ = axonflow.explain_decision("d1").await.expect("second");

    let requests = server.received_requests().await.unwrap();
    assert!(
        requests[1].headers.get(HEADER_USER_TOKEN).is_none(),
        "a per-call identity must not become client state"
    );
}

#[tokio::test]
async fn the_identity_is_trimmed() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/decisions/d1/explain"))
        .and(header(HEADER_USER_TOKEN, TEST_TOKEN))
        .respond_with(ResponseTemplate::new(200).set_body_json(explanation()))
        .expect(1)
        .mount(&server)
        .await;

    client(&server.uri(), Some(&format!("  {TEST_TOKEN}\n")))
        .explain_decision("d1")
        .await
        .map(|_| ())
        .expect("explain");
}

// ==========================================================================
// as_user — a derived client must own its identity
// ==========================================================================

#[tokio::test]
async fn as_user_scopes_a_derived_client() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/decisions"))
        .and(header(HEADER_USER_TOKEN, "ALICE-TOKEN"))
        .respond_with(ResponseTemplate::new(200).set_body_json(row_page()))
        .expect(1)
        .mount(&server)
        .await;

    let admin = client(&server.uri(), Some("ADMIN-TOKEN"));
    admin
        .as_user("ALICE-TOKEN")
        .list_decisions(ListDecisionsOptions::default())
        .await
        .expect("list");
}

#[tokio::test]
async fn as_user_does_not_mutate_the_client_it_derived_from() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/decisions"))
        .and(header(HEADER_USER_TOKEN, "ADMIN-TOKEN"))
        .respond_with(ResponseTemplate::new(200).set_body_json(row_page()))
        .expect(1)
        .mount(&server)
        .await;

    let admin = client(&server.uri(), Some("ADMIN-TOKEN"));
    let _derived = admin.as_user("ALICE-TOKEN");
    admin
        .list_decisions(ListDecisionsOptions::default())
        .await
        .expect("list");
}

#[tokio::test]
async fn as_user_with_no_token_presents_no_identity() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/decisions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(row_page()))
        .mount(&server)
        .await;

    client(&server.uri(), Some(TEST_TOKEN))
        .as_user("")
        .list_decisions(ListDecisionsOptions::default())
        .await
        .expect("list");

    let requests = server.received_requests().await.unwrap();
    assert!(requests[0].headers.get(HEADER_USER_TOKEN).is_none());
}

// ==========================================================================
// The identity rides EVERY request, not just the two reads
// ==========================================================================

/// Every route this client can reach, not only the two role-scoped reads.
///
/// This is the finding that made round 2: the identity used to be stamped by
/// the ONE read helper that took a per-call override, so a client derived with
/// `as_user` ran every other method as the PROCESS — `list_connectors`,
/// `execute_plan`, `cancel_plan` all went out with the tenant credential and no
/// person attached, silently, while the docs promised the opposite.
///
/// The four routes below are chosen to cover the axes a single-site stamp can
/// still miss, not to enumerate methods: both transports (`http_client` and the
/// longer-timeout `map_http_client`, which is a SEPARATE `reqwest::Client` and
/// therefore a separate chance to forget), and both verbs.
async fn identities_seen_by_every_route(
    client: &AxonFlowClient,
    server: &MockServer,
) -> Vec<String> {
    let _ = client.list_connectors().await; // http GET
    let _ = client.get_plan_status("p1").await; // MAP GET
    let _ = client.execute_plan("p1", None).await; // http POST
    let _ = client.cancel_plan("p1", None).await; // MAP POST

    let seen = server.received_requests().await.unwrap();
    assert_eq!(
        seen.len(),
        4,
        "precondition: all four routes must have been dialled, or this test asserts nothing about \
         the ones that were not"
    );
    seen.iter()
        .map(|r| {
            let route = format!("{} {}", r.method, r.url.path());
            match r.headers.get(HEADER_USER_TOKEN) {
                Some(v) => format!("{route} -> {}", v.to_str().unwrap()),
                None => format!("{route} -> NO IDENTITY"),
            }
        })
        .collect()
}

async fn server_answering_every_route() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/connectors"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"connectors": []})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/plan/p1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"plan_id": "p1"})))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/request"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"success": true})))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/plan/p1/cancel"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"plan_id": "p1"})))
        .mount(&server)
        .await;
    server
}

#[tokio::test]
async fn a_derived_client_presents_its_identity_on_every_route() {
    let server = server_answering_every_route().await;
    let derived = client(&server.uri(), None).as_user(TEST_TOKEN);

    let seen = identities_seen_by_every_route(&derived, &server).await;

    let unidentified: Vec<_> = seen.iter().filter(|s| s.ends_with("NO IDENTITY")).collect();
    assert!(
        unidentified.is_empty(),
        "as_user promises a client that acts as one person on EVERY method — that is the whole \
         reason it exists beside the per-call *_as reads, and a gateway deriving one per request \
         has no way to notice the routes that quietly widened back to the process's own \
         authority. Unidentified: {unidentified:?} (all: {seen:?})"
    );
}

#[tokio::test]
async fn a_client_wide_identity_reaches_the_non_read_routes_too() {
    // The config-level half of the same property. Worth its own test rather
    // than a parameter: `as_user` and `AxonFlowConfig::user_token` resolve
    // through the same line today, and a change that special-cased the derived
    // client would leave this one silently unidentified.
    let server = server_answering_every_route().await;
    let configured = client(&server.uri(), Some(TEST_TOKEN));

    let seen = identities_seen_by_every_route(&configured, &server).await;

    for entry in &seen {
        assert!(
            entry.ends_with(TEST_TOKEN),
            "a client-wide identity must ride every request the client makes: {entry} (all: \
             {seen:?})"
        );
    }
}

// ==========================================================================
// The credential goes to the header and nowhere else
// ==========================================================================

#[tokio::test]
async fn the_identity_is_never_sent_off_origin() {
    // The redirect finding, in its Rust form. reqwest strips Authorization,
    // Cookie and Proxy-Authorization on a host change and that list is FIXED —
    // a custom header is not on it. Measured in the sibling SDKs: the redirect
    // target received the tenant credential stripped and the per-user one
    // intact, which is the wrong one to lose.
    //
    // Two guards make that impossible here, and both are exercised: the request
    // is only STAMPED when its origin matches the configured endpoint, and a
    // cross-origin redirect is STOPPED rather than followed.
    let elsewhere = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/decisions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"decisions": []})))
        .mount(&elsewhere)
        .await;

    let origin = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/decisions"))
        .respond_with(
            ResponseTemplate::new(302)
                .insert_header("location", format!("{}/api/v1/decisions", elsewhere.uri())),
        )
        .mount(&origin)
        .await;

    // The redirect is STOPPED, so the call sees the 3xx rather than a request
    // that quietly carried a person's credential to another host.
    let _ = client(&origin.uri(), Some(TEST_TOKEN))
        .list_decisions(ListDecisionsOptions::default())
        .await;

    let hops = elsewhere.received_requests().await.unwrap();
    assert!(
        hops.is_empty(),
        "the cross-origin redirect was followed; it must be STOPPED so that NO credential — not \
         the identity, not the tenant Basic auth, not the client id — can leave the configured \
         origin. Stopping is stronger than the siblings' strip-and-follow: there is no header \
         list to keep in step with, so a credential added later cannot be forgotten. (The \
         TypeScript sibling learned that the hard way: its hand-rolled follower dropped only the \
         new header and leaked the tenant secret off-origin.)"
    );

    let first = origin.received_requests().await.unwrap();
    assert_eq!(
        first[0].headers.get(HEADER_USER_TOKEN).unwrap(),
        TEST_TOKEN,
        "precondition: the identity must have been on the ORIGIN request, or this test asserts \
         nothing"
    );
}

#[tokio::test]
async fn the_identity_survives_a_same_origin_redirect() {
    // The other failure direction: a guard that stops too eagerly turns an
    // ordinary redirect into an unscoped read, which now refuses.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/decisions"))
        .respond_with(
            ResponseTemplate::new(302).insert_header("location", "/api/v1/decisions/page2"),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/decisions/page2"))
        .and(header(HEADER_USER_TOKEN, TEST_TOKEN))
        .respond_with(ResponseTemplate::new(200).set_body_json(row_page()))
        .expect(1)
        .mount(&server)
        .await;

    let _ = client(&server.uri(), Some(TEST_TOKEN))
        .list_decisions(ListDecisionsOptions::default())
        .await;
    // The `.expect(1)` mount asserts the second hop arrived WITH the identity.
}

#[tokio::test]
async fn a_token_that_cannot_be_a_header_value_is_reported_not_dropped() {
    // An EMBEDDED control character: a surrounding one is trimmed on the way in
    // (a trailing newline off a `cat token.txt` is common and harmless), so the
    // case that survives to the header layer is one inside the value.
    //
    // Dropping it was the old behaviour, and it is the worst of the three
    // available outcomes: the read goes out unidentified, the platform answers
    // with a scoped-empty page, and the SDK reports "no identity was presented"
    // — a diagnosis that is true of the wire and false of what the caller did,
    // sending them to look at their minting code's OUTPUT rather than at the
    // newline on the end of it.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/decisions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(row_page()))
        .mount(&server)
        .await;

    let err = client(
        &server.uri(),
        Some(&TEST_TOKEN.replace("SENTINEL", "SENT\rINEL")),
    )
    .list_decisions(ListDecisionsOptions::default())
    .await
    .expect_err("an unsendable identity must not be silently downgraded to an unscoped read");

    let rendered = err.to_string();
    assert!(
        matches!(err, AxonFlowError::ConfigError(_)),
        "the caller's configuration is what is wrong, and naming it as such is what points them \
         at the token rather than at the platform: {rendered}"
    );
    assert!(
        !rendered.contains("SENTINEL-USER-TOKEN"),
        "the token must never appear in an error string — errors are the most-logged text a \
         client produces: {rendered}"
    );
    assert!(
        rendered.contains("control character"),
        "the message must name the CLASS of the offending byte, which is what makes it \
         actionable without echoing the credential: {rendered}"
    );

    assert!(
        server.received_requests().await.unwrap().is_empty(),
        "the request must not have been sent at all. Sending it without the header would be the \
         same silent unidentified read, merely with an error printed afterwards."
    );
}

#[tokio::test]
async fn the_map_transport_also_refuses_an_off_origin_redirect() {
    // The MAP client is a SECOND reqwest::Client with its own builder chain, so
    // it is a second chance to forget the redirect policy — and a mutation run
    // proved it: deleting `.redirect(..)` from the map_http_client builder left
    // every test green, because every redirect test drove the other transport.
    let elsewhere = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/plan/p1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"plan_id": "p1"})))
        .mount(&elsewhere)
        .await;

    let origin = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/plan/p1"))
        .respond_with(
            ResponseTemplate::new(302)
                .insert_header("location", format!("{}/api/v1/plan/p1", elsewhere.uri())),
        )
        .mount(&origin)
        .await;

    let _ = client(&origin.uri(), Some(TEST_TOKEN))
        .get_plan_status("p1")
        .await;

    assert!(
        elsewhere.received_requests().await.unwrap().is_empty(),
        "the MAP transport followed a cross-origin redirect. Both transports carry the same \
         credentials and both must stop at the configured origin; a policy applied to one of two \
         clients is a policy with a hole in it."
    );
    assert_eq!(
        origin.received_requests().await.unwrap()[0]
            .headers
            .get(HEADER_USER_TOKEN)
            .unwrap(),
        TEST_TOKEN,
        "precondition: the MAP request must have carried the identity, or the leak this test \
         guards against could not have happened and the assertion above is vacuous"
    );
}

#[test]
fn the_identity_is_redacted_from_the_config_debug() {
    // A config reaches log lines, panic messages and debugger frames; a
    // credential that rides along has left the process in every one of them.
    // Asserted alongside the other two on purpose: the read-path identity is a
    // credential of the same class, and marking one while forgetting the other
    // is the likely failure.
    let config = AxonFlowConfig {
        endpoint: "http://localhost:8080".into(),
        client_id: Some("org".into()),
        client_secret: Some("SECRET-VALUE".into()),
        user_token: Some("TOKEN-VALUE".into()),
        license_key: Some("LICENSE-VALUE".into()),
        ..AxonFlowConfig::new("http://localhost:8080")
    };
    let rendered = format!("{config:?}");

    assert!(!rendered.contains("SECRET-VALUE"));
    assert!(!rendered.contains("TOKEN-VALUE"));
    assert!(!rendered.contains("LICENSE-VALUE"));
    // ...and it still renders something, or this passes by rendering nothing.
    assert!(rendered.contains("localhost:8080"));
}

#[test]
fn the_header_is_spelled_once_and_stamped_at_one_site() {
    // Deliberately wider than the one spelling the fix uses: a guard is only as
    // wide as the syntax it matches, and there are several ways to write a
    // header onto a reqwest request.
    let mut setters = Vec::new();
    let mut literals = Vec::new();

    fn walk(dir: &Path, setters: &mut Vec<String>, literals: &mut Vec<String>) {
        for entry in fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if path.is_dir() {
                walk(&path, setters, literals);
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let text = fs::read_to_string(&path).unwrap();
            for (index, line) in text.lines().enumerate() {
                let trimmed = line.trim_start();
                // Comments are excluded: the claim is about CODE. The header is
                // named in prose in several doc comments on purpose, and
                // counting those would make the guard fail for being well
                // documented — which teaches the next author to delete the
                // explanation rather than the duplicate.
                if trimmed.starts_with("//") {
                    continue;
                }
                // Case-folded, because a HeaderName literal is conventionally
                // written lower-case (`HeaderName::from_static` REQUIRES it),
                // and a census that only knows the canonical spelling would
                // have read a second stamping site as clean.
                let folded = trimmed.to_ascii_lowercase();
                let names_the_header =
                    trimmed.contains("HEADER_USER_TOKEN") || folded.contains("x-user-token");
                // Every way this crate's dependencies write a header onto a
                // request or a HeaderMap. `append` matters as much as `insert`:
                // it is the one that does NOT replace, so a second stamp added
                // with it would put TWO X-User-Token values on the wire, which
                // is worse than the duplicate this guard exists to catch.
                let writes_a_header = [".header(", ".insert(", ".append(", ".try_insert(", ".try_append("]
                    .iter()
                    .any(|verb| trimmed.contains(verb))
                    // `RequestBuilder::headers(map)` merges a whole HeaderMap.
                    // Spelled with an argument; the read accessor is spelled
                    // `.headers()` with none, and matching the bare prefix
                    // reads every `req.headers().get(..)` assertion as a
                    // stamping site — a marker that collides with the code
                    // beside it, which is how a guard comes to fail on its own
                    // tests.
                    || (trimmed.contains(".headers(") && !trimmed.contains(".headers()"));
                if writes_a_header && names_the_header {
                    setters.push(format!("{}:{}", path.display(), index + 1));
                }
                if folded.contains("\"x-user-token\"") {
                    literals.push(format!("{}:{}", path.display(), index + 1));
                }
            }
        }
    }

    walk(Path::new("src"), &mut setters, &mut literals);

    assert_eq!(
        setters.len(),
        1,
        "X-User-Token must be stamped at exactly one site — the platform reads it once in its \
         proxy middleware, not per route, so a per-method sprinkle here is a second copy of a \
         decision made in one place on both sides. Found: {setters:?}"
    );
    assert_eq!(
        literals.len(),
        1,
        "the literal belongs in the HEADER_USER_TOKEN constant alone, so a rename cannot leave a \
         stale spelling behind. Found: {literals:?}"
    );
}

// ==========================================================================
// The read outcomes
// ==========================================================================

async fn explain_with_scope(status: u16, scope: Option<&str>) -> Result<(), AxonFlowError> {
    let server = MockServer::start().await;
    let mut template = ResponseTemplate::new(status)
        .set_body_json(json!({"error": "Decision not found or past retention window"}));
    if let Some(scope) = scope {
        template = template.insert_header(HEADER_READ_SCOPE, scope);
    }
    Mock::given(method("GET"))
        .and(path("/api/v1/decisions/dec-1/explain"))
        .respond_with(template)
        .mount(&server)
        .await;

    client(&server.uri(), None)
        .explain_decision("dec-1")
        .await
        .map(|_| ())
}

#[tokio::test]
async fn explain_surfaces_the_three_outcomes() {
    // (status, scope, want_typed, want_identity_missing)
    let cases: Vec<(u16, Option<&str>, bool, bool)> = vec![
        (404, Some("none"), true, true),
        (404, Some("own-rows"), true, false),
        (404, Some("tenant"), false, false),
        (404, None, false, false), // a pre-#2922 platform states no scope
        (404, Some("segment-rows"), false, false), // a scope this build does not know
        (500, Some("none"), false, false), // a server fault under a scoped read
    ];

    for (status, scope, want_typed, want_missing) in cases {
        let err = explain_with_scope(status, scope)
            .await
            .expect_err("must fail");
        match &err {
            AxonFlowError::ReadScope(refusal) => {
                assert!(want_typed, "unexpected typed refusal for {scope:?}: {err}");
                assert_eq!(refusal.identity_missing(), want_missing, "scope {scope:?}");
                assert_eq!(refusal.identifier.as_deref(), Some("dec-1"));
                assert_eq!(refusal.resource, "decision");
            }
            _ => assert!(
                !want_typed,
                "expected a typed refusal for scope {scope:?}, got {err}"
            ),
        }
    }
}

#[tokio::test]
async fn list_refuses_an_empty_page_under_scope_none() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/decisions"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"decisions": []}))
                .insert_header(HEADER_READ_SCOPE, "none"),
        )
        .mount(&server)
        .await;

    let err = client(&server.uri(), None)
        .list_decisions(ListDecisionsOptions::default())
        .await
        .expect_err("an empty page under scope none must be refused");

    match err {
        AxonFlowError::ReadScope(refusal) => {
            assert!(refusal.identity_missing());
            // The platform answered successfully; it is the SCOPE that makes
            // the page meaningless.
            assert_eq!(refusal.status, 200);
        }
        other => panic!("want ReadScope, got {other}"),
    }
}

#[tokio::test]
async fn list_does_not_refuse_an_honestly_empty_read() {
    for scope in ["own-rows", "tenant", "segment-rows"] {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/decisions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"decisions": []}))
                    .insert_header(HEADER_READ_SCOPE, scope),
            )
            .mount(&server)
            .await;

        let rows = client(&server.uri(), None)
            .list_decisions(ListDecisionsOptions::default())
            .await
            .unwrap_or_else(|e| panic!("scope {scope} was refused: {e}"));
        assert!(rows.is_empty());
    }
}

#[tokio::test]
async fn list_never_discards_a_populated_page() {
    // Even if a platform contradicts itself and stamps `none` over a populated
    // page, rows that arrived are never thrown away.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/decisions"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(row_page())
                .insert_header(HEADER_READ_SCOPE, "none"),
        )
        .mount(&server)
        .await;

    let rows = client(&server.uri(), None)
        .list_decisions(ListDecisionsOptions::default())
        .await
        .expect("a populated page must never be discarded on the strength of a header");
    assert_eq!(rows.len(), 1);
}

#[tokio::test]
async fn the_scope_is_matched_case_insensitively() {
    // A scope spelled `None` degrading to "no opinion" would restore the
    // vacuous empty list — too quiet a failure to leave to a constant staying
    // put.
    for spelling in ["none", "None", "NONE", " none "] {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/decisions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"decisions": []}))
                    .insert_header(HEADER_READ_SCOPE, spelling),
            )
            .mount(&server)
            .await;

        let err = client(&server.uri(), None)
            .list_decisions(ListDecisionsOptions::default())
            .await
            .expect_err("a scope spelled {spelling} must still be recognised as none");
        assert!(
            matches!(err, AxonFlowError::ReadScope(_)),
            "spelling {spelling}"
        );
    }
}

#[test]
fn absent_is_not_none_and_an_unknown_scope_round_trips() {
    assert_eq!(ReadScope::parse(None), ReadScope::Absent);
    assert_ne!(ReadScope::Absent, ReadScope::None);
    assert_eq!(ReadScope::parse(Some("  TENANT ")), ReadScope::Tenant);
    assert_eq!(
        ReadScope::parse(Some("segment-rows")),
        ReadScope::Other("segment-rows".into())
    );
}

#[test]
fn the_own_rows_message_reports_the_scope_not_a_claim_about_what_exists() {
    use axonflow_sdk_rust::ReadScopeRefusal;

    let not_yours = ReadScopeRefusal {
        resource: "decision".into(),
        identifier: Some("d1".into()),
        scope: ReadScope::OwnRows,
        status: 404,
    };
    assert!(!not_yours.identity_missing());
    let rendered = not_yours.to_string();
    assert!(!rendered.contains("resolved no per-user identity"));
    // It must not assert the row exists and is someone else's — the platform
    // answers "not yours" and "not there" identically, on purpose.
    assert!(rendered.contains("not there at all"));

    let missing = ReadScopeRefusal {
        resource: "decisions".into(),
        identifier: None,
        scope: ReadScope::None,
        status: 200,
    };
    let rendered = missing.to_string();
    assert!(rendered.contains("user_token"));
    assert!(rendered.contains("@axonflow.local"));
}

/// Reads the identity header off a captured request, for the assertions above
/// that need the value rather than a match.
#[allow(dead_code)]
fn identity_of(request: &Request) -> Option<String> {
    request
        .headers
        .get(HEADER_USER_TOKEN)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
}
