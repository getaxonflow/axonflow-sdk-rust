#!/usr/bin/env bash
# Runtime proof: the Rust SDK's typed policy authoring client against a LIVE
# agent and orchestrator. NO mocks.
#
# The helper crate drives the six operations of client.typed_policies() in
# order and asserts on what the platform answered: nothing active on a fresh
# stack, the edition and the shipped corpus, validate, publish (version 1),
# activate, the document in force as its signed bytes, and the typed 409 and
# 422 refusals.
#
# Run it on a FRESH stack: the first assertion needs nothing active on the
# organization, and each run activates a document.
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

# Built first and outside the timeout, so a cold build does not count against
# the run; the run is bounded so a hung call cannot block a runner.
cargo build --quiet --manifest-path "${HERE}/helper/Cargo.toml" || { echo "FAIL: the helper did not build"; exit 1; }
timeout 180 cargo run --quiet --manifest-path "${HERE}/helper/Cargo.toml"
rc=$?
case "$rc" in
  0|1) exit "$rc" ;;
  124) echo "FAIL: the helper did not finish within 180s"; exit 1 ;;
  *) echo "FAIL: the helper exited $rc"; exit 1 ;;
esac
