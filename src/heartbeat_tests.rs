//! Tests for the SDK heartbeat.
//!
//! Split into four layers, deliberately:
//!
//! 1. **Pure functions** — classification, normalisation, the promotion rule.
//! 2. **The probe against a real HTTP server** — every way `/health` can
//!    answer, including the ways it answers *successfully* with something
//!    hostile.
//! 3. **The whole send** — probe plus POST against one server, asserting on
//!    the bytes that actually reached the wire.
//! 4. **The gate** — the opt-out, the 1-hour in-process guard, the 7-day
//!    stamp, and the two trigger sites.
//!
//! Layers 1 and 4 touch process-global state (environment variables, the
//! stamp-path override, the gate itself) and hold [`telemetry_lock`]. Layers 2
//! and 3 construct their [`HeartbeatContext`] directly, read no environment,
//! and therefore run in parallel with everything else.

use super::*;
use std::sync::MutexGuard;
use std::time::Instant;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

// ============================================================================
// Harness
// ============================================================================

/// Serialises every test that touches process-global state. `cargo test` runs
/// a crate's unit tests in parallel threads of ONE process, so two tests
/// mutating `AXONFLOW_TELEMETRY` or the gate would otherwise observe each
/// other.
///
/// Poisoning is recovered from rather than propagated: one panicking test
/// should fail on its own assertion, not turn every later test into a
/// confusing `PoisonError`.
fn telemetry_lock() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// Holds the global lock and restores every piece of process state on drop —
/// including when the test panics, so a failure never cascades into the next
/// test.
struct TelemetryTestEnv {
    _guard: MutexGuard<'static, ()>,
    _stamp_dir: tempfile::TempDir,
    stamp_path: PathBuf,
}

impl TelemetryTestEnv {
    /// Telemetry ON, a private stamp file, and a cleared gate.
    ///
    /// `AXONFLOW_TELEMETRY` is explicitly REMOVED rather than assumed unset:
    /// this repo's CI sets `AXONFLOW_TELEMETRY=off` for the whole workflow, so
    /// a test that relied on the ambient environment would silently assert
    /// nothing there.
    fn on() -> Self {
        let guard = telemetry_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let stamp_path = dir.path().join("rust-telemetry-last-sent");

        std::env::remove_var("AXONFLOW_TELEMETRY");
        std::env::remove_var("AXONFLOW_TRY");
        std::env::remove_var("ORG_ID");
        std::env::remove_var("AXONFLOW_CHECKPOINT_URL");
        *STAMP_PATH_OVERRIDE
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(stamp_path.clone());
        reset_gate_for_tests();
        TELEMETRY_ARMED_FOR_TESTS.with(|armed| armed.set(true));

        Self {
            _guard: guard,
            _stamp_dir: dir,
            stamp_path,
        }
    }

    fn set(&self, key: &str, value: &str) {
        std::env::set_var(key, value);
    }

    /// Reopen the gate without touching the 7-day stamp or the accumulated
    /// backoff — the state a process reaches once the guard interval has
    /// elapsed. Not a full reset: erasing the failure counter here would make
    /// consecutive failures look like a first failure every time.
    fn advance_past_the_guard(&self) {
        reopen_gate_for_tests();
    }

    fn stamp_exists(&self) -> bool {
        self.stamp_path.exists()
    }
}

impl Drop for TelemetryTestEnv {
    fn drop(&mut self) {
        TELEMETRY_ARMED_FOR_TESTS.with(|armed| armed.set(false));
        *STAMP_PATH_OVERRIDE
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = None;
        std::env::remove_var("AXONFLOW_TELEMETRY");
        std::env::remove_var("AXONFLOW_TRY");
        std::env::remove_var("ORG_ID");
        std::env::remove_var("AXONFLOW_CHECKPOINT_URL");
        reset_gate_for_tests();
    }
}

/// A [`HeartbeatContext`] built literally, reading no environment, so the
/// probe/send tests are parallel-safe and depend on nothing global.
fn ctx_for(endpoint: &str, checkpoint_url: &str) -> HeartbeatContext {
    HeartbeatContext {
        endpoint: endpoint.to_string(),
        checkpoint_url: checkpoint_url.to_string(),
        stamp_path: None,
        stream: None,
        deployment_mode: DEPLOYMENT_MODE_SELF_HOSTED,
        endpoint_type: ENDPOINT_TYPE_LOCALHOST,
        org_id: "test-org".to_string(),
    }
}

/// Nothing listens on port 1, so a connection there is refused immediately —
/// a deterministic "platform unreachable" without racing on a port we bound
/// and released.
const UNREACHABLE_ENDPOINT: &str = "http://127.0.0.1:1";

/// Mount the checkpoint receiver. Every send test needs it; the status is the
/// variable.
async fn mount_checkpoint(server: &MockServer, status: u16) {
    Mock::given(method("POST"))
        .and(path("/v1/ping"))
        .respond_with(ResponseTemplate::new(status).set_body_string("{\"latest_version\":null}"))
        .mount(server)
        .await;
}

