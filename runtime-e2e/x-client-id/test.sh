#!/usr/bin/env bash
# Runtime proof — Rust SDK v9 X-Client-ID + ADR-050 §4 X-Axonflow-Client
# headers against a live community-stack agent.
#
# Mirrors the Go/Python/TS/Java SDKs' runtime-e2e/x-client-id/ runners
# (PR getaxonflow/axonflow-enterprise#2230). Brings up the community
# docker-compose stack, then runs the in-process forwarding-proxy helper
# in `helper/` which:
#
#   1. Binds a tokio TcpListener on 127.0.0.1:0.
#   2. Constructs the SDK pointing at the listener.
#   3. Issues one proxy_llm_call.
#   4. Captures the outbound headers off the wire AND forwards the
#      request to the real agent.
#   5. Asserts X-Client-ID + X-Axonflow-Client + Authorization: Basic
#      are present; X-Tenant-ID is absent.
#
# Usage:
#   ./test.sh
#
# Exit codes:
#   0 — helper passed all 4 header assertions
#   1 — helper failed (missing/wrong header)
#   2 — community stack failed to come up

set -uo pipefail

red()   { printf '\033[31m%s\033[0m\n' "$*"; }
green() { printf '\033[32m%s\033[0m\n' "$*"; }
blue()  { printf '\033[34m%s\033[0m\n' "$*"; }

SDK_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
HELPER_DIR="${SDK_ROOT}/runtime-e2e/x-client-id/helper"
STACK_DIR="${SDK_ROOT}/../axonflow-community"

cleanup() {
  if [ -d "$STACK_DIR" ]; then
    blue ">>> Tearing down community stack"
    (cd "$STACK_DIR" && docker compose down --volumes --remove-orphans 2>&1 | tail -5) || true
  fi
}
trap cleanup EXIT

if [ ! -d "$STACK_DIR" ]; then
  blue ">>> Cloning community stack into $STACK_DIR"
  git clone --depth 1 https://github.com/getaxonflow/axonflow.git "$STACK_DIR" || {
    red "FAIL: could not clone community stack"
    exit 2
  }
else
  blue ">>> Refreshing community stack at $STACK_DIR"
  (cd "$STACK_DIR" && git fetch --depth 1 origin main && git reset --hard origin/main) || true
fi

blue ">>> Bringing up community stack"
(cd "$STACK_DIR" && docker compose up -d --wait --wait-timeout 180) || {
  red "FAIL: docker compose up did not converge"
  (cd "$STACK_DIR" && docker compose logs --tail=200) || true
  exit 2
}

blue ">>> Waiting for agent (8080) /health"
if ! timeout 60 bash -c 'until curl -sf http://localhost:8080/health > /dev/null; do sleep 2; done'; then
  red "FAIL: agent did not become healthy within 60s"
  (cd "$STACK_DIR" && docker compose logs --tail=200) || true
  exit 2
fi

AGENT_GIT_REF=$(cd "$STACK_DIR" && git rev-parse --short HEAD 2>/dev/null || echo "unknown")
blue ">>> Community stack agent git ref: ${AGENT_GIT_REF}"

# Pull SDK version from the workspace Cargo.toml — used by the helper to
# build the expected X-Axonflow-Client value.
SDK_VERSION=$(grep -m1 '^version = ' "${SDK_ROOT}/Cargo.toml" | sed -E 's/version = "([^"]+)"/\1/')
blue ">>> SDK version: ${SDK_VERSION}"

blue ">>> Building + running helper (this exercises the SDK code path)"
OUTPUT=$(
  AXONFLOW_AGENT_URL=http://localhost:8080 \
  AXONFLOW_TENANT_ID="${AXONFLOW_TENANT_ID:-demo-client}" \
  AXONFLOW_TENANT_SECRET="${AXONFLOW_TENANT_SECRET:-demo-secret}" \
  timeout 120 cargo run --manifest-path "${HELPER_DIR}/Cargo.toml" --release --quiet 2>&1
)
RC=$?

echo "$OUTPUT"
echo

if [ $RC -ne 0 ]; then
  red "FAIL: helper exited with status $RC"
  exit 1
fi

if echo "$OUTPUT" | grep -q '^PASS:'; then
  green "PASS: SDK emits X-Client-ID + X-Axonflow-Client + Basic auth; X-Tenant-ID absent"
  green "      agent_git_ref=${AGENT_GIT_REF} sdk_version=${SDK_VERSION}"
  exit 0
else
  red "FAIL: helper did not print PASS: line"
  exit 1
fi
