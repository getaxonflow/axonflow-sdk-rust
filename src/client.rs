use crate::config::{AxonFlowConfig, Mode};
use crate::error::AxonFlowError;
use crate::heartbeat::maybe_send_heartbeat;
use crate::types::agent::{ClientRequest, ClientResponse};
use crate::PATH_SEGMENT;
use base64::engine::general_purpose::STANDARD as BASE64_STD;
use base64::Engine as _;
use moka::future::Cache;
use percent_encoding::utf8_percent_encode;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, warn};

const LICENSE_KEY_HEADER: &str = "X-License-Key";

#[derive(Clone)]
pub struct AxonFlowClient {
    config: AxonFlowConfig,
    http_client: reqwest::Client,
    map_http_client: reqwest::Client,
    cache: Option<Arc<Cache<String, ClientResponse>>>,
}

impl AxonFlowClient {
    pub fn new(mut config: AxonFlowConfig) -> Result<Self, AxonFlowError> {
        if config.retry.max_attempts == 0 {
            return Err(AxonFlowError::ConfigError(
                "retry.max_attempts must be at least 1".to_string(),
            ));
        }

        if std::env::var("AXONFLOW_TRY").unwrap_or_default() == "1" {
            config.endpoint = "https://try.getaxonflow.com".to_string();
            if config.client_id.is_none() {
                return Err(AxonFlowError::ConfigError(
                    "ClientID is required in try mode (AXONFLOW_TRY=1).".to_string(),
                ));
            }
        }

        if config.client_secret.is_some() && config.client_id.is_none() {
            warn!("ClientID is required when ClientSecret is set.");
        }

        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(
            "User-Agent",
            HeaderValue::from_static(concat!("axonflow-sdk-rust/", env!("CARGO_PKG_VERSION"))),
        );
        // ADR-050 §4: every governed request to the agent carries
        // X-Axonflow-Client so the agent can derive request scope (sdk)
        // and validate against the token's aud.scope via HasScope().
        // Sourced from CARGO_PKG_VERSION; no env override (the consumer
        // doesn't get to spoof its own client identity to the agent).
        headers.insert(
            "X-Axonflow-Client",
            HeaderValue::from_static(concat!("sdk-rust/", env!("CARGO_PKG_VERSION"))),
        );

        // HTTP Basic auth: "Basic base64(client_id:client_secret)".
        // When neither is configured, default to the community tenant —
        // matches the cross-SDK contract (see axonflow-sdk-go selfhosted_auth_headers_test.go).
        let basic_id = config.client_id.as_deref().unwrap_or("community");
        let basic_secret = config.client_secret.as_deref().unwrap_or("");
        let basic_credentials = BASE64_STD.encode(format!("{basic_id}:{basic_secret}"));
        let basic_value = format!("Basic {}", basic_credentials);
        if let Ok(val) = HeaderValue::from_str(&basic_value) {
            headers.insert(AUTHORIZATION, val);
        }

        // X-Client-ID (v9): server-side identity decisions don't have to
        // re-decode Basic auth. The agent's apiAuthMiddleware overwrites
        // the header with its auth-derived value, so caller-supplied
        // values are harmless (no spoofing surface).
        if let Ok(val) = HeaderValue::from_str(basic_id) {
            headers.insert("X-Client-ID", val);
        }

        // Enterprise license key — sent only when configured.
        if let Some(license_key) = &config.license_key {
            if let Ok(mut val) = HeaderValue::from_str(license_key) {
                val.set_sensitive(true);
                headers.insert(LICENSE_KEY_HEADER, val);
            }
        }

        let accept_invalid = config.insecure_skip_tls_verify
            || std::env::var("AXONFLOW_INSECURE_TLS").unwrap_or_default() == "1";

        if accept_invalid {
            warn!("TLS certificate verification is disabled.");
        }

        let http_client = reqwest::Client::builder()
            .timeout(config.timeout)
            .redirect(Self::redirect_policy(&config.endpoint))
            .default_headers(headers.clone())
            .danger_accept_invalid_certs(accept_invalid)
            .pool_max_idle_per_host(5)
            .tcp_keepalive(Duration::from_secs(30))
            .build()
            .map_err(AxonFlowError::HttpError)?;

        let map_http_client = reqwest::Client::builder()
            .timeout(config.map_timeout)
            .redirect(Self::redirect_policy(&config.endpoint))
            .default_headers(headers)
            .danger_accept_invalid_certs(accept_invalid)
            .pool_max_idle_per_host(5)
            .tcp_keepalive(Duration::from_secs(30))
            .build()
            .map_err(AxonFlowError::HttpError)?;

        let cache = if config.cache.enabled {
            Some(Arc::new(
                Cache::builder()
                    .time_to_live(config.cache.ttl)
                    .max_capacity(config.cache.max_capacity)
                    .build(),
            ))
        } else {
            None
        };

        maybe_send_heartbeat(&config.endpoint, &config.mode);

        Ok(Self {
            config,
            http_client,
            map_http_client,
            cache,
        })
    }

