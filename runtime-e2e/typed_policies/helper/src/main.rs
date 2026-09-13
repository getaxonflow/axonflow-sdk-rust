//! Runtime proof that the Rust SDK's typed policy authoring client works
//! against a real agent and orchestrator. See `../README.md`.
//!
//! It drives the six operations of `client.typed_policies()` in order, on a
//! fresh stack, and asserts on what the platform answered. Nothing is mocked.

use axonflow_sdk_rust::{
    AuthoringFinding, AxonFlowClient, AxonFlowConfig, AxonFlowError, TypedPolicyRefusal,
};
use serde_json::Value;
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

/// The body the platform's own route test proves publishable, vendored
/// byte-identical to the other SDKs'.
const PUBLISH_BODY: &str = include_str!("../../../../testdata/typed_policy_publish_body.json");

struct Run {
    failures: Vec<String>,
}

impl Run {
    fn check(&mut self, ok: bool, what: &str) {
        println!("{}: {what}", if ok { "PASS" } else { "FAIL" });
        if !ok {
            self.failures.push(what.to_string());
        }
    }
}

fn client(agent: &str, credentials: bool) -> AxonFlowClient {
    let mut config = AxonFlowConfig::new(agent);
    if credentials {
        let id = std::env::var("AXONFLOW_CLIENT_ID").unwrap_or_else(|_| "runtime-e2e".into());
        let secret = std::env::var("AXONFLOW_CLIENT_SECRET").unwrap_or_default();
        config = config.with_auth(id, secret);
    }
    config.retry.enabled = false;
    AxonFlowClient::new(config).expect("the client builds")
}

fn refusal(err: &AxonFlowError) -> Option<&TypedPolicyRefusal> {
    match err {
        AxonFlowError::TypedPolicyRefusal(r) => Some(r),
        _ => None,
    }
}

fn has_finding(findings: &[AuthoringFinding], code: &str, severity: &str, policy: &str) -> bool {
    findings
        .iter()
        .any(|f| f.code == code && f.severity == severity && f.policy_id.as_deref() == Some(policy))
}

