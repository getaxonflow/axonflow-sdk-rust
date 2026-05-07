// Decision explainability methods for the AxonFlow Rust SDK.
//
// Implements the ADR-043 contract:
//   GET /api/v1/decisions/:id/explain
//
// Returns a [`DecisionExplanation`] including the matched policies,
// risk level, override availability, and historical hit count.
//
// Cross-SDK parity:
//   Go:     axonflow-sdk-go/decisions.go (ExplainDecision)
//   Python: axonflow-sdk-python/axonflow/client.py (explain_decision)
//   TS:     axonflow-sdk-typescript/src/client.ts (explainDecision)
//   Java:   axonflow-sdk-java/src/main/java/com/getaxonflow/sdk/AxonFlow.java (explainDecision)

use crate::client::{AxonFlowClient, PATH_SEGMENT};
use crate::error::AxonFlowError;
use crate::types::decisions::DecisionExplanation;
use percent_encoding::utf8_percent_encode;

impl AxonFlowClient {
    /// Fetches the full explanation for a previously-made policy decision.
    ///
    /// The caller must either own the decision (X-User-Email match) or
    /// belong to the same tenant as the decision (X-Tenant-ID match).
    /// Returns an error wrapping HTTP 404 when the decision is past the
    /// tier's audit retention window.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use axonflow_sdk_rust::{AxonFlowClient, AxonFlowConfig};
    /// # async fn run() -> Result<(), Box<dyn std::error::Error>> {
    /// let client = AxonFlowClient::new(AxonFlowConfig::new("http://localhost:8080"))?;
    /// let exp = client.explain_decision("dec_wf123_step4").await?;
    /// if exp.override_available {
    ///     // Surface a "request override" UI affordance
    /// }
    /// # Ok(()) }
    /// ```
    pub async fn explain_decision(
        &self,
        decision_id: &str,
    ) -> Result<DecisionExplanation, AxonFlowError> {
        if decision_id.is_empty() {
            return Err(AxonFlowError::ConfigError(
                "decision_id is required".to_string(),
            ));
        }

        // Path-escape — platform-generated decision IDs are usually
        // filesystem-safe, but ADR-043 does not guarantee it. Decision
        // IDs containing '/' or '?' would otherwise corrupt the URL.
        let encoded = utf8_percent_encode(decision_id, PATH_SEGMENT).to_string();
        let url = format!("{}/api/v1/decisions/{}/explain", self.endpoint(), encoded);

        let resp = self.checked_get(&url).await?;
        let body = resp.text().await?;
        let parsed: DecisionExplanation = serde_json::from_str(&body)?;
        Ok(parsed)
    }
}

#[cfg(test)]
mod tests {
    use crate::types::decisions::DecisionExplanation;
    use crate::{AxonFlowClient, AxonFlowConfig};
    use chrono::{TimeZone, Utc};
    use httpmock::prelude::*;
    use serde_json::json;
    use std::time::Duration;

    fn make_client(endpoint: String) -> AxonFlowClient {
        let config = AxonFlowConfig {
            endpoint,
            timeout: Duration::from_secs(2),
            ..Default::default()
        };
        AxonFlowClient::new(config).expect("client init")
    }

