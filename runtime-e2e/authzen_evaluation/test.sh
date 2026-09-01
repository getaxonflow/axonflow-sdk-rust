#!/usr/bin/env bash
# Runtime proof — the AuthZEN-native authorization surface (ADR-065, enterprise
# #3603 / #3616), driven through the Rust SDK's real public API against a live
# agent.
#
# Builds + runs the helper crate in helper/, which constructs a real
# AxonFlowClient exactly as a consumer would and issues real HTTP requests. No
# mocks, no stubbed transport.
#
# Usage:
#   AXONFLOW_AGENT_URL=http://localhost:8080 ./test.sh
#
# Optional:
#   AXONFLOW_SAAS_URL      a community-saas / enterprise agent, for the one
#                          assertion that needs a deployment which actually
#                          refuses an unregistered caller. Plain community mode
#                          treats any client id as its own tenant and answers
#                          200, so without this the auth assertion is SKIPPED
#                          and the floor drops by one.
#   AXONFLOW_CLIENT_ID     defaults to s2-authzen-rust-e2e
#   AXONFLOW_CLIENT_SECRET defaults to empty (community mode needs none)
#   REQUIRE_STACK=1        turn a missing agent into a failure rather than a
#                          skip. This is how a production-posture runner invokes
#                          it: in CI a missing stack is a broken run, not a pass.
#
# Exit codes: 0 — all assertions passed; 1 — an assertion failed; 2 — agent
# unreachable (and REQUIRE_STACK is not 1).

set -uo pipefail

red()   { printf '\033[31m%s\033[0m\n' "$*"; }
green() { printf '\033[32m%s\033[0m\n' "$*"; }
blue()  { printf '\033[34m%s\033[0m\n' "$*"; }

SDK_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
HELPER_DIR="${SDK_ROOT}/runtime-e2e/authzen_evaluation/helper"
AGENT_URL="${AXONFLOW_AGENT_URL:-http://localhost:8080}"
REQUIRE_STACK="${REQUIRE_STACK:-0}"

blue ">>> Waiting for agent ${AGENT_URL}/health"
if ! timeout 60 bash -c "until curl -sf ${AGENT_URL}/health > /dev/null; do sleep 2; done"; then
  if [ "$REQUIRE_STACK" = "1" ]; then
    red "FAIL: REQUIRE_STACK=1 but no agent answered at ${AGENT_URL}/health"
    exit 1
  fi
  red "SKIP: agent at ${AGENT_URL} did not become healthy within 60s (set REQUIRE_STACK=1 to fail)"
  exit 2
fi

SDK_VERSION=$(grep -m1 '^version = ' "${SDK_ROOT}/Cargo.toml" | sed -E 's/version = "([^"]+)"/\1/')
blue ">>> SDK version: ${SDK_VERSION}"

blue ">>> Building + running helper (this exercises the SDK code path)"
# Captured rather than piped: `producer | grep -q` under `set -o pipefail`
# reports failure BECAUSE grep matched and closed the pipe early, which turns a
# passing run into a red one at random.
OUTPUT=$(
  AXONFLOW_AGENT_URL="${AGENT_URL}" \
  AXONFLOW_SAAS_URL="${AXONFLOW_SAAS_URL:-}" \
  AXONFLOW_CLIENT_ID="${AXONFLOW_CLIENT_ID:-s2-authzen-rust-e2e}" \
  AXONFLOW_CLIENT_SECRET="${AXONFLOW_CLIENT_SECRET:-}" \
  timeout 300 cargo run --manifest-path "${HELPER_DIR}/Cargo.toml" --release --quiet 2>&1
)
RC=$?

echo "$OUTPUT"
echo

if [ $RC -ne 0 ]; then
  red "FAIL: helper exited with status $RC"
  exit 1
fi

if echo "$OUTPUT" | grep -q '^ALL PASS:'; then
  green "PASS: the AuthZEN surface answers, agrees with /api/v1/decide, and refuses by name (sdk_version=${SDK_VERSION})"
  exit 0
fi

red "FAIL: helper did not print an ALL PASS: line"
exit 1
