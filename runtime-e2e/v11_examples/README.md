# v11_examples

Real-stack proof that the two v11 examples run as the README says. `test.sh` builds `examples/pep_handshake` and `examples/typed_policies` from this tree and runs them against a live Community agent in the README's order. Nothing is mocked.

## What it proves

| Run | Expected |
|---|---|
| 0. `typed_policies` without publishing | exits 0, and nothing is active: the fresh-stack precondition; on a stack a previous run used, the leg stops with exit 2 |
| 1. `pep_handshake` on a fresh stack | exits 0; the first decide's verdict is `allow`; the declaration the platform would refuse fails in the client at `/pep_id`, before anything is sent |
| 2. `typed_policies` with `AXONFLOW_TYPED_POLICY_PUBLISH=1` | exits 0; the document is published and activated |
| 3. `typed_policies` asked to publish a document the save-time checks reject | exits non-zero, printing the platform's typed `422 document_refused`: an example that was asked to publish and could not must not report success |
| 4. `pep_handshake` again, after the activation | printed as an observation, not asserted (below) |

Run 3 publishes a copy of `testdata/typed_policy_publish_body.json` with one action the registry does not contain, written under cargo's target directory. It is refused before anything is admitted, so it changes nothing on the stack.

## Why the fourth run is an observation

After a document with an organization-scope constraint is activated, a decide that does not supply the attribute the constraint conditions on is denied fail-closed with reasons `["unknown_constraint"]`. The example's default document is such a document, so the fourth run shows that deny. It is the platform's by-design answer, not the SDK's, so this leg prints it rather than pinning it. It is why the README runs the handshake example first.

The stack this leg is pinned to (`857455033`, shared with the other SDKs' legs) predates getaxonflow/axonflow-enterprise#4247 (`f33f6d351`), so the fourth run shows only the bare `["unknown_constraint"]`. A v11.0.0 platform names each constraint it could not evaluate after that code.

## What it does not prove

That the handshake reaches the wire. A fresh Community stack allows the first decide with or without a declaration, and the `/pep_id` refusal happens in the client, so this leg would pass without the header. The wire proof is `runtime-e2e/pep_handshake_planes`.

## Run

Community on the application database role, on a fresh stack:

```
AXONFLOW_AGENT_URL=http://localhost:8080 ./runtime-e2e/v11_examples/test.sh
```

Every run uses `AXONFLOW_CLIENT_ID` and `AXONFLOW_CLIENT_SECRET`, defaulting to `runtime-e2e` / `runtime-e2e-secret`. This SDK's `decide` names no organization in its request body (`caller_identity` is sent empty), so a Community agent answers it with credentials set. It exits 0 when every assertion passes, 1 when one fails, and 2 when the agent is not reachable or the stack is not fresh. It changes the organization's active policy.
