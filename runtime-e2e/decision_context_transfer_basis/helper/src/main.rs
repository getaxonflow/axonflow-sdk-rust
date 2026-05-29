// Runtime proof — Rust SDK v0.6.0 surfaces the Decision Mode request context
// (platform #2509, epic #2508) and the pasal_56b_dpa transfer basis, against a
// live agent.
//
//   1. DecisionSummary.context / DecisionExplanation.context — the sanitized
//      request context a PEP attaches to a Decision Mode call, surfaced back
//      through list_decisions + explain_decision. We act as the PEP via a raw
//      POST /api/v1/decide (that endpoint is not SDK-wrapped per ADR-056), then
//      read the decision back through the SDK and assert context is populated.
//   2. AuditLogEntry transfer_basis = "pasal_56b_dpa" round-trips verbatim.
//
// Prints a `PASS:` line per assertion and exits 0 on success; non-zero on the
// first failure. Run via ../test.sh.

use std::collections::HashMap;

use axonflow_sdk_rust::{
    transfer_basis, AuditLogEntry, AxonFlowClient, AxonFlowConfig, ListDecisionsOptions,
};

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("FAIL: {e}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), String> {
    let endpoint =
        std::env::var("AXONFLOW_AGENT_URL").unwrap_or_else(|_| "http://localhost:8080".to_string());
    let client_id =
        std::env::var("AXONFLOW_TENANT_ID").unwrap_or_else(|_| "buku-e-rust-e2e".to_string());
    let secret =
        std::env::var("AXONFLOW_TENANT_SECRET").unwrap_or_else(|_| "buku-e-secret".to_string());

    let want: HashMap<&str, &str> = HashMap::from([
        ("x_ai_agent", "refund-bot"),
        ("x_session_id", "sess-buku-42"),
        ("x_leader_identity", "ops-lead"),
    ]);

    // 1. PEP: create a decision carrying request context (body 'context' map).
    let decision_id = create_decision(&endpoint, &client_id, &secret).await?;
    println!("PEP decide -> decision_id={decision_id}");

    let cfg = AxonFlowConfig::new(endpoint.clone()).with_auth(client_id.clone(), secret.clone());
    let client = AxonFlowClient::new(cfg).map_err(|e| format!("client init: {e}"))?;

    // 2. Read it back through the SDK.
    let rows = client
        .list_decisions(ListDecisionsOptions {
            limit: Some(5),
            ..Default::default()
        })
        .await
        .map_err(|e| format!("list_decisions: {e}"))?;
    let found = rows
        .iter()
        .find(|r| r.decision_id == decision_id)
        .ok_or_else(|| {
            format!(
                "list_decisions did not return {decision_id} (got {} rows)",
                rows.len()
            )
        })?;
    let ctx = found
        .context
        .as_ref()
        .ok_or_else(|| "list_decisions context is None".to_string())?;
    println!("SDK list_decisions -> context={ctx:?}");
    assert_superset(ctx, &want)?;
    println!(
        "PASS: list_decisions DecisionSummary.context populated with {} PEP-forwarded keys",
        ctx.len()
    );

    let exp = client
        .explain_decision(&decision_id)
        .await
        .map_err(|e| format!("explain_decision: {e}"))?;
    let exp_ctx = exp
        .context
        .as_ref()
        .ok_or_else(|| "explain_decision context is None".to_string())?;
    println!(
        "SDK explain_decision -> context={exp_ctx:?} context_truncated={}",
        exp.context_truncated
    );
    assert_superset(exp_ctx, &want)?;
    println!(
        "PASS: explain_decision returned full context (context_truncated={})",
        exp.context_truncated
    );

    // 3. transfer_basis = pasal_56b_dpa round-trip (Pasal 56(b)).
    let json = format!(
        r#"{{"id":"e2e-audit","timestamp":"2026-05-30T10:00:00Z","data_residency":"ID","transfer_basis":"{}"}}"#,
        transfer_basis::PASAL_56B_DPA
    );
    let entry: AuditLogEntry =
        serde_json::from_str(&json).map_err(|e| format!("parse audit: {e}"))?;
    let reserialized =
        serde_json::to_string(&entry).map_err(|e| format!("serialize audit: {e}"))?;
    let back: AuditLogEntry =
        serde_json::from_str(&reserialized).map_err(|e| format!("reparse audit: {e}"))?;
    if back.transfer_basis.as_deref() != Some("pasal_56b_dpa") {
        return Err(format!(
            "transfer_basis round-trip = {:?}, want pasal_56b_dpa",
            back.transfer_basis
        ));
    }
    println!("SDK AuditLogEntry round-trip -> {reserialized}");
    println!("PASS: AuditLogEntry.transfer_basis = \"pasal_56b_dpa\" round-trips verbatim");

    println!("ALL PASS: v0.6.0 context + pasal_56b_dpa verified through SDK runtime");
    Ok(())
}

fn assert_superset(
    got: &HashMap<String, String>,
    want: &HashMap<&str, &str>,
) -> Result<(), String> {
    for (k, v) in want {
        if got.get(*k).map(String::as_str) != Some(*v) {
            return Err(format!("context[{k}] = {:?}, want {v}", got.get(*k)));
        }
    }
    Ok(())
}

/// Acts as the PEP: the request context lives in the body's `context` map.
async fn create_decision(endpoint: &str, client_id: &str, secret: &str) -> Result<String, String> {
    use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
    let auth = base64_basic(client_id, secret);
    let body = serde_json::json!({
        "stage": "llm",
        "query": "summarize this support ticket",
        "target": {"type": "llm", "model": "gpt-4", "provider": "openai"},
        "context": {
            "x-ai-agent": "refund-bot",
            "x-session-id": "sess-buku-42",
            "x-leader-identity": "ops-lead"
        }
    });
    let resp = reqwest::Client::new()
        .post(format!("{endpoint}/api/v1/decide"))
        .header(CONTENT_TYPE, "application/json")
        .header("X-Client-ID", client_id)
        .header(AUTHORIZATION, format!("Basic {auth}"))
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("decide request: {e}"))?;
    let status = resp.status();
    let text = resp.text().await.map_err(|e| format!("decide body: {e}"))?;
    if !status.is_success() {
        return Err(format!("decide HTTP {status}: {text}"));
    }
    println!("server /decide response: {text}");
    let v: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("decide json: {e}"))?;
    v.get("decision_id")
        .and_then(|d| d.as_str())
        .map(str::to_string)
        .ok_or_else(|| format!("no decision_id in response: {text}"))
}

// Minimal base64 (standard alphabet) so the helper needs no extra dep.
fn base64_basic(client_id: &str, secret: &str) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let input = format!("{client_id}:{secret}");
    let bytes = input.as_bytes();
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((n >> 18) & 63) as usize] as char);
        out.push(TABLE[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            TABLE[((n >> 6) & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}
