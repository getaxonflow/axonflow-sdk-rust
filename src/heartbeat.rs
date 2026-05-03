use std::fs;
use std::path::PathBuf;
use std::sync::Once;
use std::time::{Duration, SystemTime};
use tracing::debug;

static HEARTBEAT_ONCE: Once = Once::new();

const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(7 * 24 * 60 * 60); // 7 days

pub fn maybe_send_heartbeat(endpoint: &str) {
    HEARTBEAT_ONCE.call_once(|| {
        if let Some(stamp_path) = resolve_stamp_path() {
            if let Ok(metadata) = fs::metadata(&stamp_path) {
                if let Ok(modified) = metadata.modified() {
                    if let Ok(elapsed) = SystemTime::now().duration_since(modified) {
                        if elapsed < HEARTBEAT_INTERVAL {
                            debug!("Telemetry heartbeat is still fresh ({}s elapsed)", elapsed.as_secs());
                            return;
                        }
                    }
                }
            }

            let endpoint = endpoint.to_string();
            if let Ok(handle) = tokio::runtime::Handle::try_current() {
                handle.spawn(async move {
                    send_heartbeat(&endpoint, stamp_path).await;
                });
            }
        }
    });
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

async fn send_heartbeat(endpoint: &str, stamp_path: PathBuf) {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build();

    if let Ok(client) = client {
        let url = format!("{}/api/telemetry/heartbeat", endpoint);
        let payload = serde_json::json!({
            "sdk": "rust",
            "version": env!("CARGO_PKG_VERSION"),
        });

        match client.post(&url).json(&payload).send().await {
            Ok(resp) if resp.status().is_success() => {
                debug!("Telemetry heartbeat sent successfully");
                if let Some(parent) = stamp_path.parent() {
                    let _ = fs::create_dir_all(parent);
                }
                let _ = fs::write(&stamp_path, format!("last_sent={}", chrono::Utc::now().to_rfc3339()));
            }
            Ok(resp) => debug!("Telemetry heartbeat rejected by server: {}", resp.status()),
            Err(e) => debug!("Telemetry heartbeat failed: {}", e),
        }
    }
}