    #[tracing::instrument(skip(self, context))]
    pub async fn proxy_llm_call(
        &self,
        user_token: &str,
        query: &str,
        request_type: &str,
        context: HashMap<String, serde_json::Value>,
    ) -> Result<ClientResponse, AxonFlowError> {
        let user_token = if user_token.is_empty() {
            "anonymous"
        } else {
            user_token
        };

        let is_mutation = matches!(
            request_type,
            "execute-plan" | "generate-plan" | "cancel-plan" | "update-plan"
        );

        if !is_mutation {
            if let Some(cache) = &self.cache {
                let cache_key = self.build_cache_key(request_type, query, user_token, &context);
                if let Some(cached) = cache.get(&cache_key).await {
                    debug!("Cache hit for query");
                    return Ok(cached);
                }
            }
        }

        let req = ClientRequest {
            query: query.to_string(),
            user_token: user_token.to_string(),
            client_id: self.config.client_id.clone(),
            request_type: request_type.to_string(),
            context,
            media: None,
        };

        let resp = if self.config.retry.enabled && !is_mutation {
            self.execute_with_retry(&req).await
        } else {
            self.execute_request(&req).await
        };

        match resp {
            Ok(response) => {
                if response.success && !is_mutation {
                    if let Some(cache) = &self.cache {
                        let cache_key =
                            self.build_cache_key(request_type, query, user_token, &req.context);
                        cache.insert(cache_key, response.clone()).await;
                    }
                }
                Ok(response)
            }
            Err(e) => {
                if self.config.mode == Mode::Production && e.is_fail_open_eligible() {
                    debug!("AxonFlow unavailable, failing open: {}", e);
                    Ok(ClientResponse::fail_open(e))
                } else {
                    Err(e)
                }
            }
        }
    }

    // ============================================================================
    // MCP Connector Management
    // ============================================================================

    pub async fn list_connectors(
        &self,
    ) -> Result<Vec<crate::types::agent::ConnectorMetadata>, AxonFlowError> {
        let url = format!("{}/api/v1/connectors", self.config.endpoint);
        let resp = self.checked_get(&url).await?;

        let body: serde_json::Value = resp.json().await?;
        let connectors = body["connectors"]
            .as_array()
            .ok_or_else(|| AxonFlowError::ApiError {
                status: 200,
                message: "response missing 'connectors' field".to_string(),
            })?;

        let result = serde_json::from_value(serde_json::Value::Array(connectors.clone()))?;
        Ok(result)
    }

    pub async fn get_connector(
        &self,
        connector_id: &str,
    ) -> Result<crate::types::agent::ConnectorMetadata, AxonFlowError> {
        let encoded_id = utf8_percent_encode(connector_id, PATH_SEGMENT);
        let url = format!("{}/api/v1/connectors/{}", self.config.endpoint, encoded_id);
        let resp = self.checked_get(&url).await?;
        Ok(resp.json().await?)
    }

    pub async fn get_connector_health(
        &self,
        connector_id: &str,
    ) -> Result<crate::types::agent::ConnectorHealthStatus, AxonFlowError> {
        let encoded_id = utf8_percent_encode(connector_id, PATH_SEGMENT);
        let url = format!(
            "{}/api/v1/connectors/{}/health",
            self.config.endpoint, encoded_id
        );
        let resp = self.checked_get(&url).await?;
        Ok(resp.json().await?)
    }

    pub async fn install_connector(
        &self,
        req: crate::types::agent::ConnectorInstallRequest,
    ) -> Result<(), AxonFlowError> {
        let encoded_id = utf8_percent_encode(&req.connector_id, PATH_SEGMENT);
        let url = format!(
            "{}/api/v1/connectors/{}/install",
            self.config.endpoint, encoded_id
        );
        let resp = self
            .dispatch(self.http_client.post(&url).json(&req), None)
            .await?;
        Self::check_status(resp).await?;
        Ok(())
    }

    pub async fn query_connector(
        &self,
        user_token: &str,
        connector_name: &str,
        query: &str,
        params: HashMap<String, serde_json::Value>,
    ) -> Result<crate::types::agent::ConnectorResponse, AxonFlowError> {
        // Connector queries are dispatched through the agent's proxy endpoint
        // with request_type=mcp-query — there is no standalone /api/v1/query.
        // Mirror the Go SDK's QueryConnector contract.
        let mut context = HashMap::new();
        context.insert("connector".to_string(), serde_json::json!(connector_name));
        context.insert("params".to_string(), serde_json::json!(params));

        let resp = self
            .proxy_llm_call(user_token, query, "mcp-query", context)
            .await?;

        Ok(crate::types::agent::ConnectorResponse {
            success: resp.success,
            data: resp.data.unwrap_or(serde_json::Value::Null),
            error: resp.error,
            meta: resp.metadata,
            redacted: false,
            redacted_fields: Vec::new(),
            policy_info: None,
        })
    }

    // ============================================================================
    // Multi-Agent Planning (MAP)
    // ============================================================================

    #[tracing::instrument(skip(self))]
    pub async fn generate_plan(
        &self,
        query: &str,
        domain: &str,
        user_token: Option<&str>,
    ) -> Result<crate::types::agent::PlanResponse, AxonFlowError> {
        let mut context = HashMap::new();
        context.insert("domain".to_string(), serde_json::json!(domain));
        let user_token = user_token.unwrap_or("anonymous");

        let resp = self
            .proxy_llm_call(user_token, query, "generate-plan", context)
            .await?;

        if let Some(data) = resp.data {
            let plan: crate::types::agent::PlanResponse = serde_json::from_value(data)?;
            Ok(plan)
        } else {
            Err(AxonFlowError::ApiError {
                status: 500,
                message: "empty plan data".to_string(),
            })
        }
    }

