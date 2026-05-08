use std::fmt;
use std::time::Duration;

#[derive(Default, Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Production,
    Sandbox,
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

#[derive(Clone)]
pub struct AxonFlowConfig {
    pub endpoint: String,
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
    pub license_key: Option<String>,
    pub mode: Mode,
    pub debug: bool,
    pub timeout: Duration,
    pub map_timeout: Duration,
    pub retry: RetryConfig,
    pub cache: CacheConfig,
    pub insecure_skip_tls_verify: bool,
}

impl fmt::Debug for AxonFlowConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AxonFlowConfig")
            .field("endpoint", &self.endpoint)
            .field("client_id", &self.client_id)
            .field(
                "client_secret",
                &self.client_secret.as_ref().map(|_| "[REDACTED]"),
            )
            .field(
                "license_key",
                &self.license_key.as_ref().map(|_| "[REDACTED]"),
            )
            .field("mode", &self.mode)
            .field("debug", &self.debug)
            .field("timeout", &self.timeout)
            .field("map_timeout", &self.map_timeout)
            .field("retry", &self.retry)
            .field("cache", &self.cache)
            .field("insecure_skip_tls_verify", &self.insecure_skip_tls_verify)
            .finish()
    }
}

impl Default for AxonFlowConfig {
    fn default() -> Self {
        Self {
            endpoint: String::new(),
            client_id: None,
            client_secret: None,
            license_key: None,
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

    /// Convenience constructor for local development. Defaults to
    /// `http://localhost:8080` (the docker-compose agent default), sets
    /// `mode = Mode::Sandbox`, and enables debug logging.
    ///
    /// Sandbox-mode clients fire anonymous telemetry tagged `stream="sandbox"`
    /// — see `heartbeat::maybe_send_heartbeat`. Set `AXONFLOW_TELEMETRY=off`
    /// to opt out (the SOLE opt-out lever; there is intentionally no
    /// programmatic disable on the SDK config).
    pub fn sandbox(client_id: impl Into<String>, client_secret: impl Into<String>) -> Self {
        Self {
            endpoint: "http://localhost:8080".to_string(),
            client_id: Some(client_id.into()),
            client_secret: Some(client_secret.into()),
            mode: Mode::Sandbox,
            debug: true,
            ..Default::default()
        }
    }

    pub fn with_auth(
        mut self,
        client_id: impl Into<String>,
        client_secret: impl Into<String>,
    ) -> Self {
        self.client_id = Some(client_id.into());
        self.client_secret = Some(client_secret.into());
        self
    }

    pub fn with_license_key(mut self, license_key: impl Into<String>) -> Self {
        self.license_key = Some(license_key.into());
        self
    }

    pub fn with_mode(mut self, mode: Mode) -> Self {
        self.mode = mode;
        self
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn with_map_timeout(mut self, timeout: Duration) -> Self {
        self.map_timeout = timeout;
        self
    }

    pub fn with_retry(mut self, retry: RetryConfig) -> Self {
        self.retry = retry;
        self
    }

    pub fn with_cache(mut self, cache: CacheConfig) -> Self {
        self.cache = cache;
        self
    }
}
