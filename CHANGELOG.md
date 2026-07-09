# Changelog

All notable changes to the AxonFlow Rust SDK will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.8.1] - 2026-07-09 — execute_plan status + example fixes

Hostile-testing sweep ahead of the BukuWarung integration
(getaxonflow/axonflow-enterprise#2861). One library fix (`execute_plan`
status semantics) + example fixes.

### Fixed

- **`execute_plan` status semantics fixed.** The execute-plan payload
  carries no `status` field (only metadata/plan_id), so `status`
  deserialized to `""` and callers treated every successful execution as a
  failure. The default is now derived from the envelope verdict:
  `"completed"` only when `success && !blocked`, else `"failed"` (with the
  envelope error carried over) — a policy-blocked/failed plan can never
  read as completed. An explicit wire `status` is preserved. Regression
  tests cover all three paths.
- **Examples pass `AXONFLOW_USER_TOKEN` and fail honestly.** Enterprise
  stacks (`DEPLOYMENT_MODE=enterprise`) validate user tokens as JWTs; the
  examples passed `""` / hardcoded literals and 401'd. `basic` (both
  calls, now also exits non-zero on a non-success PII response), `planning`
  (`generate_plan` + `execute_plan`) and `connectors` (`query_connector`)
  read `AXONFLOW_USER_TOKEN`.
- **`interceptors` and `anthropic_interceptor` examples authenticate.**
  They constructed a credential-less client against a hardcoded
  `http://localhost:8080`, printed `❌ Request blocked or failed` on the
  agent's 401 and still exited 0. They now read
  `AXONFLOW_AGENT_URL`/`AXONFLOW_CLIENT_ID`/`AXONFLOW_CLIENT_SECRET`/
  `AXONFLOW_USER_TOKEN` and exit non-zero on failure.

## [0.8.0] - 2026-06-29 — Performance & reliability hardening

A hygiene and performance pass across the SDK. Behavior on the wire is
unchanged; the one source-compatibility note is the new `CacheConfig`
field called out under **Changed** below.

### Added

- **`CacheConfig::max_capacity`** (default `10_000`) — the in-memory response
  cache is now bounded and evicts least-recently-used entries once full,
  preventing unbounded memory growth in long-running processes. Set a higher or
  lower limit to tune the memory/hit-rate trade-off.
- **`WrappedOpenAIClient::with_provider(provider)`** — a builder method to record
  the actual upstream provider (e.g. `"azure"`, `"together"`) in the audit
  trail when calling through a proxy or an OpenAI-compatible API. Defaults to
  `"openai"`.
- **`PartialEq` for `AxonFlowError`**, so errors can be compared directly in
  tests and caller logic.
- **`#[must_use]`** on `CancelPlanResponse`, `DecisionExplanation`, and
  `DecisionSummary` to catch accidentally-discarded results at compile time.
- **Tracing spans** (`#[tracing::instrument]`) on `proxy_llm_call`,
  `generate_plan`, and `explain_decision` for richer observability.

### Changed

- **Breaking (source-only):** `CacheConfig` gains the `max_capacity` field above.
  Code that constructs `CacheConfig` with a struct literal must add the field or
  use `..Default::default()`; code that relies on `CacheConfig::default()` is
  unaffected.
- **HTTP connection reuse** — both HTTP clients now enable idle connection
  pooling and TCP keepalive, reducing connection churn and latency under
  sustained load.
- **Non-blocking heartbeat I/O** — the background telemetry heartbeat no longer
  performs blocking filesystem writes on the async runtime.
- The audit trail now records the configured provider rather than always
  reporting `"openai"`.
- Reduced allocations in request auth-header construction.

### Fixed

- `402`/`403` responses with non-JSON bodies now surface as
  `AxonFlowError::ApiError` (carrying the status and raw body) instead of a
  confusing deserialization error.
- Failures while submitting an audit record are now logged via `tracing::warn`
  instead of being silently discarded.