    pub async fn execute_plan(
        &self,
        plan_id: &str,
        user_token: Option<&str>,
    ) -> Result<crate::types::agent::PlanExecutionResponse, AxonFlowError> {
        let mut context = HashMap::new();
        context.insert("plan_id".to_string(), serde_json::json!(plan_id));
        let user_token = user_token.unwrap_or("anonymous");

        let resp = self
            .proxy_llm_call(user_token, "", "execute-plan", context)
            .await?;

        if let Some(data) = resp.data {
            let mut exec: crate::types::agent::PlanExecutionResponse =
                serde_json::from_value(data)?;
            // The execute-plan wire payload carries no `status` field (only
            // metadata/plan_id), so default it from the envelope verdict —
            // gated on `success`: a policy-blocked or failed execution must
            // never read as "completed" (enterprise#2861 sweep, R3).
            if exec.status.is_empty() {
                exec.status = if resp.success && !resp.blocked {
                    "completed".to_string()
                } else {
                    "failed".to_string()
                };
            }
            if exec.error.is_none() {
                exec.error = resp.error;
            }
            Ok(exec)
        } else {
            Err(AxonFlowError::ApiError {
                status: 500,
                message: "empty execution data".to_string(),
            })
        }
    }

    pub async fn get_plan_status(
        &self,
        plan_id: &str,
    ) -> Result<crate::types::agent::PlanExecutionResponse, AxonFlowError> {
        let encoded_id = utf8_percent_encode(plan_id, PATH_SEGMENT);
        let url = format!("{}/api/v1/plan/{}", self.config.endpoint, encoded_id);
        let resp = self.checked_map_get(&url).await?;
        Ok(resp.json().await?)
    }

    pub async fn cancel_plan(
        &self,
        plan_id: &str,
        reason: Option<&str>,
    ) -> Result<crate::types::agent::CancelPlanResponse, AxonFlowError> {
        let req_body = serde_json::json!({
            "reason": reason.unwrap_or("user_cancelled"),
        });

        let encoded_id = utf8_percent_encode(plan_id, PATH_SEGMENT);
        let url = format!("{}/api/v1/plan/{}/cancel", self.config.endpoint, encoded_id);
        let resp = self
            .dispatch(self.map_http_client.post(&url).json(&req_body), None)
            .await?;
        let resp = Self::check_status(resp).await?;
        Ok(resp.json().await?)
    }

    pub async fn audit_llm_call(
        &self,
        req: &crate::types::agent::AuditRequest,
    ) -> Result<crate::types::agent::AuditResult, AxonFlowError> {
        let client_id = self.get_effective_client_id();

        let mut req_body = serde_json::to_value(req)?;
        req_body["client_id"] = serde_json::json!(client_id);
        // Platform expects "metadata": {} when absent, not null.
        if req_body.get("metadata").map_or(true, |v| v.is_null()) {
            req_body["metadata"] = serde_json::json!({});
        }

        let url = format!("{}/api/audit/llm-call", self.config.endpoint);
        let resp = self
            .dispatch(self.http_client.post(&url).json(&req_body), None)
            .await?;

        let status = resp.status();
        let body = resp.text().await?;

        if status.is_success() {
            let audit_resp: crate::types::agent::AuditResult = serde_json::from_str(&body)?;
            Ok(audit_resp)
        } else {
            Err(AxonFlowError::ApiError {
                status: status.as_u16(),
                message: body,
            })
        }
    }

    // ============================================================================
    // Private helpers
    // ============================================================================

    fn get_effective_client_id(&self) -> String {
        self.config
            .client_id
            .clone()
            .unwrap_or_else(|| "community".to_string())
    }

    fn build_cache_key(
        &self,
        request_type: &str,
        query: &str,
        user_token: &str,
        context: &HashMap<String, serde_json::Value>,
    ) -> String {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        request_type.hash(&mut hasher);
        query.hash(&mut hasher);
        user_token.hash(&mut hasher);
        // The READ-PATH identity, which is a different thing from the
        // `user_token` argument above: that one is the write-path BODY field
        // this call was made with, and this one is the `X-User-Token` header
        // the request will carry.
        //
        // It has to be in the key because a derived client SHARES the parent's
        // cache (`as_user` clones the `Arc`) while presenting a different
        // identity. Without it, `base.as_user(ALICE)` and `base.as_user(BOB)`
        // making the same call hash to the same entry, so exactly ONE request
        // is sent — carrying ALICE — and BOB is handed ALICE's governed
        // response out of the cache. Two independent clients send two requests
        // and never see it; only the derived-client path collides, which is
        // the path `as_user` exists to make safe.
        //
        // Hashed, never stored: the key is the digest, so a cache dump cannot
        // yield the credential.
        self.config
            .user_token
            .as_deref()
            .unwrap_or("")
            .hash(&mut hasher);
        if !context.is_empty() {
            let sorted: std::collections::BTreeMap<_, _> = context.iter().collect();
            serde_json::to_string(&sorted)
                .unwrap_or_default()
                .hash(&mut hasher);
        }
        format!("{:x}", hasher.finish())
    }

