//! SDK heartbeat — telemetry parity with the Go / Python /
//! TypeScript / Java SDKs.
//!
//! Sends at most one ping per machine per 7 days to
//! `https://checkpoint.getaxonflow.com/v1/ping` carrying SDK version,
//! OS, architecture, runtime version, deployment mode, and an
//! endpoint-type classification (never the raw URL — see issue #1525
//! in the AxonFlow tracker for the privacy rationale).
//!
//! # Network behaviour change in 0.10.0 (sdk-rust#88)
//!
//! Before 0.10.0 the telemetry path made exactly one outbound request: the
//! POST to the checkpoint service. It now makes a second one FIRST — a `GET`
//! on the configured platform endpoint's `/health` — and relays four values
//! from that response so Rust rows carry the same dimensions the other four
//! SDKs already carry. `/health` is unauthenticated and is the caller's own
//! platform, so this is a request to an endpoint the SDK was already
//! configured to talk to; it is nonetheless a change to the SDK's network
//! behaviour and is disclosed as such in `README.md`.
//!
//! `AXONFLOW_TELEMETRY=off` is the SOLE opt-out path, and it suppresses the
//! `/health` probe together with the ping — nothing on this path runs. There
//! is intentionally no programmatic disable on the SDK config: the single
//! env-var lever matches HashiCorp checkpoint, Docker, and the Datadog Agent.
//! Sandbox-mode clients tag their pings with `stream="sandbox"` so analytics
//! can distinguish dev/test usage from production heartbeat. `DO_NOT_TRACK` is
//! intentionally NOT honored — host CLIs commonly inherit it, which makes it
//! an unreliable expression of AxonFlow-scoped intent.
//!
//! Pre-v0.2 the Rust SDK pinged `{configured_endpoint}/api/telemetry/heartbeat`
//! against the local agent — useful for proxy debugging but invisible to
//! AxonFlow's central telemetry pipeline. The endpoint switch in v0.2
//! brings Rust into parity with the other 4 first-class SDKs.

use std::fs;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use serde::Serialize;
use tracing::debug;

use crate::config::Mode;

/// Bounds how often a single machine delivers a telemetry ping. Aligned with
/// the cross-SDK "at most one heartbeat per environment every 7 days during
/// SDK activity" contract, and enforced by the stamp file so it survives
/// process restarts.
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// Bounds how often a single PROCESS re-consults the stamp file. Without it,
/// every SDK request would `stat()` the stamp; with it, a hot service does so
/// at most once an hour. This is also what lets a long-running service that
/// crosses the 7-day boundary re-ping at all — before 0.10.0 a `Once` gate
/// made the constructor the only opportunity for the whole process lifetime.
const HEARTBEAT_GUARD_INTERVAL: Duration = Duration::from_secs(60 * 60);

/// Total budget for the ENTIRE telemetry path: `/health` probe plus checkpoint
/// POST. One shared deadline, not one timeout per leg — two independent 3 s
/// timeouts would stack into ~6 s of work against an unreachable endpoint
/// (enterprise#1693, the same defect the Python and TypeScript SDKs fixed).
const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(3);

/// Ceiling on the `/health` probe's share of [`HEARTBEAT_TIMEOUT`]. The probe
/// is the optional leg: it enriches the ping, the POST *is* the ping. Capping
/// it guarantees the POST always has room even when `/health` is blackholed.
const HEALTH_BUDGET_CAP: Duration = Duration::from_secs(1);

/// Minimum remaining budget worth spending on an HTTP request. Below this,
/// skip rather than issue a call that is near-certain to time out before it
/// achieves anything.
const MIN_BUDGET: Duration = Duration::from_millis(100);

/// Bounds how much of a `/health` response the probe will buffer. A real
/// response is a few KB, dominated by a `capabilities` map that grows every
/// release; 1 MiB is orders of magnitude above any legitimate body while
/// capping what a misbehaving or hostile endpoint can make the telemetry task
/// allocate. Exceeding it aborts the parse, which fails open exactly like
/// every other probe failure — the relayed fields stay absent and the ping is
/// still sent without them. Matches the Go SDK's `maxHealthBodyBytes`.
const MAX_HEALTH_BODY_BYTES: usize = 1024 * 1024;

