// Runtime proof — the AuthZEN-native surface, driven through the SDK's real
// public API against a live agent. No mocks, no stubbed transport.
//
// WHAT THIS PROVES THAT THE UNIT SUITE CANNOT.
//
//   1. AGREEMENT. POST /api/v1/access/evaluation is an ADAPTER over the
//      evaluation that serves POST /api/v1/decide. A stubbed transport can
//      assert what the client does with a given body; only a live stack can
//      assert that the two surfaces agree about a real policy decision, in both
//      the allow and the deny direction.
//
//   2. BOTH SIDES NAME THE SAME MEMBER. The SDK refuses an incomplete subject
//      locally and the server refuses the same bytes on the wire. A unit test
//      can pin the local half; only a live server can establish that the two
//      name the SAME member.
//
//      The CODE is asserted too, but not for equality - because they are not
//      equal, and that is not a defect. This client knows only that a required
//      member is missing (`incomplete_evaluation`); the server additionally
//      knows which values it can evaluate and narrows the same condition to
//      `unsupported_subject` with a `supported` list. What IS asserted is that
//      the server's code is one this build knows: a code outside the closed
//      enumeration means the contract moved and this SDK cannot read the
//      refusal, which no unit test can discover.
//
//   3. THE BARE-BOOLEAN CASE IS REAL. The SDK refuses a 200 that carries no
//      profile payload. That guard is only worth anything if a server can
//      actually produce such a body - so this sends one un-negotiated request
//      and asserts the response has no `context`.
//
//   4. AN UNRESOLVABLE ATTRIBUTE NEVER REACHES THE NETWORK. Asserted by
//      pointing the real client at a port nothing is listening on: an
//      `Unresolved` error from that client is proof the check ran before any
//      I/O, which no amount of stubbing can establish. A transport error there
//      would mean the envelope had already been handed to the network.
//
// Prints one line per assertion. EXPECTED_ASSERTIONS must equal the number
// that ran: a suite whose checks stop executing prints no failures and exits 0,
// which is indistinguishable from success.

use axonflow_sdk_rust::authzen::{
    Attribute, AuthZenAction, AuthZenBulk, AuthZenDecision, AuthZenErrorCode,
    AuthZenEvaluationError, AuthZenOperationalState, AuthZenRequest, AuthZenResource,
    AuthZenSubject, AUTHZEN_PATH, AUTHZEN_PROFILE_HEADER, AUTHZEN_PROFILE_V1,
};
use axonflow_sdk_rust::{AxonFlowClient, AxonFlowConfig};
use base64::Engine as _;

const EXPECTED_ASSERTIONS: usize = 13;

/// A query the default community policy set permits.
const ALLOWED_QUERY: &str = "what is our refund policy?";
/// A query the default community policy set denies (SQL injection).
const DENIED_QUERY: &str = "'; DROP TABLE users; --";

struct Run {
    /// How many assertions actually EXECUTED. Compared against `expected` at the
    /// end - a suite whose checks stop running prints no failures and exits 0,
    /// which is indistinguishable from success.
    ran: usize,
    /// How many of those FAILED, tracked separately so a run cannot report a
    /// full floor while every assertion inside it was red.
    failed: usize,
    expected: usize,
}

impl Run {
    fn pass(&mut self, what: &str) {
        self.ran += 1;
        println!("  PASS: {what}");
    }

    fn fail(&mut self, what: &str, detail: String) {
        self.ran += 1;
        self.failed += 1;
        println!("  FAIL: {what} — {detail}");
    }

    /// A prerequisite missing for ONE assertion, discovered after others have
    /// run. It lowers the floor by exactly one rather than exiting, so earlier
    /// failures are still reported and a shrunken run is still loud.
    fn skip(&mut self, what: &str, why: &str) {
        self.expected -= 1;
        println!("  SKIP: {what} ({why})");
    }

    fn check(&mut self, what: &str, ok: Result<(), String>) {
        match ok {
            Ok(()) => self.pass(what),
            Err(e) => self.fail(what, e),
        }
    }
}