    /// Endpoint URL the client is configured against.
    /// Crate-internal accessor for sibling modules (e.g. `decisions.rs`)
    /// that need to build absolute URLs without exposing `config`.
    pub(crate) fn endpoint(&self) -> &str {
        &self.config.endpoint
    }

    /// A client identical to this one but presenting `user_token`.
    ///
    /// The shape to reach for when one process acts on behalf of several people
    /// — a gateway, a bot. Unlike the `*_as` read methods, which only the reads
    /// have, this reaches EVERY method: there is no carve-out to remember and no
    /// path on which the identity silently widens back to the process's own.
    ///
    /// ```no_run
    /// # use axonflow_sdk_rust::{AxonFlowClient, AxonFlowConfig, ListDecisionsOptions};
    /// # async fn run() -> Result<(), Box<dyn std::error::Error>> {
    /// # let client = AxonFlowClient::new(AxonFlowConfig::new("http://localhost:8080"))?;
    /// let for_alice = client.as_user("alice-token");
    /// let rows = for_alice.list_decisions(ListDecisionsOptions::default()).await?;
    /// # Ok(()) }
    /// ```
    ///
    /// The returned client SHARES this one's `reqwest::Client`s and cache —
    /// `reqwest::Client` is an `Arc` internally, so cloning it shares the
    /// connection pool, the redirect policy and every TLS and proxy setting.
    /// Deriving one per request is therefore cheap, and the derivation cannot
    /// quietly lose an egress proxy. Only the identity differs; this client is
    /// not modified.
    ///
    /// Sharing the transport is safe here in a way it was NOT in the Python
    /// sibling: the identity is resolved from `self.config` at request time by
    /// `stamp_identity`, not captured in a hook bound to the client that built
    /// it. A derived client therefore reads its OWN token by construction.
    ///
    /// An empty token returns a client presenting no identity at all, which on
    /// an enterprise stack reads nothing (see
    /// [`ReadScope::None`](crate::ReadScope::None)).
    pub fn as_user(&self, user_token: impl Into<String>) -> Self {
        let token = user_token.into();
        let trimmed = token.trim();
        let mut config = self.config.clone();
        config.user_token = if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        };
        Self {
            config,
            http_client: self.http_client.clone(),
            map_http_client: self.map_http_client.clone(),
            cache: self.cache.clone(),
        }
    }

    /// Redirects may not leave the configured origin.
    ///
    /// This is the second half of "the identity is sent to the configured
    /// endpoint and nowhere else". `stamp_identity` decides what to stamp on the
    /// FIRST request; reqwest then carries the headers through any redirect it
    /// follows, and its sensitive-header list — the one that drops
    /// `Authorization`, `Cookie` and `Proxy-Authorization` on a host change — is
    /// FIXED and does not include a custom header. Measured in the sibling SDKs
    /// on `net/http` and on Node's `fetch`: the redirect target received the
    /// tenant credential stripped and the per-user one intact, which is the
    /// wrong one to lose.
    ///
    /// A cross-origin redirect is therefore STOPPED rather than followed. That
    /// is a small behaviour change and the right one for this client: it talks
    /// to ONE configured endpoint, so a redirect leaving that origin is already
    /// anomalous, and the caller sees the 3xx rather than a request that quietly
    /// carried a person's credential somewhere they never named. Same-origin
    /// redirects are followed as before, up to reqwest's usual bound.
    fn redirect_policy(endpoint: &str) -> reqwest::redirect::Policy {
        let configured = url::Url::parse(endpoint).ok();
        reqwest::redirect::Policy::custom(move |attempt| {
            if attempt.previous().len() >= 10 {
                return attempt.error("stopped after 10 redirects");
            }
            match &configured {
                Some(origin) if crate::read_identity::same_origin(attempt.url(), origin) => {
                    attempt.follow()
                }
                // Unparseable endpoint, or an off-origin hop: stop. Stopping
                // returns the 3xx to the caller rather than erroring, so an
                // ordinary redirect to another host degrades to a visible
                // non-2xx instead of a silent credential leak.
                _ => attempt.stop(),
            }
        })
    }

    /// The SDK's one send site — and therefore its one IDENTITY site.
    ///
    /// The read-path per-user identity is stamped here rather than as a default
    /// header, because it must be conditional on the request's ORIGIN: the
    /// identity is sent to the configured endpoint and nowhere else. See
    /// [`crate::read_identity`] for why that matters (reqwest strips
    /// `Authorization` across a host change and its list is fixed; a custom
    /// header is not on it), and `redirect_policy` for the second half of the
    /// guarantee.
    ///
    /// `override_token` is the PER-CALL identity, threaded through from the one
    /// surface that takes one ([`Self::raw_get_as`]); every other caller passes
    /// `None`, meaning "this call said nothing", so the client-wide
    /// [`AxonFlowConfig::user_token`] applies. It is threaded rather than read
    /// from the client because a process acting for several people must not need
    /// a client each.
    ///
    /// The request is BUILT here before being stamped, so the origin check runs
    /// against [`reqwest::Request::url`] — the URL that will actually be dialled
    /// — rather than against a string passed alongside it. A URL argument that
    /// can disagree with the request it describes is a guard that can be told
    /// the wrong thing.
    ///
    /// # The telemetry gate
    ///
    /// This is also the SDK's one interception point for the heartbeat, the
    /// Rust equivalent of the Go SDK's `doHttpRequest` middleware, rather than
    /// a `maybe_send_heartbeat` sprinkled per method. Consulting the gate here
    /// is what keeps a long-running service visible to telemetry: before
    /// 0.10.0 the constructor was the only trigger, so a service that crossed
    /// the 7-day boundary never pinged again.
    ///
    /// The user's request is never delayed by it: `maybe_send_heartbeat`
    /// returns after one mutex acquire on the suppressed path (the case on all
    /// but at most one request per hour), and does every blocking and network
    /// step on a spawned task.
    ///
    /// **`heartbeat.rs` must not route through here.** That module builds its
    /// own [`reqwest::Client`], which is precisely what stops the telemetry
    /// path — including its `/health` probe — from re-entering this gate.
    /// `no_http_send_outside_the_dispatch_funnel` fails the build if a new
    /// call site bypasses this function.
    async fn dispatch(
        &self,
        req: reqwest::RequestBuilder,
        override_token: Option<&str>,
    ) -> Result<reqwest::Response, AxonFlowError> {
        maybe_send_heartbeat(&self.config.endpoint, &self.config.mode);
        let (client, built) = req.build_split();
        let mut built = built?;
        self.stamp_identity(&mut built, override_token)?;
        Ok(client.execute(built).await?)
    }

    /// Stamp the per-user identity on a built request, if there is one and the
    /// request is bound for the configured endpoint.
    ///
    /// Separate from [`Self::dispatch`] only so it can be asserted directly:
    /// this origin guard is defence in depth beside `redirect_policy`, and a
    /// doubly-guarded property makes a single-guard mutant survive unless one of
    /// the two is exercised on its own.
    pub(crate) fn stamp_identity(
        &self,
        req: &mut reqwest::Request,
        override_token: Option<&str>,
    ) -> Result<(), AxonFlowError> {
        let token = match override_token {
            Some(explicit) => explicit.trim(),
            None => self.config.user_token.as_deref().unwrap_or("").trim(),
        };
        if token.is_empty() {
            // Never send an empty header. To the platform a present-but-empty
            // X-User-Token is still an absent one, but sending it advertises an
            // identity mechanism the caller is not using, and it is one refactor
            // away from a present-but-invalid token, which is a hard 401.
            return Ok(());
        }
        let Ok(configured) = url::Url::parse(&self.config.endpoint) else {
            // An unparseable endpoint is not a licence to send the credential
            // anyway.
            return Ok(());
        };
        if !crate::read_identity::same_origin(req.url(), &configured) {
            return Ok(());
        }
        let mut value = reqwest::header::HeaderValue::from_str(token)
            .map_err(|_| AxonFlowError::ConfigError(crate::read_identity::unusable_token(token)))?;
        // Marked sensitive so it is redacted from reqwest's own header Debug
        // output, the way the license key already is.
        value.set_sensitive(true);
        req.headers_mut()
            .insert(crate::read_identity::HEADER_USER_TOKEN, value);
        Ok(())
    }

    pub(crate) async fn checked_get(&self, url: &str) -> Result<reqwest::Response, AxonFlowError> {
        let resp = self.dispatch(self.http_client.get(url), None).await?;
        Self::check_status(resp).await
    }

    /// Crate-internal POST that serializes `body` as JSON and translates
    /// non-2xx into [`AxonFlowError::ApiError`] — the symmetric helper to
    /// [`checked_get`](Self::checked_get). Used by sibling modules
    /// (e.g. `hitl`) that POST a typed payload and don't need to branch
    /// on specific status codes before falling back to the generic error
    /// path.
    pub(crate) async fn checked_post_json<T: serde::Serialize + ?Sized>(
        &self,
        url: &str,
        body: &T,
    ) -> Result<reqwest::Response, AxonFlowError> {
        let resp = self
            .dispatch(self.http_client.post(url).json(body), None)
            .await?;
        Self::check_status(resp).await
    }

    /// Crate-internal GET that returns the raw response without translating
    /// non-2xx into [`AxonFlowError::ApiError`]. Lets sibling modules branch
    /// on specific status codes (e.g. parse a 429 V1 upgrade envelope into
    /// [`AxonFlowError::RateLimited`]) before falling back to the generic
    /// error path.
    /// GET without status translation, carrying a per-call read identity.
    ///
    /// Replaced the old `raw_get`, whose one caller (`list_decisions`) now has
    /// to pass an identity: a wrapper that defaulted it to `None` would be a
    /// second, quieter way to make an unidentified read, which is the state
    /// this whole surface exists to make visible.
    ///
    /// `None` means "this call said nothing", so the client-wide identity
    /// applies; `Some("")` is a deliberate unidentified read. See
    /// [`Self::stamp_identity`].
    pub(crate) async fn raw_get_as(
        &self,
        url: &str,
        user_token: Option<&str>,
    ) -> Result<reqwest::Response, AxonFlowError> {
        self.dispatch(self.http_client.get(url), user_token).await
    }

    /// Crate-internal POST of an already-encoded body with per-request headers,
    /// returning the raw response without translating non-2xx.
    ///
    /// It exists so the AuthZEN surface can negotiate a profile header and read
    /// a typed refusal off a 4xx body, on THIS client - the one carrying the
    /// configured timeout, TLS posture, pool and the Basic-auth default headers
    /// every other call already uses. A second `reqwest::Client` built beside it
    /// would be a second transport with its own opinions about all four, and
    /// the two would drift on the first configuration change.
    ///
    /// The body arrives pre-encoded rather than as `impl Serialize` because the
    /// caller has to be able to report an ENCODING failure in its own
    /// vocabulary: an attribute nobody could resolve has no wire form, and
    /// `reqwest`'s `.json()` would surface that as a transport error.
    pub(crate) async fn raw_post_json_bytes(
        &self,
        url: &str,
        body: Vec<u8>,
        headers: &[(&str, &str)],
    ) -> Result<reqwest::Response, AxonFlowError> {
        let mut request = self
            .http_client
            .post(url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json")
            .body(body);
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        self.dispatch(request, None).await
    }

    async fn checked_map_get(&self, url: &str) -> Result<reqwest::Response, AxonFlowError> {
        let resp = self.dispatch(self.map_http_client.get(url), None).await?;
        Self::check_status(resp).await
    }

    pub(crate) async fn check_status(
        resp: reqwest::Response,
    ) -> Result<reqwest::Response, AxonFlowError> {
        if resp.status().is_success() {
            Ok(resp)
        } else {
            let status = resp.status().as_u16();
            let message = resp.text().await?;
            Err(AxonFlowError::ApiError { status, message })
        }
    }

    /// Retry the request with exponential backoff, honoring the
    /// SDK-wide retry contract.
    ///
    /// **Retried status codes:**
    /// - 5xx — server-side failures (treated as transient).
    /// - 429 — rate-limit responses (transient by definition).
    /// - Transport-level errors (connection refused, DNS, TLS) —
    ///   surfaced as non-`ApiError` variants of [`AxonFlowError`];
    ///   the `if let AxonFlowError::ApiError { .. }` guard doesn't
    ///   match them, so they fall through to `last_err = Some(e)` and
    ///   retry on the next iteration.
    ///
    /// **Terminal status codes (early `return Err(e)`):**
    /// - 401 — auth failure. Retrying with the same invalid
    ///   credential just compounds the storm on the agent. See
    ///   issue [#2275](https://github.com/getaxonflow/axonflow-enterprise/issues/2275)
    ///   for the customer-observed retry loop that motivated the
    ///   regression-locking test `test_401_not_retried_issue_2275`.
    /// - 400, 404, 405, 406, 408, 409, 410, 411, 412, 413, 414, 415,
    ///   416, 417, 418, 421, 422, 423, 424, 425, 426, 428, 431, 451 —
    ///   every other 4xx that isn't in the `{429, 402, 403}` allowlist.
    ///
    /// **Caveat on 402/403:** `execute_request` returns 402 + 403 as
    /// `Ok(client_resp)` because those are SUCCESS responses carrying
    /// policy/quota envelope data — not errors. They never reach this
    /// function as `Err`, so the `*status != 402` and `*status != 403`
    /// clauses below are functionally dead in current code. They're
    /// kept as intent-preserving belt-and-suspenders for any future
    /// refactor that converts 402/403 back to `Err`.
    ///
    /// See `CHANGELOG.md` for the contract's history.
    async fn execute_with_retry(
        &self,
        req: &ClientRequest,
    ) -> Result<ClientResponse, AxonFlowError> {
        let mut last_err = None;

        for attempt in 0..self.config.retry.max_attempts {
            if attempt > 0 {
                let delay =
                    self.config.retry.initial_delay.as_secs_f64() * 2f64.powi((attempt - 1) as i32);
                tokio::time::sleep(Duration::from_secs_f64(delay)).await;
            }

            match self.execute_request(req).await {
                Ok(resp) => return Ok(resp),
                Err(e) => {
                    if let AxonFlowError::ApiError { status, .. } = &e {
                        // Retry allowlist: any 4xx NOT in {429, 402, 403} is
                        // terminal. 5xx always retries (falls through to the
                        // `last_err = Some(e)` path below).
                        //
                        // 402/403 NEVER reach this branch as `Err`: see
                        // `execute_request` at line 586 — those statuses
                        // return as `Ok(client_resp)` because they carry
                        // policy/quota envelope data. The `*status != 402`
                        // and `*status != 403` clauses are intentional
                        // belt-and-suspenders for a hypothetical future
                        // refactor that errors on those statuses.
                        if *status >= 400
                            && *status < 500
                            && *status != 429
                            && *status != 402
                            && *status != 403
                        {
                            return Err(e);
                        }
                    }
                    last_err = Some(e);
                }
            }
        }

        Err(last_err.unwrap_or_else(|| {
            AxonFlowError::ConfigError("retry loop completed with no attempts".to_string())
        }))
    }

    async fn execute_request(&self, req: &ClientRequest) -> Result<ClientResponse, AxonFlowError> {
        let url = format!("{}/api/request", self.config.endpoint);
        let resp = self
            .dispatch(self.http_client.post(&url).json(req), None)
            .await?;

        let status = resp.status();
        let body = resp.text().await?;

        if status.is_success() || status.as_u16() == 402 || status.as_u16() == 403 {
            match serde_json::from_str::<ClientResponse>(&body) {
                Ok(r) => Ok(r),
                Err(_) => Err(AxonFlowError::ApiError {
                    status: status.as_u16(),
                    message: body,
                }),
            }
        } else {
            Err(AxonFlowError::ApiError {
                status: status.as_u16(),
                message: body,
            })
        }
    }
}

