use async_trait::async_trait;
use axonflow_sdk_rust::interceptors::openai::{
    ChatCompletionRequest, ChatCompletionResponse, ChatMessage, OpenAIChatCompleter, Usage,
    WrappedOpenAIClient,
};
use axonflow_sdk_rust::{AxonFlowClient, AxonFlowConfig, CacheConfig, Mode, RetryConfig};
use httpmock::prelude::*;
use serde_json::json;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

struct MockOpenAI {
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl OpenAIChatCompleter for MockOpenAI {
    async fn create_chat_completion(
        &self,
        req: ChatCompletionRequest,
    ) -> Result<ChatCompletionResponse, Box<dyn std::error::Error + Send + Sync>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(ChatCompletionResponse {
            id: "openai-123".to_string(),
            object: "chat.completion".to_string(),
            created: 123456789,
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

#[tokio::test]
async fn test_openai_interceptor() {
    let server = MockServer::start();

    // 1. AxonFlow request mock
    let axon_mock = server.mock(|when, then| {
        when.method(POST).path("/api/request");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({
                "success": true,
                "request_id": "axon-req-123"
            }));
    });

    // 2. Audit mock (optional, happens in background)
    let audit_mock = server.mock(|when, then| {
        when.method(POST).path("/api/audit/llm-call");
        then.status(200)
            .json_body(json!({"success": true, "audit_id": "audit-123"}));
    });

    let config = AxonFlowConfig {
        endpoint: server.url(""),
        ..Default::default()
    };
    let client = AxonFlowClient::new(config).unwrap();

    let openai_calls = Arc::new(AtomicUsize::new(0));
    let raw_openai = MockOpenAI {
        calls: Arc::clone(&openai_calls),
    };

    let wrapped = WrappedOpenAIClient::new(raw_openai, client, "user-123");

    let req = ChatCompletionRequest {
        model: "gpt-4".to_string(),
        messages: vec![ChatMessage {
            role: "user".to_string(),
            content: "hi".to_string(),
        }],
        temperature: None,
        max_tokens: None,
    };

    let resp = wrapped.create_chat_completion(req).await.unwrap();

    assert_eq!(resp.id, "openai-123");
    assert_eq!(openai_calls.load(Ordering::SeqCst), 1);

    axon_mock.assert();

    // Give background audit a moment to fire
    tokio::time::sleep(Duration::from_millis(100)).await;
    audit_mock.assert();
}

#[tokio::test]
async fn test_proxy_llm_call_success() {
    let server = MockServer::start();

    let mock = server.mock(|when, then| {
        when.method(POST).path("/api/request").json_body(json!({
            "query": "test query",
            "user_token": "user-123",
            "client_id": "test-client",
            "request_type": "chat",
            "context": {}
        }));
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({
                "success": true,
                "result": "Test result",
                "request_id": "req-123"
            }));
    });

    let config = AxonFlowConfig {
        endpoint: server.url(""),
        client_id: Some("test-client".to_string()),
        cache: CacheConfig {
            enabled: false,
            ..Default::default()
        },
        ..Default::default()
    };
    let client = AxonFlowClient::new(config).unwrap();

    let resp = client
        .proxy_llm_call("user-123", "test query", "chat", HashMap::new())
        .await
        .unwrap();

    assert!(resp.success);
    assert_eq!(resp.result.unwrap(), "Test result");
    assert_eq!(resp.request_id.unwrap(), "req-123");
    mock.assert();
}

#[tokio::test]
async fn test_proxy_llm_call_blocked() {
    let server = MockServer::start();

    let _mock = server.mock(|when, then| {
        when.method(POST).path("/api/request");
        then.status(403) // Policy violation
            .header("content-type", "application/json")
            .json_body(json!({
                "success": false,
                "blocked": true,
                "block_reason": "PII detected"
            }));
    });

    let config = AxonFlowConfig {
        endpoint: server.url(""),
        cache: CacheConfig {
            enabled: false,
            ..Default::default()
        },
        ..Default::default()
    };
    let client = AxonFlowClient::new(config).unwrap();

    let resp = client
        .proxy_llm_call("user-123", "bad query", "chat", HashMap::new())
        .await
        .unwrap();

    assert!(!resp.success);
    assert!(resp.blocked);
    assert_eq!(resp.block_reason.unwrap(), "PII detected");
}

#[tokio::test]
async fn test_proxy_llm_call_fail_open() {
    let server = MockServer::start();

    let _mock = server.mock(|when, then| {
        when.method(POST).path("/api/request");
        then.status(503);
    });

    let config = AxonFlowConfig {
        endpoint: server.url(""),
        mode: Mode::Production,
        retry: RetryConfig {
            enabled: true,
            max_attempts: 1,
            ..Default::default()
        },
        cache: CacheConfig {
            enabled: false,
            ..Default::default()
        },
        ..Default::default()
    };
    let client = AxonFlowClient::new(config).unwrap();

    let resp = client
        .proxy_llm_call("user", "query", "chat", HashMap::new())
        .await
        .unwrap();

    assert!(resp.success);
    assert!(resp
        .error
        .unwrap()
        .contains("AxonFlow unavailable (fail-open)"));
}

#[tokio::test]
async fn test_caching() {
    let server = MockServer::start();

    let mock = server.mock(|when, then| {
        when.method(POST).path("/api/request");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({
                "success": true,
                "result": "cached"
            }));
    });

    let config = AxonFlowConfig {
        endpoint: server.url(""),
        cache: CacheConfig {
            enabled: true,
            ttl: Duration::from_secs(60),
        },
        ..Default::default()
    };
    let client = AxonFlowClient::new(config).unwrap();

    // First call
    let _ = client
        .proxy_llm_call("user", "query", "chat", HashMap::new())
        .await
        .unwrap();
    // Second call (should hit cache)
    let _ = client
        .proxy_llm_call("user", "query", "chat", HashMap::new())
        .await
        .unwrap();

    mock.assert_hits(1);
}

#[tokio::test]
async fn test_mutation_bypass_cache() {
    let server = MockServer::start();

    let mock = server.mock(|when, then| {
        when.method(POST).path("/api/request");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({
                "success": true,
                "result": "mutation"
            }));
    });

