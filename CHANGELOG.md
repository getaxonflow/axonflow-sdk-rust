# Changelog

All notable changes to the AxonFlow Rust SDK will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
