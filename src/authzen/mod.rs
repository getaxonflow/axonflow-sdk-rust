//! AuthZEN-native authorization.
//!
//! This is the surface the ADR-065 compatibility plan commits to in all five
//! SDKs. It talks to `POST /api/v1/access/evaluation`, whose wire shape is
//! generated from the platform's canonical contract (see [`types_gen`]);
//! nothing outside that file re-states a field name or an enum value.
//!
//! # What this replaces, and when
//!
//! Nothing yet. The existing decision surface ([`crate::pep`],
//! [`crate::AxonFlowClient::proxy_llm_call`]) stays wire-stable through all of
//! v11 and is not deprecated here. This is the surface to write NEW
//! integrations against, because at v11 the engine behind it changes to the
//! ADR-065 Policy Decision Point with no wire change - an integration written
//! against it migrates once rather than twice. See
//! `docs/AUTHZEN_MIGRATION_DRAFT.md`.
//!
//! # The one thing worth knowing before you call it
//!
//! The server refuses anything it cannot evaluate rather than evaluating around
//! it. Send a subject property, an unrecognised context member, or an argument
//! beside the query, and you get an [`AuthZenError`] naming the exact member -
//! not a decision computed without it. That is deliberate: a decision that
//! silently ignored an attribute would tell you the attribute was weighed when
//! it was not, and every audit of that decision would inherit the claim.
//!
//! This SDK holds the same line on its own side of the wire. An attribute the
//! CALLER could not resolve never reaches the server either - see
//! [`attribute`], which is the module to read before writing any code that
//! fills in a `properties` bag or a correlation key.
//!
//! # Local and remote refusals name the same MEMBER
//!
//! The SDK validates before sending, and a local refusal carries the JSON
//! Pointer the server would have sent for the same bytes. The CODE may be
//! narrower on the server side: this client knows only that a required member
//! is missing and says `incomplete_evaluation`, while the server additionally
//! knows which values it can evaluate and narrows the same condition to
//! `unsupported_subject` with a `supported` list. Branch on the pointer for
//! "which member"; read the code as the server's more specific reading when
//! there is one.
//!
//! # Example
//!
//! ```no_run
//! use axonflow_sdk_rust::authzen::{
//!     Attribute, AuthZenAction, AuthZenRequest, AuthZenResource, AuthZenSubject,
//! };
//! # async fn demo(client: &axonflow_sdk_rust::AxonFlowClient) -> Result<(), Box<dyn std::error::Error>> {
//! let request = AuthZenRequest::evaluating(
//!     AuthZenSubject::new("gateway", "llm-gateway-01"),
//!     AuthZenAction::new("llm.completion"),
//!     AuthZenResource::new("llm", "llm"),
//! )
//! .with_query(Attribute::known("what is our refund policy?"))
//! .with_correlation("x-session-id", Attribute::known("sess-4711"));
//!
//! let decision = client.evaluate(request).await?;
//! if !decision.allowed() {
//!     println!("blocked: {} ({})", decision.state(), decision.category());
//! }
//! # Ok(())
//! # }
//! ```

pub mod attribute;
pub mod types_gen;

use crate::error::AxonFlowError;
use crate::AxonFlowClient;

pub use attribute::{Attribute, AttributeMap, AttributeValue};
pub use types_gen::*;

/// The AuthZEN evaluation endpoint.
pub const AUTHZEN_PATH: &str = "/api/v1/access/evaluation";

/// The header that negotiates the AxonFlow profile.
///
/// The SDK always sends it. AuthZEN 1.0's response is a bare boolean, and the
/// four-valued state, the obligations and the approval challenge ride in the
/// response context, which the server returns only to a caller that asked for
/// it by version. This SDK understands the profile, so there is no reason to
/// ask for less than it can read.
pub const AUTHZEN_PROFILE_HEADER: &str = "X-Axonflow-AuthZEN-Profile";

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

