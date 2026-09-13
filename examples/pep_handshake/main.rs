//! Declaring an enforcement point's capabilities with the PEP capability
//! handshake, which a platform reads from v10.4.0.
//!
//! An enforcement point (a PEP) declares the exact obligation types and schema
//! versions it can discharge, and the SDK sends that declaration on the calls
//! whose route reads it. From v11.0.0, on every edition, the engine refuses
//! with `unsupported_obligation` a mandatory obligation the caller's
//! declaration cannot discharge, and a caller that presents no declaration can
//! discharge none: `decide` under an organization's redact override refuses a
//! caller that does not declare redaction.
//!
//! This example builds a declaration for the client, presents a different one
//! for one call (one process can be two enforcement points), prints each
//! decision's verdict and reasons, and shows that a declaration the platform
//! would refuse fails here, before anything is sent.
//!
//! Run it before `typed_policies`. Note: after a document with an
//! organization-scope constraint is activated, a decide that does not supply
//! the attribute the constraint conditions on is denied fail-closed with
//! reasons ["unknown_constraint"]; supply the attribute or run this example on
//! a fresh stack. From v11.0.0 the deny's first reason is that code, followed
//! by one naming each constraint it could not evaluate and the attribute it
//! needed (getaxonflow/axonflow-enterprise#4247).
//!
//! Run it against a local stack:
//!
//! ```text
//! export AXONFLOW_AGENT_URL=http://localhost:8080
//! cargo run --example pep_handshake
//! ```
//!
//! `AXONFLOW_CLIENT_ID` and `AXONFLOW_CLIENT_SECRET` are optional here. This
//! SDK's `decide` names no organization in its request body, so a Community
//! deployment answers it with or without credentials; on Enterprise set them
//! to the client id and its license key.
//!
//! Exits non-zero when a step fails, so it doubles as a smoke test.

use axonflow_sdk_rust::{
    AxonFlowClient, AxonFlowConfig, DecideRequest, DecideResponse, PEPCapability, PEPHandshake,
};
use std::process::ExitCode;

/// The same governed request for every step.
fn look_up_the_weather() -> DecideRequest {
    let mut request = DecideRequest::new("tool", "look up the weather");
    request.target.r#type = Some("tool".into());
    request.target.tool = Some("search".into());
    request
}

fn print_decision(decision: &DecideResponse) {
    let reasons: &[String] = decision.reasons.as_deref().unwrap_or(&[]);
    println!(
        "verdict={} reasons={reasons:?} obligations={}",
        decision.verdict,
        decision.obligations.len()
    );
}

#[tokio::main]
async fn main() -> ExitCode {
    let agent_url =
        std::env::var("AXONFLOW_AGENT_URL").unwrap_or_else(|_| "http://localhost:8080".into());
    let mut failures = 0;

    // The request path redacts fields; it declares exactly that.
    let request_path = match PEPHandshake::new(
        "checkout-gateway",
        "https://pep.example.com",
        [PEPCapability::new("field_redact", 1)],
    ) {
        Ok(declaration) => declaration,
        Err(err) => {
            eprintln!("build the declaration: {err}");
            return ExitCode::FAILURE;
        }
    };
    let mut config = AxonFlowConfig::new(agent_url).with_pep_handshake(request_path);
    if let (Ok(id), Ok(secret)) = (
        std::env::var("AXONFLOW_CLIENT_ID"),
        std::env::var("AXONFLOW_CLIENT_SECRET"),
    ) {
        config = config.with_auth(id, secret);
    }
    let client = match AxonFlowClient::new(config) {
        Ok(client) => client,
        Err(err) => {
            eprintln!("build the client: {err}");
            return ExitCode::FAILURE;
        }
    };

    println!("\n=== decide with the client's declaration ===");
    match client.decide(look_up_the_weather()).await {
        Ok(decision) => print_decision(&decision),
        Err(err) => {
            println!("FAILED: {err}");
            failures += 1;
        }
    }

    // The response path masks fields instead. A derived client presents its
    // own declaration for the calls made through it, sharing the transport.
    println!("\n=== decide with a per-call declaration ===");
    match PEPHandshake::new(
        "checkout-gateway-response",
        "https://pep.example.com",
        [PEPCapability::new("field_mask", 1)],
    ) {
        Ok(response_path) => match client
            .with_pep_handshake(response_path)
            .decide(look_up_the_weather())
            .await
        {
            Ok(decision) => print_decision(&decision),
            Err(err) => {
                println!("FAILED: {err}");
                failures += 1;
            }
        },
        Err(err) => {
            println!("FAILED: {err}");
            failures += 1;
        }
    }

    // A declaration the platform would refuse fails at construction, naming
    // the member at fault, instead of the first governed call answering 400.
    println!("\n=== a declaration the platform would refuse ===");
    match PEPHandshake::new(
        "Checkout:Gateway",
        "https://pep.example.com",
        Vec::<PEPCapability>::new(),
    ) {
        Err(err) => println!("refused at {}: {err}", err.pointer()),
        Ok(_) => {
            println!("FAILED: the declaration was accepted");
            failures += 1;
        }
    }

    if failures > 0 {
        println!("\n{failures} step(s) failed");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
