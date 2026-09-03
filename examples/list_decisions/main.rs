// Example: list the recent AxonFlow policy decisions VISIBLE TO THE CALLER.
//
// Implements the `GET /api/v1/decisions` contract — companion to the
// `explain_decision` example. Returns the slim DecisionSummary page
// with optional filters; tier-cap 429s surface as
// AxonFlowError::RateLimited carrying the V1 upgrade envelope.
//
// # Whose decisions come back (platform #2922)
//
// Not the tenant's — the caller's. On an enterprise stack a tenant-wide role
// (admin/owner/policy_admin) lists the whole tenant, any other identity lists
// only its own rows, and a caller presenting NO identity lists nothing
// whatsoever. That last case used to look exactly like a quiet tenant; the SDK
// now refuses it as AxonFlowError::ReadScope instead of reporting an empty page
// as data.
//
// Mint an identity the way the E2E workflow does:
//
//   export AXONFLOW_USER_TOKEN=$(./scripts/generate-jwt.sh --kind user \
//       --email dev@acme.com --org-id "$AXONFLOW_CLIENT_ID" --role developer --quiet)
//
// (./scripts/setup-e2e-testing.sh already exports exactly this variable.)
// Community deployments are single-operator and need none of it.
//
// Required env vars:
//   AXONFLOW_AGENT_URL          (default: http://localhost:8080)
//   AXONFLOW_CLIENT_ID
//   AXONFLOW_CLIENT_SECRET
//   AXONFLOW_USER_TOKEN         the per-user identity to scope the read to
//                               (required on an enterprise stack)
//
// Optional filters:
//   AXONFLOW_LIST_DECISION       allowed|blocked|redacted|needs_approval|error
//                                (canonical audit verdicts, platform 9.0.0+;
//                                pre-9.0.0 allow|deny|require_approval now 400)
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
    let mut config = AxonFlowConfig::new(agent_url).with_auth(client_id, client_secret);
    // The read-path identity this listing is scoped to. See the header: leaving
    // it unset against an enterprise stack is what made this example report a
    // confident, empty page.
    config.user_token = std::env::var("AXONFLOW_USER_TOKEN")
        .ok()
        .filter(|t| !t.is_empty());
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
        Err(AxonFlowError::ReadScope(refusal)) if refusal.identity_missing() => {
            eprintln!("=== This read was unscoped ===");
            eprintln!("  {refusal}\n");
            eprintln!(
                "  The platform returned zero rows because it resolved no identity to scope on,"
            );
            eprintln!("  not because your tenant has no decisions. Set AXONFLOW_USER_TOKEN:");
            eprintln!("    export AXONFLOW_USER_TOKEN=$(./scripts/generate-jwt.sh --kind user \\");
            eprintln!(
                "        --email dev@acme.com --org-id \"$AXONFLOW_CLIENT_ID\" --role developer --quiet)"
            );
            std::process::exit(3);
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
