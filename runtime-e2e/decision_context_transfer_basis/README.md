# decision_context_transfer_basis (v0.6.0)

Real-stack proof for the v0.6.0 SDK surface (platform epic #2508):

- **`DecisionSummary.context` / `DecisionExplanation.context`
  (+ `context_truncated`)** — the sanitized request context a PEP attaches to a
  Decision Mode call is surfaced back through `list_decisions` and
  `explain_decision`.
- **`AuditLogEntry.transfer_basis = "pasal_56b_dpa"`** — the Pasal 56(b) explicit
  DPA tag (named by `transfer_basis::PASAL_56B_DPA`) round-trips verbatim.

The `helper/` crate acts as the PEP (raw `POST /api/v1/decide` — that endpoint is
not SDK-wrapped per ADR-056), then reads the decision back through the SDK against
a real running agent.

## Run

```
export AXONFLOW_AGENT_URL=http://localhost:8080
export AXONFLOW_TENANT_ID=buku-e-rust-e2e
export AXONFLOW_TENANT_SECRET=buku-e-secret
./test.sh
```

Exits non-zero if the SDK does not surface the new fields.
