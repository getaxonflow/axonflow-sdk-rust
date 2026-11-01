use axonflow_sdk_rust::{AxonFlowClient, AxonFlowConfig, AxonFlowError, CacheConfig, Mode};
use std::collections::HashMap;

#[tokio::main]
async fn main() {
    let endpoint = std::env::var("AXONFLOW_HOSTED_STAGING_AGENT_URL").unwrap_or_default();
    let endpoint = endpoint.trim_end_matches('/');
    if endpoint != "https://try-staging.getaxonflow.com" {
        eprintln!("only the HTTPS Hosted staging Agent origin is allowed");
        std::process::exit(2);
    }
    let client_id = std::env::var("AXONFLOW_HOSTED_UNPAID_CLIENT_ID").unwrap_or_default();
    let secret = std::env::var("AXONFLOW_HOSTED_UNPAID_CLIENT_SECRET").unwrap_or_default();
    if client_id.is_empty() || secret.is_empty() {
        eprintln!("staging unpaid Basic credential is required");
        std::process::exit(2);
    }

    let client = AxonFlowClient::new(AxonFlowConfig {
        endpoint: endpoint.to_string(),
        client_id: Some(client_id),
        client_secret: Some(secret),
        mode: Mode::Production,
        cache: CacheConfig {
            enabled: false,
            ..Default::default()
        },
        ..Default::default()
    })
    .expect("valid staging client config");

    match client
        .proxy_llm_call(
            "hosted-sdk-runtime-e2e",
            "synthetic Hosted refusal probe",
            "chat",
            HashMap::new(),
        )
        .await
    {
        Err(AxonFlowError::ApiError {
            status: 402,
            message,
        }) if message.contains("hosted_payment_required")
            && message.contains("\"checkout_method\":\"POST\"") =>
        {
            println!("PASS: real Hosted staging 402 is a terminal Rust error");
        }
        _ => {
            eprintln!("Hosted 402 did not surface as a terminal payment error");
            std::process::exit(1);
        }
    }
}
