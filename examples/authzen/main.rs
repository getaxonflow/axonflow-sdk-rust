//! The AuthZEN-native authorization surface, against a live agent.
//!
//! ```text
//! AXONFLOW_ENDPOINT=http://localhost:8080 cargo run --example authzen
//! ```
//!
//! Set `AXONFLOW_CLIENT_ID` / `AXONFLOW_CLIENT_SECRET` for a deployment that
//! needs credentials; community mode needs none.
//!
//! # Why the unhappy paths are most of this file
//!
//! The surface refuses what it cannot evaluate rather than evaluating around
//! it, and that is the property an integration has to be written against. An
//! example that only ever shows an allow teaches a reader to write
//! `if decision.allowed()` and nothing else, and the first refusal they meet in
//! production is a string in a log.
//!
//! Steps 4 to 8 are refusals. Each one is an outcome a real gateway hits.

use axonflow_sdk_rust::authzen::{
    Attribute, AuthZenAction, AuthZenBulk, AuthZenDecision, AuthZenErrorCode,
    AuthZenEvaluationError, AuthZenRequest, AuthZenResource, AuthZenSubject,
};
use axonflow_sdk_rust::{AxonFlowClient, AxonFlowConfig};

const STEPS: usize = 8;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // AXONFLOW_AGENT_URL is what every other example and the integration
    // workflow already set; AXONFLOW_ENDPOINT is accepted too so a reader who
    // followed the README's configuration section is not sent to a default
    // localhost that is not running.
    let endpoint = std::env::var("AXONFLOW_AGENT_URL")
        .or_else(|_| std::env::var("AXONFLOW_ENDPOINT"))
        .unwrap_or_else(|_| "http://localhost:8080".to_string());
    let client = AxonFlowClient::new(AxonFlowConfig {
        endpoint: endpoint.clone(),
        client_id: std::env::var("AXONFLOW_CLIENT_ID").ok(),
        client_secret: std::env::var("AXONFLOW_CLIENT_SECRET").ok(),
        ..Default::default()
    })?;

    println!("AuthZEN surface against {endpoint}\n");
    let mut done = 0usize;

    // --- 1. The permitted case -------------------------------------------
    let decision = client
        .evaluate(
            AuthZenRequest::evaluating(
                AuthZenSubject::new("gateway", "llm-gateway-01"),
                AuthZenAction::new("llm.completion"),
                AuthZenResource::new("llm", "llm"),
            )
            .with_query(Attribute::known("what is our refund policy?"))
            .with_correlation("x-session-id", Attribute::known("sess-4711")),
        )
        .await?;
    expect_allowed(&decision, true, "1. a permitted completion")?;
    describe(&decision);
    done += 1;

    // --- 2. The denied case ----------------------------------------------
    //
    // `allowed()` is false AND the state says which of the three non-allowing
    // outcomes this was. A caller that branched on "not DENY" would treat a
    // CHALLENGE as permission, which is why there is no such accessor.
    let decision = client
        .evaluate(
            AuthZenRequest::evaluating(
                AuthZenSubject::new("gateway", "llm-gateway-01"),
                AuthZenAction::new("llm.completion"),
                AuthZenResource::new("llm", "llm"),
            )
            .with_query(Attribute::known("'; DROP TABLE users; --")),
        )
        .await?;
    expect_allowed(&decision, false, "2. a denied completion")?;
    describe(&decision);
    done += 1;

    // --- 3. Several preconditions, ONE decision ---------------------------
    //
    // Moving a ticket has to be authorized against the destination project as
    // well as against the ticket. The entries MEET: one denied entry denies the
    // operation, and the API returns one decision so there is no entry for a
    // caller to act on selectively.
    let decision = client
        .evaluate_all(
            AuthZenBulk::over([
                AuthZenRequest {
                    resource: Some(AuthZenResource::new("tool", "jira/move_issue")),
                    ..Default::default()
                },
                AuthZenRequest {
                    resource: Some(AuthZenResource::new("tool", "jira/update_project")),
                    ..Default::default()
                },
            ])
            .with_subject(AuthZenSubject::new("gateway", "llm-gateway-01"))
            .with_action(AuthZenAction::new("tool.call"))
            .with_query(Attribute::known("move AXN-41 to the platform project")),
        )
        .await?;
    println!(
        "3. two preconditions, one decision: allowed={} state={} decision_id={}",
        decision.allowed(),
        decision.state(),
        decision.decision_id()
    );
    done += 1;

    // --- 4. An attribute the source resolved to NOTHING -------------------
    //
    // This gateway asked its directory for the caller's department and was told
    // there is none. That is ordinary resolved data: the member is omitted and
    // the evaluation proceeds.
    let mut subject = AuthZenSubject::new("gateway", "llm-gateway-01");
    subject.properties.insert_absent("department");
    let decision = client
        .evaluate(
            AuthZenRequest::evaluating(
                subject,
                AuthZenAction::new("llm.completion"),
                AuthZenResource::new("llm", "llm"),
            )
            .with_query(Attribute::known("summarise the incident report")),
        )
        .await?;
    expect_allowed(&decision, true, "4. an absent attribute still evaluates")?;
    done += 1;

    // --- 5. An attribute the source COULD NOT resolve ---------------------
    //
    // The same member, one state over. The directory timed out, so nobody knows
    // whether there is a department. Sending the request without it would
    // obtain a decision that weighed every attribute except that one - and
    // report it as complete. The SDK refuses before the round trip, and the
    // refusal is the one code worth retrying.
    let mut subject = AuthZenSubject::new("gateway", "llm-gateway-01");
    subject
        .properties
        .insert_unknown("department", "the directory timed out after 2s");
    let outcome = client
        .evaluate(
            AuthZenRequest::evaluating(
                subject,
                AuthZenAction::new("llm.completion"),
                AuthZenResource::new("llm", "llm"),
            )
            .with_query(Attribute::known("summarise the incident report")),
        )
        .await;
    expect_refusal(
        outcome,
        "5. an unresolvable attribute",
        AuthZenErrorCode::EvaluationUnavailable,
        Some("/evaluation/subject/properties/department"),
        true,
    )?;
    done += 1;

    // --- 6. An attribute the SERVER cannot evaluate -----------------------
    //
    // The mirror image of step 5, from the other side of the wire. The surface
    // has no way to read a caller-supplied property, so it names the member
    // rather than deciding without it.
    let mut subject = AuthZenSubject::new("gateway", "llm-gateway-01");
    subject.properties.insert_known("department", "finance");
    let outcome = client
        .evaluate(
            AuthZenRequest::evaluating(
                subject,
                AuthZenAction::new("llm.completion"),
                AuthZenResource::new("llm", "llm"),
            )
            .with_query(Attribute::known("summarise the incident report")),
        )
        .await;
    expect_refusal(
        outcome,
        "6. an attribute the surface cannot evaluate",
        AuthZenErrorCode::UnevaluableAttribute,
        Some("/evaluation/subject/properties"),
        false,
    )?;
    done += 1;

    // --- 7. An action outside the surface ---------------------------------
    //
    // The refusal names what WOULD have been accepted, so a caller can correct
    // itself without reading the documentation.
    let outcome = client
        .evaluate(
            AuthZenRequest::evaluating(
                AuthZenSubject::new("gateway", "llm-gateway-01"),
                AuthZenAction::new("database.truncate"),
                AuthZenResource::new("llm", "llm"),
            )
            .with_query(Attribute::known("anything")),
        )
        .await;
    let refusal = expect_refusal(
        outcome,
        "7. an action this surface does not evaluate",
        AuthZenErrorCode::UnsupportedAction,
        Some("/evaluation/action/name"),
        false,
    )?;
    println!("     supported: {:?}", refusal.supported);
    done += 1;

    // --- 8. A refusal that never leaves the process -----------------------
    //
    // An absent subject type is not the gateway type this surface evaluates,
    // and reading it as one would let a body name any caller. The SDK says so
    // at the SAME pointer the server would, so a caller reads one diagnostic
    // whichever side produced it.
    let mut subject = AuthZenSubject::new("gateway", "llm-gateway-01");
    subject.r#type = String::new();
    let outcome = client
        .evaluate(
            AuthZenRequest::evaluating(
                subject,
                AuthZenAction::new("llm.completion"),
                AuthZenResource::new("llm", "llm"),
            )
            .with_query(Attribute::known("anything")),
        )
        .await;
    expect_refusal(
        outcome,
        "8. an incomplete subject, refused locally",
        AuthZenErrorCode::IncompleteEvaluation,
        Some("/evaluation/subject/type"),
        false,
    )?;
    done += 1;

    // A run whose steps stopped executing prints no failure and exits 0, which
    // is indistinguishable from success.
    if done != STEPS {
        return Err(format!("only {done} of {STEPS} steps ran").into());
    }
    println!("\n{done}/{STEPS} steps OK");
    Ok(())
}

