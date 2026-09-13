#!/usr/bin/env bash
# Runtime proof: the Rust SDK reads what decided a governed request, as a
# v11.0.0 platform reports it, against a LIVE agent. NO mocks.
#
# Drives the SDK's own calls through the helper crate and asserts on what the
# platform answered:
#
#   1. decide() carries engine=anchored, a policy_bundle digest and a
#      subject_type; when policy_identities is present it names
#      evaluated_policies one for one, in order.
#   2. MCP check-output carries the same provenance.
#   3. proxy_llm_call() with the statement the platform's own detection corpus
#      uses for sys_dangerous_injection_override (a control that blocks on
#      /api/request) is refused by the anchored engine, and the 403's body
#      carries its provenance.
#   4. query_connector() with the same statement is dispatched through
#      /api/request, and the ConnectorResponse the SDK builds from that answer
#      by hand carries the same provenance.
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

cargo run --quiet --manifest-path "${HERE}/helper/Cargo.toml"
