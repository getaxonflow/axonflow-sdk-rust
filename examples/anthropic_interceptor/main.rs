use async_trait::async_trait;
use axonflow_sdk_rust::interceptors::anthropic::{
    AnthropicContent, AnthropicMessage, AnthropicMessageCreator, AnthropicRequest,
    AnthropicResponse, AnthropicUsage, WrappedAnthropicClient,
};
use axonflow_sdk_rust::{AxonFlowClient, AxonFlowConfig};

// Example of a raw Anthropic client implementation. The real one would call
// `https://api.anthropic.com/v1/messages` with `x-api-key`. The interceptor
// pattern is provider-agnostic — only governance + audit live in AxonFlow,
// the actual LLM call stays in the host application.
struct MyRawAnthropicClient;

#[async_trait]
impl AnthropicMessageCreator for MyRawAnthropicClient {
    async fn create_message(
        &self,
        req: AnthropicRequest,
    ) -> Result<AnthropicResponse, Box<dyn std::error::Error + Send + Sync>> {
        println!(
            "  [Underlying Client] Calling Anthropic Messages API for model: {}",
            req.model
        );

        Ok(AnthropicResponse {
            id: "msg_01ABC".to_string(),
            r#type: "message".to_string(),
            role: "assistant".to_string(),
            content: vec![AnthropicContent {
                r#type: "text".to_string(),
                text: "Hello from mock Claude.".to_string(),
            }],
            model: req.model,
            usage: AnthropicUsage {
                input_tokens: 12,
                output_tokens: 24,
            },
        })
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    println!("Initializing AxonFlow Anthropic Interceptor example...");

    let agent_url =
        std::env::var("AXONFLOW_AGENT_URL").unwrap_or_else(|_| "http://localhost:8080".to_string());
    let client_id = std::env::var("AXONFLOW_CLIENT_ID").expect("AXONFLOW_CLIENT_ID must be set");
    let client_secret =
        std::env::var("AXONFLOW_CLIENT_SECRET").expect("AXONFLOW_CLIENT_SECRET must be set");
    // Enterprise stacks validate user tokens as JWTs - export AXONFLOW_USER_TOKEN.
    let user_token = std::env::var("AXONFLOW_USER_TOKEN").unwrap_or_default();
    let mut config = AxonFlowConfig::new(&agent_url);
    config.client_id = Some(client_id);
    config.client_secret = Some(client_secret);
    let axon = AxonFlowClient::new(config)?;

    let raw_client = MyRawAnthropicClient;
    let governed = WrappedAnthropicClient::new(raw_client, axon, &user_token);

    println!("\nExecuting governed request via Anthropic interceptor...");

    let req = AnthropicRequest {
        model: "claude-3-5-sonnet-latest".to_string(),
        messages: vec![AnthropicMessage {
            role: "user".to_string(),
            content: "Hello from Rust Anthropic interceptor!".to_string(),
        }],
        max_tokens: 256,
        temperature: Some(0.7),
        system: Some("You are a concise assistant.".to_string()),
    };

    match governed.create_message(req).await {
        Ok(resp) => println!("✓ Request succeeded: {}", resp.id),
        Err(e) => {
            eprintln!("❌ Request blocked or failed: {}", e);
            std::process::exit(1);
        }
    }

    Ok(())
}
