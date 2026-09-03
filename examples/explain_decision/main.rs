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
//   AXONFLOW_USER_TOKEN         the PER-USER identity this read is scoped to
//                               (required on an enterprise stack — see below)
//
// Optional:
//   AXONFLOW_DECISION_ID        the decision to explain. When unset this
//                               example asks the platform for the most recent
//                               decision THIS identity can see.
//
// # Why AXONFLOW_USER_TOKEN is not optional here (platform #2922)
//
// client_id/client_secret say which ORGANIZATION is asking. Explain answers
// from WHO is asking. On an enterprise stack a developer or viewer explains
// only their own decisions, a tenant-wide role (admin/owner/policy_admin)
// explains the whole tenant, and a caller presenting NO identity explains
// NOTHING — the endpoint answers not-found for every id, including ids that
// plainly exist. That is why this example failed on every enterprise stack
// until the SDK grew a read-path identity: it was asking anonymously.
//
// Mint one the way the E2E workflow does:
//
//   export AXONFLOW_USER_TOKEN=$(./scripts/generate-jwt.sh --kind user \
//       --email dev@acme.com --org-id "$AXONFLOW_CLIENT_ID" --role developer --quiet)
//
// (./scripts/setup-e2e-testing.sh already exports exactly this variable.)
// Community deployments are single-operator and need none of it.
//
// Get a decision_id quickly by hitting a known-blocked policy:
//
//   curl -u "$AXONFLOW_CLIENT_ID:$AXONFLOW_CLIENT_SECRET" \
//        -X POST $AXONFLOW_AGENT_URL/api/request \
//        -H 'Content-Type: application/json' \
//        -d '{"query":"My SSN is 123-45-6789","user_token":"u1","request_type":"chat"}'
//
// then read decision_id from the block response or the most recent audit row.

use axonflow_sdk_rust::{AxonFlowClient, AxonFlowConfig, AxonFlowError, ListDecisionsOptions};

/// The sentence a reader of this example actually needs. Without it the
/// distinct causes behind "not found" arrive looking identical.
fn scope_hint(err: &AxonFlowError) -> &'static str {
    match err {
        AxonFlowError::ReadScope(refusal) if refusal.identity_missing() => {
            "\n  -> This read presented no per-user identity the platform could resolve, so it \
             returned nothing by construction. Set AXONFLOW_USER_TOKEN (see the file header) - and \
             check the address is not in a reserved domain."
        }
        AxonFlowError::ReadScope(_) => {
            "\n  -> The identity in AXONFLOW_USER_TOKEN is scoped to its own rows and this \
             decision is not among them. Use an admin, owner or policy_admin token to read the \
             whole tenant."
        }
        _ => "",
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let agent_url =
        std::env::var("AXONFLOW_AGENT_URL").unwrap_or_else(|_| "http://localhost:8080".to_string());
    let client_id = std::env::var("AXONFLOW_CLIENT_ID").expect("AXONFLOW_CLIENT_ID must be set");
    let client_secret =
        std::env::var("AXONFLOW_CLIENT_SECRET").expect("AXONFLOW_CLIENT_SECRET must be set");
    let user_token = std::env::var("AXONFLOW_USER_TOKEN").unwrap_or_default();

    println!("Initializing AxonFlow client at {}...", agent_url);
    if user_token.is_empty() {
        println!(
            "note: AXONFLOW_USER_TOKEN is unset - this read is unscoped. On an enterprise stack \
             it will explain nothing; see the file header."
        );
    }
    let mut config = AxonFlowConfig::new(agent_url).with_auth(client_id, client_secret);
    // The read-path identity. Empty is legal and means "ask anonymously",
    // which on an enterprise stack explains nothing.
    config.user_token = if user_token.is_empty() {
        None
    } else {
        Some(user_token)
    };
    let client = AxonFlowClient::new(config)?;

    // No id given: ask for one this identity can actually see, so the example
    // explains a real decision rather than failing on a placeholder.
    let decision_id = match std::env::var("AXONFLOW_DECISION_ID") {
        Ok(id) if !id.is_empty() => id,
        _ => {
            println!(
                "AXONFLOW_DECISION_ID is unset - looking up the most recent visible decision..."
            );
            let recent = client
                .list_decisions(ListDecisionsOptions {
                    limit: Some(1),
                    ..Default::default()
                })
                .await
                .map_err(|e| {
                    eprintln!(
                        "could not find a decision to explain: {e}{}",
                        scope_hint(&e)
                    );
                    e
                })?;
            let Some(first) = recent.into_iter().next() else {
                eprintln!(
                    "no decisions are visible to this identity yet - make a governed call first \
                     (see the curl in the file header), then re-run"
                );
                std::process::exit(1);
            };
            println!("  using decision_id={}", first.decision_id);
            first.decision_id
        }
    };

    println!("Explaining decision {}...\n", decision_id);
    let explanation = client.explain_decision(&decision_id).await.map_err(|e| {
        eprintln!("explain_decision failed: {e}{}", scope_hint(&e));
        e
    })?;

    // An explanation that came back without the id it was asked about is not an
    // explanation - fail loudly rather than print an empty report.
    if explanation.decision_id.is_empty() {
        eprintln!("the platform returned an explanation with no decision_id for {decision_id}");
        std::process::exit(1);
    }

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
