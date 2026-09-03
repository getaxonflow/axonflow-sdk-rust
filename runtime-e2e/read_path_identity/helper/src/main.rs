//! Real-wire proof of the read-path per-user identity (platform #2922) through
//! the Rust SDK's own runtime, against a LIVE enterprise agent + orchestrator.
//!
//! What this asserts, and why each assertion cannot pass vacuously:
//!
//!  1. WRITE       three decisions through the real `/decide` plane as dev-a.
//!  2. LIST        as dev-a: the page must contain AT LEAST the three ids this
//!                 run wrote, each checked BY ID. Then DEV-B writes one and
//!                 dev-a's page must NOT grow — which is what separates
//!                 own-rows from a broken narrowing that returns the tenant.
//!  3. EXPLAIN     as dev-a: must carry the id asked for AND the context value
//!                 THIS RUN chose, so a populated-looking stub cannot satisfy it.
//!  4. NO IDENTITY the same list, unscoped: must be REFUSED as
//!                 `AxonFlowError::ReadScope` with `identity_missing`, not `[]`.
//!  5. OTHER USER  explain dev-a's decision as dev-b: must be refused, and must
//!                 NOT report a missing identity — dev-b presented one.
//!  6. MALFORMED / EXPIRED / WRONG-ORG: each must fail CLOSED, never degrade to
//!                 the tenant credential's visibility, and never echo the token.
//!  7. TENANT-WIDE as admin: must see dev-a's decision, which is what makes
//!                 step 5 falsifiable — a read broken for everyone also
//!                 "refuses dev-b".
//!  8. AS_USER     a derived client must be scoped to the identity it was
//!                 derived FOR. The Python sibling shipped exactly the bug this
//!                 catches: a derived client silently keeping the ORIGINAL
//!                 identity.
//!  9. NO LEAK     the token must appear in NO request reaching the telemetry
//!                 collector this driver hosts, and the collector must have
//!                 received something — otherwise the assertion is vacuous.
//! 10. OBSERVABLE  the platform must leave a record of the unscoped read.
//!
//! Identities are minted at `@example.com`, never `@axonflow.local`: the
//! platform reserves that whole domain (and `@axonflow.internal`) for SHARED,
//! non-personal identities and censuses them to nothing before scoping, so a
//! perfectly valid developer token minted there reads ZERO rows and reports
//! scope `none` — identical to presenting no token at all. `generate-jwt.sh`'s
//! own default (`demo-user@axonflow.local`) lands in the reserved domain.

use axonflow_sdk_rust::{
    AxonFlowClient, AxonFlowConfig, AxonFlowError, ListDecisionsOptions, ReadScope,
};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const WROTE: usize = 3;

fn fail(message: &str) -> ! {
    eprintln!("FAIL: {message}");
    std::process::exit(1);
}

fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| {
        fail(&format!(
            "{name} must be set (source /tmp/axonflow-e2e-env.sh after \
             ./scripts/setup-e2e-testing.sh enterprise)"
        ))
    })
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// The per-user HS256 JWT the platform's own validator requires — the same
/// claim set `scripts/generate-jwt.sh --kind user` emits.
///
/// Minted in-process rather than shelled out to, because the scoping assertions
/// need SEVERAL distinct identities and the setup script's single token is
/// `role=admin`, which short-circuits to tenant-wide and would make steps 4-8
/// untestable.
fn mint(secret: &str, email: &str, org_id: &str, role: &str, valid_for: i64, run_tag: &str) -> String {
    let issued = now() as i64;
    let header = URL_SAFE_NO_PAD.encode(r#"{"alg":"HS256","typ":"JWT"}"#);
    let payload = URL_SAFE_NO_PAD.encode(
        serde_json::json!({
            "iss": "axonflow-user-token-mint",
            "sub": email,
            "email": email,
            "user_id": email,
            "tenant_id": org_id,
            "org_id": org_id,
            "role": role,
            "region": "local",
            "jti": format!("{run_tag}-{}-{}", role, issued),
            "permissions": ["query", "llm", "mcp_query"],
            "iat": issued - 60,
            "nbf": issued - 60,
            "exp": issued + valid_for,
        })
        .to_string(),
    );
    let signing = format!("{header}.{payload}");
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("hmac key");
    mac.update(signing.as_bytes());
    format!("{signing}.{}", URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes()))
}

fn client(endpoint: &str, client_id: &str, secret: &str, user_token: Option<&str>) -> AxonFlowClient {
    let mut config = AxonFlowConfig::new(endpoint).with_auth(client_id, secret);
    config.user_token = user_token.map(str::to_string);
    AxonFlowClient::new(config).expect("client")
}

