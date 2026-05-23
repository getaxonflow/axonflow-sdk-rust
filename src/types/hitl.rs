// HITL (Human-in-the-Loop) Queue types for the AxonFlow Rust SDK.
//
// Mirrors `platform/agent/hitl/handler.go` and the sister SDK
// implementations (Python / TypeScript / Go / Java).
//
// Issue: getaxonflow/axonflow-enterprise#2421 — closes the cross-SDK
// HITL gap. Rust additionally lacked the entire HITL surface (was the
// reason this is filed as a larger PR than the other four).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A pending or resolved HITL approval request.
///
/// Returned by every HITL read/write endpoint. `notify_url` is the new
/// field introduced in getaxonflow/axonflow-enterprise#2419 and may be
/// absent in payloads from platforms that don't implement the
/// outbound-webhook dispatcher yet — `Option<String>` keeps the parse
/// forward-compatible.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HITLApprovalRequest {
    pub request_id: String,
    pub org_id: String,
    pub tenant_id: String,
    pub client_id: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub user_id: Option<String>,
    pub original_query: String,
    pub request_type: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub request_context: Option<HashMap<String, serde_json::Value>>,
    pub triggered_policy_id: String,
    pub triggered_policy_name: String,
    pub trigger_reason: String,
    pub severity: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub eu_ai_act_article: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub compliance_framework: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub risk_classification: Option<String>,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub reviewer_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub reviewer_email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub review_comment: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub reviewed_at: Option<String>,
    /// Optional outbound webhook URL associated with the request.
    /// Mirrors the value supplied on creation. Platforms that implement
    /// the outbound-webhook dispatcher (introduced in
    /// getaxonflow/axonflow-enterprise#2419) fire a signed POST to this
    /// URL after the request reaches a terminal state
    /// (approved/rejected/expired/overridden). Platforms that don't,
    /// simply round-trip the field.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub notify_url: Option<String>,
    pub expires_at: String,
    pub created_at: String,
    pub updated_at: String,
}

/// Options for listing the HITL approval queue.
#[derive(Debug, Clone, Default, Serialize)]
pub struct HITLQueueListOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub severity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset: Option<i64>,
}

/// Response from listing HITL queue approval requests.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HITLQueueListResponse {
    pub items: Vec<HITLApprovalRequest>,
    pub total: i64,
    pub has_more: bool,
}

/// Input for approving or rejecting a HITL approval request.
#[derive(Debug, Clone, Serialize)]
pub struct HITLReviewInput {
    pub reviewer_id: String,
    pub reviewer_email: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reviewer_role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
}

/// Input for creating a HITL approval request.
///
/// Mirrors `platform/agent/hitl/handler.go:86 CreateRequestInput`. The
/// platform's `POST /api/v1/hitl/queue` handler reads `X-Org-ID` and
/// `X-Tenant-ID` from request headers (set by the auth middleware from
/// the SDK client's credentials), and the JSON body must carry the
/// fields below.
///
/// Used by agent-framework callers that detect `require_approval` from
/// `pre_check` / `check_tool_input` and want to enqueue the
/// corresponding HITL row before polling the reviewer's decision (or
/// pivoting to webhook-driven resume via `notify_url`).
#[derive(Debug, Clone, Default, Serialize)]
pub struct HITLCreateInput {
    /// Client identifier that triggered the request. Required.
    pub client_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
    /// Original query that triggered the gate. Required.
    pub original_query: String,
    /// Request type (e.g. "chat", "tool", "mcp"). Required.
    pub request_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_context: Option<HashMap<String, serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub triggered_policy_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub triggered_policy_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trigger_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub severity: Option<String>,
    /// Optional outbound webhook URL fired async after terminal state
    /// transition (approved/rejected/expired/overridden). Must be
    /// `https://` (or `http://` for self-hosted local-dev). Server-side
    /// validation rejects bad schemes with HTTP 400. Pair with the
    /// HMAC-SHA256 `X-AxonFlow-Signature` header on the receiver side;
    /// signing key is the deployment-configured
    /// `AXONFLOW_HITL_WEBHOOK_SIGNING_KEY`. Introduced in
    /// getaxonflow/axonflow-enterprise#2419.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notify_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub eu_ai_act_article: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compliance_framework: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub risk_classification: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_in_seconds: Option<i64>,
}

/// HITL queue dashboard statistics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HITLStats {
    pub total_pending: i64,
    pub high_priority: i64,
    pub critical_priority: i64,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub oldest_pending_hours: Option<f64>,
}

/// Internal envelope shape for the platform's `APIResponse{success, data}` wrapper
/// on list endpoints (returns array of items + meta).
#[derive(Debug, Deserialize)]
pub(crate) struct HitlListEnvelope {
    #[serde(default)]
    pub data: Vec<HITLApprovalRequest>,
    #[serde(default)]
    pub meta: HitlListMeta,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct HitlListMeta {
    #[serde(default)]
    pub total: i64,
    /// Server echoes the limit it actually applied; SDK keeps it for
    /// forward-compat (tier-aware caps that the SDK doesn't compute)
    /// even though no current caller reads it.
    #[serde(default)]
    #[allow(dead_code)]
    pub limit: i64,
    #[serde(default)]
    pub offset: i64,
}

/// Internal envelope shape for the platform's `APIResponse{success, data}` wrapper
/// on single-item endpoints.
#[derive(Debug, Deserialize)]
pub(crate) struct HitlItemEnvelope {
    pub data: HITLApprovalRequest,
}

/// Internal envelope shape for the platform's `APIResponse{success, data}` wrapper
/// on stats endpoint.
#[derive(Debug, Deserialize)]
pub(crate) struct HitlStatsEnvelope {
    pub data: HITLStats,
}
