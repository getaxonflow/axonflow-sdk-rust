# Runtime proof — Sandbox-mode telemetry fires with stream=sandbox (v0.2)

Verifies the v0.2 contract: a `AxonFlowConfig::sandbox(...)`-constructed
client produces an anonymous heartbeat ping that lands in the central
checkpoint DynamoDB with the row tagged `sdk=rust/0.2.0` AND
`stream="sandbox"`.

## When to run

**Post-deploy verification.** Three infrastructure prerequisites:

1. **`axonflow-enterprise` PR #2005 deployed.** Without the server-side
   wire-allowlist, the Lambda hardcodes `stream=heartbeat` regardless
   of payload, and this test fails at the assertion step.
2. **`"rust"` is in the server-side `ValidSDKs` map.** Check
   `ee/platform/checkpoint-service/pkg/telemetry/telemetry.go`. If
   missing, every Rust ping returns HTTP 400 (`invalid sdk value`) and
   this test fails at step 1. Filing the server-side companion is part
   of the v0.2 release plan.
3. **AWS credentials** with read on `/aws/lambda/prod-axonflow-checkpoint`.

## Usage

```sh
AWS_REGION=us-east-1 ./test.sh
```

## What it asserts

1. Builds a tiny Cargo binary against the local SDK via `path = "../.."`.
2. The program calls `AxonFlowConfig::sandbox(...)` (new in v0.2).
3. CloudWatch records an `event_stored` row with `sdk=rust/0.2.0` AND
   `stream=sandbox`.

## Pre-v0.2 behavior

The Rust heartbeat targeted `{endpoint}/api/telemetry/heartbeat` (local
agent only); the central checkpoint Lambda never saw Rust pings. v0.2
routes through `https://checkpoint.getaxonflow.com/v1/ping` matching
the other 4 first-class SDKs, so adoption is now measurable consistently
across the full SDK matrix.
