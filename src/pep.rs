//! Decision Mode PEP (Policy Enforcement Point) contract: **decide → fulfill →
//! forward** (ADR-056, epic #2563).
//!
//!   - decide:  ask the PDP (`POST /api/v1/decide`) for a verdict on a request.
//!   - fulfill: for every obligation the verdict carries, call the ENGINE
//!     endpoint named in the obligation's `fulfillment` block to obtain
//!     engine-redacted content.
//!   - forward: forward the (possibly redacted) content, or block, per verdict.
//!
//! The structural guarantee #2563 demands: a PEP built on this SDK contains NO
//! redaction logic of its own. There is no regex, no pattern table, no masking
//! branch. The ONLY way it discharges a `redact_pii` obligation is by POSTing
//! the source content to the engine endpoint the obligation names
//! ([`AxonFlowClient::fulfill_request`] / [`AxonFlowClient::decide_and_fulfill`])
//! and forwarding what the engine returns. If an obligation arrives without a
//! fulfillable engine endpoint — or the engine reports the redactor did not run —
//! the helper returns [`AxonFlowError::ObligationNotFulfillable`] and the caller
//! MUST fail closed (block), never forward unredacted.
//!
//! This mirrors `platform/shared/pep` (the Go reference PEP) so the SDK PEP
//! cannot reimplement redaction the way a hand-rolled regex would.

use crate::client::AxonFlowClient;
use crate::error::AxonFlowError;
use crate::types::pep::{
    DecideRequest, DecideResponse, MCPCheckInputRequest, MCPCheckInputResponse, Obligation,
};

// --- Obligation contract constants (mirror platform/agent/decision_handler.go) ---

/// The obligation a PEP discharges by replacing request content with
/// engine-redacted content before forwarding.
pub const OBLIGATION_REDACT_PII: &str = "redact_pii";

/// Fulfillment phase: pre-call. `/decide` runs pre-call so it only emits
/// request-phase obligations.
pub const PHASE_REQUEST: &str = "request";
/// Fulfillment phase: post-call. Part of the contract for PEP helpers that fan
/// out to the response-redaction endpoint after the backend call.
pub const PHASE_RESPONSE: &str = "response";

/// The only redaction content-type wired today. The contract is content-type
/// agnostic — a PEP holding content of a type not advertised by an obligation's
/// `content_types` must fail closed rather than forward it unredacted.
pub const CONTENT_TYPE_TEXT: &str = "text/plain";

// --- Verdict values returned by the PDP ---

/// Allow verdict — forward the (possibly redacted) content.
pub const VERDICT_ALLOW: &str = "allow";
/// Deny verdict — block.
pub const VERDICT_DENY: &str = "deny";
/// Needs-approval verdict — route to HITL; do not forward.
pub const VERDICT_NEEDS_APPROVAL: &str = "needs_approval";

// --- Engine endpoints a PEP will POST content to for fulfillment ---

/// The PDP verdict endpoint.
pub const DECIDE_PATH: &str = "/api/v1/decide";
/// The request-phase redaction engine endpoint. An obligation whose fulfillment
/// endpoint is not this (or an absolute URL whose path is this) is rejected — a
/// PEP must not be steered into calling an arbitrary URL by a malformed verdict.
pub const REQUEST_REDACTION_PATH: &str = "/api/v1/mcp/check-input";
/// The response-phase redaction engine endpoint.
pub const RESPONSE_REDACTION_PATH: &str = "/api/v1/mcp/check-output";

/// The synthetic connector tag recorded by the fulfillment endpoint in gateway
/// / PDP mode, where there is no managed connector. It lets the audit trail
/// attribute the redaction to the PEP layer (#2563, connector-agnostic gateway).
pub const GATEWAY_CONNECTOR_TAG: &str = "gateway";

/// Report whether any obligation requires request-phase PII redaction.
///
/// Exposed so a PEP can branch ("does this verdict carry work for me?") before
/// calling [`AxonFlowClient::fulfill_request`].
pub fn has_request_redaction(obligations: &[Obligation]) -> bool {
    obligations.iter().any(|o| {
        o.r#type == OBLIGATION_REDACT_PII
            && o.fulfillment
                .as_ref()
                .is_some_and(|f| f.phase == PHASE_REQUEST)
    })
}

