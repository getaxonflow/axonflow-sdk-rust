#!/usr/bin/env bash
# Runtime proof: examples/pep_handshake and examples/typed_policies, built from
# this tree, against a LIVE Community agent, in the order the README gives.
# NO mocks. It asserts what each run proves:
#
#   0. typed_policies without publishing: exits 0, and nothing is active. This
#      is the fresh-stack precondition: on a stack a previous run used, the leg
#      stops here with exit 2 rather than report the later runs as failures.
#   1. pep_handshake on a fresh stack: exits 0, the first decide is allowed,
#      and the declaration the platform would refuse fails in the client at
#      /pep_id, before anything is sent.
#   2. typed_policies with AXONFLOW_TYPED_POLICY_PUBLISH=1: exits 0, and the
#      document is published and activated.
#   3. typed_policies asked to publish a document the save-time checks reject:
#      exits 1, printing the platform's typed 422 document_refused.
#   4. pep_handshake again: printed as an OBSERVATION, not asserted. After run
#      2 activates a document with an organization-scope constraint, a decide
#      that does not supply the attribute the constraint conditions on is
#      denied fail-closed with reasons ["unknown_constraint"].
#
# Run it on a FRESH stack: it changes the organization's active policy.
#
# Environment:
#   AXONFLOW_AGENT_URL      defaults to http://localhost:8080
#   AXONFLOW_CLIENT_ID      defaults to runtime-e2e
#   AXONFLOW_CLIENT_SECRET  defaults to runtime-e2e-secret
#
# Exit codes: 0 all proofs passed; 1 a proof failed; 2 the agent is not reachable,
# or the stack is not fresh.

set -uo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT" || exit 1
AXONFLOW_AGENT_URL="${AXONFLOW_AGENT_URL:-http://localhost:8080}"

if ! timeout 60 bash -c "until curl -sf ${AXONFLOW_AGENT_URL}/health > /dev/null; do sleep 2; done"; then
  echo "FAIL: agent at ${AXONFLOW_AGENT_URL} did not become healthy within 60s"
  exit 2
fi

export AXONFLOW_AGENT_URL
export AXONFLOW_CLIENT_ID="${AXONFLOW_CLIENT_ID:-runtime-e2e}"
export AXONFLOW_CLIENT_SECRET="${AXONFLOW_CLIENT_SECRET:-runtime-e2e-secret}"
# The proof must not fire a telemetry ping at the production checkpoint.
export AXONFLOW_TELEMETRY=off

# Built first and outside the timeouts, so a cold build does not count against
# a run.
cargo build --quiet --example pep_handshake --example typed_policies || {
  echo "FAIL: the examples did not build"
  exit 1
}
TARGET="${CARGO_TARGET_DIR:-$ROOT/target}"
BIN="$TARGET/debug/examples"
OUT="$TARGET/v11-examples"
mkdir -p "$OUT"

FAILURES=0
check() {
  if [ "$1" = ok ]; then echo "PASS: $2"; else echo "FAIL: $2"; FAILURES=$((FAILURES + 1)); fi
}
# Runs one example with extra environment, prints its output, returns its exit.
run_example() {
  local example=$1 log=$2
  shift 2
  env "$@" timeout 120 "$BIN/$example" > "$log" 2>&1
  local rc=$?
  sed 's/^/  | /' "$log"
  return "$rc"
}

echo "=== 0. typed_policies without publishing: the fresh-stack precondition"
rc=0; run_example typed_policies "$OUT/0.log" \
  AXONFLOW_TYPED_POLICY_BODY="$ROOT/testdata/typed_policy_publish_body.json" || rc=$?
if [ "$rc" != 0 ] || ! grep -qx 'nothing is active' "$OUT/0.log"; then
  echo "FAIL: not a fresh stack, or typed_policies failed without publishing (exit $rc): run this leg on a fresh stack"
  exit 2
fi
echo "PASS: typed_policies exits 0 without publishing, and nothing is active"

echo "=== 1. pep_handshake on a fresh stack"
rc=0; run_example pep_handshake "$OUT/1.log" || rc=$?
check "$([ "$rc" = 0 ] && echo ok)" "pep_handshake exits 0 (exit $rc)"
# The verdict of the first decide, the one under the client's declaration.
first=$(awk '/^=== decide with the client.s declaration ===/ {f = 1; next}
  f && /^verdict=/ {sub(/ .*/, ""); print; exit}
  f && /^===/ {exit}' "$OUT/1.log")
check "$([ "$first" = verdict=allow ] && echo ok)" "the first decide is allowed on a fresh stack ($first)"
check "$(grep -q '^refused at /pep_id: ' "$OUT/1.log" && echo ok)" \
  "the declaration the platform would refuse fails in the client, at /pep_id, before anything is sent"

echo "=== 2. typed_policies: validate, publish and activate"
rc=0; run_example typed_policies "$OUT/2.log" \
  AXONFLOW_TYPED_POLICY_PUBLISH=1 AXONFLOW_TYPED_POLICY_BODY="$ROOT/testdata/typed_policy_publish_body.json" || rc=$?
check "$([ "$rc" = 0 ] && echo ok)" "typed_policies exits 0 (exit $rc)"
check "$(grep -qx 'activated' "$OUT/2.log" && echo ok)" "the document is published and activated"

echo "=== 3. typed_policies asked to publish a document the save-time checks reject"
# The vendored body with one action the registry does not contain: the same
# edit runtime-e2e/typed_policies/helper makes to it.
python3 - "$ROOT/testdata/typed_policy_publish_body.json" "$OUT/refused_body.json" <<'PY' || { echo "FAIL: could not write the refused body"; exit 1; }
import json, sys
body = json.load(open(sys.argv[1]))
body["document"]["metadata"]["document_id"] = "v11-examples-refused"
body["document"]["policy"]["policies"][0]["actions"]["actions"][0]["local"] = "tool.not_registered"
json.dump(body, open(sys.argv[2], "w"))
PY
rc=0; run_example typed_policies "$OUT/3.log" \
  AXONFLOW_TYPED_POLICY_PUBLISH=1 AXONFLOW_TYPED_POLICY_BODY="$OUT/refused_body.json" || rc=$?
check "$([ "$rc" = 1 ] && echo ok)" \
  "typed_policies exits 1 when the publish it asked for is refused (exit $rc)"
check "$(grep -q '^refused: HTTP 422 document_refused: ' "$OUT/3.log" && echo ok)" \
  "it prints the platform's typed 422 document_refused"

echo "=== 4. OBSERVATION, not asserted: pep_handshake after the activation"
run_example pep_handshake "$OUT/4.log" || true
echo "  observed: $(grep -m1 -oE '^verdict=.*' "$OUT/4.log" || echo 'no verdict printed')"

if [ "$FAILURES" -gt 0 ]; then
  echo
  echo "FAIL: v11_examples ($FAILURES assertion(s))"
  exit 1
fi
echo
echo "PASS: v11_examples"
