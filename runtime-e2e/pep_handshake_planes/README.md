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
| `proxy_llm_call()` from a declaring client | nothing: the platform does not resolve the declaration on `/api/request`. This shows only that the platform does not count it there; that the SDK does not send it there is proved on the wire by `tests/pep_handshake_test.rs` (`the_declaration_never_reaches_api_request`) |
| `PEPHandshake::new("Gateway:1", ...)` | refused at construction, with pointer `/pep_id` |

The same run is the control for the refusal phase. With no detection override recorded for the organization, a decide carrying an email address from an undeclared client is allowed, with no redaction obligation.

### `test.sh refusal`

This phase needs a `pii=redact` detection-action override recorded for the organization the requests resolve to. In a deployment that override is written through the detection-posture API. With it in effect:

| Call | Expected |
|---|---|
| `decide()` with an email address, from a client with no declaration | `deny`, a reason with the code `unsupported_obligation` (counted `absent@decision`) |
| the same call from a client declaring only `field_mask@1` | `deny`, a reason with the code `unsupported_obligation` (counted `accepted@decision`): the engine judges the mandatory obligation against the declared capabilities on every edition, Community included |
| the same call from a client declaring `field_redact@1` | `allow`, with the redaction obligation attached (counted `accepted@decision`) |
| `decide_and_fulfill()` from that client | `allow`, and the content forwarded is the engine's redaction, without the address (counted `accepted@decision` and `accepted@mcp`) |

The platform writes a refusal's reason either as the bare code (the engine's own refusal, which is what this phase gets) or as `"<code>: <detail>"` (a subject, validator or wire refusal), so the proof matches a reason by its code. This is the cost of not declaring from platform v11.0.0: under an organization's redact override, a caller that does not declare redaction is refused. The platform reads the declaration from v10.4.0.

`test.sh probe-present` and `test.sh probe-absent` exit 0 once the override is, or is no longer, in effect for the agent. The agent caches an organization's overrides for about a minute, so a runner polls these rather than sleeping.

## What it does not prove

This SDK has no call that sends MCP check-output, so the MCP plane is exercised through check-input only. The platform also reads the declaration on the gateway pre-check and on MCP `tools/call`, and this SDK calls neither. `tests/pep_handshake_test.rs` asserts that no call site reaches any of the three.

The Enterprise-only capability refusal (an allow carrying a mandatory obligation outside the declared set becomes a deny) is the platform's own suites' to prove. What the refusal phase measures is the engine's refusal of an obligation the declaration cannot discharge, which applies on every edition.

## Running it

Boot a Community agent from the platform's main, then:

```bash
AXONFLOW_AGENT_URL=http://localhost:8080 ./runtime-e2e/pep_handshake_planes/test.sh
```

The planes phase needs a Community edition: its per-call step expects `over_advertised`, and an Enterprise agent keeps the approval-family capability and counts `accepted`. `test.sh` first runs the helper's own unit tests (the reason-code predicate), since CI does not build this crate. `AXONFLOW_CLIENT_ID` defaults to `runtime-e2e`, and `AXONFLOW_CLIENT_SECRET` to empty, which community mode accepts. The planes phase writes nothing to the stack beyond the audit rows every governed request leaves. To run the refusal phase, record the organization's `pii=redact` override, wait for `probe-present`, run `refusal`, then remove the override and wait for `probe-absent`.