    let config = AxonFlowConfig {
        endpoint: server.url(""),
        cache: CacheConfig {
            enabled: true,
            ttl: Duration::from_secs(60),
        },
        ..Default::default()
    };
    let client = AxonFlowClient::new(config).unwrap();

    // Mutations should never be cached
    let _ = client
        .proxy_llm_call("user", "query", "execute-plan", HashMap::new())
        .await
        .unwrap();
    let _ = client
        .proxy_llm_call("user", "query", "execute-plan", HashMap::new())
        .await
        .unwrap();

    mock.assert_hits(2);
}

#[tokio::test]
async fn test_retry_logic() {
    let server = MockServer::start();

    let _mock = server.mock(|when, then| {
        when.method(POST).path("/api/request");
        then.status(500);
    });

    let config = AxonFlowConfig {
        endpoint: server.url(""),
        mode: Mode::Sandbox, // Disable fail-open to see the error
        retry: RetryConfig {
            enabled: true,
            max_attempts: 2,
            initial_delay: Duration::from_millis(1),
        },
        cache: CacheConfig {
            enabled: false,
            ..Default::default()
        },
        ..Default::default()
    };
    let client = AxonFlowClient::new(config).unwrap();

    let result = client
        .proxy_llm_call("user", "query", "chat", HashMap::new())
        .await;

    assert!(result.is_err());
}

#[tokio::test]
async fn test_list_connectors() {
    let server = MockServer::start();

    let mock = server.mock(|when, then| {
        when.method(GET).path("/api/v1/connectors");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({
                "connectors": [
                    {
                        "id": "conn-1",
                        "name": "Postgres",
                        "type": "database",
                        "version": "1.0",
                        "description": "desc",
                        "category": "db",
                        "icon": "icon",
                        "tags": [],
                        "capabilities": [],
                        "config_schema": {},
                        "installed": true
                    }
                ],
                "total": 1
            }));
    });

    let config = AxonFlowConfig {
        endpoint: server.url(""),
        ..Default::default()
    };
    let client = AxonFlowClient::new(config).unwrap();

    let connectors = client.list_connectors().await.unwrap();

    assert_eq!(connectors.len(), 1);
    assert_eq!(connectors[0].name, "Postgres");
    mock.assert();
}

#[tokio::test]
async fn test_generate_plan() {
    let server = MockServer::start();

    let _mock = server.mock(|when, then| {
        when.method(POST).path("/api/request");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({
                "success": true,
                "data": {
                    "plan_id": "plan-999",
                    "status": "pending",
                    "steps": [],
                    "domain": "it",
                    "complexity": 5,
                    "parallel": false,
                    "estimated_duration": "10s",
                    "metadata": {}
                }
            }));
    });

    let config = AxonFlowConfig {
        endpoint: server.url(""),
        ..Default::default()
    };
    let client = AxonFlowClient::new(config).unwrap();

    let plan = client
        .generate_plan("do something", "it", None)
        .await
        .unwrap();

    assert_eq!(plan.plan_id, "plan-999");
    assert_eq!(plan.domain, "it");
}