/// Drive the real `/decide` plane as a given identity, over raw HTTP.
///
/// `/api/v1/decide` reads identity from the request BODY (`user_token`), not
/// from `X-User-Token` — the write path and the read path are deliberately
/// different seams, which is exactly why the read path needed a surface of its
/// own. `X-Axonflow-Client` is sent because a driver that exercises the platform
/// should not be a phantom in the platform's own adoption metrics
/// (`axonflow_client_version_dropped_total{reason="absent"}` counts requests
/// that arrive without it).
async fn decide_as(
    endpoint: &str,
    client_id: &str,
    secret: &str,
    user_token: &str,
    index: usize,
    run_tag: &str,
) -> String {
    let body = serde_json::json!({
        "stage": "llm",
        "query": format!("summarize support ticket {index} for run {run_tag}"),
        "user_token": user_token,
        "target": {"type": "llm", "model": "gpt-4", "provider": "openai"},
        "context": {"x-session-id": run_tag, "x-ai-agent": "read-path-identity-e2e"},
    });
    let auth = base64::engine::general_purpose::STANDARD.encode(format!("{client_id}:{secret}"));
    let response = reqwest::Client::new()
        .post(format!("{endpoint}/api/v1/decide"))
        .header("Content-Type", "application/json")
        .header("X-Client-ID", client_id)
        .header("X-Axonflow-Client", "runtime-e2e-read-path-identity/1")
        .header("Authorization", format!("Basic {auth}"))
        .json(&body)
        .send()
        .await
        .unwrap_or_else(|e| fail(&format!("decide: {e}")));

    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        fail(&format!("decide HTTP {status}: {text}"));
    }
    let parsed: serde_json::Value =
        serde_json::from_str(&text).unwrap_or_else(|e| fail(&format!("decide body: {e}")));
    parsed["decision_id"]
        .as_str()
        .unwrap_or_else(|| fail(&format!("no decision_id in /decide response: {text}")))
        .to_string()
}

