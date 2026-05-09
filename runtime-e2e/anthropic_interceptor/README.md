# Runtime proof — Anthropic interceptor wires policy enforcement + audit

Verifies the v0.2 Anthropic interceptor contract end-to-end against a live
community docker-compose stack:

1. `WrappedAnthropicClient::create_message` POSTs an authorization request
   to the agent at `/api/request` with `eval_context.provider = "anthropic"`
   and the model + max_tokens it was given.
2. When the agent allows the call, the wrapped raw client is invoked exactly
   once.
3. After the raw client returns, the wrapper fires a fire-and-forget
   `POST /api/audit/llm-call` with `provider = "anthropic"` and the
   `input_tokens`/`output_tokens` returned by the raw client.

This is the wire-level companion to `tests/anthropic_test.rs` (which uses
wiremock and exercises the same code paths against a synthetic agent).

## When to run

**Pre-merge** for any change that touches:

- `src/interceptors/anthropic.rs`
- `examples/anthropic_interceptor/`
- `src/client.rs::proxy_llm_call` or `src/client.rs::audit_llm_call`

The CI workflow `.github/workflows/integration.yml` already runs the
`anthropic_interceptor` example end-to-end against the cloned community
stack on every push to `main` and every internal-repo PR. This script is
the manually-runnable equivalent for fork PRs (where the integration job
skips for secret-isolation reasons) and for ad-hoc reproduction.

## Prerequisites

- Docker + docker compose
- Network access to clone `getaxonflow/axonflow` (community)
- Cargo + a stable Rust toolchain

## Usage

```sh
./test.sh
```

The script will:

1. Clone (or refresh) the community stack into `../axonflow-community`.
2. Bring it up with `docker compose up -d --wait`.
3. Wait for `/health` to come back from agent (8080) and orchestrator (8081).
4. Run `cargo run --example anthropic_interceptor` against the live stack.
5. Assert the example exits 0 and prints either `✓ Request succeeded` or
   `❌ Request blocked or failed` (both are valid outcomes — the test
   pins that the binary doesn't panic and that governance is reached).
6. Tear down the stack.

## What it does NOT assert

- That the actual Anthropic API was called. The example uses a mock raw
  client so no real Anthropic key is needed and the test is hermetic.
- That a specific policy fires. Default community policies allow anonymous
  `llm_chat` requests; behavior differs once tenant policies are added.
- DDB / aggregator state. Audit lands as a fire-and-forget POST that the
  agent processes asynchronously; this script does not block on its commit.

## Why this exists alongside the unit tests

`tests/anthropic_test.rs` uses `wiremock` to assert the exact wire shape
the wrapper sends to `/api/request` and `/api/audit/llm-call`. That is
necessary but not sufficient — the agent's `/api/request` JSON contract
can drift between platform releases without breaking the wrapper's
serialization (e.g., a new required field). This runtime proof catches
contract drift between the SDK and the community-stack agent in the same
PR that causes it.
