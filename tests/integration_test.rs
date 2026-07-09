use async_trait::async_trait;
use axonflow_sdk_rust::interceptors::openai::{
    ChatCompletionRequest, ChatCompletionResponse, ChatMessage, OpenAIChatCompleter, Usage,
    WrappedOpenAIClient,
};
use axonflow_sdk_rust::{AxonFlowClient, AxonFlowConfig, CacheConfig, Mode, RetryConfig};
use base64::Engine as _;
use serde_json::json;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

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
    let server = MockServer::start().await;

    // 1. AxonFlow request mock
    Mock::given(method("POST"))
        .and(path("/api/request"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/json")
                .set_body_json(json!({
                    "success": true,
                    "request_id": "axon-req-123"
                })),
        )
        .expect(1)
        .mount(&server)
        .await;

    // 2. Audit mock (optional, happens in background)
    Mock::given(method("POST"))
        .and(path("/api/audit/llm-call"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"success": true, "audit_id": "audit-123"})),
        )
        .expect(1)
        .mount(&server)
        .await;

    let config = AxonFlowConfig {
        endpoint: server.uri(),
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

    // Give background audit a moment to fire
    tokio::time::sleep(Duration::from_millis(100)).await;
}

#[tokio::test]
async fn test_proxy_llm_call_success() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/request"))
        .and(wiremock::matchers::body_json(json!({
            "query": "test query",
            "user_token": "user-123",
            "client_id": "test-client",
            "request_type": "chat",
            "context": {}
        })))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/json")
                .set_body_json(json!({
                    "success": true,
                    "result": "Test result",
                    "request_id": "req-123"
                })),
        )
        .expect(1)
        .mount(&server)
        .await;

    let config = AxonFlowConfig {
        endpoint: server.uri(),
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
}

