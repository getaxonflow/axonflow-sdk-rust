# AxonFlow SDK for Rust

[![Crates.io](https://img.shields.io/crates/v/axonflow-sdk-rust.svg)](https://crates.io/crates/axonflow-sdk-rust)
[![Documentation](https://docs.rs/axonflow-sdk-rust/badge.svg)](https://docs.rs/axonflow-sdk-rust)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

> **Evaluating AxonFlow for a real deployment?**
>
> Choose the path that fits:
> - **Self-serve:** free 90-day [Evaluation License](https://getaxonflow.com/evaluation-license?utm_source=readme_sdk_rust_eval)
> - **Hands-on:** [Design Partner Program](https://getaxonflow.com/design-partner?utm_source=readme_sdk_rust) with either **6 months of self-hosted / in-VPC Enterprise** at no cost or **3 months of AxonFlow-managed Enterprise SaaS** with SLO-backed support, up to **50,000 write requests / 1,000,000 total requests per month**
>
> Priority support, architecture review, incident-readiness review, and roadmap input are included for selected partners. We reply within 48 hours.

Enterprise-grade Rust SDK for the AxonFlow AI governance platform. Add invisible AI governance to your applications with production-ready features including retry logic, caching, fail-open strategy, and debug mode.

## How This SDK Fits with AxonFlow

This SDK is a client library for interacting with a running AxonFlow control plane. It is used from application or agent code to send execution context, policies, and requests at runtime.

A deployed AxonFlow platform (self-hosted or cloud) is required for end-to-end AI governance. SDKs alone are not sufficient—the platform and SDKs are designed to be used together.

## Installation

Add this to your `Cargo.toml`:

```toml
[dependencies]
axonflow-sdk-rust = "0.1.0"
tokio = { version = "1", features = ["full"] }
```

## Quick Start

### Basic Usage (Invisible Governance via Interceptor)

The most common way to use AxonFlow is via an **Interceptor**. This wraps your existing LLM client (e.g., an OpenAI-compatible client) and automatically applies governance to every call.

```rust
use axonflow_sdk_rust::{AxonFlowClient, AxonFlowConfig};
use axonflow_sdk_rust::interceptors::openai::{WrappedOpenAIClient, ChatCompletionRequest, ChatMessage};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // 1. Initialize AxonFlow Client
    let config = AxonFlowConfig::new("http://localhost:8080")
        .with_auth("your-client-id", "your-client-secret");
    let axon = AxonFlowClient::new(config)?;

    // 2. Your existing OpenAI-compatible client (must implement OpenAIChatCompleter trait)
    let openai_client = MyOpenAIClient::new("api-key");

    // 3. Wrap it for automatic governance
    let governed_client = WrappedOpenAIClient::new(openai_client, axon, "user-123");

    // 4. Use as normal - governance is now "invisible"
    let resp = governed_client.create_chat_completion(ChatCompletionRequest {
        model: "gpt-4".to_string(),
        messages: vec![ChatMessage { 
            role: "user".to_string(), 
            content: "Hello, AxonFlow!".to_string() 
        }],
        ..Default::default()
    }).await?;

    println!("Result: {}", resp.choices[0].message.content);
    Ok(())
}
```

### Manual Audit (Gateway Mode)

If you are making LLM calls directly and just want to log them for compliance and cost tracking:

```rust
use axonflow_sdk_rust::{AxonFlowClient, AxonFlowConfig, TokenUsage};

let axon = AxonFlowClient::new(AxonFlowConfig::new("http://localhost:8080"))?;

// After your direct LLM call
axon.audit_llm_call(
    "request-id-from-llm",
    "Summary of the response",
    "openai",
    "gpt-4",
    TokenUsage { prompt_tokens: 100, completion_tokens: 50, total_tokens: 150 },
    250, // latency in ms
    None, // optional metadata
).await?;
```

## Examples

The SDK includes several runnable examples demonstrating common integration patterns. You can find them in the `examples/` directory.

### Running the Examples

Before running the examples, set your AxonFlow credentials as environment variables:

```bash
export AXONFLOW_CLIENT_ID="your-client-id"
export AXONFLOW_CLIENT_SECRET="your-client-secret"
# Optional: defaults to http://localhost:8080
export AXONFLOW_AGENT_URL="http://your-axonflow-endpoint"
```

Then use `cargo run --example <name>` to execute an example:

*   **Basic Chat Governance**:
    ```bash
    cargo run --example basic
    ```
*   **Model Context Protocol (MCP) Connectors**:
    ```bash
    cargo run --example connectors
    ```
*   **Multi-Agent Planning (MAP)**:
    ```bash
    cargo run --example planning
    ```
*   **Invisible Governance (Interceptors — OpenAI)**:
    ```bash
    cargo run --example interceptors
    ```
*   **Invisible Governance (Interceptors — Anthropic)**:
    ```bash
    cargo run --example anthropic_interceptor
    ```
*   **Decision Explainability** (ADR-043):
    ```bash
    export AXONFLOW_DECISION_ID="dec_..." # from a recent blocked call or audit row
    cargo run --example explain_decision
    ```

## Advanced Features

### Fail-Open Strategy
In `Production` mode, if the AxonFlow platform is unreachable, the SDK will "fail-open." This ensures your application remains available even if the governance layer is degraded.

### Caching
The SDK includes a built-in async cache (powered by `moka`) with TTL support to reduce latency for redundant requests. Caching is automatically disabled for mutation operations like plan execution.

### MCP & MAP Support
The Rust SDK provides full parity for Model Context Protocol (MCP) and Multi-Agent Planning (MAP):
*   **MCP**: List, install, and query Model Context connectors with full policy enforcement.
*   **MAP**: Generate and execute complex multi-agent plans programmatically.

## Configuration

```rust
let config = AxonFlowConfig {
    endpoint: "http://localhost:8080".to_string(),
    client_id: Some("id".into()),
    client_secret: Some("secret".into()),
    mode: Mode::Production,
    debug: true,
    timeout: Duration::from_secs(30),
    retry: RetryConfig {
        enabled: true,
        max_attempts: 3,
        initial_delay: Duration::from_secs(1),
    },
    cache: CacheConfig {
        enabled: true,
        ttl: Duration::from_secs(60),
    },
    ..Default::default()
};
```

## Telemetry

The SDK includes a non-blocking background heartbeat that follows the AxonFlow telemetry contract: **at most one anonymous ping per machine every 7 days** to `https://checkpoint.getaxonflow.com/v1/ping`. Payload is classification-only — SDK version, OS, architecture, runtime version, deployment mode, and an endpoint-type bucket (`localhost` / `private_network` / `remote` / `unknown`). The raw URL is never sent.

`AXONFLOW_TELEMETRY=off` is the **sole opt-out lever** as of v0.2. There is no programmatic disable on the SDK config — the env-var-only pattern matches HashiCorp's `CHECKPOINT_DISABLE`, Docker, and Datadog Agent. Sandbox-mode clients (constructed via `AxonFlowConfig::sandbox(...)`) tag their pings with `stream="sandbox"` so analytics can distinguish dev/test usage from production heartbeat. `DO_NOT_TRACK` is intentionally not honored.

### Scope of `AXONFLOW_TELEMETRY=off`

`AXONFLOW_TELEMETRY=off` disables the anonymous SDK heartbeat (version, OS, architecture). On **self-hosted** and **in-VPC** deployments, that heartbeat is the only data the SDK sends to AxonFlow, so setting `=off` means we receive nothing. On **Community SaaS** (`try.getaxonflow.com`) the hosted service also processes operational data — registrations, audit logs, policy enforcement records, workflow state, plan data, and request-header metadata aggregated for usage analytics — as part of running the platform; that operational data flow is governed by the [Privacy Policy](https://getaxonflow.com/privacy/), not by `AXONFLOW_TELEMETRY`.

See [Telemetry Documentation](https://docs.getaxonflow.com/docs/telemetry) for full details.

## License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.
