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

use crate::client::AxonFlowClient;
use crate::error::AxonFlowError;
use crate::types::decisions::{
    DecisionExplanation, DecisionSummary, ListDecisionsOptions, RateLimitEnvelope,
};
use crate::PATH_SEGMENT;
use percent_encoding::utf8_percent_encode;
use serde::Deserialize;
use tracing;

impl AxonFlowClient {
    /// Fetches the full explanation for a previously-made policy decision.
    ///
    /// # Which decisions this returns (platform #2922)
    ///
    /// The read is scoped to the per-user identity the caller presents, NOT to
    /// the tenant credential. On an enterprise stack:
    ///
    /// - a tenant-wide role (admin, owner, policy_admin) explains any decision
    ///   in the tenant;
    /// - any other identity (developer, viewer) explains only the decisions
    ///   attributed to it — another user's decision answers exactly like a
    ///   decision that does not exist;
    /// - a caller presenting NO identity explains nothing at all. Every call
    ///   answers not-found, whatever the decision id.
    ///
    /// Community and Community-SaaS deployments are single-operator and read
    /// tenant-wide with no identity needed.
    ///
    /// Present the identity with
    /// [`AxonFlowConfig::user_token`](crate::AxonFlowConfig::user_token),
    /// [`explain_decision_as`](Self::explain_decision_as) for one call, or
    /// [`as_user`](crate::AxonFlowClient::as_user) for a client bound to one
    /// person.
    ///
    /// # Telling the misses apart, as far as the platform allows
    ///
    /// A miss returns [`AxonFlowError::ReadScope`] whenever the platform's
    /// `X-Axonflow-Read-Scope` header says the caller's scope decided it, so
    /// "the platform resolved no identity for you" stops being indistinguishable
    /// from "past retention". It does NOT separate "not yours" from "not there":
    /// under own-rows the platform answers both with the same 404 on purpose, so
    /// that a miss cannot be used to probe for another user's rows.
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
    #[tracing::instrument(skip(self))]
    pub async fn explain_decision(
        &self,
        decision_id: &str,
    ) -> Result<DecisionExplanation, AxonFlowError> {
        self.explain_decision_as(decision_id, None).await
    }

    /// Fetches a decision explanation as a specific per-user identity.
    ///
    /// Overrides the client-wide `user_token` for THIS call only. Use it when
    /// one process acts on behalf of several people. `Some("")` is not an
    /// identity: it makes this read explicitly unidentified rather than falling
    /// back to the client-wide one — a distinction that has to exist, because
    /// "unidentified" is a state the platform treats as different from every
    /// other.
    ///
    /// For a process acting for several people across MANY methods, prefer
    /// [`as_user`](crate::AxonFlowClient::as_user): this method exists only on
    /// the reads, while a derived client reaches every method with no carve-out.
    #[tracing::instrument(skip(self, user_token))]
    pub async fn explain_decision_as(
        &self,
        decision_id: &str,
        user_token: Option<&str>,
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

        let resp = self.raw_get_as(&url, user_token).await?;

        // A scoped miss reports WHY it missed. Only 404 is interpreted: the
        // scope header is stamped before the handler writes its status, so it
        // also rides a 500 from further down the handler, and explaining a
        // server fault as a scoping outcome would be exactly the
        // confidently-wrong diagnosis this type exists to prevent.
        if resp.status().as_u16() == 404 {
            if let Some(refusal) = crate::read_identity::read_scope_refusal(
                "decision",
                Some(decision_id),
                crate::read_identity::ReadScope::of(&resp),
                404,
            ) {
                return Err(AxonFlowError::ReadScope(Box::new(refusal)));
            }
        }
        let resp = AxonFlowClient::check_status(resp).await?;
        let body = resp.text().await?;
        let parsed: DecisionExplanation = serde_json::from_str(&body)?;
        Ok(parsed)
    }

