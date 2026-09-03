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
# platform's own deployment mode over the SDK's topology classification,
# slipping a request past the dispatch funnel using a reqwest spelling the
# source guards did not think of, or — added with the adapter registry
# (enterprise#3682) — raising a cap, truncating instead of dropping, counting a
# cap in characters, bypassing the registry, or spawning the cold-path send
# instead of awaiting it, which loses the ping for exactly the short-lived
# processes the first-request trigger exists to make visible.
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

# Idempotent: `trap ... EXIT INT TERM` fires twice on Ctrl-C (once for INT,
# once for the EXIT it triggers), and the second run would try to copy from a
# directory the first one deleted.
_restored=0
restore() {
  [ "$_restored" -eq 1 ] && return 0
  _restored=1
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
  # `set -e` aborts on a bare assignment from a failing command substitution,
  # and a failing test is the EXPECTED outcome here — so the exit code is taken
  # through an || branch, which is exempt.
  local out code
  out=$(cargo test --lib "$test_name" -- --exact 2>&1) && code=0 || code=$?
  verdict "$code" "$out" "$test_name"
  restore_files
}

# verdict <exit-code> <output> <test-name>
# A test name that matches NOTHING exits 0 with "running 0 tests", which reads
# as a survivor: alarming in the safe direction, but it is a typo reported as a
# finding, and it cost a round to diagnose. Named explicitly instead.
verdict() {
  local code="$1" out="$2" test_name="$3"
  # "at least one test EXECUTED", not "no empty block appeared": a cargo run
  # prints a `running 0 tests` block for a second target even on a healthy
  # single-test run, and keying on that string reported all seventeen working
  # mutants as unreadable. The executed-test line is the thing being asserted.
  if ! printf '%s' "$out" | grep -qE '^test .+ \.\.\. '; then
    printf '    ERROR — no test matches %s; the gate cannot read this mutant\n' "$test_name" >&2
    fail=$((fail + 1))
    return
  fi
  if [ "$code" -eq 0 ]; then
    printf '    SURVIVED — the test does not detect this defect\n'
    fail=$((fail + 1))
  else
    printf '    killed\n'
    pass=$((pass + 1))
  fi
}

# mutant_it <name> <test> <find> <replace>
# Same, for a defect whose killing test lives in tests/read_identity_test.rs.
# `cargo test --lib` does NOT build the integration targets, so a mutant aimed at
# one and run with --lib would report "killed" for a test that never ran.
mutant_it() {
  local name="$1" test_name="$2" find="$3" repl="$4"
  restore_files
  apply "src/client.rs" "$find" "$repl"

  printf '\n=== mutant: %s\n    expecting RED: %s\n' "$name" "$test_name"
  local out code
  out=$(cargo test --test read_identity_test "$test_name" -- --exact 2>&1) && code=0 || code=$?
  verdict "$code" "$out" "$test_name"
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
if ! cargo test --lib client::read_identity_tests:: 2>&1 | tail -3; then
  echo "BASELINE FAILED — fix the suite before reading any mutant result" >&2
  exit 1
fi
if ! cargo test --test read_identity_test 2>&1 | tail -3; then
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
# 10b. The backoff exists but is not CONSULTED. Mutant 10 changes the interval
#      function's body; this one leaves the function perfect and substitutes
#      the base interval at the only call site. Before the call-site test this
#      one-token edit left all 58 heartbeat tests green while restoring the
#      hourly-probe-forever defect - testing the predicate is not testing the
#      call site.
# ---------------------------------------------------------------------------
mutant "backoff computed but not consulted at the call site" \
  "heartbeat::tests::the_widened_interval_actually_refuses_a_claim_at_the_call_site" \
  '        if last.elapsed() < guard_interval_for(inner.consecutive_failures) {' \
  '        if last.elapsed() < HEARTBEAT_GUARD_INTERVAL {'

# ---------------------------------------------------------------------------
# 10c. The in-memory 7-day cadence floor is removed, so a machine that cannot
#      persist the stamp file (HOME unset, read-only root filesystem) delivers
#      a ping every guard interval forever - 168x the disclosed rate, and the
#      failure backoff cannot help because these attempts succeed.
# ---------------------------------------------------------------------------
mutant "in-memory 7-day cadence floor removed" \
  "heartbeat::tests::a_stampless_environment_still_honours_the_seven_day_cadence" \
  '    if let Some(delivered) = inner.last_delivered {
        if delivered.elapsed() < HEARTBEAT_INTERVAL {
            return None;
        }
    }' \
  '    let _ = inner.last_delivered;'

# ---------------------------------------------------------------------------
# 10d. The probe stops identifying itself. This is the first SDK feature that
#      contacts the caller's own platform unsolicited; an unattributable
#      request in their access log is the difference between "the SDK" and
#      "something".
# ---------------------------------------------------------------------------
mutant "probe User-Agent removed" \
  "heartbeat::tests::probe_makes_exactly_one_request_per_heartbeat" \
  '        .user_agent(concat!("axonflow-sdk-rust/", env!("CARGO_PKG_VERSION")))
