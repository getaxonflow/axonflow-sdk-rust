//! Behaviour of the AuthZEN surface, against a stubbed transport.
//!
//! These are unit-level: they pin what the CLIENT does with a given body, which
//! a live stack cannot vary on demand (there is no way to ask a real server for
//! a decision whose boolean and state disagree). The proof that the surface
//! works against the real thing lives in `runtime-e2e/authzen_evaluation/`, and
//! neither replaces the other.

use axonflow_sdk_rust::authzen::{
    Attribute, AttributeMap, AttributeValue, AuthZenAction, AuthZenBulk, AuthZenDecision,
    AuthZenEnvelope, AuthZenError, AuthZenErrorCode, AuthZenEvaluationError,
    AuthZenOperationalState, AuthZenRequest, AuthZenResource, AuthZenResponse, AuthZenSubject,
    AUTHZEN_PATH, AUTHZEN_PROFILE_HEADER, AUTHZEN_PROFILE_V1,
};
use axonflow_sdk_rust::{AxonFlowClient, AxonFlowConfig};
use serde_json::json;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

fn client_for(server: &MockServer) -> AxonFlowClient {
    AxonFlowClient::new(AxonFlowConfig {
        endpoint: server.uri(),
        ..Default::default()
    })
    .expect("the client builds")
}

fn a_request() -> AuthZenRequest {
    AuthZenRequest::evaluating(
        AuthZenSubject::new("gateway", "llm-gateway-01"),
        AuthZenAction::new("llm.completion"),
        AuthZenResource::new("llm", "llm"),
    )
    .with_query(Attribute::known("what is our refund policy?"))
}

