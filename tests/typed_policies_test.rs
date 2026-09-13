//! The typed policy authoring client, against a stubbed transport: what each
//! operation sends, what it reads back, and how every documented refusal
//! reaches the caller.
//!
//! These pin the CLIENT. The proof against the platform itself lives in
//! `runtime-e2e/typed_policies/`, and neither replaces the other.

use axonflow_sdk_rust::{
    AuthoringFinding, AxonFlowClient, AxonFlowConfig, AxonFlowError, PEPCapability, PEPHandshake,
    TypedPolicyRefusal, HEADER_USER_TOKEN, PEP_HANDSHAKE_HEADER, TYPED_POLICIES_PATH,
};
use serde_json::{json, Value};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

fn client(server: &MockServer) -> AxonFlowClient {
    AxonFlowClient::new(AxonFlowConfig::new(server.uri())).expect("the client builds")
}

fn route(r: &str) -> String {
    format!("{TYPED_POLICIES_PATH}{r}")
}

/// Answers `status` with `body` on `verb route`.
async fn answer(server: &MockServer, verb: &str, r: &str, template: ResponseTemplate) {
    Mock::given(method(verb))
        .and(path(route(r)))
        .respond_with(template)
        .mount(server)
        .await;
}

async fn sent(server: &MockServer) -> Vec<Request> {
    server
        .received_requests()
        .await
        .expect("request recording is on")
}

async fn sent_body(server: &MockServer) -> Value {
    let requests = sent(server).await;
    let last = requests.last().expect("a request was sent");
    serde_json::from_slice(&last.body).expect("the body is JSON")
}

fn refused(err: AxonFlowError) -> TypedPolicyRefusal {
    match err {
        AxonFlowError::TypedPolicyRefusal(r) => *r,
        other => panic!("expected a typed refusal, got {other:?}"),
    }
}

fn a_document() -> Value {
    json!({"api_version": "axonflow.io/v1", "metadata": {"document_id": "org-baseline"}, "policy": []})
}

fn a_fixture() -> Value {
    json!({"name": "a case", "attributes": {}, "expect": {"decision": "allow"}})
}

// ---------------------------------------------------------------------------
// The operations: method, route, exact body, answer
// ---------------------------------------------------------------------------

#[tokio::test]
async fn edition_reads_the_boundary() {
    let server = MockServer::start().await;
    answer(
        &server,
        "GET",
        "/edition",
        ResponseTemplate::new(200).set_body_json(json!({
            "success": true,
            "catalog": "default",
            "root": "organization",
            "max_documents": -1,
            "constructs": {
                "edition": "community",
                "obligation_families": ["disclosure"],
                "attribute_namespaces": ["args"],
                "group_scope": false,
                "separation_of_duties": false,
                "tier_established": true,
                "reserved": ["team_scope"]
            },
            "persistence": "database",
            "signing_key_custody": "platform"
        })),
    )
    .await;
    let edition = client(&server).typed_policies().edition().await.unwrap();
    assert!(edition.success);
    assert_eq!(edition.root.as_deref(), Some("organization"));
    assert_eq!(edition.max_documents, Some(-1));
    let constructs = edition.constructs.expect("constructs");
    assert_eq!(constructs.edition.as_deref(), Some("community"));
    assert_eq!(constructs.obligation_families, vec!["disclosure"]);
    assert_eq!(constructs.group_scope, Some(false));
    assert_eq!(constructs.tier_established, Some(true));
    assert_eq!(constructs.reserved, vec!["team_scope"]);
    assert_eq!(edition.persistence.as_deref(), Some("database"));
    let requests = sent(&server).await;
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method.as_str(), "GET");
}