- Telemetry heartbeat instance identifiers now use standard UUIDv4 generation.

## [0.7.0] - 2026-06-09 — Decision Mode PEP (decide → fulfill → forward)

Adds the SDK analog of `platform/shared/pep` (ADR-056, epic
getaxonflow/axonflow-enterprise#2563, tracking #2571): a **decide → fulfill →
forward** Policy Enforcement Point. `decide()` surfaces engine-fulfillable
`redact_pii` obligations; `fulfill_request()` discharges them by round-tripping
content through the engine endpoint each obligation names — **never by redacting
locally**. The SDK carries no redaction logic of its own: there is no regex, no
pattern table, no masking branch. An obligation the engine cannot discharge
**fails closed** (`AxonFlowError::ObligationNotFulfillable`) rather than
forwarding unredacted content.

### Added

- **`AxonFlowClient::decide(DecideRequest) -> DecideResponse`** — `POST
  /api/v1/decide` using the client's existing HTTP Basic (`org:license`) auth.
  401 (bad / demo credentials) surfaces as `AxonFlowError::ApiError { status:
  401, .. }`; a `deny` verdict is returned in the body (HTTP 200), not as an
  error.
- **`AxonFlowClient::fulfill_request(&DecideResponse, &str) -> (String, bool)`**
  — for each request-phase `redact_pii` obligation, POSTs the statement to the
  obligation's check-input endpoint and returns the engine-redacted content plus
  whether the engine changed it. **Fails closed** (returns
  `AxonFlowError::ObligationNotFulfillable`, never the original statement) when:
  no request-phase fulfillment; `content_types` is non-empty and omits
  `text/plain`; the endpoint is not the request-redaction path (foreign URLs
  rejected); the engine call fails / returns non-200; or `redaction_evaluated`
  is false/absent.
- **`AxonFlowClient::decide_and_fulfill(DecideRequest) -> (verdict, content,
  DecideResponse)`** — one-call PEP path. On a non-allow verdict returns the
  original query (caller blocks anyway); on allow returns engine-redacted
  content. On an unfulfillable obligation it surfaces the fail-closed error so a
  caller cannot accidentally forward the unredacted query.
- **`has_request_redaction(&[Obligation]) -> bool`** free function — branch on
  whether a verdict carries request-phase redaction work.
- **PEP types** in `axonflow_sdk_rust::types::pep`, re-exported from the crate
  root: `DecideRequest`, `DecideResponse`, `Obligation`,
  `ObligationFulfillment`, `DecisionCallerIdentity`, `DecisionTarget`,
  `MCPCheckInputRequest`, `MCPCheckInputResponse`, `MCPCheckOutputRequest`,
  `MCPCheckOutputResponse`. Wire field names are byte-identical with the Go /
  Python / TypeScript / Java SDKs.
- **`content_type` field on `MCPCheckInputRequest`**; **`redacted` /
  `redacted_statement` / `redaction_evaluated` on `MCPCheckInputResponse`**;
  **`redaction_evaluated` on `MCPCheckOutputResponse`**. All `#[serde(default)]`
  so older platforms deserialize cleanly (the fail-closed default for
  `redaction_evaluated` is `false`).
- **`AxonFlowError::ObligationNotFulfillable(String)`** — the fail-closed signal
  of the PEP contract. Non-retryable and not fail-open-eligible.
- **PEP contract constants** (`OBLIGATION_REDACT_PII`, `PHASE_REQUEST` /
  `PHASE_RESPONSE`, `CONTENT_TYPE_TEXT`, `VERDICT_ALLOW` / `VERDICT_DENY` /
  `VERDICT_NEEDS_APPROVAL`, `DECIDE_PATH`, `REQUEST_REDACTION_PATH` /
  `RESPONSE_REDACTION_PATH`, `GATEWAY_CONNECTOR_TAG`).
- **22 unit tests** in `src/pep.rs::tests` covering decide parse (allow +
  obligation / deny-in-body / 401), every fail-closed branch (missing
  request-phase fulfillment, response-phase obligation, unadvertised
  content-type, foreign endpoint, engine error, `redaction_evaluated` false,
  `redaction_evaluated` absent), passthrough (no obligation, engine found
  nothing, non-redact obligation type), `endpoint_path_matches` exact/absolute/
  foreign, `has_request_redaction`, and `decide_and_fulfill`
  allow/deny/unfulfillable.
- **`runtime-e2e/decide_fulfill_obligation/`** — bash runner + Rust helper crate
  exercising the real SDK against a live enterprise agent (NO mocks): proves
  decide → allow + obligation; fulfill → engine-masked content where neither
  `john.doe@example.com` nor `4111111111111111` survives; `decide_and_fulfill`
  parity; demo creds refused with 401. Mirrors the Python SDK's runner.

### Compatibility

Additive. No existing public API is changed; no removed fields; no changed
defaults. The new request/response fields are an acknowledged SDK superset of
the wire contract — older platforms ignore the extra request field and the
SDK's `#[serde(default)]` keeps response parsing fail-closed when the platform
predates the redaction fields. Minor version bump 0.6.0 → 0.7.0 (SDK semver is
decoupled from the platform version).

Requires an AxonFlow platform exposing `POST /api/v1/decide` with Decision Mode
for the `decide` / `decide_and_fulfill` path; `fulfill_request` requires the
request-redaction `redact_pii` capability on `/api/v1/mcp/check-input`.

Cross-SDK parity: getaxonflow/axonflow-enterprise#2571.
## [0.6.0] - 2026-05-30 — Decision request context + Pasal 56(b) transfer basis

Targets AxonFlow platform **v8.5.0**.

### Added

- **`context` field on `DecisionSummary` and `DecisionExplanation`** —
  `Option<HashMap<String, String>>` with `#[serde(default, skip_serializing_if =
  "Option::is_none")]`. Surfaces the sanitized request context a PEP attaches to a
  Decision Mode call (canonical `lower_snake_case` keys such as `x_ai_agent`,
  `x_session_id`, `x_leader_identity`, and `x-bukuwarung-*`), persisted by the
  platform at the audit row's `policy_details->'context'`. `list_decisions`
  returns the platform-truncated summary (5 keys); `explain_decision` returns the
  full map. `None` for pre-v0.6.0 audit rows.
- **`context_truncated` field on `DecisionExplanation`** — `bool`. True when the
  agent dropped surplus context keys at write time.
- **`transfer_basis` module constants** (`transfer_basis::ADEQUACY`,
  `transfer_basis::SAFEGUARDS`, `transfer_basis::PASAL_56B_DPA` = `"pasal_56b_dpa"`,
  `transfer_basis::CONSENT`), re-exported at the crate root. Name the Indonesia
  UU PDP Pasal 56 legal bases.

### Changed

- **`AuditLogEntry::transfer_basis` documentation** now records `pasal_56b_dpa`
  (Pasal 56(b) explicit DPA tag) alongside `adequacy`, `safeguards`, and
  `consent`. The field stays an `Option<String>` (surfaced verbatim), so existing
  code matching on `"safeguards"` is unaffected and the SDK never rejects a value
  a newer platform may add.

## [0.5.0] - 2026-05-27 — Indonesia PII category + cross-border audit fields

### Added

- **`PolicyCategory` enum** with all policy categories including `PiiIndonesia`
  (`"pii-indonesia"`). First policies module in the Rust SDK, establishing
  cross-SDK parity for policy category constants.
- **`AuditLogEntry` struct** with full audit log fields including
  `data_residency` and `transfer_basis` for cross-border data transfer
  logging. Both are `Option<String>` for backward compatibility.

## [0.4.0] - 2026-05-23 — Full HITL surface (`list` / `get` / `create` / `approve` / `reject` / `stats`)

Cross-SDK parity bring-up for HITL (Human-in-the-Loop) approval
workflows. Prior to this release the Rust SDK exposed **no** HITL
methods at all — the four stable SDKs (Python / TS / Go / Java) all
ship the read + review surface (`list_hitl_queue`, `get_hitl_request`,
`approve_hitl_request`, `reject_hitl_request`, `get_hitl_stats`),
plus the new `create_hitl_request` method introduced in v8.2.0 of
each. This release ports the full surface in one shot so Rust callers
can implement the full 4-step HITL flow against AxonFlow:

  1. Gate evaluates `require_approval` (via `pre_check` / `check_tool_input`)
  2. Caller invokes `client.create_hitl_request(...)` to enqueue the row
  3. Caller polls `client.get_hitl_request(approval_id)` until terminal state
  4. Caller resumes the agent or denies the call based on the decision

### Added

- **`AxonFlowClient::list_hitl_queue(opts: HITLQueueListOptions) -> HITLQueueListResponse`**
  with pagination + status/severity filters.
- **`AxonFlowClient::get_hitl_request(request_id: &str) -> HITLApprovalRequest`**.
- **`AxonFlowClient::create_hitl_request(input: HITLCreateInput) -> HITLApprovalRequest`.**
  Required fields: `client_id`, `original_query`, `request_type`.
  Optional fields cover policy attribution, severity, compliance
  framework, an expiry override, and the new `notify_url` callback.
  Server-side `X-Org-ID` / `X-Tenant-ID` headers are derived by the
  platform's auth middleware from the SDK client's configured
  credentials — callers do not pass them through this method.
- **`AxonFlowClient::approve_hitl_request(request_id: &str, review: HITLReviewInput) -> ()`**
  and the symmetric **`reject_hitl_request`**.
- **`AxonFlowClient::get_hitl_stats() -> HITLStats`**.
- **`HITLApprovalRequest` / `HITLCreateInput` / `HITLReviewInput` /
  `HITLQueueListOptions` / `HITLQueueListResponse` / `HITLStats`** in
  `axonflow_sdk_rust::types::hitl`, re-exported from the crate root.
- **`notify_url` field on `HITLCreateInput` and `HITLApprovalRequest`.**
  Opt-in webhook URL fired after the request reaches a terminal state
  (approved / rejected / expired / overridden). Pairs with the
  HMAC-SHA256 `X-AxonFlow-Signature` header on the receiver side.
  Scheme allowlist (`https://`, plus `http://` for self-hosted
  local-dev) is enforced server-side; bad schemes surface as
  `AxonFlowError::ApiError { status: 400, .. }`. Companion platform
  work in getaxonflow/axonflow-enterprise#2419.
- 18 contract tests in `src/hitl.rs::tests` covering: list happy path +
  filter serialization, get happy path + empty-id guard + 404
  propagation, create happy path + minimal required-fields +
  bad-`notify_url`-scheme 400 propagation + 401 propagation +
  connect-failure propagation + the three required-field validation
  guards, approve + reject path/body shape + empty-id guard, and
  stats envelope parsing, plus a unit test for the
  `build_list_query` field-omission contract.

### Compatibility

This is the first Rust SDK release to expose HITL methods, so there
is no behavior to preserve. The new types and methods are additive;
no existing public API is changed. Minor version bump 0.3.1 → 0.4.0
for the new module-level addition.

Requires AxonFlow platform >= 8.1.0 for `notify_url` webhook delivery
and `Idempotency-Key` request deduplication.

Cross-SDK parity sweep: getaxonflow/axonflow-enterprise#2421.

## [0.3.1] - 2026-05-22 — `runtime-e2e/x-client-id/` parity + `org_id` in telemetry heartbeat + retry-allowlist regression tests

Patch release. No SDK behavior changes for the X-Client-ID + retry
path; one additive wire field (`org_id`) on the telemetry heartbeat.

### Added

- **`runtime-e2e/x-client-id/`** runner — bash entry point plus a Rust
 helper crate. Mirrors the Go / Python / TypeScript / Java SDKs'
 `runtime-e2e/x-client-id/` directories. Brings up the public community
 docker-compose stack, then runs an in-process forwarding-proxy helper
 that captures the SDK's outbound HTTP headers off the wire and asserts:
 `X-Client-ID == AXONFLOW_TENANT_ID`, `X-Axonflow-Client` starts with
 `sdk-rust/`, `Authorization` starts with `Basic `, and `X-Tenant-ID`
 is absent. This is the wire-level companion to the unit test
 (`tests/x_client_id_header_test.rs`), which uses `wiremock` and is
 necessary but not sufficient — it can't catch contract drift between
 the SDK and the live community-stack agent in the same PR that causes
 it.
- **`org_id` field in the telemetry heartbeat body.** Brings the Rust
 SDK telemetry up to parity with the other four SDKs and the platform —
 every heartbeat now identifies which deployment-organization emitted
 it. Two sources in precedence order:
 1. The `ORG_ID` env var when set (the explicit configuration
    on self-hosted deployments, or the `cs_<uuid>` tenant identifier on
    Community SaaS).
 2. Otherwise the `local-dev-org` sentinel.

 Exposed as `axonflow_sdk_rust::heartbeat::telemetry_org_id()` and
 `axonflow_sdk_rust::heartbeat::ORG_ID_LOCAL_DEV_SENTINEL`. Always
 emitted; older receivers ignore the field cleanly for backward compat.
 Honors `AXONFLOW_TELEMETRY=off` like every other heartbeat field. See
 [getaxonflow.com/privacy/](https://getaxonflow.com/privacy/) for the
 customer-facing commitment that covers this field.
- **Regression tests around the retry-allowlist contract.** Two new
 integration tests bracket the retry boundary so a future refactor
 can't silently change either side: HTTP 401 is terminal (no retries
 on bad/expired credentials, preventing the storm pattern customers
 had observed against the audit endpoint); HTTP 429 keeps triggering
 retries up to `max_attempts` so rate-limit handling remains intact.

### Changed

- **Telemetry-enabled log line** softened from "Anonymous telemetry
 enabled" to "Telemetry enabled" to stay coherent with the `org_id`
 addition — the configured `ORG_ID` on self-hosted deployments is not
 anonymized; only the `instance_id` and `cs_<uuid>` Community SaaS
 identifier remain anonymous-by-design.

### Documentation

- **Rustdoc on the retry executor** documents which status codes retry
 (5xx + 429) and which are terminal (401 and everything else 4xx
 outside the allowlist), so customers who wrap the SDK in their own
 retry middleware know which classes to exclude.
- **Clarifying comment above the retry-allowlist** explains that 402 /
 403 are handled as success responses in the request executor and
 never propagate to the retry path as errors, so the `*status != 402`
 / `*status != 403` clauses in the allowlist are intentional defense
 against any future refactor that converts 402/403 back to errors.

## [0.3.0] - 2026-05-19 — `X-Axonflow-Client` + `X-Client-ID` headers on every outbound request (v9 identity)

Companion release to the v9 identity cleanup on the platform. Two
header additions.

### Added

- **`X-Axonflow-Client: sdk-rust/<version>` header.** This was missing
 in v0.2.0 — a pre-existing gap relative to the four stable SDKs.
 Every governed request now carries it so the platform can derive
 request scope (sdk) and validate against the token's audience scope.
 Sourced from `CARGO_PKG_VERSION`; no env override (the consumer
 doesn't get to spoof its own client identity to the platform).
- **`X-Client-ID: <effective_client_id>` header.** Value matches the
 SDK's Basic Auth username — smart default `community` when no
 `client_id` is configured. Server-side identity decisions no longer
 need to re-decode Basic Auth. The platform's auth middleware
 overwrites the header with its own auth-derived value, so
 caller-supplied values are harmless (no spoofing surface).

Both headers are set on the shared HTTP client header map at
construction time so every endpoint picks them up.

### Compatibility

- Backward-compatible against v8 and v9 platforms: v8 agents ignore the
 unknown header; v9 agents derive identity from Basic Auth regardless.
- No SDK config changes. No removed fields. No changed defaults.

## [0.2.0] - 2026-05-09 — Decision History API + policy_version recorded on every decision + Anthropic interceptor + telemetry simplification

**Preview release.** The headline feature is the new decision-history client API
(`list_decisions`) plus the `explain_decision` example, both bringing Rust to
parity with the four stable SDKs. The other half is a telemetry rework that
brings Rust onto AxonFlow's central anonymous-heartbeat pipeline so adoption is
measurable consistently across all five SDKs.

### Added

- **`list_decisions(opts)`** client method paging through recorded decision history from the orchestrator. Mirrors `GET /api/v1/decisions`. Companion to `explain_decision` — list and drill in. See `examples/list_decisions/`.
- **`AxonFlowConfig::sandbox(client_id, client_secret)`** convenience constructor for local testing. Defaults to `http://localhost:8080`, sets `mode = Mode::Sandbox`, enables debug logging. Parity with Go's `Sandbox()`, Python's `.sandbox()`, TypeScript's `AxonFlow.sandbox()`, Java's `AxonFlow.sandbox(url)`.
- **`WrappedAnthropicClient` invisible-governance interceptor for Anthropic models.** Wrap any client implementing `AnthropicMessageCreator` and AxonFlow pre-checks policy on every `create_message` call, blocks denied calls, and asynchronously audits successful responses. Mirrors the existing `WrappedOpenAIClient` pattern; supports the Anthropic Messages-API request shape (required `max_tokens`, optional `system`). New `examples/anthropic_interceptor/` shows the end-to-end flow.

### Decision explainability

- **`client.explain_decision(decision_id)`** carried forward — fetches the structured `DecisionExplanation` for a previously-made policy decision (matched policies, risk level, override availability, historical hit count, tool signature). New `examples/explain_decision/` shows the end-to-end pattern.

### Fixed

- **URL-encoding parity with the other SDKs.** Path parameters (`connector_id`, `plan_id`, `decision_id`) were percent-encoded with `NON_ALPHANUMERIC`, which over-escapes the RFC-3986 unreserved characters `_`, `-`, `.`, `~`. Connector IDs like `amadeus-travel` were going on the wire as `amadeus%2Dtravel` — wrong wire form that stricter routers would 404. Replaced with a path-segment encode set matching Go's `url.PathEscape` semantics.

### Telemetry

- **Heartbeat endpoint moves to central checkpoint** (`https://checkpoint.getaxonflow.com/v1/ping`). Pre-v0.2 the Rust SDK pinged the local agent — useful for proxy debugging but invisible to the central pipeline. Now in parity with the other four SDKs.
- **`AXONFLOW_TELEMETRY=off` is the sole opt-out** (no programmatic disable; `DO_NOT_TRACK` intentionally NOT honored). Heartbeat payload expanded to the cross-SDK v1 shape (`telemetry_type`, `deployment_mode`, `endpoint_type`, `instance_id`, `stream`); sandbox clients tag `stream="sandbox"`. 7-day per-machine cadence + stamp-on-delivery unchanged.

### Maintenance

- Test-suite mocking library swapped from `httpmock` to `wiremock`. Test-only `dev-dependencies` change with no public API or wire-contract impact; downstream consumers using `axonflow-sdk-rust` as a runtime dependency are unaffected. The previous library's transitive dependency on `async-std` is unmaintained.

## [0.1.0] - 2026-05-05

Initial release of the AxonFlow Rust SDK. The foundation was contributed voluntarily by [@fpierfed](https://github.com/fpierfed) — see [CONTRIBUTORS.md](CONTRIBUTORS.md).

### Added

**Core client (`AxonFlowClient`):**
- `proxy_llm_call` — send governed queries through the AxonFlow agent.
- `audit_llm_call` — gateway-mode logging for direct LLM calls.
- HTTP Basic auth with a `community:` default tenant when no credentials are configured. With credentials: `Authorization: Basic base64(client_id:client_secret)`.
- `X-License-Key` header support for enterprise mode (`AxonFlowConfig::with_license_key`); marked sensitive and redacted in `Debug`.
- Production fail-open + Sandbox propagate-error modes.
- Cache (moka, configurable TTL, mutation-aware), retry with exponential backoff.

**MCP connectors:**
- `list_connectors`, `get_connector`, `get_connector_health`, `query_connector`.
- `install_connector` → `POST /api/v1/connectors/{id}/install`.

**Multi-Agent Planning (MAP):**
- `generate_plan`, `execute_plan`.
- `get_plan_status` → `GET /api/v1/plan/{id}`.
- `cancel_plan` → `POST /api/v1/plan/{id}/cancel`.

**LLM interceptor:**
- `WrappedOpenAIClient` for invisible-governance over any OpenAI-compatible client. Pre-checks policy via AxonFlow, blocks on policy violations, and audits asynchronously after the response.

**Resilience + ergonomics:**
- `AxonFlowConfig` builder (`with_auth`, `with_license_key`, `with_mode`, `with_timeout`, `with_map_timeout`, `with_retry`, `with_cache`).
- Custom `Debug` redacts `client_secret` and `license_key`.
- URL-encodes user-supplied path parameters (connector_id, plan_id).
- Tokio-runtime guard on the heartbeat — safe to construct without an active runtime.
- `AXONFLOW_INSECURE_TLS=1` env to skip TLS verification (debug only). `AXONFLOW_TRY=1` short-circuit for the hosted try endpoint.

**Telemetry:**
- 7-day machine-global anonymous heartbeat for licensing compliance.
- Honors `AXONFLOW_TELEMETRY=off` as the documented opt-out. `DO_NOT_TRACK` is intentionally NOT honored — it is commonly inherited from a parent shell.

**Examples (runnable):**
- `cargo run --example basic` — proxy mode with PII redaction demo.
- `cargo run --example connectors` — list / install / query MCP connectors.
- `cargo run --example planning` — generate + execute a multi-agent plan.
- `cargo run --example interceptors` — invisible governance via `WrappedOpenAIClient`.

**Tests:**
- 17 integration tests via `httpmock` covering proxy / blocked / fail-open / cache / mutation-bypass / retry / list-connectors / generate-plan / install-connector / get-plan-status / cancel-plan, plus the auth-header contract (community default, OAuth2 with creds, clientID-only-no-secret) and `X-License-Key` presence/absence.

**Documentation + ergonomics:**
- README, `docs/ARCHITECTURE.md`, `docs/ERROR_HANDLING.md`.
- `Cargo.toml` is crates.io-ready: explicit `rust-version = "1.78"` MSRV, repository / homepage / documentation / readme metadata, MIT license.

**CI / governance:**
- `test.yml` (fmt, clippy `-D warnings`, build, test, build all examples), `audit.yml` (`cargo audit` weekly + on Cargo.lock change), `integration.yml` (runs all examples against a fresh community docker-compose stack on every PR + weekly cron), `release.yml` (preflights CHANGELOG + Cargo.toml version match, creates GH release on tag, and publishes to crates.io).
- `.github/dependabot.yml` (cargo + github-actions, weekly), `pull_request_template.md`, `CODEOWNERS`.
- DCO sign-off required on every commit (see `CONTRIBUTING.md`).

### Notes

This is a preview release. The Rust SDK currently covers a subset of the surface available in the established Go / Python / TypeScript / Java SDKs (which are at v7.6.x and ship the full governance / workflow / cost / compliance surface). Subsequent releases will expand parity in well-scoped phases — track upcoming work in the [Issues](https://github.com/getaxonflow/axonflow-sdk-rust/issues).
