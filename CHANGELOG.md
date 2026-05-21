# Changelog

All notable changes to the AxonFlow Rust SDK will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **`org_id` field in the telemetry heartbeat body (v9.1 preflight, #2277).**
 Brings Rust SDK telemetry up to parity with the platform's
 `startup_telemetry.go` emitter — every heartbeat now identifies which
 deployment-organization emitted it. Two sources in precedence order:
 1. The `ORG_ID` env var when set (the operator's explicit configuration
    on self-hosted deployments, or the `cs_<uuid>` tenant identifier on
    Community SaaS).
 2. Otherwise the `local-dev-org` sentinel.

 Exposed as `axonflow_sdk_rust::heartbeat::telemetry_org_id()` and
 `axonflow_sdk_rust::heartbeat::ORG_ID_LOCAL_DEV_SENTINEL`. Always
 emitted. The receiver already accepts the field with `omitempty` for
 backward compat with pre-v0.4 SDKs that don't send it. Honors
 `AXONFLOW_TELEMETRY=off` like every other heartbeat field. See
 `axonflow-landing/content/privacy.html` for the customer-facing
 commitment that covers this field.

- **`test_401_not_retried_issue_2275`** regression test in
 `tests/integration_test.rs` — locks in that HTTP 401 responses are
 terminal in `execute_with_retry` and never trigger a retry. Backstops
 issue [#2275](https://github.com/getaxonflow/axonflow-enterprise/issues/2275)
 where a customer's misconfigured deployment caused a tight 401 retry
 loop against community-saas (~30 401/hour from a single source IP).
 Rust SDK was already safe — 401 falls through the early `return Err(e)`
 path in `src/client.rs:560-565` because it isn't in the
 `{429, 402, 403}` retry-allowlist — but there was no explicit test for
 the contract until now. Mutation-tested: adding `401` to the allowlist
 fails the test, confirming the assertion isn't tautological.
- **`test_429_is_retried_allowlist_contract`** companion regression
 test — locks in the OTHER direction of the allowlist: that HTTP 429
 (rate limit) DOES trigger retries up to `max_attempts`. Without this,
 a future refactor dropping `*status != 429` from `execute_with_retry`
 would silently make all 4xx terminal — breaking the rate-limit retry
 contract — and `test_401_not_retried_issue_2275` alone wouldn't catch
 it (401 stays terminal either way). Together the two tests bracket
 the allowlist boundary. Mutation-tested: deleting the `*status != 429`
 clause fails the test (wiremock panics on Drop because the mock is
 only called once instead of `max_attempts` times).

### Changed

- **Telemetry-enabled log line** softened from "Anonymous telemetry
 enabled" to "Telemetry enabled" to stay coherent with the v9.1
 `org_id` addition (the operator-supplied `ORG_ID` on self-hosted is
 not anonymized; only the `instance_id` and `cs_<uuid>` Community SaaS
 identifier remain anonymous-by-design). README "Telemetry" section
 and the `src/heartbeat.rs` module docstring softened similarly.

### Documentation

- **Rustdoc on `execute_with_retry`** (`src/client.rs:532`) describing
 the retry contract — which status codes retry (5xx + 429), which are
 terminal (401 + everything else 4xx outside the allowlist) — citing
 issue [#2275](https://github.com/getaxonflow/axonflow-enterprise/issues/2275)
 and CHANGELOG.md for history.
- **Clarifying comment above the retry-allowlist** explaining that
 402/403 are handled as success responses in `execute_request`
 (`src/client.rs:586`) and never propagate to `execute_with_retry` as
 errors, so the `*status != 402` / `*status != 403` clauses in the
 allowlist are intentional belt-and-suspenders defense for any future
 refactor of `execute_request` that converts 402/403 back to `Err`.

## [0.3.1] - 2026-05-20 — `runtime-e2e/x-client-id/` parity with the other 4 SDKs

**Patch release — test infrastructure only.** No SDK code changes; pure
parity work for the v9 identity rollout (Epic #2230, workstream B).

### Added

- **`runtime-e2e/x-client-id/`** runner — bash entry point + Rust
 helper crate. Mirrors the Go/Python/TS/Java SDKs' `runtime-e2e/x-client-id/`
 directories shipped in workstream B. Brings up the public community
 docker-compose stack, then runs an in-process forwarding-proxy helper
 that captures the SDK's outbound HTTP headers off the wire and asserts:
 `X-Client-ID == AXONFLOW_TENANT_ID`, `X-Axonflow-Client` starts with
 `sdk-rust/`, `Authorization` starts with `Basic `, and `X-Tenant-ID`
 is absent.

This is the wire-level companion to `tests/x_client_id_header_test.rs`
— which uses `wiremock` and is necessary but not sufficient (it can't
catch contract drift between the SDK and the live community-stack
agent in the same PR that causes it).

## [0.3.0] - 2026-05-19 — `X-Axonflow-Client` + `X-Client-ID` headers on every outbound request (v9 identity)

**Companion release to the v9 identity cleanup on the platform (Epic #2230).**
Two header additions:

### Added

- **`X-Axonflow-Client: sdk-rust/<version>` header (ADR-050 §4).** This
 was MISSING in v0.2.0 — a pre-existing gap relative to the four stable
 SDKs. Every governed request now carries it so the agent can derive
 request scope (sdk) and validate against the token's aud.scope via
 `HasScope()`. Sourced from `CARGO_PKG_VERSION`; no env override (the
 consumer doesn't get to spoof its own client identity to the agent).
- **`X-Client-ID: <effective_client_id>` header (v9 identity).** Value
 matches the SDK's Basic Auth username — smart default `community`
 when no `client_id` is configured. Server-side identity decisions no
 longer need to re-decode Basic Auth. The agent's `apiAuthMiddleware`
 overwrites the header with its own auth-derived value, so caller-
 supplied values are harmless (no spoofing surface).

Both headers are set in `AxonFlowClient::new` (`src/client.rs`) on the
shared `HeaderMap` that both `http_client` and `map_http_client` reuse,
so every endpoint picks them up.

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