/// Bounds the length of any single value relayed from `/health` onto the wire.
///
/// The values are supplied by whatever is answering at the configured
/// endpoint, and the checkpoint service rejects a request body over 64 KiB
/// with HTTP 413 — so an uncapped relay lets a `/health` response that
/// SUCCEEDS destroy the ping it was supposed to enrich, silently losing every
/// other dimension in the payload. Real values are single-digit-to-teens
/// bytes (`10.4.0`, `Enterprise`, `self_hosted`).
///
/// An over-long value is DROPPED WHOLE, never truncated: a truncated string
/// would be a claim the platform never made, and this field's entire contract
/// is that it relays verbatim or says nothing.
const MAX_RELAYED_VALUE_LEN: usize = 64;

const DEFAULT_CHECKPOINT_URL: &str = "https://checkpoint.getaxonflow.com/v1/ping";

/// Stream classifications written to the telemetry payload. Only the
/// SDK-derived heartbeat values are produced from this code path —
/// see `IsValidIncomingStream` server-side.
const STREAM_SANDBOX: &str = "sandbox";

/// Endpoint-type classifications for the SDK-derived `endpoint_type`
/// field on the telemetry payload. Mirrors Go SDK
/// `ClassifyEndpoint`. The raw URL never leaves the process.
const ENDPOINT_TYPE_LOCALHOST: &str = "localhost";
const ENDPOINT_TYPE_PRIVATE: &str = "private_network";
const ENDPOINT_TYPE_REMOTE: &str = "remote";
const ENDPOINT_TYPE_UNKNOWN: &str = "unknown";

// ============================================================================
// The process-wide gate
// ============================================================================

/// Process-wide heartbeat gate. The stamp file is the source of truth across
/// restarts; these fields gate within a process.
///
/// `last_checked` implements [`HEARTBEAT_GUARD_INTERVAL`]; `in_flight`
/// coalesces concurrent callers onto a single ping rather than one per caller.
struct GateInner {
    last_checked: Option<Instant>,
    in_flight: bool,
}

fn gate() -> &'static Mutex<GateInner> {
    static GATE: OnceLock<Mutex<GateInner>> = OnceLock::new();
    GATE.get_or_init(|| {
        Mutex::new(GateInner {
            last_checked: None,
            in_flight: false,
        })
    })
}

/// Claim on the in-flight slot. Releasing on `Drop` rather than at the end of
/// the send is deliberate: the send runs on a spawned task, and a task dropped
/// mid-flight (runtime shutdown, `JoinHandle` abort) would otherwise leave
/// `in_flight` stuck true and suppress telemetry for the rest of the process.
struct GateSlot;

impl Drop for GateSlot {
    fn drop(&mut self) {
        if let Ok(mut inner) = gate().lock() {
            inner.in_flight = false;
        }
    }
}

/// Synchronous half of the heartbeat decision, run on the CALLER's thread —
/// so it must stay cheap: one mutex acquire and two comparisons, no syscalls
/// and no allocation on the suppressed path.
///
/// This is what makes the request-site trigger affordable. A service handling
/// thousands of requests a second calls this on every one of them; if the
/// 1-hour guard is warm it returns here, having spawned nothing. The `stat()`
/// of the stamp file and all network work happen later, on a spawned task,
/// at most once per [`HEARTBEAT_GUARD_INTERVAL`].
///
/// Returns `None` when this call must not ping.
fn claim_gate_slot() -> Option<GateSlot> {
    let mut inner = gate().lock().ok()?;
    if inner.in_flight {
        return None;
    }
    if let Some(last) = inner.last_checked {
        if last.elapsed() < HEARTBEAT_GUARD_INTERVAL {
            return None;
        }
    }
    // Stamped BEFORE the send, not after: this bounds how often the gate RUNS,
    // which is a different question from whether a ping was DELIVERED. A
    // failed ping deliberately leaves the 7-day stamp untouched so the next
    // run after the guard expires retries.
    inner.last_checked = Some(Instant::now());
    inner.in_flight = true;
    Some(GateSlot)
}

/// Everything the heartbeat decides synchronously: the opt-out, the
/// process-wide gate, and the snapshot of the environment the ping will
/// describe. Returns `None` when this call must not ping.
///
/// Both the production entry point and the tests go through this function, so
/// no test can observe an ordering the shipped path does not have.
fn prepare_heartbeat(endpoint: &str, mode: &Mode) -> Option<(HeartbeatContext, GateSlot)> {
    if telemetry_off() {
        debug!("Telemetry disabled via AXONFLOW_TELEMETRY=off");
        return None;
    }
    let slot = claim_gate_slot()?;
    Some((HeartbeatContext::from_env(endpoint, mode), slot))
}

