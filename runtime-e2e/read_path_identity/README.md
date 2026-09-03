# read_path_identity - per-user identity on the READ path (platform #2922)

Real-wire proof, through the Rust SDK's own runtime against a **live enterprise
stack**, that `explain_decision` and `list_decisions` are scoped to the identity
the caller presents - and that the SDK reports the outcomes honestly instead of
collapsing them into "nothing there".

## The defect this pins

All five SDKs carried `user_token` as a **write-path body field only**, so both
read methods asked the platform anonymously. Measured live before the fix:

```
$ curl -H "Authorization: Basic $AUTH" .../api/v1/decisions?limit=5
HTTP/1.1 200 OK
X-Axonflow-Read-Scope: none
{"decisions":[]}
```

A `200` with an empty page, indistinguishable from a tenant that has made no
decisions - and `explain` answering `404` for ids that plainly existed.

## What the driver asserts

| # | Step | Why it cannot pass vacuously |
|---|---|---|
| 1 | Write 3 decisions as dev-a | - |
| 2 | List as dev-a, then **dev-b writes one** | Floor is **the number this run wrote**, each checked **by id**. The floor alone cannot tell own-rows from tenant-wide, so dev-b then writes a row and dev-a's page must **not grow** |
| 3 | Explain as dev-a | Asserts a context value **this run chose**, not merely "non-empty" |
| 4 | List with **no identity** | Must be `AxonFlowError::ReadScope` with `identity_missing`, never `[]`. A stack that returns rows here fails loudly - every other scoping assertion would be vacuous |
| 5 | Explain dev-a's decision **as dev-b** | Must refuse, and must **not** report a missing identity - dev-b presented one |
| 6 | **Malformed / expired / another-org** tokens | Each must fail **closed** with 401 and not echo the credential, and explicitly **not** as a `ReadScope` refusal - a rejected token reported as a scoping outcome would mean it degraded to the unscoped path |
| 7 | Explain as **admin** | Without it, step 5 is unfalsifiable: a read broken for everyone also "refuses dev-b" |
| 8 | `as_user` | A derived client must be scoped to the identity it was derived FOR. The Python sibling shipped exactly the bug this catches: a derived client silently keeping the ORIGINAL identity |
| 9 | No leak | The token must appear in **no** request reaching the telemetry collector this driver hosts, and the step **fails if the collector received nothing** |
| 10 | Observable | The orchestrator must have **recorded** the unscoped read |
| 11 | Non-read routes | A valid identity reaches `list_connectors` (**control**); a malformed one is refused **401** there |

## Three traps this driver exists to not fall into

**Identities are minted at `@example.com`, never `@axonflow.local`.** The
platform reserves that whole domain (and `@axonflow.internal`) for *shared,
non-personal* identities and censuses them to nothing before scoping. A
perfectly valid developer token minted there reads **zero rows** and reports
scope `none` - identical to presenting no token at all. `generate-jwt.sh`'s own
default (`demo-user@axonflow.local`) lands in the reserved domain, so a driver
built on it would prove nothing about own-rows scoping while appearing to pass.

**Tokens are minted in-process.** The scoping assertions need *several distinct*
identities - two developers, an admin, an expired one, one from another org. A
single shared env token cannot express them, and the setup script's token is
`role=admin`, which short-circuits to tenant-wide and would make steps 4-8
untestable.

**The telemetry stamp is PARKED and restored, not deleted** (`test.sh`). It
lives in the developer's real cache dir; deleting it would make their next
unrelated SDK run fire a genuine ping at the production checkpoint. Without the
park the collector is empty on every run after the first, and step 9 fails
loudly rather than passing on an unasserted absence.

## Run

```bash
# 1. Enterprise stack, FROM THE axonflow-enterprise CHECKOUT, per
#    axonflow-internal-docs/engineering/E2E_EXAMPLES_TESTING_WORKFLOW.md
(cd /path/to/axonflow-enterprise && ./scripts/setup-e2e-testing.sh enterprise)

# 2. Then, FROM THIS REPO's root:
set -a; source /tmp/axonflow-e2e-env.sh; set +a
export AXONFLOW_AGENT_URL=http://localhost:8080
./runtime-e2e/read_path_identity/test.sh
```

Env: `AXONFLOW_AGENT_URL`, `AXONFLOW_CLIENT_ID`, `AXONFLOW_CLIENT_SECRET`,
`JWT_SECRET` (or `AXONFLOW_JWT_SECRET`). Optional `AXONFLOW_ORCH_CONTAINER`
(default `axonflow-orchestrator`) for step 10.

**Step 11 is round 2's step.** The identity is stamped in `dispatch`, so it
rides every request rather than only the two role-scoped reads - which is what
the other four SDKs already did, and what `as_user`'s "reaches EVERY method"
promise requires. It carries a control leg on purpose: a step that only
asserted the 401 would pass identically on a stack where `list_connectors` is
simply down, and would then report an outage as an access-control property.
Verified falsifiable - against the pre-round-2 SDK the control passes and the
refusal leg FAILS, because the identity never reached that route.

Exits non-zero on the first failed assertion.