impl AuthZenErrorCode {
    /// Whether the caller could get a different answer by sending the same
    /// request again.
    ///
    /// Only a dependency failure is. Every other code names something about the
    /// request itself, which will not change on a retry - so a client that
    /// retries on any refusal burns its budget on requests that cannot succeed.
    ///
    /// A code this build does not know is NOT retryable. Guessing the other way
    /// would turn every future code into a retry loop against a server that has
    /// already given its final answer.
    pub fn retryable(&self) -> bool {
        matches!(self, AuthZenErrorCode::EvaluationUnavailable)
    }
}

impl AuthZenError {
    /// Attaches the JSON Pointer naming the member at fault.
    ///
    /// `"unsupported_action"` without the offending member is a puzzle rather
    /// than a diagnosis, which is why the server never sends one without a
    /// pointer and neither does this SDK.
    ///
    /// An EMPTY pointer is dropped rather than sent. The root has no member to
    /// name, and `"pointer": ""` renders as `... at : ...` and reads to a caller
    /// as a member whose name is the empty string. The server sends no pointer
    /// at all for a refusal about the request as a whole, and neither does this.
    pub fn at(mut self, pointer: &str) -> Self {
        self.pointer = if pointer.is_empty() {
            None
        } else {
            Some(pointer.to_string())
        };
        self
    }

    /// Attaches the values that WOULD have been accepted.
    pub fn supporting<S: Into<String>>(mut self, supported: impl IntoIterator<Item = S>) -> Self {
        self.supported = supported.into_iter().map(Into::into).collect();
        self
    }

    /// Whether retrying this exact request could produce a different answer.
    pub fn retryable(&self) -> bool {
        self.code.retryable()
    }
}

impl std::fmt::Display for AuthZenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.pointer {
            Some(p) => write!(f, "axonflow: {} at {}: {}", self.code, p, self.message),
            None => write!(f, "axonflow: {}: {}", self.code, self.message),
        }
    }
}

impl std::error::Error for AuthZenError {}