async fn mount_health_json(server: &MockServer, body: serde_json::Value) {
    Mock::given(method("GET"))
        .and(path("/health"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(server)
        .await;
}

fn checkpoint_url(server: &MockServer) -> String {
    format!("{}/v1/ping", server.uri())
}

/// Every request the server saw, as (method, path).
async fn seen(server: &MockServer) -> Vec<(String, String)> {
    server
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .map(|r| (r.method.to_string(), r.url.path().to_string()))
        .collect()
}

/// The decoded body of the single ping the server received. Panics with a
/// useful message when there was none — "no ping was sent" is a different
/// failure from "the ping was wrong", and a test that cannot tell them apart
/// passes vacuously.
async fn only_ping_body(server: &MockServer) -> serde_json::Value {
    let requests = server.received_requests().await.unwrap_or_default();
    let pings: Vec<_> = requests
        .iter()
        .filter(|r| r.url.path() == "/v1/ping")
        .collect();
    assert_eq!(
        pings.len(),
        1,
        "expected exactly one checkpoint POST, saw {}: {:?}",
        pings.len(),
        seen(server).await
    );
    serde_json::from_slice(&pings[0].body).expect("ping body is valid JSON")
}

fn count_pings(seen: &[(String, String)]) -> usize {
    seen.iter()
        .filter(|(m, p)| m == "POST" && p == "/v1/ping")
        .count()
}

fn count_health(seen: &[(String, String)]) -> usize {
    seen.iter()
        .filter(|(m, p)| m == "GET" && p == "/health")
        .count()
}

/// Captures `tracing` output so a test can assert on what the SDK told the
/// operator — and, more importantly, on what it did NOT tell them.
///
/// Two things force this shape, both learned the hard way:
///
/// 1. **The subscriber must be the GLOBAL default, installed once.** `tracing`
///    caches each callsite's interest process-wide, and a callsite first
///    reached while no subscriber is installed is cached as *never*
///    interested. `cargo test` runs these tests in parallel, so another test
///    routinely reaches a diagnostic first and switches it off for this one.
///    Only `set_global_default` rebuilds that cache; a scoped
///    `set_default` does not, and neither does calling
///    `rebuild_interest_cache` under one.
/// 2. **A global subscriber sees every thread**, so the buffer would fill with
///    concurrent tests' output and an assertion could pass on a line another
///    test emitted. The writer therefore records only while an *armed* thread
///    is emitting, which makes the captured buffer exactly one test's output.
#[derive(Default)]
struct LogCapture {
    buf: Mutex<Vec<u8>>,
    armed: Mutex<Option<std::thread::ThreadId>>,
}

/// Exclusive arming of the capture, released on drop.
struct LogCaptureArmed(
    &'static LogCapture,
    /// Held for the armed window so two tests can never be armed at once.
    #[allow(dead_code)]
    MutexGuard<'static, ()>,
);

impl LogCapture {
    /// Install the capture as the process-wide subscriber, once.
    fn global() -> &'static LogCapture {
        static CAP: OnceLock<LogCapture> = OnceLock::new();
        let cap = CAP.get_or_init(LogCapture::default);
        static INSTALLED: OnceLock<()> = OnceLock::new();
        INSTALLED.get_or_init(|| {
            let subscriber = tracing_subscriber::fmt()
                .with_max_level(tracing::Level::DEBUG)
                .with_writer(CapWriter(cap))
                .with_ansi(false)
                .finish();
            tracing::subscriber::set_global_default(subscriber)
                .expect("no other global tracing subscriber may be installed in this test binary");
        });
        cap
    }

    /// Claim the capture for this thread and clear it. Serialised so two tests
    /// can never be armed at once.
    fn arm() -> LogCaptureArmed {
        static ARM_LOCK: Mutex<()> = Mutex::new(());
        let guard = ARM_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let cap = LogCapture::global();
        cap.buf.lock().unwrap_or_else(|e| e.into_inner()).clear();
        *cap.armed.lock().unwrap_or_else(|e| e.into_inner()) = Some(std::thread::current().id());
        LogCaptureArmed(cap, guard)
    }
}

impl LogCaptureArmed {
    fn contents(&self) -> String {
        String::from_utf8_lossy(&self.0.buf.lock().unwrap_or_else(|e| e.into_inner())).to_string()
    }
}

impl Drop for LogCaptureArmed {
    fn drop(&mut self) {
        *self.0.armed.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

#[derive(Clone, Copy)]
struct CapWriter(&'static LogCapture);

impl std::io::Write for CapWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let armed = *self.0.armed.lock().unwrap_or_else(|e| e.into_inner());
        if armed == Some(std::thread::current().id()) {
            self.0
                .buf
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .extend_from_slice(buf);
        }
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CapWriter {
    type Writer = Self;
    fn make_writer(&'a self) -> Self::Writer {
        *self
    }
}

// ============================================================================
// 1. Pure functions
// ============================================================================

#[test]
fn classify_endpoint_localhost_variants() {
    assert_eq!(
        classify_endpoint("http://localhost:8080"),
        ENDPOINT_TYPE_LOCALHOST
    );
    assert_eq!(
        classify_endpoint("https://127.0.0.1:8080"),
        ENDPOINT_TYPE_LOCALHOST
    );
    assert_eq!(
        classify_endpoint("http://0.0.0.0:9090"),
        ENDPOINT_TYPE_LOCALHOST
    );
    assert_eq!(
        classify_endpoint("http://my.localhost"),
        ENDPOINT_TYPE_LOCALHOST
    );
    assert_eq!(
        classify_endpoint("http://[::1]:8080"),
        ENDPOINT_TYPE_LOCALHOST
    );
}

#[test]
fn classify_endpoint_private_variants() {
    assert_eq!(classify_endpoint("http://10.1.2.3"), ENDPOINT_TYPE_PRIVATE);
    assert_eq!(
        classify_endpoint("http://192.168.1.1"),
        ENDPOINT_TYPE_PRIVATE
    );
    assert_eq!(
        classify_endpoint("http://172.16.0.1"),
        ENDPOINT_TYPE_PRIVATE
    );
    assert_eq!(classify_endpoint("http://api.local"), ENDPOINT_TYPE_PRIVATE);
    assert_eq!(
        classify_endpoint("http://api.internal"),
        ENDPOINT_TYPE_PRIVATE
    );
}

#[test]
fn classify_endpoint_remote() {
    assert_eq!(
        classify_endpoint("https://api.example.com"),
        ENDPOINT_TYPE_REMOTE
    );
    assert_eq!(
        classify_endpoint("https://203.0.113.5"),
        ENDPOINT_TYPE_REMOTE
    );
}

#[test]
fn classify_endpoint_unknown() {
    assert_eq!(classify_endpoint(""), ENDPOINT_TYPE_UNKNOWN);
    assert_eq!(classify_endpoint("not a url"), ENDPOINT_TYPE_UNKNOWN);
}

#[test]
fn stream_for_mode_classification() {
    assert_eq!(stream_for_mode(&Mode::Sandbox), Some(STREAM_SANDBOX));
    assert_eq!(stream_for_mode(&Mode::Production), None);
}

#[test]
fn classify_deployment_mode_v1_schema() {
    let env = TelemetryTestEnv::on();

    // v1 schema: deployment_mode is endpoint-derived, not Mode-derived.
    // Empty/unparseable -> unknown.
    assert_eq!(classify_deployment_mode(""), DEPLOYMENT_MODE_UNKNOWN);
    assert_eq!(
        classify_deployment_mode("not a url"),
        DEPLOYMENT_MODE_UNKNOWN
    );
    // Public host -> self_hosted.
    assert_eq!(
        classify_deployment_mode("https://api.example.com"),
        DEPLOYMENT_MODE_SELF_HOSTED
    );
    // *.try.getaxonflow.com -> community_saas.
    assert_eq!(
        classify_deployment_mode("https://try.getaxonflow.com"),
        DEPLOYMENT_MODE_COMMUNITY_SAAS
    );
    assert_eq!(
        classify_deployment_mode("https://eu.try.getaxonflow.com"),
        DEPLOYMENT_MODE_COMMUNITY_SAAS
    );
    // AXONFLOW_TRY=1 forces community_saas regardless of host.
    env.set("AXONFLOW_TRY", "1");
    assert_eq!(
        classify_deployment_mode("https://my-proxy.example.com"),
        DEPLOYMENT_MODE_COMMUNITY_SAAS
    );
}

#[test]
fn telemetry_off_recognizes_off_value() {
    let env = TelemetryTestEnv::on();
    env.set("AXONFLOW_TELEMETRY", "off");
    assert!(telemetry_off());
    env.set("AXONFLOW_TELEMETRY", "OFF");
    assert!(telemetry_off());
    env.set("AXONFLOW_TELEMETRY", "  off  ");
    assert!(telemetry_off());
    env.set("AXONFLOW_TELEMETRY", "");
    assert!(!telemetry_off());
    env.set("AXONFLOW_TELEMETRY", "on");
    assert!(!telemetry_off());
    std::env::remove_var("AXONFLOW_TELEMETRY");
    assert!(!telemetry_off());
}

// --- v9.1 org_id (#2277) ---

#[test]
fn telemetry_org_id_env_wins() {
    let env = TelemetryTestEnv::on();
    env.set("ORG_ID", "acme-corp");
    assert_eq!(telemetry_org_id(), "acme-corp");
}

#[test]
fn telemetry_org_id_unset_returns_sentinel() {
    let _env = TelemetryTestEnv::on();
    assert_eq!(telemetry_org_id(), ORG_ID_LOCAL_DEV_SENTINEL);
    assert_eq!(ORG_ID_LOCAL_DEV_SENTINEL, "local-dev-org");
}

#[test]
fn telemetry_org_id_empty_falls_through_to_sentinel() {
    let env = TelemetryTestEnv::on();
    env.set("ORG_ID", "");
    assert_eq!(telemetry_org_id(), ORG_ID_LOCAL_DEV_SENTINEL);
}

#[test]
fn telemetry_org_id_cs_prefixed_passes_through() {
    let env = TelemetryTestEnv::on();
    let cs_id = "cs_e3a4b5c6-d7e8-4f90-a1b2-c3d4e5f6a7b8";
    env.set("ORG_ID", cs_id);
    assert_eq!(telemetry_org_id(), cs_id);
}

// --- runtime_version (#88 item 5) ---

#[test]
fn normalize_rustc_version_keeps_the_version_and_drops_the_build_id() {
    assert_eq!(
        normalize_rustc_version(Some("rustc 1.95.0 (59807616e 2026-04-14)")),
        "rustc 1.95.0"
    );
    assert_eq!(
        normalize_rustc_version(Some("rustc 1.96.0-nightly (abcdef012 2026-05-01)")),
        "rustc 1.96.0-nightly"
    );
    assert_eq!(
        normalize_rustc_version(Some("rustc 1.95.0-beta.2 (deadbeef1 2026-03-01)")),
        "rustc 1.95.0-beta.2"
    );
    // No build id at all is still a valid rustc line.
    assert_eq!(
        normalize_rustc_version(Some("rustc 1.95.0")),
        "rustc 1.95.0"
    );
    assert_eq!(
        normalize_rustc_version(Some("  rustc 1.95.0  ")),
        "rustc 1.95.0"
    );
}

#[test]
fn normalize_rustc_version_refuses_anything_it_cannot_recognise() {
    // build.rs never set the variable.
    assert_eq!(normalize_rustc_version(None), RUNTIME_VERSION_UNKNOWN);
    assert_eq!(normalize_rustc_version(Some("")), RUNTIME_VERSION_UNKNOWN);
    assert_eq!(
        normalize_rustc_version(Some("   ")),
        RUNTIME_VERSION_UNKNOWN
    );
    // A wrapper that prints something else entirely.
    assert_eq!(
        normalize_rustc_version(Some("my-wrapper 1.0")),
        RUNTIME_VERSION_UNKNOWN
    );
    // "rustc" with no version token.
    assert_eq!(
        normalize_rustc_version(Some("rustc")),
        RUNTIME_VERSION_UNKNOWN
    );
    // A version token that is not a version.
    assert_eq!(
        normalize_rustc_version(Some("rustc version-one")),
        RUNTIME_VERSION_UNKNOWN
    );
    // An over-long token cannot widen the field.
    let long = format!("rustc 1{}", "9".repeat(MAX_RELAYED_VALUE_LEN));
    assert_eq!(
        normalize_rustc_version(Some(&long)),
        RUNTIME_VERSION_UNKNOWN
    );
}

#[test]
fn runtime_version_is_the_real_toolchain_and_never_the_old_literal() {
    let v = runtime_version_str();
    assert_ne!(
        v, "rustc-stable",
        "the fabricated pre-0.10.0 literal must not survive anywhere"
    );
    assert!(
        v == RUNTIME_VERSION_UNKNOWN || v.starts_with("rustc "),
        "runtime_version was {v:?}; expected the real toolchain or an honest 'unknown'"
    );
    // In this repo's CI and on any developer machine, build.rs CAN run rustc,
    // so the honest-fallback branch must not be what we are shipping.
    assert!(
        v.starts_with("rustc "),
        "build.rs failed to capture the toolchain: runtime_version was {v:?}"
    );
}

// --- the budget split ---

#[test]
fn budget_split_leaves_room_for_the_post() {
    // The property the whole two-phase design exists for: however long the
    // probe takes, the POST is still above the floor. `send_heartbeat`'s
    // "budget exhausted" branch documents itself as unreachable while this
    // holds — this is what makes that claim true rather than aspirational.
    assert!(
        HEALTH_BUDGET_CAP + MIN_BUDGET < HEARTBEAT_TIMEOUT,
        "health cap {HEALTH_BUDGET_CAP:?} + floor {MIN_BUDGET:?} must leave the POST room inside {HEARTBEAT_TIMEOUT:?}"
    );
    // And the probe must be worth attempting at all.
    assert!(HEALTH_BUDGET_CAP > MIN_BUDGET);
}

// --- the promotion rule ---

#[test]
fn learned_value_accepts_only_a_present_non_empty_bounded_string() {
    let body = serde_json::json!({
        "ok": "10.4.0",
        "empty": "",
        "number": 42,
        "null": null,
        "object": {"nested": "x"},
        "array": ["x"],
        "boolean": true,
        "at_cap": "a".repeat(MAX_RELAYED_VALUE_LEN),
        "over_cap": "a".repeat(MAX_RELAYED_VALUE_LEN + 1),
    });

    assert_eq!(learned_value(&body, "ok"), Some("10.4.0".to_string()));
    assert_eq!(
        learned_value(&body, "at_cap"),
        Some("a".repeat(MAX_RELAYED_VALUE_LEN))
    );

    // Every "not learned" shape. None of them may produce a value.
    for key in [
        "empty", "number", "null", "object", "array", "boolean", "over_cap", "absent",
    ] {
        assert_eq!(
            learned_value(&body, key),
            None,
            "key {key:?} must not be learned"
        );
    }
}

#[test]
fn learned_value_drops_an_over_long_value_whole_rather_than_truncating() {
    let long = "E".repeat(MAX_RELAYED_VALUE_LEN + 1);
    let body = serde_json::json!({ "tier": long });
    // Not `Some(truncated)` — a truncated string is a claim the platform
    // never made.
    assert_eq!(learned_value(&body, "tier"), None);
}

// ============================================================================
// 2. The probe
// ============================================================================

/// The SHIPPED telemetry client, not a lookalike. Building one here instead
/// is how the first version of `a_redirecting_health_endpoint_is_refused_not_followed`
/// passed a redirect straight through: it tested the helper, not the code.
fn probe_client() -> reqwest::Client {
    telemetry_client().expect("client")
}

#[tokio::test]
async fn probe_learns_every_field_when_health_answers() {
    let server = MockServer::start().await;
    mount_health_json(
        &server,
        serde_json::json!({
            "status": "healthy",
            "version": "10.4.0",
            "tier": "Enterprise",
            "edition": "enterprise",
            "deployment_mode": "self_hosted",
        }),
    )
    .await;

    let probe = probe_platform_health(&probe_client(), &server.uri(), HEALTH_BUDGET_CAP).await;

    assert_eq!(
        probe,
        HealthProbe {
            platform_version: Some("10.4.0".into()),
            license_tier: Some("Enterprise".into()),
            edition: Some("enterprise".into()),
            deployment_mode: Some("self_hosted".into()),
        }
    );
}

#[tokio::test]
async fn probe_learns_the_pre_3660_shape_without_edition_or_deployment_mode() {
    // Every platform released before enterprise#3660 answers with `tier` and
    // `version` only. The two new relays must be absent, not defaulted — this
    // is what makes the SDK correct against a platform that predates the lane
    // it relays for.
    let server = MockServer::start().await;
    mount_health_json(
        &server,
        serde_json::json!({"status": "healthy", "version": "10.3.0", "tier": "Community"}),
    )
    .await;

    let probe = probe_platform_health(&probe_client(), &server.uri(), HEALTH_BUDGET_CAP).await;

    assert_eq!(probe.platform_version.as_deref(), Some("10.3.0"));
    assert_eq!(probe.license_tier.as_deref(), Some("Community"));
    assert_eq!(probe.edition, None);
    assert_eq!(probe.deployment_mode, None);
}

#[tokio::test]
async fn probe_forwards_the_transient_starting_tier_verbatim() {
    // An agent caught inside its pre-init window reports "starting". It is a
    // real signal the receiver buckets deliberately, not an error to filter
    // client-side.
    let server = MockServer::start().await;
    mount_health_json(
        &server,
        serde_json::json!({"status": "starting", "tier": "starting"}),
    )
    .await;

    let probe = probe_platform_health(&probe_client(), &server.uri(), HEALTH_BUDGET_CAP).await;
    assert_eq!(probe.license_tier.as_deref(), Some("starting"));
}

#[tokio::test]
async fn probe_promotes_each_field_independently() {
    // A badly-typed member must not take down a member that was fine. With a
    // typed struct decode, `tier: 42` would fail the whole body and silently
    // drop `version` — a field that worked before the tier was added.
    let server = MockServer::start().await;
    mount_health_json(
        &server,
        serde_json::json!({"version": "10.4.0", "tier": 42, "edition": null, "deployment_mode": ""}),
    )
    .await;

    let probe = probe_platform_health(&probe_client(), &server.uri(), HEALTH_BUDGET_CAP).await;

    assert_eq!(probe.platform_version.as_deref(), Some("10.4.0"));
    assert_eq!(probe.license_tier, None);
    assert_eq!(probe.edition, None);
    assert_eq!(probe.deployment_mode, None);
}

#[tokio::test]
async fn probe_returns_nothing_when_health_is_unreachable() {
    let probe =
        probe_platform_health(&probe_client(), UNREACHABLE_ENDPOINT, HEALTH_BUDGET_CAP).await;
    assert_eq!(probe, HealthProbe::default());
}

#[tokio::test]
async fn probe_returns_nothing_on_a_server_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/health"))
        .respond_with(
            ResponseTemplate::new(500).set_body_json(serde_json::json!({"tier": "Enterprise"})),
        )
        .mount(&server)
        .await;

    let probe = probe_platform_health(&probe_client(), &server.uri(), HEALTH_BUDGET_CAP).await;
    assert_eq!(
        probe,
        HealthProbe::default(),
        "a non-2xx body must not be read for values"
    );
}

#[tokio::test]
async fn probe_returns_nothing_when_health_is_absent() {
    // Nothing mounted: wiremock answers 404, which is what a platform without
    // the route does.
    let server = MockServer::start().await;
    let probe = probe_platform_health(&probe_client(), &server.uri(), HEALTH_BUDGET_CAP).await;
    assert_eq!(probe, HealthProbe::default());
}

#[tokio::test]
async fn probe_returns_nothing_on_a_non_json_body() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/health"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html>not json</html>"))
        .mount(&server)
        .await;

    let probe = probe_platform_health(&probe_client(), &server.uri(), HEALTH_BUDGET_CAP).await;
    assert_eq!(probe, HealthProbe::default());
}

