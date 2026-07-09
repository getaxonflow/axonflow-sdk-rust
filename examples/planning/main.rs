use axonflow_sdk_rust::{AxonFlowClient, AxonFlowConfig, CacheConfig, RetryConfig};
use std::time::Duration;

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

    // Initialize client with advanced configuration
    println!("Initializing AxonFlow client with advanced features...");
    let config = AxonFlowConfig {
        endpoint: agent_url,
        client_id: Some(client_id),
        client_secret: Some(client_secret),
        timeout: Duration::from_secs(90), // Longer timeout for planning
        retry: RetryConfig {
            enabled: true,
            max_attempts: 3,
            initial_delay: Duration::from_secs(1),
        },
        cache: CacheConfig {
            enabled: true,
            ttl: Duration::from_secs(120),
            ..Default::default()
        },
        ..Default::default()
    };
    let client = AxonFlowClient::new(config)?;

    // Step 1: Generate Multi-Agent Plan
    println!("\n{}", "=".repeat(60));
    println!("Step 1: Generate Multi-Agent Plan");
    println!("{}", "=".repeat(60));

    let goal = "Plan a 3-day business trip to Paris with meetings at La Défense";
    println!("Goal: {}\n", goal);

    let plan = client
        .generate_plan(goal, "travel", Some(&user_token))
        .await?;

    println!("✓ Plan generated successfully!");
    println!("  Plan ID: {}", plan.plan_id);
    println!("  Steps: {}", plan.steps.len());
    println!("  Complexity: {}/10", plan.complexity);
    println!("  Estimated duration: {}", plan.estimated_duration);

    // Step 2: Execute Plan
    println!("\n{}", "=".repeat(60));
    println!("Step 2: Execute Plan");
    println!("{}", "=".repeat(60));

    println!("Executing plan...");
    let start_time = std::time::Instant::now();
    let exec_resp = client
        .execute_plan(&plan.plan_id, Some(&user_token))
        .await?;

    println!("\n✓ Plan execution completed in {:?}", start_time.elapsed());
    println!("  Status: {}", exec_resp.status);

    if exec_resp.status == "completed" {
        println!("\nPlan Result: {:?}", exec_resp.result);
    } else {
        println!(
            "❌ Plan execution failed: {}",
            exec_resp.error.unwrap_or_default()
        );
    }

    Ok(())
}