fn allow_body() -> serde_json::Value {
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

fn deny_body() -> serde_json::Value {
    json!({
        "decision": false,
        "context": {
            "profile": AUTHZEN_PROFILE_V1,
            "state": "DENY",
            "category": "not_permitted",
            "reason": "explicit_constraint",
            "decision_id": "dec-2",
            "schema_version": "2026-08-29"
        }
    })
}

/// Serves `body` at the AuthZEN path and returns every request it received.
async fn answering(
    status: u16,
    body: serde_json::Value,
) -> (MockServer, std::sync::Arc<std::sync::Mutex<Vec<Request>>>) {
    let server = MockServer::start().await;
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let recorder = seen.clone();
    Mock::given(method("POST"))
        .and(path(AUTHZEN_PATH))
        .respond_with(move |req: &Request| {
            recorder.lock().expect("not poisoned").push(req.clone());
            ResponseTemplate::new(status).set_body_json(body.clone())
        })
        .mount(&server)
        .await;
    (server, seen)
}

fn sent_body(seen: &std::sync::Arc<std::sync::Mutex<Vec<Request>>>) -> serde_json::Value {
    let guard = seen.lock().expect("not poisoned");
    let req = guard.first().expect("a request was sent");
    serde_json::from_slice(&req.body).expect("the body is JSON")
}

// ---------------------------------------------------------------------------
// The three-valued attribute: the whole point of this lane
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_absent_attribute_is_omitted_and_the_request_is_evaluated() {
    // ABSENT is resolved data. The identity provider answered: this caller has
    // no department. A decision made without a fact that has no value is a
    // complete decision, so the request goes.
    let (server, seen) = answering(200, allow_body()).await;
    let mut subject = AuthZenSubject::new("gateway", "llm-gateway-01");
    subject.properties.insert_absent("department");

    let decision = client_for(&server)
        .evaluate(
            AuthZenRequest::evaluating(
                subject,
                AuthZenAction::new("llm.completion"),
                AuthZenResource::new("llm", "llm"),
            )
            .with_query(Attribute::known("hello")),
        )
        .await
        .expect("an absent attribute does not stop the evaluation");

    assert!(decision.allowed());
    let body = sent_body(&seen);
    assert_eq!(
        body["evaluation"]["subject"]["properties"],
        json!({}),
        "an absent member must be omitted from the bag, not sent as null"
    );
}

#[tokio::test]
async fn an_unknown_attribute_refuses_the_request_before_it_is_sent() {
    // UNKNOWN is a failure to resolve. Sending the request without the member
    // would obtain a decision that weighed every attribute except the one
    // nobody could read - and report it as complete.
    let (server, seen) = answering(200, allow_body()).await;
    let mut subject = AuthZenSubject::new("gateway", "llm-gateway-01");
    subject
        .properties
        .insert_unknown("department", "the directory timed out");

    let err = client_for(&server)
        .evaluate(
            AuthZenRequest::evaluating(
                subject,
                AuthZenAction::new("llm.completion"),
                AuthZenResource::new("llm", "llm"),
            )
            .with_query(Attribute::known("hello")),
        )
        .await
        .expect_err("an unresolvable attribute must not be evaluated around");

    let refusal = err.as_refusal().expect("a typed refusal");
    assert_eq!(refusal.code, AuthZenErrorCode::EvaluationUnavailable);
    assert_eq!(
        refusal.pointer.as_deref(),
        Some("/evaluation/subject/properties/department")
    );
    assert!(
        refusal.message.contains("the directory timed out"),
        "the reason the source gave must reach the operator: {}",
        refusal.message
    );
    assert!(
        err.retryable(),
        "a source that could not answer may answer next time"
    );
    assert!(
        seen.lock().expect("not poisoned").is_empty(),
        "the request reached the server despite carrying an unresolvable attribute"
    );
}

#[tokio::test]
async fn absent_and_unknown_are_not_the_same_outcome() {
    // The fixture that fails if the two states are collapsed. With `Option<T>`
    // in place of `Attribute<T>` both of these are `None`, both take the same
    // branch, and exactly one of the two assertions below is wrong whichever
    // way that branch is written.
    let (server, _) = answering(200, allow_body()).await;
    let client = client_for(&server);

    let mut absent = AuthZenSubject::new("gateway", "g1");
    absent.properties.insert_absent("department");
    let absent_outcome = client
        .evaluate(
            AuthZenRequest::evaluating(
                absent,
                AuthZenAction::new("llm.completion"),
                AuthZenResource::new("llm", "llm"),
            )
            .with_query(Attribute::known("hello")),
        )
        .await;

    let mut unknown = AuthZenSubject::new("gateway", "g1");
    unknown.properties.insert_unknown("department", "idp down");
    let unknown_outcome = client
        .evaluate(
            AuthZenRequest::evaluating(
                unknown,
                AuthZenAction::new("llm.completion"),
                AuthZenResource::new("llm", "llm"),
            )
            .with_query(Attribute::known("hello")),
        )
        .await;

    assert!(
        absent_outcome.is_ok(),
        "an absent attribute must produce a decision"
    );

    // The unknown side asserts the STAGE, not merely that something failed.
    //
    // Two guards cover this property - `AttributeMap::validate` refuses it
    // before the round trip, and `AttributeMap`'s `Serialize` refuses to encode
    // it at all - and a test that only asked `is_err()` PASSED with the first
    // one removed, because the second one caught the mutant and produced a
    // different, wronger error. Naming the code, the pointer and the
    // retryability is what distinguishes "the validator refused this" from
    // "the encoder blew up on the way out".
    let err = unknown_outcome.expect_err("an unknown attribute must NOT produce a decision");
    let refusal = err
        .as_refusal()
        .unwrap_or_else(|| panic!("expected a typed refusal, got {err}"));
    assert_eq!(refusal.code, AuthZenErrorCode::EvaluationUnavailable);
    assert_eq!(
        refusal.pointer.as_deref(),
        Some("/evaluation/subject/properties/department")
    );
    assert!(err.retryable());
}

#[tokio::test]
async fn a_known_attribute_reaches_the_wire() {
    // The third state, so the test above cannot pass by refusing everything.
    let (server, seen) = answering(200, allow_body()).await;
    let mut subject = AuthZenSubject::new("gateway", "g1");
    subject.properties.insert_known("department", "finance");

    let _ = client_for(&server)
        .evaluate(
            AuthZenRequest::evaluating(
                subject,
                AuthZenAction::new("llm.completion"),
                AuthZenResource::new("llm", "llm"),
            )
            .with_query(Attribute::known("hello")),
        )
        .await;

    assert_eq!(
        sent_body(&seen)["evaluation"]["subject"]["properties"],
        json!({"department": "finance"})
    );
}

#[tokio::test]
async fn an_unresolvable_nested_leaf_is_named_by_the_leaf_not_the_bag() {
    // `context.correlation.x-session-id` is a leaf two levels down. A refusal
    // pointing at `/evaluation/context/correlation` tells an operator to go
    // looking through an object rather than at a member, which is most of the
    // reason the bag nests at all.
    let (server, _) = answering(200, allow_body()).await;
    let err = client_for(&server)
        .evaluate(a_request().with_correlation(
            "x-session-id",
            Attribute::unknown("the trace header was unreadable"),
        ))
        .await
        .expect_err("refused");

    assert_eq!(
        err.as_refusal().and_then(|r| r.pointer.as_deref()),
        Some("/evaluation/context/correlation/x-session-id")
    );
}

#[test]
fn an_unknown_attribute_has_no_wire_representation_at_all() {
    // The backstop underneath the validator. A future code path that encoded an
    // envelope without validating it first must not be able to drop the
    // unresolved member quietly, so there is no encoding of `Unknown` to fall
    // back to.
    let mut bag = AttributeMap::new();
    bag.insert_unknown("department", "idp down");
    let err = serde_json::to_string(&bag).expect_err("encoding must fail");
    assert!(err.to_string().contains("department"), "{err}");
}

#[test]
fn a_json_object_normalises_into_a_nested_bag_so_equality_survives_a_round_trip() {
    let from_json = AttributeValue::from(json!({"a": {"b": 1}}));
    let mut inner = AttributeMap::new();
    inner.insert_known("b", 1i64);
    let mut outer = AttributeMap::new();
    outer.insert_known("a", inner);
    assert_eq!(from_json, AttributeValue::Nested(outer));

    let encoded = serde_json::to_value(&from_json).expect("encodes");
    let decoded: AttributeValue = serde_json::from_value(encoded).expect("decodes");
    assert_eq!(from_json, decoded);
}

#[test]
fn fold_sees_all_three_states() {
    let known: Attribute<String> = Attribute::known("x");
    let absent: Attribute<String> = Attribute::absent();
    let unknown: Attribute<String> = Attribute::unknown("why");
    let read = |a: &Attribute<String>| a.fold(|_| "known", || "absent", |_| "unknown");
    assert_eq!(read(&known), "known");
    assert_eq!(read(&absent), "absent");
    assert_eq!(read(&unknown), "unknown");
}

// ---------------------------------------------------------------------------
// The wire shape
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_envelope_is_exactly_the_members_the_caller_set() {
    let (server, seen) = answering(200, allow_body()).await;
    let _ = client_for(&server)
        .evaluate(a_request().with_correlation("x-session-id", Attribute::known("sess-1")))
        .await;

    assert_eq!(
        sent_body(&seen),
        json!({
            "evaluation": {
                "subject": {"type": "gateway", "id": "llm-gateway-01"},
                "action": {"name": "llm.completion"},
                "resource": {"type": "llm", "id": "llm"},
                "context": {
                    "args": {"query": "what is our refund policy?"},
                    "correlation": {"x-session-id": "sess-1"}
                }
            }
        }),
        "the envelope carries a member the caller did not set, or is missing one they did"
    );
}

#[tokio::test]
async fn every_request_negotiates_the_profile() {
    // Without the header the server answers with the bare boolean, and this
    // SDK refuses a body with no profile payload - so a dropped header would
    // turn every call into an unusable response rather than a silent
    // downgrade. The header is asserted anyway, because "it fails loudly" is a
    // worse guarantee than "it is sent".
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(AUTHZEN_PATH))
        .and(header(AUTHZEN_PROFILE_HEADER, AUTHZEN_PROFILE_V1))
        .respond_with(ResponseTemplate::new(200).set_body_json(allow_body()))
        .expect(1)
        .mount(&server)
        .await;

    client_for(&server)
        .evaluate(a_request())
        .await
        .expect("allowed");
    // MockServer asserts the `expect(1)` on drop.
}

