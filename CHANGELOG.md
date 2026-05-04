# Changelog

All notable changes to the AxonFlow Rust SDK will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- CI workflows: `test.yml` (fmt, clippy, build, test on stable + MSRV 1.75, build all examples), `audit.yml` (`cargo audit` weekly + on Cargo.lock change), `release.yml` (preflight CHANGELOG + version match, release-on-tag).
- `.github/dependabot.yml` covering cargo + github-actions ecosystems.
- `.github/pull_request_template.md` and `.github/CODEOWNERS`.

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
- Auth scheme uses `X-AxonFlow-Client-ID` / `X-AxonFlow-Client-Secret` headers; will switch to HTTP Basic in 0.2.0 (see issue #3).
- Plan endpoint paths use `/api/v1/plans/{id}/...`; will switch to `/api/v1/plan/{id}` in 0.2.0 (see issue #7).
- Connector install path will switch to `/api/v1/connectors/{id}/install` in 0.2.0 (see issue #4).
- No `AXONFLOW_TELEMETRY=off` opt-out yet (see issue #5).
- No `X-License-Key` header for enterprise mode yet (see issue #6).
- `Cargo.toml` repository URL fix pending (see issue #8).