    /// Lists the recent policy decisions VISIBLE TO THE CALLER.
    ///
    /// # Whose decisions come back (platform #2922)
    ///
    /// Not the tenant's — the caller's SCOPE. On an enterprise stack a
    /// tenant-wide role lists the whole tenant, any other identity lists only
    /// its own rows, and a caller presenting NO identity lists nothing
    /// whatsoever. That last case used to return an empty `Vec`, which reads as
    /// "your tenant has made no decisions" and is a different statement from
    /// what happened; it now returns [`AxonFlowError::ReadScope`] instead. A
    /// genuinely empty own-rows or tenant-wide read is NOT an error.
    ///
    /// Returns the slim 5-field [`DecisionSummary`] page; the platform
    /// applies a tier-gated cap (5/24h on Free + Community, 100/30d on
    /// Pro + Evaluation, 1000/full on Enterprise). Requesting a `limit`
    /// above the tier cap yields a 429 with the V1 upgrade envelope —
    /// surfaced here as [`AxonFlowError::RateLimited`] so callers can
    /// branch on `envelope.upgrade.{tier,compare_url,buy_url}` without
    /// re-parsing the body.
    ///
    /// Filters compose: passing `decision = Some("blocked")` AND
    /// `policy_id = Some("pol-sqli")` returns only blocked decisions
    /// matching that policy. `since` is RFC3339 (chrono `DateTime<Utc>`).
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use axonflow_sdk_rust::{AxonFlowClient, AxonFlowConfig, ListDecisionsOptions};
    /// # async fn run() -> Result<(), Box<dyn std::error::Error>> {
    /// let client = AxonFlowClient::new(AxonFlowConfig::new("http://localhost:8080"))?;
    /// let opts = ListDecisionsOptions {
    ///     decision: Some("blocked".into()),
    ///     limit: Some(10),
    ///     ..Default::default()
    /// };
    /// let decisions = client.list_decisions(opts).await?;
    /// for d in decisions {
    ///     println!("{} {} {}", d.decision_id, d.decision, d.timestamp);
    /// }
    /// # Ok(()) }
    /// ```
    pub async fn list_decisions(
        &self,
        opts: ListDecisionsOptions,
    ) -> Result<Vec<DecisionSummary>, AxonFlowError> {
        self.list_decisions_as(opts, None).await
    }

    /// Lists the decisions visible to a specific per-user identity.
    ///
    /// Overrides the client-wide `user_token` for THIS call only. See
    /// [`explain_decision_as`](Self::explain_decision_as) for the semantics, and
    /// [`as_user`](crate::AxonFlowClient::as_user) for the shape to prefer when
    /// one process acts for several people across many methods.
    pub async fn list_decisions_as(
        &self,
        opts: ListDecisionsOptions,
        user_token: Option<&str>,
    ) -> Result<Vec<DecisionSummary>, AxonFlowError> {
        let mut url = format!("{}/api/v1/decisions", self.endpoint());
        let qs = build_decisions_query(&opts);
        if !qs.is_empty() {
            url.push('?');
            url.push_str(&qs);
        }

        // raw_get bypasses check_status so we can branch on 429 BEFORE
        // it turns into a generic ApiError. Other failures fall through
        // to the same shape check_status would have produced.
        let resp = self.raw_get_as(&url, user_token).await?;
        let scope = crate::read_identity::ReadScope::of(&resp);
        let status = resp.status().as_u16();
        if resp.status().as_u16() == 429 {
            let body = resp.text().await?;
            return match serde_json::from_str::<RateLimitEnvelope>(&body) {
                Ok(envelope) => Err(AxonFlowError::RateLimited {
                    envelope: Box::new(envelope),
                }),
                Err(_) => Err(AxonFlowError::ApiError {
                    status: 429,
                    message: body,
                }),
            };
        }
        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let message = resp.text().await?;
            return Err(AxonFlowError::ApiError { status, message });
        }
        let body = resp.text().await?;
        #[derive(Deserialize)]
        struct ListResponse {
            #[serde(default)]
            decisions: Vec<DecisionSummary>,
        }
        let parsed: ListResponse = serde_json::from_str(&body)?;

        // An empty page under ReadScope::None is the fail-closed shape, not a
        // finding: the platform returned zero rows because it resolved no
        // identity to scope on, so the page says nothing about what exists.
        if let Some(refusal) = crate::read_identity::refuse_vacuous_scoped_page(
            "decisions",
            scope,
            status,
            parsed.decisions.len(),
        ) {
            return Err(AxonFlowError::ReadScope(Box::new(refusal)));
        }
        Ok(parsed.decisions)
    }
}