/// Report whether `endpoint` is the expected engine path.
///
/// Tolerates an absolute URL whose path component matches (some PDPs return a
/// fully-qualified obligation endpoint); a blank endpoint never matches.
pub(crate) fn endpoint_path_matches(endpoint: &str, expected: &str) -> bool {
    let e = endpoint.trim();
    if e == expected {
        return true;
    }
    if let Some(idx) = e.find("://") {
        let rest = &e[idx + 3..];
        if let Some(slash) = rest.find('/') {
            let mut path = &rest[slash..];
            if let Some(q) = path.find('?') {
                path = &path[..q];
            }
            return path == expected;
        }
    }
    false
}

impl AxonFlowClient {
    /// Ask the PDP for a verdict on a request (`POST /api/v1/decide`).
    ///
    /// This is the PDP step of a PEP. `/decide` is a pure decision point: it
    /// NEVER mutates content. When an allow verdict carries a `redact_pii`
    /// obligation, discharge it with [`fulfill_request`](Self::fulfill_request)
    /// (or use the one-call [`decide_and_fulfill`](Self::decide_and_fulfill)) —
    /// never by redacting locally.
    ///
    /// Decision Mode auth is HTTP Basic (org:license), which this client already
    /// sends on every request. Demo / wrong credentials are refused with HTTP
    /// 401 → [`AxonFlowError::ApiError`] with `status: 401`. A deny verdict is
    /// returned in the body with HTTP 200, not as an error.
    ///
    /// # Errors
    ///
    /// - [`AxonFlowError::ApiError`] with `status: 401` for bad / demo creds.
    /// - [`AxonFlowError::ApiError`] for other non-2xx responses.
    /// - [`AxonFlowError::HttpError`] for transport failures.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use axonflow_sdk_rust::{AxonFlowClient, AxonFlowConfig};
    /// # use axonflow_sdk_rust::DecideRequest;
    /// # async fn run() -> Result<(), Box<dyn std::error::Error>> {
    /// let client = AxonFlowClient::new(
    ///     AxonFlowConfig::new("http://localhost:8080").with_auth("org", "license"))?;
    /// let decision = client.decide(DecideRequest::new("tool", "send to a@b.com")).await?;
    /// println!("{}", decision.verdict);
    /// # Ok(()) }
    /// ```
    pub async fn decide(&self, request: DecideRequest) -> Result<DecideResponse, AxonFlowError> {
        let url = format!("{}{}", self.endpoint(), DECIDE_PATH);
        // checked_post_json maps any non-2xx (incl. 401) into ApiError, so a
        // demo-cred 401 surfaces as ApiError { status: 401, .. }. A deny verdict
        // is HTTP 200 with verdict="deny" in the body, returned as Ok.
        let resp = self.checked_post_json(&url, &request).await?;
        let body = resp.text().await?;
        let parsed: DecideResponse = serde_json::from_str(&body)?;
        Ok(parsed)
    }

