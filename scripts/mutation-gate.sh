#!/usr/bin/env bash
#
# Mutation gate for the telemetry heartbeat (axonflow-sdk-rust#88).
#
# A passing test suite proves the code does what the tests say TODAY. It does
# not prove the tests would notice if the code stopped doing it. This script
# plants one defect at a time and requires the named test to go RED. A mutant
# that SURVIVES is a test that was decorative.
#
# Every mutant is a defect somebody could plausibly introduce: relaxing the
# split deadline back to a per-leg timeout, letting an unlearned field reach
# the wire as a null or a guess, dropping the length cap, dropping the 1-hour
# guard or its failure backoff, dropping the non-2xx check, following
# redirects again, going back to a fabricated runtime_version, writing the
# platform's own deployment mode over the SDK's topology classification, or
# slipping a request past the dispatch funnel using a reqwest spelling the
# source guards did not think of.
#
# Usage:  ./scripts/mutation-gate.sh
# Exit 0 only when the unmutated tree passes AND every mutant is killed.
#
# The tree is restored on every exit path — including SIGINT and SIGTERM, so a
# timed-out or interrupted run never leaves a mutated source file behind.

set -euo pipefail

cd "$(dirname "$0")/.."

# Two files carry mutable behaviour for this feature: the heartbeat itself, and
# the dispatch funnel that decides whether the gate is ever consulted at all.
TARGETS=("src/heartbeat.rs" "src/client.rs")
# An explicit XXXXXX template rather than `mktemp -t <prefix>`: BSD mktemp
# (macOS) reads -t as a prefix, GNU mktemp (CI) reads it as a template and
# refuses one with "too few X's". This form is correct on both.
BACKUP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/axonflow-mutation-backup.XXXXXX")"
for f in "${TARGETS[@]}"; do
  cp "$f" "$BACKUP_DIR/$(basename "$f")"
done

restore_files() {
  for f in "${TARGETS[@]}"; do
    cp "$BACKUP_DIR/$(basename "$f")" "$f"
  done
}

restore() {
  restore_files
  rm -rf "$BACKUP_DIR"
}
trap restore EXIT INT TERM

# Telemetry is force-disabled in this crate's test binary by an internal
# cfg(test) switch, so this only documents intent for anything else the run
# might touch.
export AXONFLOW_TELEMETRY=off

pass=0
fail=0

# apply <file> <find> <replace>
# Fails loudly when the pattern is absent or ambiguous: a replacement that
# matched nothing would leave the tree pristine and report every mutant as
# "killed", which is the exact vacuous-pass this gate exists to prevent.
apply() {
  python3 - "$1" "$2" "$3" <<'PY'
import sys, pathlib
path, find, repl = sys.argv[1], sys.argv[2], sys.argv[3]
p = pathlib.Path(path)
src = p.read_text()
n = src.count(find)
if n != 1:
    sys.exit(f"mutation pattern matched {n} times, expected exactly 1 in {path}:\n{find}")
mutated = src.replace(find, repl)
if mutated == src:
    sys.exit("mutation produced no change")
p.write_text(mutated)
PY
}

# mutant_in <file> <name> <test> <find> <replace>
mutant_in() {
  local file="$1" name="$2" test_name="$3" find="$4" repl="$5"
  restore_files
  apply "$file" "$find" "$repl"

  printf '\n=== mutant: %s\n    expecting RED: %s\n' "$name" "$test_name"
  if cargo test --lib "$test_name" -- --exact >/dev/null 2>&1; then
    printf '    SURVIVED — the test does not detect this defect\n'
    fail=$((fail + 1))
  else
    printf '    killed\n'
    pass=$((pass + 1))
  fi
  restore_files
}

# mutant <name> <test> <find> <replace> - shorthand for the heartbeat file.
mutant() {
  mutant_in "src/heartbeat.rs" "$1" "$2" "$3" "$4"
}