#[tokio::main]
async fn main() {
    let agent = std::env::var("AXONFLOW_AGENT_URL")
        .unwrap_or_else(|_| "http://localhost:8080".to_string());
    let client_id =
        std::env::var("AXONFLOW_CLIENT_ID").unwrap_or_else(|_| "s2-authzen-rust-e2e".to_string());
    let secret = std::env::var("AXONFLOW_CLIENT_SECRET").unwrap_or_default();

    println!("=== runtime-e2e: authzen_evaluation (Rust SDK) ===");
    println!("agent: {agent}");

    let client = match AxonFlowClient::new(AxonFlowConfig {
        endpoint: agent.clone(),
        client_id: Some(client_id.clone()),
        client_secret: if secret.is_empty() {
            None
        } else {
            Some(secret.clone())
        },
        ..Default::default()
    }) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("FAIL: the SDK client could not be built: {e}");
            std::process::exit(1);
        }
    };

    let mut run = Run {
        ran: 0,
        failed: 0,
        expected: EXPECTED_ASSERTIONS,
    };

    // --- 1 / 2: a decision in both directions, through the SDK ------------
    let allow = evaluate_query(&client, ALLOWED_QUERY).await;
    run.check(
        "an evaluable request yields a readable ALLOW",
        allow.as_ref().map_err(|e| e.to_string()).and_then(|d| {
            expect(d.allowed(), "allowed() was false")?;
            expect(
                d.state() == &AuthZenOperationalState::Allow,
                &format!("state was {}", d.state()),
            )?;
            expect(!d.decision_id().is_empty(), "decision_id was empty")
        }),
    );

    let deny = evaluate_query(&client, DENIED_QUERY).await;
    run.check(
        "a policy-denied request yields a readable DENY, not an error",
        deny.as_ref().map_err(|e| e.to_string()).and_then(|d| {
            expect(!d.allowed(), "allowed() was true")?;
            expect(
                d.state() == &AuthZenOperationalState::Deny,
                &format!("state was {}", d.state()),
            )
        }),
    );

    // --- 3 / 4: agreement with the legacy Decision API --------------------
    //
    // The release constraint is that this surface answers with the SAME
    // evaluation. Agreement in ONE direction would be satisfied by a route that
    // always allows, so both are asserted.
    for (query, want_authzen_allow, label) in [
        (ALLOWED_QUERY, true, "allow"),
        (DENIED_QUERY, false, "deny"),
    ] {
        let what = format!("the AuthZEN verdict agrees with /api/v1/decide ({label})");
        match decide_verdict(&agent, &client_id, &secret, query).await {
            Err(e) => run.fail(&what, e),
            Ok(verdict) => {
                let legacy_allows = verdict == "allow";
                let authzen = evaluate_query(&client, query).await;
                match authzen {
                    Err(e) => run.fail(&what, e.to_string()),
                    Ok(d) => run.check(
                        &what,
                        expect(
                            d.allowed() == legacy_allows && d.allowed() == want_authzen_allow,
                            &format!(
                                "authzen allowed={} state={} but /decide verdict={verdict}",
                                d.allowed(),
                                d.state()
                            ),
                        ),
                    ),
                }
            }
        }
    }

    // --- 5: several preconditions, one decision ---------------------------
    let bulk = client
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
            .with_query(Attribute::known(ALLOWED_QUERY)),
        )
        .await;
    run.check(
        "a plural envelope yields ONE decision over two preconditions",
        bulk.map_err(|e| e.to_string()).and_then(|d| {
            expect(!d.decision_id().is_empty(), "no decision_id")?;
            expect(
                d.state().is_known(),
                &format!("unreadable state {}", d.state()),
            )
        }),
    );

    // --- 6 / 7 / 8: the three attribute states, against the real server ---
    //
    // ABSENT is resolved data and evaluates; KNOWN reaches the server and is
    // refused by name; UNKNOWN never leaves the process. Three OBSERVABLY
    // different outcomes for one member, which is the whole argument for the
    // three-valued type.
    let mut absent_subject = AuthZenSubject::new("gateway", "llm-gateway-01");
    absent_subject.properties.insert_absent("department");
    let absent = client
        .evaluate(request_with(absent_subject, ALLOWED_QUERY))
        .await;
    run.check(
        "an ABSENT attribute is omitted and the request is evaluated",
        absent
            .map_err(|e| e.to_string())
            .and_then(|d| expect(d.allowed(), "the evaluation did not allow")),
    );

    let mut known_subject = AuthZenSubject::new("gateway", "llm-gateway-01");
    known_subject
        .properties
        .insert_known("department", "finance");
    let known = client
        .evaluate(request_with(known_subject, ALLOWED_QUERY))
        .await;
    run.check(
        "a KNOWN attribute reaches the server and is refused BY NAME",
        expect_refusal(
            known,
            AuthZenErrorCode::UnevaluableAttribute,
            "/evaluation/subject/properties",
            false,
        ),
    );

    // Pointed at a port nothing is listening on. A typed refusal from THIS
    // client is proof the check ran before any I/O; a transport error would
    // mean the envelope had already been handed to the network.
    let offline = AxonFlowClient::new(AxonFlowConfig {
        endpoint: "http://127.0.0.1:1".to_string(),
        client_id: Some(client_id.clone()),
        ..Default::default()
    });
    match offline {
        Err(e) => run.fail(
            "an UNKNOWN attribute is refused before any network I/O",
            e.to_string(),
        ),
        Ok(offline) => {
            let mut unknown_subject = AuthZenSubject::new("gateway", "llm-gateway-01");
            unknown_subject
                .properties
                .insert_unknown("department", "the directory timed out after 2s");
            let unknown = offline
                .evaluate(request_with(unknown_subject, ALLOWED_QUERY))
                .await;
            run.check(
                "an UNKNOWN attribute is refused before any network I/O",
                match unknown {
                    Ok(d) => Err(format!(
                        "expected a refusal, got a decision (allowed={})",
                        d.allowed()
                    )),
                    Err(AuthZenEvaluationError::Unresolved { pointer, .. }) => expect(
                        pointer == "/evaluation/subject/properties/department",
                        &format!("pointer was {pointer:?}"),
                    ),
                    Err(other) => Err(format!(
                        "expected an unresolved-attribute error, got {other}"
                    )),
                },
            );
        }
    }

    // --- 9: the SDK and the server name the SAME member -------------------
    let mut typeless = AuthZenSubject::new("gateway", "llm-gateway-01");
    typeless.r#type = String::new();
    let local = client
        .evaluate(request_with(typeless, ALLOWED_QUERY))
        .await
        .err()
        .and_then(|e| {
            e.as_refusal()
                .and_then(|r| r.pointer.clone().map(|p| (p, r.code.to_string())))
        });
    let remote = raw_refusal(
        &agent,
        &client_id,
        &secret,
        serde_json::json!({
            "evaluation": {
                "subject": {"id": "llm-gateway-01"},
                "action": {"name": "llm.completion"},
                "resource": {"type": "llm", "id": "llm"},
                "context": {"args": {"query": ALLOWED_QUERY}}
            }
        }),
    )
    .await;
    run.check(
        "the SDK's local refusal names the same member the server names",
        match (&local, &remote) {
            (Some((l, _)), Ok((Some(r), _))) => expect(
                l == r && l == "/evaluation/subject/type",
                &format!("local pointer {l:?} vs server pointer {r:?}"),
            ),
            (l, r) => Err(format!("local={l:?} remote={r:?}")),
        },
    );

    // The code is READ, not equated. It is reported either way so a divergence
    // that matters - a code this build cannot name - is visible in the log
    // rather than hidden behind a pointer-only assertion.
    run.check(
        "the server's refusal code is one this build knows",
        match (&local, &remote) {
            (Some((_, l)), Ok((_, r))) => {
                println!("       local code={l}  server code={r}");
                expect(
                    AuthZenErrorCode::from(r.clone()).is_known(),
                    &format!("the server sent {r:?}, which is not in this build's enumeration"),
                )
            }
            (l, r) => Err(format!("local={l:?} remote={r:?}")),
        },
    );

    // --- 10: the bare boolean an un-negotiated caller receives ------------
    run.check(
        "an un-negotiated request really does come back with NO profile payload",
        raw_unnegotiated_has_no_context(&agent, &client_id, &secret).await,
    );

    // --- 11: an auth failure stays observable -----------------------------
    //
    // Needs a deployment that actually refuses an unregistered caller. Plain
    // community mode treats any client id as its own tenant and answers 200, so
    // running this there would assert nothing.
    match std::env::var("AXONFLOW_SAAS_URL").ok().filter(|v| !v.is_empty()) {
        None => run.skip(
            "an auth failure surfaces as an error, never as a denial",
            "AXONFLOW_SAAS_URL is unset; plain community mode never refuses a caller",
        ),
        Some(saas) => {
            let bad = AxonFlowClient::new(AxonFlowConfig {
                endpoint: saas,
                client_id: Some("s2-not-registered".to_string()),
                client_secret: Some("wrong".to_string()),
                ..Default::default()
            });
            let what = "an auth failure surfaces as an error, never as a denial";
            match bad {
                Err(e) => run.fail(what, e.to_string()),
                Ok(bad) => {
                    let outcome = evaluate_query(&bad, ALLOWED_QUERY).await;
                    run.check(
                        what,
                        match outcome {
                            Ok(d) => Err(format!(
                                "an unauthenticated call produced a decision (allowed={})",
                                d.allowed()
                            )),
                            Err(AuthZenEvaluationError::Transport(inner)) => expect(
                                inner.to_string().contains("401"),
                                &format!("the error did not name the status: {inner}"),
                            ),
                            Err(other) => {
                                Err(format!("expected a transport error, got {other:?}"))
                            }
                        },
                    );
                }
            }
        }
    }

    // --- 12: the legacy surface is untouched ------------------------------
    run.check(
        "POST /api/v1/decide still answers, so this surface is purely additive",
        decide_verdict(&agent, &client_id, &secret, ALLOWED_QUERY)
            .await
            .and_then(|v| expect(v == "allow", &format!("verdict was {v:?}"))),
    );

    println!();
    if run.ran != run.expected {
        println!(
            "FAIL: {} assertion(s) ran but {} were expected — checks stopped executing",
            run.ran, run.expected
        );
        std::process::exit(1);
    }
    if run.failed > 0 {
        println!("FAIL: {} of {} assertions failed", run.failed, run.ran);
        std::process::exit(1);
    }
    println!("ALL PASS: {}/{} assertions", run.ran, run.expected);
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn expect(ok: bool, detail: &str) -> Result<(), String> {
    if ok {
        Ok(())
    } else {
        Err(detail.to_string())
    }
}