/// Everything that can come back instead of a decision.
///
/// The variants are separated by what a caller should DO, not by where the
/// failure happened:
///
/// * [`Self::Refused`] - fix the request; the refusal names the member.
/// * [`Self::Unresolved`] - re-resolve an attribute and build a NEW request.
/// * [`Self::UnreadableProfile`] - upgrade the SDK.
/// * [`Self::UnusableResponse`] - a server contract violation to report.
/// * [`Self::UnusableRequest`] - the envelope could not be encoded; a backstop.
/// * [`Self::Transport`] - no answer; may simply be retried.
///
/// Collapsing them into one opaque error would leave a caller with a string to
/// match on.
#[derive(Debug, thiserror::Error)]
pub enum AuthZenEvaluationError {
    /// The request was refused rather than evaluated - by the server, or by
    /// this client before the round trip.
    ///
    /// Both name the SAME MEMBER: a local refusal carries the JSON Pointer the
    /// server would have sent for the same bytes, verified against a live
    /// server by `runtime-e2e/authzen_evaluation`.
    ///
    /// The CODE may be narrower on the server side, and that is not a defect in
    /// either. This client knows only that a required member is missing, and
    /// says `incomplete_evaluation`; the server additionally knows which values
    /// it can evaluate, and narrows the same condition to `unsupported_subject`
    /// with a `supported` list. Branch on the pointer for "which member", and
    /// treat the code as the server's more specific reading when there is one.
    #[error("{0}")]
    Refused(#[from] AuthZenError),

    /// The server answered in a profile this build cannot interpret.
    ///
    /// NOT retryable, and not folded into [`Self::Refused`] for exactly that
    /// reason: `evaluation_unavailable` is the enumeration's retryable code,
    /// and reporting "upgrade the SDK" through it would send a client into a
    /// retry loop against a server that will answer identically every time.
    #[error(
        "the server answered with AuthZEN profile {received:?}; this build can only interpret \
         {understood:?}. The obligations and approval challenge that constrain an allow are \
         carried in that payload, so the decision cannot be acted on safely. Upgrade the SDK."
    )]
    UnreadableProfile {
        /// What the server said it was speaking.
        received: String,
        /// What this build can read.
        understood: &'static str,
    },

    /// The server answered `200` with a body this build will not act on.
    ///
    /// A decision that cannot be read completely is not a decision. Acting on
    /// the half that parsed is how an allow carrying a mandatory obligation
    /// reaches an enforcement point that never saw it.
    #[error("the server's decision cannot be acted on: {detail}")]
    UnusableResponse {
        /// What about the body could not be trusted.
        detail: String,
    },

    /// The request could not be SENT as built: it carries an attribute the
    /// caller could not resolve.
    ///
    /// Separate from [`Self::Refused`], and NOT retryable, because the two need
    /// opposite actions from the caller. A server `evaluation_unavailable` says
    /// "send these bytes again"; this says "re-resolve the attribute and build a
    /// NEW request". Reporting it as retryable - which an earlier version of
    /// this SDK did - sends a `while err.retryable()` loop against a request
    /// whose refusal is frozen inside it, so every attempt produces the
    /// identical error until the budget runs out.
    ///
    /// The OPERATION may well succeed once the attribute resolves. That is a
    /// statement about a different request, and it is why this carries the
    /// pointer and the reason rather than a boolean.
    #[error(
        "this request cannot be sent as built. At {pointer}: {reason} Re-resolve the attribute and \
         build a NEW request; resending this one cannot succeed."
    )]
    Unresolved {
        /// The JSON Pointer naming the member nobody could resolve.
        pointer: String,
        /// The refusal message, which carries the reason the caller gave.
        reason: String,
    },

    /// The envelope could not be encoded.
    ///
    /// A backstop, not an ordinary outcome: the only way to reach it is to
    /// bypass validation and hand the encoder an unresolved attribute. It is
    /// distinct from [`Self::UnusableResponse`] because that one names a SERVER
    /// contract violation to report, and an operator handed one label for both
    /// cannot tell "the platform is emitting a body I must file a bug about"
    /// from "my own request was not built correctly".
    #[error("the request could not be encoded: {detail}")]
    UnusableRequest {
        /// What about the envelope could not be encoded.
        detail: String,
    },

    /// The request never got an answer: connection, timeout, credentials, or a
    /// non-refusal error status.
    ///
    /// This surface does NOT apply the client's [`crate::RetryConfig`]: that
    /// executor is wired to the proxy path's request type, and retrying an
    /// authorization decision on the caller's behalf is a policy decision this
    /// SDK does not make for them. Retry is the caller's, guided by
    /// [`AuthZenEvaluationError::retryable`].
    #[error("the evaluation request failed: {0}")]
    Transport(#[from] AxonFlowError),
}

impl AuthZenEvaluationError {
    /// Whether sending the same request again could produce a different answer.
    ///
    /// This is the whole retryable set, in one place, so a caller never has to
    /// assemble it from status codes:
    ///
    /// * a refusal - only when its code is `evaluation_unavailable`;
    /// * a transport failure - timeout, connect, `5xx`, `429`;
    /// * an unreadable profile - never;
    /// * an unusable response - never;
    /// * an unresolved attribute - NEVER, because the refusal is frozen inside
    ///   the request. The OPERATION may succeed once the attribute resolves,
    ///   but that is a different request, and this method answers only about
    ///   this one.
    /// * an unencodable request - never.
    pub fn retryable(&self) -> bool {
        match self {
            AuthZenEvaluationError::Refused(e) => e.retryable(),
            AuthZenEvaluationError::Transport(e) => e.is_retryable(),
            AuthZenEvaluationError::UnreadableProfile { .. }
            | AuthZenEvaluationError::UnusableResponse { .. }
            | AuthZenEvaluationError::Unresolved { .. }
            | AuthZenEvaluationError::UnusableRequest { .. } => false,
        }
    }

