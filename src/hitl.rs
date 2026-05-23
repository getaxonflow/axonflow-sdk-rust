// HITL (Human-in-the-Loop) Queue methods for the AxonFlow Rust SDK.
//
// Implements the full HITL surface added in v0.4.0
// (getaxonflow/axonflow-enterprise#2421 — Rust was the only SDK
// without ANY HITL methods, so this PR is the entire 6-method surface
// rather than just the create-method delta the other four SDKs ship).
//
// Method shape mirrors the cross-SDK contract used in Python /
// TypeScript / Go / Java:
//
//   list_hitl_queue(opts) -> HITLQueueListResponse
//   get_hitl_request(request_id) -> HITLApprovalRequest
//   create_hitl_request(input) -> HITLApprovalRequest          [#2421]
//   approve_hitl_request(request_id, review) -> ()
//   reject_hitl_request(request_id, review) -> ()
//   get_hitl_stats() -> HITLStats

use crate::client::{AxonFlowClient, PATH_SEGMENT};
use crate::error::AxonFlowError;
use crate::types::hitl::{
    HITLApprovalRequest, HITLCreateInput, HITLQueueListOptions, HITLQueueListResponse,
    HITLReviewInput, HITLStats, HitlItemEnvelope, HitlListEnvelope, HitlStatsEnvelope,
};
use percent_encoding::utf8_percent_encode;

impl AxonFlowClient {
    /// Lists approval requests in the HITL queue.
    ///
    /// Enterprise Feature: Requires AxonFlow Enterprise license.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use axonflow_sdk_rust::{AxonFlowClient, AxonFlowConfig, HITLQueueListOptions};
    /// # async fn run() -> Result<(), Box<dyn std::error::Error>> {
    /// let client = AxonFlowClient::new(AxonFlowConfig::new("http://localhost:8080"))?;
    /// let opts = HITLQueueListOptions {
    ///     status: Some("pending".into()),
    ///     severity: Some("high,critical".into()),
    ///     limit: Some(20),
    ///     ..Default::default()
    /// };
    /// let page = client.list_hitl_queue(opts).await?;
    /// println!("{} of {} pending HITL approvals", page.items.len(), page.total);
    /// # Ok(()) }
    /// ```
    pub async fn list_hitl_queue(
        &self,
        opts: HITLQueueListOptions,
    ) -> Result<HITLQueueListResponse, AxonFlowError> {
        let mut url = format!("{}/api/v1/hitl/queue", self.endpoint());
        let qs = build_list_query(&opts);
        if !qs.is_empty() {
            url.push('?');
            url.push_str(&qs);
        }

        let resp = self.checked_get(&url).await?;
        let body = resp.text().await?;
        let envelope: HitlListEnvelope = serde_json::from_str(&body)?;
        let total = envelope.meta.total;
        let returned = envelope.data.len() as i64;
        let offset = envelope.meta.offset;
        Ok(HITLQueueListResponse {
            items: envelope.data,
            total,
            has_more: offset + returned < total,
        })
    }

    /// Gets a specific HITL approval request by ID.
    ///
    /// Enterprise Feature: Requires AxonFlow Enterprise license.
    pub async fn get_hitl_request(
        &self,
        request_id: &str,
    ) -> Result<HITLApprovalRequest, AxonFlowError> {
        if request_id.is_empty() {
            return Err(AxonFlowError::ConfigError(
                "request_id is required".to_string(),
            ));
        }
        let encoded = utf8_percent_encode(request_id, PATH_SEGMENT).to_string();
        let url = format!("{}/api/v1/hitl/queue/{}", self.endpoint(), encoded);

        let resp = self.checked_get(&url).await?;
        let body = resp.text().await?;
        let envelope: HitlItemEnvelope = serde_json::from_str(&body)?;
        Ok(envelope.data)
    }

