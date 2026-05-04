use axonflow_sdk_rust::{AxonFlowClient, AxonFlowConfig, ConnectorInstallRequest};
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

    if let (Some(key), Some(secret)) = (amadeus_key, amadeus_secret) {
        println!("Installing Amadeus connector...");

        let mut options = HashMap::new();
        options.insert("environment".to_string(), serde_json::json!("production"));
        
        let mut credentials = HashMap::new();
        credentials.insert("api_key".to_string(), key);
        credentials.insert("api_secret".to_string(), secret);

        let install_req = ConnectorInstallRequest {
            connector_id: "amadeus-travel".to_string(),
            name: "amadeus-prod".to_string(),
            tenant_id: "demo-tenant".to_string(),
            options,
            credentials,
        };

        match client.install_connector(install_req).await {
            Ok(_) => println!("✓ Connector installed successfully!"),
            Err(e) => println!("Failed to install connector: {}", e),
        }
    } else {
        println!("⚠ Skipping connector installation (AMADEUS_API_KEY and AMADEUS_API_SECRET not set)");
    }

    // Step 3: Query Connector
    println!("\n{}", "=".repeat(60));
    println!("Step 3: Query Connector");
    println!("{}", "=".repeat(60));

    // Query Redis (if available)
    println!("Querying Redis connector...");
    let mut params = HashMap::new();
    params.insert("key".to_string(), serde_json::json!("user:123:preferences"));

    let resp = client.query_connector(
        "user-123",
        "redis-cache",
        "Get cached user preferences for user-123",
        params,
    ).await;

    match resp {
        Ok(r) => {
            if r.success {
                println!("✓ Redis data retrieved: {:?}", r.data);
            } else {
                println!("⚠ Query failed: {}", r.error.unwrap_or_default());
            }
        },
        Err(e) => println!("⚠ Redis query failed (expected if not installed): {}", e),
    }

    Ok(())
}
