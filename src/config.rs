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
    /// Whether retries are enabled. Note: even when `true`, mutations
    /// (`execute-plan`, `generate-plan`, `cancel-plan`, `update-plan`) are
    /// never retried to avoid double-execution.
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
    /// Maximum number of entries in the cache. When the cache reaches this
    /// size, the least recently used entry is evicted. Defaults to 10,000.
    pub max_capacity: u64,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            ttl: Duration::from_secs(60),
            max_capacity: 10_000,
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn retry_config_default_matches_docs() {
        let retry = RetryConfig::default();
        assert!(retry.enabled);
        assert_eq!(retry.max_attempts, 3);
        assert_eq!(retry.initial_delay, Duration::from_secs(1));
    }

    #[test]
    fn cache_config_default_matches_docs() {
        let cache = CacheConfig::default();
        assert!(cache.enabled);
        assert_eq!(cache.ttl, Duration::from_secs(60));
        assert_eq!(cache.max_capacity, 10_000);
    }

    #[test]
    fn axonflow_config_default_and_new() {
        let default = AxonFlowConfig::default();
        assert_eq!(default.endpoint, "");
        assert_eq!(default.client_id, None);
        assert_eq!(default.client_secret, None);
        assert_eq!(default.license_key, None);
        assert_eq!(default.mode, Mode::Production);
        assert!(!default.debug);
        assert_eq!(default.timeout, Duration::from_secs(60));
        assert_eq!(default.map_timeout, Duration::from_secs(120));
        assert!(!default.insecure_skip_tls_verify);
        assert_eq!(default.retry.max_attempts, 3);
        assert_eq!(default.cache.max_capacity, 10_000);

        let cfg = AxonFlowConfig::new("https://api.example.com");
        assert_eq!(cfg.endpoint, "https://api.example.com");
        assert_eq!(cfg.mode, Mode::Production);
        assert_eq!(cfg.timeout, Duration::from_secs(60));
        assert_eq!(cfg.retry.max_attempts, 3);
        assert_eq!(cfg.cache.ttl, Duration::from_secs(60));
    }

    #[test]
    fn sandbox_constructor_sets_local_defaults() {
        let cfg = AxonFlowConfig::sandbox("cid", "csecret");
        assert_eq!(cfg.endpoint, "http://localhost:8080");
        assert_eq!(cfg.client_id.as_deref(), Some("cid"));
        assert_eq!(cfg.client_secret.as_deref(), Some("csecret"));
        assert_eq!(cfg.mode, Mode::Sandbox);
        assert!(cfg.debug);
        // remaining fields stay at Default
        assert_eq!(cfg.timeout, Duration::from_secs(60));
        assert_eq!(cfg.license_key, None);
    }

    #[test]
    fn builder_methods_set_only_their_fields() {
        let base = AxonFlowConfig::new("https://api.example.com");

        let with_auth = base.clone().with_auth("id", "secret");
        assert_eq!(with_auth.client_id.as_deref(), Some("id"));
        assert_eq!(with_auth.client_secret.as_deref(), Some("secret"));
        assert_eq!(with_auth.endpoint, "https://api.example.com");
        assert_eq!(with_auth.mode, Mode::Production);
        assert_eq!(with_auth.license_key, None);

        let with_key = base.clone().with_license_key("lic-123");
        assert_eq!(with_key.license_key.as_deref(), Some("lic-123"));
        assert_eq!(with_key.client_id, None);
        assert_eq!(with_key.endpoint, "https://api.example.com");

        let with_mode = base.clone().with_mode(Mode::Sandbox);
        assert_eq!(with_mode.mode, Mode::Sandbox);
        assert_eq!(with_mode.endpoint, "https://api.example.com");
        assert_eq!(with_mode.client_id, None);
    }
}