#[cfg(test)]
mod read_identity_tests {
    use crate::read_identity::HEADER_USER_TOKEN;
    use crate::{AxonFlowClient, AxonFlowConfig};

    fn client_for(endpoint: &str, token: Option<&str>) -> AxonFlowClient {
        AxonFlowClient::new(AxonFlowConfig {
            endpoint: endpoint.to_string(),
            user_token: token.map(str::to_string),
            ..AxonFlowConfig::new(endpoint)
        })
        .expect("client")
    }

    /// What `stamp_identity` stamped, for a request to `url`.
    ///
    /// Built from the client's OWN transport rather than a fresh
    /// `reqwest::Client`, for two reasons: it is what production does, so the
    /// default headers and configuration are the real ones; and
    /// `the_telemetry_path_builds_exactly_one_http_client` counts transports
    /// constructed in this FILE — it reads the source text, so a `#[cfg(test)]`
    /// module trips it even though the client never ships. That guard exists to
    /// stop a second transport with its own opinions about timeouts, TLS and
    /// pooling, and a test is not a licence to add one.
    fn stamped(client: &AxonFlowClient, url: &str, override_token: Option<&str>) -> Option<String> {
        let mut req = client.http_client.get(url).build().expect("request");
        client
            .stamp_identity(&mut req, override_token)
            .expect("the fixture tokens are all usable header values");
        req.headers()
            .get(HEADER_USER_TOKEN)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
    }

