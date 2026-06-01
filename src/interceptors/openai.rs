use crate::client::AxonFlowClient;
use crate::error::AxonFlowError;
use crate::types::agent::{AuditRequest, TokenUsage};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ChatCompletionRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<usize>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ChatCompletionChoice {
    pub index: usize,
    pub message: ChatMessage,
    pub finish_reason: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Usage {
    pub prompt_tokens: usize,
    pub completion_tokens: usize,
    pub total_tokens: usize,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ChatCompletionResponse {
    pub id: String,
    pub object: String,
    pub created: i64,
    pub model: String,
    pub choices: Vec<ChatCompletionChoice>,
    pub usage: Usage,
}

#[async_trait]
pub trait OpenAIChatCompleter: Send + Sync {
    async fn create_chat_completion(
        &self,
        req: ChatCompletionRequest,
    ) -> Result<ChatCompletionResponse, Box<dyn std::error::Error + Send + Sync>>;
}

pub struct WrappedOpenAIClient<C: OpenAIChatCompleter> {
    client: Arc<C>,
    axonflow: Arc<AxonFlowClient>,
    user_token: String,
    /// Provider name recorded in the audit trail. Defaults to `"openai"` but
    /// should be set to the actual provider (e.g. `"azure"`, `"together"`)
    /// when calling through a proxy or alternative OpenAI-compatible API.
    provider: String,
}

impl<C: OpenAIChatCompleter> WrappedOpenAIClient<C> {
    pub fn new(client: C, axonflow: AxonFlowClient, user_token: impl Into<String>) -> Self {
        Self {
            client: Arc::new(client),
            axonflow: Arc::new(axonflow),
            user_token: user_token.into(),
            provider: "openai".to_string(),
        }
    }

    pub fn with_provider(mut self, provider: impl Into<String>) -> Self {
        self.provider = provider.into();
        self
    }

    pub async fn create_chat_completion(
        &self,
        req: ChatCompletionRequest,
    ) -> Result<ChatCompletionResponse, Box<dyn std::error::Error + Send + Sync>> {
        // Extract prompt
        let prompt = req
            .messages
            .iter()
            .map(|m| format!("{}: {}", m.role, m.content))
            .collect::<Vec<_>>()
            .join("\n");

        let mut eval_context = std::collections::HashMap::new();
        eval_context.insert("provider".to_string(), serde_json::json!(self.provider));
        eval_context.insert("model".to_string(), serde_json::json!(req.model));
        if let Some(t) = req.temperature {
            eval_context.insert("temperature".to_string(), serde_json::json!(t));
        }
        if let Some(m) = req.max_tokens {
            eval_context.insert("max_tokens".to_string(), serde_json::json!(m));
        }

        // Check with AxonFlow
        let start_time = std::time::Instant::now();
        let response = self
            .axonflow
            .proxy_llm_call(&self.user_token, &prompt, "llm_chat", eval_context)
            .await
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;

        if response.blocked {
            return Err(Box::new(AxonFlowError::ApiError {
                status: 403,
                message: response
                    .block_reason
                    .unwrap_or_else(|| "Blocked by policy".to_string()),
            }));
        }

        // Make actual call
        let result = self.client.create_chat_completion(req.clone()).await?;

        // Audit (async, fire-and-forget)
        let axonflow = Arc::clone(&self.axonflow);
        let result_clone = result.clone();
        let request_id = response.request_id.clone();
        let latency_ms = start_time.elapsed().as_millis() as i64;
        let model = req.model.clone();
        let provider = self.provider.clone();

        tokio::spawn(async move {
            if let Some(context_id) = request_id {
                let summary = result_clone
                    .choices
                    .first()
                    .map(|c| {
                        let content = &c.message.content;
                        match content.char_indices().nth(100) {
                            Some((idx, _)) => format!("{}...", &content[..idx]),
                            None => content.clone(),
                        }
                    })
                    .unwrap_or_default();

                let token_usage = TokenUsage {
                    prompt_tokens: result_clone.usage.prompt_tokens,
                    completion_tokens: result_clone.usage.completion_tokens,
                    total_tokens: result_clone.usage.total_tokens,
                };

                let audit_req = AuditRequest {
                    context_id,
                    response_summary: summary,
                    provider,
                    model,
                    token_usage,
                    latency_ms,
                    metadata: None,
                };

                if let Err(e) = axonflow.audit_llm_call(&audit_req).await {
                    tracing::warn!("Audit call failed: {e}");
                }
            }
        });

        Ok(result)
    }
}