fn describe(decision: &AuthZenDecision) {
    println!(
        "     state={} category={} reason={} obligations={} decision_id={}",
        decision.state(),
        decision.category(),
        decision
            .reason()
            .map(|r| r.to_string())
            .unwrap_or_else(|| "-".to_string()),
        decision.obligations().len(),
        decision.decision_id()
    );
}

fn expect_allowed(
    decision: &AuthZenDecision,
    want: bool,
    step: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    if decision.allowed() != want {
        return Err(format!(
            "{step}: expected allowed={want}, got allowed={} state={}",
            decision.allowed(),
            decision.state()
        )
        .into());
    }
    println!("{step}: allowed={}", decision.allowed());
    Ok(())
}

fn expect_refusal(
    outcome: Result<AuthZenDecision, AuthZenEvaluationError>,
    step: &str,
    code: AuthZenErrorCode,
    pointer: Option<&str>,
    retryable: bool,
) -> Result<axonflow_sdk_rust::authzen::AuthZenError, Box<dyn std::error::Error>> {
    let err = match outcome {
        Ok(d) => {
            return Err(format!(
                "{step}: expected a refusal, got a decision (allowed={})",
                d.allowed()
            )
            .into())
        }
        Err(e) => e,
    };
    if err.retryable() != retryable {
        return Err(format!(
            "{step}: expected retryable={retryable}, got {}",
            err.retryable()
        )
        .into());
    }
    let refusal = err
        .as_refusal()
        .ok_or_else(|| format!("{step}: expected a typed refusal, got {err}"))?;
    if refusal.code != code {
        return Err(format!("{step}: expected code {code}, got {}", refusal.code).into());
    }
    if refusal.pointer.as_deref() != pointer {
        return Err(format!(
            "{step}: expected pointer {pointer:?}, got {:?}",
            refusal.pointer
        )
        .into());
    }
    println!(
        "{step}: {} at {} (retryable={})",
        refusal.code,
        refusal.pointer.as_deref().unwrap_or("-"),
        err.retryable()
    );
    Ok(refusal.clone())
}
