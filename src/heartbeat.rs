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

/// Bounds on the `features` array, mirroring the receiver's own `MaxFeatures` /
/// `MaxFeatureBytes`. Applying them client-side means an over-long array is
/// shaped HERE, where the SDK still knows what it dropped, rather than silently
/// at ingest.
///
/// READ WHAT THESE TWO ACTUALLY REACH. The entry cap is live: register 33
/// adapters and the 33rd does not reach the wire. The byte cap is a BACKSTOP
/// that today's only producer cannot trigger — [`register_adapter`] already
/// refuses a name over [`MAX_RELAYED_VALUE_LEN`], so the longest entry it can
/// emit is `"adapter:".len() + 64 == 72` bytes. It is tested directly on
/// [`bound_features`], because a test driven through the registry could not
/// express it.
const MAX_FEATURES: usize = 32;
const MAX_FEATURE_BYTES: usize = 128;

/// Marks a `features[]` entry as an adapter identifier. The vocabulary is
/// SERVER-DEFINED (checkpoint-service `FeatureAdapterPrefix`) and is not this
/// SDK's to extend.
const FEATURE_ADAPTER_PREFIX: &str = "adapter:";

/// Adapter names declared by [`register_adapter`].
///
/// A set, so a framework that registers on every wrapper construction — the
/// ordinary case for an adapter whose constructor runs per request — declares
/// itself once on the wire rather than N times.
fn adapter_registry() -> &'static Mutex<std::collections::BTreeSet<String>> {
    static REGISTRY: OnceLock<Mutex<std::collections::BTreeSet<String>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(std::collections::BTreeSet::new()))
}

/// Declare that a framework adapter is driving this SDK, so the next telemetry
/// heartbeat carries `adapter:<name>` in its `features` array.
///
/// A framework adapter (LangChain, LangGraph, LiteLLM, …) wrapping this SDK is
/// indistinguishable from bare SDK use on every other telemetry dimension —
/// same `sdk`, same `sdk_version`, same endpoint. This is the one call that
/// makes the difference visible, and it is adoption signal only.
///
/// # It adds no request
///
/// The name rides the `features` array of the heartbeat that already fires;
/// there is no second ping, no second endpoint and no new configuration
/// surface. Calling this does not itself send anything.
///
/// Call it before your first API call for day-one attribution: the heartbeat
/// fires on the client's FIRST OUTBOUND REQUEST, not at construction, so a name
/// registered afterwards rides the next heartbeat.
///
/// Idempotent and safe from any thread.
///
/// # The name is not validated against a list, deliberately
///
/// The canonical vocabulary lives on the receiver (checkpoint-service
/// `NormalizeAdapterFeature`, which folds an unrecognised name into
/// `adapter:unknown` at READ time while keeping the raw name on the row). An
/// allowlist here would be a second vocabulary that drifts from the first: a
/// name this SDK build predates would be dropped at the client instead of
/// arriving and rendering as "someone is using an adapter we do not know
/// about" — precisely the signal the unknown bucket exists to preserve.
///
/// So the only transformations are the two the receiver also applies before
/// matching: trim, and lowercase. A name empty after trimming, and a name
/// longer than [`MAX_RELAYED_VALUE_LEN`], are refused SILENTLY — this is a
/// fire-and-forget telemetry declaration on a path whose overriding constraint
/// is that it never disrupts the caller.
///
/// # This SDK ships no adapter of its own
///
/// Unlike the Go, Python, TypeScript and Java SDKs, this crate exports no
/// framework adapter, so nothing here calls this function. It exists for
/// third-party integrations built on top of the crate. The `interceptors`
/// module wraps LLM PROVIDER clients (Anthropic, OpenAI), which is a different
/// dimension from the agent framework driving the SDK and deliberately not
/// reported here.
pub fn register_adapter(name: &str) {
    let normalized = name.trim().to_lowercase();
    if normalized.is_empty() || normalized.len() > MAX_RELAYED_VALUE_LEN {
        return;
    }
    // Poison-recovering like every other lock in this module: a panic elsewhere
    // must not make registration a permanent no-op.
    adapter_registry()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(normalized);
}

