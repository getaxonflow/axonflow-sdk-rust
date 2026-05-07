// Example: list recent AxonFlow policy decisions for the caller's tenant.
//
// Implements the `GET /api/v1/decisions` contract — companion to the
// `explain_decision` example. Returns the slim DecisionSummary page
// with optional filters; tier-cap 429s surface as
// AxonFlowError::RateLimited carrying the V1 upgrade envelope.
//
// Required env vars:
//   AXONFLOW_AGENT_URL          (default: http://localhost:8080)
//   AXONFLOW_CLIENT_ID
//   AXONFLOW_CLIENT_SECRET
//
// Optional filters:
//   AXONFLOW_LIST_DECISION       allow|deny|require_approval
//   AXONFLOW_LIST_POLICY_ID      e.g. sys_sqli_stacked_drop
//   AXONFLOW_LIST_LIMIT          integer (server-capped per tier)
//
// Seed decisions quickly via /api/v1/mcp/check-input — anything that
// trips a static policy will land an audit row this endpoint reads.

use axonflow_sdk_rust::{AxonFlowClient, AxonFlowConfig, AxonFlowError, ListDecisionsOptions};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let agent_url =
        std::env::var("AXONFLOW_AGENT_URL").unwrap_or_else(|_| "http://localhost:8080".to_string());
    let client_id = std::env::var("AXONFLOW_CLIENT_ID").expect("AXONFLOW_CLIENT_ID must be set");
    let client_secret =
        std::env::var("AXONFLOW_CLIENT_SECRET").expect("AXONFLOW_CLIENT_SECRET must be set");

    let opts = ListDecisionsOptions {
        decision: std::env::var("AXONFLOW_LIST_DECISION").ok(),
        policy_id: std::env::var("AXONFLOW_LIST_POLICY_ID").ok(),
        limit: std::env::var("AXONFLOW_LIST_LIMIT")
            .ok()
            .and_then(|s| s.parse().ok()),
        ..Default::default()
    };

    println!("Initializing AxonFlow client at {}...", agent_url);
    let config = AxonFlowConfig::new(agent_url).with_auth(client_id, client_secret);
    let client = AxonFlowClient::new(config)?;

    match client.list_decisions(opts).await {
        Ok(decisions) => {
            println!("=== Recent decisions ({}) ===", decisions.len());
            for d in decisions {
                let policy = d.policy_id.as_deref().unwrap_or("-");
                let tool = d.tool_signature.as_deref().unwrap_or("-");
                println!(
                    "  {} {:18} {} policy={} tool={}",
                    d.timestamp, d.decision, d.decision_id, policy, tool
                );
            }
            Ok(())
        }
        Err(AxonFlowError::RateLimited { envelope }) => {
            // Tier-cap path — surface the V1 upgrade context to the
            // user, then exit non-zero so callers can branch on it.
            eprintln!("=== Tier limit reached ({}) ===", envelope.limit_type);
            eprintln!("  current tier: {}", envelope.tier);
            eprintln!("  limit:        {}", envelope.limit);
            eprintln!("  reason:       {}", envelope.error);
            eprintln!();
            eprintln!(
                "  upgrade to {}: {}",
                envelope.upgrade.tier, envelope.upgrade.wording
            );
            eprintln!("    compare:    {}", envelope.upgrade.compare_url);
            eprintln!("    buy:        {}", envelope.upgrade.buy_url);
            std::process::exit(2);
        }
        Err(e) => Err(Box::new(e) as Box<dyn std::error::Error>),
    }
}