/// Builds the URL-encoded query string from [`ListDecisionsOptions`].
/// Empty / `None` fields are omitted so the platform applies its tier
/// defaults. Field order is stable so test mocks can match the URL.
fn build_decisions_query(opts: &ListDecisionsOptions) -> String {
    let mut pairs: Vec<(&str, String)> = Vec::with_capacity(5);
    if let Some(since) = &opts.since {
        // Use the "Z" UTC marker rather than "+00:00" — `+` in a query
        // string decodes to space under application/x-www-form-urlencoded,
        // so emitting `+00:00` would wire-corrupt the timestamp on the
        // platform side.
        pairs.push((
            "since",
            since.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        ));
    }
    if let Some(decision) = &opts.decision {
        pairs.push(("decision", decision.clone()));
    }
    if let Some(policy_id) = &opts.policy_id {
        pairs.push(("policy_id", policy_id.clone()));
    }
    if let Some(tool_signature) = &opts.tool_signature {
        pairs.push(("tool_signature", tool_signature.clone()));
    }
    if let Some(limit) = opts.limit {
        pairs.push(("limit", limit.to_string()));
    }
    pairs
        .into_iter()
        .map(|(k, v)| {
            let v = utf8_percent_encode(&v, PATH_SEGMENT).to_string();
            format!("{k}={v}")
        })
        .collect::<Vec<_>>()
        .join("&")
}

