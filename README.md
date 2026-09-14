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
axonflow-sdk-rust = "0.11.0"
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
*   **AuthZEN-native authorization** (ADR-065) — nine steps, four of them refusals or unresolved:
    ```bash
    cargo run --example authzen
    ```
*   **The PEP capability handshake** (platform v10.4.0+):
    ```bash
    cargo run --example pep_handshake
    ```
*   **Typed policy authoring** (platform v11.0.0+). It publishes and activates only with `AXONFLOW_TYPED_POLICY_PUBLISH=1`, and exits non-zero when a publication or activation it asked for is refused:
    ```bash
    AXONFLOW_TYPED_POLICY_PUBLISH=1 cargo run --example typed_policies
    ```

Run `pep_handshake` before `typed_policies`: after a document with an organization-scope constraint is activated, a decide that does not supply the attribute the constraint conditions on is denied fail-closed with reasons ["unknown_constraint"]; supply the attribute or run this example on a fresh stack. From v11.0.0 the deny's first reason is that code, followed by one naming each constraint it could not evaluate and the attribute it needed (getaxonflow/axonflow-enterprise#4247). Both examples print what the platform answered, a decision's `reasons` included. A Community deployment answers both with or without credentials: this SDK's `decide` names no organization in its request body, and a Community agent stamps every call's organization from its deployment (`ORG_ID`), not from the credentials.

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
| `Attribute::absent()` | the source answered: there is no value | the MEMBER is omitted from the bag; the bag itself is still sent, so `properties` arrives as `{}` | evaluated; a fact with no value changes nothing |
| `Attribute::unknown(why)` | the source **could not answer** | never reaches the wire | `AuthZenEvaluationError::Unresolved`, before the round trip |

Absent and unknown are not the same event. Dropping an unknown attribute from the request would obtain a decision that weighed every attribute except the one nobody could read — and report it as complete. That is the exact failure the server refuses on its side of the wire ("accepting it would report that it was considered when it was not"); `Attribute` is the same refusal on yours. Read a value with `Attribute::fold`, which does not compile until you have said what all three states mean; `as_known()` collapses two of them and is for logging.

**A later write never overwrites an unresolved attribute.** `with_query` and `with_correlation` decline a write over an `Attribute::unknown`, at both the bag and the leaf, so a caller that recorded "nobody could read the request body" and then wrote a recovered partial query does not end up sending a complete-looking envelope. The declined write is not silent: the surviving unknown refuses the envelope at its own pointer, carrying the reason. `AttributeMap::insert` is the ordinary map write and DOES replace — use `record` if you want the rule.

**Only one refusal code is worth retrying.** `AuthZenEvaluationError::retryable()` is the whole set in one place: a server refusal only when its code is `evaluation_unavailable`; a transport failure (timeout, connect, `5xx`, `429`); never an unreadable profile (retrying cannot make an older SDK able to read a newer one), never an unusable response, and never an unresolved attribute - that refusal is frozen inside the request, so every resend reproduces it. The OPERATION may succeed once the attribute resolves, but only after you build a new request.

This surface does **not** apply the client's `RetryConfig`: that executor is wired to the proxy path's request type, and retrying an authorization decision on your behalf is a policy decision this SDK does not make for you. Retry is yours, guided by `retryable()`.

**A refusal is not a denial.** `decision: false` says the request was evaluated and denied. A refusal says it was never evaluated. They arrive as different types — `Ok(decision)` versus `Err(Refused(..))` — so no caller branch can conflate an auth failure, a malformed envelope or an outage with a policy denial.

**A local refusal names the same MEMBER the server would.** The SDK validates before sending, and a local refusal carries the JSON Pointer the server would have sent for the same bytes - verified against a live server by `runtime-e2e/authzen_evaluation`. The CODE may be narrower on the server side, and that is not a defect in either: this client knows only that a required member is missing and says `incomplete_evaluation`, while the server additionally knows which values it can evaluate and narrows the same condition to `unsupported_subject` with a `supported` list. Branch on `refusal.pointer` for "which member"; read the code as the server's more specific reading when there is one.

