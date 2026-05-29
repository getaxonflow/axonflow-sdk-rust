#!/usr/bin/env bash
# Runtime proof — Rust SDK v0.6.0 surfaces the Decision Mode request context
# (platform #2509, epic #2508) and the pasal_56b_dpa transfer basis.
#
# Builds + runs the helper crate in helper/, which exercises the SDK code path
# against a live agent (AXONFLOW_AGENT_URL, default http://localhost:8080):
#
#   1. acts as the PEP via a raw POST /api/v1/decide (that endpoint is not
#      SDK-wrapped per ADR-056), forwarding request context in the body;
#   2. reads the decision back through list_decisions + explain_decision and
#      asserts DecisionSummary.context / DecisionExplanation.context are
#      populated with the forwarded keys;
#   3. round-trips an AuditLogEntry carrying transfer_basis = "pasal_56b_dpa".
#
# Usage:
#   AXONFLOW_AGENT_URL=http://localhost:8080 ./test.sh
#
# Exit codes: 0 — all assertions passed; 1 — helper failed; 2 — agent unreachable.

set -uo pipefail

red()   { printf '\033[31m%s\033[0m\n' "$*"; }
green() { printf '\033[32m%s\033[0m\n' "$*"; }
blue()  { printf '\033[34m%s\033[0m\n' "$*"; }

SDK_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
HELPER_DIR="${SDK_ROOT}/runtime-e2e/decision_context_transfer_basis/helper"
AGENT_URL="${AXONFLOW_AGENT_URL:-http://localhost:8080}"

blue ">>> Waiting for agent ${AGENT_URL}/health"
if ! timeout 60 bash -c "until curl -sf ${AGENT_URL}/health > /dev/null; do sleep 2; done"; then
  red "FAIL: agent at ${AGENT_URL} did not become healthy within 60s"
  exit 2
fi

SDK_VERSION=$(grep -m1 '^version = ' "${SDK_ROOT}/Cargo.toml" | sed -E 's/version = "([^"]+)"/\1/')
blue ">>> SDK version: ${SDK_VERSION}"

blue ">>> Building + running helper (this exercises the SDK code path)"
OUTPUT=$(
  AXONFLOW_AGENT_URL="${AGENT_URL}" \
  AXONFLOW_TENANT_ID="${AXONFLOW_TENANT_ID:-buku-e-rust-e2e}" \
  AXONFLOW_TENANT_SECRET="${AXONFLOW_TENANT_SECRET:-buku-e-secret}" \
  timeout 180 cargo run --manifest-path "${HELPER_DIR}/Cargo.toml" --release --quiet 2>&1
)
RC=$?

echo "$OUTPUT"
echo

if [ $RC -ne 0 ]; then
  red "FAIL: helper exited with status $RC"
  exit 1
fi

if echo "$OUTPUT" | grep -q '^ALL PASS:'; then
  green "PASS: SDK surfaces DecisionSummary.context + DecisionExplanation.context + pasal_56b_dpa (sdk_version=${SDK_VERSION})"
  exit 0
else
  red "FAIL: helper did not print ALL PASS: line"
  exit 1
fi