#[tokio::test]
async fn validate_sends_the_document_and_fixtures_and_returns_every_finding() {
    let server = MockServer::start().await;
    answer(
        &server,
        "POST",
        "/validate",
        ResponseTemplate::new(200).set_body_json(json!({
            "success": false,
            "findings": [
                {"code": "ACTION_NOT_REGISTERED", "severity": "reject", "policy_id": "grant.refund",
                 "summary": "the action is not registered", "detail": "tool.not_registered"},
                {"code": "UNUSED_ATTRIBUTE", "severity": "warn"}
            ]
        })),
    )
    .await;
    let validation = client(&server)
        .typed_policies()
        .validate(&a_document(), Some(&[a_fixture()]))
        .await
        .unwrap();
    assert!(!validation.success);
    assert_eq!(
        validation.findings,
        vec![
            AuthoringFinding {
                code: "ACTION_NOT_REGISTERED".into(),
                severity: "reject".into(),
                policy_id: Some("grant.refund".into()),
                summary: Some("the action is not registered".into()),
                detail: Some("tool.not_registered".into()),
            },
            AuthoringFinding {
                code: "UNUSED_ATTRIBUTE".into(),
                severity: "warn".into(),
                policy_id: None,
                summary: None,
                detail: None,
            },
        ]
    );
    assert_eq!(
        sent_body(&server).await,
        json!({"document": a_document(), "fixtures": [a_fixture()]})
    );
}

#[tokio::test]
async fn validate_without_fixtures_omits_the_member() {
    let server = MockServer::start().await;
    answer(
        &server,
        "POST",
        "/validate",
        ResponseTemplate::new(200).set_body_json(json!({"success": true, "findings": []})),
    )
    .await;
    client(&server)
        .typed_policies()
        .validate(&a_document(), None)
        .await
        .unwrap();
    let body = sent_body(&server).await;
    assert!(body.get("fixtures").is_none(), "absent is not sent: {body}");
    assert_eq!(body, json!({"document": a_document()}));
}

#[tokio::test]
async fn validate_with_empty_fixtures_sends_an_empty_list() {
    let server = MockServer::start().await;
    answer(
        &server,
        "POST",
        "/validate",
        ResponseTemplate::new(200).set_body_json(json!({"success": true, "findings": []})),
    )
    .await;
    client(&server)
        .typed_policies()
        .validate(&a_document(), Some(&[]))
        .await
        .unwrap();
    assert_eq!(
        sent_body(&server).await,
        json!({"document": a_document(), "fixtures": []})
    );
}

#[tokio::test]
async fn publish_sends_the_request_and_returns_the_digest() {
    let server = MockServer::start().await;
    answer(
        &server,
        "POST",
        "/publish",
        ResponseTemplate::new(200).set_body_json(json!({
            "success": true, "digest": "sha256:abc", "version": 1, "findings": null
        })),
    )
    .await;
    let published = client(&server)
        .typed_policies()
        .publish(&a_document(), Some(&[a_fixture()]))
        .await
        .unwrap();
    assert!(published.success);
    assert_eq!(published.digest, "sha256:abc");
    assert_eq!(published.version, Some(1));
    assert!(published.findings.is_empty());
    let requests = sent(&server).await;
    assert_eq!(requests[0].method.as_str(), "POST");
    assert_eq!(requests[0].url.path(), route("/publish"));
    assert_eq!(
        sent_body(&server).await,
        json!({"document": a_document(), "fixtures": [a_fixture()]})
    );
}

/// The body the platform's own route test proves publishable, vendored
/// byte-identical to the other SDKs', is sent as it is.
#[tokio::test]
async fn publish_sends_the_vendored_fixture_as_it_is() {
    let fixture: Value =
        serde_json::from_str(include_str!("../testdata/typed_policy_publish_body.json"))
            .expect("the vendored fixture");
    let server = MockServer::start().await;
    answer(
        &server,
        "POST",
        "/publish",
        ResponseTemplate::new(200)
            .set_body_json(json!({"success": true, "digest": "d", "version": 1, "findings": []})),
    )
    .await;
    let fixtures = fixture["fixtures"].as_array().expect("fixtures").clone();
    client(&server)
        .typed_policies()
        .publish(&fixture["document"], Some(&fixtures))
        .await
        .unwrap();
    assert_eq!(sent_body(&server).await, fixture);
}

