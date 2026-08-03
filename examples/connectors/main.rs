use axonflow_sdk_rust::{AxonFlowClient, AxonFlowConfig, ConnectorInstallRequest};
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
    // Connector installs are tenant-scoped: the tenant must exist on the
    // stack, so default to the caller's own tenant (== client_id) instead
    // of a made-up one that trips the tenant FK.
    let tenant_id = std::env::var("AXONFLOW_TENANT_ID").unwrap_or_else(|_| client_id.clone());

    // Initialize client
    println!("Initializing AxonFlow client...");
    let config = AxonFlowConfig::new(agent_url).with_auth(client_id, client_secret);
    let client = AxonFlowClient::new(config)?;

    // Step 1: List Available Connectors
    println!("\n{}", "=".repeat(60));
    println!("Step 1: List Available Connectors");
    println!("{}", "=".repeat(60));

    let connectors = client.list_connectors().await?;

    println!("Found {} connectors:\n", connectors.len());
    for (i, conn) in connectors.iter().enumerate() {
        println!("{}. {} ({})", i + 1, conn.name, conn.r#type);
        println!("   Description: {}", conn.description);
        println!("   Version: {}", conn.version);
        println!("   Installed: {}", conn.installed);
        if let Some(instance) = &conn.instance_name {
            println!("   Instance Name: {}", instance);
        }
        println!();
    }

    // Step 2: Install a Connector (Example: Amadeus)
    println!("{}", "=".repeat(60));
    println!("Step 2: Install Amadeus Travel Connector");
    println!("{}", "=".repeat(60));

    let amadeus_key = std::env::var("AMADEUS_API_KEY").ok();
    let amadeus_secret = std::env::var("AMADEUS_API_SECRET").ok();
    let amadeus_installed = connectors
        .iter()
        .any(|c| c.r#type == "amadeus" && c.installed);

    if amadeus_installed {
        // Keep the example re-runnable: the platform rejects duplicate
        // registrations, so don't re-install an already-installed connector.
        println!("✓ Amadeus connector already installed - skipping install");
    } else if let (Some(key), Some(secret)) = (amadeus_key, amadeus_secret) {
        println!("Installing Amadeus connector...");

        // Amadeus self-service keys are test-environment keys; production
        // keys require an Amadeus production agreement. Default to "test"
        // and let AMADEUS_ENVIRONMENT=production override.
        let amadeus_env =
            std::env::var("AMADEUS_ENVIRONMENT").unwrap_or_else(|_| "test".to_string());
        let mut options = HashMap::new();
        options.insert("environment".to_string(), serde_json::json!(amadeus_env));

        let mut credentials = HashMap::new();
        credentials.insert("api_key".to_string(), key);
        credentials.insert("api_secret".to_string(), secret);

        let install_req = ConnectorInstallRequest {
            connector_id: "amadeus-travel".to_string(),
            name: "amadeus-prod".to_string(),
            tenant_id: tenant_id.clone(),
            options,
            credentials,
        };

        match client.install_connector(install_req).await {
            Ok(_) => println!("✓ Connector installed successfully!"),
            Err(e) => println!("Failed to install connector: {}", e),
        }
    } else {
        println!(
            "⚠ Skipping connector installation (AMADEUS_API_KEY and AMADEUS_API_SECRET not set)"
        );
    }

    // Step 3: Query Connector
    println!("\n{}", "=".repeat(60));
    println!("Step 3: Query Connector");
    println!("{}", "=".repeat(60));

    // Query Redis (if available). The Redis connector takes the operation
    // as the query statement (GET / EXISTS / TTL / KEYS / STATS) with the
    // key in params - it does not parse natural language.
    println!("Querying Redis connector...");
    let mut params = HashMap::new();
    params.insert("key".to_string(), serde_json::json!("user:123:preferences"));

    let resp = client
        .query_connector(&user_token, "redis-cache", "GET", params)
        .await;

    match resp {
        Ok(r) => {
            if r.success {
                println!("✓ Redis data retrieved: {:?}", r.data);
            } else {
                println!("⚠ Query failed: {}", r.error.unwrap_or_default());
            }
        }
        Err(e) => println!("⚠ Redis query failed (expected if not installed): {}", e),
    }

    Ok(())
}
