use async_trait::async_trait;
use axonflow_sdk_rust::interceptors::anthropic::{
    AnthropicContent, AnthropicMessage, AnthropicMessageCreator, AnthropicRequest,
    AnthropicResponse, AnthropicUsage, WrappedAnthropicClient,
};
use axonflow_sdk_rust::{AxonFlowClient, AxonFlowConfig};
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

struct MockAnthropic {
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl AnthropicMessageCreator for MockAnthropic {
    async fn create_message(
        &self,
        req: AnthropicRequest,
    ) -> Result<AnthropicResponse, Box<dyn std::error::Error + Send + Sync>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(AnthropicResponse {
            id: "msg_123".to_string(),
            r#type: "message".to_string(),
            role: "assistant".to_string(),
            content: vec![AnthropicContent {
                r#type: "text".to_string(),
                text: "Hello from mock Claude".to_string(),
            }],
            model: req.model,
            usage: AnthropicUsage {
                input_tokens: 10,
                output_tokens: 20,
            },
        })
    }
}

#[tokio::test]
async fn test_anthropic_interceptor() {
    let server = MockServer::start().await;

    // 1. AxonFlow request mock
    // Verifies: Correct path, method, headers, and JSON body structure
    Mock::given(method("POST"))
        .and(path("/api/request"))
        .and(header("authorization", "Basic Y29tbXVuaXR5Og==")) // community default
        .and(wiremock::matchers::body_partial_json(json!({
            "query": "System: You are a helpful assistant\nuser: Hello!",
            "user_token": "user-123",
            "request_type": "llm_chat",
            "context": {
                "provider": "anthropic",
                "model": "claude-3-sonnet-20240229",
                "max_tokens": 1024
            }
        })))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/json")
                .set_body_json(json!({
                    "success": true,
                    "request_id": "axon-req-anthropic-123"
                })),
        )
        .expect(1)
        .mount(&server)
        .await;

    // 2. Audit mock
    // Verifies: Correct async audit payload including usage and latency
    Mock::given(method("POST"))
        .and(path("/api/audit/llm-call"))
        .and(wiremock::matchers::body_partial_json(json!({
            "context_id": "axon-req-anthropic-123",
            "client_id": "community",
            "response_summary": "Hello from mock Claude",
            "provider": "anthropic",
            "model": "claude-3-sonnet-20240229",
            "token_usage": {
                "prompt_tokens": 10,
                "completion_tokens": 20,
                "total_tokens": 30
            }
        })))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"success": true, "audit_id": "audit-anthropic-123"})),
        )
        .expect(1)
        .mount(&server)
        .await;

    let config = AxonFlowConfig {
        endpoint: server.uri(),
        ..Default::default()
    };
    let client = AxonFlowClient::new(config).unwrap();

    let anthropic_calls = Arc::new(AtomicUsize::new(0));
    let raw_anthropic = MockAnthropic {
        calls: Arc::clone(&anthropic_calls),
    };

    let wrapped = WrappedAnthropicClient::new(raw_anthropic, client, "user-123");

    let req = AnthropicRequest {
        model: "claude-3-sonnet-20240229".to_string(),
        messages: vec![AnthropicMessage {
            role: "user".to_string(),
            content: "Hello!".to_string(),
        }],
        max_tokens: 1024,
        temperature: None,
        system: Some("You are a helpful assistant".to_string()),
    };

    let resp = wrapped.create_message(req).await.unwrap();

    assert_eq!(resp.id, "msg_123");
    assert_eq!(resp.content[0].text, "Hello from mock Claude");
    assert_eq!(anthropic_calls.load(Ordering::SeqCst), 1);

    // Give background audit a moment to fire
    tokio::time::sleep(Duration::from_millis(150)).await;
}

#[tokio::test]
async fn test_anthropic_interceptor_blocked() {
    let server = MockServer::start().await;

    // 1. AxonFlow request mock (Blocked)
    Mock::given(method("POST"))
        .and(path("/api/request"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/json")
                .set_body_json(json!({
                    "success": false,
                    "blocked": true,
                    "block_reason": "Policy violation: PII found"
                })),
        )
        .expect(1)
        .mount(&server)
        .await;

    let config = AxonFlowConfig {
        endpoint: server.uri(),
        ..Default::default()
    };
    let client = AxonFlowClient::new(config).unwrap();

    let anthropic_calls = Arc::new(AtomicUsize::new(0));
    let raw_anthropic = MockAnthropic {
        calls: Arc::clone(&anthropic_calls),
    };

    let wrapped = WrappedAnthropicClient::new(raw_anthropic, client, "user-123");

    let req = AnthropicRequest {
        model: "claude-3-sonnet-20240229".to_string(),
        messages: vec![AnthropicMessage {
            role: "user".to_string(),
            content: "My SSN is 000-00-0000".to_string(),
        }],
        max_tokens: 1024,
        temperature: None,
        system: None,
    };

    let result = wrapped.create_message(req).await;

    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("PII found"));

    // Ensure the actual Anthropic call was NEVER made
    assert_eq!(anthropic_calls.load(Ordering::SeqCst), 0);
}