#[tokio::test]
async fn probe_returns_nothing_when_the_body_exceeds_the_cap() {
    let server = MockServer::start().await;
    // Valid JSON, and it carries the fields — but it is larger than the SDK
    // will buffer, so it is refused before parsing.
    let huge = serde_json::json!({
        "version": "10.4.0",
        "tier": "Enterprise",
        "padding": "x".repeat(MAX_HEALTH_BODY_BYTES + 1),
    });
    mount_health_json(&server, huge).await;

    let probe = probe_platform_health(&probe_client(), &server.uri(), HEALTH_BUDGET_CAP).await;
    assert_eq!(probe, HealthProbe::default());
}

#[tokio::test]
async fn probe_makes_exactly_one_request_per_heartbeat() {
    let server = MockServer::start().await;
    mount_health_json(
        &server,
        serde_json::json!({"version": "10.4.0", "tier": "Community"}),
    )
    .await;

    let _ = probe_platform_health(&probe_client(), &server.uri(), HEALTH_BUDGET_CAP).await;

    let seen = seen(&server).await;
    assert_eq!(
        count_health(&seen),
        1,
        "every relayed dimension must ride ONE /health response; saw {seen:?}"
    );
}

#[tokio::test]
async fn probe_skips_a_blank_endpoint_without_attempting_a_request() {
    // Asserting only the return value could not tell "skipped" from "attempted
    // and failed" — both are the default probe. The log is what distinguishes
    // them: an attempt that dies at URL parse emits a failure diagnostic.
    let logs = LogCapture::arm();
    for endpoint in ["", "/", "   ", "///"] {
        let probe = probe_platform_health(&probe_client(), endpoint, HEALTH_BUDGET_CAP).await;
        assert_eq!(probe, HealthProbe::default(), "endpoint {endpoint:?}");
    }
    let captured = logs.contents();
    assert!(
        !captured.contains("/health probe failed"),
        "a blank endpoint must be skipped, not attempted; logs:\n{captured}"
    );
}

