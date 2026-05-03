use crate::config::{AxonFlowConfig, Mode};
use crate::error::AxonFlowError;
use crate::heartbeat::maybe_send_heartbeat;
use crate::types::agent::{ClientRequest, ClientResponse};
use moka::future::Cache;
use reqwest::header::{HeaderMap, HeaderValue, CONTENT_TYPE};
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, warn};

pub struct AxonFlowClient {
    config: AxonFlowConfig,
    http_client: reqwest::Client,
    cache: Option<Arc<Cache<String, ClientResponse>>>,
}

impl AxonFlowClient {
    pub fn new(mut config: AxonFlowConfig) -> Result<Self, AxonFlowError> {
        // Try mode override
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

        if let Some(client_id) = &config.client_id {
            if let Ok(val) = HeaderValue::from_str(client_id) {
                headers.insert("X-AxonFlow-Client-ID", val);
            }
        }
        if let Some(client_secret) = &config.client_secret {
            if let Ok(val) = HeaderValue::from_str(client_secret) {
                headers.insert("X-AxonFlow-Client-Secret", val);
            }
        }

        let builder = reqwest::Client::builder()
            .timeout(config.timeout)
            .default_headers(headers);

        let builder = if config.insecure_skip_tls_verify || std::env::var("NODE_TLS_REJECT_UNAUTHORIZED").unwrap_or_default() == "0" {
            warn!("TLS certificate verification is disabled.");
            builder.danger_accept_invalid_certs(true)
        } else {
            builder
        };

        let http_client = builder.build().map_err(AxonFlowError::HttpError)?;

        let cache = if config.cache.enabled {
            Some(Arc::new(
                Cache::builder()
                    .time_to_live(config.cache.ttl)
                    .build(),
            ))
        } else {
            None
        };

        maybe_send_heartbeat(&config.endpoint);

        Ok(Self {
            config,
            http_client,
            cache,
        })
    }