/// Fire-and-forget heartbeat. Called from `AxonFlowClient::new` and from
/// `AxonFlowClient::dispatch` — the single site every SDK HTTP request passes
/// through — so a long-running service stays visible instead of getting one
/// chance at construction.
///
/// Never blocks and never awaits on the caller's path: the gating decision is
/// a mutex acquire, and everything that can block (the stamp `stat()`, the
/// `/health` probe, the POST) runs on a spawned tokio task.
///
/// `AXONFLOW_TELEMETRY=off` short-circuits before any filesystem or network
/// access — including before the `/health` probe. Anything else allows the
/// ping; the per-machine 7-day stamp file (in `~/Library/Caches/axonflow/` on
/// macOS, `~/.cache/axonflow/` elsewhere) bounds the delivery cadence.
pub fn maybe_send_heartbeat(endpoint: &str, mode: &Mode) {
    // The runtime is checked BEFORE the gate is claimed, and the order is load
    // bearing. Claiming first would burn the hour-long slot on a call that
    // cannot send anything — so a program that constructs its client outside a
    // runtime and then makes requests inside one would go a whole hour with no
    // telemetry, for no reason.
    let handle = match tokio::runtime::Handle::try_current() {
        Ok(handle) => handle,
        Err(_) => {
            // Only claim this as the reason when it IS the reason: saying "no
            // runtime" to someone who set AXONFLOW_TELEMETRY=off would read as
            // the opt-out not having taken effect.
            if !telemetry_off() {
                debug!("Telemetry skipped: no tokio runtime in scope");
            }
            return;
        }
    };

    let Some((ctx, slot)) = prepare_heartbeat(endpoint, mode) else {
        return;
    };
    handle.spawn(gated_send(ctx, slot));
}

/// Asynchronous half: the 7-day stamp check and the send. Holds the gate slot
/// for its whole lifetime so concurrent callers coalesce onto this one run.
async fn gated_send(ctx: HeartbeatContext, _slot: GateSlot) {
    // resolve_stamp_path returns None on environments without a usable cache
    // dir — containerized runtimes with HOME unset, AWS Lambda, distroless
    // images. There the in-process gate is the only rate limit, matching the
    // Go / Python / TypeScript / Java SDKs, which also degrade to per-process
    // gating when their stamp path is unavailable.
    match ctx.stamp_path {
        Some(ref path) if stamp_is_fresh(path) => {
            debug!("Telemetry heartbeat is still fresh (<7 days)");
            return;
        }
        None => debug!("Telemetry stamp path unavailable; falling back to in-process gate"),
        _ => {}
    }

    if send_heartbeat(&ctx).await {
        write_stamp(&ctx).await;
    }
}

