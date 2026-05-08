#!/usr/bin/env bash
# Runtime proof — Rust SDK v0.2 sandbox-mode telemetry fires with stream=sandbox.
#
# Builds a tiny cargo binary that uses the LOCAL SDK (via [path = "../.."]
# in a temporary Cargo.toml) in sandbox mode against an unreachable agent
# endpoint. The SDK fires its anonymous telemetry ping during
# AxonFlowClient::new. We then query the deployed checkpoint Lambda's
# CloudWatch logs for the audit line that should record stream=sandbox
# in DynamoDB.
#
# Pre-v0.2 the Rust heartbeat targeted `{endpoint}/api/telemetry/heartbeat`
# (local agent only); the central checkpoint Lambda never saw Rust pings.
# v0.2 routes through https://checkpoint.getaxonflow.com/v1/ping like the
# other 4 SDKs.
#
# Stack-state assumptions:
#   - axonflow-enterprise PR #2005 is deployed (server-side stream allowlist
#     accepts and persists "sandbox").
#   - "rust" is in the server-side ValidSDKs map (companion server-side
#     change to add this — without it, every Rust ping returns HTTP 400
#     "invalid sdk value" and this test fails at step 1).
#   - AWS credentials with read access on /aws/lambda/prod-axonflow-checkpoint.
#
# Usage:
#   AWS_REGION=us-east-1 ./test.sh

set -uo pipefail

REGION=${AWS_REGION:-us-east-1}
LOG_GROUP=${LOG_GROUP:-/aws/lambda/prod-axonflow-checkpoint}
RUN_TAG=$(date -u +%s)
SDK_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"

red()   { printf '\033[31m%s\033[0m\n' "$*"; }
green() { printf '\033[32m%s\033[0m\n' "$*"; }

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

cat > "$WORK/Cargo.toml" <<EOF
[package]
name = "sandbox-rt-${RUN_TAG}"
version = "0.0.0"
edition = "2021"

[dependencies]
axonflow-sdk-rust = { path = "${SDK_ROOT}" }
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
EOF

mkdir -p "$WORK/src"
cat > "$WORK/src/main.rs" <<'EOF'
use axonflow_sdk_rust::{AxonFlowClient, AxonFlowConfig};
use std::time::Duration;

#[tokio::main]
async fn main() {
    std::env::remove_var("AXONFLOW_TELEMETRY");
    println!("Constructing sandbox client (unreachable agent)...");
    let cfg = AxonFlowConfig::sandbox("rt-test", "rt-test");
    let _client = AxonFlowClient::new(cfg).expect("client construct");
    println!("AxonFlowClient::new returned. Sleeping 3s for inflight HTTP...");
    tokio::time::sleep(Duration::from_secs(3)).await;
    println!("Done.");
}
EOF

T0_MS=$(($(date -u +%s)*1000))
echo "Run tag: $RUN_TAG"
echo "T0 (ms): $T0_MS"
echo

(
  cd "$WORK"
  cargo build 2>&1 | tail -5
  cargo run 2>&1 | tail -10
)

echo
echo "Waiting 10s for CloudWatch log delivery..."
sleep 10

echo "Querying CloudWatch logs since T0 for sdk=rust event_stored entries..."
HITS=$(aws --region "$REGION" logs filter-log-events \
  --log-group-name "$LOG_GROUP" \
  --start-time "$T0_MS" \
  --filter-pattern '"event_stored" "sdk=rust"' \
  --query 'events[*].message' \
  --output text 2>&1)

if [ -z "$HITS" ]; then
  red "FAIL: no event_stored sdk=rust row landed since T0"
  red "  Possible causes:"
  red "  1. PR #2005 not yet deployed (server hardcodes stream=heartbeat)"
  red "  2. server-side ValidSDKs map missing \"rust\" (every ping → 400)"
  red "  3. tokio runtime issue / network drop"
  exit 1
fi

echo "Audit rows found:"
echo "$HITS"
echo

if echo "$HITS" | grep -q 'stream=sandbox'; then
  green "PASS: Rust SDK v0.2 sandbox-mode ping landed with stream=sandbox"
else
  red "FAIL: audit row did not include stream=sandbox"
  red "  Server is likely hardcoding stream=heartbeat — verify PR #2005 deployed."
  exit 1
fi