' \
  ''

# ---------------------------------------------------------------------------
# 11 + 12. The two source-scanning guards, attacked the way an earlier version
#     of them was actually evaded in review: a request issued through
#     `Client::execute` rather than `.send()`, and a second transport built
#     through `ClientBuilder::new()` rather than `reqwest::Client::builder()`.
#     Both left the previous guards green.
# ---------------------------------------------------------------------------
# Planted at `raw_get_as`, deliberately NOT at `checked_get`: `checked_get` is
# exercised by a behavioural test, which would kill this mutant on its own and
# leave the source guard's own strength unmeasured. UFCS form, because that is
# what evaded the guard in review.
#
# Re-anchored from `raw_get` when the read-path identity work (#85) replaced
# that method with `raw_get_as` — a defaulting wrapper would have been a second,
# quieter way to make an unidentified read, so it was removed rather than kept.
# The isolation the comment above depends on still holds: what this mutant
# bypasses is `dispatch`, and `dispatch` is where the heartbeat gate runs, which
# no behavioural test on this path asserts. Verified by running the gate: the
# mutant is killed by the SOURCE guard, and the suite is otherwise green under
# it.
mutant_in "src/client.rs" "an ungated request issued via UFCS Client::execute" \
  "heartbeat::tests::no_http_send_outside_the_dispatch_funnel" \
  '        let resp = self.dispatch(self.http_client.get(url), None).await?;' \
  '        let built = self.http_client.get(url).build()?;
        let resp = reqwest::Client::execute(&self.http_client, built).await?;'

# Qualified-path form, which is what evaded the guard in review.
mutant_in "src/client.rs" "a second transport built via a qualified path" \
  "heartbeat::tests::the_telemetry_path_builds_exactly_one_http_client" \
  '        let http_client = reqwest::Client::builder()' \
  '        let _extra: Result<reqwest::Client, _> = <reqwest::Client>::builder().build();
        let http_client = reqwest::Client::builder()'

# ---------------------------------------------------------------------------
# The read-path identity properties. Each of these four was a REAL defect in an
# earlier push of this branch, not an invented one — which is why they are here
# rather than left to the suite.
# ---------------------------------------------------------------------------

# The identity reaches only the per-call read helper, so a client derived with
# as_user runs every other method as the process. This was the shipped
# behaviour until round 2; the doc claiming otherwise is what made it invisible.
mutant_it "the identity stamped only when a per-call override is given" \
  "a_derived_client_presents_its_identity_on_every_route" \
  '        let token = match override_token {
            Some(explicit) => explicit.trim(),
            None => self.config.user_token.as_deref().unwrap_or("").trim(),
        };' \
  '        let token = match override_token {
            Some(explicit) => explicit.trim(),
            None => "",
        };'

# A token no header value can carry is dropped instead of reported. The read
# then goes out unidentified and the SDK blames the platform for it.
mutant_it "an unusable token silently dropped" \
  "a_token_that_cannot_be_a_header_value_is_reported_not_dropped" \
  '        let mut value = reqwest::header::HeaderValue::from_str(token)
            .map_err(|_| AxonFlowError::ConfigError(crate::read_identity::unusable_token(token)))?;' \
  '        let Ok(mut value) = reqwest::header::HeaderValue::from_str(token) else {
            return Ok(());
        };'

# The MAP transport is a SECOND reqwest::Client, and therefore a second chance
# to forget the redirect policy. Every redirect test drove the other one.
mutant_it "the redirect policy dropped from the MAP transport" \
  "the_map_transport_also_refuses_an_off_origin_redirect" \
  '        let map_http_client = reqwest::Client::builder()
            .timeout(config.map_timeout)
            .redirect(Self::redirect_policy(&config.endpoint))' \
  '        let map_http_client = reqwest::Client::builder()
            .timeout(config.map_timeout)'

# The identity dropped from the cache key. A derived client shares the parent's
# Arc<Cache> by design, so without the identity in the key two derived clients
# making the same call hash to ONE entry and the second is handed the first
# one's governed response - a cross-user data leak with no request made on the
# second caller's behalf at all.
mutant_it "the read identity dropped from the cache key" \
  "two_derived_clients_do_not_share_a_cached_response" \
  '        self.config
            .user_token
            .as_deref()
            .unwrap_or("")
            .hash(&mut hasher);' \
  '        "".hash(&mut hasher);'

# The other direction: a key that never matches is a disabled cache wearing a
# fix's name, and it would satisfy the leak test above.
mutant_it "the cache key made unique per call" \
  "one_identity_asking_twice_still_hits_the_cache" \
  '        format!("{:x}", hasher.finish())' \
  '        format!("{:x}-{:?}", hasher.finish(), std::time::Instant::now())'