// Master switch for the crate's OWN test binary, off by default and armed
// PER THREAD.
//
// Every module's unit tests construct an `AxonFlowClient`, and construction
// consults the heartbeat gate. Without this switch two things go wrong during
// `cargo test`: unrelated tests fire real pings at the production checkpoint
// service (CI hides this by setting `AXONFLOW_TELEMETRY=off` for the whole
// workflow; a developer's `cargo test` does not), and — because the gate is
// process-wide by design — those constructions race the heartbeat tests for
// the in-flight claim, so an assertion about "exactly one ping" depends on
// which other test happened to be running.
//
// Thread-local rather than a global flag, and that distinction is load
// bearing: a global one is only off until the first heartbeat test turns it
// on, and every OTHER test constructing a client during that window can still
// claim the gate. Arming only the running test's thread closes the window
// completely. `prepare_heartbeat` — the only reader — always runs on the
// caller's thread, so this is consulted where it is armed.
//
// `TelemetryTestEnv` is the only thing that arms it, while holding the global
// test lock, and it disarms on drop.
#[cfg(test)]
thread_local! {
    static TELEMETRY_ARMED_FOR_TESTS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn telemetry_off() -> bool {
    #[cfg(test)]
    {
        if !TELEMETRY_ARMED_FOR_TESTS.with(|armed| armed.get()) {
            return true;
        }
    }
    std::env::var("AXONFLOW_TELEMETRY")
        .unwrap_or_default()
        .trim()
        .eq_ignore_ascii_case("off")
}

fn stamp_is_fresh(stamp_path: &PathBuf) -> bool {
    let metadata = match fs::metadata(stamp_path) {
        Ok(m) => m,
        Err(_) => return false,
    };
    let modified = match metadata.modified() {
        Ok(m) => m,
        Err(_) => return false,
    };
    let elapsed = match SystemTime::now().duration_since(modified) {
        Ok(e) => e,
        Err(_) => return false,
    };
    elapsed < HEARTBEAT_INTERVAL
}

/// Test-only redirection of the stamp file. Keeps the gate tests off the
/// developer's real `~/Library/Caches/axonflow/` stamp — which would otherwise
/// both suppress the tests and clobber a real heartbeat cadence — without
/// adding an environment override to the shipped surface.
#[cfg(test)]
static STAMP_PATH_OVERRIDE: Mutex<Option<PathBuf>> = Mutex::new(None);

fn resolve_stamp_path() -> Option<PathBuf> {
    #[cfg(test)]
    {
        if let Some(path) = STAMP_PATH_OVERRIDE.lock().unwrap().clone() {
            return Some(path);
        }
    }
    home::home_dir().map(|mut p| {
        #[cfg(target_os = "macos")]
        {
            p.push("Library");
            p.push("Caches");
        }
        #[cfg(not(target_os = "macos"))]
        {
            p.push(".cache");
        }
        p.push("axonflow");
        p.push("rust-telemetry-last-sent");
        p
    })
}

/// Classify the configured AxonFlow endpoint URL into one of
/// `localhost` / `private_network` / `remote` / `unknown`. The raw URL
/// is never sent — only the classification (see issue #1525). Mirrors
/// `ClassifyEndpoint` in the Go SDK.
fn classify_endpoint(endpoint: &str) -> &'static str {
    if endpoint.is_empty() {
        return ENDPOINT_TYPE_UNKNOWN;
    }
    let parsed = match url::Url::parse(endpoint) {
        Ok(u) => u,
        Err(_) => return ENDPOINT_TYPE_UNKNOWN,
    };
    let host = match parsed.host_str() {
        // url crate returns IPv6 host strings with surrounding brackets
        // (e.g. "[::1]") — strip them before IP-parsing so loopback
        // detection works for `http://[::1]:8080` and similar URLs.
        Some(h) => h
            .trim_start_matches('[')
            .trim_end_matches(']')
            .to_lowercase(),
        None => return ENDPOINT_TYPE_UNKNOWN,
    };

    if host == "localhost" || host == "0.0.0.0" || host.ends_with(".localhost") {
        return ENDPOINT_TYPE_LOCALHOST;
    }
    for suffix in &[".local", ".internal", ".lan", ".intranet"] {
        if host.ends_with(suffix) {
            return ENDPOINT_TYPE_PRIVATE;
        }
    }
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        if ip.is_loopback() {
            return ENDPOINT_TYPE_LOCALHOST;
        }
        if is_private_or_link_local(&ip) {
            return ENDPOINT_TYPE_PRIVATE;
        }
        return ENDPOINT_TYPE_REMOTE;
    }
    ENDPOINT_TYPE_REMOTE
}

fn is_private_or_link_local(ip: &std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => v4.is_private() || v4.is_link_local(),
        std::net::IpAddr::V6(v6) => {
            // ULA fc00::/7 + link-local fe80::/10. Stable methods aren't
            // available on stable Rust for IPv6 private detection, so
            // we hand-check the high bits.
            let segs = v6.segments();
            (segs[0] & 0xfe00) == 0xfc00 || (segs[0] & 0xffc0) == 0xfe80
        }
    }
}

// v1 telemetry-schema deployment_mode allowlist (axonflow-enterprise#2008).
// Reflects deployment topology only — the prior config.Mode-based
// production/sandbox split moved to the `stream` field.
pub const DEPLOYMENT_MODE_SELF_HOSTED: &str = "self_hosted";
pub const DEPLOYMENT_MODE_COMMUNITY_SAAS: &str = "community_saas";
pub const DEPLOYMENT_MODE_UNKNOWN: &str = "unknown";

/// Classify the configured AxonFlow endpoint into the v1 deployment-mode
/// allowlist (`self_hosted | community_saas | unknown`). Community-SaaS
/// detection fires on either an `*.try.getaxonflow.com` host or
/// `AXONFLOW_TRY=1` (the explicit override path for tenants behind a
/// custom hostname proxying try.getaxonflow.com). Empty/unparseable
/// endpoints resolve to `unknown`.
///
/// This is the SDK's own classification of the URL it was handed. It is a
/// different question from the platform's own `DEPLOYMENT_MODE`, which the
/// platform reports on `/health` and which rides the wire separately as
/// `platform_deployment_mode` — see [`HealthProbe`].
fn classify_deployment_mode(endpoint: &str) -> &'static str {
    if std::env::var("AXONFLOW_TRY").unwrap_or_default() == "1" {
        return DEPLOYMENT_MODE_COMMUNITY_SAAS;
    }
    if endpoint.is_empty() {
        return DEPLOYMENT_MODE_UNKNOWN;
    }
    let parsed = match url::Url::parse(endpoint) {
        Ok(u) => u,
        Err(_) => return DEPLOYMENT_MODE_UNKNOWN,
    };
    let host = match parsed.host_str() {
        Some(h) => h.to_lowercase(),
        None => return DEPLOYMENT_MODE_UNKNOWN,
    };
    if host == "try.getaxonflow.com" || host.ends_with(".try.getaxonflow.com") {
        return DEPLOYMENT_MODE_COMMUNITY_SAAS;
    }
    DEPLOYMENT_MODE_SELF_HOSTED
}

