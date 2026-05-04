# Changelog

All notable changes to the AxonFlow Rust SDK will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.0] - 2026-05-04

This release brings the Rust SDK in line with the AxonFlow platform's wire contract. Without these fixes the v0.1.0 SDK does not authenticate or reach the right plan/connector endpoints against a real AxonFlow agent. **Breaking change:** the auth scheme moved from custom headers to HTTP Basic.

### Changed (breaking)
- **Auth: HTTP Basic, with `community:` default.** Replaces the v0.1.0 `X-AxonFlow-Client-ID` + `X-AxonFlow-Client-Secret` custom headers. With no credentials configured the SDK now sends `Authorization: Basic base64("community:")` to match the cross-SDK community-mode contract; with credentials configured it sends `Authorization: Basic base64("client_id:client_secret")`.
- **Plan endpoint paths: singular, no `/status` suffix.** `get_plan_status()` now hits `/api/v1/plan/{id}`; `cancel_plan()` now hits `/api/v1/plan/{id}/cancel`.
- **Connector install endpoint:** `install_connector()` now POSTs to `/api/v1/connectors/{id}/install` instead of `/api/v1/connectors`.

### Added
- `AxonFlowConfig::with_license_key()` and a `license_key` field. When set, the SDK sends `X-License-Key: <value>` for enterprise-mode license validation (header marked sensitive). Custom `Debug` redacts the value.
- `AXONFLOW_TELEMETRY=off` honored as the documented opt-out for the 7-day heartbeat. `DO_NOT_TRACK` is intentionally NOT honored — it is commonly inherited from a parent shell.
- 9 new tests covering the strict auth contract: community default, OAuth2 with creds, clientID-only-no-secret, X-License-Key presence/absence, and the corrected install + plan endpoint paths.
- `Cargo.toml`: explicit `rust-version = "1.78"` MSRV, `homepage`, `documentation`, `readme` fields. Repository URL corrected from `axonflow/...` to `getaxonflow/...` (was a crates.io publish blocker).
- CI workflows shipped in [#9]: `test.yml` (fmt, clippy, build, test on stable, build all examples), `audit.yml` (`cargo audit`), `release.yml` (preflights CHANGELOG section + Cargo.toml version match).
- `.github/dependabot.yml` covering cargo + github-actions, `.github/pull_request_template.md`, `.github/CODEOWNERS`.

### Fixed
- Resolves issues #3, #4, #5, #6, #7, #8 — see "Changed" / "Added" above for the per-issue mapping.

## [0.1.0] - 2026-05-03

### Added
- Initial implementation of the AxonFlow Rust SDK.
- `AxonFlowClient` with `proxy_llm_call`, `audit_llm_call`, MCP connectors (`list`, `get`, `get_health`, `install`, `query`), and MAP plans (`generate`, `execute`, `get_status`, `cancel`).
- `WrappedOpenAIClient` invisible-governance interceptor for OpenAI-compatible clients.
- `AxonFlowConfig` builder with timeout / map_timeout / retry / cache / TLS-skip options. `client_secret` redacted via custom `Debug`.
- 7-day machine-global heartbeat with per-process `Once` gating and Tokio-runtime guard.
- 4 runnable examples: `basic`, `connectors`, `planning`, `interceptors`.
- 8 integration tests via `httpmock`.

### Known Limitations
- Auth scheme uses `X-AxonFlow-Client-ID` / `X-AxonFlow-Client-Secret` headers — fixed in 0.2.0.
- Plan endpoint paths use `/api/v1/plans/{id}/...` — fixed in 0.2.0.
- Connector install path — fixed in 0.2.0.
- No `AXONFLOW_TELEMETRY=off` opt-out — added in 0.2.0.
- No `X-License-Key` header for enterprise mode — added in 0.2.0.
- `Cargo.toml` repository URL — fixed in 0.2.0.