    /// The typed refusal, when there is one.
    pub fn as_refusal(&self) -> Option<&AuthZenError> {
        match self {
            AuthZenEvaluationError::Refused(e) => Some(e),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Building a request
// ---------------------------------------------------------------------------

impl AuthZenRequest {
    /// One subject performing one action on one resource.
    pub fn evaluating(
        subject: AuthZenSubject,
        action: AuthZenAction,
        resource: AuthZenResource,
    ) -> Self {
        AuthZenRequest {
            subject: Some(subject),
            action: Some(action),
            resource: Some(resource),
            context: AttributeMap::new(),
        }
    }

    /// The content the policy engine inspects, at `context.args.query`.
    ///
    /// Takes an [`Attribute`] rather than a `String` because a gateway does not
    /// always have the content in hand: a request whose body failed to decode
    /// has a query nobody could read, and evaluating as though there were
    /// nothing to inspect is the difference between "no content" and "content I
    /// could not see".
    pub fn with_query(mut self, query: Attribute<String>) -> Self {
        set_query(&mut self.context, query);
        self
    }

    /// One audit correlation key, at `context.correlation.<key>`.
    ///
    /// The deployment records an allowlisted, capped set of these; a key it does
    /// not record is refused by name rather than dropped, because telling a
    /// caller a key was captured when it was not is the same lie in both
    /// directions.
    pub fn with_correlation(mut self, key: &str, value: Attribute<String>) -> Self {
        set_correlation(&mut self.context, key, value);
        self
    }
}

impl AuthZenBulk {
    /// Several preconditions of ONE operation.
    pub fn over(evaluations: impl IntoIterator<Item = AuthZenRequest>) -> Self {
        AuthZenBulk::new(evaluations.into_iter().collect())
    }

    /// The subject every entry inherits unless it names its own.
    pub fn with_subject(mut self, subject: AuthZenSubject) -> Self {
        self.subject = Some(subject);
        self
    }

    /// The action every entry inherits unless it names its own.
    pub fn with_action(mut self, action: AuthZenAction) -> Self {
        self.action = Some(action);
        self
    }

    /// The resource every entry inherits unless it names its own.
    pub fn with_resource(mut self, resource: AuthZenResource) -> Self {
        self.resource = Some(resource);
        self
    }

    /// The shared `context.args.query` every entry inherits.
    pub fn with_query(mut self, query: Attribute<String>) -> Self {
        set_query(&mut self.context, query);
        self
    }

    /// A shared audit correlation key.
    pub fn with_correlation(mut self, key: &str, value: Attribute<String>) -> Self {
        set_correlation(&mut self.context, key, value);
        self
    }
}

/// Writes `context.args.query`, creating the nested bag if it is not there.
///
/// If `context.args` OR `context.args.query` already holds an UNRESOLVED
/// attribute, the write is declined and the `Unknown` stays. The rule applies
/// at BOTH levels: guarding only the parent left the defect reachable one level
/// down, which is where a caller would actually hit it. Overwriting it was the fail-open this
/// whole module exists to prevent, arriving through its own builder: a caller
/// that had recorded "nobody could read the request body" and then wrote a
/// recovered partial query over it would have produced a complete-looking
/// envelope, passed `validate`, and been handed a verdict that named every
/// attribute it weighed. Leaving the `Unknown` in place means the envelope is
/// refused at `/…/context/args` and never sent.
fn set_query(context: &mut AttributeMap, query: Attribute<String>) {
    if let Some(args) = context.nested_for_write("args") {
        args.record("query", query.map(AttributeValue::from));
    }
}

/// Writes one `context.correlation.<key>`, creating the nested bag if needed.
///
/// Declines the write over an unresolved `context.correlation`, for the reason
/// in [`set_query`].
fn set_correlation(context: &mut AttributeMap, key: &str, value: Attribute<String>) {
    if let Some(correlation) = context.nested_for_write("correlation") {
        correlation.record(key, value.map(AttributeValue::from));
    }
}

// ---------------------------------------------------------------------------
// Reading a decision
// ---------------------------------------------------------------------------

/// A decision this build could read COMPLETELY.
///
/// The type exists so that "the profile payload was there and hung together" is
/// established once, by construction, rather than re-asked at every accessor.
/// A [`AuthZenResponse`] that failed any of those checks never becomes one of
/// these - it becomes an [`AuthZenEvaluationError`].
#[derive(Clone, Debug, PartialEq)]
pub struct AuthZenDecision {
    decision: bool,
    context: AuthZenResponseContext,
}

impl AuthZenDecision {
    /// Whether the enforcement point may proceed.
    ///
    /// Read this rather than comparing the state yourself. It requires BOTH the
    /// collapsed boolean and the operational state to say `ALLOW`: exactly one
    /// state permits execution, and a caller that branches on anything else -
    /// "not DENY", say - treats a CHALLENGE or an ERROR as permission.
    ///
    /// A decision whose boolean and state DISAGREE never reaches this method;
    /// it is refused as an unusable response, because there is no reading of
    /// such a body that is not a guess.
    ///
    /// Which makes the `state == ALLOW` conjunct here UNREACHABLE while that
    /// refusal stands: by the time a value is an `AuthZenDecision`, the two
    /// already agree. It is kept because the two checks live in different
    /// functions, and this is not the one a future refactor of the decoding
    /// path is likely to touch. No test kills a mutant that deletes it - the
    /// mutation gate is where that was measured, not assumed - and saying so
    /// here is better than a comment implying coverage that does not exist.
    ///
    /// An allow is not the end of it: a mandatory obligation the enforcement
    /// point cannot discharge means the operation must NOT proceed. See
    /// [`AuthZenDecision::mandatory_obligations`].
    pub fn allowed(&self) -> bool {
        self.decision && self.context.state == AuthZenOperationalState::Allow
    }

    /// The four-valued operational state.
    pub fn state(&self) -> &AuthZenOperationalState {
        &self.context.state
    }

    /// The coarse outcome category.
    pub fn category(&self) -> &AuthZenCategory {
        &self.context.category
    }

    /// The safe machine reason, when the server sent one.
    pub fn reason(&self) -> Option<&AuthZenReasonCode> {
        self.context.reason.as_ref()
    }

    /// Every instruction the enforcement point must discharge.
    pub fn obligations(&self) -> &[AuthZenObligation] {
        &self.context.obligations
    }

    /// The obligations that must be discharged for the allow to stand.
    ///
    /// An allow with an undischarged mandatory obligation is not an allow. A
    /// caller that cannot discharge one must block.
    pub fn mandatory_obligations(&self) -> impl Iterator<Item = &AuthZenObligation> {
        self.context.obligations.iter().filter(|o| o.mandatory)
    }

    /// The approval challenge the contract declares for a `CHALLENGE` state.
    ///
    /// NO DEPLOYED SERVER POPULATES THIS TODAY. The v10 route is an adapter over
    /// the legacy evaluation, and its handler builds the response context
    /// without an `approval` member - so a `CHALLENGE` arrives with this empty,
    /// and a caller that writes `decision.approval().unwrap()` panics on its
    /// first real challenge. It is surfaced because the contract declares it and
    /// the ADR-065 Policy Decision Point fills it at v11; until then, treat an
    /// empty approval on a CHALLENGE as the normal case and read
    /// [`AuthZenDecision::state`] and [`AuthZenDecision::category`] instead.
    pub fn approval(&self) -> Option<&AuthZenApprovalRequirement> {
        self.context.approval.as_ref()
    }

    /// The id of the entry that DETERMINED the outcome.
    ///
    /// For a plural envelope this names the entry that decided the meet, not
    /// the last one evaluated - it is the id an operator looks up to explain
    /// the outcome.
    pub fn decision_id(&self) -> &str {
        &self.context.decision_id
    }

    /// The contract version the server evaluated under.
    pub fn schema_version(&self) -> &str {
        &self.context.schema_version
    }

    /// The whole profile payload, for a caller that wants a member this type
    /// does not surface.
    pub fn context(&self) -> &AuthZenResponseContext {
        &self.context
    }

    /// Checks everything that has to hold before a body becomes a decision.
    fn from_response(response: AuthZenResponse) -> Result<Self, AuthZenEvaluationError> {
        // AN ABSENT CONTEXT IS A BLANKED CONTEXT, NOT AN EMPTY ONE.
        //
        // The server omits the profile payload for a caller that did not
        // negotiate - and this SDK ALWAYS negotiates. So a 200 with no context
        // is a server that ignored the header or a proxy that stripped it, and
        // the parts this build cannot see are exactly the parts that constrain
        // an allow: the obligations and the approval challenge. Reading it as
        // "no obligations" leaves `allowed()` returning true and the caller
        // proceeding on an allow whose mandatory redaction it never saw.
        let context = match response.context {
            Some(c) => c,
            None => {
                return Err(AuthZenEvaluationError::UnusableResponse {
                    detail: format!(
                        "the response carries no profile payload, though this request negotiated \
                         {AUTHZEN_PROFILE_HEADER}: {AUTHZEN_PROFILE_V1}. The obligations and the \
                         approval challenge ride in that payload, so an allow cannot be \
                         distinguished from an allow this client must not act on"
                    ),
                })
            }
        };

        // A profile from a version this build does not know is REFUSED, not
        // silently dropped. It is also the case that matters at the v11
        // cutover, which is precisely when a server starts speaking a profile
        // an older SDK does not know.
        if context.profile != AUTHZEN_PROFILE_V1 {
            return Err(AuthZenEvaluationError::UnreadableProfile {
                received: context.profile,
                understood: AUTHZEN_PROFILE_V1,
            });
        }

        // THE DECODED BODY IS VALIDATED, not assumed. Decoding establishes that
        // the members are the right SHAPE; it says nothing about a required
        // member being empty, an obligation naming no source policy, or an
        // approval clause with no eligible approvers - each of which would be
        // read by a caller as a fact about the decision.
        let response = AuthZenResponse {
            decision: response.decision,
            context: Some(context),
        };
        response
            .validate("")
            .map_err(|e| AuthZenEvaluationError::UnusableResponse {
                detail: e.to_string(),
            })?;
        let context = response.context.expect("set immediately above");

        // The boolean and the state are two renderings of ONE outcome: the
        // contract says `decision` is true exactly when the state is ALLOW. If
        // they disagree, one of them is wrong and nothing here can tell which,
        // so acting on either is a coin flip on an authorization decision.
        //
        // This also covers a state this build does not know: an unknown state
        // with `decision: true` cannot be ALLOW as far as this build can tell,
        // and is refused rather than proceeding.
        let state_allows = context.state == AuthZenOperationalState::Allow;
        if state_allows != response.decision {
            return Err(AuthZenEvaluationError::UnusableResponse {
                detail: format!(
                    "the decision boolean is {} but the operational state is {}; the contract \
                     makes them one outcome, so a body where they disagree cannot be acted on",
                    response.decision, context.state
                ),
            });
        }

        Ok(AuthZenDecision {
            decision: response.decision,
            context,
        })
    }
}

/// Maps a LOCAL validation refusal onto the outcome the caller needs.
///
/// The one code that has to be re-read on this side is `evaluation_unavailable`.
/// From the server it means "the evaluator could not be reached; send these
/// bytes again". Produced locally it means "an attribute in this request was
/// never resolved", and resending the identical request reproduces the identical
/// refusal forever. Same code, opposite action, so they must not arrive as the
/// same thing.
fn local_refusal(refusal: AuthZenError) -> AuthZenEvaluationError {
    if refusal.code == AuthZenErrorCode::EvaluationUnavailable {
        return AuthZenEvaluationError::Unresolved {
            pointer: refusal.pointer.clone().unwrap_or_default(),
            reason: refusal.message.clone(),
        };
    }
    AuthZenEvaluationError::Refused(refusal)
}

// ---------------------------------------------------------------------------
// The client surface
// ---------------------------------------------------------------------------

impl AxonFlowClient {
    /// Asks whether one subject may perform one action on one resource.
    ///
    /// Fails closed: every outcome that is not a readable decision is an error,
    /// and there is no path through this function that returns an allow it
    /// could not fully read.
    pub async fn evaluate(
        &self,
        request: AuthZenRequest,
    ) -> Result<AuthZenDecision, AuthZenEvaluationError> {
        self.evaluate_envelope(AuthZenEnvelope {
            evaluation: Some(request),
            evaluations: None,
        })
        .await
    }

    /// Asks whether ONE operation is permitted against SEVERAL preconditions.
    ///
    /// It returns ONE decision, not one per entry. The entries of a bulk
    /// request are preconditions of a single operation (moving a ticket must be
    /// authorized against the destination project as well as against the
    /// ticket), so they combine to the least permissive outcome: one denied
    /// entry denies the operation. An API returning a list would invite a caller to
    /// act on the entry it liked.
    ///
    /// Any member an entry omits is inherited from the envelope's shared base,
    /// so the common case is a shared subject and action with one resource per
    /// entry.
    pub async fn evaluate_all(
        &self,
        bulk: AuthZenBulk,
    ) -> Result<AuthZenDecision, AuthZenEvaluationError> {
        self.evaluate_envelope(AuthZenEnvelope {
            evaluation: None,
            evaluations: Some(bulk),
        })
        .await
    }

    /// The one transport path both entry points share.
    async fn evaluate_envelope(
        &self,
        envelope: AuthZenEnvelope,
    ) -> Result<AuthZenDecision, AuthZenEvaluationError> {
        // Validated before the round trip. The server enforces the same rules
        // and answers with a typed refusal, so for most of these this is a
        // convenience - a caller that mis-built an envelope learns it from a
        // local error naming the member instead of from a 422.
        //
        // For ONE class it is not a convenience but the whole point: an
        // attribute the caller could not resolve has no wire representation, so
        // the server can never refuse it. Only this check can.
        if let Err(refusal) = envelope.validate("") {
            return Err(local_refusal(refusal));
        }

        let body =
            serde_json::to_vec(&envelope).map_err(|e| AuthZenEvaluationError::UnusableRequest {
                detail: e.to_string(),
            })?;

        let url = format!("{}{}", self.endpoint(), AUTHZEN_PATH);
        let response = self
            .raw_post_json_bytes(&url, body, &[(AUTHZEN_PROFILE_HEADER, AUTHZEN_PROFILE_V1)])
            .await?;

        let status = response.status();
        let raw = response
            .bytes()
            .await
            .map_err(|e| AuthZenEvaluationError::Transport(AxonFlowError::HttpError(e)))?;

        if !status.is_success() {
            // A refusal is a typed document, so the caller can branch on the
            // code and be pointed at the member to fix. A body that is not one
            // still surfaces as an error - never as a decision.
            //
            // A 5xx is only read as a refusal when the code is one this build
            // KNOWS. An unrecognised code round-trips as `Unknown`, which is
            // deliberately non-retryable - so an ingress or sidecar answering
            // 503 with its own JSON error body would otherwise turn a transient
            // outage into a permanent refusal that a `while err.retryable()`
            // loop will not retry. A 4xx is still read as a refusal whatever the
            // code, because "fix the request" is right either way and the
            // pointer is worth more than the code.
            let client_error = status.is_client_error();
            if let Ok(refusal) = serde_json::from_slice::<AuthZenError>(&raw) {
                let usable = !refusal.code.as_str().is_empty() && !refusal.message.is_empty();
                if usable && (client_error || refusal.code.is_known()) {
                    return Err(AuthZenEvaluationError::Refused(refusal));
                }
            }
            return Err(AuthZenEvaluationError::Transport(AxonFlowError::ApiError {
                status: status.as_u16(),
                message: String::from_utf8_lossy(&raw).into_owned(),
            }));
        }

        // Strict decoding on the success path: every generated type carries
        // `deny_unknown_fields`. An unknown member in a decision is a server
        // speaking a profile this build does not understand, and quietly
        // dropping it would mean acting on a partial reading of an
        // authorization decision.
        let decoded: AuthZenResponse =
            serde_json::from_slice(&raw).map_err(|e| AuthZenEvaluationError::UnusableResponse {
                detail: format!(
                    "the decision could not be decoded: {e}; body={}",
                    String::from_utf8_lossy(&raw)
                ),
            })?;

        AuthZenDecision::from_response(decoded)
    }
}