#[tokio::test]
async fn activate_sends_the_digest_and_a_reason() {
    let server = MockServer::start().await;
    answer(
        &server,
        "POST",
        "/activate",
        ResponseTemplate::new(200).set_body_json(json!({
            "success": true, "activation": {"digest": "d", "activated_by": "runtime-e2e"}
        })),
    )
    .await;
    let activation = client(&server)
        .typed_policies()
        .activate("d", Some("quarterly review"))
        .await
        .unwrap();
    assert!(activation.success);
    assert_eq!(activation.activation["activated_by"], "runtime-e2e");
    assert_eq!(
        sent_body(&server).await,
        json!({"digest": "d", "reason": "quarterly review"})
    );
}

/// An absent reason and an empty one are both left out.
#[tokio::test]
async fn activate_omits_an_absent_or_empty_reason() {
    for reason in [None, Some("")] {
        let server = MockServer::start().await;
        answer(
            &server,
            "POST",
            "/activate",
            ResponseTemplate::new(200).set_body_json(json!({"success": true, "activation": {}})),
        )
        .await;
        client(&server)
            .typed_policies()
            .activate("d", reason)
            .await
            .unwrap();
        assert_eq!(
            sent_body(&server).await,
            json!({"digest": "d"}),
            "{reason:?}"
        );
    }
}

/// The document in force keeps the exact bytes that were signed, whitespace
/// and member order included, beside the parsed document.
#[tokio::test]
async fn active_keeps_the_exact_bytes_that_were_signed() {
    let signed =
        br#"{"metadata": {"document_id": "org-baseline"},  "api_version":"axonflow.io/v1"}"#;
    let server = MockServer::start().await;
    answer(
        &server,
        "GET",
        "/active",
        ResponseTemplate::new(200).set_body_raw(signed.to_vec(), "application/json"),
    )
    .await;
    let active = client(&server)
        .typed_policies()
        .active()
        .await
        .unwrap()
        .expect("a document is active");
    assert_eq!(active.source, signed.to_vec());
    assert_eq!(active.document["metadata"]["document_id"], "org-baseline");
}

#[tokio::test]
async fn nothing_active_is_none() {
    let server = MockServer::start().await;
    answer(
        &server,
        "GET",
        "/active",
        ResponseTemplate::new(404).set_body_json(json!({
            "success": false, "reason": "nothing_active", "error": "no document is active"
        })),
    )
    .await;
    assert_eq!(
        client(&server).typed_policies().active().await.unwrap(),
        None
    );
}

#[tokio::test]
async fn active_that_is_not_an_object_is_an_error() {
    let server = MockServer::start().await;
    answer(
        &server,
        "GET",
        "/active",
        ResponseTemplate::new(200).set_body_string("[1, 2]"),
    )
    .await;
    match client(&server).typed_policies().active().await.unwrap_err() {
        AxonFlowError::ApiError { status, message } => {
            assert_eq!(status, 200);
            assert!(message.contains("not an object"), "{message}");
        }
        other => panic!("expected ApiError, got {other:?}"),
    }
}

#[tokio::test]
async fn system_reads_the_shipped_corpus() {
    let server = MockServer::start().await;
    answer(
        &server,
        "GET",
        "/system",
        ResponseTemplate::new(200).set_body_json(json!({
            "success": true,
            "system": {
                "root": "system", "version": 3, "digest": "sha256:sys", "authority": "platform",
                "controls": [{"id": "sys_pii_email", "assurance": "advisory", "mandatory": false,
                              "obligations": [{"type": "field_redact"}]}],
                "assurance_counts": {"advisory": 1},
                "document": {"api_version": "axonflow.io/v1"}
            }
        })),
    )
    .await;
    let system = client(&server).typed_policies().system().await.unwrap();
    assert_eq!(system.root.as_deref(), Some("system"));
    assert_eq!(system.version, Some(3));
    assert_eq!(system.digest.as_deref(), Some("sha256:sys"));
    assert_eq!(system.controls.len(), 1);
    assert_eq!(system.controls[0].id, "sys_pii_email");
    assert_eq!(system.controls[0].mandatory, Some(false));
    assert_eq!(system.assurance_counts.get("advisory"), Some(&1));
    assert_eq!(system.document["api_version"], "axonflow.io/v1");
}