    /// The ORIGIN guard on the stamping side, exercised on its own.
    ///
    /// It exists as defence in depth beside `redirect_policy`, and a mutation
    /// run proved why it needs its own test: with only the integration tests,
    /// deleting this guard changed NOTHING observable, because the redirect
    /// policy already stopped every off-origin hop before a request could be
    /// built for one. A doubly-guarded property makes a single-guard mutant
    /// survive — so the inner guard is asserted here, directly, where the outer
    /// one cannot stand in for it.
    ///
    /// It is not redundant: any future method that takes a caller-supplied URL
    /// rather than deriving one from `endpoint()` would reach the transport
    /// without passing the redirect policy at all.
    #[test]
    fn the_identity_is_stamped_only_for_the_configured_origin() {
        let client = client_for("https://localhost:8080", Some("SENTINEL"));

        assert_eq!(
            stamped(&client, "https://localhost:8080/api/v1/decisions", None).as_deref(),
            Some("SENTINEL"),
            "the configured origin must carry the identity"
        );
        for elsewhere in [
            "https://elsewhere.invalid/api/v1/decisions", // different host
            "https://localhost:9999/api/v1/decisions",    // different port
            // Different scheme, and the direction that matters: a downgrade to
            // cleartext must not carry the credential. Same host, same port.
            "http://localhost:8080/api/v1/decisions",
        ] {
            assert_eq!(
                stamped(&client, elsewhere, None),
                None,
                "{elsewhere} is not the configured origin and must not receive the identity"
            );
        }
    }

