#!/usr/bin/env bash
# Runtime proof — Rust SDK Decision Mode PEP (decide -> fulfill -> forward)
# against a LIVE enterprise agent. NO mocks. (epic #2563, tracking #2571)
#
# Mirrors the Python SDK's runtime-e2e/decide_fulfill_obligation/ runner.
# Drives the real SDK code path against the agent at AXONFLOW_ENDPOINT
# (default http://localhost:8080) using the enterprise credentials sourced
# from /tmp/axonflow-e2e-env.sh:
#
#   AXONFLOW_CLIENT_ID      — org id           (HTTP Basic username)
#   AXONFLOW_CLIENT_SECRET  — Ed25519 license  (HTTP Basic password)
#   AXONFLOW_TENANT_ID      — tenant scope
#   AXONFLOW_USER_TOKEN     — optional validated-user token
#
# Proves:
#   1. decide() -> allow + request-phase redact_pii obligation.
#   2. fulfill_request() -> ENGINE-masked content where neither
#      john.doe@example.com nor 4111111111111111 survives, content != original.
#   3. decide_and_fulfill() -> same masked content in one call.
#   4. demo creds (demo-org / demo-license-not-real) -> 401.
#
# Usage:
#   source /tmp/axonflow-e2e-env.sh && ./test.sh
#
# Exit codes:
#   0 — all four proofs passed
#   1 — a proof failed
#   2 — agent not reachable / creds not set

set -uo pipefail

red()   { printf '\033[31m%s\033[0m\n' "$*"; }
green() { printf '\033[32m%s\033[0m\n' "$*"; }
blue()  { printf '\033[34m%s\033[0m\n' "$*"; }

SDK_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
HELPER_DIR="${SDK_ROOT}/runtime-e2e/decide_fulfill_obligation/helper"
ENDPOINT="${AXONFLOW_ENDPOINT:-http://localhost:8080}"

if [ -z "${AXONFLOW_CLIENT_ID:-}" ] || [ -z "${AXONFLOW_CLIENT_SECRET:-}" ]; then
  red "FAIL: AXONFLOW_CLIENT_ID / AXONFLOW_CLIENT_SECRET not set — source /tmp/axonflow-e2e-env.sh first"
  exit 2
fi

blue ">>> Waiting for agent (${ENDPOINT}) /health"
if ! timeout 60 bash -c "until curl -sf ${ENDPOINT}/health > /dev/null; do sleep 2; done"; then
  red "FAIL: agent at ${ENDPOINT} did not become healthy within 60s"
  exit 2
fi

SDK_VERSION=$(grep -m1 '^version = ' "${SDK_ROOT}/Cargo.toml" | sed -E 's/version = "([^"]+)"/\1/')
blue ">>> SDK version: ${SDK_VERSION}"
blue ">>> Building + running helper (exercises the real SDK PEP code path)"

OUTPUT=$(
  AXONFLOW_ENDPOINT="${ENDPOINT}" \
  AXONFLOW_CLIENT_ID="${AXONFLOW_CLIENT_ID}" \
  AXONFLOW_CLIENT_SECRET="${AXONFLOW_CLIENT_SECRET}" \
  AXONFLOW_TENANT_ID="${AXONFLOW_TENANT_ID:-}" \
  AXONFLOW_USER_TOKEN="${AXONFLOW_USER_TOKEN:-}" \
  timeout 180 cargo run --manifest-path "${HELPER_DIR}/Cargo.toml" --release --quiet 2>&1
)
RC=$?

echo "$OUTPUT"
echo

if [ $RC -ne 0 ]; then
  red "FAIL: helper exited with status $RC"
  exit 1
fi

if echo "$OUTPUT" | grep -q '^RESULT: PASS'; then
  green "PASS: decide -> fulfill -> masked; demo creds refused (sdk_version=${SDK_VERSION})"
  exit 0
else
  red "FAIL: helper did not print RESULT: PASS"
  exit 1
fi