#[tokio::test]
async fn a_bulk_envelope_yields_one_decision_over_a_shared_base() {
    let (server, seen) = answering(200, deny_body()).await;
    let decision = client_for(&server)
        .evaluate_all(
            AuthZenBulk::over([
                AuthZenRequest {
                    resource: Some(AuthZenResource::new("tool", "jira/move_issue")),
                    ..Default::default()
                },
                AuthZenRequest {
                    resource: Some(AuthZenResource::new("tool", "jira/update_project")),
                    ..Default::default()
                },
            ])
            .with_subject(AuthZenSubject::new("gateway", "g1"))
            .with_action(AuthZenAction::new("tool.call"))
            .with_query(Attribute::known("move the ticket")),
        )
        .await
        .expect("a decision");

    assert!(!decision.allowed());
    assert_eq!(decision.state(), &AuthZenOperationalState::Deny);
    let body = sent_body(&seen);
    assert_eq!(
        body["evaluations"]["evaluations"]
            .as_array()
            .map(|a| a.len()),
        Some(2)
    );
    assert_eq!(body["evaluations"]["action"], json!({"name": "tool.call"}));
}

// ---------------------------------------------------------------------------
// Local validation, in the same vocabulary the server uses
// ---------------------------------------------------------------------------

#[test]
fn an_envelope_carrying_both_members_is_malformed() {
    let envelope = AuthZenEnvelope {
        evaluation: Some(a_request()),
        evaluations: Some(AuthZenBulk::over([a_request()])),
    };
    let err = envelope.validate("").expect_err("refused");
    assert_eq!(err.code, AuthZenErrorCode::MalformedEnvelope);
}