#[tokio::test]
async fn system_without_a_corpus_is_an_empty_one() {
    let server = MockServer::start().await;
    answer(
        &server,
        "GET",
        "/system",
        ResponseTemplate::new(200).set_body_json(json!({"success": true, "system": null})),
    )
    .await;
    let system = client(&server).typed_policies().system().await.unwrap();
    assert!(system.controls.is_empty() && system.document.is_empty());
    assert_eq!(system.digest, None);
}

// ---------------------------------------------------------------------------
// Null collections and absent flags
// ---------------------------------------------------------------------------

/// The platform marshals a nil Go collection as JSON `null`; each reads as
/// empty.
#[tokio::test]
async fn a_nil_go_collection_reads_as_empty() {
    let server = MockServer::start().await;
    answer(
        &server,
        "POST",
        "/validate",
        ResponseTemplate::new(200).set_body_json(json!({"success": true, "findings": null})),
    )
    .await;
    answer(
        &server,
        "GET",
        "/edition",
        ResponseTemplate::new(200).set_body_json(json!({
            "success": true,
            "constructs": {"obligation_families": null, "attribute_namespaces": null, "reserved": null}
        })),
    )
    .await;
    answer(
        &server,
        "POST",
        "/activate",
        ResponseTemplate::new(200).set_body_json(json!({"success": true, "activation": null})),
    )
    .await;
    answer(
        &server,
        "GET",
        "/system",
        ResponseTemplate::new(200).set_body_json(json!({
            "success": true,
            "system": {"controls": null, "assurance_counts": null, "document": null}
        })),
    )
    .await;
    let typed = client(&server);
    let typed = typed.typed_policies();
    assert!(typed
        .validate(&a_document(), None)
        .await
        .unwrap()
        .findings
        .is_empty());
    let constructs = typed
        .edition()
        .await
        .unwrap()
        .constructs
        .expect("constructs");
    assert!(constructs.obligation_families.is_empty());
    assert!(constructs.attribute_namespaces.is_empty());
    assert!(constructs.reserved.is_empty());
    assert!(typed
        .activate("d", None)
        .await
        .unwrap()
        .activation
        .is_empty());
    let system = typed.system().await.unwrap();
    assert!(system.controls.is_empty());
    assert!(system.assurance_counts.is_empty());
    assert!(system.document.is_empty());
}

/// Absent and `false` are different answers: an absent flag stays `None`.
#[tokio::test]
async fn an_absent_flag_is_not_false() {
    let server = MockServer::start().await;
    answer(
        &server,
        "GET",
        "/edition",
        ResponseTemplate::new(200)
            .set_body_json(json!({"success": true, "constructs": {"separation_of_duties": false}})),
    )
    .await;
    let constructs = client(&server)
        .typed_policies()
        .edition()
        .await
        .unwrap()
        .constructs
        .expect("constructs");
    assert_eq!(constructs.separation_of_duties, Some(false));
    assert_eq!(constructs.group_scope, None);
    assert_eq!(constructs.tier_established, None);
}

// ---------------------------------------------------------------------------
// The refusals
// ---------------------------------------------------------------------------

async fn refusal_on_publish(template: ResponseTemplate) -> AxonFlowError {
    let server = MockServer::start().await;
    answer(&server, "POST", "/publish", template).await;
    client(&server)
        .typed_policies()
        .publish(&a_document(), Some(&[a_fixture()]))
        .await
        .unwrap_err()
}

