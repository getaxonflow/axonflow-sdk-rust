# `create_hitl_request` — runtime-e2e

Real-stack assertion for the cross-SDK
[`create_hitl_request`](https://github.com/getaxonflow/axonflow-enterprise/issues/2421)
surface added in Rust SDK v0.4.0. Sister proof to the equivalent
Python / TypeScript / Go / Java runtime-e2e tests shipping in the
same parity sweep.

## What this proves

Drives `client.create_hitl_request(...)` through the real `reqwest`
transport against a `hyper` HTTP server bound to a real localhost
TCP socket. The server mimics the platform handler at
`platform/agent/hitl/handler.go:177`. Captures the raw HTTP body,
decodes it, and asserts every required field from
`HITLCreateInput` lands on the wire — including the new `notify_url`
field added in
[#2419](https://github.com/getaxonflow/axonflow-enterprise/issues/2419)
— then asserts the SDK parses the platform's `APIResponse{success,
data}` envelope back into a populated `HITLApprovalRequest`.

No `wiremock`, no `MockServer`, no test doubles — runs the production
`reqwest` transport against an in-process `hyper` HTTP server bound to
a real TCP socket, which is what the `runtime-e2e/` DoD gate is asking
for. Built via a temp Cargo project that depends on the local SDK
checkout (`[path = "../.."]`) so the proof always runs against the
in-tree code.

## Usage

```bash
./runtime-e2e/create_hitl_request/test.sh
```

Exits `0` on PASS, non-zero on FAIL. Prints captured wire body +
parsed response fields on success for human-readable confirmation.

## Companion unit coverage

`src/hitl.rs::tests` exercises the same surface through `wiremock`
for 18 scenarios across the full HITL method set
(`list_hitl_queue`, `get_hitl_request`, `create_hitl_request`,
`approve_hitl_request`, `reject_hitl_request`, `get_hitl_stats`),
including: list happy-path + filter serialization, get happy-path +
empty-id guard + 404 propagation, create happy-path + minimal
required-fields + bad-`notify_url`-scheme 400 propagation + 401
propagation + connect-failure propagation + the three required-field
validation guards, approve + reject path/body shape + empty-id guard,
and stats envelope parsing. The runtime proof here is the redundant
real-stack confirmation.
