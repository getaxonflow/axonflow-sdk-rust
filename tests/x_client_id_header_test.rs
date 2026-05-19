//! X-Client-ID + X-Axonflow-Client header verification (v9 identity + ADR-050 §4).
//!
//! Every governed request carries `X-Client-ID` alongside Basic Auth, and
//! every request additionally carries `X-Axonflow-Client: sdk-rust/<version>`
//! so the agent can derive request scope. The agent's apiAuthMiddleware
//! overwrites `X-Client-ID` with its own auth-derived value, so a missing or
//! wrong client-side header is harmless server-side. These tests pin
//! SDK-emitted behaviour so future regressions are caught early.

use axonflow_sdk_rust::{AxonFlowClient, AxonFlowConfig};
use serde_json::json;
use std::collections::HashMap;
use wiremock::matchers::{header, header_exists, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn ok_response() -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "success": true,
        "request_id": "rid"
    }))
}

#[tokio::test]
async fn x_client_id_community_default() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/request"))
        .and(header("X-Client-ID", "community"))
        .respond_with(ok_response())
        .expect(1)
        .mount(&server)
        .await;

    let config = AxonFlowConfig {
        endpoint: server.uri(),
        ..Default::default()
    };
    let client = AxonFlowClient::new(config).unwrap();
    let _ = client
        .proxy_llm_call("", "ping", "chat", HashMap::new())
        .await;
}

#[tokio::test]
async fn x_client_id_configured_client() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/request"))
        .and(header("X-Client-ID", "acme-corp"))
        .respond_with(ok_response())
        .expect(1)
        .mount(&server)
        .await;

    let config = AxonFlowConfig {
        endpoint: server.uri(),
        client_id: Some("acme-corp".to_string()),
        client_secret: Some("secret".to_string()),
        ..Default::default()
    };
    let client = AxonFlowClient::new(config).unwrap();
    let _ = client
        .proxy_llm_call("", "ping", "chat", HashMap::new())
        .await;
}

#[tokio::test]
async fn x_axonflow_client_present_adr_050() {
    // ADR-050 §4: every governed request carries X-Axonflow-Client.
    // This header was MISSING in v0.2.0 (pre-existing gap); v0.3.0 fixes it.
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/request"))
        .and(header_exists("X-Axonflow-Client"))
        .respond_with(ok_response())
        .expect(1)
        .mount(&server)
        .await;

    let config = AxonFlowConfig {
        endpoint: server.uri(),
        ..Default::default()
    };
    let client = AxonFlowClient::new(config).unwrap();
    let _ = client
        .proxy_llm_call("", "ping", "chat", HashMap::new())
        .await;
}

#[tokio::test]
async fn x_axonflow_client_value_shape() {
    // Pin the value shape: "sdk-rust/<semver>" — matches cross-SDK
    // contract. Agent's deriveScopeFromClientHeader splits on '/' and
    // maps "sdk-*" prefixes to scope=sdk.
    let server = MockServer::start().await;
    let expected = format!("sdk-rust/{}", env!("CARGO_PKG_VERSION"));
    Mock::given(method("POST"))
        .and(path("/api/request"))
        .and(header("X-Axonflow-Client", expected.as_str()))
        .respond_with(ok_response())
        .expect(1)
        .mount(&server)
        .await;

    let config = AxonFlowConfig {
        endpoint: server.uri(),
        ..Default::default()
    };
    let client = AxonFlowClient::new(config).unwrap();
    let _ = client
        .proxy_llm_call("", "ping", "chat", HashMap::new())
        .await;
}
