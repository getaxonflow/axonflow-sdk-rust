//! Typed policy authoring against a running AxonFlow v11.0.0 platform.
//!
//! A v11 platform authors policy as a typed document: validated, published as a
//! signed artifact pinned by its digest, and promoted to active. This example
//! reads what the deployment may author, validates a document and prints every
//! finding, and shows the document in force. It publishes and activates only
//! when `AXONFLOW_TYPED_POLICY_PUBLISH=1`, because that changes the
//! organization's active policy, and it exits non-zero when a publication or
//! activation it asked for is refused.
//!
//! Run `pep_handshake` first. Note: after a document with an
//! organization-scope constraint is activated, a decide that does not supply
//! the attribute the constraint conditions on is denied fail-closed with
//! reasons ["unknown_constraint"]; supply the attribute or run this example on
//! a fresh stack. From v11.0.0 the deny's first reason is that code, followed
//! by one naming each constraint it could not evaluate and the attribute it
//! needed (getaxonflow/axonflow-enterprise#4247). The default document below is
//! such a document.
//!
//! Run it against a local stack:
//!
//! ```text
//! export AXONFLOW_AGENT_URL=http://localhost:8080
//! AXONFLOW_TYPED_POLICY_PUBLISH=1 cargo run --example typed_policies
//! ```
//!
//! `AXONFLOW_TYPED_POLICY_BODY` names a JSON file holding
//! `{"document": ..., "fixtures": [...]}`; the default is this repository's
//! `testdata/typed_policy_publish_body.json`. `AXONFLOW_CLIENT_ID` and
//! `AXONFLOW_CLIENT_SECRET` are the credentials the agent authenticates, and
//! the author is the client they name. On Community any credentials are
//! accepted and every call's organization is the deployment's (`ORG_ID`); on
//! Enterprise it is the one the credentials resolve to.
//!
//! Exits non-zero when a step fails, so it doubles as a smoke test.

use axonflow_sdk_rust::{AxonFlowClient, AxonFlowConfig, AxonFlowError};
use serde_json::Value;
use std::process::ExitCode;

/// Prints an error, with the platform's reason and findings for a refusal.
fn report(err: &AxonFlowError) {
    match err {
        AxonFlowError::TypedPolicyRefusal(refusal) => {
            println!(
                "refused: HTTP {} {}: {}",
                refusal.status,
                refusal.reason.as_deref().unwrap_or("-"),
                refusal.message
            );
            for f in &refusal.findings {
                println!(
                    "  {} {} {}",
                    f.severity,
                    f.code,
                    f.policy_id.as_deref().unwrap_or("-")
                );
            }
        }
        other => println!("FAILED: {other}"),
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    let agent_url =
        std::env::var("AXONFLOW_AGENT_URL").unwrap_or_else(|_| "http://localhost:8080".into());
    let body_path = std::env::var("AXONFLOW_TYPED_POLICY_BODY").unwrap_or_else(|_| {
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/typed_policy_publish_body.json"
        )
        .into()
    });
    let body: Value = match std::fs::read_to_string(&body_path)
        .map_err(|e| e.to_string())
        .and_then(|text| serde_json::from_str(&text).map_err(|e| e.to_string()))
    {
        Ok(body) => body,
        Err(err) => {
            eprintln!("read {body_path}: {err}");
            return ExitCode::FAILURE;
        }
    };
    let document = body["document"].clone();
    let fixtures: Vec<Value> = body["fixtures"].as_array().cloned().unwrap_or_default();

    let mut config = AxonFlowConfig::new(agent_url);
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
    let typed = client.typed_policies();
    let mut failures = 0;

    // What this deployment may author: consult it before publishing rather
    // than learning the edition's boundary from a refusal.
    println!("\n=== what this deployment may author ===");
    match typed.edition().await {
        Ok(edition) => {
            println!(
                "root={:?} max_documents={:?} persistence={:?}",
                edition.root, edition.max_documents, edition.persistence
            );
            if let Some(constructs) = &edition.constructs {
                println!(
                    "edition={:?} obligation families={:?}",
                    constructs.edition, constructs.obligation_families
                );
            }
        }
        Err(err) => {
            report(&err);
            failures += 1;
        }
    }

    // Validation reports every finding. A refused document is still a
    // successful validation: read `success` and the findings.
    println!("\n=== validate the document ===");
    match typed.validate(&document, Some(&fixtures)).await {
        Ok(validation) => {
            println!("success={}", validation.success);
            for f in &validation.findings {
                println!(
                    "  {} {} {}: {}",
                    f.severity,
                    f.code,
                    f.policy_id.as_deref().unwrap_or("-"),
                    f.detail.as_deref().unwrap_or("")
                );
            }
        }
        Err(err) => {
            report(&err);
            failures += 1;
        }
    }

    if std::env::var("AXONFLOW_TYPED_POLICY_PUBLISH").as_deref() == Ok("1") {
        println!("\n=== publish and activate ===");
        match typed.publish(&document, Some(&fixtures)).await {
            Ok(published) => {
                println!(
                    "published {} (version {:?})",
                    published.digest, published.version
                );
                // Activating a document that omits the organization template's
                // controls removes those controls for the organization, so the
                // report is printed before the activation.
                match (
                    &published.template_omissions,
                    &published.template_omissions_unavailable,
                ) {
                    (Some(omissions), _) => {
                        let omitted: Vec<&str> = omissions["omitted"]
                            .as_array()
                            .map(|ids| ids.iter().filter_map(Value::as_str).collect())
                            .unwrap_or_default();
                        println!(
                            "template omissions: {} of {} template controls: {}",
                            omitted.len(),
                            omissions["of"],
                            omitted.join(", ")
                        );
                    }
                    (None, Some(why)) => println!("template omissions: unavailable: {why}"),
                    (None, None) => println!("template omissions: none"),
                }
                match typed
                    .activate(&published.digest, Some("examples/typed_policies"))
                    .await
                {
                    Ok(activation) => {
                        println!("activated");
                        println!(
                            "  activation: {}",
                            Value::Object(activation.activation.clone())
                        );
                    }
                    Err(err) => {
                        report(&err);
                        failures += 1;
                    }
                }
            }
            // A refusal carries the platform's reason and, for a refused
            // document, the findings that refused it. Publishing was asked for,
            // so a refusal fails the run.
            Err(err) => {
                report(&err);
                failures += 1;
            }
        }
    }

    // The document in force is returned as the exact bytes that were signed.
    println!("\n=== the document in force ===");
    match typed.active().await {
        Ok(Some(active)) => println!(
            "{} signed bytes; document_id={}",
            active.source.len(),
            active.document["metadata"]["document_id"]
        ),
        Ok(None) => println!("nothing is active"),
        Err(err) => {
            report(&err);
            failures += 1;
        }
    }

    if failures > 0 {
        println!("\n{failures} step(s) failed");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
