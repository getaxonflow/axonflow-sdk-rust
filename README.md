# AxonFlow SDK for Rust

[![Crates.io](https://img.shields.io/crates/v/axonflow-sdk-rust.svg)](https://crates.io/crates/axonflow-sdk-rust)
[![Documentation](https://docs.rs/axonflow-sdk-rust/badge.svg)](https://docs.rs/axonflow-sdk-rust)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

> **Taking a sponsored workflow to production?**
>
> Choose the path that fits:
> - **Self-serve:** free 90-day [Evaluation License](https://getaxonflow.com/evaluation-license?utm_source=readme_sdk_rust_eval)
> - **Paid production program:** [Design Partner or Confidential Pilot](https://getaxonflow.com/design-partner?utm_source=readme_sdk_rust)  -  one scoped workflow over 60 or 75 days, founder-led rollout support, upfront conversion pricing, and a fixed decision date; public track from $2,000 or confidential track from $4,000
>
> The paid program requires a dated forcing event, written controls, an executive sponsor, and a technical owner. Prices are subject to eligibility and a signed agreement.

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
*   **AuthZEN-native authorization** (ADR-065) — eight steps, five of them refusals:
    ```bash
    cargo run --example authzen
    ```

## AuthZEN-native authorization (ADR-065)

`POST /api/v1/access/evaluation` is the AuthZEN-shaped authorization surface. It is the surface to write **new** integrations against: at v11 the engine behind it becomes the ADR-065 Policy Decision Point with no wire change, so an integration written against it migrates once rather than twice. Nothing here is deprecated — the existing decision surface stays wire-stable through all of v11. See `docs/AUTHZEN_MIGRATION_DRAFT.md`.

```rust
use axonflow_sdk_rust::authzen::{
    Attribute, AuthZenAction, AuthZenRequest, AuthZenResource, AuthZenSubject,
};

let decision = client
    .evaluate(
        AuthZenRequest::evaluating(
            AuthZenSubject::new("gateway", "llm-gateway-01"),
            AuthZenAction::new("llm.completion"),
            AuthZenResource::new("llm", "llm"),
        )
        .with_query(Attribute::known(user_prompt))
        .with_correlation("x-session-id", Attribute::known(session_id)),
    )
    .await?;

if !decision.allowed() {
    return Err(format!("blocked: {} ({})", decision.state(), decision.category()).into());
}
for obligation in decision.mandatory_obligations() {
    // An allow with an undischarged mandatory obligation is NOT an allow.
    discharge(obligation)?;
}
```

`evaluate_all` takes several preconditions of **one** operation and returns **one** decision: the entries combine to the least permissive outcome, so one denied entry denies the operation. An API returning a list would invite a caller to act on the entry it liked.

### Known gotchas

**A resolved attribute has three states, and `Option` carries two.** Every attribute bag — `subject.properties`, `action.properties`, `resource.properties`, and `context` — holds `Attribute<T>` values, not `Option<T>`:

| | meaning | wire | outcome |
|---|---|---|---|
| `Attribute::known(v)` | the source answered with `v` | the member, with its value | evaluated |
| `Attribute::absent()` | the source answered: there is no value | the member is **omitted** | evaluated; a fact with no value changes nothing |
| `Attribute::unknown(why)` | the source **could not answer** | never reaches the wire | refused before the round trip, `evaluation_unavailable`, retryable |

Absent and unknown are not the same event. Dropping an unknown attribute from the request would obtain a decision that weighed every attribute except the one nobody could read — and report it as complete. That is the exact failure the server refuses on its side of the wire ("accepting it would report that it was considered when it was not"); `Attribute` is the same refusal on yours. Read a value with `Attribute::fold`, which does not compile until you have said what all three states mean; `as_known()` collapses two of them and is for logging.

**Only one refusal code is worth retrying.** `AuthZenEvaluationError::retryable()` is the whole set in one place: a refusal only when its code is `evaluation_unavailable`; a transport failure (timeout, connect, `5xx`, `429`); never an unreadable profile (retrying cannot make an older SDK able to read a newer one) and never an unusable response. Every other refusal code names something about the request, which will not change on a retry.

**A refusal is not a denial.** `decision: false` says the request was evaluated and denied. A refusal says it was never evaluated. They arrive as different types — `Ok(decision)` versus `Err(Refused(..))` — so no caller branch can conflate an auth failure, a malformed envelope or an outage with a policy denial.

**The refusal vocabulary is shared across the wire.** The SDK validates before sending, and a local refusal carries the same code and the same JSON Pointer the server would have sent for the same bytes. `refusal.pointer` names the exact member to fix.

**`allowed()` requires the state, not just the boolean.** It is true only when the collapsed boolean *and* the four-valued operational state both say `ALLOW`. A body where they disagree, one carrying no profile payload at all, or one written in a profile this build cannot read never becomes a decision — it becomes an error. There is no path that returns an allow the SDK could not fully read.

**The types are generated, never hand-written.** `src/authzen/types_gen.rs` is emitted from `testdata/authzen-surface.json`, the platform's canonical contract artifact, by `tools/gen-authzen-types`. Regenerate with `cargo run -p axonflow-authzen-codegen`; `cargo test` fails if the committed file is not what the artifact generates.

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

The SDK includes a non-blocking background heartbeat that follows the AxonFlow telemetry contract: **at most one ping per machine every 7 days** to `https://checkpoint.getaxonflow.com/v1/ping`. Payload is classification-only — SDK version, OS, architecture, runtime version, deployment mode, an endpoint-type bucket (`localhost` / `private_network` / `remote` / `unknown`), and the deployment's `org_id` (the `ORG_ID` env value, or `local-dev-org` sentinel when unset). The raw URL is never sent.

`AXONFLOW_TELEMETRY=off` is the **sole opt-out lever** as of v0.2. There is no programmatic disable on the SDK config — the env-var-only pattern matches HashiCorp's `CHECKPOINT_DISABLE`, Docker, and Datadog Agent. Sandbox-mode clients (constructed via `AxonFlowConfig::sandbox(...)`) tag their pings with `stream="sandbox"` so analytics can distinguish dev/test usage from production heartbeat. `DO_NOT_TRACK` is intentionally not honored.

### Scope of `AXONFLOW_TELEMETRY=off`

`AXONFLOW_TELEMETRY=off` disables the SDK heartbeat (version, OS, architecture, deployment org_id). On **self-hosted** and **in-VPC** deployments, that heartbeat is the only data the SDK sends to AxonFlow, so setting `=off` means we receive nothing. On **Community SaaS** (`try.getaxonflow.com`) the hosted service also processes operational data — registrations, audit logs, policy enforcement records, workflow state, plan data, and request-header metadata aggregated for usage analytics — as part of running the platform; that operational data flow is governed by the [Privacy Policy](https://getaxonflow.com/privacy/), not by `AXONFLOW_TELEMETRY`.

See [Telemetry Documentation](https://docs.getaxonflow.com/docs/telemetry) for full details.

## License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.