fn request_with(subject: AuthZenSubject, query: &str) -> AuthZenRequest {
    AuthZenRequest::evaluating(
        subject,
        AuthZenAction::new("llm.completion"),
        AuthZenResource::new("llm", "llm"),
    )
    .with_query(Attribute::known(query))
}

async fn evaluate_query(
    client: &AxonFlowClient,
    query: &str,
) -> Result<AuthZenDecision, AuthZenEvaluationError> {
    client
        .evaluate(request_with(
            AuthZenSubject::new("gateway", "llm-gateway-01"),
            query,
        ))
        .await
}

fn expect_refusal(
    outcome: Result<AuthZenDecision, AuthZenEvaluationError>,
    code: AuthZenErrorCode,
    pointer: &str,
    retryable: bool,
) -> Result<(), String> {
    let err = match outcome {
        Ok(d) => {
            return Err(format!(
                "expected a refusal, got a decision (allowed={})",
                d.allowed()
            ))
        }
        Err(e) => e,
    };
    expect(
        err.retryable() == retryable,
        &format!("retryable was {}, wanted {retryable}", err.retryable()),
    )?;
    let refusal = err
        .as_refusal()
        .ok_or_else(|| format!("expected a typed refusal, got {err}"))?;
    expect(
        refusal.code == code,
        &format!("code was {}, wanted {code}", refusal.code),
    )?;
    expect(
        refusal.pointer.as_deref() == Some(pointer),
        &format!("pointer was {:?}, wanted {pointer:?}", refusal.pointer),
    )
}