**`allowed()` requires the state, not just the boolean.** It is true only when the collapsed boolean *and* the four-valued operational state both say `ALLOW`. A body where they disagree, one carrying no profile payload at all, or one written in a profile this build cannot read never becomes a decision — it becomes an error. There is no path that returns an allow the SDK could not fully read.

**The types are generated, never hand-written.** `src/authzen/types_gen.rs` is emitted from `testdata/authzen-surface.json`, the platform's canonical contract artifact, by `tools/gen-authzen-types`. Regenerate with `cargo run -p axonflow-authzen-codegen`; `cargo test` fails if the committed file is not what the artifact generates.

## PEP capability handshake

An enforcement point declares the obligation types and schema versions it can discharge, and the SDK sends that declaration as the `X-Axonflow-PEP-Handshake` header on the calls whose route reads it. The platform reads it from **v10.4.0**. From **v11.0.0**, `decide` under an organization's redact override refuses a caller that does not declare redaction (`field_redact` at version 1) with `unsupported_obligation`, so a client that declares nothing is refused there.

```rust
use axonflow_sdk_rust::{AxonFlowClient, AxonFlowConfig, DecideRequest, PEPCapability, PEPHandshake};

let declared = PEPHandshake::new(
    "gateway",                    // this enforcement point, within your credential
    "https://pep.example.test",   // the audience a decision proof is bound to
    [PEPCapability::new("field_redact", 1)],
)?; // a declaration the platform would refuse fails here, naming the member

let client = AxonFlowClient::new(
    AxonFlowConfig::new("http://localhost:8080").with_pep_handshake(declared),
)?;
let decision = client.decide(DecideRequest::new("tool", "look up the weather")).await?;
```

- **Where it is sent.** `decide`, `evaluate`, `evaluate_all`, and the MCP check-input round-trip that `fulfill_request` and `decide_and_fulfill` make. Never on `proxy_llm_call` or `query_connector` (`/api/request`), which do not read it, and never on any other route. It is not a default header. The platform also reads it on the gateway pre-check (`/api/policy/pre-check`) and on MCP `tools/call`; this SDK calls neither.
- **Absent is not empty.** A client without a declaration sends no header; there is no default, because only you know what your enforcement point can discharge. An empty capability list is a declaration that it discharges nothing, so every mandatory obligation is one it cannot discharge.
- **One process, two enforcement points.** `client.with_pep_handshake(other)` derives a client that presents a different declaration and shares the transport and cache; use it for a call that should present its own. `as_user` keeps the declaration, and `with_pep_handshake` keeps the user token.
- **Refused before it is sent.** `PEPHandshake::new` applies the platform's rules (a lower-case `pep_id` and an `audience` of at most 128 bytes each, at most 64 known capabilities at positive versions with no repeats, at most 4096 bytes encoded) and returns a `PEPHandshakeError` whose `pointer` names the member at fault, instead of a `400` on the first governed call.
- **Which edition refuses what.** From v11.0.0, on every edition, the engine refuses with `unsupported_obligation` a mandatory obligation the caller's declaration cannot discharge, and a caller that presents no declaration can discharge none (an organization's redact override is the shipped case), so on Community too, a caller that declares `field_mask` but not `field_redact` is refused under a redact override. What only Enterprise adds happens at the handler, for an enforcement point that presented a declaration: an allow carrying a mandatory obligation outside the declared set becomes a deny, a refusal names the capability the declaration lacks, and on the MCP check-input round-trip that `fulfill_request` and `decide_and_fulfill` make, a redaction the declaration cannot discharge is refused rather than handed back masked. A Community deployment drops a declared capability in a family its edition does not issue, counts it, and lets the request proceed.

## v11.0.0 platform

