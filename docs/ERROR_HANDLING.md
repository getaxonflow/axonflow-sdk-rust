# Error Handling in the Rust SDK

## Design Philosophy

The AxonFlow Rust SDK follows idiomatic Rust error handling patterns using the `Result` type and a comprehensive `enum` for error categorization. We leverage the `thiserror` crate to provide descriptive, type-safe error variants.

## The AxonFlowError Enum

All SDK methods return a `Result<T, AxonFlowError>`. The `AxonFlowError` enum provides clear distinction between different failure modes:

```rust
pub enum AxonFlowError {
    /// Network-level errors (connection refused, timeout, etc.)
    HttpError(reqwest::Error),
    
    /// JSON serialization or deserialization failures
    SerdeError(serde_json::Error),
    
    /// API-level errors returned by the AxonFlow platform
    ApiError { status: u16, message: String },
    
    /// Invalid client configuration (e.g., missing ClientID in try mode)
    ConfigError(String),
    
    /// Platform is unreachable or degraded (used in fail-open logic)
    Unavailable(String),
}
```

## Error Handling Patterns

### Basic Pattern

```rust
use axonflow_sdk_rust::{AxonFlowClient, AxonFlowConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = AxonFlowClient::new(AxonFlowConfig::new("http://localhost:8080"))?;

    match client.list_connectors().await {
        Ok(connectors) => println!("Found {} connectors", connectors.len()),
        Err(e) => eprintln!("Failed to list connectors: {}", e),
    }
    
    Ok(())
}
```

### Checking for Specific Errors

You can use pattern matching to handle specific error conditions, such as rate limits or policy violations:

```rust
use axonflow_sdk_rust::AxonFlowError;

let result = client.proxy_llm_call(user, query, "chat", context).await;

if let Err(AxonFlowError::ApiError { status, message }) = result {
    match status {
        403 => println!("Policy violation: {}", message),
        429 => println!("Rate limited, try again later"),
        500..=599 => println!("Server-side error: {}", message),
        _ => println!("Other API error: {}", message),
    }
}
```

### Fail-Open Resilience

One of the core features of the AxonFlow SDK is the **Fail-Open** strategy. By default, in `Production` mode, the SDK will catch `HttpError` and `Unavailable` variants and return a success response with an embedded warning.

This ensures that your application remains operational even if the governance platform is temporarily down.

```rust
// In Production mode, this will succeed even if the server is offline
let resp = client.proxy_llm_call(user, query, "chat", context).await?;

if let Some(err) = resp.error {
    println!("Warning: Governance degraded (failing open): {}", err);
}
```

## Best Practices

### 1. Leverage `?` for Error Propagation

Rust's `?` operator is the preferred way to propagate errors up to a caller that can handle them:

```rust
async fn get_plan(client: &AxonFlowClient) -> Result<PlanResponse, AxonFlowError> {
    let plan = client.generate_plan("do something", "it", None).await?;
    Ok(plan)
}
```

### 2. Use `is_retryable()`

The SDK provides a helper method `is_retryable()` on the error type. While the SDK handles most retries internally via `RetryConfig`, you can use this for manual retry loops:

```rust
if let Err(e) = result {
    if e.is_retryable() {
        // Log and retry...
    }
}
```

### 3. Handle 403 (Policy Violations) vs 500 (Server Errors)

Policy violations are returned as `ApiError` with a `403` status. It's important to distinguish these from infrastructure failures to provide correct feedback to your end-users.

## HTTP Status Code Mapping

The SDK maps HTTP status codes as follows:

| Status Code | AxonFlowError Variant | Note |
|-------------|-----------------------|------|
| 200 OK | `Ok(T)` | Standard success |
| 402/403 | `Ok(ClientResponse)` | Blocked requests are returned as `Ok` with `blocked: true` |
| 429 | `ApiError` | Rate limited (Retryable) |
| 4xx | `ApiError` | Client errors (Non-retryable) |
| 5xx | `ApiError` | Server errors (Retryable) |
| Connection Refused | `HttpError` | Trigger fail-open in Production |