# ---------------------------------------------------------------------------
# Survivor check: the unmutated tree must PASS every test used below.
# Without this, a test that is broken for an unrelated reason reports every
# mutant as killed and the whole gate becomes a rubber stamp.
# ---------------------------------------------------------------------------
printf '=== baseline: the unmutated tree must pass every target test\n'
if ! cargo test --lib heartbeat:: 2>&1 | tail -3; then
  echo "BASELINE FAILED — fix the suite before reading any mutant result" >&2
  exit 1
fi

# ---------------------------------------------------------------------------
# 1. The split deadline collapses back to a per-leg timeout.
#    The probe then consumes the whole budget and the POST is never sent.
# ---------------------------------------------------------------------------
mutant "flat per-leg timeout on the /health probe" \
  "heartbeat::tests::the_post_is_not_starved_when_health_consumes_its_whole_cap" \
  '        .timeout(budget)' \
  '        .timeout(HEARTBEAT_TIMEOUT)'

# ---------------------------------------------------------------------------
# 2. An unlearned field reaches the wire as JSON null instead of being omitted.
#    "absent" and "null" are different claims; the presence tests ask has(key)
#    precisely so this cannot slip through.
# ---------------------------------------------------------------------------
mutant "unlearned license_tier serialised as null" \
  "heartbeat::tests::ping_is_still_sent_and_omits_the_keys_on_every_health_failure" \
  '    #[serde(skip_serializing_if = "Option::is_none")]
    license_tier: Option<String>,' \
  '    license_tier: Option<String>,'

# ---------------------------------------------------------------------------
# 3. An unlearned field is defaulted to a guess. This is the dangerous one:
#    the wire shape stays valid and the value looks entirely plausible.
# ---------------------------------------------------------------------------
mutant "unlearned license_tier defaulted to a guess" \
  "heartbeat::tests::ping_is_still_sent_and_omits_the_keys_on_every_health_failure" \
  '            license_tier: probe.license_tier,' \
  '            license_tier: probe.license_tier.or(Some("Community".to_string())),'

# ---------------------------------------------------------------------------
# 4. The per-value length cap is dropped, so a hostile /health value reaches
#    the wire uncapped and can push the ping past the receiver's body limit.
# ---------------------------------------------------------------------------
mutant "relayed value length cap removed" \
  "heartbeat::tests::an_oversized_health_value_is_dropped_without_costing_the_others" \
  '    if raw.len() > MAX_RELAYED_VALUE_LEN {' \
  '    if false {'

# ---------------------------------------------------------------------------
# 5. The 1-hour in-process guard is dropped, so every request re-consults the
#    stamp and a failing checkpoint is re-pinged on every call.
# ---------------------------------------------------------------------------
mutant "1-hour in-process guard removed" \
  "heartbeat::tests::the_one_hour_guard_suppresses_a_second_pass" \
  '    if let Some(last) = inner.last_checked {
        if last.elapsed() < guard_interval_for(inner.consecutive_failures) {
            return None;
        }
    }' \
  '    let _ = inner.last_checked;'

# ---------------------------------------------------------------------------
# 6. The non-2xx guard is dropped, so an error page is mined for values.
# ---------------------------------------------------------------------------
mutant "non-2xx guard removed from the probe" \
  "heartbeat::tests::probe_returns_nothing_on_a_server_error" \
  '    if !resp.status().is_success() {' \
  '    if false {'

# ---------------------------------------------------------------------------
# 7. runtime_version goes back to a fabricated literal.
# ---------------------------------------------------------------------------
mutant "runtime_version reverted to a fabricated literal" \
  "heartbeat::tests::runtime_version_is_the_real_toolchain_and_never_the_old_literal" \
  '    normalize_rustc_version(option_env!("AXONFLOW_RUSTC_VERSION"))' \
  '    "rustc-stable".to_string()'

