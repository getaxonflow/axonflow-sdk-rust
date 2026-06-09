# runtime-e2e: Decision Mode PEP — decide → fulfill → forward

Runtime proof of the Rust SDK's Decision Mode PEP contract (epic
getaxonflow/axonflow-enterprise#2563, tracking #2571) against a **live
enterprise agent**. NO mocks.

Mirrors the Python SDK's `runtime-e2e/decide_fulfill_obligation/` runner.

## What it proves

Driving the real SDK code path (`AxonFlowClient::decide` /
`fulfill_request` / `decide_and_fulfill`) against the agent at
`AXONFLOW_ENDPOINT` (default `http://localhost:8080`):

1. `decide()` on the PII-bearing query
   `"Send the receipt to john.doe@example.com and charge card 4111111111111111"`
   returns an **allow** verdict carrying a request-phase `redact_pii`
   obligation.
2. `fulfill_request()` round-trips the query through the engine's
   `/api/v1/mcp/check-input` endpoint and returns ENGINE-masked content in
   which **neither `john.doe@example.com` nor `4111111111111111` survives**,
   and the content differs from the original.
3. `decide_and_fulfill()` produces the same masked content in one call.
4. Demo / wrong credentials (`demo-org` / `demo-license-not-real`) are
   refused with **HTTP 401** (`AxonFlowError::ApiError { status: 401, .. }`),
   never silently allowed.

The SDK contains **no redaction logic of its own** — fulfillment is always the
engine round-trip. An obligation the engine cannot discharge fails closed with
`AxonFlowError::ObligationNotFulfillable`, never by forwarding the unredacted
query.

## Running

```bash
source /tmp/axonflow-e2e-env.sh   # provides AXONFLOW_CLIENT_ID / _SECRET / _TENANT_ID / _USER_TOKEN
./test.sh
```

Auth is HTTP Basic `org:license` — `AXONFLOW_CLIENT_ID` is the org id
(Basic username) and `AXONFLOW_CLIENT_SECRET` is the Ed25519 license
(Basic password), exactly as the `/api/v1/decide` PDP requires.

See `EVIDENCE/` for captured passing runs.
