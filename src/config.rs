use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    Production,
    Sandbox,
}

impl Default for Mode {
    fn default() -> Self {
        Self::Production
    }
}

#[derive(Debug, Clone)]
pub struct RetryConfig {
    pub enabled: bool,
    pub max_attempts: u32,
    pub initial_delay: Duration,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_attempts: 3,
            initial_delay: Duration::from_secs(1),
        }
    }
}

#[derive(Debug, Clone)]
pub struct CacheConfig {
    pub enabled: bool,
    pub ttl: Duration,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            ttl: Duration::from_secs(60),
        }
    }
}

#[derive(Debug, Clone)]
pub struct AxonFlowConfig {
    pub endpoint: String,
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
    pub mode: Mode,
    pub debug: bool,
    pub timeout: Duration,
    pub map_timeout: Duration,
    pub retry: RetryConfig,
    pub cache: CacheConfig,
    pub insecure_skip_tls_verify: bool,
}

impl Default for AxonFlowConfig {
    fn default() -> Self {
        Self {
            endpoint: String::new(),
            client_id: None,
            client_secret: None,
            mode: Mode::default(),
            debug: false,
            timeout: Duration::from_secs(60),
            map_timeout: Duration::from_secs(120),
            retry: RetryConfig::default(),
            cache: CacheConfig::default(),
            insecure_skip_tls_verify: false,
        }
    }
}

impl AxonFlowConfig {
    pub fn new(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            ..Default::default()
        }
    }

    pub fn with_auth(mut self, client_id: impl Into<String>, client_secret: impl Into<String>) -> Self {
        self.client_id = Some(client_id.into());
        self.client_secret = Some(client_secret.into());
        self
    }

    pub fn with_mode(mut self, mode: Mode) -> Self {
        self.mode = mode;
        self
    }
}
