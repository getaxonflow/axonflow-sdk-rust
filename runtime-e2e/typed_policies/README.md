# Typed policy authoring through the Rust SDK

`test.sh` builds and runs the helper crate, which drives the six operations of `client.typed_policies()` (`edition`, `validate`, `publish`, `activate`, `active` and `system`) against a real agent and orchestrator. Nothing is mocked.

## What it proves

On a fresh stack, in order:

1. Nothing is active yet: `active()` answers `None`, from the platform's 404. This first assertion depends on a FRESH stack. Every run activates a document, so a re-run on a stack a previous run used fails it by design, and that failure is not evidence of a defect. Boot a new stack for each run.
2. `edition()` reports the deployment's boundary: the `organization` root, and its construct report. `system()` reports the shipped controls with their digest.
3. The document the platform's own route test proves publishable validates clean. It then publishes to a digest at version 1, and activates.
4. `active()` returns that document as the exact signed source, and the source parses to the document. The platform overwrites the author: the document deliberately names `someone-else`, and the platform signs the caller the agent resolved.
5. Activating the same digest again is a typed 409 `activation_refused`: activation promotes, and here the version does not advance.
6. Publishing with no fixtures is a typed 422 `publication_refused` whose message names the missing fixtures.
7. A document naming an action the registry does not hold validates with the platform's rejecting finding (`ACTION_NOT_REGISTERED` on `grant.refund`). Publishing it is a typed 422 `document_refused` carrying that finding.
8. A client with no credentials gets a readable answer (an edition or a typed refusal), not a transport error.

The document is `testdata/typed_policy_publish_body.json`, byte-identical to the other SDKs' copies: the body the platform's own route test proves publishable. Its `document_id` is made unique per run.

## What it does not prove

Rolling back and withdrawing are customer portal operations the agent does not proxy, so the SDK has no method for either. An edition with separation of duties refuses every publication through this route with `APPROVER_IS_AUTHOR`; the unit tests cover that refusal's shape, and this proof runs on Community, which has no separation of duties. A `401` keeps the client's authentication error; the unit tests assert that, because a Community agent accepts any credentials.

After this proof activates its document, a `decide` that does not supply the attribute the document's organization-scope constraint conditions on (`args.request_type`) is denied fail-closed with reasons `["unknown_constraint"]`. Run any decide proof before this one, or on a fresh stack.

## Running it

Boot a Community stack from the platform's main, with the agent and orchestrator on the application database role so it behaves as a deployment does. Then:

```bash
AXONFLOW_AGENT_URL=http://localhost:8080 ./runtime-e2e/typed_policies/test.sh
```

Run it against a fresh stack: the first assertion needs nothing active on the organization, and each run activates a document.