#[tokio::test]
async fn test_proxy_llm_call_blocked() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/request"))
        .respond_with(
            ResponseTemplate::new(403) // Policy violation
                .insert_header("content-type", "application/json")
                .set_body_json(json!({
                    "success": false,
                    "blocked": true,
                    "block_reason": "PII detected"
                })),
        )
        .expect(1)
        .mount(&server)
        .await;

    let config = AxonFlowConfig {
        endpoint: server.uri(),
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
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/request"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;

    let config = AxonFlowConfig {
        endpoint: server.uri(),
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
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/request"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/json")
                .set_body_json(json!({
                    "success": true,
                    "result": "cached"
                })),
        )
        .expect(1) // Should only be called once
        .mount(&server)
        .await;

    let config = AxonFlowConfig {
        endpoint: server.uri(),
        cache: CacheConfig {
            enabled: true,
            ttl: Duration::from_secs(60),
            ..Default::default()
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
}

#[tokio::test]
async fn test_mutation_bypass_cache() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/request"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/json")
                .set_body_json(json!({
                    "success": true,
                    "result": "mutation"
                })),
        )
        .expect(2) // Mutations should never be cached
        .mount(&server)
        .await;

    let config = AxonFlowConfig {
        endpoint: server.uri(),
        cache: CacheConfig {
            enabled: true,
            ttl: Duration::from_secs(60),
            ..Default::default()
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
}

#[tokio::test]
async fn test_retry_logic() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/request"))
        .respond_with(ResponseTemplate::new(500))
        .expect(2)
        .mount(&server)
        .await;

    let config = AxonFlowConfig {
        endpoint: server.uri(),
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

// Regression test for issue #2275: 401 must be terminal — the SDK MUST
// NOT retry an auth failure, because retrying with the same invalid token
// just compounds the storm on the agent. `.expect(1)` makes wiremock fail
// the test (panic on Drop) if the SDK ever calls the endpoint more than
// once for the same auth failure.
#[tokio::test]
async fn test_401_not_retried_issue_2275() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/request"))
        .respond_with(ResponseTemplate::new(401).set_body_string("unauthorized"))
        .expect(1)
        .mount(&server)
        .await;

    let config = AxonFlowConfig {
        endpoint: server.uri(),
        mode: Mode::Sandbox, // Disable fail-open so 401 surfaces as Err
        retry: RetryConfig {
            enabled: true,
            max_attempts: 3,
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

    assert!(result.is_err(), "401 must propagate as an error");
}

// Companion to `test_401_not_retried_issue_2275` — locks in the OTHER
// direction of the retry allowlist: 429 (rate limit) MUST trigger retry
// up to `max_attempts`. Without this, a future refactor that drops
// `*status != 429` from `execute_with_retry` would silently make every
// 4xx terminal, breaking the rate-limit retry contract; the 401-not-
// retried test alone wouldn't catch that flip (401 stays terminal
// either way). `.expect(3)` makes wiremock fail the test (panic on
// Drop) if the SDK fails to retry the documented number of times.
#[tokio::test]
async fn test_429_is_retried_allowlist_contract() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/request"))
        .respond_with(ResponseTemplate::new(429).set_body_string("rate limited"))
        .expect(3)
        .mount(&server)
        .await;

    let config = AxonFlowConfig {
        endpoint: server.uri(),
        mode: Mode::Sandbox, // Disable fail-open so 429 surfaces as Err after exhausting retries
        retry: RetryConfig {
            enabled: true,
            max_attempts: 3,
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

    assert!(
        result.is_err(),
        "429 should propagate as an error after exhausting all retry attempts"
    );
}

#[tokio::test]
async fn test_list_connectors() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/v1/connectors"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/json")
                .set_body_json(json!({
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

    let connectors = client.list_connectors().await.unwrap();

    assert_eq!(connectors.len(), 1);
    assert_eq!(connectors[0].name, "Postgres");
}

#[tokio::test]
async fn test_generate_plan() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/request"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/json")
                .set_body_json(json!({
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

    let plan = client
        .generate_plan("do something", "it", None)
        .await
        .unwrap();

    assert_eq!(plan.plan_id, "plan-999");
    assert_eq!(plan.domain, "it");
}

// ============================================================================
// Auth header tests — see axonflow-sdk-go selfhosted_auth_headers_test.go
// ============================================================================

#[tokio::test]
async fn test_auth_defaults_to_community() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/request"))
        .and(header("authorization", "Basic Y29tbXVuaXR5Og==")) // base64("community:")
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "success": true,
            "result": "ok"
        })))
        .expect(1)
        .mount(&server)
        .await;

    let config = AxonFlowConfig {
        endpoint: server.uri(),
        cache: CacheConfig {
            enabled: false,
            ..Default::default()
        },
        ..Default::default()
    };
    let client = AxonFlowClient::new(config).unwrap();
    let _ = client
        .proxy_llm_call("user", "query", "chat", HashMap::new())
        .await
        .unwrap();
}

#[tokio::test]
async fn test_auth_basic_with_credentials() {
    let server = MockServer::start().await;
    let expected = format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(b"my-client:my-secret".as_slice())
    );
    Mock::given(method("POST"))
        .and(path("/api/request"))
        .and(header("authorization", &expected))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "success": true,
            "result": "ok"
        })))
        .expect(1)
        .mount(&server)
        .await;

    let config = AxonFlowConfig {
        endpoint: server.uri(),
        client_id: Some("my-client".to_string()),
        client_secret: Some("my-secret".to_string()),
        cache: CacheConfig {
            enabled: false,
            ..Default::default()
        },
        ..Default::default()
    };
    let client = AxonFlowClient::new(config).unwrap();
    let _ = client
        .proxy_llm_call("user", "query", "chat", HashMap::new())
        .await
        .unwrap();
}

#[tokio::test]
async fn test_auth_clientid_only_empty_secret() {
    let server = MockServer::start().await;
    let expected = format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(b"my-client:".as_slice())
    );
    Mock::given(method("POST"))
        .and(path("/api/request"))
        .and(header("authorization", &expected))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "success": true,
            "result": "ok"
        })))
        .expect(1)
        .mount(&server)
        .await;

    let config = AxonFlowConfig {
        endpoint: server.uri(),
        client_id: Some("my-client".to_string()),
        cache: CacheConfig {
            enabled: false,
            ..Default::default()
        },
        ..Default::default()
    };
    let client = AxonFlowClient::new(config).unwrap();
    let _ = client
        .proxy_llm_call("user", "query", "chat", HashMap::new())
        .await
        .unwrap();
}

#[tokio::test]
async fn test_license_key_header_when_set() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/request"))
        .and(header("x-license-key", "test-license-abc-123"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "success": true,
            "result": "ok"
        })))
        .expect(1)
        .mount(&server)
        .await;

    let config = AxonFlowConfig {
        endpoint: server.uri(),
        client_id: Some("my-client".to_string()),
        client_secret: Some("my-secret".to_string()),
        license_key: Some("test-license-abc-123".to_string()),
        cache: CacheConfig {
            enabled: false,
            ..Default::default()
        },
        ..Default::default()
    };
    let client = AxonFlowClient::new(config).unwrap();
    let _ = client
        .proxy_llm_call("user", "query", "chat", HashMap::new())
        .await
        .unwrap();
}

