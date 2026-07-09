use axonflow_sdk_rust::{AxonFlowClient, AxonFlowConfig};
use std::collections::HashMap;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Load configuration from environment variables
    let agent_url =
        std::env::var("AXONFLOW_AGENT_URL").unwrap_or_else(|_| "http://localhost:8080".to_string());
    let client_id = std::env::var("AXONFLOW_CLIENT_ID").expect("AXONFLOW_CLIENT_ID must be set");
    let client_secret =
        std::env::var("AXONFLOW_CLIENT_SECRET").expect("AXONFLOW_CLIENT_SECRET must be set");
    // Enterprise stacks (DEPLOYMENT_MODE=enterprise) validate user tokens as
    // JWTs - export AXONFLOW_USER_TOKEN. Community stacks skip JWT validation.
    let user_token = std::env::var("AXONFLOW_USER_TOKEN").unwrap_or_default();

    // Initialize client
    println!("Initializing AxonFlow client...");
    let config = AxonFlowConfig::new(agent_url).with_auth(client_id, client_secret);
    let client = AxonFlowClient::new(config)?;

    // Execute a simple query
    println!("\nExecuting governed query...");
    let mut context = HashMap::new();
    context.insert("temperature".to_string(), serde_json::json!(0.7));
    context.insert("max_tokens".to_string(), serde_json::json!(100));

    let resp = client
        .proxy_llm_call(
            &user_token,
            "What is the capital of France?",
            "chat",
            context,
        )
        .await?;

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
        let err = resp.error.unwrap_or_default();
        if err.contains("Invalid user token") {
            // Real failure: export AXONFLOW_USER_TOKEN on JWT-validating stacks.
            eprintln!("❌ Query failed: {err}");
            std::process::exit(1);
        }
        // Stacks without a working LLM provider (e.g. community CI without
        // provider keys) legitimately can't route the call - governance
        // still ran. Mirror the Java example's carve-out.
        println!("  Query non-success (expected without an LLM provider): {err}");
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

    let resp2 = client
        .proxy_llm_call(
            &user_token,
            "My email is john.doe@example.com and my SSN is 123-45-6789",
            "chat",
            HashMap::new(),
        )
        .await?;

    if resp2.blocked {
        println!("✓ PII detected and request blocked");
        println!("  Reason: {}", resp2.block_reason.unwrap_or_default());
    } else if !resp2.success {
        let err = resp2.error.unwrap_or_default();
        if err.contains("Invalid user token") {
            eprintln!("❌ PII test query failed: {err}");
            std::process::exit(1);
        }
        println!("  PII query non-success (expected without an LLM provider): {err}");
    } else {
        println!("✓ PII handled: {:?}", resp2.data);
    }

    Ok(())
}
