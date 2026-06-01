pub mod client;
pub mod config;
pub mod decisions;
pub mod error;
pub mod heartbeat;
pub mod hitl;
pub mod interceptors;
pub mod types;

use percent_encoding::{AsciiSet, CONTROLS};

// Path-segment encode set: mirrors Go's `url.PathEscape` semantics so
// percent-encoding parity holds across SDKs. Keeps RFC 3986 unreserved
// characters (alphanum, `-`, `.`, `_`, `~`) unencoded; escapes path-
// significant chars (`/`, `?`, `#`, `%`) plus controls and characters
// that web infra commonly rejects (` "<>``\\{}`).
//
// Replaces the previous `NON_ALPHANUMERIC` usage which over-escaped
// underscores and dashes — observable as `dec_wf1_step2` becoming
// `dec%5Fwf1%5Fstep2` in the explain path, and `amadeus-travel`
// becoming `amadeus%2Dtravel` for connector lookups. Gorilla mux
// percent-decodes path segments so the platform happened to tolerate
// the over-escaped form, but the wire was wrong and any stricter
// router would 404. Found while wiring `decisions::explain_decision`.
pub(crate) const PATH_SEGMENT: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'<')
    .add(b'>')
    .add(b'`')
    .add(b'\\')
    .add(b'{')
    .add(b'}')
    .add(b'#')
    .add(b'?')
    .add(b'/')
    .add(b'%');

pub use client::AxonFlowClient;
pub use config::{AxonFlowConfig, CacheConfig, Mode, RetryConfig};
pub use error::AxonFlowError;
pub use types::agent::transfer_basis;
pub use types::agent::{
    AuditLogEntry, AuditRequest, AuditResult, BudgetInfo, CancelPlanResponse, ClientRequest,
    ClientResponse, CodeArtifact, ConnectorHealthStatus, ConnectorInstallRequest,
    ConnectorMetadata, ConnectorResponse, MediaContent, PlanExecutionResponse, PlanResponse,
    PlanStep, PolicyEvaluationInfo, PolicyInfo, PolicyMatchInfo, StepResult, TokenUsage,
};
pub use types::decisions::{
    DecisionExplanation, DecisionSummary, ExplainPolicy, ExplainRule, ListDecisionsOptions,
    RateLimitEnvelope, UpgradeInfo,
};
pub use types::hitl::{
    HITLApprovalRequest, HITLCreateInput, HITLQueueListOptions, HITLQueueListResponse,
    HITLReviewInput, HITLStats,
};
pub use types::policies::PolicyCategory;