#[tokio::test]
async fn probe_tolerates_a_trailing_slash_on_the_endpoint() {
    let server = MockServer::start().await;
    mount_health_json(&server, serde_json::json!({"version": "10.4.0"})).await;

    let probe = probe_platform_health(
        &probe_client(),
        &format!("{}/", server.uri()),
        HEALTH_BUDGET_CAP,
    )
    .await;
    assert_eq!(probe.platform_version.as_deref(), Some("10.4.0"));
}

// ============================================================================
// 3. The whole send — asserting on the bytes that reached the wire
// ============================================================================

#[tokio::test]
async fn ping_carries_every_relayed_field_when_health_answers() {
    let server = MockServer::start().await;
    mount_health_json(
        &server,
        serde_json::json!({
            "version": "10.4.0",
            "tier": "EnterprisePlus",
            "edition": "enterprise",
            // DELIBERATELY not the value the SDK derives for this endpoint.
            // The platform's own deployment mode and the SDK's topology
            // classification are different questions that happen to share a
            // vocabulary; a fixture where they agree cannot tell a correct
            // relay from one that wrote the platform's answer over the SDK's
            // field, which would corrupt every existing deployment-mode
            // dashboard (flagged by the platform lane, enterprise#3660).
            "deployment_mode": "community_saas",
        }),
    )
    .await;
    mount_checkpoint(&server, 200).await;

    let delivered = send_heartbeat(&ctx_for(&server.uri(), &checkpoint_url(&server))).await;
    assert!(delivered);

    let body = only_ping_body(&server).await;
    assert_eq!(body["platform_version"], "10.4.0");
    assert_eq!(body["license_tier"], "EnterprisePlus");
    assert_eq!(body["edition"], "enterprise");
    assert_eq!(body["platform_deployment_mode"], "community_saas");

    // The SDK's own endpoint classification is a DIFFERENT field and must
    // survive untouched, still carrying what the SDK derived rather than what
    // the platform reported.
    assert_eq!(body["deployment_mode"], DEPLOYMENT_MODE_SELF_HOSTED);
    assert_ne!(
        body["deployment_mode"], body["platform_deployment_mode"],
        "the SDK's topology classification was overwritten by the platform's answer"
    );

    // The pre-existing wire shape is unchanged.
    assert_eq!(body["telemetry_type"], "sdk");
    assert_eq!(body["sdk"], "rust");
    assert_eq!(body["sdk_version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(body["org_id"], "test-org");
    assert_eq!(body["features"], serde_json::json!([]));
    assert!(body.get("stream").is_none(), "production mode omits stream");
}

/// Every way `/health` can fail. In all of them the ping must still be
/// delivered, and every relayed key must be ABSENT — asked as `has(key)`, so
/// a `null` fails the assertion just as loudly as a substituted default.
#[tokio::test]
async fn ping_is_still_sent_and_omits_the_keys_on_every_health_failure() {
    #[derive(Debug)]
    enum Health {
        Unreachable,
        Missing,
        ServerError,
        NotJson,
        NoKeys,
        WrongTypes,
        OversizedBody,
    }

    for case in [
        Health::Unreachable,
        Health::Missing,
        Health::ServerError,
        Health::NotJson,
        Health::NoKeys,
        Health::WrongTypes,
        Health::OversizedBody,
    ] {
        let server = MockServer::start().await;
        mount_checkpoint(&server, 200).await;

        match case {
            Health::Unreachable | Health::Missing => {}
            Health::ServerError => {
                Mock::given(method("GET"))
                    .and(path("/health"))
                    .respond_with(ResponseTemplate::new(500))
                    .mount(&server)
                    .await;
            }
            Health::NotJson => {
                Mock::given(method("GET"))
                    .and(path("/health"))
                    .respond_with(ResponseTemplate::new(200).set_body_string("nope"))
                    .mount(&server)
                    .await;
            }
            Health::NoKeys => {
                mount_health_json(&server, serde_json::json!({"status": "healthy"})).await;
            }
            Health::WrongTypes => {
                mount_health_json(
                    &server,
                    serde_json::json!({"version": 1, "tier": [], "edition": {}, "deployment_mode": false}),
                )
                .await;
            }
            Health::OversizedBody => {
                mount_health_json(
                    &server,
                    serde_json::json!({
                        "version": "10.4.0",
                        "padding": "x".repeat(MAX_HEALTH_BODY_BYTES + 1),
                    }),
                )
                .await;
            }
        }

        // The unreachable case points the probe somewhere dead while still
        // POSTing to the live server, so "the ping survived" is observable.
        let endpoint = match case {
            Health::Unreachable => UNREACHABLE_ENDPOINT.to_string(),
            _ => server.uri(),
        };

        let delivered = send_heartbeat(&ctx_for(&endpoint, &checkpoint_url(&server))).await;
        assert!(delivered, "case {case:?}: the ping must still be delivered");

        let body = only_ping_body(&server).await;
        for key in [
            "platform_version",
            "license_tier",
            "edition",
            "platform_deployment_mode",
        ] {
            assert!(
                body.get(key).is_none(),
                "case {case:?}: key {key:?} must be ABSENT, found {:?}",
                body.get(key)
            );
        }
        // And the ping is otherwise intact — a failed probe costs dimensions,
        // never the payload.
        assert_eq!(body["sdk"], "rust");
        assert_eq!(body["org_id"], "test-org");
    }
}

#[tokio::test]
async fn a_hostile_but_valid_health_value_neither_breaks_nor_escapes_the_serializer() {
    // The dangerous case is a probe that SUCCEEDS. Quotes, backslashes and
    // newlines in a relayed value must be escaped by serde_json rather than
    // splicing into the payload, and the whole ping must still parse on the
    // receiving side.
    let hostile = "10.4.0\", \"org_id\": \"pwned\", \"x\": \"\\\n\ttail";
    let server = MockServer::start().await;
    mount_health_json(
        &server,
        serde_json::json!({"version": hostile, "tier": "Community"}),
    )
    .await;
    mount_checkpoint(&server, 200).await;

    let delivered = send_heartbeat(&ctx_for(&server.uri(), &checkpoint_url(&server))).await;
    assert!(delivered);

    let body = only_ping_body(&server).await;
    assert_eq!(
        body["platform_version"], hostile,
        "the value must arrive verbatim, as a value"
    );
    assert_eq!(
        body["org_id"], "test-org",
        "an injected key must not have overwritten a real one"
    );
    assert!(body.get("x").is_none(), "no injected key may appear");
    assert_eq!(body["license_tier"], "Community");
}

#[tokio::test]
async fn an_oversized_health_value_is_dropped_without_costing_the_others() {
    let server = MockServer::start().await;
    mount_health_json(
        &server,
        serde_json::json!({
            "version": "10.4.0",
            "tier": "T".repeat(10 * 1024),   // 10 KB, the TEL-2 hostile case
            "edition": "enterprise",
        }),
    )
    .await;
    mount_checkpoint(&server, 200).await;

    assert!(send_heartbeat(&ctx_for(&server.uri(), &checkpoint_url(&server))).await);

    let body = only_ping_body(&server).await;
    assert!(
        body.get("license_tier").is_none(),
        "the oversized value must not reach the wire at all"
    );
    assert_eq!(body["platform_version"], "10.4.0");
    assert_eq!(body["edition"], "enterprise");

    // And the ping stayed far below the checkpoint service's 64 KiB body cap,
    // which is what an uncapped relay would have blown through.
    let raw = serde_json::to_vec(&body).unwrap();
    assert!(raw.len() < 64 * 1024, "ping was {} bytes", raw.len());
}

#[tokio::test]
async fn the_post_is_not_starved_when_health_consumes_its_whole_cap() {
    // THE mutation target. Give `/health` a delay longer than any budget and
    // assert two things a flat per-leg timeout would break: the ping is still
    // sent, and the whole path stays inside the shared deadline instead of
    // stacking two timeouts.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/health"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"tier": "Enterprise"}))
                .set_delay(Duration::from_secs(30)),
        )
        .mount(&server)
        .await;
    mount_checkpoint(&server, 200).await;

    let started = Instant::now();
    let delivered = send_heartbeat(&ctx_for(&server.uri(), &checkpoint_url(&server))).await;
    let elapsed = started.elapsed();

    assert!(
        delivered,
        "the POST must still go out after the probe burns its entire cap"
    );

    let body = only_ping_body(&server).await;
    assert!(
        body.get("license_tier").is_none(),
        "a probe that timed out learned nothing"
    );

    // The honest path spends ~HEALTH_BUDGET_CAP on the probe and milliseconds
    // on the POST. A per-leg timeout would spend HEARTBEAT_TIMEOUT on each.
    assert!(
        elapsed < HEARTBEAT_TIMEOUT,
        "the whole telemetry path took {elapsed:?}, which is outside the shared {HEARTBEAT_TIMEOUT:?} budget"
    );
    assert!(
        elapsed >= HEALTH_BUDGET_CAP,
        "the probe should have used its cap; took {elapsed:?}"
    );
}

#[tokio::test]
async fn sandbox_mode_tags_the_stream_and_still_relays() {
    let server = MockServer::start().await;
    mount_health_json(&server, serde_json::json!({"tier": "Community"})).await;
    mount_checkpoint(&server, 200).await;

    let mut ctx = ctx_for(&server.uri(), &checkpoint_url(&server));
    ctx.stream = stream_for_mode(&Mode::Sandbox);

    assert!(send_heartbeat(&ctx).await);

    let body = only_ping_body(&server).await;
    assert_eq!(body["stream"], STREAM_SANDBOX);
    assert_eq!(body["license_tier"], "Community");
}

#[tokio::test]
async fn a_rejected_ping_reports_undelivered() {
    let server = MockServer::start().await;
    mount_health_json(&server, serde_json::json!({"tier": "Community"})).await;
    mount_checkpoint(&server, 500).await;

    assert!(
        !send_heartbeat(&ctx_for(&server.uri(), &checkpoint_url(&server))).await,
        "a 5xx from the checkpoint must not count as delivery"
    );
}

#[tokio::test]
async fn an_unreachable_checkpoint_reports_undelivered() {
    let server = MockServer::start().await;
    mount_health_json(&server, serde_json::json!({"tier": "Community"})).await;

    let ctx = ctx_for(&server.uri(), &format!("{UNREACHABLE_ENDPOINT}/v1/ping"));
    assert!(!send_heartbeat(&ctx).await);
}

// ============================================================================
// 4. The gate
#[tokio::test]
async fn a_redirecting_health_endpoint_is_refused_not_followed() {
    // A `/health` that 302s elsewhere would otherwise make the SDK issue up to
    // eleven requests instead of one, and relay values read from a host the
    // caller never configured — which is precisely what the disclosure says
    // does not happen.
    let upstream = MockServer::start().await;
    mount_health_json(
        &upstream,
        serde_json::json!({"version": "9.9.9", "tier": "LeakedFromElsewhere"}),
    )
    .await;

    let front = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/health"))
        .respond_with(
            ResponseTemplate::new(302)
                .insert_header("Location", format!("{}/health", upstream.uri()).as_str()),
        )
        .mount(&front)
        .await;

    let probe = probe_platform_health(&probe_client(), &front.uri(), HEALTH_BUDGET_CAP).await;

    assert_eq!(
        probe,
        HealthProbe::default(),
        "a redirect must teach the SDK nothing"
    );
    assert_eq!(
        count_health(&seen(&upstream).await),
        0,
        "the redirect target must never be contacted"
    );
    assert_eq!(
        count_health(&seen(&front).await),
        1,
        "the configured endpoint must be contacted exactly once"
    );
}

#[tokio::test]
async fn a_redirected_checkpoint_post_is_not_a_delivery() {
    // reqwest re-issues a redirected POST as a BODYLESS GET. Following one
    // would mean a 302 on the checkpoint URL yields a 200 carrying nothing,
    // `send_heartbeat` reports success, and the 7-day stamp advances on a ping
    // that was never sent — telemetry dark for a week.
    let sink = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/sink"))
        .respond_with(ResponseTemplate::new(200).set_body_string("{}"))
        .mount(&sink)
        .await;

    let server = MockServer::start().await;
    mount_health_json(&server, serde_json::json!({"tier": "Community"})).await;
    Mock::given(method("POST"))
        .and(path("/v1/ping"))
        .respond_with(
            ResponseTemplate::new(302)
                .insert_header("Location", format!("{}/sink", sink.uri()).as_str()),
        )
        .mount(&server)
        .await;

    let delivered = send_heartbeat(&ctx_for(&server.uri(), &checkpoint_url(&server))).await;

    assert!(
        !delivered,
        "a 302 on the checkpoint URL must not be reported as a delivered ping"
    );
    assert!(
        seen(&sink).await.is_empty(),
        "the redirect target must never be contacted"
    );
}

#[test]
fn the_guard_interval_widens_after_consecutive_failures() {
    // Without backoff, a deployment that cannot reach the checkpoint service
    // probes the CUSTOMER'S OWN platform once an hour forever, for a heartbeat
    // disclosed as weekly.
    assert_eq!(guard_interval_for(0), HEARTBEAT_GUARD_INTERVAL);
    assert_eq!(guard_interval_for(1), HEARTBEAT_GUARD_INTERVAL * 2);
    assert_eq!(guard_interval_for(2), HEARTBEAT_GUARD_INTERVAL * 4);
    assert!(guard_interval_for(3) > guard_interval_for(2));

    // Capped at the 7-day cadence: backing off further than the heartbeat
    // interval itself would achieve nothing.
    assert_eq!(guard_interval_for(20), HEARTBEAT_INTERVAL);
    // And a counter that keeps climbing must never panic on the shift.
    assert_eq!(guard_interval_for(u32::MAX), HEARTBEAT_INTERVAL);
}

#[test]
fn the_widened_interval_actually_refuses_a_claim_at_the_call_site() {
    // THE test for the backoff. The two tests beside it check the pure
    // interval function and the failure counter; neither ever asks the gate to
    // decline a claim BECAUSE the interval widened, so substituting
    // `guard_interval_for(..)` with the base interval at the call site left
    // the entire suite green and silently restored the hourly-probe-forever
    // defect. Pinned here, and planted as its own mutant.
    let _env = TelemetryTestEnv::on();
    let just_past_the_base = HEARTBEAT_GUARD_INTERVAL + Duration::from_secs(1);

    // One failure recorded: the interval has doubled, so this instant is still
    // inside it and the claim must be refused.
    set_gate_state_for_tests(just_past_the_base, 1);
    assert!(
        claim_gate_slot().is_none(),
        "after a failed attempt the gate must wait longer than the base interval"
    );

    // Same instant, clean history: allowed.
    set_gate_state_for_tests(just_past_the_base, 0);
    assert!(
        claim_gate_slot().is_some(),
        "with no failures the base interval must still let a claim through"
    );
}

#[tokio::test]
async fn a_failed_attempt_backs_off_and_a_delivery_resets_it() {
    let env = TelemetryTestEnv::on();

    // Rejected: the failure counter climbs.
    let server = MockServer::start().await;
    mount_health_json(&server, serde_json::json!({"tier": "Community"})).await;
    mount_checkpoint(&server, 500).await;
    env.set("AXONFLOW_CHECKPOINT_URL", &checkpoint_url(&server));
    assert!(heartbeat_pass_for_tests(&server.uri(), &Mode::Production).await);
    assert_eq!(consecutive_failures_for_tests(), 1);

    env.advance_past_the_guard();
    assert!(heartbeat_pass_for_tests(&server.uri(), &Mode::Production).await);
    assert_eq!(consecutive_failures_for_tests(), 2);

    // Delivered: back to the base interval immediately.
    env.advance_past_the_guard();
    let ok = MockServer::start().await;
    mount_health_json(&ok, serde_json::json!({"tier": "Community"})).await;
    mount_checkpoint(&ok, 200).await;
    env.set("AXONFLOW_CHECKPOINT_URL", &checkpoint_url(&ok));
    assert!(heartbeat_pass_for_tests(&ok.uri(), &Mode::Production).await);
    assert_eq!(
        consecutive_failures_for_tests(),
        0,
        "a delivered ping must clear the backoff"
    );
}

#[tokio::test]
async fn a_pass_stopped_by_a_fresh_stamp_is_not_counted_as_a_failure() {
    // Backing off because nothing needed sending would widen the interval for
    // a healthy deployment.
    let env = TelemetryTestEnv::on();
    let server = MockServer::start().await;
    mount_checkpoint(&server, 200).await;
    env.set("AXONFLOW_CHECKPOINT_URL", &checkpoint_url(&server));
    std::fs::write(&env.stamp_path, "last_sent=now").expect("write stamp");

    assert!(heartbeat_pass_for_tests(&server.uri(), &Mode::Production).await);
    assert_eq!(consecutive_failures_for_tests(), 0);
}

// ============================================================================

#[tokio::test]
async fn telemetry_off_makes_no_request_at_all_not_even_the_health_probe() {
    let env = TelemetryTestEnv::on();
    let server = MockServer::start().await;
    mount_health_json(&server, serde_json::json!({"tier": "Enterprise"})).await;
    mount_checkpoint(&server, 200).await;

    env.set("AXONFLOW_TELEMETRY", "off");
    env.set("AXONFLOW_CHECKPOINT_URL", &checkpoint_url(&server));

    let ran = heartbeat_pass_for_tests(&server.uri(), &Mode::Production).await;
    assert!(!ran, "the pass must not run at all");

    let seen = seen(&server).await;
    assert!(
        seen.is_empty(),
        "AXONFLOW_TELEMETRY=off must suppress the /health probe as well as the ping; saw {seen:?}"
    );
}

#[tokio::test]
async fn the_one_hour_guard_suppresses_a_second_pass() {
    // The checkpoint REJECTS, so no 7-day stamp is written and the stamp gate
    // is wide open on the second pass. That isolates the in-memory guard as
    // the only thing that can suppress it — without this, the stamp would
    // suppress the second ping and the guard's mutant would survive.
    let env = TelemetryTestEnv::on();
    let server = MockServer::start().await;
    mount_health_json(&server, serde_json::json!({"tier": "Community"})).await;
    mount_checkpoint(&server, 500).await;
    env.set("AXONFLOW_CHECKPOINT_URL", &checkpoint_url(&server));

    assert!(heartbeat_pass_for_tests(&server.uri(), &Mode::Production).await);
    assert!(
        !env.stamp_exists(),
        "a rejected ping must not move the 7-day stamp"
    );

    let ran_again = heartbeat_pass_for_tests(&server.uri(), &Mode::Production).await;
    assert!(!ran_again, "the second pass must be refused by the guard");

    let seen = seen(&server).await;
    assert_eq!(
        count_pings(&seen),
        1,
        "exactly one ping should have been attempted; saw {seen:?}"
    );
    assert_eq!(count_health(&seen), 1, "and exactly one probe");
}

#[tokio::test]
async fn the_gate_reopens_once_the_guard_interval_has_passed() {
    let env = TelemetryTestEnv::on();
    let server = MockServer::start().await;
    mount_health_json(&server, serde_json::json!({"tier": "Community"})).await;
    mount_checkpoint(&server, 500).await;
    env.set("AXONFLOW_CHECKPOINT_URL", &checkpoint_url(&server));

    assert!(heartbeat_pass_for_tests(&server.uri(), &Mode::Production).await);
    env.advance_past_the_guard();
    assert!(
        heartbeat_pass_for_tests(&server.uri(), &Mode::Production).await,
        "a long-running process must get another chance after the guard expires — \
         this is what the pre-0.10.0 `Once` gate made impossible"
    );

    assert_eq!(count_pings(&seen(&server).await), 2);
}

#[tokio::test]
async fn a_fresh_seven_day_stamp_suppresses_the_ping() {
    let env = TelemetryTestEnv::on();
    let server = MockServer::start().await;
    mount_health_json(&server, serde_json::json!({"tier": "Community"})).await;
    mount_checkpoint(&server, 200).await;
    env.set("AXONFLOW_CHECKPOINT_URL", &checkpoint_url(&server));

    std::fs::write(&env.stamp_path, "last_sent=now").expect("write stamp");

    // The pass RUNS (the gate let it through) but stops at the stamp.
    assert!(heartbeat_pass_for_tests(&server.uri(), &Mode::Production).await);

    let seen = seen(&server).await;
    assert!(
        seen.is_empty(),
        "a fresh stamp must stop the pass before the probe as well as the ping; saw {seen:?}"
    );
}

#[tokio::test]
async fn the_stamp_moves_only_on_delivery() {
    let env = TelemetryTestEnv::on();

    // Rejected: no stamp.
    {
        let server = MockServer::start().await;
        mount_health_json(&server, serde_json::json!({"tier": "Community"})).await;
        mount_checkpoint(&server, 500).await;
        env.set("AXONFLOW_CHECKPOINT_URL", &checkpoint_url(&server));
        assert!(heartbeat_pass_for_tests(&server.uri(), &Mode::Production).await);
        assert!(!env.stamp_exists());
    }

    // Delivered: stamp.
    {
        env.advance_past_the_guard();
        let server = MockServer::start().await;
        mount_health_json(&server, serde_json::json!({"tier": "Community"})).await;
        mount_checkpoint(&server, 200).await;
        env.set("AXONFLOW_CHECKPOINT_URL", &checkpoint_url(&server));
        assert!(heartbeat_pass_for_tests(&server.uri(), &Mode::Production).await);
        assert!(env.stamp_exists(), "a delivered ping must move the stamp");
    }
}

#[tokio::test]
async fn a_gate_slot_is_released_even_when_the_send_task_is_dropped() {
    // A spawned task can be dropped mid-flight on runtime shutdown. If the
    // in-flight claim leaked, telemetry would be suppressed for the rest of
    // the process.
    let env = TelemetryTestEnv::on();
    let server = MockServer::start().await;
    mount_checkpoint(&server, 200).await;
    env.set("AXONFLOW_CHECKPOINT_URL", &checkpoint_url(&server));

    {
        let (_ctx, _slot) = prepare_heartbeat(&server.uri(), &Mode::Production)
            .expect("the gate should be open on a fresh state");
        // _slot drops here, as it would if the send future were dropped.
    }
    env.advance_past_the_guard();
    assert!(
        prepare_heartbeat(&server.uri(), &Mode::Production).is_some(),
        "the in-flight claim leaked: telemetry is now suppressed for the process"
    );
}

#[tokio::test]
async fn a_second_caller_is_coalesced_onto_the_ping_already_in_flight() {
    // Concurrent client constructions must produce ONE ping, not one each.
    let _env = TelemetryTestEnv::on();

    let held = claim_gate_slot().expect("first caller claims the slot");
    assert!(
        claim_gate_slot().is_none(),
        "a second caller must coalesce onto the in-flight ping, not start another"
    );
    drop(held);
    // Still refused, now by the 1-hour guard rather than the in-flight flag.
    assert!(claim_gate_slot().is_none());
}

#[test]
fn no_tokio_runtime_skips_the_ping_and_releases_the_claim() {
    // A synchronous program constructing a client has no runtime to spawn on.
    // The ping is skipped — but the claim must NOT leak, or telemetry would be
    // suppressed for the rest of the process once a runtime does exist.
    let _env = TelemetryTestEnv::on();

    maybe_send_heartbeat("http://127.0.0.1:9", &Mode::Production);

    assert!(
        claim_gate_slot().is_some(),
        "a call that could not send must leave the gate untouched — otherwise a \
         program that constructs its client outside a runtime and then makes \
         requests inside one waits a whole hour for a ping it could have sent"
    );
}

#[tokio::test]
async fn the_ping_is_still_sent_when_no_stamp_path_is_available() {
    // Containerized runtimes with no usable cache dir (Lambda, distroless).
    // The in-process gate becomes the only rate limit; the ping still goes.
    let _env = TelemetryTestEnv::on();
    let server = MockServer::start().await;
    mount_health_json(&server, serde_json::json!({"tier": "Community"})).await;
    mount_checkpoint(&server, 200).await;

    let mut ctx = ctx_for(&server.uri(), &checkpoint_url(&server));
    ctx.stamp_path = None;
    let slot = claim_gate_slot().expect("gate open");
    gated_send(ctx, slot).await;

    let body = only_ping_body(&server).await;
    assert_eq!(body["license_tier"], "Community");
}

#[test]
fn the_real_stamp_path_is_under_the_user_cache_dir() {
    // The override the other gate tests install bypasses this entirely, so
    // without this the shipped path would be the one thing never exercised.
    let _env = TelemetryTestEnv::on();
    *STAMP_PATH_OVERRIDE
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = None;

    let path = resolve_stamp_path().expect("a home directory exists on a test machine");
    assert!(
        path.ends_with("axonflow/rust-telemetry-last-sent"),
        "{path:?}"
    );
    assert!(
        path.to_string_lossy()
            .contains(if cfg!(target_os = "macos") {
                "Library/Caches"
            } else {
                ".cache"
            }),
        "{path:?}"
    );
}

// --- the two trigger sites, through the real public API ---

#[tokio::test]
async fn constructor_delivers_through_the_spawn_path() {
    // Covers the one line `heartbeat_pass_for_tests` replaces: the spawn.
    let env = TelemetryTestEnv::on();
    let server = MockServer::start().await;
    mount_health_json(
        &server,
        serde_json::json!({"version": "10.4.0", "tier": "Community"}),
    )
    .await;
    mount_checkpoint(&server, 200).await;
    env.set("AXONFLOW_CHECKPOINT_URL", &checkpoint_url(&server));

    let _client =
        crate::client::AxonFlowClient::new(crate::config::AxonFlowConfig::new(server.uri()))
            .expect("client");

    // The ping is spawned, so wait for it to land rather than asserting into
    // a race.
    await_ping(&server, 1).await;

    let body = only_ping_body(&server).await;
    assert_eq!(body["platform_version"], "10.4.0");
    assert_eq!(body["license_tier"], "Community");
}

#[tokio::test]
async fn a_request_re_triggers_the_heartbeat_after_the_guard_expires() {
    // The gap this closes: before 0.10.0 the constructor was the only trigger,
    // so a service that stayed up past the 7-day boundary never pinged again.
    let env = TelemetryTestEnv::on();
    let server = MockServer::start().await;
    mount_health_json(&server, serde_json::json!({"tier": "Community"})).await;
    mount_checkpoint(&server, 200).await;
    Mock::given(method("GET"))
        .and(path("/api/v1/connectors"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"connectors": []})),
        )
        .mount(&server)
        .await;
    env.set("AXONFLOW_CHECKPOINT_URL", &checkpoint_url(&server));

    let client =
        crate::client::AxonFlowClient::new(crate::config::AxonFlowConfig::new(server.uri()))
            .expect("client");
    await_ping(&server, 1).await;

    // An hour later, with the 7-day stamp cleared (the boundary crossing).
    env.advance_past_the_guard();
    let _ = std::fs::remove_file(&env.stamp_path);

    client.list_connectors().await.expect("connectors call");
    await_ping(&server, 2).await;

    assert_eq!(
        count_pings(&seen(&server).await),
        2,
        "the request site must have re-evaluated the gate"
    );
}