#[test]
fn an_envelope_carrying_neither_member_is_malformed() {
    let err = AuthZenEnvelope {
        evaluation: None,
        evaluations: None,
    }
    .validate("")
    .expect_err("refused");
    assert_eq!(err.code, AuthZenErrorCode::MalformedEnvelope);
}

#[test]
fn a_singular_evaluation_must_carry_its_own_subject_action_and_resource() {
    // It has no shared base to inherit from, and the pointer names which member
    // is missing rather than saying the evaluation is incomplete.
    for (build, pointer) in [
        (
            AuthZenRequest {
                action: Some(AuthZenAction::new("llm.completion")),
                resource: Some(AuthZenResource::new("llm", "llm")),
                ..Default::default()
            },
            "/evaluation/subject",
        ),
        (
            AuthZenRequest {
                subject: Some(AuthZenSubject::new("gateway", "g")),
                resource: Some(AuthZenResource::new("llm", "llm")),
                ..Default::default()
            },
            "/evaluation/action",
        ),
        (
            AuthZenRequest {
                subject: Some(AuthZenSubject::new("gateway", "g")),
                action: Some(AuthZenAction::new("llm.completion")),
                ..Default::default()
            },
            "/evaluation/resource",
        ),
    ] {
        let err = AuthZenEnvelope {
            evaluation: Some(build),
            evaluations: None,
        }
        .validate("")
        .expect_err("refused");
        assert_eq!(err.code, AuthZenErrorCode::IncompleteEvaluation);
        assert_eq!(err.pointer.as_deref(), Some(pointer));
    }
}

