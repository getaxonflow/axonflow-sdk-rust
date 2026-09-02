#!/usr/bin/env bash
#
# Runs the /health-relay matrix, one scenario per process.
#
# One process per scenario is not tidiness: the SDK's 1-hour in-process guard
# is process-wide by design, so a second heartbeat in the same process is
# exactly what it suppresses.
#
# Add a live agent to also prove the relay against a real platform:
#   AXONFLOW_LIVE_HEALTH_URL=http://localhost:8080 ./test.sh

set -euo pipefail
cd "$(dirname "$0")/helper"

SCENARIOS=(full pre_3660 starting http_error not_json hostile oversized_value)
if [ -n "${AXONFLOW_LIVE_HEALTH_URL:-}" ]; then
  SCENARIOS+=(live)
fi

cargo build --quiet

failed=0
for s in "${SCENARIOS[@]}"; do
  echo "───────────── scenario: $s"
  if SCENARIO="$s" cargo run --quiet; then
    echo "  ✓ $s"
  else
    echo "  ✗ $s"
    failed=$((failed + 1))
  fi
done

echo
if [ "$failed" -ne 0 ]; then
  echo "FAIL: $failed scenario(s) failed"
  exit 1
fi
echo "PASS: all ${#SCENARIOS[@]} scenarios"
