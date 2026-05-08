//! Anonymous SDK heartbeat — telemetry parity with the Go / Python /
//! TypeScript / Java SDKs.
//!
//! Sends one ping per machine per 7 days to
//! `https://checkpoint.getaxonflow.com/v1/ping` carrying SDK version,
//! OS, architecture, runtime version, deployment mode, and an
//! endpoint-type classification (never the raw URL — see issue #1525
//! in the AxonFlow tracker for the privacy rationale).
//!
//! `AXONFLOW_TELEMETRY=off` is the SOLE opt-out path. There is
//! intentionally no programmatic disable on the SDK config — the
//! single env-var lever matches HashiCorp checkpoint, Docker, Datadog
//! Agent. Sandbox-mode clients tag their pings with `stream="sandbox"`
//! so analytics can distinguish dev/test usage from production
//! heartbeat. `DO_NOT_TRACK` is intentionally NOT honored — host CLIs
//! commonly inherit it, which makes it an unreliable expression of
//! AxonFlow-scoped intent.
//!
//! Pre-v0.2 the Rust SDK pinged `{configured_endpoint}/api/telemetry/heartbeat`
//! against the local agent — useful for proxy debugging but invisible to
//! AxonFlow's central telemetry pipeline. The endpoint switch in v0.2
//! brings Rust into parity with the other 4 first-class SDKs.

use std::fs;
use std::path::PathBuf;
use std::sync::Once;
use std::time::{Duration, SystemTime};
use tracing::debug;

use crate::config::Mode;

static HEARTBEAT_ONCE: Once = Once::new();

const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(7 * 24 * 60 * 60); // 7 days
const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(3);
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

/// Fire-and-forget anonymous heartbeat. Called once per process from
/// `AxonFlowClient::new`. Internally synchronous on the gating decision
/// (env-var, stamp file mtime), then spawns a tokio task for the HTTP
/// POST so the constructor returns promptly.
///
/// `AXONFLOW_TELEMETRY=off` short-circuits before any filesystem or
/// network access. Anything else allows the ping; the per-machine
/// 7-day stamp file (in `~/Library/Caches/axonflow/` on macOS,
/// `~/.cache/axonflow/` elsewhere) bounds the cadence.
pub fn maybe_send_heartbeat(endpoint: &str, mode: &Mode) {
    HEARTBEAT_ONCE.call_once(|| {
        if telemetry_off() {
            debug!("Telemetry disabled via AXONFLOW_TELEMETRY=off");
            return;
        }

        // resolve_stamp_path returns None on environments without a usable
        // cache dir — containerized runtimes with HOME unset, AWS Lambda,
        // distroless images, etc. In that case we fall back to "one ping
        // per process" via HEARTBEAT_ONCE (the Once gate above) — same
        // semantic as the Go / Python / TypeScript / Java SDKs, which
        // also degrade to per-process gating when their stamp path is
        // unavailable. Pre-fix the Rust SDK exited early here, leaving
        // restricted/containerized environments invisible to central
        // telemetry — undercutting v0.2's parity goal. Reviewer-flagged
        // 2026-05-08.
        let stamp_path = resolve_stamp_path();

        // Only consult the stamp file if we have one. When None, the
        // per-process Once gate is the only rate-limit.
        if let Some(ref p) = stamp_path {
            if stamp_is_fresh(p) {
                debug!("Telemetry heartbeat is still fresh (<7 days)");
                return;
            }
        } else {
            debug!("Telemetry stamp path unavailable; falling back to per-process gate");
        }

        let endpoint = endpoint.to_string();
        let mode = mode.clone();
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                send_heartbeat(&endpoint, &mode, stamp_path).await;
            });
        } else {
            debug!("Telemetry skipped: no tokio runtime in scope");
        }
    });
}

fn telemetry_off() -> bool {
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

fn resolve_stamp_path() -> Option<PathBuf> {
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

fn os_str() -> &'static str {
    std::env::consts::OS
}

fn arch_str() -> &'static str {
    std::env::consts::ARCH
}

fn runtime_version_str() -> String {
    // rustc version is not available at runtime without a build script;
    // report the SDK's MSRV channel instead so the field has a stable
    // shape across rustc-host variations.
    "rustc-stable".to_string()
}

fn instance_id() -> String {
    // UUIDv4 without external deps — same approach as the Go SDK's
    // generateInstanceID.
    use std::time::UNIX_EPOCH;
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let pid = std::process::id() as u64;
    let mix = nanos ^ pid.rotate_left(17);
    let bytes = mix.to_le_bytes();
    format!(
        "{:08x}-{:04x}-4{:03x}-{:04x}-{:012x}",
        u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
        u16::from_le_bytes([bytes[4], bytes[5]]),
        u16::from_le_bytes([bytes[6], bytes[7]]) & 0x0fff,
        (u16::from_le_bytes([bytes[0], bytes[7]]) & 0x3fff) | 0x8000,
        u64::from_le_bytes([
            bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[0], bytes[7]
        ]) & 0xffff_ffff_ffff
    )
}

