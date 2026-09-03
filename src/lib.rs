pub mod authzen;
pub mod client;
pub mod config;
pub mod decisions;
pub mod error;
pub mod heartbeat;
pub mod hitl;
pub mod interceptors;
pub mod pep;
pub mod read_identity;
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

// `Attribute`, `AttributeMap` and `AttributeValue` are deliberately NOT
// re-exported at the crate root. They are three maximally generic names, and
// claiming them at the top level of a governance SDK for one surface's attribute
// bag is a permanent commitment nobody would make on purpose. They live at
// `axonflow_sdk_rust::authzen::{Attribute, ...}`.
pub use authzen::{
    AuthZenAction, AuthZenApprovalClause, AuthZenApprovalRequirement, AuthZenBulk, AuthZenCategory,
    AuthZenDecision, AuthZenEnvelope, AuthZenError, AuthZenErrorCode, AuthZenEvaluationError,
    AuthZenIdentifier, AuthZenIdentifierKind, AuthZenObligation, AuthZenObligationType,
    AuthZenOperationalState, AuthZenReasonCode, AuthZenRequest, AuthZenResource, AuthZenResponse,
    AuthZenResponseContext, AuthZenSubject, AUTHZEN_CONTRACT_SCHEMA_VERSION, AUTHZEN_PATH,
    AUTHZEN_PROFILE_HEADER, AUTHZEN_PROFILE_V1,
};
pub use client::AxonFlowClient;
// Telemetry: declaring a framework adapter on the existing heartbeat. Performs
// no I/O — see `heartbeat::register_adapter`.
pub use config::{AxonFlowConfig, CacheConfig, Mode, RetryConfig};
pub use error::AxonFlowError;
pub use heartbeat::register_adapter;
pub use pep::{
    has_request_redaction, CONTENT_TYPE_TEXT, DECIDE_PATH, GATEWAY_CONNECTOR_TAG,
    OBLIGATION_REDACT_PII, PHASE_REQUEST, PHASE_RESPONSE, REQUEST_REDACTION_PATH,
    RESPONSE_REDACTION_PATH, VERDICT_ALLOW, VERDICT_DENY, VERDICT_NEEDS_APPROVAL,
};
pub use read_identity::{ReadScope, ReadScopeRefusal, HEADER_READ_SCOPE, HEADER_USER_TOKEN};
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
pub use types::pep::{
    DecideRequest, DecideResponse, DecisionCallerIdentity, DecisionTarget, MCPCheckInputRequest,
    MCPCheckInputResponse, MCPCheckOutputRequest, MCPCheckOutputResponse, Obligation,
    ObligationFulfillment,
};
pub use types::policies::PolicyCategory;