fn stream_for_mode(mode: &Mode) -> Option<&'static str> {
    match mode {
        Mode::Sandbox => Some(STREAM_SANDBOX),
        // Production: omit the field so the wire shape is byte-identical
        // to v0.1.x for production-mode clients (server defaults empty
        // to "heartbeat").
        Mode::Production => None,
    }
}

/// Sentinel emitted on the telemetry wire when `ORG_ID` is unset — the
/// default-config Community-mode developer case. See #2277.
pub const ORG_ID_LOCAL_DEV_SENTINEL: &str = "local-dev-org";

/// Returns the `org_id` value to emit on the next telemetry ping. Reads
/// `ORG_ID` from the environment (the operator's explicit configuration
/// for self-hosted deployments, or the `cs_<uuid>` tenant identifier on
/// Community SaaS) and falls back to [`ORG_ID_LOCAL_DEV_SENTINEL`] when
/// unset. Always returns a non-empty string. See #2277.
pub fn telemetry_org_id() -> String {
    match std::env::var("ORG_ID") {
        Ok(v) if !v.is_empty() => v,
        _ => ORG_ID_LOCAL_DEV_SENTINEL.to_string(),
    }
}

fn os_str() -> &'static str {
    std::env::consts::OS
}

fn arch_str() -> &'static str {
    std::env::consts::ARCH
}

/// Value reported when the toolchain could not be established at build time.
/// Distinct from any real rustc string, and honest: the field says "not
/// known" rather than naming a channel the build may not have used.
const RUNTIME_VERSION_UNKNOWN: &str = "unknown";

/// Normalise the verbatim `rustc --version` line `build.rs` captured into the
/// low-cardinality `rustc <version>` shape the telemetry warehouse aggregates
/// on — matching the Go SDK's `go1.22` and the Python SDK's `python 3.12.1`.
///
/// `rustc --version` prints `rustc 1.95.0 (59807616e 2026-04-14)`. The commit
/// hash and build date are dropped: they are a per-toolchain-build identifier
/// that would explode the dimension's cardinality without answering the
/// question the field exists for (which Rust versions must the SDK support).
///
/// Anything that does not have a recognisable version token resolves to
/// [`RUNTIME_VERSION_UNKNOWN`] rather than being passed through, so a wrapper
/// that prints something unexpected cannot put arbitrary text on the wire.
fn normalize_rustc_version(raw: Option<&str>) -> String {
    let raw = raw.unwrap_or_default().trim();
    let mut parts = raw.split_whitespace();
    let (Some("rustc"), Some(version)) = (parts.next(), parts.next()) else {
        return RUNTIME_VERSION_UNKNOWN.to_string();
    };
    // A version token is `1.95.0`, `1.96.0-nightly`, `1.95.0-beta.2`. Reject
    // anything that does not start with a digit, and bound the length so a
    // hostile wrapper cannot widen the field.
    if !version.starts_with(|c: char| c.is_ascii_digit()) || version.len() > MAX_RELAYED_VALUE_LEN {
        return RUNTIME_VERSION_UNKNOWN.to_string();
    }
    format!("rustc {version}")
}

/// The compiling toolchain, captured by `build.rs`. `None` when the build
/// script could not run `rustc --version`, which reports `unknown`.
fn runtime_version_str() -> String {
    normalize_rustc_version(option_env!("AXONFLOW_RUSTC_VERSION"))
}

fn instance_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

// ============================================================================
// The /health probe
// ============================================================================