async fn send_heartbeat(endpoint: &str, mode: &Mode, stamp_path: Option<PathBuf>) {
    let checkpoint_url = std::env::var("AXONFLOW_CHECKPOINT_URL")
        .unwrap_or_else(|_| DEFAULT_CHECKPOINT_URL.to_string());

    let client = match reqwest::Client::builder()
        .timeout(HEARTBEAT_TIMEOUT)
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            debug!("Telemetry skipped: client build failed: {}", e);
            return;
        }
    };

    // Stream classifier — sandbox-mode clients self-tag; production
    // clients omit the field (server defaults empty to "heartbeat").
    // The omitempty semantic is implemented by serializing without the
    // field when None.
    let mut payload = serde_json::Map::new();
    // v1 telemetry-schema discriminator — always "sdk" for this crate.
    payload.insert("telemetry_type".into(), serde_json::Value::from("sdk"));
    payload.insert("sdk".into(), serde_json::Value::from("rust"));
    payload.insert(
        "sdk_version".into(),
        serde_json::Value::from(env!("CARGO_PKG_VERSION")),
    );
    payload.insert("os".into(), serde_json::Value::from(os_str()));
    payload.insert("arch".into(), serde_json::Value::from(arch_str()));
    payload.insert(
        "runtime_version".into(),
        serde_json::Value::from(runtime_version_str()),
    );
    // v1 schema: deployment_mode classifies from endpoint host + AXONFLOW_TRY=1.
    payload.insert(
        "deployment_mode".into(),
        serde_json::Value::from(classify_deployment_mode(endpoint)),
    );
    payload.insert(
        "endpoint_type".into(),
        serde_json::Value::from(classify_endpoint(endpoint)),
    );
    payload.insert("features".into(), serde_json::Value::Array(vec![]));
    payload.insert("instance_id".into(), serde_json::Value::from(instance_id()));
    if let Some(stream) = stream_for_mode(mode) {
        payload.insert("stream".into(), serde_json::Value::from(stream));
    }

    debug!(
        "[AxonFlow] Anonymous telemetry enabled. Opt out: AXONFLOW_TELEMETRY=off | https://docs.getaxonflow.com/docs/telemetry"
    );

    match client.post(&checkpoint_url).json(&payload).send().await {
        Ok(resp) if resp.status().is_success() => {
            debug!("Telemetry heartbeat delivered");
            // Stamp-on-delivery: only update the file when we know the
            // ping landed AND we have a stamp path. When stamp_path is
            // None (containerized envs without a usable cache dir), the
            // per-process Once gate is the only rate-limit — we still
            // sent the ping but persist nothing across restarts. Same
            // fallback as Go / Python / TS / Java SDKs in equivalent
            // environments.
            if let Some(stamp_path) = stamp_path {
                if let Some(parent) = stamp_path.parent() {
                    let _ = fs::create_dir_all(parent);
                }
                let _ = fs::write(
                    &stamp_path,
                    format!("last_sent={}", chrono::Utc::now().to_rfc3339()),
                );
            }
        }
        Ok(resp) => debug!("Telemetry heartbeat rejected by server: {}", resp.status()),
        Err(e) => debug!("Telemetry heartbeat failed: {}", e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        // v1 schema: deployment_mode is endpoint-derived, not Mode-derived.
        // Empty/unparseable -> unknown.
        std::env::remove_var("AXONFLOW_TRY");
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
        std::env::set_var("AXONFLOW_TRY", "1");
        assert_eq!(
            classify_deployment_mode("https://my-proxy.example.com"),
            DEPLOYMENT_MODE_COMMUNITY_SAAS
        );
        std::env::remove_var("AXONFLOW_TRY");
    }

    #[test]
    fn telemetry_off_recognizes_off_value() {
        std::env::set_var("AXONFLOW_TELEMETRY", "off");
        assert!(telemetry_off());
        std::env::set_var("AXONFLOW_TELEMETRY", "OFF");
        assert!(telemetry_off());
        std::env::set_var("AXONFLOW_TELEMETRY", "  off  ");
        assert!(telemetry_off());
        std::env::set_var("AXONFLOW_TELEMETRY", "");
        assert!(!telemetry_off());
        std::env::set_var("AXONFLOW_TELEMETRY", "on");
        assert!(!telemetry_off());
        std::env::remove_var("AXONFLOW_TELEMETRY");
        assert!(!telemetry_off());
    }
}
