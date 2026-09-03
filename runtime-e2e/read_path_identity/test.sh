#!/usr/bin/env bash
# Runtime proof — the Rust SDK's read-path per-user identity (#2922) against a
# LIVE enterprise agent + orchestrator. NO mocks.
#
# The defect this pins: every SDK carried `user_token` as a write-path body
# field only, so explain_decision and list_decisions asked the platform
# anonymously. On an enterprise stack that is not "a caller who sees
# everything" — it is a caller the platform cannot scope, so explain answered
# not-found for ids that plainly existed and list answered a confident empty
# page.
#
# Usage:
#   set -a; source /tmp/axonflow-e2e-env.sh; set +a
#   ./runtime-e2e/read_path_identity/test.sh
#
# Exit codes:
#   0 — every step passed
#   1 — a step failed
set -uo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
: "${AXONFLOW_AGENT_URL:=${AXONFLOW_ENDPOINT:-http://localhost:8080}}"
: "${AXONFLOW_CLIENT_ID:?AXONFLOW_CLIENT_ID must be set (source /tmp/axonflow-e2e-env.sh)}"
: "${AXONFLOW_CLIENT_SECRET:?AXONFLOW_CLIENT_SECRET must be set}"
: "${AXONFLOW_JWT_SECRET:=${JWT_SECRET:?JWT_SECRET (or AXONFLOW_JWT_SECRET) must be set}}"
: "${AXONFLOW_ORCH_CONTAINER:=axonflow-orchestrator}"

# The 7-day telemetry stamp is PARKED for this run and restored on exit — not
# deleted. It lives in the developer's real cache dir, and deleting it would
# make their next unrelated SDK run fire a genuine ping at the PRODUCTION
# checkpoint: a test reaching outside its own sandbox to change the machine's
# state. Without the park, the collector stays empty on every run after the
# first and the step FAILS loudly rather than passing on an unasserted absence.
case "$(uname -s)" in
  Darwin) STAMP="${HOME}/Library/Caches/axonflow/rust-telemetry-last-sent" ;;
  *)      STAMP="${XDG_CACHE_HOME:-${HOME}/.cache}/axonflow/rust-telemetry-last-sent" ;;
esac
if [ -f "$STAMP" ]; then
  mv "$STAMP" "${STAMP}.s3-parked"
  trap 'mv -f "${STAMP}.s3-parked" "$STAMP" 2>/dev/null || true' EXIT
fi

export AXONFLOW_AGENT_URL AXONFLOW_JWT_SECRET AXONFLOW_ORCH_CONTAINER
export AXONFLOW_TELEMETRY=on

cargo run --quiet --manifest-path "${HERE}/helper/Cargo.toml"