#[tokio::main]
async fn main() {
    let endpoint = std::env::var("AXONFLOW_AGENT_URL")
        .unwrap_or_else(|_| "http://localhost:8080".to_string());
    let client_id = env("AXONFLOW_CLIENT_ID");
    let secret = env("AXONFLOW_CLIENT_SECRET");
    let jwt_secret = env("AXONFLOW_JWT_SECRET");
    let orch = std::env::var("AXONFLOW_ORCH_CONTAINER")
        .unwrap_or_else(|_| "axonflow-orchestrator".to_string());

    // Makes every assertion specific to THIS run: the context value below is
    // unique per invocation, so "the explanation is populated" becomes "the
    // explanation carries the value this process chose".
    let run_tag = format!("s3-rust-{}", now());

    // A real listener standing in for the telemetry checkpoint — a THIRD PARTY.
    //
    // allow-mocks-here: not a stand-in for the system under test. It is the far
    // end of a request the SDK sends on its own initiative, and the assertion is
    // about what actually arrives there, which cannot be observed at all without
    // owning that end.
    let collected: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("collector bind");
    let collector_url = format!("http://{}/telemetry", listener.local_addr().unwrap());
    {
        let collected = Arc::clone(&collected);
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                let collected = Arc::clone(&collected);
                tokio::spawn(async move {
                    let io = hyper_util::rt::TokioIo::new(stream);
                    let service = hyper::service::service_fn(move |req: hyper::Request<hyper::body::Incoming>| {
                        let collected = Arc::clone(&collected);
                        async move {
                            use http_body_util::BodyExt;
                            let headers = format!("{:?}", req.headers());
                            let body = req
                                .into_body()
                                .collect()
                                .await
                                .map(|b| String::from_utf8_lossy(&b.to_bytes()).to_string())
                                .unwrap_or_default();
                            collected.lock().unwrap().push(format!("{headers}{body}"));
                            Ok::<_, std::convert::Infallible>(hyper::Response::new(
                                http_body_util::Full::new(hyper::body::Bytes::from(
                                    r#"{"status":"ok"}"#,
                                )),
                            ))
                        }
                    });
                    let _ = hyper::server::conn::http1::Builder::new()
                        .serve_connection(io, service)
                        .await;
                });
            }
        });
    }
    std::env::set_var("AXONFLOW_CHECKPOINT_URL", &collector_url);

    let dev_a = mint(&jwt_secret, &format!("dev-a-{run_tag}@example.com"), &client_id, "developer", 3600, &run_tag);
    let dev_b = mint(&jwt_secret, &format!("dev-b-{run_tag}@example.com"), &client_id, "developer", 3600, &run_tag);
    let admin = mint(&jwt_secret, &format!("admin-{run_tag}@example.com"), &client_id, "admin", 3600, &run_tag);
    let expired = mint(&jwt_secret, &format!("old-{run_tag}@example.com"), &client_id, "developer", -3600, &run_tag);
    let wrong_org = mint(&jwt_secret, &format!("out-{run_tag}@example.com"), &format!("other-org-{run_tag}"), "admin", 3600, &run_tag);
    let malformed = "not.a.jwt";

    // ============================================================= 1. WRITE
    // Three, not one: the floor in step 2 is "at least the number this run
    // wrote", and a floor of one is satisfied by almost any page.
    let mut written = Vec::new();
    for i in 0..WROTE {
        written.push(decide_as(&endpoint, &client_id, &secret, &dev_a, i, &run_tag).await);
    }
    println!("step 1 PASS: wrote {} decisions as dev-a: {written:?}", written.len());

    let as_dev_a = client(&endpoint, &client_id, &secret, Some(&dev_a));

    // The audit write is asynchronous; bound the wait and say so, so a later
    // assertion fails on SCOPE rather than on timing.
    let deadline = std::time::Instant::now() + Duration::from_secs(45);
    loop {
        if as_dev_a.explain_decision(&written[0]).await.is_ok() {
            break;
        }
        if std::time::Instant::now() >= deadline {
            fail(&format!(
                "the decision {} never became visible to the identity that wrote it within 45s — \
                 the audit write did not land, so every read assertion below would be about \
                 timing, not scope",
                written[0]
            ));
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }

    // ============================================================== 2. LIST
    let rows = as_dev_a
        .list_decisions(ListDecisionsOptions { limit: Some(50), ..Default::default() })
        .await
        .unwrap_or_else(|e| fail(&format!("step 2: list as dev-a: {e}")));
    if rows.len() < WROTE {
        fail(&format!(
            "step 2: dev-a's page has {} rows, want at least the {WROTE} this run wrote — a page \
             smaller than what we just wrote cannot be a correctly-scoped read",
            rows.len()
        ));
    }
    for id in &written {
        if !rows.iter().any(|r| &r.decision_id == id) {
            fail(&format!("step 2: dev-a's page does not contain {id}, which dev-a wrote"));
        }
    }
    // The floor alone cannot tell own-rows from tenant-wide: a broken narrowing
    // returning the WHOLE tenant would clear it comfortably.
    decide_as(&endpoint, &client_id, &secret, &dev_b, 99, &run_tag).await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    let after = as_dev_a
        .list_decisions(ListDecisionsOptions { limit: Some(50), ..Default::default() })
        .await
        .unwrap_or_else(|e| fail(&format!("step 2: re-list as dev-a: {e}")));
    if after.len() != rows.len() {
        fail(&format!(
            "step 2: dev-a's page grew from {} to {} rows after DEV-B wrote one — the read is not \
             narrowed to dev-a's own rows, so every scoping assertion below is vacuous",
            rows.len(),
            after.len()
        ));
    }
    println!(
        "step 2 PASS: dev-a's page ({} rows) is exactly its own; dev-b's write did not appear",
        rows.len()
    );

    // =========================================================== 3. EXPLAIN
    let explanation = as_dev_a
        .explain_decision(&written[0])
        .await
        .unwrap_or_else(|e| fail(&format!("step 3: explain as dev-a: {e}")));
    if explanation.decision_id != written[0] {
        fail(&format!("step 3: explanation decision_id = {}", explanation.decision_id));
    }
    // A field THIS RUN controls. "Non-empty" would pass on any stub.
    let session = explanation
        .context
        .as_ref()
        .and_then(|c| c.get("x_session_id"))
        .map(String::as_str);
    if session != Some(run_tag.as_str()) {
        fail(&format!(
            "step 3: explanation context[x_session_id] = {session:?}, want {run_tag:?} — the \
             explanation must carry the value this run wrote, not merely be non-empty"
        ));
    }
    println!(
        "step 3 PASS: explanation for {} is populated and carries this run's context \
         (x_session_id={run_tag}, decision={})",
        written[0], explanation.decision
    );

    // ======================================================= 4. NO IDENTITY
    let anon = client(&endpoint, &client_id, &secret, None);
    match anon
        .list_decisions(ListDecisionsOptions { limit: Some(50), ..Default::default() })
        .await
    {
        Err(AxonFlowError::ReadScope(refusal)) if refusal.identity_missing() => {
            println!("step 4 PASS: the unscoped list is refused, not answered empty");
        }
        Err(AxonFlowError::ReadScope(refusal)) => fail(&format!(
            "step 4: the unscoped list was refused with scope {}, want none",
            refusal.scope
        )),
        Err(other) => fail(&format!("step 4: want a typed ReadScope refusal, got {other}")),
        Ok(rows) if !rows.is_empty() => fail(&format!(
            "step 4: the unscoped list returned {} rows — this stack is not enforcing role-scoped \
             reads, so every scoping assertion in this driver is vacuous",
            rows.len()
        )),
        Ok(_) => fail(
            "step 4: the unscoped list returned 0 rows and NO error. That is the defect: the read \
             could not have returned a row, and reporting it as an empty page is a confident lie",
        ),
    }

    // ======================================================== 5. OTHER USER
    match client(&endpoint, &client_id, &secret, Some(&dev_b))
        .explain_decision(&written[0])
        .await
    {
        Err(AxonFlowError::ReadScope(refusal)) => {
            if refusal.identity_missing() {
                fail(&format!(
                    "step 5: dev-b's refusal reports a MISSING identity; dev-b presented one. \
                     Reporting the wrong cause is the confidently-wrong-diagnosis class (scope={})",
                    refusal.scope
                ));
            }
            if refusal.scope != ReadScope::OwnRows {
                fail(&format!("step 5: dev-b's refusal reports scope {}, want own-rows", refusal.scope));
            }
            println!("step 5 PASS: dev-b is refused dev-a's decision, with the RIGHT cause");
        }
        Err(other) => fail(&format!("step 5: dev-b's refusal is {other}, want a typed ReadScope refusal")),
        Ok(_) => fail(&format!(
            "step 5: dev-b explained dev-a's decision {} — that is the cross-user leak #2922 closed",
            written[0]
        )),
    }

    // ============================ 6. MALFORMED / EXPIRED / WRONG-ORG
    // The common real-world state, not the exception. Each must fail CLOSED: a
    // rejected token must never degrade into "no token", which would hand the
    // caller the tenant credential's visibility.
    for (name, bad) in [("malformed", malformed), ("expired", expired.as_str()), ("another org", wrong_org.as_str())] {
        match client(&endpoint, &client_id, &secret, Some(bad))
            .list_decisions(ListDecisionsOptions { limit: Some(5), ..Default::default() })
            .await
        {
            Err(AxonFlowError::ReadScope(_)) => fail(&format!(
                "step 6 ({name}): a REJECTED token was reported as a scoping outcome, which means \
                 it degraded to the unscoped path instead of failing closed"
            )),
            Err(e) => {
                let text = e.to_string();
                if !text.contains("401") {
                    fail(&format!("step 6 ({name}): want a 401, got: {text}"));
                }
                if text.contains(bad) {
                    fail(&format!("step 6 ({name}): the error message echoes the rejected credential"));
                }
                println!("step 6 PASS ({name}): rejected fail-closed with 401, credential not echoed");
            }
            Ok(_) => fail(&format!(
                "step 6 ({name}): a rejected per-user token produced a SUCCESSFUL read. A \
                 present-but-invalid identity must fail closed, never degrade to the unscoped path"
            )),
        }
    }

    // ======================================================== 7. TENANT-WIDE
    // Without this, step 5 is unfalsifiable: a read broken for everyone would
    // also "refuse dev-b".
    let as_admin = client(&endpoint, &client_id, &secret, Some(&admin));
    let admin_explanation = as_admin
        .explain_decision(&written[0])
        .await
        .unwrap_or_else(|e| fail(&format!("step 7: an admin identity could not explain dev-a's decision: {e}")));
    if admin_explanation.decision_id != written[0] {
        fail("step 7: admin explanation carried the wrong decision_id");
    }
    println!("step 7 PASS: an admin identity reads tenant-wide — step 5's refusal is scoping, not breakage");

    // ============================================================ 8. AS_USER
    match as_admin.as_user(&dev_b).explain_decision(&written[0]).await {
        Err(AxonFlowError::ReadScope(refusal)) if refusal.scope == ReadScope::OwnRows => {
            println!("step 8 PASS: as_user(dev-b) is scoped to dev-b, not to the admin it derived from");
        }
        Err(other) => fail(&format!("step 8: as_user(dev-b) failed with {other}, want own-rows")),
        Ok(_) => fail(
            "step 8: as_user(dev-b) read dev-a's decision — the derived client kept the ADMIN \
             identity, which is the silent widening as_user exists to prevent",
        ),
    }
    // ...and the client it came from is unchanged.
    if as_admin.explain_decision(&written[0]).await.is_err() {
        fail("step 8: as_user mutated the client it was derived from");
    }

    // ============================================================ 9. NO LEAK
    tokio::time::sleep(Duration::from_secs(1)).await;
    let seen = collected.lock().unwrap().clone();
    if seen.is_empty() {
        fail(
            "step 9: the telemetry collector received NOTHING, so its leak assertions asserted \
             nothing. AXONFLOW_TELEMETRY must be on, the stamp must be parked (test.sh does both) \
             and the heartbeat must have fired.",
        );
    }
    for (name, token) in [("dev-a", &dev_a), ("dev-b", &dev_b), ("admin", &admin)] {
        for (i, request) in seen.iter().enumerate() {
            if request.contains(token.as_str()) {
                fail(&format!("step 9: the {name} token reached the telemetry collector in request {i}"));
            }
        }
    }
    println!("step 9 PASS: no token in any of {} telemetry requests", seen.len());

    // ========================================================= 10. OBSERVABLE
    // A fail-closed read the platform leaves no trace of is a read nobody can
    // audit; "it failed closed" is only half the property.
    let logs = std::process::Command::new("docker")
        .args(["logs", "--tail", "500", &orch])
        .output();
    match logs {
        Ok(out) if out.status.success() => {
            let text = format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            if !text.contains("[read-scope]") {
                fail(
                    "step 10: the orchestrator logged no [read-scope] line for the unscoped read \
                     in step 4. The read failed closed but left no platform-side record of having \
                     done so",
                );
            }
            println!("step 10 PASS: the orchestrator recorded the unscoped read");
        }
        // Loudly inconclusive, never a silent pass.
        _ => fail(&format!(
            "step 10: could not read {orch}'s logs to confirm the platform recorded the unscoped \
             read. Set AXONFLOW_ORCH_CONTAINER, or run where the stack's logs are reachable — an \
             unverified observability claim is not evidence"
        )),
    }

    // ================================================ 11. THE NON-READ ROUTES
    // Round 2's behaviour change, asserted on the live platform rather than
    // claimed in a doc comment.
    //
    // The identity is stamped in `dispatch`, so it now rides EVERY request, not
    // just the two role-scoped reads. That is the shape the other four SDKs
    // already had, and it has a consequence the README states and this step is
    // what makes that statement evidence: the agent VALIDATES X-User-Token on
    // every route it proxies, so a bad identity turns an ordinary non-read call
    // into a 401 instead of merely unscoping a read.
    //
    // Both directions are checked. A step that only asserted the 401 would pass
    // just as happily on a stack where `list_connectors` is broken for
    // everyone, and would then be reporting an outage as a security property.
    let connectors_as_dev_a = client(&endpoint, &client_id, &secret, Some(&dev_a))
        .list_connectors()
        .await;
    if let Err(e) = &connectors_as_dev_a {
        fail(&format!(
            "step 11: list_connectors failed for a VALID identity ({e}). This is the control: \
             without it, the refusal asserted below would hold on a stack where the route is \
             simply down, and an outage would read as an access-control property"
        ));
    }
    println!(
        "step 11 PASS (control): a non-read route succeeds under a valid identity, so it is \
         reachable and the refusal below is about the identity"
    );

    match client(&endpoint, &client_id, &secret, Some(malformed))
        .list_connectors()
        .await
    {
        Err(AxonFlowError::ApiError { status: 401, message }) => {
            if message.contains(malformed) {
                fail("step 11: the platform echoed the rejected credential back in its error body");
            }
            println!(
                "step 11 PASS: a non-read route is refused 401 under a malformed identity — the \
                 identity reaches every proxied route, and a stale token fails CLOSED there \
                 rather than silently widening to the process's own authority"
            );
        }
        Err(other) => fail(&format!(
            "step 11: want a 401 from the non-read route under a malformed identity, got {other}"
        )),
        Ok(_) => fail(
            "step 11: a malformed identity read a non-read route SUCCESSFULLY. Either the \
             identity is not reaching that route — the round-2 defect, in which case a client \
             derived with as_user runs it as the PROCESS — or the platform is not validating it \
             there. Both are reportable; neither is a pass",
        ),
    }

    println!("\nALL PASS: read-path identity verified end to end through the Rust SDK runtime");
}