    /// Discharge every request-phase `redact_pii` obligation on `decision`.
    ///
    /// For each request-phase `redact_pii` obligation, POSTs `statement` to the
    /// engine endpoint the obligation names (`check-input`) and returns the
    /// engine-redacted statement to forward.
    ///
    /// There is NO code path in which this method redacts locally — fulfillment
    /// is always the engine round-trip (ADR-056 / #2563).
    ///
    /// Returns `(content, did_redact)`. `content` is the engine-redacted
    /// statement (or the original when no obligation mutates the request).
    /// `did_redact` reflects whether the ENGINE actually changed the content,
    /// not merely that an obligation was present.
    ///
    /// # Errors
    ///
    /// [`AxonFlowError::ObligationNotFulfillable`] when a `redact_pii` obligation
    /// cannot be discharged through the engine: it named no request-phase
    /// fulfillment, advertised a content-type the PEP is not holding, named an
    /// endpoint this client will not call, the engine call failed / returned
    /// non-200, or the engine reported the redactor did not run
    /// (`redaction_evaluated=false`). The caller MUST fail closed (block) —
    /// never forward the original `statement`.
    pub async fn fulfill_request(
        &self,
        decision: &DecideResponse,
        statement: &str,
    ) -> Result<(String, bool), AxonFlowError> {
        let mut redacted = statement.to_string();
        let mut did_redact = false;
        for ob in &decision.obligations {
            if ob.r#type != OBLIGATION_REDACT_PII {
                // redact_pii is the only content-mutating obligation today;
                // other types are pass-through by contract.
                continue;
            }
            let fulfillment = match &ob.fulfillment {
                Some(f) if f.phase == PHASE_REQUEST => f,
                _ => {
                    // A redact_pii obligation with no request-phase fulfillment
                    // cannot be discharged here — fail closed.
                    return Err(AxonFlowError::ObligationNotFulfillable(
                        "redact_pii obligation missing request-phase fulfillment".to_string(),
                    ));
                }
            };
            // Content-type-agnostic check: this client submits text. If the
            // endpoint advertises content types and text is not one of them,
            // fail closed — never assume the endpoint can handle our content.
            if let Some(cts) = &fulfillment.content_types {
                if !cts.is_empty() && !cts.iter().any(|c| c == CONTENT_TYPE_TEXT) {
                    return Err(AxonFlowError::ObligationNotFulfillable(format!(
                        "fulfillment endpoint does not advertise a {CONTENT_TYPE_TEXT} detector"
                    )));
                }
            }
            if !endpoint_path_matches(&fulfillment.endpoint, REQUEST_REDACTION_PATH) {
                return Err(AxonFlowError::ObligationNotFulfillable(format!(
                    "fulfillment endpoint {:?} is not the request-redaction endpoint",
                    fulfillment.endpoint
                )));
            }
            redacted = self.fulfill_via_check_input(&redacted).await?;
            if redacted != statement {
                did_redact = true;
            }
        }
        Ok((redacted, did_redact))
    }

    /// POST `statement` to the request-redaction engine endpoint and return the
    /// engine-masked statement.
    ///
    /// Fails closed ([`AxonFlowError::ObligationNotFulfillable`]) when the engine
    /// call errors, the engine returns non-200, or `redaction_evaluated` is
    /// false — never returns unredacted content under an unfulfillable condition.
    async fn fulfill_via_check_input(&self, statement: &str) -> Result<String, AxonFlowError> {
        let req = MCPCheckInputRequest {
            connector_type: GATEWAY_CONNECTOR_TAG.to_string(),
            statement: statement.to_string(),
            operation: Some("execute".to_string()),
            tenant_id: None,
            content_type: Some(CONTENT_TYPE_TEXT.to_string()),
        };
        let url = format!("{}{}", self.endpoint(), REQUEST_REDACTION_PATH);
        let result: MCPCheckInputResponse = match self.checked_post_json(&url, &req).await {
            Ok(resp) => {
                let body = resp.text().await?;
                serde_json::from_str(&body).map_err(|e| {
                    AxonFlowError::ObligationNotFulfillable(format!(
                        "decode request-redaction engine response: {e}"
                    ))
                })?
            }
            Err(e) => {
                return Err(AxonFlowError::ObligationNotFulfillable(format!(
                    "request-redaction engine call failed: {e}"
                )));
            }
        };
        // FAIL CLOSED if the redactor did not actually run (#2563 B1). Without
        // this the PEP cannot distinguish "engine looked, found nothing" (safe to
        // forward) from "engine wasn't looking" (would leak PII).
        if !result.redaction_evaluated {
            return Err(AxonFlowError::ObligationNotFulfillable(
                "engine reported the redactor did not run (redaction disabled)".to_string(),
            ));
        }
        match (result.redacted, result.redacted_statement) {
            (true, Some(masked)) if !masked.is_empty() => Ok(masked),
            // Redactor ran and found nothing to mask — forward unchanged.
            _ => Ok(statement.to_string()),
        }
    }

    /// One-call PEP path: decide, then fulfill any request-phase obligation.
    ///
    /// Returns `(verdict, content, decision)`. Branch on `verdict`: forward
    /// `content` on `"allow"`; block on `"deny"` / `"needs_approval"`.
    ///
    /// On the not-fulfillable path this returns
    /// [`AxonFlowError::ObligationNotFulfillable`] — a caller that handles the
    /// error cannot accidentally forward the unredacted query, so fail-closed is
    /// guaranteed by construction (#2563 L2). The original query is returned as
    /// `content` only on the non-allow path (where the caller blocks anyway).
    ///
    /// # Errors
    ///
    /// Propagates [`Self::decide`] errors, and
    /// [`AxonFlowError::ObligationNotFulfillable`] from [`Self::fulfill_request`].
    pub async fn decide_and_fulfill(
        &self,
        request: DecideRequest,
    ) -> Result<(String, String, DecideResponse), AxonFlowError> {
        let query = request.query.clone();
        let decision = self.decide(request).await?;
        if decision.verdict != VERDICT_ALLOW {
            return Ok((decision.verdict.clone(), query, decision));
        }
        let (redacted, _) = self.fulfill_request(&decision, &query).await?;
        Ok((decision.verdict.clone(), redacted, decision))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::pep::ObligationFulfillment;
    use crate::{AxonFlowConfig, AxonFlowError};
    use serde_json::json;
    use std::time::Duration;
    use wiremock::matchers::{body_partial_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn make_client(endpoint: String) -> AxonFlowClient {
        let config = AxonFlowConfig {
            endpoint,
            client_id: Some("org-1".into()),
            client_secret: Some("license-1".into()),
            timeout: Duration::from_secs(2),
            ..Default::default()
        };
        AxonFlowClient::new(config).expect("client init")
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

    fn allow_with(obligations: Vec<Obligation>) -> DecideResponse {
        DecideResponse {
            verdict: VERDICT_ALLOW.into(),
            obligations,
            ..Default::default()
        }
    }

    // ---- endpoint_path_matches ----

    #[test]
    fn endpoint_path_matches_exact_and_absolute() {
        assert!(endpoint_path_matches(
            REQUEST_REDACTION_PATH,
            REQUEST_REDACTION_PATH
        ));
        assert!(endpoint_path_matches(
            "  /api/v1/mcp/check-input  ",
            REQUEST_REDACTION_PATH
        ));
        assert!(endpoint_path_matches(
            "https://pdp.internal:8443/api/v1/mcp/check-input",
            REQUEST_REDACTION_PATH
        ));
        assert!(endpoint_path_matches(
            "https://pdp.internal/api/v1/mcp/check-input?x=1",
            REQUEST_REDACTION_PATH
        ));
    }

    #[test]
    fn endpoint_path_matches_rejects_foreign() {
        assert!(!endpoint_path_matches("", REQUEST_REDACTION_PATH));
        assert!(!endpoint_path_matches(
            "/api/v1/mcp/check-output",
            REQUEST_REDACTION_PATH
        ));
        assert!(!endpoint_path_matches(
            "https://evil.example.com/steal",
            REQUEST_REDACTION_PATH
        ));
        // Absolute URL with no path component never matches.
        assert!(!endpoint_path_matches(
            "https://pdp.internal",
            REQUEST_REDACTION_PATH
        ));
    }

    // ---- has_request_redaction ----

    #[test]
    fn has_request_redaction_detects_request_phase() {
        assert!(has_request_redaction(&[redact_obligation()]));
    }

    #[test]
    fn has_request_redaction_ignores_response_phase_and_no_fulfillment() {
        let resp_phase = Obligation {
            r#type: OBLIGATION_REDACT_PII.into(),
            detail: None,
            fulfillment: Some(ObligationFulfillment {
                endpoint: RESPONSE_REDACTION_PATH.into(),
                method: "POST".into(),
                phase: PHASE_RESPONSE.into(),
                content_types: None,
            }),
        };
        let no_fulfillment = Obligation {
            r#type: OBLIGATION_REDACT_PII.into(),
            detail: None,
            fulfillment: None,
        };
        let other_type = Obligation {
            r#type: "log_only".into(),
            detail: None,
            fulfillment: Some(ObligationFulfillment {
                endpoint: REQUEST_REDACTION_PATH.into(),
                method: "POST".into(),
                phase: PHASE_REQUEST.into(),
                content_types: None,
            }),
        };
        assert!(!has_request_redaction(&[
            resp_phase,
            no_fulfillment,
            other_type
        ]));
        assert!(!has_request_redaction(&[]));
    }

    // ---- decide: parse ----

    #[tokio::test]
    async fn decide_parses_allow_with_obligation() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/decide"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "verdict": "allow",
                "decision_id": "dec-1",
                "trace_id": "04110a0b50577bbbdda23a00dcbaf6da",
                "obligations": [{
                    "type": "redact_pii",
                    "fulfillment": {
                        "endpoint": "/api/v1/mcp/check-input",
                        "method": "POST",
                        "phase": "request",
                        "content_types": ["text/plain"],
                    },
                }],
                "evaluated_policies": ["sys_pii_email"],
                "stage": "tool",
                "expires_at": "2026-06-09T05:05:06.8Z",
            })))
            .mount(&server)
            .await;

        let client = make_client(server.uri());
        let d = client
            .decide(DecideRequest::new("tool", "send to a@b.com"))
            .await
            .unwrap();
        assert_eq!(d.verdict, "allow");
        assert_eq!(d.decision_id.as_deref(), Some("dec-1"));
        assert_eq!(d.obligations.len(), 1);
        assert_eq!(d.obligations[0].r#type, "redact_pii");
        assert!(has_request_redaction(&d.obligations));
        assert_eq!(d.evaluated_policies, vec!["sys_pii_email"]);
    }

    #[tokio::test]
    async fn decide_returns_deny_in_body_not_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/decide"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "verdict": "deny",
                "error": "stage is required and must be one of: llm, tool, agent",
            })))
            .mount(&server)
            .await;

        let client = make_client(server.uri());
        let d = client
            .decide(DecideRequest::new("", "x"))
            .await
            .expect("deny is a 200 body, not an error");
        assert_eq!(d.verdict, "deny");
        assert!(d.error.is_some());
        assert!(d.obligations.is_empty());
    }

    #[tokio::test]
    async fn decide_maps_401_to_api_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/decide"))
            .respond_with(ResponseTemplate::new(401).set_body_string("unauthorized"))
            .mount(&server)
            .await;

        let client = make_client(server.uri());
        let err = client
            .decide(DecideRequest::new("tool", "x"))
            .await
            .unwrap_err();
        match err {
            AxonFlowError::ApiError { status, .. } => assert_eq!(status, 401),
            other => panic!("expected ApiError 401, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn decide_sends_basic_auth_and_body() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/decide"))
            .and(body_partial_json(json!({"stage": "tool", "query": "hi"})))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"verdict": "allow"})))
            .expect(1)
            .mount(&server)
            .await;

        let client = make_client(server.uri());
        let d = client
            .decide(DecideRequest::new("tool", "hi"))
            .await
            .unwrap();
        assert_eq!(d.verdict, "allow");
    }

    // ---- fulfill_request: happy path + passthrough ----

    #[tokio::test]
    async fn fulfill_request_returns_engine_masked_content() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/mcp/check-input"))
            .and(body_partial_json(
                json!({"connector_type": "gateway", "content_type": "text/plain"}),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "allowed": true,
                "redacted": true,
                "redacted_statement": "Email jo****om and card 4****1",
                "redaction_evaluated": true,
            })))
            .mount(&server)
            .await;

        let client = make_client(server.uri());
        let (content, did_redact) = client
            .fulfill_request(
                &allow_with(vec![redact_obligation()]),
                "Email john and card 4111",
            )
            .await
            .unwrap();
        assert!(did_redact);
        assert_eq!(content, "Email jo****om and card 4****1");
    }

    #[tokio::test]
    async fn fulfill_request_no_obligation_is_passthrough() {
        // No HTTP mock — must not call the engine at all.
        let client = make_client("http://127.0.0.1:1".into());
        let (content, did_redact) = client
            .fulfill_request(&allow_with(vec![]), "untouched")
            .await
            .unwrap();
        assert!(!did_redact);
        assert_eq!(content, "untouched");
    }

    #[tokio::test]
    async fn fulfill_request_engine_found_nothing_is_passthrough() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/mcp/check-input"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "allowed": true,
                "redacted": false,
                "redaction_evaluated": true,
            })))
            .mount(&server)
            .await;

        let client = make_client(server.uri());
        let (content, did_redact) = client
            .fulfill_request(&allow_with(vec![redact_obligation()]), "no pii here")
            .await
            .unwrap();
        assert!(!did_redact);
        assert_eq!(content, "no pii here");
    }

    // ---- fulfill_request: every fail-closed branch ----

    #[tokio::test]
    async fn fulfill_fails_closed_on_missing_request_phase_fulfillment() {
        let ob = Obligation {
            r#type: OBLIGATION_REDACT_PII.into(),
            detail: None,
            fulfillment: None,
        };
        let client = make_client("http://127.0.0.1:1".into());
        let err = client
            .fulfill_request(&allow_with(vec![ob]), "secret a@b.com")
            .await
            .unwrap_err();
        assert!(matches!(err, AxonFlowError::ObligationNotFulfillable(_)));
    }

    #[tokio::test]
    async fn fulfill_fails_closed_on_response_phase_obligation() {
        let ob = Obligation {
            r#type: OBLIGATION_REDACT_PII.into(),
            detail: None,
            fulfillment: Some(ObligationFulfillment {
                endpoint: REQUEST_REDACTION_PATH.into(),
                method: "POST".into(),
                phase: PHASE_RESPONSE.into(),
                content_types: None,
            }),
        };
        let client = make_client("http://127.0.0.1:1".into());
        let err = client
            .fulfill_request(&allow_with(vec![ob]), "secret a@b.com")
            .await
            .unwrap_err();
        assert!(matches!(err, AxonFlowError::ObligationNotFulfillable(_)));
    }

    #[tokio::test]
    async fn fulfill_fails_closed_on_unadvertised_content_type() {
        let ob = Obligation {
            r#type: OBLIGATION_REDACT_PII.into(),
            detail: None,
            fulfillment: Some(ObligationFulfillment {
                endpoint: REQUEST_REDACTION_PATH.into(),
                method: "POST".into(),
                phase: PHASE_REQUEST.into(),
                content_types: Some(vec!["image/png".into()]),
            }),
        };
        let client = make_client("http://127.0.0.1:1".into());
        let err = client
            .fulfill_request(&allow_with(vec![ob]), "secret a@b.com")
            .await
            .unwrap_err();
        assert!(matches!(err, AxonFlowError::ObligationNotFulfillable(_)));
    }

    #[tokio::test]
    async fn fulfill_fails_closed_on_foreign_endpoint() {
        let ob = Obligation {
            r#type: OBLIGATION_REDACT_PII.into(),
            detail: None,
            fulfillment: Some(ObligationFulfillment {
                endpoint: "https://evil.example.com/steal".into(),
                method: "POST".into(),
                phase: PHASE_REQUEST.into(),
                content_types: Some(vec![CONTENT_TYPE_TEXT.into()]),
            }),
        };
        let client = make_client("http://127.0.0.1:1".into());
        let err = client
            .fulfill_request(&allow_with(vec![ob]), "secret a@b.com")
            .await
            .unwrap_err();
        assert!(matches!(err, AxonFlowError::ObligationNotFulfillable(_)));
    }

    #[tokio::test]
    async fn fulfill_fails_closed_on_engine_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/mcp/check-input"))
            .respond_with(ResponseTemplate::new(500).set_body_string("boom"))
            .mount(&server)
            .await;

        let client = make_client(server.uri());
        let err = client
            .fulfill_request(&allow_with(vec![redact_obligation()]), "secret a@b.com")
            .await
            .unwrap_err();
        assert!(matches!(err, AxonFlowError::ObligationNotFulfillable(_)));
    }

    #[tokio::test]
    async fn fulfill_fails_closed_when_redaction_evaluated_false() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/mcp/check-input"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "allowed": true,
                "redacted": false,
                "redaction_evaluated": false,
            })))
            .mount(&server)
            .await;

        let client = make_client(server.uri());
        let err = client
            .fulfill_request(&allow_with(vec![redact_obligation()]), "secret a@b.com")
            .await
            .unwrap_err();
        assert!(matches!(err, AxonFlowError::ObligationNotFulfillable(_)));
    }

    #[tokio::test]
    async fn fulfill_fails_closed_when_redaction_evaluated_absent() {
        let server = MockServer::start().await;
        // Field absent entirely -> serde default false -> fail closed.
        Mock::given(method("POST"))
            .and(path("/api/v1/mcp/check-input"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "allowed": true,
                "redacted": true,
                "redacted_statement": "Email jo****om",
            })))
            .mount(&server)
            .await;

        let client = make_client(server.uri());
        let err = client
            .fulfill_request(&allow_with(vec![redact_obligation()]), "Email john")
            .await
            .unwrap_err();
        assert!(matches!(err, AxonFlowError::ObligationNotFulfillable(_)));
    }

    #[tokio::test]
    async fn fulfill_ignores_non_redact_obligation_types() {
        // A non-redact obligation is pass-through; no engine call, no error.
        let ob = Obligation {
            r#type: "audit_only".into(),
            detail: None,
            fulfillment: None,
        };
        let client = make_client("http://127.0.0.1:1".into());
        let (content, did_redact) = client
            .fulfill_request(&allow_with(vec![ob]), "left alone")
            .await
            .unwrap();
        assert!(!did_redact);
        assert_eq!(content, "left alone");
    }

    // ---- decide_and_fulfill: allow + deny + unfulfillable ----

    #[tokio::test]
    async fn decide_and_fulfill_allow_redacts() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/decide"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "verdict": "allow",
                "obligations": [{
                    "type": "redact_pii",
                    "fulfillment": {
                        "endpoint": "/api/v1/mcp/check-input",
                        "phase": "request",
                        "content_types": ["text/plain"],
                    },
                }],
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/v1/mcp/check-input"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "allowed": true,
                "redacted": true,
                "redacted_statement": "card 4****1",
                "redaction_evaluated": true,
            })))
            .mount(&server)
            .await;

        let client = make_client(server.uri());
        let (verdict, content, decision) = client
            .decide_and_fulfill(DecideRequest::new("tool", "card 4111111111111111"))
            .await
            .unwrap();
        assert_eq!(verdict, "allow");
        assert_eq!(content, "card 4****1");
        assert_eq!(decision.verdict, "allow");
        assert!(!content.contains("4111111111111111"));
    }

    #[tokio::test]
    async fn decide_and_fulfill_deny_returns_original_without_engine_call() {
        let server = MockServer::start().await;
        // Only the decide mock is mounted — a check-input call would 404/error.
        Mock::given(method("POST"))
            .and(path("/api/v1/decide"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "verdict": "deny",
                "reasons": ["blocked by policy"],
            })))
            .mount(&server)
            .await;

        let client = make_client(server.uri());
        let (verdict, content, _) = client
            .decide_and_fulfill(DecideRequest::new("tool", "original query"))
            .await
            .unwrap();
        assert_eq!(verdict, "deny");
        assert_eq!(content, "original query");
    }

    #[tokio::test]
    async fn decide_and_fulfill_unfulfillable_surfaces_error_not_original() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/decide"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "verdict": "allow",
                "obligations": [{
                    "type": "redact_pii",
                    "fulfillment": {
                        "endpoint": "https://evil.example.com/steal",
                        "phase": "request",
                    },
                }],
            })))
            .mount(&server)
            .await;

        let client = make_client(server.uri());
        let err = client
            .decide_and_fulfill(DecideRequest::new("tool", "leak me a@b.com"))
            .await
            .unwrap_err();
        // The caller gets the fail-closed signal, never the unredacted query.
        assert!(matches!(err, AxonFlowError::ObligationNotFulfillable(_)));
    }
}
