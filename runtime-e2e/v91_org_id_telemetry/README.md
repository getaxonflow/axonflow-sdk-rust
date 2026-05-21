# Runtime proof — `org_id` in SDK telemetry payload (v9.1)

Verifies the v9.1 contract for the Rust SDK: every telemetry heartbeat
body carries an `org_id` field, populated from the `ORG_ID` env var
with a `local-dev-org` sentinel fallback. Issue #2277.

Sister proofs across the other 4 SDKs live under the same path:
`runtime-e2e/v91_org_id_telemetry/`.

## Usage

The Rust SDK helper is a separate cargo crate (sibling pattern matching
the Rust `x-client-id/helper/` runner) — `target/` and `Cargo.lock` are
both gitignored at the helper-crate level so building locally doesn't
pollute the SDK workspace.

```sh
cd runtime-e2e/v91_org_id_telemetry/helper

# ORG_ID set — operator-supplied (self-hosted) or cs_<uuid>:
ORG_ID=acme-corp cargo run

# ORG_ID unset — local-dev-org sentinel:
unset ORG_ID && cargo run

# cs_<uuid> Community SaaS tenant identifier:
ORG_ID="cs_f29e9c5c-5c5b-4e0d-8e0d-aabbccddeeff" cargo run
```

Expected output:

```
PASS: telemetry wire payload carries org_id="acme-corp" (expected="acme-corp")
Wire body: {"arch":"aarch64", ... ,"org_id":"acme-corp", ... ,"sdk":"rust", ...}
```

## What it asserts

1. The SDK's `maybe_send_heartbeat` emits a POST to the configured
   checkpoint within seconds of invocation.
2. The body is valid JSON.
3. The body has an `org_id` key.
4. The value matches `$ORG_ID` (when set) or `local-dev-org` (when unset).

## CI coverage

Companion unit tests run in CI via `cargo test`:

- `heartbeat::tests::telemetry_org_id_env_wins`
- `heartbeat::tests::telemetry_org_id_unset_returns_sentinel`
- `heartbeat::tests::telemetry_org_id_empty_falls_through_to_sentinel`
- `heartbeat::tests::telemetry_org_id_cs_prefixed_passes_through`

(Serialized via a `Mutex` to prevent cargo's parallel test runner from
racing env-var mutations across the four tests.)

## Mutation proof

Remove the `payload.insert("org_id".into(), serde_json::Value::from(telemetry_org_id()));`
line from `src/heartbeat.rs::send_heartbeat` and rerun the proof. It exits
with `FAIL: wire org_id = "<MISSING>", want "<expected>"`.

## Cross-SDK parity

Companion runtime-e2e tests live under the same subdirectory in the
other 4 SDKs:

- `axonflow-sdk-go/runtime-e2e/v91_org_id_telemetry/`
- `axonflow-sdk-python/runtime-e2e/v91_org_id_telemetry/`
- `axonflow-sdk-typescript/runtime-e2e/v91_org_id_telemetry/`
- `axonflow-sdk-java/runtime-e2e/v91_org_id_telemetry/`

All five SDKs emit `org_id` with the same wire name, same sentinel
value (`local-dev-org`), and the same precedence (env → sentinel).