/// Apply the receiver's array bounds: at most [`MAX_FEATURES`] entries, none
/// over [`MAX_FEATURE_BYTES`] bytes.
///
/// An over-long entry is DROPPED rather than truncated, deliberately differing
/// from the receiver's own `BoundFeatures`. The receiver truncates because it is
/// defending storage against arbitrary clients; here the entry is something this
/// process declared about itself, and a truncated adapter name is a name nothing
/// is running.
fn bound_features(features: Vec<String>) -> Vec<String> {
    features
        .into_iter()
        .filter(|f| f.len() <= MAX_FEATURE_BYTES)
        .take(MAX_FEATURES)
        .collect()
}

/// Render the registry as the `features` array for one ping.
///
/// A `BTreeSet` keeps it sorted, so the wire is deterministic and "which 32
/// survive" is a defined answer rather than a hash-iteration accident.
fn registered_features() -> Vec<String> {
    let names: Vec<String> = adapter_registry()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .cloned()
        .collect();
    bound_features(
        names
            .into_iter()
            .map(|n| format!("{FEATURE_ADAPTER_PREFIX}{n}"))
            .collect(),
    )
}

/// Test-only: empty the registry and return what was there.
#[cfg(test)]
fn reset_adapter_registry_for_tests() -> std::collections::BTreeSet<String> {
    let mut guard = adapter_registry().lock().unwrap_or_else(|e| e.into_inner());
    std::mem::take(&mut *guard)
}

/// Test-only: restore a registry saved by [`reset_adapter_registry_for_tests`].
#[cfg(test)]
fn restore_adapter_registry_for_tests(previous: std::collections::BTreeSet<String>) {
    *adapter_registry().lock().unwrap_or_else(|e| e.into_inner()) = previous;
}

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
    /// Consecutive undelivered attempts. Widens the re-check interval so a
    /// deployment that can never reach the checkpoint service stops probing
    /// its own platform every hour forever. Reset on delivery.
    consecutive_failures: u32,
    /// When this process last DELIVERED a ping.
    ///
    /// The stamp file is the cross-restart record of that, but it is not
    /// always available: `resolve_stamp_path` returns `None` where there is no
    /// usable cache dir (HOME unset — distroless and scratch containers,
    /// Lambda custom runtimes), and `write_stamp` silently fails on a
    /// read-only root filesystem (`readOnlyRootFilesystem: true` is ordinary
    /// Kubernetes hardening). In both, `stamp_is_fresh` is false forever.
    ///
    /// Before 0.10.0 that was bounded by the `Once` gate at one ping per
    /// PROCESS. Replacing `Once` with the 1-hour guard removed that bound, and
    /// a SUCCESSFUL ping would then recur every hour indefinitely — 168x the
    /// "at most one ping per machine every 7 days" this SDK discloses, in
    /// exactly the environments least able to notice. The failure backoff
    /// cannot help: it resets on delivery, and these deliveries succeed.
    ///
    /// So the cadence is enforced in memory too. Redundant whenever the stamp
    /// works, and the only bound when it does not.
    last_delivered: Option<Instant>,
}