    /// Creates a new HITL approval request via `POST /api/v1/hitl/queue`.
    ///
    /// Enterprise Feature: Requires AxonFlow Enterprise license. The
    /// platform returns 403 with `ErrHITLApprovalDisabledByTier` when
    /// called against a community tier that hasn't enabled HITL, and
    /// 401 when credentials are invalid.
    ///
    /// This is the explicit row-creation step for callers that detect
    /// `require_approval` from a separate gate (`pre_check`,
    /// `check_tool_input`, MAP plan approvals) and want the row
    /// enqueued so a reviewer can act on it. After creating, either
    /// poll [`get_hitl_request`](Self::get_hitl_request) until terminal
    /// state, or supply
    /// [`HITLCreateInput::notify_url`](crate::HITLCreateInput::notify_url)
    /// so the platform fires a signed webhook on the transition (n8n
    /// Wait-node "On Webhook Call" pattern, ADK plugin polling-free
    /// mode).
    ///
    /// `client_id`, `original_query`, and `request_type` are required;
    /// all other fields are optional. Bad `notify_url` schemes are
    /// rejected by the platform with HTTP 400 (surfaced here as
    /// [`AxonFlowError::ApiError`] with `status = 400`); only `https://`
    /// (and `http://` for self-hosted local-dev) are accepted.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use axonflow_sdk_rust::{AxonFlowClient, AxonFlowConfig, HITLCreateInput};
    /// # async fn run() -> Result<(), Box<dyn std::error::Error>> {
    /// let client = AxonFlowClient::new(AxonFlowConfig::new("http://localhost:8080"))?;
    /// let req = client
    ///     .create_hitl_request(HITLCreateInput {
    ///         client_id: "loan-desk".into(),
    ///         original_query: "disburse $50000 to cust-001".into(),
    ///         request_type: "adk-tool".into(),
    ///         triggered_policy_id: Some("loan-amount-cap".into()),
    ///         triggered_policy_name: Some("Loan amount cap".into()),
    ///         trigger_reason: Some(
    ///             "Disbursement above $10k requires manager approval".into(),
    ///         ),
    ///         severity: Some("high".into()),
    ///         notify_url: Some(
    ///             "https://workflows.example.com/hooks/loan-approve".into(),
    ///         ),
    ///         ..Default::default()
    ///     })
    ///     .await?;
    /// println!("Created HITL approval {}", req.request_id);
    /// # Ok(()) }
    /// ```
    pub async fn create_hitl_request(
        &self,
        input: HITLCreateInput,
    ) -> Result<HITLApprovalRequest, AxonFlowError> {
        if input.client_id.is_empty() {
            return Err(AxonFlowError::ConfigError(
                "client_id is required".to_string(),
            ));
        }
        if input.original_query.is_empty() {
            return Err(AxonFlowError::ConfigError(
                "original_query is required".to_string(),
            ));
        }
        if input.request_type.is_empty() {
            return Err(AxonFlowError::ConfigError(
                "request_type is required".to_string(),
            ));
        }

        let url = format!("{}/api/v1/hitl/queue", self.endpoint());
        let resp = self.checked_post_json(&url, &input).await?;
        let body = resp.text().await?;
        let envelope: HitlItemEnvelope = serde_json::from_str(&body)?;
        Ok(envelope.data)
    }

    /// Approves a pending HITL approval request.
    ///
    /// Enterprise Feature: Requires AxonFlow Enterprise license.
    pub async fn approve_hitl_request(
        &self,
        request_id: &str,
        review: HITLReviewInput,
    ) -> Result<(), AxonFlowError> {
        self.review_hitl_request(request_id, "approve", &review)
            .await
    }

    /// Rejects a pending HITL approval request.
    ///
    /// Enterprise Feature: Requires AxonFlow Enterprise license.
    pub async fn reject_hitl_request(
        &self,
        request_id: &str,
        review: HITLReviewInput,
    ) -> Result<(), AxonFlowError> {
        self.review_hitl_request(request_id, "reject", &review)
            .await
    }

    /// Gets HITL queue dashboard statistics.
    ///
    /// Enterprise Feature: Requires AxonFlow Enterprise license.
    pub async fn get_hitl_stats(&self) -> Result<HITLStats, AxonFlowError> {
        let url = format!("{}/api/v1/hitl/stats", self.endpoint());
        let resp = self.checked_get(&url).await?;
        let body = resp.text().await?;
        let envelope: HitlStatsEnvelope = serde_json::from_str(&body)?;
        Ok(envelope.data)
    }

    /// Shared helper for the approve/reject endpoints — same shape, same
    /// error mapping, distinct path suffix.
    async fn review_hitl_request(
        &self,
        request_id: &str,
        action: &str,
        review: &HITLReviewInput,
    ) -> Result<(), AxonFlowError> {
        if request_id.is_empty() {
            return Err(AxonFlowError::ConfigError(
                "request_id is required".to_string(),
            ));
        }
        let encoded = utf8_percent_encode(request_id, PATH_SEGMENT).to_string();
        let url = format!(
            "{}/api/v1/hitl/queue/{}/{}",
            self.endpoint(),
            encoded,
            action
        );
        let _ = self.checked_post_json(&url, review).await?;
        Ok(())
    }
}