#[tokio::test]
async fn a_400_malformed_request_is_a_typed_refusal() {
    let r = refused(
        refusal_on_publish(ResponseTemplate::new(400).set_body_json(json!({
            "success": false, "reason": "malformed_request", "error": "unexpected end of JSON input"
        })))
        .await,
    );
    assert_eq!(r.status, 400);
    assert_eq!(r.reason.as_deref(), Some("malformed_request"));
    assert_eq!(r.message, "unexpected end of JSON input");
    assert!(r.findings.is_empty());
    assert_eq!(r.retry_after, None);
}

#[tokio::test]
async fn a_402_tier_limit_carries_its_code_and_retry_after() {
    let r = refused(
        refusal_on_publish(
            ResponseTemplate::new(402)
                .insert_header("Retry-After", "3600")
                .set_body_json(json!({
                    "success": false, "reason": "tier_limit", "code": "max_documents",
                    "error": "the edition's document ceiling is reached"
                })),
        )
        .await,
    );
    assert_eq!(r.status, 402);
    assert_eq!(r.reason.as_deref(), Some("tier_limit"));
    assert_eq!(r.code.as_deref(), Some("max_documents"));
    assert_eq!(r.retry_after, Some(3600));
}

#[tokio::test]
async fn a_402_without_retry_after_has_none() {
    let r = refused(
        refusal_on_publish(ResponseTemplate::new(402).set_body_json(json!({
            "success": false, "reason": "tier_limit", "code": "max_documents", "error": "ceiling"
        })))
        .await,
    );
    assert_eq!(r.retry_after, None);
}

/// Only a plain non-negative integer is a `Retry-After` this reads: `+5`
/// parses as a `u64` in Rust, so the digit check is what refuses it.
#[tokio::test]
async fn a_retry_after_that_is_not_seconds_is_none() {
    for value in ["Wed, 21 Oct 2015 07:28:00 GMT", "-1", "+5", "1.5", ""] {
        let r = refused(
            refusal_on_publish(
                ResponseTemplate::new(429)
                    .insert_header("Retry-After", value)
                    .set_body_json(
                        json!({"success": false, "reason": "artifact_cap", "error": "cap"}),
                    ),
            )
            .await,
        );
        assert_eq!(r.retry_after, None, "{value:?}");
    }
}

/// A 404 on any route but `active` is a refusal: the route is missing.
#[tokio::test]
async fn a_404_on_another_route_is_a_typed_refusal() {
    let server = MockServer::start().await;
    answer(
        &server,
        "GET",
        "/edition",
        ResponseTemplate::new(404).set_body_json(json!({
            "success": false, "reason": "no_such_endpoint", "error": "no such typed-policies endpoint"
        })),
    )
    .await;
    let r = refused(
        client(&server)
            .typed_policies()
            .edition()
            .await
            .unwrap_err(),
    );
    assert_eq!(r.status, 404);
    assert_eq!(r.reason.as_deref(), Some("no_such_endpoint"));
}

#[tokio::test]
async fn a_409_activation_refused_is_a_typed_refusal() {
    let server = MockServer::start().await;
    answer(
        &server,
        "POST",
        "/activate",
        ResponseTemplate::new(409).set_body_json(json!({
            "success": false, "reason": "activation_refused",
            "error": "version 1 does not advance past the active version 1"
        })),
    )
    .await;
    let r = refused(
        client(&server)
            .typed_policies()
            .activate("d", None)
            .await
            .unwrap_err(),
    );
    assert_eq!(r.status, 409);
    assert_eq!(r.reason.as_deref(), Some("activation_refused"));
    assert!(r.message.contains("does not advance"), "{}", r.message);
}

/// A publication refused for want of fixtures names the cause in its message
/// and carries no findings: only the save-time checks carry findings.
#[tokio::test]
async fn a_422_publication_refused_names_its_cause_in_the_message() {
    let r = refused(
        refusal_on_publish(ResponseTemplate::new(422).set_body_json(json!({
            "success": false, "reason": "publication_refused",
            "error": "the document declares no fixtures", "findings": []
        })))
        .await,
    );
    assert_eq!(r.status, 422);
    assert_eq!(r.reason.as_deref(), Some("publication_refused"));
    assert!(r.message.contains("declares no fixtures"), "{}", r.message);
    assert!(r.findings.is_empty());
}

