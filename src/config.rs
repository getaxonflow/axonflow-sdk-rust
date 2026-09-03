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
    /// The per-user identity this client presents on the READ path, sent as the
    /// `X-User-Token` header on every request bound for the configured endpoint.
    ///
    /// `client_id`/`client_secret` authenticate the ORGANIZATION; this
    /// authenticates the PERSON. Since platform #2922 the role-scoped reads
    /// (`explain_decision`, `list_decisions`, the audit reads) are answered from
    /// this identity: an enterprise stack scopes a developer or viewer to their
    /// own rows, gives a tenant-wide role (admin / owner / policy_admin) the
    /// whole tenant, and returns ZERO rows to a caller that presents no identity
    /// at all — which the SDK now reports as
    /// [`AxonFlowError::ReadScope`](crate::AxonFlowError::ReadScope) rather than
    /// as an empty result.
    ///
    /// SETTING THIS AFFECTS MORE THAN READS. The header rides every request and
    /// the agent VALIDATES it on every route it proxies, so a stale or rotated
    /// token turns `list_connectors`, `install_connector` and policy CRUD into
    /// 401s rather than merely unscoping a read. Fail-closed is the right
    /// direction, but it puts this value in the same rotation story as
    /// `client_secret`.
    ///
    /// The value is a per-user JWT: minted by the customer portal's user-token
    /// API, or for local testing by `scripts/generate-jwt.sh --kind user`. It is
    /// NOT the tenant JWT and not `client_secret`. Community deployments are
    /// single-operator and ignore it.
    ///
    /// Override per call with the `*_as` read methods, or derive a client bound
    /// to one person with [`AxonFlowClient::as_user`](crate::AxonFlowClient::as_user).
    pub user_token: Option<String>,
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
            // The read-path identity is a per-user CREDENTIAL, redacted here
            // for the same reason as the other two: a config reaches log lines,
            // panic messages and debugger frames, and a credential that rides
            // along has left the process in every one of them.
            .field(
                "user_token",
                &self.user_token.as_ref().map(|_| "[REDACTED]"),
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
            user_token: None,
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
    /// on their first request — see `heartbeat::maybe_send_heartbeat_on_request`. Set `AXONFLOW_TELEMETRY=off`
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