# set_sensitive is what keeps the credential out of tracing spans and panic
# messages; nothing about the wire changes when it goes, so only a Debug
# assertion can see it.
mutant_in "src/client.rs" "the identity header no longer marked sensitive" \
  "client::read_identity_tests::the_identity_is_redacted_from_the_requests_debug_output" \
  '        value.set_sensitive(true);' \
  '        value.set_sensitive(false);'

# ---------------------------------------------------------------------------
# The adapter registry and the awaited cold path (enterprise#3682).
# ---------------------------------------------------------------------------

mutant "the relayed-value cap raised past the boundary" \
  "heartbeat::tests::the_relayed_value_cap_keeps_64_bytes_and_drops_65_whole" \
  'const MAX_RELAYED_VALUE_LEN: usize = 64;' \
  'const MAX_RELAYED_VALUE_LEN: usize = 65;'

# A truncated adapter name is a name nothing is running, and the receiver
# records it as a real value.
mutant "an over-cap adapter name truncated instead of dropped" \
  "heartbeat::tests::the_relayed_value_cap_keeps_64_bytes_and_drops_65_whole" \
  '    if normalized.is_empty() || normalized.len() > MAX_RELAYED_VALUE_LEN {
        return;
    }' \
  '    if normalized.is_empty() {
        return;
    }
    let normalized = if normalized.len() > MAX_RELAYED_VALUE_LEN {
        normalized[..MAX_RELAYED_VALUE_LEN].to_string()
    } else {
        normalized
    };'

# str::len() is already bytes in Rust, so this SDK gets the distinction for
# free — but a refactor to chars().count() would be silent without a gate.
mutant "the cap counted in characters instead of bytes" \
  "heartbeat::tests::the_cap_counts_bytes_not_characters" \
  '    if normalized.is_empty() || normalized.len() > MAX_RELAYED_VALUE_LEN {' \
  '    if normalized.is_empty() || normalized.chars().count() > MAX_RELAYED_VALUE_LEN {'

mutant "the features array cap raised past the boundary" \
  "heartbeat::tests::the_features_array_is_bounded_to_32_entries" \
  'const MAX_FEATURES: usize = 32;' \
  'const MAX_FEATURES: usize = 33;'

mutant "bound_features keeps an over-long entry" \
  "heartbeat::tests::bound_features_drops_an_overlong_entry_whole" \
  '        .filter(|f| f.len() <= MAX_FEATURE_BYTES)' \
  '        .filter(|_f| true)'

# The receiver folds case before matching; sending the fold keeps one spelling
# on the row. Dropping it puts two.
mutant "adapter names no longer lowercased" \
  "heartbeat::tests::names_are_lowercased_trimmed_deduplicated_and_sorted" \
  '    let normalized = name.trim().to_lowercase();' \
  '    let normalized = name.trim().to_string();'

# The registry is the ONLY producer of `features`; hardcoding the array is the
# state this feature replaced.
mutant "the registry bypassed and features hardcoded empty" \
  "heartbeat::tests::a_registered_adapter_reaches_the_wire" \
  '            features: registered_features(),' \
  '            features: Vec::new(),'

# `adapter:` alone is not an identifier.
mutant "an empty adapter name accepted" \
  "heartbeat::tests::an_unusable_name_is_refused_silently" \
  '    if normalized.is_empty() || normalized.len() > MAX_RELAYED_VALUE_LEN {' \
  '    if normalized.len() > MAX_RELAYED_VALUE_LEN {'

# THE ONE THAT MATTERS MOST. A spawned send is dropped when the process does
# not outlive it: measured at 1 delivery in 12 for a compiled one-call binary,
# while the /health GET reached the platform every time.
#
# It needs a test that does NOT poll. Every other heartbeat test uses
# `await_ping`, which waits for a spawned send too, so the whole suite stayed
# green under this mutant on the first run of this gate — the named test asserts
# the ping has already landed the instant `dispatch` returns.
mutant_in "src/client.rs" "the cold-path send spawned instead of awaited" \
  "heartbeat::tests::the_cold_path_send_is_awaited_not_spawned" \
  '        maybe_send_heartbeat_on_request(&self.config.endpoint, &self.config.mode).await;' \
  '        crate::heartbeat::maybe_send_heartbeat(&self.config.endpoint, &self.config.mode);'

# Constructing a client is not usage. Restoring the constructor trigger makes
# every adapter registration too late for the first ping again.
mutant_in "src/client.rs" "the constructor pings again" \
  "heartbeat::tests::the_first_request_delivers_through_the_spawn_path" \
  '        Ok(Self {
            config,' \
  '        crate::heartbeat::maybe_send_heartbeat(&config.endpoint, &config.mode);
        Ok(Self {
            config,'

printf '\n=== mutation gate: %d killed, %d survived\n' "$pass" "$fail"
if [ "$fail" -ne 0 ]; then
  echo "FAIL: a planted defect went undetected" >&2
  exit 1
fi
echo "PASS"
