# Architecture of the Rust SDK

## Design Goals

The AxonFlow Rust SDK is designed for **Invisible Governance**, **Production Resilience**, and **Developer Ergonomics**.

1.  **Invisible Governance**: AI governance should not require rewriting your application logic. We achieve this via the **Interceptor Pattern**.
2.  **Production Resilience**: High-availability AI applications cannot afford for their governance layer to be a single point of failure. We implement this via the **Fail-Open Strategy**.
3.  **Performance**: Crate provides high-performance async primitives using `tokio` and zero-copy serialization where possible.

## Core Components

### 1. The AxonFlowClient (Single Entry Point)

Following the ADR-026 specification, the AxonFlow Agent acts as a unified proxy for all platform features. The `AxonFlowClient` follows this "Single Entry Point" architecture.

All requests flow through the `proxy_llm_call` or specific feature methods (like `query_connector`). This ensures that authentication, telemetry, and resilience logic are centralized.

### 2. Interceptor Pattern

The Interceptor pattern is the recommended way to integrate AxonFlow. It allows the SDK to wrap popular LLM clients (like `async-openai`) and transparently apply governance.

**The Workflow:**
1.  **Request Capture**: The interceptor intercepts the outgoing LLM request.
2.  **Policy Pre-check**: It sends the prompt and context to AxonFlow for evaluation.
3.  **Gatekeeping**: If AxonFlow blocks the request (e.g., PII detected), the interceptor returns an error immediately, and the underlying LLM client is **never called**.
4.  **Delegation**: If approved, the interceptor forwards the call to the underlying client.
5.  **Background Audit**: Once the response is received, the interceptor asynchronously logs the metadata (tokens, latency) to AxonFlow for auditing.

*See the [interceptors example](../README.md#examples) for a concrete realization of this pattern.*

### 3. Fail-Open Strategy

Resilience is built into the core of the SDK. In `Mode::Production`, the SDK follows a "fail-open" approach:

- If the AxonFlow Agent returns a network error or a `5xx` server error, the SDK will catch the error and return a success response to the application.
- The response will contain a warning in the `error` field, but the application remains functional.
- This prevents governance infrastructure from causing downtime for your AI features.

## Telemetry Heartbeat

The SDK implements a strictly compliant 7-day machine-global telemetry heartbeat.

**Implementation Details:**
- **Persistence**: A timestamp file is stored in the OS-specific cache directory (e.g., `~/Library/Caches/axonflow/` on macOS).
- **Process Gating**: A `Once` block ensures that only one heartbeat check is performed per process lifetime.
- **Machine Gating**: If the stamp file indicates a heartbeat was sent within the last 7 days, no request is made.
- **Non-Blocking**: The heartbeat check happens on client initialization, but the actual network call is spawned in a background `tokio` task.

## Caching Strategy

The SDK uses `moka` for high-concurrency, asynchronous caching.

- **Selective Caching**: Only non-mutating operations (like chat queries) are cached.
- **Mutation Safety**: Operations like generating or executing plans automatically bypass the cache to ensure state consistency.
- **TTL**: Cache entries follow a configurable Time-To-Live (default 60s).

## Data Models (DTOs)

The SDK provides exact-parity Rust `structs` for all AxonFlow wire shapes, including:
- **Governance**: `PolicyEvaluationInfo`, `BudgetInfo`, `CodeArtifact`.
- **Orchestration**: `PlanResponse`, `PlanStep`, `PlanExecutionResponse`.
- **MCP**: `ConnectorMetadata`, `ConnectorResponse`.
