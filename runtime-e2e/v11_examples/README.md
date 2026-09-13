# v11_examples

Real-stack proof that the two v11 examples run as the README says. `test.sh` builds `examples/pep_handshake` and `examples/typed_policies` from this tree and runs them against a live Community agent in the README's order, the handshake example first. Every run executes from a directory outside the tree, and the runs that match the README's commands set no body file. Nothing is mocked.

## Precondition

Before any run, `curl` asks `GET /api/v1/typed-policies/active` and requires `404` with reason `nothing_active`: no typed document is active. Otherwise the leg stops with exit 2, because it changes the organization's active policy and needs a fresh stack. The check is made outside the SDK, so an SDK regression is never reported as a stale stack. It sends no credentials: on Community every call's organization is the deployment's (`ORG_ID`), so it asks about the organization the examples use.

## What it proves

| Run | Expected |
|---|---|
| 1. `pep_handshake` | exits 0; the first decide's verdict is `allow`; the declaration the platform would refuse fails in the client at `/pep_id`, before anything is sent |
| 2. `typed_policies` without publishing, the README's default | exits 0; `active()` reads the platform's `nothing_active` as nothing active |
| 3. `typed_policies` with `AXONFLOW_TYPED_POLICY_PUBLISH=1`, the README's command | exits 0; prints the publish response's template-omission report; the document is published and activated |
| 4. `typed_policies` asked to publish a document the save-time checks reject | exits 1, printing the platform's typed `422 document_refused`: an example that was asked to publish and could not must not report success |
| 5. `pep_handshake` again, after the activation | printed as an observation, not asserted (below) |

Run 4 publishes a copy of `testdata/typed_policy_publish_body.json` with one action the registry does not contain (the same edit `runtime-e2e/typed_policies/helper` makes), written under cargo's target directory. It is refused before anything is admitted, so it changes nothing on the stack.

## Why the fifth run is an observation

After a document with an organization-scope constraint is activated, a decide that does not supply the attribute the constraint conditions on is denied fail-closed with reasons `["unknown_constraint"]`. The example's default document is such a document, so the fifth run shows that deny. It is the platform's by-design answer, not the SDK's, so this leg prints it rather than pinning it. It is why the README runs the handshake example first.

The stack this leg is pinned to (`857455033`, shared with the other SDKs' legs) predates getaxonflow/axonflow-enterprise#4247 (`f33f6d351`), so the fifth run shows only the bare `["unknown_constraint"]`. A v11.0.0 platform follows that code with one reason naming each constraint it could not evaluate and the attribute it needed.

## What it does not prove

- **That the handshake reaches the wire.** A fresh Community stack allows the first decide with or without a declaration, and the `/pep_id` refusal happens in the client, so this leg would pass without the header. The wire proof is `runtime-e2e/pep_handshake_planes`.
- **That the organization carries no other state.** The precondition proves no typed document is active. A recorded detection override, or a legacy per-policy override (the pinned stack predates getaxonflow/axonflow-enterprise#4221, which retires them), would still apply. And while getaxonflow/axonflow-enterprise#4255 is open, an unreadable document store also answers `nothing_active`.

## Run

Community on the application database role, on a fresh stack:

```
AXONFLOW_AGENT_URL=http://localhost:8080 ./runtime-e2e/v11_examples/test.sh
```

The runs use `AXONFLOW_CLIENT_ID` and `AXONFLOW_CLIENT_SECRET`, defaulting to `runtime-e2e` / `runtime-e2e-secret`. On Community every call's organization is the deployment's (`ORG_ID`), whatever the credentials, and this SDK's `decide` names no organization in its request body (`caller_identity` is sent empty). It exits 0 when every assertion passes, 1 when one fails, and 2 when the agent is not reachable or a typed document is already active. It changes the organization's active policy.
