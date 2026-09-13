# PEP capability handshake: the Rust SDK's declaration reaches the platform

`test.sh` builds and runs the helper crate against a real AxonFlow agent. Around each SDK call it reads the agent's own counter, `axonflow_pep_handshake_total{outcome, plane}` on `/prometheus`. The agent moves that counter once per inbound request on each plane that resolves the declaration. Every assertion is something the agent recorded. There are no mocks.

## What it proves

### `test.sh planes` (the default)

| Call | The agent counts |
|---|---|
| `decide()` from a client declaring `field_redact@1` | `accepted@decision` +1 |
| `evaluate()`, then `evaluate_all()` | `accepted@access_evaluation` +1 each |
| `fulfill_request()` with a request-phase redaction obligation (its MCP check-input round-trip) | `accepted@mcp` +1 |
| `with_pep_handshake(...)` declaring `approval_challenge@1`, then `decide()` | `over_advertised@decision` +1: a Community agent drops the approval-family capability, so the outcome tells the derived client's document apart from the base client's |
| `decide()` again from the base client | `accepted@decision` +1: deriving a client did not change the base client |
| `decide()` from a client with no declaration | `absent@decision` +1: the SDK sends nothing it was not given |
| `proxy_llm_call()` from a declaring client | nothing: `/api/request` does not read the declaration, and the SDK does not send it there |
| `PEPHandshake::new("Gateway:1", ...)` | refused with pointer `/pep_id` before anything is sent; the counter does not move |

The same run is the control for the refusal phase. With no detection override recorded for the organization, a decide carrying an email address from an undeclared client is allowed, with no redaction obligation.

### `test.sh refusal`

This phase needs a `pii=redact` detection-action override recorded for the organization the requests resolve to. In a deployment that override is written through the detection-posture API. With it in effect:

| Call | Expected |
|---|---|
| `decide()` with an email address, from a client with no declaration | `deny`, reasons include `unsupported_obligation` (counted `absent@decision`) |
| the same call from a client declaring `field_redact@1` | `allow`, with the redaction obligation attached (counted `accepted@decision`) |
| `decide_and_fulfill()` from that client | `allow`, and the content forwarded is the engine's redaction, without the address (counted `accepted@decision` and `accepted@mcp`) |

This is the cost of not declaring from platform v11.0.0: under an organization's redact override, a caller that does not declare redaction is refused. The platform reads the declaration from v10.4.0.

`test.sh probe-present` and `test.sh probe-absent` exit 0 once the override is, or is no longer, in effect for the agent. The agent caches an organization's overrides for about a minute, so a runner polls these rather than sleeping.

## What it does not prove

This SDK has no call that sends MCP check-output, so the MCP plane is exercised through check-input only. `tests/pep_handshake_test.rs` asserts that no call site sends check-output.

The Enterprise deny for an admitted declaration that cannot discharge a mandatory obligation is the platform's own suites' to prove. A Community agent records a declaration and does not deny on it.

## Running it

Boot an agent from the platform's main (community mode is enough), then:

```bash
AXONFLOW_AGENT_URL=http://localhost:8080 ./runtime-e2e/pep_handshake_planes/test.sh
```

`AXONFLOW_CLIENT_ID` defaults to `runtime-e2e`, and `AXONFLOW_CLIENT_SECRET` to empty, which community mode accepts. The planes phase writes nothing to the stack beyond the audit rows every governed request leaves. To run the refusal phase, record the organization's `pii=redact` override, wait for `probe-present`, run `refusal`, then remove the override and wait for `probe-absent`.