#[cfg(test)]
mod tests {
    use crate::types::decisions::DecisionExplanation;
    use crate::{AxonFlowClient, AxonFlowConfig};
    use chrono::{TimeZone, Utc};
    use serde_json::json;
    use std::time::Duration;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

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
        let server = MockServer::start().await;
        let want = json!({
            "decision_id": "dec_wf1_step2",
            "timestamp": "2026-04-17T12:00:00Z",
            "decision": "blocked",
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

        Mock::given(method("GET"))
            .and(path("/api/v1/decisions/dec_wf1_step2/explain"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/json")
                    .set_body_json(want),
            )
            .expect(1)
            .mount(&server)
            .await;

        let client = make_client(server.uri());
        let got = client.explain_decision("dec_wf1_step2").await.unwrap();

        assert_eq!(got.decision_id, "dec_wf1_step2");
        assert_eq!(got.decision, "blocked");
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
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/decisions/a%2Fb/explain"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "decision_id": "a/b",
                "timestamp": "2026-04-17T12:00:00Z",
                "decision": "allowed",
                "reason": "",
                "policy_matches": []
            })))
            .expect(1)
            .mount(&server)
            .await;

        let client = make_client(server.uri());
        let _ = client.explain_decision("a/b").await.unwrap();
    }

    #[tokio::test]
    async fn http_404_surfaces_as_api_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/decisions/dec-missing/explain"))
            .respond_with(
                ResponseTemplate::new(404)
                    .set_body_json(json!({"error": "Decision not found or past retention window"})),
            )
            .mount(&server)
            .await;

        let client = make_client(server.uri());
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
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/decisions/dec-x/explain"))
            .respond_with(
                ResponseTemplate::new(401)
                    .set_body_json(json!({"error": "X-Tenant-ID header is required"})),
            )
            .mount(&server)
            .await;

        let client = make_client(server.uri());
        let err = client.explain_decision("dec-x").await.unwrap_err();
        match err {
            crate::error::AxonFlowError::ApiError { status, .. } => assert_eq!(status, 401),
            other => panic!("expected ApiError(401), got: {other}"),
        }
    }

    #[tokio::test]
    async fn malformed_json_response_is_serde_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/decisions/dec-x/explain"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/json")
                    .set_body_string("{not valid json"),
            )
            .mount(&server)
            .await;

        let client = make_client(server.uri());
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
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/decisions/dec-x/explain"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "decision_id": "dec-x",
                "timestamp": "2026-04-17T12:00:00Z",
                "decision": "allowed",
                "reason": "",
                "policy_matches": [],
                "policy_version_at_decision": "v3",      // future-additive (V1.1)
                "latest_policy_version": "v5",            // future-additive (V1.1)
                "yet_another_future_field": "shrug"      // arbitrary forward-compat
            })))
            .mount(&server)
            .await;

        let client = make_client(server.uri());
        let got: DecisionExplanation = client.explain_decision("dec-x").await.unwrap();
        assert_eq!(got.decision_id, "dec-x");
    }

    // ------------------------------------------------------------------
    // list_decisions — 6 contract tests covering happy path, every
    // filter, the 429 upgrade envelope, 401, and forward-compat.
    // ------------------------------------------------------------------

    use crate::decisions::build_decisions_query;
    use crate::error::AxonFlowError;
    use crate::types::decisions::{DecisionSummary, ListDecisionsOptions};

    #[tokio::test]
    async fn list_decisions_happy_path_parses_three_rows() {
        let server = MockServer::start().await;
        let want = json!({
            "decisions": [
                {
                    "decision_id": "dec-1",
                    "timestamp": "2026-05-07T12:00:00Z",
                    "decision": "blocked",
                    "policy_id": "pol-sqli",
                    "tool_signature": "postgres.query"
                },
                {
                    "decision_id": "dec-2",
                    "timestamp": "2026-05-07T11:00:00Z",
                    "decision": "allowed",
                    "policy_id": "pol-default",
                    "tool_signature": "github.status"
                },
                {
                    "decision_id": "dec-3",
                    "timestamp": "2026-05-07T10:00:00Z",
                    "decision": "needs_approval",
                    "policy_id": "pol-amount",
                    "tool_signature": "stripe.charge"
                }
            ]
        });

        Mock::given(method("GET"))
            .and(path("/api/v1/decisions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/json")
                    .set_body_json(want),
            )
            .mount(&server)
            .await;

        let client = make_client(server.uri());
        let got = client
            .list_decisions(ListDecisionsOptions::default())
            .await
            .unwrap();

        assert_eq!(got.len(), 3);
        assert_eq!(got[0].decision_id, "dec-1");
        assert_eq!(got[0].decision, "blocked");
        assert_eq!(got[0].policy_id.as_deref(), Some("pol-sqli"));
        assert_eq!(got[0].tool_signature.as_deref(), Some("postgres.query"));
        assert_eq!(got[2].decision, "needs_approval");
    }

    #[tokio::test]
    async fn list_decisions_serializes_every_filter_into_url() {
        let server = MockServer::start().await;
        // Mock matches the EXACT query string we expect — if the SDK
        // forgets to register a field in the URL builder, the mock
        // returns 404 and the test fails via the unmatched-request
        // assertion (.expect(1) below).
        Mock::given(method("GET"))
            .and(path("/api/v1/decisions"))
            .and(query_param("since", "2026-05-07T00:00:00Z"))
            .and(query_param("decision", "blocked"))
            .and(query_param("policy_id", "pol-sqli"))
            .and(query_param("tool_signature", "postgres.query"))
            .and(query_param("limit", "25"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"decisions": []})))
            .expect(1)
            .mount(&server)
            .await;

        let client = make_client(server.uri());
        let opts = ListDecisionsOptions {
            since: Some(Utc.with_ymd_and_hms(2026, 5, 7, 0, 0, 0).unwrap()),
            decision: Some("blocked".into()),
            policy_id: Some("pol-sqli".into()),
            tool_signature: Some("postgres.query".into()),
            limit: Some(25),
        };
        let _ = client.list_decisions(opts).await.unwrap();
    }

    #[tokio::test]
    async fn list_decisions_429_surfaces_typed_rate_limit_envelope() {
        let server = MockServer::start().await;
        let envelope = json!({
            "error": "Free tier shows the last 5 decisions in 24h. Pro raises this to 100 decisions in the last 30 days.",
            "limit_type": "decision_list_size",
            "tier": "Community",
            "limit": 5,
            "remaining": 0,
            "upgrade": {
                "tier": "Pro",
                "wording": "Free tier shows the last 5 decisions in 24h. Pro raises this to 100 decisions in the last 30 days.",
                "compare_url": "https://getaxonflow.com/pricing/",
                "buy_url": "https://buy.stripe.com/bJe28qbztcdVchjdkw8k800"
            }
        });

        Mock::given(method("GET"))
            .and(path("/api/v1/decisions"))
            .respond_with(
                ResponseTemplate::new(429)
                    .insert_header("content-type", "application/json")
                    .insert_header("X-Axonflow-Tier-Limit", "decision_list_size")
                    .set_body_json(envelope),
            )
            .mount(&server)
            .await;

        let client = make_client(server.uri());
        let err = client
            .list_decisions(ListDecisionsOptions {
                limit: Some(10),
                ..Default::default()
            })
            .await
            .expect_err("must reject with RateLimited");

        match err {
            AxonFlowError::RateLimited { envelope } => {
                assert_eq!(envelope.tier, "Community");
                assert_eq!(envelope.limit_type, "decision_list_size");
                assert_eq!(envelope.limit, 5);
                assert_eq!(envelope.upgrade.tier, "Pro");
                assert_eq!(
                    envelope.upgrade.compare_url,
                    "https://getaxonflow.com/pricing/"
                );
                assert_eq!(
                    envelope.upgrade.buy_url,
                    "https://buy.stripe.com/bJe28qbztcdVchjdkw8k800"
                );
            }
            other => panic!("expected RateLimited, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn list_decisions_429_with_malformed_body_falls_back_to_apierror() {
        // If the platform changes the 429 shape and the SDK can't parse
        // the envelope, we must still surface the 429 — not panic or
        // silently succeed. Falls through to ApiError{status=429}.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/decisions"))
            .respond_with(
                ResponseTemplate::new(429)
                    .insert_header("content-type", "application/json")
                    .set_body_string("not a json envelope"),
            )
            .mount(&server)
            .await;

        let client = make_client(server.uri());
        let err = client
            .list_decisions(ListDecisionsOptions::default())
            .await
            .expect_err("must reject");
        match err {
            AxonFlowError::ApiError { status, .. } => assert_eq!(status, 429),
            other => panic!("expected ApiError{{status=429}}, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn list_decisions_401_surfaces_as_apierror() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/decisions"))
            .respond_with(
                ResponseTemplate::new(401)
                    .insert_header("content-type", "application/json")
                    .set_body_json(json!({"error": "X-Tenant-ID header is required"})),
            )
            .mount(&server)
            .await;

        let client = make_client(server.uri());
        let err = client
            .list_decisions(ListDecisionsOptions::default())
            .await
            .expect_err("must reject");
        match err {
            AxonFlowError::ApiError { status, message } => {
                assert_eq!(status, 401);
                assert!(message.contains("X-Tenant-ID"), "msg = {message}");
            }
            other => panic!("expected ApiError{{status=401}}, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn list_decisions_forward_compat_unknown_fields_ignored() {
        let server = MockServer::start().await;
        let want = json!({
            "decisions": [{
                "decision_id": "dec-fwd",
                "timestamp": "2026-05-07T12:00:00Z",
                "decision": "blocked",
                "policy_id": "pol-x",
                "tool_signature": "tool-x",
                "policy_version": 7,                  // future-additive (#1983 α3)
                "latest_policy_version": 9,           // future-additive
                "arbitrary_unknown": "ignored"        // arbitrary forward-compat
            }],
            "next_cursor": "future_cursor_pagination" // outer envelope additive
        });

        Mock::given(method("GET"))
            .and(path("/api/v1/decisions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(want))
            .mount(&server)
            .await;

        let client = make_client(server.uri());
        let got = client
            .list_decisions(ListDecisionsOptions::default())
            .await
            .unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].decision_id, "dec-fwd");
    }

    #[test]
    fn build_decisions_query_omits_none_fields() {
        let qs = build_decisions_query(&ListDecisionsOptions::default());
        assert_eq!(qs, "");

        let qs = build_decisions_query(&ListDecisionsOptions {
            decision: Some("blocked".into()),
            limit: Some(7),
            ..Default::default()
        });
        assert_eq!(qs, "decision=blocked&limit=7");
    }

    #[test]
    fn decision_summary_optional_fields_round_trip() {
        // Platform may write rows without policy_id / tool_signature
        // (dynamic-only blocks); SDK must accept them as Option::None
        // and round-trip without emitting empty strings.
        let raw = json!({
            "decision_id": "dec-min",
            "timestamp": "2026-05-07T12:00:00Z",
            "decision": "blocked"
        });
        let parsed: DecisionSummary = serde_json::from_value(raw).unwrap();
        assert_eq!(parsed.decision_id, "dec-min");
        assert_eq!(parsed.policy_id, None);
        assert_eq!(parsed.tool_signature, None);
        // Re-serialize: omitempty must drop the optional fields.
        let s = serde_json::to_string(&parsed).unwrap();
        assert!(!s.contains("policy_id"), "policy_id must be omitted: {s}");
        assert!(
            !s.contains("tool_signature"),
            "tool_signature must be omitted: {s}"
        );
    }
}