#[test]
fn a_subject_with_no_type_is_refused_at_the_member_the_server_would_name() {
    // The wave's sharpest defect, from the client's side: an absent `type` is
    // not the one type the surface evaluates. The server refuses this at
    // `/evaluation/subject/type`, and so does this - the same pointer, so a
    // caller reads one diagnostic whichever side produced it.
    let mut subject = AuthZenSubject::new("gateway", "g1");
    subject.r#type = String::new();
    let err = AuthZenEnvelope {
        evaluation: Some(AuthZenRequest::evaluating(
            subject,
            AuthZenAction::new("llm.completion"),
            AuthZenResource::new("llm", "llm"),
        )),
        evaluations: None,
    }
    .validate("")
    .expect_err("refused");
    assert_eq!(err.pointer.as_deref(), Some("/evaluation/subject/type"));
    assert!(!err.retryable());
}

#[test]
fn a_bulk_envelope_with_no_entries_is_malformed_rather_than_a_request_for_no_decisions() {
    let err = AuthZenEnvelope {
        evaluation: None,
        evaluations: Some(AuthZenBulk::over([])),
    }
    .validate("")
    .expect_err("refused");
    assert_eq!(err.code, AuthZenErrorCode::MalformedEnvelope);
    assert_eq!(err.pointer.as_deref(), Some("/evaluations/evaluations"));
}

// ---------------------------------------------------------------------------
// Reading the answer
// ---------------------------------------------------------------------------

/// Every response case goes through the real client, so nothing here asserts
/// against a hand-built `AuthZenDecision` the transport never produced.
async fn evaluate_against(
    status: u16,
    body: serde_json::Value,
) -> Result<AuthZenDecision, AuthZenEvaluationError> {
    let (server, _) = answering(status, body).await;
    client_for(&server).evaluate(a_request()).await
}

#[tokio::test]
async fn a_200_with_no_profile_payload_is_not_an_allow() {
    // The SDK always negotiates, so an absent context is a BLANKED context: the
    // obligations and the approval challenge that constrain an allow are
    // exactly what is missing. Reading it as "no obligations" is the fail-open.
    let err = evaluate_against(200, json!({"decision": true}))
        .await
        .expect_err("a decision with no readable context must not be acted on");
    assert!(matches!(
        err,
        AuthZenEvaluationError::UnusableResponse { .. }
    ));
    assert!(!err.retryable());
}

#[tokio::test]
async fn a_decision_whose_boolean_and_state_disagree_is_refused_both_ways() {
    let mut allow_but_deny = allow_body();
    allow_but_deny["context"]["state"] = json!("DENY");
    let err = evaluate_against(200, allow_but_deny)
        .await
        .expect_err("refused");
    assert!(matches!(
        err,
        AuthZenEvaluationError::UnusableResponse { .. }
    ));

    let mut deny_but_allow = deny_body();
    deny_but_allow["context"]["state"] = json!("ALLOW");
    let err = evaluate_against(200, deny_but_allow)
        .await
        .expect_err("refused");
    assert!(matches!(
        err,
        AuthZenEvaluationError::UnusableResponse { .. }
    ));
}

#[tokio::test]
async fn an_operational_state_this_build_cannot_read_never_becomes_an_allow() {
    let mut body = allow_body();
    body["context"]["state"] = json!("QUARANTINE");
    let err = evaluate_against(200, body).await.expect_err("refused");
    assert!(matches!(
        err,
        AuthZenEvaluationError::UnusableResponse { .. }
    ));
}

#[tokio::test]
async fn an_unreadable_state_on_a_denial_stays_a_denial() {
    // The other direction, which must NOT be an error: the server collapsed a
    // state this build does not know to `false`, and "not allowed" is a reading
    // this build can act on safely.
    let mut body = deny_body();
    body["context"]["state"] = json!("QUARANTINE");
    let decision = evaluate_against(200, body)
        .await
        .expect("readable as a denial");
    assert!(!decision.allowed());
    assert!(!decision.state().is_known());
}