#[tokio::test]
async fn a_422_document_refused_carries_its_findings() {
    let r = refused(
        refusal_on_publish(ResponseTemplate::new(422).set_body_json(json!({
            "success": false, "reason": "document_refused", "error": "the document is refused",
            "findings": [{"code": "ACTION_NOT_REGISTERED", "severity": "reject", "policy_id": "grant.refund"}]
        })))
        .await,
    );
    assert_eq!(r.reason.as_deref(), Some("document_refused"));
    assert_eq!(r.findings.len(), 1);
    assert_eq!(r.findings[0].code, "ACTION_NOT_REGISTERED");
    assert_eq!(r.findings[0].severity, "reject");
    assert_eq!(r.findings[0].policy_id.as_deref(), Some("grant.refund"));
}

#[tokio::test]
async fn a_429_artifact_cap_is_a_retryable_typed_refusal() {
    let err = refusal_on_publish(
        ResponseTemplate::new(429)
            .insert_header("Retry-After", "60")
            .set_body_json(
                json!({"success": false, "reason": "artifact_cap", "error": "cap reached"}),
            ),
    )
    .await;
    assert!(err.is_retryable());
    let r = refused(err);
    assert_eq!(r.status, 429);
    assert_eq!(r.reason.as_deref(), Some("artifact_cap"));
    assert_eq!(r.retry_after, Some(60));
}

#[tokio::test]
async fn a_503_storage_unavailable_is_a_retryable_typed_refusal() {
    let err = refusal_on_publish(ResponseTemplate::new(503).set_body_json(json!({
        "success": false, "reason": "storage_unavailable", "error": "the artifact store is down"
    })))
    .await;
    assert!(err.is_retryable());
    assert_eq!(refused(err).reason.as_deref(), Some("storage_unavailable"));
}

#[tokio::test]
async fn a_4xx_refusal_is_not_retryable() {
    let err = refusal_on_publish(ResponseTemplate::new(422).set_body_json(json!({
        "success": false, "reason": "publication_refused", "error": "no fixtures"
    })))
    .await;
    assert!(!err.is_retryable());
}

/// A 401 keeps the client's authentication error, observably: it is not
/// folded into a refusal.
#[tokio::test]
async fn a_401_is_the_clients_authentication_error() {
    let err = refusal_on_publish(ResponseTemplate::new(401).set_body_json(json!({
        "error": "invalid credentials"
    })))
    .await;
    match err {
        AxonFlowError::ApiError { status, message } => {
            assert_eq!(status, 401);
            assert_eq!(message, "invalid credentials");
        }
        other => panic!("expected ApiError 401, got {other:?}"),
    }
}

#[tokio::test]
async fn a_refusal_without_a_json_body_still_names_its_status() {
    let r = refused(
        refusal_on_publish(ResponseTemplate::new(502).set_body_string("<html>bad gateway</html>"))
            .await,
    );
    assert_eq!(r.status, 502);
    assert_eq!(r.reason, None);
    assert_eq!(r.message, "HTTP 502 from /publish");
}

/// A member of an unexpected type leaves the others read.
#[tokio::test]
async fn a_member_of_the_wrong_type_leaves_the_others_read() {
    let r = refused(
        refusal_on_publish(ResponseTemplate::new(422).set_body_json(json!({
            "reason": 5, "code": "c", "error": "the message", "findings": "not a list"
        })))
        .await,
    );
    assert_eq!(r.reason, None);
    assert_eq!(r.code.as_deref(), Some("c"));
    assert_eq!(r.message, "the message");
    assert!(r.findings.is_empty());
}