/// Builds the URL-encoded query string from [`HITLQueueListOptions`].
/// Empty/`None` fields are omitted so the platform applies its tier
/// defaults. Field order is stable so test mocks can match the URL.
fn build_list_query(opts: &HITLQueueListOptions) -> String {
    let mut pairs: Vec<(&str, String)> = Vec::with_capacity(4);
    if let Some(status) = &opts.status {
        pairs.push(("status", status.clone()));
    }
    if let Some(severity) = &opts.severity {
        pairs.push(("severity", severity.clone()));
    }
    if let Some(limit) = opts.limit {
        pairs.push(("limit", limit.to_string()));
    }
    if let Some(offset) = opts.offset {
        pairs.push(("offset", offset.to_string()));
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
    use super::*;
    use crate::{AxonFlowClient, AxonFlowConfig};
    use serde_json::json;
    use std::time::Duration;
    use wiremock::matchers::{body_partial_json, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn make_client(endpoint: String) -> AxonFlowClient {
        let config = AxonFlowConfig {
            endpoint,
            timeout: Duration::from_secs(2),
            ..Default::default()
        };
        AxonFlowClient::new(config).expect("client init")
    }

    fn sample_row() -> serde_json::Value {
        json!({
            "request_id": "hitl-req-runtime-001",
            "org_id": "org-1",
            "tenant_id": "tenant-1",
            "client_id": "loan-desk",
            "user_id": "cust-001",
            "original_query": "disburse $50000 to cust-001",
            "request_type": "adk-tool",
            "request_context": {"tool_name": "disburse_payment"},
            "triggered_policy_id": "loan-amount-cap",
            "triggered_policy_name": "Loan amount cap",
            "trigger_reason": "Disbursement above $10k requires manager approval",
            "severity": "high",
            "status": "pending",
            "notify_url": "https://workflows.example.com/hooks/loan-approve",
            "expires_at": "2026-05-23T11:00:00Z",
            "created_at": "2026-05-23T10:00:00Z",
            "updated_at": "2026-05-23T10:00:00Z",
        })
    }

    // ---- list ----

    #[tokio::test]
    async fn list_happy_path_parses_payload_and_pagination() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/hitl/queue"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "success": true,
                "data": [sample_row()],
                "meta": {"total": 1, "limit": 50, "offset": 0},
            })))
            .mount(&server)
            .await;

        let client = make_client(server.uri());
        let page = client
            .list_hitl_queue(HITLQueueListOptions::default())
            .await
            .unwrap();

        assert_eq!(page.total, 1);
        assert_eq!(page.items.len(), 1);
        assert!(!page.has_more);
        assert_eq!(page.items[0].request_id, "hitl-req-runtime-001");
        assert_eq!(
            page.items[0].notify_url.as_deref(),
            Some("https://workflows.example.com/hooks/loan-approve")
        );
    }

    #[tokio::test]
    async fn list_passes_filters_via_query_string() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/hitl/queue"))
            .and(query_param("status", "pending"))
            .and(query_param("severity", "critical"))
            .and(query_param("limit", "5"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "success": true,
                "data": [],
                "meta": {"total": 0, "limit": 5, "offset": 0},
            })))
            .expect(1)
            .mount(&server)
            .await;

        let client = make_client(server.uri());
        let opts = HITLQueueListOptions {
            status: Some("pending".into()),
            severity: Some("critical".into()),
            limit: Some(5),
            offset: None,
        };
        let _ = client.list_hitl_queue(opts).await.unwrap();
    }

    // ---- get ----

    #[tokio::test]
    async fn get_happy_path_parses_full_row() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/hitl/queue/hitl-req-runtime-001"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "success": true,
                "data": sample_row(),
            })))
            .mount(&server)
            .await;

        let client = make_client(server.uri());
        let got = client
            .get_hitl_request("hitl-req-runtime-001")
            .await
            .unwrap();
        assert_eq!(got.request_id, "hitl-req-runtime-001");
        assert_eq!(got.severity, "high");
        assert_eq!(
            got.notify_url.as_deref(),
            Some("https://workflows.example.com/hooks/loan-approve")
        );
    }

    #[tokio::test]
    async fn get_empty_id_returns_config_error() {
        let client = make_client("http://127.0.0.1:1".into());
        let err = client.get_hitl_request("").await.unwrap_err();
        assert!(err.to_string().contains("request_id is required"));
    }

    #[tokio::test]
    async fn get_404_surfaces_as_api_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/hitl/queue/nope"))
            .respond_with(ResponseTemplate::new(404).set_body_json(json!({"error": "not found"})))
            .mount(&server)
            .await;

        let client = make_client(server.uri());
        let err = client.get_hitl_request("nope").await.unwrap_err();
        match err {
            AxonFlowError::ApiError { status, .. } => assert_eq!(status, 404),
            other => panic!("expected ApiError(404), got {other}"),
        }
    }

    // ---- create ----

    #[tokio::test]
    async fn create_happy_path_round_trips_full_input() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/hitl/queue"))
            .and(body_partial_json(json!({
                "client_id": "loan-desk",
                "original_query": "disburse $50000 to cust-001",
                "request_type": "adk-tool",
                "notify_url": "https://workflows.example.com/hooks/loan-approve",
                "severity": "high",
            })))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({
                "success": true,
                "data": sample_row(),
            })))
            .expect(1)
            .mount(&server)
            .await;

        let client = make_client(server.uri());
        let req = client
            .create_hitl_request(HITLCreateInput {
                client_id: "loan-desk".into(),
                user_id: Some("cust-001".into()),
                original_query: "disburse $50000 to cust-001".into(),
                request_type: "adk-tool".into(),
                triggered_policy_id: Some("loan-amount-cap".into()),
                triggered_policy_name: Some("Loan amount cap".into()),
                trigger_reason: Some("Disbursement above $10k requires manager approval".into()),
                severity: Some("high".into()),
                notify_url: Some("https://workflows.example.com/hooks/loan-approve".into()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(req.request_id, "hitl-req-runtime-001");
        assert_eq!(
            req.notify_url.as_deref(),
            Some("https://workflows.example.com/hooks/loan-approve")
        );
    }

    #[tokio::test]
    async fn create_minimal_required_fields_only() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/hitl/queue"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({
                "success": true,
                "data": {
                    "request_id": "hitl-req-minimal",
                    "org_id": "org-1",
                    "tenant_id": "tenant-1",
                    "client_id": "c1",
                    "original_query": "q",
                    "request_type": "chat",
                    "triggered_policy_id": "",
                    "triggered_policy_name": "",
                    "trigger_reason": "",
                    "severity": "high",
                    "status": "pending",
                    "expires_at": "2026-05-23T11:00:00Z",
                    "created_at": "2026-05-23T10:00:00Z",
                    "updated_at": "2026-05-23T10:00:00Z",
                },
            })))
            .mount(&server)
            .await;

        let client = make_client(server.uri());
        let req = client
            .create_hitl_request(HITLCreateInput {
                client_id: "c1".into(),
                original_query: "q".into(),
                request_type: "chat".into(),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(req.request_id, "hitl-req-minimal");
        assert_eq!(req.notify_url, None);
    }

    #[tokio::test]
    async fn create_bad_notify_url_scheme_surfaces_400() {
        // Mirrors platform/agent/hitl/webhook.go:105 ValidateNotifyURL.
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/hitl/queue"))
            .respond_with(ResponseTemplate::new(400).set_body_json(json!({
                "success": false,
                "error": "notify_url scheme \"javascript\" is not allowed (use https:// or http://)",
            })))
            .mount(&server)
            .await;

        let client = make_client(server.uri());
        let err = client
            .create_hitl_request(HITLCreateInput {
                client_id: "loan-desk".into(),
                original_query: "disburse $50000".into(),
                request_type: "adk-tool".into(),
                notify_url: Some("javascript:alert(1)".into()),
                ..Default::default()
            })
            .await
            .unwrap_err();
        match err {
            AxonFlowError::ApiError { status, .. } => assert_eq!(status, 400),
            other => panic!("expected ApiError(400), got {other}"),
        }
    }

    #[tokio::test]
    async fn create_401_surfaces_as_api_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/hitl/queue"))
            .respond_with(ResponseTemplate::new(401).set_body_json(json!({
                "success": false,
                "error": "Invalid API key",
            })))
            .mount(&server)
            .await;

        let client = make_client(server.uri());
        let err = client
            .create_hitl_request(HITLCreateInput {
                client_id: "loan-desk".into(),
                original_query: "disburse $50000".into(),
                request_type: "adk-tool".into(),
                ..Default::default()
            })
            .await
            .unwrap_err();
        match err {
            AxonFlowError::ApiError { status, .. } => assert_eq!(status, 401),
            other => panic!("expected ApiError(401), got {other}"),
        }
    }

    #[tokio::test]
    async fn create_network_failure_surfaces_as_error() {
        // Bind+close to get a guaranteed-unused localhost port.
        let server = MockServer::start().await;
        let url = server.uri();
        drop(server);

        let client = make_client(url);
        let err = client
            .create_hitl_request(HITLCreateInput {
                client_id: "loan-desk".into(),
                original_query: "disburse $50000".into(),
                request_type: "adk-tool".into(),
                ..Default::default()
            })
            .await
            .unwrap_err();
        // Either HttpError or ApiError depending on connect-vs-response race
        // — either is correct propagation.
        let _ = err;
    }

    #[tokio::test]
    async fn create_missing_client_id_rejected() {
        let client = make_client("http://127.0.0.1:1".into());
        let err = client
            .create_hitl_request(HITLCreateInput {
                client_id: "".into(),
                original_query: "q".into(),
                request_type: "chat".into(),
                ..Default::default()
            })
            .await
            .unwrap_err();
        assert!(err.to_string().contains("client_id is required"));
    }

    #[tokio::test]
    async fn create_missing_original_query_rejected() {
        let client = make_client("http://127.0.0.1:1".into());
        let err = client
            .create_hitl_request(HITLCreateInput {
                client_id: "c1".into(),
                original_query: "".into(),
                request_type: "chat".into(),
                ..Default::default()
            })
            .await
            .unwrap_err();
        assert!(err.to_string().contains("original_query is required"));
    }

    #[tokio::test]
    async fn create_missing_request_type_rejected() {
        let client = make_client("http://127.0.0.1:1".into());
        let err = client
            .create_hitl_request(HITLCreateInput {
                client_id: "c1".into(),
                original_query: "q".into(),
                request_type: "".into(),
                ..Default::default()
            })
            .await
            .unwrap_err();
        assert!(err.to_string().contains("request_type is required"));
    }

    // ---- approve / reject ----

    #[tokio::test]
    async fn approve_posts_review_input_to_correct_path() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/hitl/queue/hitl-req-runtime-001/approve"))
            .and(body_partial_json(json!({
                "reviewer_id": "user_456",
                "reviewer_email": "reviewer@example.com",
                "comment": "Approved after review",
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"success": true})))
            .expect(1)
            .mount(&server)
            .await;

        let client = make_client(server.uri());
        client
            .approve_hitl_request(
                "hitl-req-runtime-001",
                HITLReviewInput {
                    reviewer_id: "user_456".into(),
                    reviewer_email: "reviewer@example.com".into(),
                    reviewer_role: None,
                    comment: Some("Approved after review".into()),
                },
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn reject_posts_review_input_to_correct_path() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/hitl/queue/hitl-req-runtime-001/reject"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"success": true})))
            .expect(1)
            .mount(&server)
            .await;

        let client = make_client(server.uri());
        client
            .reject_hitl_request(
                "hitl-req-runtime-001",
                HITLReviewInput {
                    reviewer_id: "user_456".into(),
                    reviewer_email: "reviewer@example.com".into(),
                    reviewer_role: None,
                    comment: None,
                },
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn approve_empty_id_rejected_before_http() {
        let client = make_client("http://127.0.0.1:1".into());
        let err = client
            .approve_hitl_request(
                "",
                HITLReviewInput {
                    reviewer_id: "u".into(),
                    reviewer_email: "u@e".into(),
                    reviewer_role: None,
                    comment: None,
                },
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("request_id is required"));
    }

    // ---- stats ----

    #[tokio::test]
    async fn stats_happy_path_parses_envelope() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/hitl/stats"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "success": true,
                "data": {
                    "total_pending": 12,
                    "high_priority": 4,
                    "critical_priority": 2,
                    "oldest_pending_hours": 9.5,
                },
            })))
            .mount(&server)
            .await;

        let client = make_client(server.uri());
        let stats = client.get_hitl_stats().await.unwrap();
        assert_eq!(stats.total_pending, 12);
        assert_eq!(stats.high_priority, 4);
        assert_eq!(stats.critical_priority, 2);
        assert_eq!(stats.oldest_pending_hours, Some(9.5));
    }

    #[test]
    fn build_list_query_omits_none_fields() {
        let qs = build_list_query(&HITLQueueListOptions::default());
        assert_eq!(qs, "");
        let qs = build_list_query(&HITLQueueListOptions {
            status: Some("pending".into()),
            severity: None,
            limit: Some(20),
            offset: None,
        });
        assert_eq!(qs, "status=pending&limit=20");
    }
}
