#!/usr/bin/env bash
# Runtime proof — Rust SDK Anthropic interceptor against live community stack.
#
# Brings up getaxonflow/axonflow community docker-compose, runs the
# anthropic_interceptor example against it, and verifies the binary exits
# cleanly with one of the two expected outcomes (allow or block). See
# README.md for what this does and does not assert.
#
# Usage:
#   ./test.sh
#
# Exit codes:
#   0 — example exited 0 with a recognized outcome line
#   1 — example failed to build, panicked, or printed an unexpected line
#   2 — community stack failed to come up

set -uo pipefail

red()   { printf '\033[31m%s\033[0m\n' "$*"; }
green() { printf '\033[32m%s\033[0m\n' "$*"; }
blue()  { printf '\033[34m%s\033[0m\n' "$*"; }

SDK_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
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

blue ">>> Waiting for orchestrator (8081) /health"
if ! timeout 60 bash -c 'until curl -sf http://localhost:8081/health > /dev/null; do sleep 2; done'; then
  red "FAIL: orchestrator did not become healthy within 60s"
  (cd "$STACK_DIR" && docker compose logs --tail=200) || true
  exit 2
fi

blue ">>> Running anthropic_interceptor example"
cd "$SDK_ROOT"

OUTPUT=$(AXONFLOW_AGENT_URL=http://localhost:8080 \
         AXONFLOW_CLIENT_ID=demo-client \
         AXONFLOW_CLIENT_SECRET=demo-secret \
         timeout 60 cargo run --example anthropic_interceptor 2>&1) || {
  red "FAIL: example did not exit 0"
  echo "$OUTPUT"
  exit 1
}

echo "$OUTPUT"
echo

if echo "$OUTPUT" | grep -qE '^(✓ Request succeeded|❌ Request blocked or failed)'; then
  green "PASS: anthropic_interceptor reached governance + ran to completion"
  exit 0
else
  red "FAIL: example output did not contain expected outcome line"
  exit 1
fi
