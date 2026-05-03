use axonflow_sdk_rust::{AxonFlowClient, AxonFlowConfig};
use std::collections::HashMap;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Load configuration from environment variables
    let agent_url = std::env::var("AXONFLOW_AGENT_URL")
        .unwrap_or_else(|_| "http://localhost:8080".to_string());
    let client_id = std::env::var("AXONFLOW_CLIENT_ID")
        .expect("AXONFLOW_CLIENT_ID must be set");
    let client_secret = std::env::var("AXONFLOW_CLIENT_SECRET")
        .expect("AXONFLOW_CLIENT_SECRET must be set");

    // Initialize client
    println!("Initializing AxonFlow client...");
    let config = AxonFlowConfig::new(agent_url)
        .with_auth(client_id, client_secret);
    let client = AxonFlowClient::new(config)?;

    // Execute a simple query
    println!("\nExecuting governed query...");
    let mut context = HashMap::new();
    context.insert("temperature".to_string(), serde_json::json!(0.7));
    context.insert("max_tokens".to_string(), serde_json::json!(100));

    let resp = client.proxy_llm_call(
        "", // SDK auto-populates user_token (defaults to "anonymous")
        "What is the capital of France?",
        "chat",
        context,
    ).await?;

    // Check if request was blocked
    if resp.blocked {
        println!("❌ Request blocked by governance policy");
        println!("   Reason: {}", resp.block_reason.unwrap_or_default());
        if let Some(info) = resp.policy_info {
            println!("   Policies evaluated: {:?}", info.policies_evaluated);
        }
        return Ok(());
    }

    // Check if request succeeded
    if !resp.success {
        println!("❌ Query failed: {}", resp.error.unwrap_or_default());
        return Ok(());
    }

    // Display result
    println!("✓ Query executed successfully");
    println!("Result: {:?}", resp.data);

    // Display governance metadata
    println!("\nGovernance Metadata:");
    println!("  Request ID: {}", resp.request_id.unwrap_or_default());
    if let Some(info) = resp.policy_info {
        println!("  Policies Evaluated: {:?}", info.policies_evaluated);
        println!("  Processing Time: {}", info.processing_time);
    }

    // Test with sensitive data (should be redacted)
    println!("\n{}", "=".repeat(60));
    println!("Testing PII detection and redaction...");
    println!("{}", "=".repeat(60));

    let resp2 = client.proxy_llm_call(
        "",
        "My email is john.doe@example.com and my SSN is 123-45-6789",
        "chat",
        HashMap::new(),
    ).await?;

    if resp2.blocked {
        println!("✓ PII detected and request blocked");
        println!("  Reason: {}", resp2.block_reason.unwrap_or_default());
    } else {
        println!("✓ PII handled: {:?}", resp2.data);
    }

    Ok(())
}