/// What a single `/health` fetch established.
///
/// Every field is INDEPENDENT: a response carrying one but not another yields
/// a partially-populated probe rather than discarding all of them. `None`
/// means NOT LEARNED and is omitted from the wire entirely — it never degrades
/// to a default, an empty string, or a JSON `null`.
///
/// # Trust boundary
///
/// These values are whatever is answering at the endpoint the caller
/// configured. The SDK derives nothing from them, verifies nothing about them,
/// and the receiver cannot verify the relay either. They are adoption
/// analytics; they must never gate entitlement, unlock a feature, or enter an
/// authorization or billing decision.
#[derive(Debug, Default, PartialEq, Eq)]
struct HealthProbe {
    /// `/health` → `version`. The platform build the SDK is talking to.
    platform_version: Option<String>,
    /// `/health` → `tier`. Forwarded verbatim, INCLUDING the transient
    /// `starting` an agent returns before its licence is validated: that is a
    /// real signal the receiver buckets deliberately, not an error to filter
    /// client-side. Casing and alias folding are the receiver's job
    /// (`NormalizeLicenseTier`) so a tier this SDK build predates still
    /// arrives intact.
    license_tier: Option<String>,
    /// `/health` → `edition`. Added platform-side by enterprise#3660; absent
    /// against any platform that predates it, which is exactly what
    /// "omitted when not learned" already handles.
    edition: Option<String>,
    /// `/health` → `deployment_mode`, relayed as `platform_deployment_mode`.
    /// The PLATFORM's own deployment mode, which is a different question from
    /// the SDK's `deployment_mode` classification of the endpoint URL. The
    /// two travel under different names so neither overwrites the other.
    deployment_mode: Option<String>,
}

/// Promote one `/health` member to a relayable value.
///
/// Learned only when the member is present, is a JSON string, is non-empty,
/// and is within [`MAX_RELAYED_VALUE_LEN`]. An absent key, a non-string value,
/// an explicit `""`, and an over-long string are all NOT LEARNED — the field
/// stays `None` rather than becoming a value the platform did not report.
fn learned_value(body: &serde_json::Value, key: &str) -> Option<String> {
    let raw = body.get(key)?.as_str()?;
    if raw.is_empty() {
        return None;
    }
    if raw.len() > MAX_RELAYED_VALUE_LEN {
        // The VALUE is deliberately not logged: it is remote-controlled text,
        // and the diagnostic exists to say which field was dropped and why.
        debug!(
            "Telemetry: /health field '{}' exceeded {} bytes ({} bytes); omitted",
            key,
            MAX_RELAYED_VALUE_LEN,
            raw.len()
        );
        return None;
    }
    Some(raw.to_string())
}

/// Probe the configured platform's `/health` endpoint ONCE and extract every
/// telemetry dimension it carries.
///
/// Returns a default (all fields `None`) on ANY failure — no endpoint,
/// unreachable, non-2xx, oversized body, unparseable body — so telemetry
/// degrades to omitting the fields. It never fails the ping and never surfaces
/// an error to the caller.
///
/// This is the SDK's ONLY `/health` fetch on the telemetry path. Every relayed
/// dimension rides this one response. A second request here would double the
/// path's blocking budget and its failure surface — do not add one.
///
/// `budget` comes from the shared deadline, so this leg and the POST cannot
/// stack into a larger combined wait.
async fn probe_platform_health(
    client: &reqwest::Client,
    endpoint: &str,
    budget: Duration,
) -> HealthProbe {
    let endpoint = endpoint.trim_end_matches('/');
    if endpoint.is_empty() {
        return HealthProbe::default();
    }

    let mut resp = match client
        .get(format!("{endpoint}/health"))
        .timeout(budget)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            // `without_url` keeps the configured endpoint out of the log line;
            // the URL never leaves the process either way, but the diagnostic
            // has no need for it.
            debug!("Telemetry: /health probe failed: {}", e.without_url());
            return HealthProbe::default();
        }
    };

    if !resp.status().is_success() {
        debug!("Telemetry: /health returned {}", resp.status());
        return HealthProbe::default();
    }

    // Read incrementally against a cap rather than buffering whatever the
    // endpoint chooses to send. `resp.json()` and `resp.bytes()` both buffer
    // the whole body first, so neither can express this bound.
    let mut buf: Vec<u8> = Vec::new();
    loop {
        match resp.chunk().await {
            Ok(Some(chunk)) => {
                if buf.len() + chunk.len() > MAX_HEALTH_BODY_BYTES {
                    debug!(
                        "Telemetry: /health body exceeded {} bytes; relayed fields omitted",
                        MAX_HEALTH_BODY_BYTES
                    );
                    return HealthProbe::default();
                }
                buf.extend_from_slice(&chunk);
            }
            Ok(None) => break,
            Err(e) => {
                debug!("Telemetry: /health body read failed: {}", e.without_url());
                return HealthProbe::default();
            }
        }
    }

    // Decoded into a generic JSON value rather than a typed struct ON PURPOSE.
    // With a struct carrying every member, ONE badly-typed member fails the
    // WHOLE decode — so a platform answering {"version":"10.4.0","tier":42}
    // would drop `platform_version`, a field that worked before the tier was
    // added. A new dimension must never be able to regress an existing one.
    // Matches the Go, Python, TypeScript and Java SDKs, which all type-check
    // each member individually.
    let body: serde_json::Value = match serde_json::from_slice(&buf) {
        Ok(v) => v,
        Err(e) => {
            debug!("Telemetry: /health body was not JSON: {}", e);
            return HealthProbe::default();
        }
    };

    HealthProbe {
        platform_version: learned_value(&body, "version"),
        license_tier: learned_value(&body, "tier"),
        edition: learned_value(&body, "edition"),
        deployment_mode: learned_value(&body, "deployment_mode"),
    }
}

