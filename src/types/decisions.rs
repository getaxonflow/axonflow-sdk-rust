// Decision explainability types — implements ADR-043.
//
// The DecisionExplanation shape is frozen per ADR-043. Additive fields
// may be added with `Option<>` + `serde(skip_serializing_if = "Option::is_none")`;
// renames or removals require a major version bump.
//
// Cross-SDK parity:
//   Go:     axonflow-sdk-go/decisions.go
//   Python: axonflow-sdk-python/axonflow/decisions.py
//   TS:     axonflow-sdk-typescript/src/types/decisions.ts
//   Java:   axonflow-sdk-java/src/main/java/com/getaxonflow/sdk/types/DecisionExplanation.java

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A policy reference inside a decision explanation.
#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq)]
pub struct ExplainPolicy {
    pub policy_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub risk_level: Option<String>,
    #[serde(default)]
    pub allow_override: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy_description: Option<String>,
}

/// Rule-level detail inside a decision explanation.
#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq)]
pub struct ExplainRule {
    pub policy_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rule_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rule_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matched_on: Option<String>,
}

/// Canonical payload returned by `AxonFlowClient::explain_decision`.
///
/// Shape frozen per ADR-043. Field semantics:
///
/// * `decision_id` — the global decision identifier.
/// * `timestamp` — when the decision was made.
/// * `policy_matches` — every policy that contributed to the decision,
///   with risk level and overridability.
/// * `matched_rules` — rule-level detail (optional, populated when the
///   upstream engine supports it).
/// * `decision` — `"allow"` | `"deny"` | `"require_approval"`.
/// * `reason` — human-readable reason string.
/// * `risk_level` — aggregate risk label (`"low"` | `"medium"` | `"high"` | `"critical"`).
/// * `override_available` — true iff at least one non-critical policy with
///   `allow_override = true` matched.
/// * `override_existing_id` — populated when an active override already
///   covers this caller + policy + tool scope.
/// * `historical_hit_count_session` — how many times the same
///   `(policy_id, user_email)` tuple matched in a rolling 24h window.
/// * `policy_source_link` — optional URL to the policy source.
/// * `tool_signature` — the tool the decision was scoped to (may be empty
///   when the decision had no tool context).
#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq)]
pub struct DecisionExplanation {
    pub decision_id: String,
    pub timestamp: DateTime<Utc>,
    #[serde(default)]
    pub policy_matches: Vec<ExplainPolicy>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub matched_rules: Vec<ExplainRule>,
    pub decision: String,
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub risk_level: Option<String>,
    #[serde(default)]
    pub override_available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub override_existing_id: Option<String>,
    #[serde(default)]
    pub historical_hit_count_session: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy_source_link: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_signature: Option<String>,
}

/// Slim summary returned by `AxonFlowClient::list_decisions`.
///
/// Matches the platform `GET /api/v1/decisions` contract: 5 fields,
/// `policy_id` and `tool_signature` optional because pre-α1 audit rows
/// + dynamic-only blocks may not populate them. ADR-043 §"Versioning"
/// rules apply: additive `Option<>` fields are non-breaking.
///
/// Cross-SDK parity:
///   Go:     axonflow-sdk-go/decisions.go (DecisionSummary)
///   Python: axonflow-sdk-python/axonflow/decisions.py (DecisionSummary)
///   TS:     axonflow-sdk-typescript/src/types/decisions.ts (DecisionSummary)
///   Java:   axonflow-sdk-java/src/main/java/com/getaxonflow/sdk/types/DecisionSummary.java
#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq)]
pub struct DecisionSummary {
    pub decision_id: String,
    pub timestamp: DateTime<Utc>,
    pub decision: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_signature: Option<String>,
}

/// Optional filters for `AxonFlowClient::list_decisions`.
///
/// Every field is optional — leaving all `None` returns the tier-default
/// page from the caller's tenant. `since` is RFC3339; `decision` is one of
/// `"allow"|"deny"|"require_approval"`. `limit` is server-capped per tier;
/// over-cap requests get a 429 with the V1 upgrade envelope.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ListDecisionsOptions {
    pub since: Option<DateTime<Utc>>,
    pub decision: Option<String>,
    pub policy_id: Option<String>,
    pub tool_signature: Option<String>,
    pub limit: Option<u32>,
}

/// Pricing-tier upgrade context returned in a 429 envelope when the caller's
/// tier limits the operation. Mirrors the platform-side
/// `feedback_429_no_upgrade_hint_is_conversion_gap.md` contract.
#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq)]
pub struct UpgradeInfo {
    pub tier: String,
    pub wording: String,
    pub compare_url: String,
    pub buy_url: String,
}

/// Parsed body of a 429 response carrying a tier-cap envelope.
/// Surfaced via `AxonFlowError::RateLimited`.
#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq)]
pub struct RateLimitEnvelope {
    pub error: String,
    pub limit_type: String,
    pub tier: String,
    pub limit: u32,
    pub remaining: u32,
    pub upgrade: UpgradeInfo,
}
