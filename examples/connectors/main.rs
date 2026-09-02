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
    // The Redis connector connects FROM the platform (orchestrator), not from
    // this process. On the docker-compose stack the Redis service is reachable
    // as "redis"; override for other topologies.
    let redis_host = std::env::var("AXONFLOW_REDIS_HOST").unwrap_or_else(|_| "redis".to_string());
    let redis_port: u16 = std::env::var("AXONFLOW_REDIS_PORT")
        .unwrap_or_else(|_| "6379".to_string())
        .parse()
        .expect("AXONFLOW_REDIS_PORT must be a number");

    let mut failed = false;
    // Community-edition stacks run connectors from config files and have no
    // DB persistence for marketplace installs, so the install→query arc is
    // skipped there rather than failed.
    let mut install_arc = true;

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

    // Step 2: Install a Connector. Redis ships with the docker-compose stack,
    // so the install→query arc runs end-to-end with no external service or
    // paid credentials. (Earlier revisions installed the Amadeus travel
    // connector here; Amadeus decommissioned its self-service APIs on
    // 2026-07-17, so an example pinned to it can never succeed again.)
    println!("{}", "=".repeat(60));
    println!("Step 2: Install Redis Connector");
    println!("{}", "=".repeat(60));

    let redis_installed = connectors
        .iter()
        .any(|c| c.r#type == "redis" && c.installed);

    if redis_installed {
        // Keep the example re-runnable: the platform rejects duplicate
        // registrations, so don't re-install an already-installed connector.
        println!("✓ Redis connector already installed - skipping install");
    } else {
        println!(
            "Installing Redis connector (host={} port={})...",
            redis_host, redis_port
        );

        let mut options = HashMap::new();
        options.insert("host".to_string(), serde_json::json!(redis_host));
        options.insert("port".to_string(), serde_json::json!(redis_port));

        let install_req = ConnectorInstallRequest {
            connector_id: "redis-cache".to_string(),
            name: "redis-cache".to_string(),
            tenant_id: tenant_id.clone(),
            options,
            credentials: HashMap::new(),
        };

        match client.install_connector(install_req).await {
            Ok(_) => println!("✓ Connector installed successfully!"),
            Err(e) if e.to_string().contains("Failed to persist connector config") => {
                println!("⚠ This stack cannot persist connector installs (community edition");
                println!("  runs connectors from config files) - skipping the install/query arc");
                install_arc = false;
            }
            Err(e) => {
                println!("⚠ Failed to install connector: {}", e);
                failed = true;
            }
        }
    }

    // Step 3: Query Connector
    println!("\n{}", "=".repeat(60));
    println!("Step 3: Query Connector");
    println!("{}", "=".repeat(60));

    if !install_arc {
        println!("Skipped (connector install is not available on this stack).");
        println!("\n✅ Connector examples completed (listing only on this edition)");
        return Ok(());
    }

    // The Redis connector takes the operation as the query statement
    // (GET / EXISTS / TTL / KEYS / STATS) with the key in params - it does
    // not parse natural language.
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
                failed = true;
            }
        }
        Err(e) => {
            println!("⚠ Redis query failed: {}", e);
            failed = true;
        }
    }

    if failed {
        println!("\n⚠ Connector examples completed with failures");
        std::process::exit(1);
    }
    println!("\n✅ Connector examples completed");

    Ok(())
}