#[tokio::test]
async fn a_profile_this_build_cannot_interpret_is_refused_and_is_not_retryable() {
    let mut body = allow_body();
    body["context"]["profile"] = json!("axonflow-authzen-profile-2099-01-01");
    let err = evaluate_against(200, body).await.expect_err("refused");
    match &err {
        AuthZenEvaluationError::UnreadableProfile { received, .. } => {
            assert_eq!(received, "axonflow-authzen-profile-2099-01-01")
        }
        other => panic!("expected an unreadable profile, got {other:?}"),
    }
    assert!(
        !err.retryable(),
        "retrying cannot make an older SDK able to read a newer profile"
    );
}

#[tokio::test]
async fn an_unknown_member_in_a_decision_is_refused_rather_than_partly_read() {
    let mut body = allow_body();
    body["context"]["quarantine_until"] = json!("2099-01-01");
    let err = evaluate_against(200, body).await.expect_err("refused");
    assert!(matches!(
        err,
        AuthZenEvaluationError::UnusableResponse { .. }
    ));
}

#[tokio::test]
async fn a_decision_missing_a_required_member_is_refused_by_validation_not_by_decoding() {
    // Decoding establishes the SHAPE. A `decision_id` present but empty decodes
    // happily and is read by a caller as the id to look up.
    let mut body = allow_body();
    body["context"]["decision_id"] = json!("");
    let err = evaluate_against(200, body).await.expect_err("refused");
    match &err {
        AuthZenEvaluationError::UnusableResponse { detail } => {
            assert!(detail.contains("decision_id"), "{detail}")
        }
        other => panic!("expected an unusable response, got {other:?}"),
    }
}

#[tokio::test]
async fn an_allow_surfaces_its_obligations_and_which_of_them_are_mandatory() {
    let mut body = allow_body();
    body["context"]["obligations"] = json!([
        {
            "type": "field_redact",
            "target": "args.query",
            "params": {"fulfillment_endpoint": "/api/v1/mcp/check-input"},
            "mandatory": true,
            "source_policy": "legacy:redact_pii",
            "schema_version": 1
        },
        {
            "type": "notification",
            "mandatory": false,
            "source_policy": "policy:notify",
            "schema_version": 1
        }
    ]);
    let decision = evaluate_against(200, body).await.expect("allowed");
    assert!(decision.allowed());
    assert_eq!(decision.obligations().len(), 2);
    assert_eq!(decision.mandatory_obligations().count(), 1);
    assert_eq!(
        decision
            .mandatory_obligations()
            .next()
            .unwrap()
            .target
            .as_deref(),
        Some("args.query")
    );
}

#[tokio::test]
async fn a_challenge_is_not_an_allow_and_carries_its_approval_requirement() {
    let body = json!({
        "decision": false,
        "context": {
            "profile": AUTHZEN_PROFILE_V1,
            "state": "CHALLENGE",
            "category": "approval_required",
            "reason": "approval_required",
            "decision_id": "dec-3",
            "schema_version": "2026-08-29",
            "approval": {
                "all_of": [{
                    "quorum": 2,
                    "eligible": [{"kind": "group", "type": "team", "local": "risk"}]
                }],
                "separation_of_duties": true,
                "expires_at": "2026-09-02T00:00:00Z"
            }
        }
    });
    let decision = evaluate_against(200, body).await.expect("a decision");
    assert!(!decision.allowed(), "a challenge is not permission");
    assert_eq!(decision.state(), &AuthZenOperationalState::Challenge);
    assert_eq!(decision.approval().map(|a| a.all_of.len()), Some(1));
}

