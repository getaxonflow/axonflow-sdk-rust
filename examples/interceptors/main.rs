use async_trait::async_trait;
use axonflow_sdk_rust::interceptors::openai::{
    ChatCompletionRequest, ChatCompletionResponse, ChatMessage, OpenAIChatCompleter, Usage,
    WrappedOpenAIClient,
};
use axonflow_sdk_rust::{AxonFlowClient, AxonFlowConfig};

// Example of a raw OpenAI client implementation
struct MyRawOpenAIClient;

#[async_trait]
impl OpenAIChatCompleter for MyRawOpenAIClient {
    async fn create_chat_completion(
        &self,
        req: ChatCompletionRequest,
    ) -> Result<ChatCompletionResponse, Box<dyn std::error::Error + Send + Sync>> {
        println!(
            "  [Underlying Client] Calling OpenAI API for model: {}",
            req.model
        );

        // Mock response
        Ok(ChatCompletionResponse {
            id: "chatcmpl-123".to_string(),
            object: "chat.completion".to_string(),
            created: 1677652288,
            model: req.model,
            choices: vec![],
            usage: Usage {
                prompt_tokens: 10,
                completion_tokens: 20,
                total_tokens: 30,
            },
        })
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    println!("Initializing AxonFlow Interceptor example...");

    // 1. Initialize AxonFlow
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

    // 2. Wrap your LLM client
    let raw_client = MyRawOpenAIClient;
    let governed_client = WrappedOpenAIClient::new(raw_client, axon, &user_token);

    println!("\nExecuting governed request via Interceptor...");

    // 3. Use as normal - governance is now "invisible"
    let req = ChatCompletionRequest {
        model: "gpt-4".to_string(),
        messages: vec![ChatMessage {
            role: "user".to_string(),
            content: "Hello from Rust interceptor!".to_string(),
        }],
        temperature: Some(0.7),
        max_tokens: Some(50),
    };

    match governed_client.create_chat_completion(req).await {
        Ok(resp) => println!("✓ Request succeeded: {}", resp.id),
        Err(e) => {
            eprintln!("❌ Request blocked or failed: {}", e);
            std::process::exit(1);
        }
    }

    Ok(())
}