#[tokio::test]
async fn test_no_license_key_header_when_unset() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/request"))
        .and(move |req: &wiremock::Request| {
            !req.headers
                .iter()
                .any(|(k, _)| k.as_str().eq_ignore_ascii_case("x-license-key"))
        })
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "success": true,
            "result": "ok"
        })))
        .expect(1)
        .mount(&server)
        .await;

    let config = AxonFlowConfig {
        endpoint: server.uri(),
        cache: CacheConfig {
            enabled: false,
            ..Default::default()
        },
        ..Default::default()
    };
    let client = AxonFlowClient::new(config).unwrap();
    let _ = client
        .proxy_llm_call("user", "query", "chat", HashMap::new())
        .await
        .unwrap();
}

// ============================================================================
// Endpoint path tests — verify Phase 0 corrections
// ============================================================================

#[tokio::test]
async fn test_install_connector_uses_install_subpath() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/connectors/postgres/install"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;

    let config = AxonFlowConfig {
        endpoint: server.uri(),
        ..Default::default()
    };
    let client = AxonFlowClient::new(config).unwrap();

    let req = axonflow_sdk_rust::ConnectorInstallRequest {
        connector_id: "postgres".to_string(),
        name: "pg-prod".to_string(),
        tenant_id: "demo".to_string(),
        options: HashMap::new(),
        credentials: HashMap::new(),
    };
    client.install_connector(req).await.unwrap();
}

#[tokio::test]
async fn test_get_plan_status_uses_singular_path() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/plan/plan42"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "plan_id": "plan42",
            "status": "completed",
            "duration": "1s",
            "completed_steps": 1,
            "total_steps": 1
        })))
        .expect(1)
        .mount(&server)
        .await;

    let config = AxonFlowConfig {
        endpoint: server.uri(),
        ..Default::default()
    };
    let client = AxonFlowClient::new(config).unwrap();
    let resp = client.get_plan_status("plan42").await.unwrap();
    assert_eq!(resp.plan_id, "plan42");
}

#[tokio::test]
async fn test_cancel_plan_uses_singular_path() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/plan/plan42/cancel"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "plan_id": "plan42",
            "status": "cancelled",
            "success": true
        })))
        .expect(1)
        .mount(&server)
        .await;

    let config = AxonFlowConfig {
        endpoint: server.uri(),
        ..Default::default()
    };
    let client = AxonFlowClient::new(config).unwrap();
    let resp = client.cancel_plan("plan42", Some("test")).await.unwrap();
    assert_eq!(resp.plan_id, "plan42");
    assert!(resp.success);
}

#[tokio::test]
async fn test_execute_plan_defaults_status_completed_when_wire_omits_it() {
    // Regression (enterprise#2861 sweep): the execute-plan success payload
    // carries no `status` field (only metadata/plan_id), so `status`
    // deserialized to "" and callers treated a successful execution as a
    // failure. A successful round-trip must report "completed" (Go parity).
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/request"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/json")
                .set_body_json(json!({
                    "success": true,
                    "data": {
                        "plan_id": "plan-77",
                        "metadata": {
                            "execution_mode": "auto",
                            "tasks_executed": 2
                        }
                    }
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

    let exec = client
        .execute_plan("plan-77", Some("jwt-user"))
        .await
        .unwrap();
    assert_eq!(exec.status, "completed");
}

#[tokio::test]
async fn test_execute_plan_preserves_explicit_wire_status() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/request"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/json")
                .set_body_json(json!({
                    "success": true,
                    "data": {
                        "plan_id": "plan-88",
                        "status": "partial"
                    }
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

    let exec = client
        .execute_plan("plan-88", Some("jwt-user"))
        .await
        .unwrap();
    assert_eq!(exec.status, "partial");
}

#[tokio::test]
async fn test_execute_plan_failed_envelope_never_reads_completed() {
    // R3 regression: a policy-blocked/failed execution whose data payload
    // omits `status` must NOT default to "completed" — the default is gated
    // on the envelope's success verdict.
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/request"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/json")
                .set_body_json(json!({
                    "success": false,
                    "error": "execution failed: step search-flights failed",
                    "data": {
                        "plan_id": "plan-99",
                        "metadata": { "tasks_executed": 1 }
                    }
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

    let exec = client
        .execute_plan("plan-99", Some("jwt-user"))
        .await
        .unwrap();
    assert_eq!(exec.status, "failed");
    assert_eq!(
        exec.error.as_deref(),
        Some("execution failed: step search-flights failed")
    );
}
