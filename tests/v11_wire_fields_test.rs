//! The v11 decision-provenance fields: what decided a governed request.
//!
//! A v11.0.0 platform reports the engine, the subject type, the policy bundle
//! and any checksum validators that acted on `/api/v1/decide`, on MCP
//! check-output and on `/api/request`; `/decide` also names the policies it
//! matched, the packs that composed into the bundle and the published document
//! version. Every field is absent on an older platform and on a refusal no
//! engine decided, so `None` means "not reported", never "no engine decided".
//! The platform marshals a nil Go slice as JSON `null`, which must read as
//! `None` too.

use axonflow_sdk_rust::{
    AxonFlowClient, AxonFlowConfig, CacheConfig, ClientResponse, DecideResponse,
    LegacyValidatorAction, MCPCheckOutputResponse, PolicyIdentity,
};
use serde_json::json;
use std::collections::HashMap;
use wiremock::matchers::{body_partial_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const BUNDLE: &str = "sha256:0a70b037ed02cd7f65ae1f69742fcc978b5b4b53c199eeb2a3d59bd4d4fdc9e5";

fn validator(validator: &str, action: &str) -> LegacyValidatorAction {
    LegacyValidatorAction {
        validator: validator.into(),
        action: action.into(),
    }
}

#[test]
fn decide_reads_every_v11_field() {
    let resp: DecideResponse = serde_json::from_value(json!({
        "verdict": "allow",
        "decision_id": "dec-1",
        "evaluated_policies": ["grant.refund", "sys.pii.ssn"],
        "obligations": [],
        "engine": "anchored",
        "subject_type": "Client",
        "policy_bundle": BUNDLE,
        "legacy_validators": [{"validator": "india_pii", "action": "masked"}],
        "policy_identities": [
            {"id": "grant.refund", "name": "Refunds", "source": "organization", "version": 3},
            {"id": "sys.pii.ssn", "source": "shipped"}
        ],
        "policy_packs": ["banking@sha256:aa"],
        "document_version": 3
    }))
    .unwrap();

    assert_eq!(resp.engine.as_deref(), Some("anchored"));
    assert_eq!(resp.subject_type.as_deref(), Some("Client"));
    assert_eq!(resp.policy_bundle.as_deref(), Some(BUNDLE));
    assert_eq!(
        resp.legacy_validators,
        Some(vec![validator("india_pii", "masked")])
    );
    assert_eq!(
        resp.policy_identities,
        Some(vec![
            PolicyIdentity {
                id: "grant.refund".into(),
                name: Some("Refunds".into()),
                source: Some("organization".into()),
                version: Some(3),
            },
            PolicyIdentity {
                id: "sys.pii.ssn".into(),
                name: None,
                source: Some("shipped".into()),
                version: None,
            },
        ])
    );
    assert_eq!(
        resp.policy_packs,
        Some(vec!["banking@sha256:aa".to_string()])
    );
    assert_eq!(resp.document_version, Some(3));
}

#[test]
fn decide_reads_null_and_absent_as_not_reported() {
    // A nil Go slice arrives as JSON null; an older platform sends nothing.
    let with_nulls: DecideResponse = serde_json::from_value(json!({
        "verdict": "deny",
        "engine": "anchored",
        "legacy_validators": null,
        "policy_identities": null,
        "policy_packs": null,
        "document_version": null
    }))
    .unwrap();
    assert_eq!(with_nulls.engine.as_deref(), Some("anchored"));
    assert_eq!(with_nulls.legacy_validators, None);
    assert_eq!(with_nulls.policy_identities, None);
    assert_eq!(with_nulls.policy_packs, None);
    assert_eq!(with_nulls.document_version, None);

    let older: DecideResponse = serde_json::from_value(json!({"verdict": "allow"})).unwrap();
    assert_eq!(older.engine, None);
    assert_eq!(older.subject_type, None);
    assert_eq!(older.policy_bundle, None);
}

#[test]
fn a_field_the_platform_did_not_report_is_not_sent_back() {
    let body = serde_json::to_value(DecideResponse {
        verdict: "allow".into(),
        ..Default::default()
    })
    .unwrap();
    for field in [
        "engine",
        "subject_type",
        "policy_bundle",
        "legacy_validators",
        "policy_identities",
        "policy_packs",
        "document_version",
    ] {
        assert!(
            body.get(field).is_none(),
            "{field} was serialized as {body}"
        );
    }
}

#[test]
fn a_legacy_validator_entry_names_both_its_validator_and_its_action() {
    let missing_action =
        serde_json::from_value::<LegacyValidatorAction>(json!({"validator": "india_pii"}));
    assert!(missing_action.is_err());
    let identity: PolicyIdentity = serde_json::from_value(json!({"id": "grant.refund"})).unwrap();
    assert_eq!(identity.id, "grant.refund");
    assert_eq!(
        (identity.name, identity.source, identity.version),
        (None, None, None)
    );
}

#[test]
fn mcp_check_output_reads_the_provenance() {
    let resp: MCPCheckOutputResponse = serde_json::from_value(json!({
        "allowed": true,
        "policies_evaluated": 4,
        "engine": "anchored",
        "subject_type": "Client",
        "policy_bundle": BUNDLE,
        "legacy_validators": [{"validator": "indonesia_pii", "action": "blocked"}]
    }))
    .unwrap();
    assert_eq!(resp.engine.as_deref(), Some("anchored"));
    assert_eq!(resp.subject_type.as_deref(), Some("Client"));
    assert_eq!(resp.policy_bundle.as_deref(), Some(BUNDLE));
    assert_eq!(
        resp.legacy_validators,
        Some(vec![validator("indonesia_pii", "blocked")])
    );
}

#[test]
fn client_response_reads_the_provenance_including_the_tier_engine() {
    // `legacy` is the proxy's tier engine, which authors the verdict of a
    // request the decision plane approved.
    let resp: ClientResponse = serde_json::from_value(json!({
        "success": true,
        "engine": "legacy",
        "subject_type": "User",
        "policy_bundle": BUNDLE,
        "legacy_validators": null
    }))
    .unwrap();
    assert_eq!(resp.engine.as_deref(), Some("legacy"));
    assert_eq!(resp.subject_type.as_deref(), Some("User"));
    assert_eq!(resp.policy_bundle.as_deref(), Some(BUNDLE));
    assert_eq!(resp.legacy_validators, None);
}

#[tokio::test]
async fn query_connector_carries_the_provenance_of_the_request_it_dispatched() {
    // The connector query is dispatched through /api/request, and the SDK
    // builds its ConnectorResponse from that answer by hand: every provenance
    // field must be carried, not dropped.
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/request"))
        .and(body_partial_json(json!({"request_type": "mcp-query"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "success": true,
            "data": {"rows": []},
            "engine": "anchored",
            "subject_type": "Client",
            "policy_bundle": BUNDLE,
            "legacy_validators": [{"validator": "india_pii", "action": "masked"}]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = AxonFlowClient::new(AxonFlowConfig {
        endpoint: server.uri(),
        cache: CacheConfig {
            enabled: false,
            ..Default::default()
        },
        ..Default::default()
    })
    .unwrap();
    let resp = client
        .query_connector("user-123", "postgres", "SELECT 1", HashMap::new())
        .await
        .unwrap();

    assert_eq!(resp.engine.as_deref(), Some("anchored"));
    assert_eq!(resp.subject_type.as_deref(), Some("Client"));
    assert_eq!(resp.policy_bundle.as_deref(), Some(BUNDLE));
    assert_eq!(
        resp.legacy_validators,
        Some(vec![validator("india_pii", "masked")])
    );
}
