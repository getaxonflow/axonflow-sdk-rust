# Runtime proof - the Rust SDK relays `/health` onto its telemetry ping

Verifies axonflow-sdk-rust#88: the heartbeat fetches the configured platform's `/health` once and relays `version`, `tier`, `edition` and `deployment_mode` onto the checkpoint ping as `platform_version`, `license_tier`, `edition` and `platform_deployment_mode` - each forwarded verbatim and **omitted when not learned**.

Sister proofs for the other four SDKs live under `runtime-e2e/` in their own repos; they have read `/health` on the telemetry path since enterprise#3619. The platform contract for the two new members is enterprise#3660.

## What makes this a proof rather than a demonstration

* It drives the **real public entry point** - `AxonFlowClient::new` - not the heartbeat module. Nothing is reached into and nothing is mocked; the helper supplies only the platform's side of the conversation.
* It asserts on the **bytes the SDK actually sent**, parsed back from the wire.
* Absence is asked as `has(key)`. A JSON `null` or a substituted default fails exactly as loudly as a wrong value, so "omitted when not learned" is tested rather than assumed.
* The unhappy paths are first-class cases, not an afterthought: an erroring `/health`, a non-JSON `/health`, a `/health` that predates the fields, and - the dangerous one - a `/health` that **succeeds** with a hostile value.

## Usage

The helper is a standalone cargo crate (the sibling pattern used by `v91_org_id_telemetry/helper` and `x-client-id/helper`); `target/` and `Cargo.lock` are gitignored at the helper level so building locally does not disturb the SDK.

```sh
./test.sh
```

One process per scenario. That is not tidiness: the SDK's 1-hour in-process guard is process-wide by design, so a second heartbeat inside one process is precisely what it suppresses.

To also prove the relay against a **live agent** - `/health` answered by the real platform, the ping still captured locally so it can be asserted on:

```sh
AXONFLOW_LIVE_HEALTH_URL=http://localhost:8080 ./test.sh
```

A single scenario:

```sh
cd helper && SCENARIO=hostile cargo run
```

## The matrix

| Scenario | `/health` answers | The ping must carry |
|---|---|---|
| `full` | version, tier, edition, deployment_mode | all four relays, verbatim |
| `pre_3660` | version + tier only | those two; `edition` and `platform_deployment_mode` **absent** |
| `starting` | `tier: "starting"` | `starting` forwarded unchanged - the receiver buckets it deliberately |
| `http_error` | 503 | no relays; the ping is still delivered |
| `not_json` | 200 with HTML | no relays; the ping is still delivered |
| `hostile` | 200 with a quote, a backslash, a newline and an injected `"org_id"` | the value verbatim **as a value**; `org_id` unchanged; no injected key |
| `oversized_value` | a 10 KB `tier` | `license_tier` **absent** (dropped whole, never truncated); `platform_version` survives |
| `live` | the real agent | `version` and `tier` relayed; `edition`/`deployment_mode` reported present-or-absent depending on whether the stack has #3660 |

Every scenario additionally asserts that the ping is otherwise intact (`telemetry_type`, `sdk`, `org_id`, a real `runtime_version`), that exactly **one** `/health` fetch happened, and that the ping stays inside the checkpoint service's 64 KiB request-body limit - the bound an uncapped relay would blow through, taking every other dimension with it.