// ---------------------------------------------------------------------------
// Refusals off the wire
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_typed_refusal_reaches_the_caller_with_its_code_and_pointer() {
    let err = evaluate_against(
        422,
        json!({
            "code": "unevaluable_attribute",
            "pointer": "/evaluation/context/department",
            "message": "this surface cannot evaluate the context member \"department\"",
            "supported": ["args", "correlation"]
        }),
    )
    .await
    .expect_err("refused");

    let refusal = err.as_refusal().expect("typed");
    assert_eq!(refusal.code, AuthZenErrorCode::UnevaluableAttribute);
    assert_eq!(
        refusal.pointer.as_deref(),
        Some("/evaluation/context/department")
    );
    assert_eq!(refusal.supported, vec!["args", "correlation"]);
    assert!(!err.retryable());
}

#[tokio::test]
async fn a_dependency_failure_is_the_one_refusal_worth_retrying() {
    let err = evaluate_against(
        502,
        json!({"code": "evaluation_unavailable", "message": "the evaluator did not answer"}),
    )
    .await
    .expect_err("refused");
    assert!(err.retryable());
}

#[tokio::test]
async fn an_authentication_failure_stays_observable_and_never_becomes_a_denial() {
    // A 401 rendered as `decision: false` would be indistinguishable from a
    // policy denial in every caller branch and every dashboard.
    let err = evaluate_against(
        401,
        json!({"error": {"code": 401, "message": "unauthorized"}}),
    )
    .await
    .expect_err("must not be a decision");
    match &err {
        AuthZenEvaluationError::Transport(inner) => {
            assert!(inner.to_string().contains("401"), "{inner}")
        }
        other => panic!("expected a transport error naming the status, got {other:?}"),
    }
    assert!(err.as_refusal().is_none());
}

#[tokio::test]
async fn an_error_body_that_is_not_a_typed_refusal_is_still_never_a_decision() {
    let err = evaluate_against(500, json!("<html>gateway error</html>"))
        .await
        .expect_err("must not be a decision");
    assert!(matches!(err, AuthZenEvaluationError::Transport(_)));
}

// ---------------------------------------------------------------------------
// The refusal enumeration itself
// ---------------------------------------------------------------------------

#[test]
fn exactly_one_refusal_code_is_retryable() {
    // Derived from the artifact's own enumeration rather than from a list
    // written beside it, so a code added to the contract fails this test until
    // somebody decides which side of the line it is on.
    let retryable: Vec<&str> = AuthZenErrorCode::KNOWN_WIRE_VALUES
        .iter()
        .copied()
        .filter(|v| AuthZenErrorCode::from(v.to_string()).retryable())
        .collect();
    assert_eq!(retryable, vec!["evaluation_unavailable"]);
}

#[test]
fn a_refusal_code_this_build_does_not_know_round_trips_and_is_not_retryable() {
    let code = AuthZenErrorCode::from("quarantined".to_string());
    assert!(!code.is_known());
    assert!(!code.retryable());
    assert_eq!(code.as_str(), "quarantined");
    assert_eq!(
        serde_json::to_value(&code).expect("encodes"),
        json!("quarantined")
    );
}

#[test]
fn a_refusal_reads_as_an_error_naming_the_member() {
    let err = AuthZenError::new(AuthZenErrorCode::UnsupportedSubject, "no")
        .at("/evaluation/subject/type");
    assert!(
        err.to_string().contains("/evaluation/subject/type"),
        "{err}"
    );
    let _: &dyn std::error::Error = &err;
}

#[test]
fn a_response_context_that_names_another_profile_does_not_validate() {
    // The generated `const` check, which is what catches a payload whose
    // profile member was rewritten in transit rather than negotiated.
    let mut body = allow_body();
    body["context"]["profile"] = json!("something-else");
    let decoded: AuthZenResponse = serde_json::from_value(body).expect("decodes");
    let err = decoded.validate("").expect_err("refused");
    assert_eq!(err.pointer.as_deref(), Some("/context/profile"));
}