fn basic(client_id: &str, secret: &str) -> String {
    base64::engine::general_purpose::STANDARD.encode(format!("{client_id}:{secret}"))
}

/// The legacy Decision API's verdict for the same query.
///
/// Raw HTTP because `/api/v1/decide` is deliberately not SDK-wrapped (ADR-056),
/// with the identical Basic-auth credentials the SDK's own transport sends.
async fn decide_verdict(
    agent: &str,
    client_id: &str,
    secret: &str,
    query: &str,
) -> Result<String, String> {
    let body = serde_json::json!({
        "stage": "llm",
        "query": query,
        "target": {"type": "llm"},
        "caller_identity": {"gateway_id": "llm-gateway-01"}
    });
    let resp = reqwest::Client::new()
        .post(format!("{agent}/api/v1/decide"))
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Basic {}", basic(client_id, secret)))
        .header("X-Client-ID", client_id)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("/api/v1/decide: {e}"))?;
    let status = resp.status();
    let text = resp.text().await.map_err(|e| format!("body: {e}"))?;
    if !status.is_success() {
        return Err(format!("/api/v1/decide HTTP {status}: {text}"));
    }
    let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| format!("json: {e}"))?;
    v.get("verdict")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| format!("no verdict in {text}"))
}

/// The pointer AND code the SERVER names for an envelope the SDK refuses locally.
async fn raw_refusal(
    agent: &str,
    client_id: &str,
    secret: &str,
    envelope: serde_json::Value,
) -> Result<(Option<String>, String), String> {
    let resp = reqwest::Client::new()
        .post(format!("{agent}{AUTHZEN_PATH}"))
        .header("Content-Type", "application/json")
        .header(AUTHZEN_PROFILE_HEADER, AUTHZEN_PROFILE_V1)
        .header("Authorization", format!("Basic {}", basic(client_id, secret)))
        .header("X-Client-ID", client_id)
        .json(&envelope)
        .send()
        .await
        .map_err(|e| format!("raw evaluation: {e}"))?;
    if resp.status().is_success() {
        return Err("the server ACCEPTED an envelope the SDK refuses; the two have diverged".into());
    }
    let v: serde_json::Value = resp.json().await.map_err(|e| format!("json: {e}"))?;
    Ok((
        v.get("pointer").and_then(|p| p.as_str()).map(String::from),
        v.get("code")
            .and_then(|c| c.as_str())
            .unwrap_or_default()
            .to_string(),
    ))
}

/// A request that does NOT negotiate the profile gets the bare boolean.
async fn raw_unnegotiated_has_no_context(
    agent: &str,
    client_id: &str,
    secret: &str,
) -> Result<(), String> {
    let resp = reqwest::Client::new()
        .post(format!("{agent}{AUTHZEN_PATH}"))
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Basic {}", basic(client_id, secret)))
        .header("X-Client-ID", client_id)
        .json(&serde_json::json!({
            "evaluation": {
                "subject": {"type": "gateway", "id": "llm-gateway-01"},
                "action": {"name": "llm.completion"},
                "resource": {"type": "llm", "id": "llm"},
                "context": {"args": {"query": ALLOWED_QUERY}}
            }
        }))
        .send()
        .await
        .map_err(|e| format!("raw evaluation: {e}"))?;
    let status = resp.status();
    let v: serde_json::Value = resp.json().await.map_err(|e| format!("json: {e}"))?;
    expect(status.is_success(), &format!("HTTP {status}: {v}"))?;
    expect(
        v.get("decision").is_some(),
        "the bare response carried no decision at all",
    )?;
    expect(
        v.get("context").is_none(),
        &format!("the un-negotiated response carried a profile payload: {v}"),
    )
}