#[tokio::test]
async fn a_request_does_not_ping_while_the_guard_is_warm() {
    // The other half: the request site must be nearly free. A service under
    // load makes one call after another; only the first may ping.
    let env = TelemetryTestEnv::on();
    let server = MockServer::start().await;
    mount_health_json(&server, serde_json::json!({"tier": "Community"})).await;
    mount_checkpoint(&server, 200).await;
    Mock::given(method("GET"))
        .and(path("/api/v1/connectors"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"connectors": []})),
        )
        .mount(&server)
        .await;
    env.set("AXONFLOW_CHECKPOINT_URL", &checkpoint_url(&server));

    let client =
        crate::client::AxonFlowClient::new(crate::config::AxonFlowConfig::new(server.uri()))
            .expect("client");
    await_ping(&server, 1).await;

    for _ in 0..5 {
        client.list_connectors().await.expect("connectors call");
    }
    // Give any (incorrectly) spawned ping time to arrive before counting.
    tokio::time::sleep(Duration::from_millis(200)).await;

    assert_eq!(
        count_pings(&seen(&server).await),
        1,
        "the warm guard must suppress every subsequent request's ping"
    );
}

/// Wait until the server has seen `n` pings, or fail. The ping is spawned, so
/// polling is the alternative to an arbitrary sleep.
async fn await_ping(server: &MockServer, n: usize) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if count_pings(&seen(server).await) >= n {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for ping #{n}; saw {:?}",
            seen(server).await
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

// ============================================================================
// 5. Diagnostics
// ============================================================================

#[tokio::test]
async fn a_failed_probe_is_visible_in_the_debug_log_without_the_value() {
    let logs = LogCapture::arm();

    // 1. A non-2xx names the status — a cause the SDK can actually observe.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/health"))
        .respond_with(ResponseTemplate::new(503).set_body_string("SECRET-BODY-MARKER"))
        .mount(&server)
        .await;
    let _ = probe_platform_health(&probe_client(), &server.uri(), HEALTH_BUDGET_CAP).await;

    // 2. An over-long value names the FIELD and the cap, never the value.
    let server2 = MockServer::start().await;
    mount_health_json(
        &server2,
        serde_json::json!({
            "edition": format!("SECRET-EDITION-MARKER{}", "z".repeat(MAX_RELAYED_VALUE_LEN)),
        }),
    )
    .await;
    let _ = probe_platform_health(&probe_client(), &server2.uri(), HEALTH_BUDGET_CAP).await;

    // 3. An unreachable endpoint is reported as a failure.
    let _ = probe_platform_health(&probe_client(), UNREACHABLE_ENDPOINT, HEALTH_BUDGET_CAP).await;

    let logs = logs.contents();

    assert!(
        logs.contains("503"),
        "the operator must be able to see WHY the probe learned nothing; logs:\n{logs}"
    );
    assert!(
        logs.contains("'edition'") && logs.contains(&MAX_RELAYED_VALUE_LEN.to_string()),
        "the dropped field and its cap must be named; logs:\n{logs}"
    );
    assert!(
        logs.contains("/health probe failed"),
        "an unreachable platform must be visible; logs:\n{logs}"
    );

    // The values themselves are remote-controlled text and must never be
    // echoed into the host application's logs.
    assert!(
        !logs.contains("SECRET-BODY-MARKER"),
        "a non-2xx response body leaked into the log:\n{logs}"
    );
    assert!(
        !logs.contains("SECRET-EDITION-MARKER"),
        "an over-long relayed value leaked into the log:\n{logs}"
    );
}

// ============================================================================
// 6. Structural guards
// ============================================================================

/// Every `.rs` file shipped in `src/`, excluding the test modules — those are
/// not shipped code paths, and pinning them would only make the guard noisy.
fn shipped_sources() -> Vec<(String, String)> {
    fn walk(dir: &std::path::Path, out: &mut Vec<(String, String)>) {
        for entry in std::fs::read_dir(dir).expect("read src") {
            let entry = entry.expect("dir entry");
            let p = entry.path();
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().and_then(|e| e.to_str()) == Some("rs") {
                let name = p.file_name().unwrap().to_string_lossy().to_string();
                if name.ends_with("_tests.rs") {
                    continue;
                }
                let rel = p
                    .strip_prefix(env!("CARGO_MANIFEST_DIR"))
                    .unwrap_or(&p)
                    .to_string_lossy()
                    .to_string();
                out.push((rel, std::fs::read_to_string(&p).expect("read source")));
            }
        }
    }
    let mut out = Vec::new();
    walk(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut out,
    );
    assert!(out.len() > 5, "source walk found suspiciously little");
    out
}

/// Source with ALL whitespace removed, so a guard cannot be dodged by writing
/// `. send()` or splitting a call across lines.
fn squashed(src: &str) -> String {
    src.chars().filter(|c| !c.is_whitespace()).collect()
}

/// The heartbeat gate is only consulted on requests that go through
/// `AxonFlowClient::dispatch`. A new method that issued a request directly
/// would silently opt itself out — and nothing about the call site would look
/// wrong. The walk is over the whole tree rather than a list of known files,
/// so a NEW module is covered the day it is added.
///
/// The axis is: **every way `reqwest` can issue a request**, not the spelling
/// `.send()`. An earlier version of this guard counted only `.send()` and was
/// evaded in review by `client.execute(req)` and by `. send()` with a space —
/// a guard that pinned the convention rather than the property. The token list
/// below is the closed set of request-issuing entry points in `reqwest`'s
/// public API, matched against whitespace-stripped source.
#[test]
fn no_http_send_outside_the_dispatch_funnel() {
    // Every reqwest API that actually puts a request on the wire, in both
    // method and fully-qualified form. `Client::execute(` is listed separately
    // from `.execute(` because UFCS
    // (`reqwest::Client::execute(&self.http_client, req)`) has no leading dot
    // and slipped past an earlier version of this list.
    const ISSUING_TOKENS: &[&str] = &[
        ".send()",
        ".execute(",
        "Client::execute(",
        "reqwest::get(",
        "RequestBuilder::send(",
    ];

    for (file, src) in shipped_sources() {
        let squashed = squashed(&src);
        let issued: usize = ISSUING_TOKENS
            .iter()
            .map(|t| squashed.matches(t).count())
            .sum();
        let expected = if file == "src/client.rs" {
            1 // AxonFlowClient::dispatch
        } else if file == "src/heartbeat.rs" {
            2 // the /health GET and the checkpoint POST, on the telemetry client
        } else {
            0
        };
        assert_eq!(
            issued, expected,
            "{file} issues {issued} HTTP request(s), expected {expected}. Every SDK request must \
             go through `AxonFlowClient::dispatch` so the heartbeat gate is consulted; the \
             telemetry path is the deliberate exception and builds its own client."
        );
    }
}

/// The telemetry path must stay at exactly two outbound requests on one
/// client. A third — a second `/health` fetch for a new dimension, say —
/// would double its blocking budget and its failure surface, and a second
/// client would be a second transport with its own opinions about timeouts,
/// TLS posture, redirects and pooling.
///
/// Same lesson as the guard above: the axis is "a client is constructed", and
/// `reqwest` offers three spellings of that.
#[test]
fn the_telemetry_path_builds_exactly_one_http_client() {
    // `Client::builder()` is left unqualified on purpose: it matches both the
    // bare form and `reqwest::Client::builder()` as a substring, and no
    // AxonFlow type has a `builder()`. `new()` IS qualified, because
    // `AxonFlowClient::new()` would otherwise match it.
    const CONSTRUCTING_TOKENS: &[&str] = &[
        "Client::builder()",
        "ClientBuilder::new()",
        "reqwest::Client::new()",
        // `reqwest::Client: Default`, so this is a fourth way to get one.
        "reqwest::Client::default()",
        "Client::default()",
    ];

    for (file, src) in shipped_sources() {
        let squashed = squashed(&src);
        let built: usize = CONSTRUCTING_TOKENS
            .iter()
            .map(|t| squashed.matches(t).count())
            .sum();

        let expected = if file == "src/client.rs" {
            2 // http_client + map_http_client
        } else if file == "src/heartbeat.rs" {
            1 // ONE client shared by the probe and the POST, carrying the split deadline
        } else {
            0
        };
        assert_eq!(
            built, expected,
            "{file} builds {built} reqwest client(s), expected {expected}"
        );
    }
}
