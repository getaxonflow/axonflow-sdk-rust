// Example: explain a previously-made AxonFlow policy decision.
//
// Implements the ADR-043 explainability flow. Given a decision_id (typically
// surfaced on the response of a blocked governed call, an audit_logs row, or
// the `explain_decision` MCP tool), this example fetches the structured
// explanation and renders the matched policies, risk level, and override
// availability.
//
// Required env vars:
//   AXONFLOW_AGENT_URL          (default: http://localhost:8080)
//   AXONFLOW_CLIENT_ID
//   AXONFLOW_CLIENT_SECRET
//   AXONFLOW_DECISION_ID        the decision to explain
//
// Get a decision_id quickly by hitting a known-blocked policy:
//
//   curl -u "$AXONFLOW_CLIENT_ID:$AXONFLOW_CLIENT_SECRET" \
//        -X POST $AXONFLOW_AGENT_URL/api/request \
//        -H 'Content-Type: application/json' \
//        -d '{"query":"My SSN is 123-45-6789","user_token":"u1","request_type":"chat"}'
//
// then read decision_id from the block response or the most recent audit row.

use axonflow_sdk_rust::{AxonFlowClient, AxonFlowConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let agent_url =
        std::env::var("AXONFLOW_AGENT_URL").unwrap_or_else(|_| "http://localhost:8080".to_string());
    let client_id = std::env::var("AXONFLOW_CLIENT_ID").expect("AXONFLOW_CLIENT_ID must be set");
    let client_secret =
        std::env::var("AXONFLOW_CLIENT_SECRET").expect("AXONFLOW_CLIENT_SECRET must be set");
    let decision_id = std::env::var("AXONFLOW_DECISION_ID")
        .expect("AXONFLOW_DECISION_ID must be set (a decision_id from a recent blocked call)");

    println!("Initializing AxonFlow client at {}...", agent_url);
    let config = AxonFlowConfig::new(agent_url).with_auth(client_id, client_secret);
    let client = AxonFlowClient::new(config)?;

    println!("Explaining decision {}...\n", decision_id);
    let explanation = client.explain_decision(&decision_id).await?;

    println!("=== Decision Explanation ===");
    println!("  decision_id: {}", explanation.decision_id);
    println!("  timestamp:   {}", explanation.timestamp);
    println!("  decision:    {}", explanation.decision);
    println!("  reason:      {}", explanation.reason);
    if let Some(risk) = &explanation.risk_level {
        println!("  risk_level:  {}", risk);
    }
    if let Some(tool) = &explanation.tool_signature {
        println!("  tool:        {}", tool);
    }

    println!("\n  policy_matches ({}):", explanation.policy_matches.len());
    for (i, m) in explanation.policy_matches.iter().enumerate() {
        let name = m.policy_name.as_deref().unwrap_or("(unnamed)");
        let action = m.action.as_deref().unwrap_or("-");
        let risk = m.risk_level.as_deref().unwrap_or("-");
        println!(
            "    [{}] {} ({}) — action={} risk={} allow_override={}",
            i, m.policy_id, name, action, risk, m.allow_override
        );
    }

    if !explanation.matched_rules.is_empty() {
        println!("\n  matched_rules ({}):", explanation.matched_rules.len());
        for r in &explanation.matched_rules {
            let rule_id = r.rule_id.as_deref().unwrap_or("(no rule id)");
            let matched_on = r.matched_on.as_deref().unwrap_or("-");
            println!("    {} on {}: matched={}", r.policy_id, rule_id, matched_on);
        }
    }

    println!(
        "\n  override_available:           {}",
        explanation.override_available
    );
    if let Some(existing) = &explanation.override_existing_id {
        println!("  override_existing_id:         {}", existing);
    }
    println!(
        "  historical_hit_count_session: {}",
        explanation.historical_hit_count_session
    );
    if let Some(link) = &explanation.policy_source_link {
        println!("  policy_source_link:           {}", link);
    }

    Ok(())
}
