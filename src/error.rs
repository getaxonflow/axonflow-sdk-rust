use crate::types::decisions::RateLimitEnvelope;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum AxonFlowError {
    #[error("HTTP request failed: {0}")]
    HttpError(#[from] reqwest::Error),
    #[error("Serialization/Deserialization failed: {0}")]
    SerdeError(#[from] serde_json::Error),
    #[error("API error ({status}): {message}")]
    ApiError { status: u16, message: String },
    /// Tier-cap 429 with a parsed V1 upgrade envelope. Distinct from a
    /// generic 429 ApiError because callers should branch on the upgrade
    /// fields (tier / compare_url / buy_url) without re-parsing the
    /// raw body. Mirrors the cross-SDK 429-with-envelope pattern
    /// (#1982 / #1958). Boxed to keep `AxonFlowError` small —
    /// `RateLimitEnvelope` is ~176 bytes and would dominate the enum
    /// otherwise (clippy::result_large_err).
    #[error("Rate limited (tier={}, limit_type={}): {}", .envelope.tier, .envelope.limit_type, .envelope.error)]
    RateLimited { envelope: Box<RateLimitEnvelope> },
    /// A role-scoped read whose answer was decided by the caller's identity
    /// scope rather than by the data (platform #2922).
    ///
    /// It exists because "no rows" and "no identity" are the same bytes on the
    /// wire. See [`ReadScopeRefusal`](crate::ReadScopeRefusal) for the two
    /// shapes and why an own-rows miss is NOT a claim that the row exists.
    ///
    /// Boxed for the same reason `RateLimited` is: it would otherwise dominate
    /// the enum's size (clippy::result_large_err).
    #[error("{0}")]
    ReadScope(Box<crate::read_identity::ReadScopeRefusal>),
    #[error("Configuration error: {0}")]
    ConfigError(String),
    #[error("AxonFlow platform is unavailable: {0}")]
    Unavailable(String),
    /// A Decision Mode `redact_pii` obligation could not be discharged through
    /// the engine — it named no request-phase fulfillment, advertised a
    /// content-type the PEP is not holding, named an endpoint this client will
    /// not call, the engine call failed / returned non-200, or the engine
    /// reported the redactor did not run (`redaction_evaluated=false`).
    ///
    /// This is the fail-closed signal of the PEP contract (ADR-056, #2563): the
    /// caller MUST block, never forward the unredacted content. There is NO code
    /// path in which the SDK redacts locally — fulfillment is always the engine
    /// round-trip — so an obligation the engine cannot discharge fails closed
    /// here rather than leaking PII.
    #[error("Obligation not engine-fulfillable: {0}")]
    ObligationNotFulfillable(String),
}

impl AxonFlowError {
    pub fn is_retryable(&self) -> bool {
        match self {
            AxonFlowError::HttpError(e) => e.is_timeout() || e.is_connect(),
            AxonFlowError::ApiError { status, .. } => *status >= 500 || *status == 429,
            AxonFlowError::RateLimited { .. } => true,
            AxonFlowError::Unavailable(_) => true,
            _ => false,
        }
    }

    /// Whether this error should trigger fail-open (return a synthetic success
    /// response). Currently identical to [`is_retryable`]; maintained as a
    /// separate method because future policy changes may diverge them (e.g.
    /// `ConfigError` could be fail-open-eligible but not retryable).
    pub fn is_fail_open_eligible(&self) -> bool {
        self.is_retryable()
    }
}

impl PartialEq for AxonFlowError {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::HttpError(a), Self::HttpError(b)) => {
                a.is_timeout() == b.is_timeout()
                    && a.is_connect() == b.is_connect()
                    && a.status() == b.status()
                    && a.to_string() == b.to_string()
            }
            (Self::SerdeError(a), Self::SerdeError(b)) => a.to_string() == b.to_string(),
            (
                Self::ApiError {
                    status: s1,
                    message: m1,
                },
                Self::ApiError {
                    status: s2,
                    message: m2,
                },
            ) => s1 == s2 && m1 == m2,
            (Self::RateLimited { envelope: e1 }, Self::RateLimited { envelope: e2 }) => e1 == e2,
            (Self::ConfigError(m1), Self::ConfigError(m2)) => m1 == m2,
            (Self::Unavailable(m1), Self::Unavailable(m2)) => m1 == m2,
            _ => false,
        }
    }
}
