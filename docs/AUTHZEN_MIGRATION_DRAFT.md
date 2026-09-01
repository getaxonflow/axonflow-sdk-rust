# Moving to the AuthZEN surface - DRAFT, not in effect

> **Status: DRAFT. Nothing here is deprecated today.** The existing decision surface is wire-stable through all of v11. It is written now, and shipped now, so that customers have a migration target *during* the shadow window rather than being handed one at the moment the default flips. The DRAFT marker governs when this text moves into the README and the public docs site; it does not make the timeline below provisional. The v11.0.0 deprecation and the v12.0.0 removal are set by the ADR-065 release and compatibility plan, not by this file.

## Why this exists before there is anything to migrate

Deprecating the legacy surface today would tell people to move off something that is not going anywhere for two releases. The reason to publish the plan anyway is the opposite of urgency: an integration being written *this month* should be written against the surface that will not need rewriting, and that is a decision the author can only make if they can see what is coming.

The short version: **write new integrations against `evaluate` / `evaluate_all`. Leave working integrations alone.**

## Timeline

| release | the AuthZEN surface | the legacy decision surface |
|---|---|---|
| v10.3.0 (this, crate 0.9.0) | New. Available. Recommended for new integrations. An **adapter** over the same evaluation `POST /api/v1/decide` runs. | Fully supported. Not deprecated. No warnings. Unchanged, byte for byte. |
| v10.3.x | unchanged | unchanged |
| **v11.0.0** | the engine behind it becomes the ADR-065 Policy Decision Point. **No wire change.** | **Deprecated.** Still works; wire-stable. Doc + release-note notice. |
| v12.0.0 | the only decision surface | **Removed.** |

The releases named here are AxonFlow platform releases. This crate carries its own pre-1.0 version line; the platform v10.3.0 train ships as crate 0.9.0.

The legacy surface is **wire-stable through all of v11**. A v10.x integration keeps working on v11 without edits; deprecation is a signal to plan, not a breakage.

The v11 engine swap is the reason to prefer this surface now. An integration written against `/api/v1/decide` migrates twice - once to a new shape, once when the engine changes. One written against `evaluate` migrates zero times: the same call, the same types, a different evaluator underneath.

## Field-by-field

| legacy `DecideRequest` | AuthZEN | notes |
|---|---|---|
| `stage: "llm"` | `action.name = "llm.completion"`, `resource.type = "llm"` | the action and the resource must describe **one** operation; a mismatch is refused |
| `stage: "tool"` | `action.name = "tool.call"`, `resource.type = "tool"` | |
| `stage: "agent"` | `action.name = "agent.invoke"`, `resource.type = "agent"` | |
| `query` | `context.args.query` | `AuthZenRequest::with_query` |
| `target.server` / `target.tool` | `resource.id = "server/tool"` | both halves are read by policy and audit |
| `target.provider` / `target.model` | **not accepted** | nothing reads them for an `llm` target, so `resource.id` must be exactly `"llm"`. Accepting a model name would report that it was considered when it was not. A future release that teaches the evaluator to read a model widens this. |
| `caller_identity.gateway_id` | `subject.id`, with `subject.type = "gateway"` | |
| an end-user subject | **not yet** | the identity plane that can resolve and bind one activates at v11. Until then a subject naming a user would have to be trusted from caller-supplied JSON, which is an impersonation surface |
| `context` (allowlisted headers) | `context.correlation.<key>` | `AuthZenRequest::with_correlation`; the deployment's allowlist and its key cap still apply, and a key it does not record is refused rather than dropped |
| `verdict: "allow" / "deny" / "needs_approval"` | `decision` + `context.state` (`ALLOW` / `DENY` / `CHALLENGE` / `ERROR`) | `AuthZenDecision::allowed()` requires both |
| `obligations[]` | `context.obligations[]`, typed | the `fulfillment` block rides in `params`; the discharge path is unchanged |

## The one behavioural difference to plan for

**The new surface refuses what the old one ignored.**

`POST /api/v1/decide` accepts a body with members it does not read. The AuthZEN surface does not: an unrecognised context member, a property bag, an argument beside `query`, a provider or model on an `llm` resource - each is a `422` naming the exact member, rather than a decision computed without it.

So code that was quietly sending fields nothing read will start getting refusals that name them. That is the intended outcome and the reason the surface exists: a decision that silently ignored an attribute tells you the attribute was weighed when it was not, and every audit of that decision inherits the claim.

Practically: port one call, run it, and read the pointers. The refusal names the member, so the diff is mechanical.

## Two things the SDK does that the wire cannot

1. **`Attribute` is three-valued.** The wire has no way to say "I could not resolve this", so the SDK refuses to send a request carrying an unresolved attribute - locally, before the round trip, with the retryable code. Porting an integration that resolves attributes from an identity provider or a trace propagator means deciding, per attribute, whether a failure to resolve is `absent` (there is no value) or `unknown` (nobody knows). That decision has to be made somewhere; the type is where it is made visible.

2. **A refusal is a different type from a denial.** `Err(AuthZenEvaluationError::Refused(..))` versus `Ok(decision)` with `allowed() == false`. Legacy callers that branched on a boolean will need one more arm. Callers that treated a transport failure as a denial were already wrong, and this makes it a compile error rather than a production incident.

## What has not been decided

- Whether an end-user subject becomes available at v11 or later, and what the identity plane requires of a caller to bind one.
- Whether `resource.id` for an `llm` target widens to name a provider and model, which depends on the evaluator learning to read them.

None of these should be planned around until this file loses its DRAFT marker.
