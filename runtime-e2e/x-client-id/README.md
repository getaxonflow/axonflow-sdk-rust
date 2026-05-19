# Runtime proof — Rust SDK emits v9 `X-Client-ID` + ADR-050 §4 `X-Axonflow-Client`

Brings up the community `docker-compose` stack, then runs an in-process
forwarding-proxy helper (`helper/`) that points the Rust SDK at a local
TCP listener, captures the SDK's outbound HTTP headers off the wire,
and forwards the request through to the real agent.

This is the wire-level companion to `tests/x_client_id_header_test.rs`
(which uses `wiremock` and asserts the same four headers against a
synthetic agent). Both must pass for the v9 identity contract to be
considered held.

## When to run

**Pre-merge** for any change that touches:

- `src/client.rs::new` (the headers are set on construction)
- `Cargo.toml` (a version bump changes the expected `X-Axonflow-Client`)
- The agent's `apiAuthMiddleware` upstream (`X-Client-ID` is overwritten
  server-side with the auth-derived value; client-side spoofing is harmless,
  but the SDK still needs to emit the header for the auth path to function
  correctly in mode-mismatch scenarios)

It is the same shape as `runtime-e2e/anthropic_interceptor/` — boots the
public community stack, runs a real SDK call against it. Matches the
parity established by the other 4 SDKs' `runtime-e2e/x-client-id/`
runners shipped in PR getaxonflow/axonflow-enterprise#2230 (workstream B).

## Prerequisites

- Docker + docker compose
- Network access to clone `getaxonflow/axonflow` (community)
- Cargo + a stable Rust toolchain

## Usage

```sh
./test.sh
```

Optional env vars:

- `AXONFLOW_TENANT_ID` — Basic Auth username (default: `demo-client`)
- `AXONFLOW_TENANT_SECRET` — Basic Auth password (default: `demo-secret`)

The script will:

1. Clone (or refresh) the community stack into `../axonflow-community`.
2. Bring it up with `docker compose up -d --wait`.
3. Wait for `/health` to come back from the agent on port 8080.
4. Build + run `helper/` against the live stack, with the helper's
   in-process forwarder bound on `127.0.0.1:0`.
5. Assert all 4 header invariants (see below).
6. Tear down the stack.

## What it asserts

The helper (`helper/src/main.rs`) reads the SDK's outbound HTTP headers
off the wire after the SDK has called `proxy_llm_call`, then verifies:

| Header | Required | Expected value |
|---|---|---|
| `X-Client-ID` | present | equals `AXONFLOW_TENANT_ID` |
| `X-Axonflow-Client` | present | starts with `sdk-rust/` (ADR-050 §4) |
| `Authorization` | present | starts with `Basic ` |
| `X-Tenant-ID` | ABSENT | (the agent still accepts it as an alias for back-compat through v9, but the SDK standardizes on `X-Client-ID` post-v0.3.0) |

Helper exits 0 on all-pass; 1 on any failed assertion.

## What it does NOT assert

- The agent's response correctness (the SDK call may succeed or fail
  depending on whether `demo-client/demo-secret` is provisioned in the
  community stack — neither outcome affects the header verdict, since
  the headers are read off the request side of the wire before any
  response).
- Server-side persistence (this is a pure SDK-emission test).
- Any other header beyond the four above.

## Why this exists alongside the unit tests

`tests/x_client_id_header_test.rs` uses `wiremock` to assert the SDK's
emission against a synthetic agent. That's necessary but not sufficient
— the agent's request-acceptance contract can drift between platform
releases without breaking the wiremock matcher (e.g., a new header the
agent's middleware requires). This runtime proof catches contract drift
between the SDK and the community-stack agent in the same PR that
causes it.

It also matches the cross-SDK parity contract: every first-class SDK
(Go, Python, TS, Java, Rust) now ships a `runtime-e2e/x-client-id/`
runner. Drift in one SDK is caught locally; drift in the agent's
handling of the header is caught by all 5 simultaneously.