    #[tokio::test]
    async fn empty_decision_id_returns_config_error() {
        // No HTTP server needed — guard fires before any wire call.
        let client = make_client("http://127.0.0.1:1".into());
        let err = client.explain_decision("").await.unwrap_err();
        assert!(
            err.to_string().contains("decision_id is required"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn happy_path_parses_full_payload() {
        let server = MockServer::start();
        let want = json!({
            "decision_id": "dec_wf1_step2",
            "timestamp": "2026-04-17T12:00:00Z",
            "decision": "deny",
            "reason": "SQL injection detected",
            "risk_level": "high",
            "policy_matches": [{
                "policy_id": "pol-sqli",
                "policy_name": "SQL Injection Detector",
                "action": "deny",
                "risk_level": "high",
                "allow_override": true
            }],
            "override_available": true,
            "historical_hit_count_session": 3
        });

        let mock = server.mock(|when, then| {
            when.method(GET)
                .path("/api/v1/decisions/dec_wf1_step2/explain");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(want);
        });

        let client = make_client(server.url(""));
        let got = client.explain_decision("dec_wf1_step2").await.unwrap();

        mock.assert();
        assert_eq!(got.decision_id, "dec_wf1_step2");
        assert_eq!(got.decision, "deny");
        assert_eq!(got.reason, "SQL injection detected");
        assert_eq!(got.risk_level.as_deref(), Some("high"));
        assert_eq!(got.policy_matches.len(), 1);
        assert_eq!(got.policy_matches[0].policy_id, "pol-sqli");
        assert!(got.policy_matches[0].allow_override);
        assert!(got.override_available);
        assert_eq!(got.historical_hit_count_session, 3);
        assert_eq!(
            got.timestamp,
            Utc.with_ymd_and_hms(2026, 4, 17, 12, 0, 0).unwrap()
        );
    }

    #[tokio::test]
    async fn decision_id_is_url_encoded() {
        // Decision IDs containing '/' must be percent-encoded so they don't
        // corrupt the path. Ensures parity with axonflow-sdk-go's PathEscape
        // contract test (decisions_test.go::TestExplainDecision_URLEncodesDecisionID).
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(GET).path("/api/v1/decisions/a%2Fb/explain");
            then.status(200).json_body(json!({
                "decision_id": "a/b",
                "timestamp": "2026-04-17T12:00:00Z",
                "decision": "allow",
                "reason": "",
                "policy_matches": []
            }));
        });

        let client = make_client(server.url(""));
        client.explain_decision("a/b").await.unwrap();
        mock.assert();
    }

    #[tokio::test]
    async fn http_404_surfaces_as_api_error() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET)
                .path("/api/v1/decisions/dec-missing/explain");
            then.status(404)
                .json_body(json!({"error": "Decision not found or past retention window"}));
        });

        let client = make_client(server.url(""));
        let err = client.explain_decision("dec-missing").await.unwrap_err();
        match err {
            crate::error::AxonFlowError::ApiError { status, .. } => assert_eq!(status, 404),
            other => panic!("expected ApiError(404), got: {other}"),
        }
    }

    #[tokio::test]
    async fn http_401_surfaces_as_api_error() {
        // explainDecisionHandler returns 401 when X-Tenant-ID is missing
        // (platform/orchestrator/explain_handler.go:80). Caller-side rendering
        // should distinguish "not authorized" from "not found" — covered by
        // the ApiError status.
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/api/v1/decisions/dec-x/explain");
            then.status(401)
                .json_body(json!({"error": "X-Tenant-ID header is required"}));
        });

        let client = make_client(server.url(""));
        let err = client.explain_decision("dec-x").await.unwrap_err();
        match err {
            crate::error::AxonFlowError::ApiError { status, .. } => assert_eq!(status, 401),
            other => panic!("expected ApiError(401), got: {other}"),
        }
    }

    #[tokio::test]
    async fn malformed_json_response_is_serde_error() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/api/v1/decisions/dec-x/explain");
            then.status(200)
                .header("content-type", "application/json")
                .body("{not valid json");
        });

        let client = make_client(server.url(""));
        let err = client.explain_decision("dec-x").await.unwrap_err();
        match err {
            crate::error::AxonFlowError::SerdeError(_) => {}
            other => panic!("expected SerdeError, got: {other}"),
        }
    }

    #[tokio::test]
    async fn additive_unknown_fields_are_ignored() {
        // Forward-compat: ADR-043 §"Versioning" allows additive fields on
        // future platform versions. The Rust SDK must NOT fail when the
        // platform returns a field the SDK doesn't know about yet — this is
        // the failure mode that breaks customers when the platform is ahead
        // of the SDK. (Default serde_json behavior is to ignore unknown
        // fields; this test pins that contract so it cannot regress via a
        // future #[serde(deny_unknown_fields)] addition.)
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/api/v1/decisions/dec-x/explain");
            then.status(200).json_body(json!({
                "decision_id": "dec-x",
                "timestamp": "2026-04-17T12:00:00Z",
                "decision": "allow",
                "reason": "",
                "policy_matches": [],
                "policy_version_at_decision": "v3",      // future-additive (V1.1)
                "latest_policy_version": "v5",            // future-additive (V1.1)
                "yet_another_future_field": "shrug"      // arbitrary forward-compat
            }));
        });

        let client = make_client(server.url(""));
        let got: DecisionExplanation = client.explain_decision("dec-x").await.unwrap();
        assert_eq!(got.decision_id, "dec-x");
    }
}