# ---------------------------------------------------------------------------
# 8. The platform's own deployment mode is written over the SDK's topology
#    classification. Both are called "deployment mode" and share a vocabulary,
#    so the mistake looks harmless; it would corrupt every existing
#    deployment-mode dashboard. Flagged by the platform lane (enterprise#3660).
# ---------------------------------------------------------------------------
mutant "platform deployment mode conflated with the SDK's own field" \
  "heartbeat::tests::ping_carries_every_relayed_field_when_health_answers" \
  '    #[serde(skip_serializing_if = "Option::is_none")]
    platform_deployment_mode: Option<String>,' \
  '    #[serde(rename = "deployment_mode", skip_serializing_if = "Option::is_none")]
    platform_deployment_mode: Option<String>,'

# ---------------------------------------------------------------------------
# 9. Redirects are followed again. On the probe this means "exactly one
#    /health fetch" is false and the relayed values can come from a host that
#    is not the configured endpoint; on the POST it is worse, because reqwest
#    re-issues a redirected POST as a bodyless GET, so a 302 yields a 200
#    carrying nothing and the 7-day stamp advances on a ping never sent.
# ---------------------------------------------------------------------------
mutant "redirects followed on the telemetry client" \
  "heartbeat::tests::a_redirecting_health_endpoint_is_refused_not_followed" \
  '        .redirect(reqwest::redirect::Policy::none())' \
  '        .redirect(reqwest::redirect::Policy::limited(10))'

mutant "a redirected checkpoint POST counts as delivery" \
  "heartbeat::tests::a_redirected_checkpoint_post_is_not_a_delivery" \
  '        .redirect(reqwest::redirect::Policy::none())' \
  '        .redirect(reqwest::redirect::Policy::limited(10))'

# ---------------------------------------------------------------------------
# 10. The failure backoff is removed, so a deployment that can never reach the
#     checkpoint service probes the customer's own platform every hour forever.
# ---------------------------------------------------------------------------
mutant "failure backoff removed" \
  "heartbeat::tests::the_guard_interval_widens_after_consecutive_failures" \
  '    let doublings = consecutive_failures.min(16);
    HEARTBEAT_GUARD_INTERVAL
        .saturating_mul(1u32 << doublings)
        .min(HEARTBEAT_INTERVAL)' \
  '    let _ = consecutive_failures;
    HEARTBEAT_GUARD_INTERVAL'

# ---------------------------------------------------------------------------
# 11 + 12. The two source-scanning guards, attacked the way an earlier version
#     of them was actually evaded in review: a request issued through
#     `Client::execute` rather than `.send()`, and a second transport built
#     through `ClientBuilder::new()` rather than `reqwest::Client::builder()`.
#     Both left the previous guards green.
# ---------------------------------------------------------------------------
mutant_in "src/client.rs" "an ungated request issued via Client::execute" \
  "heartbeat::tests::no_http_send_outside_the_dispatch_funnel" \
  '    pub(crate) async fn checked_get(&self, url: &str) -> Result<reqwest::Response, AxonFlowError> {
        let resp = self.dispatch(self.http_client.get(url)).await?;' \
  '    pub(crate) async fn checked_get(&self, url: &str) -> Result<reqwest::Response, AxonFlowError> {
        let built = self.http_client.get(url).build()?;
        let resp = self.http_client.execute(built).await?;'

mutant_in "src/client.rs" "a second transport built via ClientBuilder::new" \
  "heartbeat::tests::the_telemetry_path_builds_exactly_one_http_client" \
  '        let http_client = reqwest::Client::builder()' \
  '        let _extra = reqwest::ClientBuilder::new().build();
        let http_client = reqwest::Client::builder()'

printf '\n=== mutation gate: %d killed, %d survived\n' "$pass" "$fail"
if [ "$fail" -ne 0 ]; then
  echo "FAIL: a planted defect went undetected" >&2
  exit 1
fi
echo "PASS"
