#!/usr/bin/env bash
# Runtime proof: the Rust SDK's PEP capability declaration reaches the platform
# on the planes that read it, and only there, against a LIVE agent. NO mocks.
#
# The helper crate drives the SDK's own calls and reads the agent's own counter,
# axonflow_pep_handshake_total{outcome, plane} on /prometheus, around each one.
#
# Usage:
#   test.sh [planes]      the declaration on each plane, the per-call form,
#                         no declaration, /api/request, a refused declaration,
#                         and the no-override control. Needs NO detection
#                         override recorded for the organization.
#   test.sh refusal       after a pii=redact detection override is recorded
#                         for the organization: the undeclared caller and a
#                         field_mask@1-only caller are refused, the
#                         field_redact@1 caller is allowed and fulfils.
#   test.sh probe-present exit 0 once the override is in effect for the agent.
#   test.sh probe-absent  exit 0 once no override is in effect for the agent.
#
# Environment:
#   AXONFLOW_AGENT_URL      defaults to http://localhost:8080
#   AXONFLOW_CLIENT_ID      defaults to runtime-e2e
#   AXONFLOW_CLIENT_SECRET  defaults to empty (community mode needs none)
#
# Exit codes: 0 all proofs passed; 1 a proof failed; 2 the agent is not reachable.

set -uo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
AXONFLOW_AGENT_URL="${AXONFLOW_AGENT_URL:-http://localhost:8080}"

if ! timeout 60 bash -c "until curl -sf ${AXONFLOW_AGENT_URL}/health > /dev/null; do sleep 2; done"; then
  echo "FAIL: agent at ${AXONFLOW_AGENT_URL} did not become healthy within 60s"
  exit 2
fi

export AXONFLOW_AGENT_URL
export AXONFLOW_CLIENT_ID="${AXONFLOW_CLIENT_ID:-runtime-e2e}"
export AXONFLOW_CLIENT_SECRET="${AXONFLOW_CLIENT_SECRET:-}"
# The proof must not fire a telemetry ping at the production checkpoint.
export AXONFLOW_TELEMETRY=off

# The helper's own unit tests (the reason-code predicate) run first: CI does
# not build this crate, so this is where they run.
cargo test --quiet --manifest-path "${HERE}/helper/Cargo.toml" || { echo "FAIL: the helper's unit tests failed"; exit 1; }

# Built first and outside the timeout, so a cold build does not count against
# the run; the run is bounded so a hung agent call cannot block a runner.
cargo build --quiet --manifest-path "${HERE}/helper/Cargo.toml" || { echo "FAIL: the helper did not build"; exit 1; }
timeout 180 cargo run --quiet --manifest-path "${HERE}/helper/Cargo.toml" -- "${1:-planes}"
rc=$?
case "$rc" in
  0|1) exit "$rc" ;;
  124) echo "FAIL: the helper did not finish within 180s"; exit 1 ;;
  *) echo "FAIL: the helper exited $rc"; exit 1 ;;
esac
