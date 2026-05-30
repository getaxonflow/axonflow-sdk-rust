use serde::{Deserialize, Deserializer, Serialize};
use std::collections::HashMap;

/// Deserialize helper: when the wire value is `null`, fall back to `T::default()`.
///
/// The platform sometimes serializes empty collections as `null` rather than
/// `[]`. `#[serde(default)]` only fires for missing fields, so without this
/// helper a payload containing `"policies_evaluated": null` would fail with
/// "invalid type: null, expected a sequence". Combine with `#[serde(default)]`
/// so both null AND missing are tolerated.
fn null_to_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    let opt = Option::<T>::deserialize(deserializer)?;
    Ok(opt.unwrap_or_default())
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct MediaContent {
    pub id: String,
    pub r#type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base64: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ClientRequest {
    pub query: String,
    pub user_token: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    pub request_type: String,
    #[serde(default)]
    pub context: HashMap<String, serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media: Option<Vec<MediaContent>>,
}

#[must_use]
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ClientResponse {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<HashMap<String, serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default)]
    pub blocked: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub block_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy_info: Option<PolicyEvaluationInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub budget_info: Option<BudgetInfo>,
}

impl ClientResponse {
    pub fn fail_open(error: crate::error::AxonFlowError) -> Self {
        Self {
            success: true,
            data: None,
            result: None,
            plan_id: None,
            request_id: None,
            metadata: None,
            error: Some(format!("AxonFlow unavailable (fail-open): {}", error)),
            blocked: false,
            block_reason: None,
            policy_info: None,
            budget_info: None,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct BudgetInfo {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub budget_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub budget_name: Option<String>,
    pub used_usd: f64,
    pub limit_usd: f64,
    pub percentage: f64,
    pub exceeded: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
#[serde(default)]
pub struct PolicyEvaluationInfo {
    #[serde(deserialize_with = "null_to_default")]
    pub policies_evaluated: Vec<String>,
    #[serde(deserialize_with = "null_to_default")]
    pub static_checks: Vec<String>,
    pub processing_time: String,
    pub tenant_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code_artifact: Option<CodeArtifact>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct TokenUsage {
    pub prompt_tokens: usize,
    pub completion_tokens: usize,
    pub total_tokens: usize,
}

#[derive(Debug, Clone)]
pub struct AuditRequest {
    pub context_id: String,
    pub response_summary: String,
    pub provider: String,
    pub model: String,
    pub token_usage: TokenUsage,
    pub latency_ms: i64,
    pub metadata: Option<HashMap<String, serde_json::Value>>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
#[serde(default)]
pub struct AuditResult {
    pub success: bool,
    pub audit_id: String,
}

/// A single audit log entry from the platform.
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
#[serde(default)]
pub struct AuditLogEntry {
    pub id: String,
    pub request_id: String,
    pub timestamp: String,
    pub user_email: String,
    pub client_id: String,
    pub tenant_id: String,
    pub request_type: String,
    pub query_summary: String,
    pub success: bool,
    pub blocked: bool,
    pub risk_score: f64,
    pub provider: String,
    pub model: String,
    pub tokens_used: i64,
    pub latency_ms: i64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub policy_violations: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<HashMap<String, serde_json::Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_residency: Option<String>,
    /// Cross-border transfer legal basis under Indonesia UU PDP Pasal 56:
    /// `adequacy`, `safeguards`, `pasal_56b_dpa`, or `consent`. Surfaced
    /// verbatim — never auto-translated. See the [`transfer_basis`] module
    /// constants for the recognized set. The field stays an `Option<String>`
    /// so the SDK never rejects a value a newer platform may add.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transfer_basis: Option<String>,
}

/// Cross-border transfer-basis values recognized under Indonesia UU PDP Pasal 56,
/// for [`AuditLogEntry::transfer_basis`]:
///
/// * [`ADEQUACY`](transfer_basis::ADEQUACY) — Pasal 56(a): destination with adequate protection
/// * [`SAFEGUARDS`](transfer_basis::SAFEGUARDS) — Pasal 56(b): binding legal instrument (generic label)
/// * [`PASAL_56B_DPA`](transfer_basis::PASAL_56B_DPA) — Pasal 56(b): binding legal instrument, explicit DPA tag
/// * [`CONSENT`](transfer_basis::CONSENT) — Pasal 56(c): explicit data-subject consent
///
/// `safeguards` and `pasal_56b_dpa` are semantic equivalents; the platform
/// surfaces whichever was recorded at decision time. (platform #2513 / epic #2508)
pub mod transfer_basis {
    pub const ADEQUACY: &str = "adequacy";
    pub const SAFEGUARDS: &str = "safeguards";
    pub const PASAL_56B_DPA: &str = "pasal_56b_dpa";
    pub const CONSENT: &str = "consent";
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
#[serde(default)]
pub struct ConnectorMetadata {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub r#type: String,
    pub version: String,
    pub description: String,
    pub category: String,
    pub icon: String,
    #[serde(deserialize_with = "null_to_default")]
    pub tags: Vec<String>,
    #[serde(deserialize_with = "null_to_default")]
    pub capabilities: Vec<String>,
    #[serde(deserialize_with = "null_to_default")]
    pub config_schema: HashMap<String, serde_json::Value>,
    pub installed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instance_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub healthy: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_check: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
#[serde(default)]
pub struct ConnectorHealthStatus {
    pub healthy: bool,
    pub latency: i64,
    #[serde(deserialize_with = "null_to_default")]
    pub details: HashMap<String, String>,
    pub timestamp: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ConnectorInstallRequest {
    pub connector_id: String,
    pub name: String,
    pub tenant_id: String,
    pub options: HashMap<String, serde_json::Value>,
    pub credentials: HashMap<String, String>,
}

#[must_use]
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ConnectorResponse {
    pub success: bool,
    pub data: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meta: Option<HashMap<String, serde_json::Value>>,
    #[serde(default)]
    pub redacted: bool,
    #[serde(default, deserialize_with = "null_to_default")]
    pub redacted_fields: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy_info: Option<PolicyInfo>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PolicyInfo {
    pub policies_evaluated: usize,
    pub blocked: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub block_reason: Option<String>,
    pub redactions_applied: usize,
    pub processing_time_ms: i64,
    #[serde(default, deserialize_with = "null_to_default")]
    pub matched_policies: Vec<PolicyMatchInfo>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PolicyMatchInfo {
    pub policy_id: String,
    pub policy_name: String,
    pub category: String,
    pub severity: String,
    pub action: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct PlanStep {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default, rename = "type")]
    pub r#type: String,
    #[serde(default)]
    pub description: String,
    #[serde(default, deserialize_with = "null_to_default")]
    pub dependencies: Vec<String>,
    #[serde(default)]
    pub agent: String,
    #[serde(default, deserialize_with = "null_to_default")]
    pub parameters: HashMap<String, serde_json::Value>,
    #[serde(default)]
    pub estimated_time: String,
}

#[must_use]
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
#[serde(default)]
pub struct PlanResponse {
    pub plan_id: String,
    pub status: String,
    #[serde(deserialize_with = "null_to_default")]
    pub steps: Vec<PlanStep>,
    pub domain: String,
    pub complexity: i32,
    pub parallel: bool,
    pub estimated_duration: String,
    #[serde(deserialize_with = "null_to_default")]
    pub metadata: HashMap<String, serde_json::Value>,
    pub success: bool,
    pub version: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
#[serde(default)]
pub struct StepResult {
    pub step_id: String,
    pub step_name: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub duration: String,
}

#[must_use]
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
#[serde(default)]
pub struct PlanExecutionResponse {
    pub plan_id: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workflow_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    #[serde(deserialize_with = "null_to_default")]
    pub step_results: Vec<StepResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub duration: String,
    pub completed_steps: i32,
    pub total_steps: i32,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
#[serde(default)]
pub struct CancelPlanResponse {
    pub plan_id: String,
    pub status: String,
    pub success: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CodeArtifact {
    pub is_code_output: bool,
    pub language: String,
    pub code_type: String,
    pub size_bytes: usize,
    pub line_count: usize,
    pub secrets_detected: usize,
    pub unsafe_patterns: usize,
    #[serde(default, deserialize_with = "null_to_default")]
    pub policies_checked: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audit_log_entry_with_residency_fields_deserializes() {
        let json = r#"{
            "id": "audit-001",
            "request_id": "req-123",
            "timestamp": "2026-05-26T00:00:00Z",
            "user_email": "user@example.com",
            "client_id": "client-1",
            "tenant_id": "tenant-1",
            "request_type": "llm_query",
            "query_summary": "test query",
            "success": true,
            "blocked": false,
            "risk_score": 0.15,
            "provider": "openai",
            "model": "gpt-4",
            "tokens_used": 500,
            "latency_ms": 120,
            "policy_violations": [],
            "data_residency": "id-jakarta",
            "transfer_basis": "pdp-consent"
        }"#;

        let entry: AuditLogEntry = serde_json::from_str(json).unwrap();
        assert_eq!(entry.id, "audit-001");
        assert_eq!(entry.data_residency.as_deref(), Some("id-jakarta"));
        assert_eq!(entry.transfer_basis.as_deref(), Some("pdp-consent"));
        assert!(entry.success);
        assert!(!entry.blocked);
        assert_eq!(entry.tokens_used, 500);
    }

    #[test]
    fn audit_log_entry_without_new_fields_deserializes_backward_compat() {
        let json = r#"{
            "id": "audit-002",
            "request_id": "req-456",
            "timestamp": "2026-05-26T00:00:00Z",
            "user_email": "user@example.com",
            "client_id": "client-1",
            "tenant_id": "tenant-1",
            "request_type": "llm_query",
            "query_summary": "test query",
            "success": true,
            "blocked": false,
            "risk_score": 0.0,
            "provider": "anthropic",
            "model": "claude-3",
            "tokens_used": 100,
            "latency_ms": 50
        }"#;

        let entry: AuditLogEntry = serde_json::from_str(json).unwrap();
        assert_eq!(entry.id, "audit-002");
        assert!(entry.data_residency.is_none());
        assert!(entry.transfer_basis.is_none());
        assert!(entry.policy_violations.is_empty());
        assert!(entry.metadata.is_none());
    }

    #[test]
    fn audit_log_entry_empty_optional_fields_omitted_in_serialization() {
        let entry = AuditLogEntry {
            id: "audit-003".to_string(),
            request_id: "req-789".to_string(),
            success: true,
            ..Default::default()
        };

        let json = serde_json::to_string(&entry).unwrap();
        // Option<String> fields set to None should be absent
        assert!(!json.contains("data_residency"));
        assert!(!json.contains("transfer_basis"));
        assert!(!json.contains("metadata"));
        // Empty Vec should be absent (skip_serializing_if = "Vec::is_empty")
        assert!(!json.contains("policy_violations"));
    }

    #[test]
    fn audit_log_entry_with_metadata_round_trips() {
        let mut meta = HashMap::new();
        meta.insert(
            "region".to_string(),
            serde_json::Value::String("ap-southeast-3".to_string()),
        );

        let entry = AuditLogEntry {
            id: "audit-004".to_string(),
            data_residency: Some("id-jakarta".to_string()),
            transfer_basis: Some("pdp-consent".to_string()),
            metadata: Some(meta),
            ..Default::default()
        };

        let json = serde_json::to_string(&entry).unwrap();
        let back: AuditLogEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(back.data_residency.as_deref(), Some("id-jakarta"));
        assert_eq!(back.transfer_basis.as_deref(), Some("pdp-consent"));
        assert!(back.metadata.is_some());
        assert_eq!(
            back.metadata.unwrap().get("region").unwrap(),
            &serde_json::Value::String("ap-southeast-3".to_string())
        );
    }

    // v0.6.0 (platform #2513): pasal_56b_dpa accepted; existing values kept.

    #[test]
    fn transfer_basis_constants_wire_values() {
        assert_eq!(transfer_basis::ADEQUACY, "adequacy");
        assert_eq!(transfer_basis::SAFEGUARDS, "safeguards");
        assert_eq!(transfer_basis::PASAL_56B_DPA, "pasal_56b_dpa");
        assert_eq!(transfer_basis::CONSENT, "consent");
    }

    #[test]
    fn audit_log_entry_pasal_56b_dpa_round_trips_verbatim() {
        let json = r#"{
            "id": "aud-56b",
            "timestamp": "2026-05-30T10:00:00Z",
            "data_residency": "ID",
            "transfer_basis": "pasal_56b_dpa"
        }"#;
        let entry: AuditLogEntry = serde_json::from_str(json).unwrap();
        assert_eq!(
            entry.transfer_basis.as_deref(),
            Some(transfer_basis::PASAL_56B_DPA)
        );

        // never auto-translated to "safeguards"
        let back: AuditLogEntry =
            serde_json::from_str(&serde_json::to_string(&entry).unwrap()).unwrap();
        assert_eq!(back.transfer_basis.as_deref(), Some("pasal_56b_dpa"));
    }

    #[test]
    fn audit_log_entry_safeguards_backward_compat() {
        // Existing v0.5.0-shaped rows using "safeguards" are unaffected by the widening.
        let json =
            r#"{"id":"aud-sg","timestamp":"2026-05-26T10:00:00Z","transfer_basis":"safeguards"}"#;
        let entry: AuditLogEntry = serde_json::from_str(json).unwrap();
        assert_eq!(
            entry.transfer_basis.as_deref(),
            Some(transfer_basis::SAFEGUARDS)
        );
    }
}
