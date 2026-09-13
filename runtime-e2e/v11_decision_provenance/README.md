# v11 decision provenance: the Rust SDK reads what decided a request

`test.sh` builds and runs the helper crate against a real AxonFlow agent, and asserts on what the platform answered. There are no mocks.

## What it proves

| Call | Expected answer |
|---|---|
| `decide()` | `engine` is `anchored`, with a `policy_bundle` digest and a `subject_type`. When `policy_identities` is present, it names `evaluated_policies` one for one, in order. |
| `POST /api/v1/mcp/check-output`, read into the SDK's `MCPCheckOutputResponse` | the same provenance. The SDK has no check-output call of its own, so the helper makes the request directly and deserializes the platform's body into the SDK's public type. |
| `proxy_llm_call()` with the statement the platform's own detection corpus uses for the shipped control `sys_dangerous_injection_override` | blocked by the anchored engine. The 403's body carries `engine`, `subject_type` and `policy_bundle`, and the SDK reads all three. |
| `query_connector()` with the same statement | the query is dispatched through `/api/request` and blocked the same way. The `ConnectorResponse` the SDK builds from that answer by hand carries the same provenance. |

`sys_dangerous_injection_override` is one of the controls the shipped posture sets to `block` on `/api/request`, so the refusal is the anchored engine's own on every edition. It does not depend on an organization override or an LLM provider.

## What it does not prove

`legacy_validators` appears only when an organization records a `pii=block` or `pii=redact` detection override and a checksum validator acts. A fresh stack has no such override, so the field is absent here. The unit tests in `tests/v11_wire_fields_test.rs` assert its shape, including a JSON `null`.

## Running it

Boot an agent from the platform's main (community mode is enough), then:

```bash
AXONFLOW_AGENT_URL=http://localhost:8080 ./runtime-e2e/v11_decision_provenance/test.sh
```

`AXONFLOW_CLIENT_ID` defaults to `runtime-e2e`, and `AXONFLOW_CLIENT_SECRET` to empty, which community mode accepts. The run writes nothing to the stack beyond the audit rows every governed request leaves.
