#!/usr/bin/env bash
# Real Hosted staging Agent proof; no local response fixture.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
export AXONFLOW_TELEMETRY=off
cargo build --quiet --manifest-path "$here/helper/Cargo.toml"
cargo run --quiet --manifest-path "$here/helper/Cargo.toml"
