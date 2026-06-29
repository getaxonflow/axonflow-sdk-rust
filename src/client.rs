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
            .default_headers(headers.clone())
            .danger_accept_invalid_certs(accept_invalid)
            .pool_max_idle_per_host(5)
            .tcp_keepalive(Duration::from_secs(30))
            .build()
            .map_err(AxonFlowError::HttpError)?;

        let map_http_client = reqwest::Client::builder()
            .timeout(config.map_timeout)
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
        let resp = self.http_client.post(&url).json(&req).send().await?;
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
            let exec: crate::types::agent::PlanExecutionResponse = serde_json::from_value(data)?;
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
            .map_http_client
            .post(&url)
            .json(&req_body)
            .send()
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
        let resp = self.http_client.post(&url).json(&req_body).send().await?;

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

    pub(crate) async fn checked_get(&self, url: &str) -> Result<reqwest::Response, AxonFlowError> {
        let resp = self.http_client.get(url).send().await?;
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
        let resp = self.http_client.post(url).json(body).send().await?;
        Self::check_status(resp).await
    }

    /// Crate-internal GET that returns the raw response without translating
    /// non-2xx into [`AxonFlowError::ApiError`]. Lets sibling modules branch
    /// on specific status codes (e.g. parse a 429 V1 upgrade envelope into
    /// [`AxonFlowError::RateLimited`]) before falling back to the generic
    /// error path.
    pub(crate) async fn raw_get(&self, url: &str) -> Result<reqwest::Response, AxonFlowError> {
        Ok(self.http_client.get(url).send().await?)
    }

    async fn checked_map_get(&self, url: &str) -> Result<reqwest::Response, AxonFlowError> {
        let resp = self.map_http_client.get(url).send().await?;
        Self::check_status(resp).await
    }

    async fn check_status(resp: reqwest::Response) -> Result<reqwest::Response, AxonFlowError> {
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
        let resp = self.http_client.post(&url).json(req).send().await?;

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