    /// The header is marked sensitive, so the credential does not reach a log
    /// line through the ordinary `{:?}` that every debug print of a request
    /// uses.
    ///
    /// The positive control is the point of this test, not decoration: a
    /// `{:?}` that happens to render no headers at all would satisfy the
    /// absence assertion while proving nothing, and that is exactly what a
    /// future reqwest release changing its Debug shape would look like. The
    /// control asserts the same rendering DOES show an ordinary header, so the
    /// absence of the sensitive one is a redaction rather than an empty page.
    #[test]
    fn the_identity_is_redacted_from_the_requests_debug_output() {
        let client = client_for("https://localhost:8080", Some("SENTINEL-TOKEN-VALUE"));
        let mut req = client
            .http_client
            .get("https://localhost:8080/api/v1/decisions")
            .header("X-Ordinary-Header", "ORDINARY-CONTROL-VALUE")
            .build()
            .expect("request");
        client.stamp_identity(&mut req, None).expect("usable token");

        let rendered = format!("{:?}", req);

        assert!(
            rendered.contains("ORDINARY-CONTROL-VALUE"),
            "positive control: this Debug rendering must show an ordinary header value, or the \
             absence assertion below holds for the wrong reason. Got: {rendered}"
        );
        assert!(
            rendered.contains(HEADER_USER_TOKEN)
                || rendered.contains(&HEADER_USER_TOKEN.to_ascii_lowercase()),
            "positive control: the header must be PRESENT on the request and merely redacted — \
             a test that passes because nothing was stamped is not a redaction test. Got: \
             {rendered}"
        );
        assert!(
            !rendered.contains("SENTINEL-TOKEN-VALUE"),
            "the per-user identity reached a Debug rendering. set_sensitive(true) is what keeps \
             it out of tracing spans, panic messages and debugger frames. Got: {rendered}"
        );
    }

