use serde::{Deserialize, Serialize};
use std::collections::HashMap;

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

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PolicyEvaluationInfo {
    #[serde(default)]
    pub policies_evaluated: Vec<String>,
    #[serde(default)]
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

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AuditResult {
    pub success: bool,
    pub audit_id: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ConnectorMetadata {
    pub id: String,
    pub name: String,
    pub r#type: String,
    pub version: String,
    pub description: String,
    pub category: String,
    pub icon: String,
    pub tags: Vec<String>,
    pub capabilities: Vec<String>,
    pub config_schema: HashMap<String, serde_json::Value>,
    pub installed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instance_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub healthy: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_check: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ConnectorHealthStatus {
    pub healthy: bool,
    pub latency: i64,
    pub details: HashMap<String, String>,
    pub timestamp: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
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
    #[serde(default)]
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
    #[serde(default)]
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

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PlanStep {
    pub id: String,
    pub name: String,
    pub r#type: String,
    pub description: String,
    pub dependencies: Vec<String>,
    pub agent: String,
    pub parameters: HashMap<String, serde_json::Value>,
    pub estimated_time: String,
}

#[must_use]
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PlanResponse {
    pub plan_id: String,
    pub status: String,
    pub steps: Vec<PlanStep>,
    pub domain: String,
    pub complexity: i32,
    pub parallel: bool,
    pub estimated_duration: String,
    pub metadata: HashMap<String, serde_json::Value>,
    #[serde(default)]
    pub success: bool,
    #[serde(default)]
    pub version: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
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
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PlanExecutionResponse {
    pub plan_id: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workflow_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    #[serde(default)]
    pub step_results: Vec<StepResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub duration: String,
    pub completed_steps: i32,
    pub total_steps: i32,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CancelPlanResponse {
    pub plan_id: String,
    pub status: String,
    #[serde(default)]
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
    #[serde(default)]
    pub policies_checked: Vec<String>,
}