Against a v11.0.0 platform this SDK reaches the new decision plane. Against an older platform the calls that existed before work as before, and each v11 field reads `None`. What each part needs:

- **The PEP capability handshake: v10.4.0+.** A platform reads the declaration from v10.4.0. From v11.0.0, `decide` under an organization's redact override refuses a caller that does not declare redaction (see [PEP capability handshake](#pep-capability-handshake)).
- **Decision provenance: v11.0.0+.** `DecideResponse`, `MCPCheckOutputResponse`, `ClientResponse` and `ConnectorResponse` carry `engine`, `subject_type`, `policy_bundle` and `legacy_validators`, and a `DecideResponse` adds `policy_identities`, `policy_packs` and `document_version`. A v11.0.0 platform fills them; `legacy_validators` only where a checksum validator acted.
- **Typed policy authoring: v11.0.0+.** The routes exist from v11.0.0 (see [Typed policy authoring](#typed-policy-authoring-v1100)). An older platform does not serve them, so each call is refused rather than answered, and `active()` does not read that as nothing active.
- **Route deprecation stamps and the legacy policy write freeze: v11.0.0+, with nothing to handle here.** This SDK never reached the routes removed in v11.1: it calls none of the legacy policy routes (static, system or dynamic policies, their overrides, impact reports, conflicts or simulation), so neither a deprecation stamp nor a frozen write reaches a call it makes.

Runnable programs, in this order: [`examples/pep_handshake`](examples/pep_handshake/main.rs), then [`examples/typed_policies`](examples/typed_policies/main.rs).

## Typed policy authoring (v11.0.0+)

A v11 platform authors policy as a typed document: validated, published as a signed artifact pinned by its digest, and promoted to active. `client.typed_policies()` reaches the six routes the agent proxies under `/api/v1/typed-policies`:

```rust
let typed = client.typed_policies();
let edition = typed.edition().await?;                                // what this deployment may author
let validation = typed.validate(&document, Some(&fixtures)).await?; // every finding
let published = typed.publish(&document, Some(&fixtures)).await?;   // signed, pinned by its digest
typed.activate(&published.digest, Some("quarterly review")).await?; // promote to active
let active = typed.active().await?;                                 // the signed bytes in force, or None
let system = typed.system().await?;                                 // the platform's own controls
```

- **Activation replaces the organization template.** Activating a document that omits the organization template's controls removes those controls for the organization. The template's controls carry the destructive-command blocks, DROP and TRUNCATE prevention and the blocking SQL-injection rows. `publish` and `activate` report which ones a document omits as `template_omissions`, and the example prints that report. The publish fixture the example uses is a minimal example, not a starting point for production: it omits all of them.
- **Activation promotes.** A digest whose version does not advance past the active one is refused. Rolling back to an earlier document, and withdrawing the active one, are operations of the customer portal behind its session; the agent does not proxy them, so the SDK has no method for either.
- **The agent stamps the organization and the author.** The platform signs the caller as author whatever the document names. The organization is the one your credentials resolve to; on Community it is the deployment's (`ORG_ID`).
- **System controls are changed in the document, not per policy.** A v11.0.0 platform retires per-policy overrides: `POST` and `DELETE /api/v1/{static,system}-policies/{id}/override` answer `409 LEGACY_POLICY_WRITE_FROZEN` (agent-api's `PerPolicyOverrideRetired` response), and a system control is enabled, disabled or re-actioned in the organization's typed document, in its `system_controls` section. This SDK never called those routes.
- **Refusals are typed.** Every refusal except a `401` is `AxonFlowError::TypedPolicyRefusal`, with the HTTP `status`, the platform's `reason` (such as `publication_refused`, `activation_refused` or `tier_limit`), any `findings`, the `policy` a tier refusal names, and `retry_after`. `is_retryable()` follows the platform's `Retry-After`, not the status alone. On an edition with separation of duties, publishing refuses with the finding code `APPROVER_IS_AUTHOR`: the route names no approver, and such a deployment approves in the customer portal.
- **`active()` is `None` only for the platform's `nothing_active`.** Any other `404` is a refusal with status `404`. The platform currently also answers `nothing_active` when its document store cannot be read (getaxonflow/axonflow-enterprise#4255).

## Reading decisions: who is asking decides what comes back

`explain_decision` and `list_decisions` are scoped to the **per-user identity**
you present, not to the tenant credential. Since platform #2922:

| What you present | What an enterprise stack returns |
|---|---|
| a tenant-wide role (`admin`, `owner`, `policy_admin`) | the whole tenant |
| any other identity (`developer`, `viewer`) | only the rows attributed to it |
| **no identity** | **nothing at all** — every list is empty, every explain is not-found |

`client_id`/`client_secret` authenticate the **organization**. They do not say
who is asking, so on their own they land in the third row. Community and
Community-SaaS deployments are single-operator and read tenant-wide with no
identity needed.

```rust
let mut config = AxonFlowConfig::new("http://localhost:8080")
    .with_auth(client_id, client_secret);
config.user_token = Some(user_token);          // the per-user identity
let client = AxonFlowClient::new(config)?;

// Per call:
let exp = client.explain_decision_as(&decision_id, Some(&users_token)).await?;

// Or, for a process acting on behalf of several people, derive a client bound
// to one person. Unlike the `*_as` methods, which only the reads have, this
// reaches EVERY method.
let rows = client.as_user(&alices_token)
    .list_decisions(ListDecisionsOptions::default())
    .await?;
```

The token is a per-user JWT — minted by the customer portal's user-token API, or
for local testing by `scripts/generate-jwt.sh --kind user`. It is **not** the
tenant JWT and not `client_secret`. It is sent as `X-User-Token`, is redacted
from the config's `Debug`, never reaches telemetry, and is never sent to any
origin but the configured endpoint.

Surrounding whitespace is trimmed, so a trailing newline off a file read is
harmless. A token carrying an *embedded* control character cannot be an HTTP
header value at all, and that is reported as an `AxonFlowError::ConfigError`
naming the offending byte's position and class — never its value — with the
request not sent. It is deliberately not dropped: a dropped one would make the
read silently unidentified and the SDK would then report "no identity was
presented", which is true of the wire and false of what you did.

### Telling the outcomes apart

"Not found", "not yours" and "no identity resolved" used to arrive as the same
`404`, and an unscoped list arrived as an ordinary empty page. Both now carry a
cause:

```rust
match client.list_decisions(ListDecisionsOptions::default()).await {
    Err(AxonFlowError::ReadScope(refusal)) if refusal.identity_missing() => {
        // The platform resolved no identity, so it returned zero rows by
        // construction. The empty answer was never evidence about your data.
    }
    Ok(rows) => { /* ... */ }
    Err(e) => return Err(e.into()),
}
```

`explain_decision` is where the other scope shows up. Under `own-rows` the
platform answers "not attributed to you" and "not there at all" with the **same
404**, deliberately, so that a miss cannot be used to probe for another user's
rows — the refusal reports the scope the read ran under, never a claim about
what exists.

> **A valid token can still resolve to nobody.** The platform reserves the whole
> of `@axonflow.local` and `@axonflow.internal` for *shared* identities and
> censuses them to nothing before scoping. A correctly-signed developer token
> minted at `demo-user@axonflow.local` — which is `generate-jwt.sh`'s own
> default — reads zero rows and reports `identity_missing()`, exactly like no
> token at all. Mint per-user identities at a real domain.

> **Setting `user_token` affects more than reads.** The header rides every
> request and the agent validates it on every route it proxies — not just the
> scoped reads. A stale or rotated token therefore turns `list_connectors`,
> `install_connector` and policy CRUD into `401`s rather than merely unscoping a
> read. That is the correct, fail-closed direction, but it puts this value in
> the same rotation story as `client_secret`.

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

The SDK includes a non-blocking background heartbeat that follows the AxonFlow telemetry contract: **at most one ping per machine every 7 days** to `https://checkpoint.getaxonflow.com/v1/ping`. Payload is classification-only — SDK version, OS, architecture, Rust toolchain version, deployment mode, an endpoint-type bucket (`localhost` / `private_network` / `remote` / `unknown`), and the deployment's `org_id` (the `ORG_ID` env value, or the `local-dev-org` sentinel when unset; on Community SaaS it is the `cs_<uuid>` tenant identifier). The raw URL of your endpoint is never sent — only the bucket it falls into.

The gate is evaluated **on each SDK request** — not when a client is constructed — so a long-running service stays visible across the 7-day boundary, and a client that is built but never used sends nothing at all. Evaluating it costs one in-process check, which is what almost every request pays.

When a ping is actually due, the `/health` probe and the POST are **awaited on that request**, bounded at 3 seconds. They are not spawned, and that is deliberate: a spawned send dies with a process that does not outlive it, measured at **1 delivery in 12** for a compiled one-call binary — precisely the short-lived CLI, job and function population the signal exists to count. The cost is reachable at most once per hour per process, and only when a ping is due, which the 7-day stamp limits to once per machine per week.

### Declaring a framework adapter (`register_adapter`)

If you are building a framework integration on top of this crate, you can declare it so aggregate adoption figures can tell adapter-driven usage apart from bare SDK usage. Without this they are indistinguishable: an adapter reports the same `sdk`, the same `sdk_version` and the same endpoint as any other client.

```rust
use axonflow_sdk_rust::register_adapter;

register_adapter("my-framework");
```

**This crate ships no adapter of its own**, so nothing in it calls this — it exists for third-party integrations. (The `interceptors` module wraps LLM *provider* clients, which is a different dimension from the agent framework driving the SDK and is deliberately not reported here.)

The name is added to the `features` array of the heartbeat that already fires, as `adapter:my-framework`. **It adds no network request**, and calling `register_adapter` does not itself send anything. It is idempotent and safe from any thread.

The heartbeat fires on the client's **first outbound request**, not at construction, so anything registered before that request is on the very first ping. A name registered afterwards rides the next heartbeat.

What is and is not collected:

- **Collected:** the adapter name you pass, lowercased and trimmed.
- **Not collected:** anything about what the adapter *does* — no prompts, no payloads, no tool names, no user identities, no configuration.

Bounds, so a malformed call cannot damage the ping it rides on:

- A name longer than **64 bytes** is **dropped whole**, never truncated — a truncated adapter name is a name nothing is running. A name that is empty after trimming is ignored.
- The `features` array carries at most **32 entries**, none longer than **128 bytes**, mirroring the receiver's own bounds.

The name is **not** validated against a list of known frameworks. The canonical vocabulary lives on the receiving service, which folds an unrecognised name into an `adapter:unknown` bucket while keeping the raw name on the row.

### When the heartbeat fires

It fires on the client's **first outbound request**, not at construction — so a client that is created and never used does not ping at all. At most one ping per machine per 7 days is delivered, and the cadence is held both by a stamp file and in memory, so a runtime that cannot write the stamp is still bounded (per process rather than per machine). If the checkpoint cannot be reached, the re-check interval doubles from 1 hour to a ceiling of 7 days and a single delivery resets it.

`AXONFLOW_TELEMETRY=off` is the **sole opt-out lever** as of v0.2. There is no programmatic disable on the SDK config — the env-var-only pattern matches HashiCorp's `CHECKPOINT_DISABLE`, Docker, and Datadog Agent. Sandbox-mode clients (constructed via `AxonFlowConfig::sandbox(...)`) tag their pings with `stream="sandbox"` so analytics can distinguish dev/test usage from production heartbeat. `DO_NOT_TRACK` is intentionally not honored.

### The heartbeat contacts your platform's `/health` (new in 0.10.0)

**This is a change to the SDK's network behaviour.** Before 0.10.0 the telemetry path made exactly one outbound request, to the checkpoint service. It now makes one more, first: a `GET` on your configured platform endpoint's `/health`. That endpoint is your own platform, it is unauthenticated, and it is one the SDK was already configured to talk to — but the request is new, so it is called out here rather than left to be discovered.

Four values are read from that one response and relayed onto the ping: the platform's version, its licence tier, its edition, and its own deployment mode. The probe does not follow redirects and does not disable certificate verification, so the values can only ever be what your configured endpoint itself served over a connection this SDK could verify. This brings the Rust SDK to parity with the Go, Python, TypeScript and Java SDKs, which already read the same response. There is exactly one `/health` fetch per heartbeat; every relayed value rides it.

**What is and is not collected.** Collected: the coarse strings `/health` returned, exactly as it returned them. **Not collected: your licence key, its expiry, its seat or node count, your organisation's name, your endpoint URL, or any other licence or deployment detail.** The SDK never reads your licence key, and it sends nothing else from the `/health` response.

**This is an adoption-analytics signal, not an entitlement one.** The values are whatever the platform at your configured endpoint reported about itself, relayed unchanged: the SDK derives nothing and verifies nothing, and the receiver cannot verify the relay either. Whoever operates that endpoint controls them completely, so they must never gate entitlement, unlock a feature, or enter any authorization or billing decision.

**Absent means unknown, never a guess.** Each field is omitted from the ping entirely whenever it could not be determined — your platform is unreachable, presents a certificate this SDK cannot verify, redirects elsewhere, returns an error, returns an unparseable or oversized body, or returns no such field. It is never defaulted to a substituted value, so an omitted field means "not known" and never implies a particular tier or edition. A platform released before these fields existed simply omits them, which is the same case. A value longer than 64 bytes is dropped whole rather than truncated, since a truncated string would be a claim your platform never made.

**Transient values are reported as-is.** A platform that is still starting reports `starting`, and the SDK forwards that unchanged rather than filtering it.

**A failure here never costs you the ping, and what it can cost your request is bounded and stated.** The probe has its own capped share of a single 3-second budget covering the whole telemetry path, so an unreachable or slow `/health` cannot cost you the ping, stack timeouts, or surface an error to your code. Because the telemetry path is awaited on the request that triggers it, a slow probe **can** delay that one call — by at most its capped share of those 3 seconds, at most once per hour per process, and only when a ping is due.

If the checkpoint service cannot be reached at all — an air-gapped or egress-restricted deployment — repeated failures widen the retry interval rather than retrying hourly forever. That backoff is per process, so a fleet of short-lived processes still attempts once per process start; `AXONFLOW_TELEMETRY=off` is the way to stop it entirely.

`AXONFLOW_TELEMETRY=off` suppresses the `/health` probe together with the rest of the heartbeat — with it set, the SDK makes no telemetry request of any kind, including to your own platform.

### Scope of `AXONFLOW_TELEMETRY=off`

`AXONFLOW_TELEMETRY=off` disables the SDK heartbeat (version, OS, architecture, deployment org_id). On **self-hosted** and **in-VPC** deployments, that heartbeat is the only data the SDK sends to AxonFlow, so setting `=off` means we receive nothing. On **Community SaaS** (`try.getaxonflow.com`) the hosted service also processes operational data — registrations, audit logs, policy enforcement records, workflow state, plan data, and request-header metadata aggregated for usage analytics — as part of running the platform; that operational data flow is governed by the [Privacy Policy](https://getaxonflow.com/privacy/), not by `AXONFLOW_TELEMETRY`.

See [Telemetry Documentation](https://docs.getaxonflow.com/docs/telemetry) for full details.

## License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.
