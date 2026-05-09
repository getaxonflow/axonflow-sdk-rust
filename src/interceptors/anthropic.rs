use crate::client::AxonFlowClient;
use crate::error::AxonFlowError;
use crate::types::agent::{AuditRequest, TokenUsage};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AnthropicMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AnthropicRequest {
    pub model: String,
    pub messages: Vec<AnthropicMessage>,
    pub max_tokens: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AnthropicContent {
    pub r#type: String,
    pub text: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AnthropicUsage {
    pub input_tokens: usize,
    pub output_tokens: usize,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AnthropicResponse {
    pub id: String,
    pub r#type: String,
    pub role: String,
    pub content: Vec<AnthropicContent>,
    pub model: String,
    pub usage: AnthropicUsage,
}

#[async_trait]
pub trait AnthropicMessageCreator: Send + Sync {
    async fn create_message(
        &self,
        req: AnthropicRequest,
    ) -> Result<AnthropicResponse, Box<dyn std::error::Error + Send + Sync>>;
}

pub struct WrappedAnthropicClient<C: AnthropicMessageCreator> {
    client: Arc<C>,
    axonflow: Arc<AxonFlowClient>,
    user_token: String,
}

impl<C: AnthropicMessageCreator> WrappedAnthropicClient<C> {
    pub fn new(client: C, axonflow: AxonFlowClient, user_token: impl Into<String>) -> Self {
        Self {
            client: Arc::new(client),
            axonflow: Arc::new(axonflow),
            user_token: user_token.into(),
        }
    }

    pub async fn create_message(
        &self,
        req: AnthropicRequest,
    ) -> Result<AnthropicResponse, Box<dyn std::error::Error + Send + Sync>> {
        // Extract prompt
        let mut prompt_parts = Vec::new();
        if let Some(system) = &req.system {
            prompt_parts.push(format!("System: {}", system));
        }
        for m in &req.messages {
            prompt_parts.push(format!("{}: {}", m.role, m.content));
        }
        let prompt = prompt_parts.join("\n");

        let mut eval_context = std::collections::HashMap::new();
        eval_context.insert("provider".to_string(), serde_json::json!("anthropic"));
        eval_context.insert("model".to_string(), serde_json::json!(req.model));
        eval_context.insert("max_tokens".to_string(), serde_json::json!(req.max_tokens));
        if let Some(t) = req.temperature {
            eval_context.insert("temperature".to_string(), serde_json::json!(t));
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
        let result = self.client.create_message(req.clone()).await?;

        // Audit (async, fire-and-forget)
        let axonflow = Arc::clone(&self.axonflow);
        let result_clone = result.clone();
        let request_id = response.request_id.clone();
        let latency_ms = start_time.elapsed().as_millis() as i64;
        let model = req.model.clone();

        tokio::spawn(async move {
            if let Some(context_id) = request_id {
                let summary = result_clone
                    .content
                    .first()
                    .map(|c| {
                        let content = &c.text;
                        match content.char_indices().nth(100) {
                            Some((idx, _)) => format!("{}...", &content[..idx]),
                            None => content.clone(),
                        }
                    })
                    .unwrap_or_default();

                let token_usage = TokenUsage {
                    prompt_tokens: result_clone.usage.input_tokens,
                    completion_tokens: result_clone.usage.output_tokens,
                    total_tokens: result_clone.usage.input_tokens
                        + result_clone.usage.output_tokens,
                };

                let audit_req = AuditRequest {
                    context_id,
                    response_summary: summary,
                    provider: "anthropic".to_string(),
                    model,
                    token_usage,
                    latency_ms,
                    metadata: None,
                };

                let _ = axonflow.audit_llm_call(&audit_req).await;
            }
        });

        Ok(result)
    }
}
