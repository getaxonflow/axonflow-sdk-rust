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
    let axon = AxonFlowClient::new(AxonFlowConfig::new("http://localhost:8080"))?;

    // 2. Wrap your LLM client
    let raw_client = MyRawOpenAIClient;
    let governed_client = WrappedOpenAIClient::new(raw_client, axon, "user-789");

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
        Err(e) => println!("❌ Request blocked or failed: {}", e),
    }

    Ok(())
}
