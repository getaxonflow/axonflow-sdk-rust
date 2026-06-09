//! Runtime proof — Rust SDK Decision Mode PEP (decide -> fulfill -> forward)
//! against a LIVE enterprise agent. NO mocks.
//!
//! Proves the #2563 / #2571 contract end-to-end:
//!
//!   1. decide() on a PII-bearing query returns an `allow` verdict carrying a
//!      `redact_pii` request-phase obligation.
//!   2. fulfill_request() round-trips the query through the engine's
//!      check-input endpoint and returns ENGINE-masked content in which neither
//!      `john.doe@example.com` nor `4111111111111111` survives, and the content
//!      differs from the original (the engine actually changed it).
//!   3. decide_and_fulfill() produces the same masked content in one call.
//!   4. Demo / wrong credentials are refused with HTTP 401 (AxonFlowError::ApiError
//!      status 401) — never silently allowed.
//!
//! Reads enterprise auth from the environment (sourced from
//! /tmp/axonflow-e2e-env.sh):
//!   AXONFLOW_ENDPOINT (default http://localhost:8080)
//!   AXONFLOW_CLIENT_ID      — org id
//!   AXONFLOW_CLIENT_SECRET  — Ed25519 license key (HTTP Basic password)
//!   AXONFLOW_TENANT_ID      — tenant scope (caller_identity.tenant_id)
//!   AXONFLOW_USER_TOKEN     — optional validated-user token

use axonflow_sdk_rust::{
    AxonFlowClient, AxonFlowConfig, AxonFlowError, DecideRequest, DecisionCallerIdentity,
    DecisionTarget, VERDICT_ALLOW,
};

const PII_QUERY: &str =
    "Send the receipt to john.doe@example.com and charge card 4111111111111111";
const EMAIL: &str = "john.doe@example.com";
const CARD: &str = "4111111111111111";

fn env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}

fn build_request(tenant: Option<&str>, user_token: Option<&str>) -> DecideRequest {
    DecideRequest {
        stage: "tool".to_string(),
        query: PII_QUERY.to_string(),
        caller_identity: DecisionCallerIdentity {
            tenant_id: tenant.map(|s| s.to_string()),
            ..Default::default()
        },
        target: DecisionTarget {
            r#type: Some("tool".to_string()),
            tool: Some("send_receipt".to_string()),
            ..Default::default()
        },
        user_token: user_token.map(|s| s.to_string()),
        context: None,
    }
}

#[tokio::main]
async fn main() {
    let endpoint = env("AXONFLOW_ENDPOINT").unwrap_or_else(|| "http://localhost:8080".to_string());
    let org = env("AXONFLOW_CLIENT_ID").expect("AXONFLOW_CLIENT_ID required");
    let license = env("AXONFLOW_CLIENT_SECRET").expect("AXONFLOW_CLIENT_SECRET required");
    let tenant = env("AXONFLOW_TENANT_ID");
    let user_token = env("AXONFLOW_USER_TOKEN");

    let client = AxonFlowClient::new(
        AxonFlowConfig::new(&endpoint).with_auth(org.clone(), license.clone()),
    )
    .expect("client init");

    let mut failed = false;

    // --- 1) decide: allow + redact_pii request-phase obligation ---
    let req = build_request(tenant.as_deref(), user_token.as_deref());
    let decision = match client.decide(req).await {
        Ok(d) => d,
        Err(e) => {
            println!("FAIL: decide() errored: {e}");
            std::process::exit(1);
        }
    };
    println!(
        ">>> decide verdict={} decision_id={:?} obligations={}",
        decision.verdict,
        decision.decision_id,
        decision.obligations.len()
    );
    if decision.verdict != VERDICT_ALLOW {
        println!(
            "FAIL: expected allow verdict, got {:?} (error={:?})",
            decision.verdict, decision.error
        );
        std::process::exit(1);
    }
    if !axonflow_sdk_rust::has_request_redaction(&decision.obligations) {
        println!("FAIL: allow verdict carried no request-phase redact_pii obligation");
        for ob in &decision.obligations {
            println!("    obligation type={} fulfillment={:?}", ob.r#type, ob.fulfillment);
        }
        std::process::exit(1);
    }
    println!("PASS: decide -> allow + request-phase redact_pii obligation");

    // --- 2) fulfill_request: engine-masked, no PII survives, content changed ---
    let (masked, did_redact) = match client.fulfill_request(&decision, PII_QUERY).await {
        Ok(t) => t,
        Err(e) => {
            println!("FAIL: fulfill_request() errored: {e}");
            std::process::exit(1);
        }
    };
    println!(">>> fulfilled content: {masked}");
    if !did_redact {
        println!("FAIL: engine reported no redaction (did_redact=false)");
        failed = true;
    }
    if masked == PII_QUERY {
        println!("FAIL: fulfilled content equals the original (no masking happened)");
        failed = true;
    }
    if masked.contains(EMAIL) {
        println!("FAIL: email {EMAIL} survived redaction");
        failed = true;
    }
    if masked.contains(CARD) {
        println!("FAIL: card {CARD} survived redaction");
        failed = true;
    }
    if !failed {
        println!("PASS: fulfill_request -> engine-masked; neither email nor card survives");
    }

    // --- 3) decide_and_fulfill: same masked content in one call ---
    let req2 = build_request(tenant.as_deref(), user_token.as_deref());
    match client.decide_and_fulfill(req2).await {
        Ok((verdict, content, _)) => {
            println!(">>> decide_and_fulfill verdict={verdict} content={content}");
            if verdict != VERDICT_ALLOW {
                println!("FAIL: decide_and_fulfill verdict was {verdict}, expected allow");
                failed = true;
            }
            if content.contains(EMAIL) || content.contains(CARD) {
                println!("FAIL: decide_and_fulfill leaked PII");
                failed = true;
            }
            if content == PII_QUERY {
                println!("FAIL: decide_and_fulfill returned the unredacted query");
                failed = true;
            }
            if !failed {
                println!("PASS: decide_and_fulfill -> masked content, no PII");
            }
        }
        Err(e) => {
            println!("FAIL: decide_and_fulfill() errored: {e}");
            failed = true;
        }
    }

    // --- 4) demo creds refused with 401 ---
    let demo = AxonFlowClient::new(
        AxonFlowConfig::new(&endpoint).with_auth("demo-org", "demo-license-not-real"),
    )
    .expect("demo client init");
    let req3 = build_request(Some("demo-org"), None);
    match demo.decide(req3).await {
        Err(AxonFlowError::ApiError { status: 401, .. }) => {
            println!("PASS: demo creds refused with 401");
        }
        Ok(d) => {
            println!(
                "FAIL: demo creds were NOT refused — got verdict={:?}",
                d.verdict
            );
            failed = true;
        }
        Err(e) => {
            println!("FAIL: demo creds produced a non-401 error: {e}");
            failed = true;
        }
    }

    if failed {
        println!("\nRESULT: FAIL");
        std::process::exit(1);
    }
    println!("\nRESULT: PASS — decide -> fulfill -> masked; demo creds refused");
}