// ============================================================================
// The payload
// ============================================================================

/// The telemetry wire payload.
///
/// A typed struct rather than a hand-built map so "omitted when not learned"
/// is structural: every relayed field is an `Option` with
/// `skip_serializing_if`, and there is no code path that can put a `null` or a
/// substituted default on the wire for one. Every value is handed to
/// `serde_json` AS A VALUE — nothing is spliced into a JSON fragment — so
/// quotes, backslashes and newlines in a `/health` response are escaped by the
/// serializer rather than breaking it.
#[derive(Debug, Serialize)]
struct TelemetryPayload {
    /// v1 telemetry-schema discriminator — always `sdk` for this crate.
    telemetry_type: &'static str,
    sdk: &'static str,
    sdk_version: &'static str,
    os: &'static str,
    arch: &'static str,
    runtime_version: String,
    /// The SDK's classification of its configured endpoint URL.
    deployment_mode: &'static str,
    endpoint_type: &'static str,
    /// Always empty for this SDK; the field is a plugin dimension. Emitted as
    /// `[]` rather than omitted to keep the wire shape stable.
    features: Vec<&'static str>,
    instance_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<&'static str>,
    /// v9.1 deployment-organization identifier (#2277). Always emitted.
    org_id: String,

    // --- relayed verbatim from /health, absent when not learned (#88) ---
    #[serde(skip_serializing_if = "Option::is_none")]
    platform_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    license_tier: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    edition: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    platform_deployment_mode: Option<String>,
}

/// Immutable snapshot of everything the ping describes, taken once on the
/// caller's thread so the spawned send never re-reads process environment that
/// may have changed underneath it.
struct HeartbeatContext {
    endpoint: String,
    checkpoint_url: String,
    stamp_path: Option<PathBuf>,
    stream: Option<&'static str>,
    deployment_mode: &'static str,
    endpoint_type: &'static str,
    org_id: String,
}

impl HeartbeatContext {
    fn from_env(endpoint: &str, mode: &Mode) -> Self {
        Self {
            endpoint: endpoint.to_string(),
            checkpoint_url: std::env::var("AXONFLOW_CHECKPOINT_URL")
                .unwrap_or_else(|_| DEFAULT_CHECKPOINT_URL.to_string()),
            stamp_path: resolve_stamp_path(),
            stream: stream_for_mode(mode),
            deployment_mode: classify_deployment_mode(endpoint),
            endpoint_type: classify_endpoint(endpoint),
            org_id: telemetry_org_id(),
        }
    }

    fn payload(&self, probe: HealthProbe) -> TelemetryPayload {
        TelemetryPayload {
            telemetry_type: "sdk",
            sdk: "rust",
            sdk_version: env!("CARGO_PKG_VERSION"),
            os: os_str(),
            arch: arch_str(),
            runtime_version: runtime_version_str(),
            deployment_mode: self.deployment_mode,
            endpoint_type: self.endpoint_type,
            features: Vec::new(),
            instance_id: instance_id(),
            stream: self.stream,
            org_id: self.org_id.clone(),
            platform_version: probe.platform_version,
            license_tier: probe.license_tier,
            edition: probe.edition,
            platform_deployment_mode: probe.deployment_mode,
        }
    }
}