    pub async fn proxy_llm_call(
        &self,
        user_token: &str,
        query: &str,
        request_type: &str,
        context: std::collections::HashMap<String, serde_json::Value>,
    ) -> Result<ClientResponse, AxonFlowError> {
        let user_token = if user_token.is_empty() {
            "anonymous"
        } else {
            user_token
        };

        let cache_key = format!("{}:{}:{}", request_type, query, user_token);
        
        let is_mutation = request_type == "execute-plan"
            || request_type == "generate-plan"
            || request_type == "cancel-plan"
            || request_type == "update-plan";

        if !is_mutation {
            if let Some(cache) = &self.cache {
                if let Some(cached) = cache.get(&cache_key).await {
                    if self.config.debug {
                        debug!("Cache hit for query");
                    }
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
            self.execute_with_retry(req).await
        } else {
            self.execute_request(&req).await
        };

        match resp {
            Ok(response) => {
                if response.success && !is_mutation {
                    if let Some(cache) = &self.cache {
                        cache.insert(cache_key, response.clone()).await;
                    }
                }
                Ok(response)
            }
            Err(e) => {
                if self.config.mode == Mode::Production && e.is_retryable() {
                    if self.config.debug {
                        debug!("AxonFlow unavailable, failing open: {}", e);
                    }
                    Ok(ClientResponse::fail_open(e))
                } else {
                    Err(e)
                }
            }
        }
    }

    // ============================================================================
    // MCP Connector Management (Issue #963, #975)
    // ============================================================================

    pub async fn list_connectors(&self) -> Result<Vec<crate::types::agent::ConnectorMetadata>, AxonFlowError> {
        let url = format!("{}/api/v1/connectors", self.config.endpoint);
        let resp = self.http_client.get(&url).send().await?;

        if resp.status() != reqwest::StatusCode::OK {
            return Err(AxonFlowError::ApiError {
                status: resp.status().as_u16(),
                message: resp.text().await?,
            });
        }

        let body: serde_json::Value = resp.json().await?;
        let connectors = body["connectors"].as_array()
            .ok_or_else(|| AxonFlowError::SerdeError(serde::de::Error::custom("missing connectors field")))?;

        let result = serde_json::from_value(serde_json::Value::Array(connectors.clone()))?;
        Ok(result)
    }

    pub async fn get_connector(&self, connector_id: &str) -> Result<crate::types::agent::ConnectorMetadata, AxonFlowError> {
        let url = format!("{}/api/v1/connectors/{}", self.config.endpoint, connector_id);
        let resp = self.http_client.get(&url).send().await?;

        if resp.status() != reqwest::StatusCode::OK {
            return Err(AxonFlowError::ApiError {
                status: resp.status().as_u16(),
                message: resp.text().await?,
            });
        }

        Ok(resp.json().await?)
    }

    pub async fn get_connector_health(&self, connector_id: &str) -> Result<crate::types::agent::ConnectorHealthStatus, AxonFlowError> {
        let url = format!("{}/api/v1/connectors/{}/health", self.config.endpoint, connector_id);
        let resp = self.http_client.get(&url).send().await?;

        if resp.status() != reqwest::StatusCode::OK {
            return Err(AxonFlowError::ApiError {
                status: resp.status().as_u16(),
                message: resp.text().await?,
            });
        }

        Ok(resp.json().await?)
    }

    pub async fn install_connector(&self, req: crate::types::agent::ConnectorInstallRequest) -> Result<(), AxonFlowError> {
        let url = format!("{}/api/v1/connectors", self.config.endpoint);
        let resp = self.http_client.post(&url).json(&req).send().await?;

        if resp.status() != reqwest::StatusCode::OK {
            return Err(AxonFlowError::ApiError {
                status: resp.status().as_u16(),
                message: resp.text().await?,
            });
        }

        Ok(())
    }

    pub async fn query_connector(
        &self,
        user_token: &str,
        connector_name: &str,
        query: &str,
        params: std::collections::HashMap<String, serde_json::Value>,
    ) -> Result<crate::types::agent::ConnectorResponse, AxonFlowError> {
        let req_body = serde_json::json!({
            "user_token": user_token,
            "connector": connector_name,
            "query": query,
            "params": params,
            "client_id": self.get_effective_client_id(),
        });

        let url = format!("{}/api/v1/query", self.config.endpoint);
        let resp = self.http_client.post(&url).json(&req_body).send().await?;

        if resp.status() != reqwest::StatusCode::OK {
            return Err(AxonFlowError::ApiError {
                status: resp.status().as_u16(),
                message: resp.text().await?,
            });
        }

        Ok(resp.json().await?)
    }

    // ============================================================================
    // Multi-Agent Planning (MAP) (Issue #1019, #1020)
    // ============================================================================

    pub async fn generate_plan(&self, query: &str, _domain: &str, user_token: Option<&str>) -> Result<crate::types::agent::PlanResponse, AxonFlowError> {
        let context = std::collections::HashMap::new();
        let user_token = user_token.unwrap_or("anonymous");
        
        let resp = self.proxy_llm_call(user_token, query, "generate-plan", context).await?;
        
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

    pub async fn execute_plan(&self, plan_id: &str, user_token: Option<&str>) -> Result<crate::types::agent::PlanExecutionResponse, AxonFlowError> {
        let mut context = std::collections::HashMap::new();
        context.insert("plan_id".to_string(), serde_json::json!(plan_id));
        let user_token = user_token.unwrap_or("anonymous");

        let resp = self.proxy_llm_call(user_token, "", "execute-plan", context).await?;

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

    pub async fn get_plan_status(&self, plan_id: &str) -> Result<crate::types::agent::PlanExecutionResponse, AxonFlowError> {
        let url = format!("{}/api/v1/plans/{}/status", self.config.endpoint, plan_id);
        let resp = self.http_client.get(&url).send().await?;

        if resp.status() != reqwest::StatusCode::OK {
            return Err(AxonFlowError::ApiError {
                status: resp.status().as_u16(),
                message: resp.text().await?,
            });
        }

        Ok(resp.json().await?)
    }

    pub async fn cancel_plan(&self, plan_id: &str, reason: Option<&str>) -> Result<crate::types::agent::CancelPlanResponse, AxonFlowError> {
        let req_body = serde_json::json!({
            "reason": reason.unwrap_or("user_cancelled"),
        });

        let url = format!("{}/api/v1/plans/{}/cancel", self.config.endpoint, plan_id);
        let resp = self.http_client.post(&url).json(&req_body).send().await?;

        if resp.status() != reqwest::StatusCode::OK {
            return Err(AxonFlowError::ApiError {
                status: resp.status().as_u16(),
                message: resp.text().await?,
            });
        }

        Ok(resp.json().await?)
    }

    pub async fn audit_llm_call(
        &self,
        context_id: &str,
        response_summary: &str,
        provider: &str,
        model: &str,
        token_usage: crate::types::agent::TokenUsage,
        latency_ms: i64,
        metadata: Option<std::collections::HashMap<String, serde_json::Value>>,
    ) -> Result<crate::types::agent::AuditResult, AxonFlowError> {
        let client_id = self.get_effective_client_id();

        let mut req_body = serde_json::json!({
            "context_id": context_id,
            "client_id": client_id,
            "response_summary": response_summary,
            "provider": provider,
            "model": model,
            "token_usage": {
                "prompt_tokens": token_usage.prompt_tokens,
                "completion_tokens": token_usage.completion_tokens,
                "total_tokens": token_usage.total_tokens,
            },
            "latency_ms": latency_ms,
        });

        if let Some(meta) = metadata {
            req_body["metadata"] = serde_json::to_value(meta)?;
        } else {
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

    fn get_effective_client_id(&self) -> String {
        self.config.client_id.clone().unwrap_or_else(|| "community".to_string())
    }

    async fn execute_with_retry(&self, req: ClientRequest) -> Result<ClientResponse, AxonFlowError> {
        let mut last_err = None;

        for attempt in 0..self.config.retry.max_attempts {
            if attempt > 0 {
                let delay = self.config.retry.initial_delay.as_secs_f64() * 2f64.powi((attempt - 1) as i32);
                tokio::time::sleep(Duration::from_secs_f64(delay)).await;
            }

            match self.execute_request(&req).await {
                Ok(resp) => return Ok(resp),
                Err(e) => {
                    if let AxonFlowError::ApiError { status, .. } = &e {
                        if *status >= 400 && *status < 500 && *status != 429 && *status != 402 && *status != 403 {
                            return Err(e); // Don't retry 4xx errors unless 429/402/403
                        }
                    }
                    last_err = Some(e);
                }
            }
        }

        Err(last_err.unwrap())
    }

    async fn execute_request(&self, req: &ClientRequest) -> Result<ClientResponse, AxonFlowError> {
        let url = format!("{}/api/request", self.config.endpoint);
        let resp = self.http_client.post(&url).json(req).send().await?;

        let status = resp.status();
        let body = resp.text().await?;

        if status.is_success() || status.as_u16() == 402 || status.as_u16() == 403 {
            let client_resp: ClientResponse = serde_json::from_str(&body)?;
            Ok(client_resp)
        } else {
            Err(AxonFlowError::ApiError {
                status: status.as_u16(),
                message: body,
            })
        }
    }
}
