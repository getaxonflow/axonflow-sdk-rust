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
use std::collections::HashMap;

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
/// * `decision` — canonical audit verdict `"allowed"` | `"blocked"` |
///   `"redacted"` | `"needs_approval"` | `"error"` (platform 9.0.0+; pre-9.0.0
///   used `"allow"` | `"deny"` | `"require_approval"`, see the v8 → v9 migration
///   guide <https://docs.getaxonflow.com/docs/deployment/v8-to-v9-migration/>).
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
/// * `context` — the FULL sanitized request context the PEP attached to the
///   decision (canonical `lower_snake_case` keys, string values), read from the
///   audit row's `policy_details->'context'`. Unlike [`DecisionSummary`] (which
///   the platform truncates to 5 keys), explain returns every persisted key up
///   to the 10-key cap (e.g. `x_ai_agent`, `x_session_id`, `x_leader_identity`,
///   `x-bukuwarung-*`). `None` for pre-v0.6.0 audit rows. (platform #2509)
/// * `context_truncated` — true when the agent dropped surplus context keys at
///   write time.
#[must_use]
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<HashMap<String, String>>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub context_truncated: bool,
}

/// serde `skip_serializing_if` helper: drop `context_truncated` when it is the
/// `false` default, matching the platform's `omitempty` wire shape.
fn is_false(b: &bool) -> bool {
    !*b
}

/// Slim summary returned by `AxonFlowClient::list_decisions`.
///
/// Matches the platform `GET /api/v1/decisions` contract: 5 fields.
///   `policy_id` and `tool_signature` are optional because pre-α1 audit rows
///   and dynamic-only blocks may not populate them. ADR-043 §"Versioning"
///   rules apply: additive `Option<>` fields are non-breaking.
///
/// Cross-SDK parity:
///   Go:     axonflow-sdk-go/decisions.go (DecisionSummary)
///   Python: axonflow-sdk-python/axonflow/decisions.py (DecisionSummary)
///   TS:     axonflow-sdk-typescript/src/types/decisions.ts (DecisionSummary)
///   Java:   axonflow-sdk-java/src/main/java/com/getaxonflow/sdk/types/DecisionSummary.java
#[must_use]
#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq)]
pub struct DecisionSummary {
    pub decision_id: String,
    pub timestamp: DateTime<Utc>,
    pub decision: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_signature: Option<String>,
    /// The sanitized request context the PEP attached to the decision (canonical
    /// `lower_snake_case` keys, string values), surfaced from the audit row's
    /// `policy_details->'context'`. The list summary is truncated by the
    /// platform to the 5 most-correlated keys; the full map is available via
    /// `AxonFlowClient::explain_decision`. `None` for pre-v0.6.0 audit rows or
    /// decisions with no context. (platform #2509 / epic #2508)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<HashMap<String, String>>,
}

/// Optional filters for `AxonFlowClient::list_decisions`.
///
/// Every field is optional — leaving all `None` returns the tier-default
/// page from the caller's tenant. `since` is RFC3339; `decision`, when set, is
/// one of the canonical audit verdicts
/// `"allowed"|"blocked"|"redacted"|"needs_approval"|"error"` (platform 9.0.0+);
/// the pre-9.0.0 values `"allow"|"deny"|"require_approval"` are rejected with
/// HTTP 400 by 9.0.0 (see the v8 → v9 migration guide
/// <https://docs.getaxonflow.com/docs/deployment/v8-to-v9-migration/>). `limit`
/// is server-capped per tier; over-cap requests get a 429 with the V1 upgrade envelope.
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

#[cfg(test)]
mod tests {
    use super::*;

    // v0.6.0 (platform #2509): request context surfaced on decision reads.

    #[test]
    fn summary_context_round_trips() {
        let json = r#"{
            "decision_id": "dec-ctx",
            "timestamp": "2026-05-30T12:00:00Z",
            "decision": "blocked",
            "context": {
                "x_ai_agent": "refund-bot",
                "x_session_id": "sess-42",
                "x_leader_identity": "ops-lead"
            }
        }"#;
        let summary: DecisionSummary = serde_json::from_str(json).unwrap();
        let ctx = summary.context.as_ref().expect("context present");
        assert_eq!(ctx.len(), 3);
        assert_eq!(
            ctx.get("x_ai_agent").map(String::as_str),
            Some("refund-bot")
        );

        // re-serialize -> re-parse without loss
        let back: DecisionSummary =
            serde_json::from_str(&serde_json::to_string(&summary).unwrap()).unwrap();
        assert_eq!(
            back.context
                .unwrap()
                .get("x_leader_identity")
                .map(String::as_str),
            Some("ops-lead")
        );
    }

    #[test]
    fn summary_context_absent_is_none_and_omitted() {
        let json = r#"{"decision_id":"dec-noctx","timestamp":"2026-05-30T12:00:00Z","decision":"allowed"}"#;
        let summary: DecisionSummary = serde_json::from_str(json).unwrap();
        assert!(summary.context.is_none());
        // omitted on the wire (skip_serializing_if), preserving pre-v0.6.0 byte-shape
        assert!(!serde_json::to_string(&summary).unwrap().contains("context"));
    }

    #[test]
    fn explanation_full_context_and_truncated_flag() {
        let json = r#"{
            "decision_id": "dec-x",
            "timestamp": "2026-05-30T12:00:00Z",
            "decision": "blocked",
            "reason": "pii",
            "policy_matches": [],
            "context": {"x_ai_agent": "a", "x_session_id": "s"},
            "context_truncated": true
        }"#;
        let exp: DecisionExplanation = serde_json::from_str(json).unwrap();
        assert_eq!(exp.context.as_ref().unwrap().len(), 2);
        assert!(exp.context_truncated);
        assert!(serde_json::to_string(&exp)
            .unwrap()
            .contains("\"context_truncated\":true"));
    }

    #[test]
    fn explanation_context_truncated_false_omitted() {
        let exp = DecisionExplanation {
            decision_id: "d".to_string(),
            decision: "allowed".to_string(),
            ..Default::default()
        };
        let json = serde_json::to_string(&exp).unwrap();
        assert!(!json.contains("context_truncated"));
        assert!(!json.contains("\"context\""));
    }
}
