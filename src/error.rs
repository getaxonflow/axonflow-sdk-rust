use thiserror::Error;

#[derive(Error, Debug)]
pub enum AxonFlowError {
    #[error("HTTP request failed: {0}")]
    HttpError(#[from] reqwest::Error),
    #[error("Serialization/Deserialization failed: {0}")]
    SerdeError(#[from] serde_json::Error),
    #[error("API error ({status}): {message}")]
    ApiError { status: u16, message: String },
    #[error("Configuration error: {0}")]
    ConfigError(String),
    #[error("AxonFlow platform is unavailable: {0}")]
    Unavailable(String),
}

impl AxonFlowError {
    pub fn is_retryable(&self) -> bool {
        match self {
            AxonFlowError::HttpError(e) => e.is_timeout() || e.is_connect(),
            AxonFlowError::ApiError { status, .. } => *status >= 500 || *status == 429,
            _ => false,
        }
    }
}