/// Run the telemetry path: probe `/health`, then POST the ping. Returns
/// whether the ping was DELIVERED, which is what licenses the caller to move
/// the 7-day stamp forward.
///
/// # The shared budget
///
/// One deadline covers both legs. The probe gets at most
/// [`HEALTH_BUDGET_CAP`]; the POST gets everything left. This is the whole
/// reason there is a single [`reqwest::Client`] here with NO client-level
/// timeout: a client-level timeout is per-request and would let the two legs
/// stack, which is the defect the other SDKs already fixed.
///
/// The POST is attempted regardless of what the probe did — an unreachable,
/// broken or hostile `/health` costs the ping some of its dimensions, never
/// the ping itself.
async fn send_heartbeat(ctx: &HeartbeatContext) -> bool {
    let deadline = Instant::now() + HEARTBEAT_TIMEOUT;

    // ONE client for both legs. Deliberately built with no `.timeout(...)`:
    // the budget is per-request, set from the shared deadline below.
    let client = match reqwest::Client::builder().build() {
        Ok(c) => c,
        Err(e) => {
            debug!("Telemetry skipped: client build failed: {}", e);
            return false;
        }
    };

    let health_budget = remaining(deadline).min(HEALTH_BUDGET_CAP);
    let probe = if health_budget > MIN_BUDGET {
        probe_platform_health(&client, &ctx.endpoint, health_budget).await
    } else {
        HealthProbe::default()
    };

    let payload = ctx.payload(probe);

    debug!(
        "[AxonFlow] Telemetry enabled. Opt out: AXONFLOW_TELEMETRY=off | https://docs.getaxonflow.com/docs/telemetry"
    );

    let post_budget = remaining(deadline);
    if post_budget < MIN_BUDGET {
        // Unreachable while HEALTH_BUDGET_CAP + MIN_BUDGET < HEARTBEAT_TIMEOUT
        // — an invariant `budget_split_leaves_room_for_the_post` pins on the
        // constants. Kept because it is the property under test: give the
        // probe the whole budget and this branch is what drops the ping.
        debug!("Telemetry skipped: budget exhausted before the checkpoint POST");
        return false;
    }

    match client
        .post(&ctx.checkpoint_url)
        .timeout(post_budget)
        .json(&payload)
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => {
            debug!("Telemetry heartbeat delivered");
            true
        }
        Ok(resp) => {
            debug!("Telemetry heartbeat rejected by server: {}", resp.status());
            false
        }
        Err(e) => {
            debug!("Telemetry heartbeat failed: {}", e.without_url());
            false
        }
    }
}

/// Budget left before `deadline`, saturating at zero rather than panicking on
/// an already-passed instant.
fn remaining(deadline: Instant) -> Duration {
    deadline.saturating_duration_since(Instant::now())
}

/// Stamp-on-delivery: the stamp only moves when we know the ping landed. A
/// failed ping leaves it untouched so the next run after the in-process guard
/// expires retries. When `stamp_path` is `None` (containerized environments
/// with no usable cache dir) nothing is persisted and the in-process gate is
/// the only rate limit.
async fn write_stamp(ctx: &HeartbeatContext) {
    let Some(stamp_path) = ctx.stamp_path.as_ref() else {
        return;
    };
    if let Some(parent) = stamp_path.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }
    let _ = tokio::fs::write(
        stamp_path,
        format!("last_sent={}", chrono::Utc::now().to_rfc3339()),
    )
    .await;
}

/// Clear the process-wide gate so an individual test starts from a known
/// state. Tests hold `telemetry_lock()` while doing this — the gate is
/// process-wide by design, so two tests racing on it would see each other's
/// claims.
#[cfg(test)]
fn reset_gate_for_tests() {
    let mut inner = gate().lock().unwrap_or_else(|e| e.into_inner());
    inner.last_checked = None;
    inner.in_flight = false;
}

/// One complete heartbeat pass, awaited rather than spawned.
///
/// Deliberately NOT a re-implementation of the shipped sequence: it calls the
/// same [`prepare_heartbeat`] and [`gated_send`] in the same order that
/// [`maybe_send_heartbeat`] does. The only thing it replaces is the spawn, so
/// a test cannot pass against an ordering the shipped path does not have.
/// The spawn itself is covered separately, through the real public entry
/// point, by `constructor_delivers_through_the_spawn_path`.
///
/// Returns whether the pass ran at all (i.e. whether the gate let it through).
#[cfg(test)]
async fn heartbeat_pass_for_tests(endpoint: &str, mode: &Mode) -> bool {
    match prepare_heartbeat(endpoint, mode) {
        Some((ctx, slot)) => {
            gated_send(ctx, slot).await;
            true
        }
        None => false,
    }
}

#[cfg(test)]
#[path = "heartbeat_tests.rs"]
mod tests;