fn finish(run: &Run) -> ExitCode {
    if run.failures.is_empty() {
        println!("\nPASS: typed_policies");
        ExitCode::SUCCESS
    } else {
        println!(
            "\nFAIL: typed_policies ({} assertion(s))",
            run.failures.len()
        );
        for f in &run.failures {
            println!("  - {f}");
        }
        ExitCode::FAILURE
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    let agent =
        std::env::var("AXONFLOW_AGENT_URL").unwrap_or_else(|_| "http://localhost:8080".into());
    println!("agent: {agent}");
    let client_id = std::env::var("AXONFLOW_CLIENT_ID").unwrap_or_else(|_| "runtime-e2e".into());
    let mut run = Run {
        failures: Vec::new(),
    };
    let body: Value = serde_json::from_str(PUBLISH_BODY).expect("the vendored publish body");
    let fixtures: Vec<Value> = body["fixtures"].as_array().expect("fixtures").clone();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("a clock after 1970")
        .as_nanos();
    let document_id = format!("sdk-rust-e2e-{:012x}", nanos & 0xffff_ffff_ffff);
    let mut document = body["document"].clone();
    document["metadata"]["document_id"] = Value::String(document_id.clone());

    let c = client(&agent, true);
    let typed = c.typed_policies();

    println!("== nothing active yet");
    match typed.active().await {
        Ok(None) => run.check(true, "active() is None before any activation"),
        Ok(Some(a)) => run.check(
            false,
            &format!(
                "active() is None before any activation (a document is active: {}; run this on a fresh stack)",
                a.document["metadata"]["document_id"]
            ),
        ),
        Err(e) => run.check(false, &format!("active() before any activation: {e}")),
    }

    println!("== edition and system");
    let edition = match typed.edition().await {
        Ok(e) => e,
        Err(e) => {
            run.check(false, &format!("edition(): {e}"));
            return finish(&run);
        }
    };
    println!(
        "  edition: catalog={:?} root={:?} max_documents={:?} persistence={:?} constructs.edition={:?}",
        edition.catalog,
        edition.root,
        edition.max_documents,
        edition.persistence,
        edition.constructs.as_ref().and_then(|c| c.edition.clone())
    );
    run.check(
        edition.success && edition.root.as_deref() == Some("organization"),
        "edition() reports the root",
    );
    run.check(
        edition.constructs.is_some(),
        "edition() reports the construct boundary",
    );
    match typed.system().await {
        Ok(system) => {
            println!(
                "  system: root={:?} version={:?} digest={:?} controls={} assurance_counts={:?}",
                system.root,
                system.version,
                system.digest,
                system.controls.len(),
                system.assurance_counts
            );
            run.check(
                system.digest.as_deref().is_some_and(|d| !d.is_empty())
                    && !system.controls.is_empty(),
                "system() returns the shipped corpus",
            );
        }
        Err(e) => run.check(false, &format!("system(): {e}")),
    }

    println!("== validate, publish, activate");
    match typed.validate(&document, Some(&fixtures)).await {
        Ok(v) => {
            println!(
                "  validate: success={} findings={:?}",
                v.success, v.findings
            );
            run.check(v.success, "the document validates clean");
        }
        Err(e) => run.check(false, &format!("validate(): {e}")),
    }
    let published = match typed.publish(&document, Some(&fixtures)).await {
        Ok(p) => p,
        Err(e) => {
            run.check(false, &format!("publish(): {e}"));
            return finish(&run);
        }
    };
    println!(
        "  publish: digest={} version={:?}",
        published.digest, published.version
    );
    run.check(
        !published.digest.is_empty(),
        "publish() returns the artifact digest",
    );
    run.check(
        published.version == Some(1),
        "the first publication of a new document is version 1",
    );
    match typed
        .activate(&published.digest, Some("sdk-rust runtime proof"))
        .await
    {
        Ok(a) => {
            println!(
                "  activate: success={} activation={:?}",
                a.success, a.activation
            );
            run.check(a.success, "activate() promotes the digest");
        }
        Err(e) => run.check(false, &format!("activate(): {e}")),
    }

    println!("== the document in force");
    match typed.active().await {
        Ok(Some(active)) => {
            let metadata = &active.document["metadata"];
            println!(
                "  active: document_id={} author={}",
                metadata["document_id"], metadata["author"]
            );
            run.check(true, "active() returns the document in force");
            run.check(
                metadata["document_id"] == Value::String(document_id.clone()),
                "active() is the document just activated",
            );
            // The signed source must carry the policies that were published:
            // compared by id against the request, not against a parse of the
            // same bytes.
            let ids = |doc: &Value| -> Vec<String> {
                doc["policy"]["policies"]
                    .as_array()
                    .map(|all| {
                        all.iter()
                            .filter_map(|p| p["id"].as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default()
            };
            let published_ids = ids(&document);
            println!("  active policy ids: {:?}", ids(&active.document));
            run.check(
                !published_ids.is_empty() && ids(&active.document) == published_ids,
                "the document in force carries the policies that were published",
            );
            // The author is the caller the agent stamped, never the name the
            // request carried. On Community that caller is the API client: a
            // `Client` principal in the api-credential realm, named by the client
            // id this proof presents.
            let author = &metadata["author"];
            let requested = &document["metadata"]["author"];
            let local = author["local"].as_str().unwrap_or("");
            println!(
                "  author: type={} local={local:?} (the request named {})",
                author["type"], requested["local"]
            );
            let community = edition
                .constructs
                .as_ref()
                .and_then(|c| c.edition.as_deref())
                == Some("community");
            let stamped = if community {
                author["type"] == "Client"
                    && author["qualifier"] == "axonflow-api-credential"
                    && local == client_id
            } else {
                author["type"].as_str().is_some_and(|t| !t.is_empty()) && !local.is_empty()
            };
            run.check(
                stamped && Some(local) != requested["local"].as_str(),
                "the platform signed the caller as author, not the name in the request",
            );
        }
        Ok(None) => run.check(false, "active() returns the document in force (got None)"),
        Err(e) => run.check(false, &format!("active() after activation: {e}")),
    }

    println!("== typed refusals");
    match typed.activate(&published.digest, None).await {
        Err(ref e) if refusal(e).is_some() => {
            let r = refusal(e).expect("checked");
            println!(
                "  re-activate: status={} reason={:?} error={}",
                r.status, r.reason, r.message
            );
            run.check(
                r.status == 409 && r.reason.as_deref() == Some("activation_refused"),
                "re-activating the active digest is a typed 409 activation_refused",
            );
        }
        other => run.check(
            false,
            &format!("re-activating the active digest is refused (got {other:?})"),
        ),
    }
    match typed.publish(&document, Some(&[])).await {
        Err(ref e) if refusal(e).is_some() => {
            let r = refusal(e).expect("checked");
            println!(
                "  publish without fixtures: status={} reason={:?} error={}",
                r.status, r.reason, r.message
            );
            run.check(
                r.status == 422
                    && r.reason.as_deref() == Some("publication_refused")
                    && r.message.contains("declares no fixtures"),
                "publishing with no fixtures is a typed 422 publication_refused naming the cause",
            );
        }
        other => run.check(
            false,
            &format!("publishing with no fixtures is refused (got {other:?})"),
        ),
    }

    println!("== a document the save-time checks reject");
    let mut unregistered = document.clone();
    unregistered["metadata"]["document_id"] = Value::String(format!("{document_id}-unregistered"));
    unregistered["policy"]["policies"][0]["actions"]["actions"][0]["local"] =
        Value::String("tool.not_registered".into());
    match typed.validate(&unregistered, Some(&fixtures)).await {
        Ok(v) => {
            println!(
                "  validate: success={} findings={:?}",
                v.success, v.findings
            );
            run.check(
                !v.success
                    && has_finding(
                        &v.findings,
                        "ACTION_NOT_REGISTERED",
                        "reject",
                        "grant.refund",
                    ),
                "validate() answers an unregistered action with the platform's rejecting finding",
            );
        }
        Err(e) => run.check(false, &format!("validate() (unregistered): {e}")),
    }
    match typed.publish(&unregistered, Some(&fixtures)).await {
        Err(ref e) if refusal(e).is_some() => {
            let r = refusal(e).expect("checked");
            println!(
                "  publish: status={} reason={:?} findings={:?}",
                r.status, r.reason, r.findings
            );
            run.check(
                r.status == 422
                    && r.reason.as_deref() == Some("document_refused")
                    && has_finding(
                        &r.findings,
                        "ACTION_NOT_REGISTERED",
                        "reject",
                        "grant.refund",
                    ),
                "publishing it is a typed 422 document_refused carrying that finding",
            );
        }
        other => run.check(
            false,
            &format!(
                "publishing a document the save-time checks reject is refused (got {other:?})"
            ),
        ),
    }

    println!("== no credentials (observed, not asserted)");
    // What a Community agent answers a caller that presents no credentials is
    // the deployment's business, not this SDK's; it is printed for the record.
    match client(&agent, false).typed_policies().edition().await {
        Ok(e) => println!(
            "  OBSERVED: edition without credentials: success={} root={:?}",
            e.success, e.root
        ),
        Err(e) => println!("  OBSERVED: edition without credentials: {e}"),
    }

    finish(&run)
}