    /// A token no header value can carry is REPORTED, and the report does not
    /// echo it.
    ///
    /// The unit-level half of the integration test that also proves nothing was
    /// sent; this one pins the message's content, which is the part a caller
    /// actually acts on.
    #[test]
    fn an_unusable_token_is_reported_without_echoing_it() {
        let client = client_for("https://localhost:8080", Some("SENTINEL\u{7f}VALUE"));
        let mut req = client
            .http_client
            .get("https://localhost:8080/api/v1/decisions")
            .build()
            .expect("request");

        let err = client
            .stamp_identity(&mut req, None)
            .expect_err("an unsendable identity must be reported");
        let rendered = err.to_string();

        assert!(!rendered.contains("SENTINEL"), "token echoed: {rendered}");
        assert!(!rendered.contains("VALUE"), "token echoed: {rendered}");
        assert!(
            rendered.contains("control character") && rendered.contains("byte 8"),
            "the message must locate the offending byte and name its class: {rendered}"
        );

        // A TAB before the control character. Tab is a LEGAL header-value byte,
        // so a predicate written as "first non-printable" would report the
        // TAB's position and send the reader to the one character that was
        // fine. Here the tab is byte 8 and the DEL is byte 10.
        let mut tabbed = client
            .http_client
            .get("https://localhost:8080/api/v1/decisions")
            .build()
            .expect("request");
        let tab_err = client
            .stamp_identity(&mut tabbed, Some("SENTINEL\tX\u{7f}VALUE"))
            .expect_err("an embedded DEL must be refused");
        assert!(
            tab_err.to_string().contains("byte 10"),
            "want the DEL's position, not the legal tab's: {tab_err}"
        );
        assert!(
            req.headers().get(HEADER_USER_TOKEN).is_none(),
            "a rejected token must leave no header behind"
        );
    }

    /// The diagnostic must point at the byte that is actually refused.
    ///
    /// A header value admits obs-text, so every byte from 0x80 up is legal —
    /// `café-token` is sent as-is. A predicate written as "not printable ASCII"
    /// accepts the same tokens but, when one IS refused, reports the position
    /// of the first non-ASCII byte instead of the control character that caused
    /// it, sending the reader to fix the one character that was fine.
    #[test]
    fn the_diagnostic_points_at_the_control_char_not_at_legal_non_ascii() {
        let client = client_for("https://localhost:8080", None);
        let mut req = client
            .http_client
            .get("https://localhost:8080/api/v1/decisions")
            .build()
            .expect("request");

        // "café" is 5 bytes: the é is bytes 3-4. The newline is byte 9.
        let err = client
            .stamp_identity(&mut req, Some("café-tok\nen"))
            .expect_err("an embedded newline must be refused");
        let rendered = err.to_string();

        assert!(
            rendered.contains("byte 9"),
            "want the newline's position, not the first non-ASCII byte's: {rendered}"
        );
        assert!(
            !rendered.contains("non-ASCII byte at"),
            "non-ASCII is legal in a header value and must not be named as the offender: \
             {rendered}"
        );
    }

    /// The other direction: a token that is merely non-ASCII must be SENT, not
    /// refused. A diagnostic fixed by tightening the predicate instead of
    /// correcting it would reject a legal credential.
    #[test]
    fn a_non_ascii_token_is_sent_rather_than_refused() {
        let client = client_for("https://localhost:8080", Some("café-token"));
        let mut req = client
            .http_client
            .get("https://localhost:8080/api/v1/decisions")
            .build()
            .expect("request");
        client
            .stamp_identity(&mut req, None)
            .expect("obs-text is legal in a header value; refusing it would break a caller whose identity provider mints one");

        // Compared as BYTES, not through `to_str()`: `HeaderValue::to_str`
        // refuses obs-text itself, so a value that is perfectly legal on the
        // wire reads back as None through it. The `stamped` helper takes that
        // path, which is why this test builds the request itself — a helper
        // that cannot represent the value under test would have made this
        // assertion fail for a reason that has nothing to do with the SDK.
        assert_eq!(
            req.headers().get(HEADER_USER_TOKEN).map(|v| v.as_bytes()),
            Some("café-token".as_bytes()),
            "a non-ASCII token must be SENT, not refused"
        );
    }

    /// An unparseable configured endpoint is not a licence to send the
    /// credential anyway — the guard fails CLOSED, not open.
    #[test]
    fn an_unparseable_endpoint_sends_no_identity() {
        let client = client_for("not a url", Some("SENTINEL"));
        assert_eq!(stamped(&client, "https://localhost:8080/x", None), None);
    }

    /// `Some("")` is a caller deliberately making one read unidentified and must
    /// not fall back to the client-wide value; `None` means "this call said
    /// nothing" and must.
    #[test]
    fn an_explicit_empty_override_does_not_fall_back() {
        let client = client_for("https://localhost:8080", Some("CLIENT-WIDE"));
        let url = "https://localhost:8080/api/v1/decisions";

        assert_eq!(stamped(&client, url, None).as_deref(), Some("CLIENT-WIDE"));
        assert_eq!(
            stamped(&client, url, Some("PER-CALL")).as_deref(),
            Some("PER-CALL")
        );
        assert_eq!(stamped(&client, url, Some("   ")), None);
    }
}