#[tokio::test]
async fn a_success_whose_body_is_not_an_object_is_an_error() {
    for body in ["[]", "not json", "\"a string\""] {
        let server = MockServer::start().await;
        answer(
            &server,
            "GET",
            "/edition",
            ResponseTemplate::new(200).set_body_string(body),
        )
        .await;
        match client(&server)
            .typed_policies()
            .edition()
            .await
            .unwrap_err()
        {
            AxonFlowError::ApiError { status, message } => {
                assert_eq!(status, 200, "{body}");
                assert!(message.contains("not an object"), "{body}: {message}");
            }
            other => panic!("{body}: expected ApiError, got {other:?}"),
        }
    }
}

#[test]
fn the_refusal_renders_its_status_reason_and_message() {
    let r = TypedPolicyRefusal {
        status: 409,
        reason: Some("activation_refused".into()),
        code: None,
        message: "version does not advance".into(),
        findings: Vec::new(),
        retry_after: None,
    };
    assert_eq!(
        r.to_string(),
        "typed policy request refused (HTTP 409, activation_refused): version does not advance"
    );
    let bare = TypedPolicyRefusal {
        reason: None,
        ..r.clone()
    };
    assert_eq!(
        bare.to_string(),
        "typed policy request refused (HTTP 409): version does not advance"
    );
    assert_eq!(
        AxonFlowError::TypedPolicyRefusal(Box::new(r.clone())).to_string(),
        r.to_string()
    );
}

// ---------------------------------------------------------------------------
// Whose request: no handshake, and the derived client's identity
// ---------------------------------------------------------------------------

/// These routes do not read the PEP capability declaration, so a declaring
/// client sends it on none of the six.
#[tokio::test]
async fn the_declaration_is_sent_on_none_of_the_six_routes() {
    let server = MockServer::start().await;
    for (verb, r, body) in [
        ("GET", "/edition", json!({"success": true})),
        (
            "POST",
            "/validate",
            json!({"success": true, "findings": []}),
        ),
        (
            "POST",
            "/publish",
            json!({"success": true, "digest": "d", "findings": []}),
        ),
        (
            "POST",
            "/activate",
            json!({"success": true, "activation": {}}),
        ),
        ("GET", "/active", json!({"api_version": "axonflow.io/v1"})),
        ("GET", "/system", json!({"success": true, "system": {}})),
    ] {
        answer(
            &server,
            verb,
            r,
            ResponseTemplate::new(200).set_body_json(body),
        )
        .await;
    }
    let declared = PEPHandshake::new(
        "sdk-rust-test",
        "https://pep.example.test",
        [PEPCapability::new("field_redact", 1)],
    )
    .unwrap();
    let c = AxonFlowClient::new(AxonFlowConfig::new(server.uri()).with_pep_handshake(declared))
        .unwrap();
    let typed = c.typed_policies();
    typed.edition().await.unwrap();
    typed.validate(&a_document(), None).await.unwrap();
    typed.publish(&a_document(), Some(&[])).await.unwrap();
    typed.activate("d", None).await.unwrap();
    typed.active().await.unwrap();
    typed.system().await.unwrap();

    let requests = sent(&server).await;
    assert_eq!(requests.len(), 6, "positive control: all six were sent");
    for r in &requests {
        assert!(
            r.headers.get(PEP_HANDSHAKE_HEADER).is_none(),
            "{} {} carried the declaration",
            r.method,
            r.url.path()
        );
    }
}

/// The namespace borrows the client it came from, so a client derived for a
/// person presents that person.
#[tokio::test]
async fn a_derived_clients_namespace_presents_its_own_identity() {
    let server = MockServer::start().await;
    answer(
        &server,
        "GET",
        "/edition",
        ResponseTemplate::new(200).set_body_json(json!({"success": true})),
    )
    .await;
    let base = client(&server);
    base.as_user("alice-token")
        .typed_policies()
        .edition()
        .await
        .unwrap();
    base.typed_policies().edition().await.unwrap();
    let requests = sent(&server).await;
    assert_eq!(
        requests[0]
            .headers
            .get(HEADER_USER_TOKEN)
            .and_then(|v| v.to_str().ok()),
        Some("alice-token")
    );
    assert!(requests[1].headers.get(HEADER_USER_TOKEN).is_none());
}