fn gate() -> &'static Mutex<GateInner> {
    static GATE: OnceLock<Mutex<GateInner>> = OnceLock::new();
    GATE.get_or_init(|| {
        Mutex::new(GateInner {
            last_checked: None,
            in_flight: false,
            consecutive_failures: 0,
            last_delivered: None,
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
        // Poisoning is recovered from, never swallowed. Dropping the release
        // because some other caller panicked would pin `in_flight` true and
        // silently disable telemetry for the rest of the process — a
        // fail-CLOSED outcome from an error path that has nothing to do with
        // telemetry.
        let mut inner = gate().lock().unwrap_or_else(|e| e.into_inner());
        inner.in_flight = false;
    }
}

/// Synchronous half of the heartbeat decision, run on the CALLER's thread —
/// so it must stay cheap: one mutex acquire and two comparisons, no syscalls
/// and no allocation on the suppressed path.
///
/// This is what makes the request-site trigger affordable. A service handling
/// thousands of requests a second calls this on every one of them; if the
/// 1-hour guard is warm it returns here, having spawned nothing. The `stat()`
/// of the stamp file and all network work happen later — awaited inline on the
/// request path (see [`maybe_send_heartbeat_on_request`]) or on a spawned task
/// via [`maybe_send_heartbeat`] — at most once per
/// [`HEARTBEAT_GUARD_INTERVAL`].
///
/// Returns `None` when this call must not ping.
fn claim_gate_slot() -> Option<GateSlot> {
    // See `GateSlot::drop`: a poisoned mutex must not become a permanent
    // telemetry outage.
    let mut inner = gate().lock().unwrap_or_else(|e| e.into_inner());
    if inner.in_flight {
        return None;
    }
    if let Some(last) = inner.last_checked {
        if last.elapsed() < guard_interval_for(inner.consecutive_failures) {
            return None;
        }
    }
    // The 7-day cadence, enforced in memory rather than only by the stamp
    // file. See `GateInner::last_delivered`: where the stamp cannot be
    // persisted this is the only thing standing between a delivered ping and
    // an hourly one.
    if let Some(delivered) = inner.last_delivered {
        if delivered.elapsed() < HEARTBEAT_INTERVAL {
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

/// How long the gate waits before re-consulting, given how many attempts in a
/// row have failed to deliver.
///
/// Doubling from [`HEARTBEAT_GUARD_INTERVAL`], capped at
/// [`HEARTBEAT_INTERVAL`]. Without this the SDK has no backoff at all, and
/// two deliberate design choices combine into a defect: the 7-day stamp only
/// advances on DELIVERY, and the gate is now re-evaluated on every request.
/// In a deployment where egress to the checkpoint service is blocked — which
/// is the normal state of the air-gapped and in-VPC self-hosted topologies
/// this SDK supports — every process would issue a `/health` GET against the
/// CUSTOMER'S OWN platform once an hour, indefinitely, and a failed POST
/// beside it. Unsolicited hourly traffic against someone else's platform, for
/// a heartbeat disclosed as weekly, is not defensible.
///
/// Backing off does not lose a ping: the stamp is still untouched, so the
/// first attempt after the widened interval sends normally.
fn guard_interval_for(consecutive_failures: u32) -> Duration {
    // Clamped before shifting: the counter is unbounded, and shifting a u32
    // by 32 or more panics in debug and is undefined in release. 16 doublings
    // already exceeds the 7-day cap by orders of magnitude.
    let doublings = consecutive_failures.min(16);
    HEARTBEAT_GUARD_INTERVAL
        .saturating_mul(1u32 << doublings)
        .min(HEARTBEAT_INTERVAL)
}

/// Record what an attempt achieved, so the next one can back off.
///
/// Only called when an attempt was actually MADE. A pass that stopped at the
/// fresh 7-day stamp is not a failure and must not widen the interval.
fn record_attempt(delivered: bool) {
    let mut inner = gate().lock().unwrap_or_else(|e| e.into_inner());
    if delivered {
        inner.consecutive_failures = 0;
        inner.last_delivered = Some(Instant::now());
    } else {
        inner.consecutive_failures = inner.consecutive_failures.saturating_add(1);
    }
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
/// **This is NOT what the request path uses.** A spawned send is dropped when
/// the process does not outlive it — measured at 1 delivery in 12 for a
/// compiled one-call binary — so the client's first request awaits the send
/// inline via [`maybe_send_heartbeat_on_request`]. This entry point remains for
/// callers that cannot await.
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

/// The heartbeat trigger for the REQUEST path, awaited inline on the caller's
/// task rather than spawned.
///
/// # Why this is not `maybe_send_heartbeat`
///
/// Spawning drops the ping when the process does not outlive it. Measured on a
/// compiled one-call binary that returns from `main`: the ping was delivered
/// **1 time in 12** — and worse than "no telemetry", the `/health` GET reached
/// the customer's own platform every time while the checkpoint POST was
/// cancelled, so the SDK made an unsolicited request to someone else's server
/// and recorded nothing for it.
///
/// That shape — construct, one call, exit — is a CLI, a Lambda handler, a CI
/// step. It is the population the first-request trigger exists to make visible,
/// so losing it here would have defeated the change that introduced it. Go and
/// Java run their cold path inline for exactly this reason (their issue #1693);
/// this is the same decision.
///
/// # What it costs, stated as a number
///
/// The whole telemetry path is bounded by [`HEARTBEAT_TIMEOUT`] (3 s) — the
/// `/health` probe and the checkpoint POST share that one deadline rather than
/// stacking — so this can add at most ~3 s to a caller's request. The outer
/// timeout here is belt-and-braces on top of that internal budget.
///
/// It is reachable at most once per [`HEARTBEAT_GUARD_INTERVAL`] per process,
/// and only actually sends when a ping is DUE, which the 7-day stamp limits to
/// once per machine per week. On every other request `prepare_heartbeat`
/// returns `None` after one mutex acquire and this function returns having
/// awaited nothing.
pub async fn maybe_send_heartbeat_on_request(endpoint: &str, mode: &Mode) {
    let Some((ctx, slot)) = prepare_heartbeat(endpoint, mode) else {
        // Warm gate, or opted out: the overwhelmingly common case, and the
        // reason this is affordable on a request path at all.
        return;
    };
    // A margin over HEARTBEAT_TIMEOUT so this outer bound never fires FIRST and
    // pre-empts the inner one — which would skip `record_attempt` and leave the
    // failure backoff blind to an attempt that really did fail.
    let bound = HEARTBEAT_TIMEOUT + Duration::from_millis(500);
    if tokio::time::timeout(bound, gated_send(ctx, slot))
        .await
        .is_err()
    {
        debug!("Telemetry heartbeat exceeded {bound:?} on the request path; abandoned");
    }
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

    let delivered = send_heartbeat(&ctx).await;
    record_attempt(delivered);
    if delivered {
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
/// Outer `None`: no override, use the real path. Outer `Some(None)`: model an
/// environment with NO usable stamp path at all.
#[cfg(test)]
static STAMP_PATH_OVERRIDE: Mutex<Option<Option<PathBuf>>> = Mutex::new(None);

fn resolve_stamp_path() -> Option<PathBuf> {
    #[cfg(test)]
    {
        // Poison-recovering like every other telemetry lock: a panic in one
        // test must not make every later client construction panic here.
        if let Some(path) = STAMP_PATH_OVERRIDE
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
        {
            return path;
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
    // `trim()` before the slash trim so a whitespace-only endpoint is treated
    // as no endpoint and skipped, rather than building a request that can only
    // die at URL parse.
    let endpoint = endpoint.trim().trim_end_matches('/');
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
    /// Adapter identifiers declared through [`register_adapter`]. Emitted as
    /// `[]` rather than omitted — that wire shape is load-bearing, because the
    /// receiver distinguishes "reported no features" from "does not report
    /// them".
    features: Vec<String>,
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
            // The registry is the ONLY producer of this array. Read here rather
            // than snapshotted in `from_env` so an adapter that registers after
            // the client is built still reaches the next heartbeat.
            features: registered_features(),
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

/// The ONE HTTP client the telemetry path uses, for both legs.
///
/// A function rather than an inline builder so the tests exercise the SHIPPED
/// construction. When the probe tests built their own client they were testing
/// the test helper: the redirect policy below was live in production and
/// absent from every probe test, and the test written to prove redirects are
/// refused passed a redirect straight through.
///
/// Deliberately built with no `.timeout(...)`: the budget is per-request, set
/// from the shared deadline in [`send_heartbeat`].
///
/// Redirects are REFUSED, and that is load bearing on both legs. reqwest
/// follows up to 10 by default, which would mean:
///
///   * `/health` is no longer one request, and the values relayed would be
///     whatever answered at the redirect TARGET — so the disclosure's
///     "whatever is answering at the endpoint you configured" would be false,
///     and the endpoint's operator would choose who supplies them.
///   * worse on the POST: reqwest re-issues a redirected POST as a bodyless
///     GET, so a 302 on the checkpoint URL yields a 200 carrying NOTHING,
///     `send_heartbeat` reports delivery, and the 7-day stamp advances on a
///     ping that was never sent — telemetry then goes dark for a week.
///
/// A `User-Agent` is set because this is the first SDK feature that contacts
/// the caller's own platform unsolicited; it must be attributable in their
/// access logs. No other default header is set, so the SDK's `Authorization`
/// and `X-License-Key` never reach the probe.
fn telemetry_client() -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(concat!("axonflow-sdk-rust/", env!("CARGO_PKG_VERSION")))
        .build()
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

    let client = match telemetry_client() {
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
/// A fresh process: no prior check, nothing in flight, no accumulated
/// backoff.
#[cfg(test)]
fn reset_gate_for_tests() {
    let mut inner = gate().lock().unwrap_or_else(|e| e.into_inner());
    inner.last_checked = None;
    inner.in_flight = false;
    inner.consecutive_failures = 0;
    inner.last_delivered = None;
}

/// The guard interval has elapsed — and NOTHING else has changed.
///
/// Deliberately distinct from [`reset_gate_for_tests`]: clearing the failure
/// counter here would mean "time passed" also erased the backoff, and the
/// backoff test would then measure one failure over and over instead of
/// consecutive ones.
#[cfg(test)]
fn reopen_gate_for_tests() {
    let mut inner = gate().lock().unwrap_or_else(|e| e.into_inner());
    inner.last_checked = None;
    inner.in_flight = false;
}

/// The 7-day boundary has been crossed: the short guard has elapsed AND the
/// last delivery is older than the heartbeat interval.
///
/// Distinct from [`reopen_gate_for_tests`], which models only an hour passing.
/// A test that means "a week later" has to say so, or the in-memory cadence
/// floor refuses its claim and the test reads as a regression.
#[cfg(test)]
fn cross_the_heartbeat_interval_for_tests() {
    let mut inner = gate().lock().unwrap_or_else(|e| e.into_inner());
    inner.last_checked = None;
    inner.in_flight = false;
    inner.last_delivered = Instant::now().checked_sub(HEARTBEAT_INTERVAL + Duration::from_secs(1));
}

/// Put the gate into a specific state so a test can ask it to REFUSE.
///
/// Needed because the widened interval is only observable at the moment a
/// claim is declined, and no test can wait an hour. Without it the backoff was
/// pinned only by a test of the pure interval function and a test that read
/// the counter — so substituting the call site with the base interval left the
/// whole suite green while the hourly-probe-forever defect came back.
#[cfg(test)]
fn set_gate_state_for_tests(last_checked_ago: Duration, consecutive_failures: u32) {
    let mut inner = gate().lock().unwrap_or_else(|e| e.into_inner());
    inner.last_checked = Instant::now().checked_sub(last_checked_ago);
    inner.in_flight = false;
    inner.consecutive_failures = consecutive_failures;
}

#[cfg(test)]
fn consecutive_failures_for_tests() -> u32 {
    gate()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .consecutive_failures
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
