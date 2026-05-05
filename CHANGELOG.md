# Changelog

All notable changes to the AxonFlow Rust SDK will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
