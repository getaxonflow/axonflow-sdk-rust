//! Decision Mode PEP (Policy Enforcement Point) wire types (ADR-056, epic #2563).
//!
//! These mirror the platform Decision API DTOs (`platform/agent/decision_handler.go`)
//! and the MCP request-redaction DTOs (`platform/agent/mcp_handler.go`). They are
//! deliberately re-declared (not derived from a shared crate) so they stay light
//! enough to vendor into a customer gateway; cross-SDK parity is enforced by the
//! shared wire field names being byte-identical with the Go / Python / TypeScript
//! / Java SDKs.
//!
//! Field names are snake_case on the wire (serde derives match the struct field
//! names verbatim), which already matches the platform JSON contract.

use serde::{Deserialize, Serialize};

/// Names the engine call a PEP makes to discharge an obligation.
///
/// Fulfillment is a property of the contract, not of PEP-author discipline: a
/// conforming PEP POSTs the obligation's source content to `endpoint` and
/// forwards the engine-redacted content the endpoint returns.
///
/// `content_types` advertises the mime-types the endpoint's detectors can
/// handle today. The contract is content-type-agnostic: a PEP holding content
/// of a type NOT in this list must fail closed rather than forward it
/// unredacted. Mirrors platform `ObligationFulfillment`.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct ObligationFulfillment {
    /// Engine path, e.g. `"/api/v1/mcp/check-input"`.
    pub endpoint: String,
    /// HTTP method, e.g. `"POST"`.
    #[serde(default)]
    pub method: String,
    /// `"request"` | `"response"`.
    pub phase: String,
    /// Mime-types the endpoint can redact today. Absent/empty means the PEP
    /// must not assume a particular detector is registered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_types: Option<Vec<String>>,
}

/// A self-describing, engine-fulfillable PEP requirement on an allow verdict.
///
/// Obligations are SELF-DESCRIBING and ENGINE-FULFILLABLE (ADR-056, #2563):
/// `/decide` is a pure PDP and never mutates content, so a `redact_pii`
/// obligation is not "go redact this yourself with your own patterns" — it is
/// "call the AxonFlow engine endpoint named in `fulfillment` to obtain
/// engine-redacted content." There is no other blessed way to satisfy it;
/// client-side redaction is forbidden. Mirrors platform `DecisionObligation`.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct Obligation {
    /// Obligation type, e.g. `"redact_pii"`.
    pub r#type: String,
    /// Human-readable detail for audit logs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// How a PEP discharges this obligation via the engine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fulfillment: Option<ObligationFulfillment>,
}

/// Gateway-asserted identity for a `/decide` request.
///
/// `org_id` / `tenant_id` are optional in the body — the auth-derived identity
/// is authoritative; body-supplied values are accepted only when they match.
/// Mirrors platform `DecisionCallerIdentity`.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct DecisionCallerIdentity {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gateway_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tenant_id: Option<String>,
}

/// Describes what the gateway is about to call. Mirrors platform `DecisionTarget`.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct DecisionTarget {
    /// `"llm"` | `"tool"` | `"agent"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub r#type: Option<String>,
    /// When `type=llm`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// When `type=llm`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// When `type=tool`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
}

/// Inbound contract for `POST /api/v1/decide`. Mirrors platform `DecideRequest`.
///
/// Required: `stage` (one of `"llm"` | `"tool"` | `"agent"`) and `query`.
/// `user_token` is optional — a PEP that supplies one gets the validated-user
/// record on the audit row; one that doesn't gets a synthesized service user.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DecideRequest {
    pub stage: String,
    pub query: String,
    #[serde(default)]
    pub caller_identity: DecisionCallerIdentity,
    #[serde(default)]
    pub target: DecisionTarget,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<serde_json::Value>,
}

impl DecideRequest {
    /// Construct a request with the two required fields. `stage` is one of
    /// `"llm"` | `"tool"` | `"agent"`.
    pub fn new(stage: impl Into<String>, query: impl Into<String>) -> Self {
        Self {
            stage: stage.into(),
            query: query.into(),
            ..Default::default()
        }
    }
}

/// PDP verdict returned by `POST /api/v1/decide`. Mirrors platform `DecideResponse`.
///
/// `obligations` is always a list so PEP code can iterate without a None-check.
/// `trace_id` is W3C-format (32 lowercase hex chars). `error` is set on the
/// deny path when the request was malformed.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DecideResponse {
    pub verdict: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasons: Option<Vec<String>>,
    #[serde(default)]
    pub obligations: Vec<Obligation>,
    #[serde(default)]
    pub evaluated_policies: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Request to the MCP request-redaction endpoint (`POST /api/v1/mcp/check-input`).
///
/// Mirrors platform `MCPCheckInputRequest`. `content_type` selects the
/// request-redaction detector (ADR-056 / #2563 addendum). When omitted the
/// platform defaults to `text/plain`; a `content_type` with no registered
/// detector is rejected (415) so a PEP fails closed rather than forwarding
/// content the engine cannot govern.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MCPCheckInputRequest {
    pub connector_type: String,
    pub statement: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tenant_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
}

/// Result of MCP request-redaction policy evaluation.
///
/// Mirrors platform `MCPCheckInputResponse`. The `redacted` / `redacted_statement`
/// / `redaction_evaluated` fields (ADR-056 / #2563) are what make a `/decide`
/// `redact_pii` obligation engine-fulfillable: when an allowed statement carries
/// PII under a redact (not block) policy the engine returns the masked statement
/// here so a PEP can forward redacted content WITHOUT hand-rolling its own
/// patterns.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MCPCheckInputResponse {
    pub allowed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub block_reason: Option<String>,
    #[serde(default)]
    pub policies_evaluated: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision_id: Option<String>,
    /// Whether the engine actually masked something in `redacted_statement`.
    #[serde(default)]
    pub redacted: bool,
    /// The engine-masked statement (present only when `redacted` is true).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redacted_statement: Option<String>,
    /// Whether the redaction detector actually RAN (regardless of whether it
    /// masked anything). A PEP fulfilling a `redact_pii` obligation MUST fail
    /// closed when this is false — it means the redactor did not run (detection
    /// disabled), so `redacted:false` would otherwise be indistinguishable from
    /// "looked, found nothing" (#2563 B1). Defaults false so a PEP stays
    /// fail-closed against a platform that predates the field.
    #[serde(default)]
    pub redaction_evaluated: bool,
}

/// Request to the MCP response-redaction endpoint (`POST /api/v1/mcp/check-output`).
///
/// Mirrors platform `MCPCheckOutputRequest`. Carried for completeness of the
/// response-phase contract; the SDK PEP helper fulfills request-phase
/// obligations only today.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MCPCheckOutputRequest {
    pub connector_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tenant_id: Option<String>,
}

/// Result of MCP response-redaction policy evaluation.
///
/// Mirrors platform `MCPCheckOutputResponse`. `redaction_evaluated` mirrors the
/// check-input field for the response phase (ADR-056 / #2563): a PEP fulfilling
/// a response-phase `redact_pii` obligation MUST fail closed when this is false.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MCPCheckOutputResponse {
    pub allowed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub block_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redacted_message: Option<String>,
    #[serde(default)]
    pub policies_evaluated: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision_id: Option<String>,
    /// Whether the response-phase redaction detector actually RAN. Defaults
    /// false so a PEP stays fail-closed against a platform that predates it.
    #[serde(default)]
    pub redaction_evaluated: bool,
}
