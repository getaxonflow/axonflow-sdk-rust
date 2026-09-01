# authzen_evaluation (getaxonflow/axonflow-enterprise#3616, epic #3603)

Real-stack proof that the Rust SDK's AuthZEN-native surface - `AxonFlowClient::evaluate` and `AxonFlowClient::evaluate_all` against `POST /api/v1/access/evaluation` - answers, agrees with the evaluation the legacy Decision API runs, and refuses what it cannot evaluate by name. Driven entirely through the SDK's real public API against a live agent. No mocks.

## What this proves that the unit suite cannot

`tests/authzen_surface_test.rs` pins what the client does with a given body: it can serve a decision whose boolean and state disagree, a profile from 2099, or a 200 with no context, none of which a real server will produce on request. What it cannot establish is anything about the server.

1. **Agreement.** The route is an *adapter* over the evaluation that serves `POST /api/v1/decide`. The release constraint is that it answers with the *same* evaluation. A stubbed transport can assert the client sends the right bytes; only a live stack can assert the two surfaces agree about a real policy decision - and it is asserted in **both** directions, because agreement on allow alone would be satisfied by a route that always allows.

2. **Both sides name the same MEMBER.** The SDK refuses an incomplete subject locally, at `/evaluation/subject/type`. The server refuses the same bytes at the same pointer. A unit test can pin the local half; only a live server can establish that the two name the **same member**.

   The **codes are not equal, and that is not a defect** - the suite prints both rather than asserting equality. This client knows only that a required member is missing (`incomplete_evaluation`); the server additionally knows which values it can evaluate and narrows the same condition to `unsupported_subject` with a `supported` list. What the suite *does* assert is that the server's code is one this build knows: a code outside the closed enumeration means the contract moved and this SDK cannot read the refusal, which no unit test can discover.

3. **The bare-boolean case is real.** The SDK refuses a `200` carrying no profile payload, on the grounds that the obligations and the approval challenge that constrain an allow ride in it. That guard is only worth something if a server can actually produce such a body, so this sends one un-negotiated request and asserts the response has a `decision` and no `context`.

4. **An unresolvable attribute never reaches the network.** Asserted by pointing the *real* client at a port nothing is listening on: a typed refusal from that client is proof the check ran before any I/O. No amount of stubbing establishes that - a stub answers, so a request that reached it looks the same as one that did not.

## The three attribute states, observed

The lane's central claim is that a resolved attribute has three states and `Option` carries two. Three assertions here make the difference observable against a real server, on one member (`subject.properties.department`):

| state | what the caller knows | wire | server |
|---|---|---|---|
| `Attribute::absent()` | the directory answered: no department | the MEMBER is omitted; the bag arrives as `{}` | evaluates, **allows** |
| `Attribute::known("finance")` | the directory answered `finance` | `{"department":"finance"}` | refuses `unevaluable_attribute` at `/evaluation/subject/properties` |
| `Attribute::unknown("…timed out")` | the directory did not answer | never sent | the SDK refuses with `Unresolved`, before any I/O |

The wire shape of an absent member is pinned by `tests/authzen_surface_test.rs`, not here: the server tolerates both `{}` and no bag at all, so there is nothing for a live assertion to see. What IS observable live is the pair - absent allows, known is refused by name.

Collapse absent and unknown into one `None` and rows 1 and 3 become the same call. Whichever way that single branch is written, one of those two rows is wrong: either an unresolvable fact is silently dropped from an authorization decision, or an ordinary "there is no value" becomes an error the caller cannot clear.

## Assertion floor

`EXPECTED_ASSERTIONS` must equal the number that ran. A suite whose checks stop executing prints no failures and exits 0, which is indistinguishable from success. The one assertion that can legitimately not run - the auth failure, which needs a deployment that actually refuses an unregistered caller - lowers the floor by exactly one and says so, rather than being silently skipped.

## Run

Community mode needs no license - any client id is its own tenant:

```bash
AXONFLOW_AGENT_URL=http://localhost:8080 ./test.sh
```

The auth assertion needs a second agent in `community-saas` (or enterprise) mode, because plain community mode treats any client id as its own tenant and never answers 401:

```bash
AXONFLOW_AGENT_URL=http://localhost:8080 \
AXONFLOW_SAAS_URL=http://localhost:8090 \
  ./test.sh
```

`REQUIRE_STACK=1` turns a missing agent into a failure rather than a skip, which is how a production-posture runner invokes it.

## Platform support

`POST /api/v1/access/evaluation` merged to `axonflow-enterprise` main in #3611 (`afff5d1a0`). Any stack built from that commit or later has it. Against an older agent the route does not exist and every assertion here fails - deliberately: a surface the deployment does not serve is not a surface this SDK can claim.

## Companion coverage

- `tests/authzen_surface_test.rs` - the response cases a live server will not produce on demand.
- `tests/authzen_generated_types_are_current.rs` - the committed wire types are what the vendored contract artifact generates.
- `examples/authzen/main.rs` - the same surface as a readable walkthrough: nine steps, four of them refusals, and one showing what a PEP still owes on an ALLOW.
- Platform-side runtime proof: `axonflow-enterprise/runtime-e2e/3603_authzen_evaluation/test.sh`.
