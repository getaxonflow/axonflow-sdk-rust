//! Typed policy authoring: the v11 successor to the legacy policy routes.
//!
//! A v11 platform authors policy as a typed document: validated, published as a
//! signed artifact pinned by its digest, and promoted to active. Six routes
//! under `/api/v1/typed-policies` do that, and the agent proxies all six with
//! this client's credentials. Reach them as
//! [`client.typed_policies()`](crate::AxonFlowClient::typed_policies):
//!
//! - [`edition`](TypedPolicies::edition): what this deployment may author,
//!   consulted BEFORE a publication rather than learned from a refusal.
//! - [`validate`](TypedPolicies::validate): every finding for a candidate
//!   document. It answers the same on every edition; the edition's boundary
//!   applies at publication.
//! - [`publish`](TypedPolicies::publish): validates, compiles, runs the
//!   declared fixtures, signs, and pins the artifact by its digest.
//! - [`activate`](TypedPolicies::activate): promotes a published digest to
//!   active.
//! - [`active`](TypedPolicies::active): the document in force, as the exact
//!   bytes that were signed, or `None` when nothing is active.
//! - [`system`](TypedPolicies::system): the platform's own controls,
//!   read-only.
//!
//! The organization and the author are the ones this client's credentials
//! resolve to. The agent stamps both, and neither can be named in a request.
//!
//! Activation PROMOTES: a digest whose version does not advance past the active
//! one is refused. Rolling back to an earlier document and withdrawing the
//! active one are operations of the customer portal, behind its session; the
//! agent does not proxy them, so this module has no method for either. The
//! per-policy override routes are retired (`PerPolicyOverrideRetired`): an
//! organization changes a shipped control through its typed document.
//!
//! On an edition with separation of duties, `publish` refuses every
//! publication with the finding code `APPROVER_IS_AUTHOR`: publishing through
//! this route names no approver, and such a deployment approves in the customer
//! portal.
//!
//! Every refusal is [`AxonFlowError::TypedPolicyRefusal`], carrying the HTTP
//! status, the platform's `reason` and, where there are any, the findings. A
//! `401` stays [`AxonFlowError::ApiError`] with `status: 401`, the client's
//! authentication error. These routes do not read the PEP capability
//! declaration, and the client never sends it on them. They need a v11.0.0
//! platform.

use crate::client::AxonFlowClient;
use crate::error::AxonFlowError;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::fmt;

/// The route prefix the six operations share.
pub const TYPED_POLICIES_PATH: &str = "/api/v1/typed-policies";

/// The platform marshals a nil Go slice or map as JSON `null`, not `[]` or
/// `{}`: a clean validation answers `"findings": null`. Every collection it
/// declares without `omitempty` therefore reads `null` as empty.
fn null_as_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

/// What this edition may spend (the spec's `EditionConstructReport`).
///
/// The booleans are `Option`s: absent and `false` are different answers, and
/// an absent flag must not read as a permission's zero value.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditionConstructReport {
    /// `community`, `evaluation` or `enterprise`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edition: Option<String>,
    #[serde(default, deserialize_with = "null_as_default")]
    pub obligation_families: Vec<String>,
    #[serde(default, deserialize_with = "null_as_default")]
    pub attribute_namespaces: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_scope: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub separation_of_duties: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier_established: Option<bool>,
    /// Constructs withheld for want of an edition ruling, not by one.
    #[serde(default, deserialize_with = "null_as_default")]
    pub reserved: Vec<String>,
}

/// One declared save-time or publication result (the spec's
/// `AuthoringFinding`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthoringFinding {
    /// The finding code, for example `ACTION_NOT_REGISTERED`.
    pub code: String,
    /// `reject` or `warn`.
    pub severity: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy_id: Option<String>,
    /// The declared, code-level sentence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// What was wrong, naming the offending value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// A candidate document and its fixtures (the spec's
/// `TypedAuthoringDocumentRequest`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TypedAuthoringDocumentRequest {
    /// The authoring document. Opaque to this SDK on purpose: the spec declares
    /// it `type: object`, the authoring model itself rather than a mirror of
    /// it, and a typed struct would drop the members a later vocabulary adds.
    pub document: Value,
    /// The author-declared cases the publication gauntlet runs. `None` omits
    /// the member; `Some(vec![])` sends `[]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fixtures: Option<Vec<Value>>,
}

/// What this deployment may author.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypedAuthoringEdition {
    #[serde(default)]
    pub success: bool,
    /// The configured authoring vocabulary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog: Option<String>,
    /// The one authority root this surface publishes under.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<String>,
    /// Customer-authored documents admitted per organization; `-1` is
    /// unlimited.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_documents: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub constructs: Option<EditionConstructReport>,
    /// `process`, `database` or `unavailable`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub persistence: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signing_key_custody: Option<String>,
}

/// Every finding for a candidate document. `success` is false when any is a
/// rejection.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypedPolicyValidation {
    #[serde(default)]
    pub success: bool,
    #[serde(default, deserialize_with = "null_as_default")]
    pub findings: Vec<AuthoringFinding>,
}

/// A published artifact. Activation names `digest`, never the version.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypedPolicyPublication {
    #[serde(default)]
    pub success: bool,
    pub digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<i64>,
    #[serde(default, deserialize_with = "null_as_default")]
    pub findings: Vec<AuthoringFinding>,
}

/// The audited activation record.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TypedPolicyActivation {
    #[serde(default)]
    pub success: bool,
    #[serde(default, deserialize_with = "null_as_default")]
    pub activation: Map<String, Value>,
}

/// The document in force.
///
/// `source` is the exact byte sequence that was signed, so a caller can verify
/// it; `document` is the same bytes parsed.
#[derive(Debug, Clone, PartialEq)]
pub struct ActiveTypedPolicy {
    pub source: Vec<u8>,
    pub document: Value,
}

/// One shipped control, with what happens when it cannot be evaluated.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TypedPolicySystemControl {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authority: Option<String>,
    /// `enforcement`, `gating_risk` or `advisory`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assurance: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mandatory: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, deserialize_with = "null_as_default")]
    pub obligations: Vec<Value>,
}

/// The platform's own controls: the system root activated beneath every
/// organization.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TypedPolicySystemCorpus {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<i64>,
    /// The digest an enforcing engine anchors to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authority: Option<String>,
    #[serde(default, deserialize_with = "null_as_default")]
    pub controls: Vec<TypedPolicySystemControl>,
    #[serde(default, deserialize_with = "null_as_default")]
    pub assurance_counts: BTreeMap<String, i64>,
    #[serde(default, deserialize_with = "null_as_default")]
    pub document: Map<String, Value>,
}

/// A typed policy authoring request the platform refused.
///
/// `status` is the HTTP status and `reason` the platform's reason, for example
/// `publication_refused` (422), `document_refused` (422, with the save-time
/// findings), `activation_refused` (409), `tier_limit` (402, with `code`
/// naming the limit) or `artifact_cap` (429). `findings` holds the declared
/// findings a refused publication or document carries, and `retry_after` the
/// seconds from `Retry-After` when the refusal is retryable. `message` is the
/// platform's own explanation, or `HTTP <status> from <route>` when it gave
/// none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypedPolicyRefusal {
    pub status: u16,
    pub reason: Option<String>,
    pub code: Option<String>,
    pub message: String,
    pub findings: Vec<AuthoringFinding>,
    pub retry_after: Option<u64>,
}

impl fmt::Display for TypedPolicyRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.reason {
            Some(reason) => write!(
                f,
                "typed policy request refused (HTTP {}, {reason}): {}",
                self.status, self.message
            ),
            None => write!(
                f,
                "typed policy request refused (HTTP {}): {}",
                self.status, self.message
            ),
        }
    }
}

/// The error for a non-2xx answer: the client's authentication error for a
/// `401`, the typed refusal for everything else.
///
/// The body is read member by member, so a member of an unexpected type leaves
/// the others read, and a body that is not JSON at all still names its status.
fn refusal(status: u16, retry_after: Option<u64>, body: &[u8], route: &str) -> AxonFlowError {
    let parsed: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    let text = |member: &str| {
        parsed
            .get(member)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    let message = text("error").unwrap_or_else(|| format!("HTTP {status} from {route}"));
    if status == 401 {
        return AxonFlowError::ApiError { status, message };
    }
    let findings = parsed
        .get("findings")
        .and_then(Value::as_array)
        .map(|all| {
            all.iter()
                .filter_map(|f| serde_json::from_value::<AuthoringFinding>(f.clone()).ok())
                .collect()
        })
        .unwrap_or_default();
    AxonFlowError::TypedPolicyRefusal(Box::new(TypedPolicyRefusal {
        status,
        reason: text("reason"),
        code: text("code"),
        message,
        findings,
        retry_after,
    }))
}

/// `Retry-After` in seconds, when it is a plain non-negative integer.
fn retry_after(response: &reqwest::Response) -> Option<u64> {
    response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .filter(|v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()))
        .and_then(|v| v.parse().ok())
}

/// `client.typed_policies()`: typed policy authoring through the agent. See
/// [`crate::typed_policies`] for what each operation does and what it does
/// not.
///
/// It borrows the client, so it presents that client's credentials and
/// per-user identity; a client derived with
/// [`as_user`](AxonFlowClient::as_user) gets a namespace bound to itself.
#[derive(Clone, Copy)]
pub struct TypedPolicies<'a> {
    client: &'a AxonFlowClient,
}

impl AxonFlowClient {
    /// Typed policy authoring: the six `/api/v1/typed-policies` routes. See
    /// [`crate::typed_policies`].
    pub fn typed_policies(&self) -> TypedPolicies<'_> {
        TypedPolicies { client: self }
    }
}

impl TypedPolicies<'_> {
    fn url(&self, route: &str) -> String {
        format!("{}{TYPED_POLICIES_PATH}{route}", self.client.endpoint())
    }

    /// A 2xx answer's body as a JSON object; the error for any other answer.
    async fn object(
        &self,
        response: reqwest::Response,
        route: &str,
    ) -> Result<Value, AxonFlowError> {
        let status = response.status().as_u16();
        let wait = retry_after(&response);
        let body = response.bytes().await?;
        if status >= 400 {
            return Err(refusal(status, wait, &body, route));
        }
        match serde_json::from_slice::<Value>(&body) {
            Ok(value) if value.is_object() => Ok(value),
            _ => Err(AxonFlowError::ApiError {
                status,
                message: format!(
                    "{TYPED_POLICIES_PATH}{route} answered {status} with a body that is not an object"
                ),
            }),
        }
    }

    async fn get(&self, route: &str) -> Result<Value, AxonFlowError> {
        let response = self.client.raw_get_as(&self.url(route), None).await?;
        self.object(response, route).await
    }

    async fn post<T: Serialize>(&self, route: &str, payload: &T) -> Result<Value, AxonFlowError> {
        let body = serde_json::to_vec(payload)?;
        let response = self
            .client
            .raw_post_json_bytes(&self.url(route), body, &[])
            .await?;
        self.object(response, route).await
    }

    /// What this deployment may author: its construct boundary and document
    /// ceiling.
    pub async fn edition(&self) -> Result<TypedAuthoringEdition, AxonFlowError> {
        Ok(serde_json::from_value(self.get("/edition").await?)?)
    }

    /// Every finding for a candidate document, ordered and complete.
    ///
    /// A document that is refused is still a successful validation: read
    /// `success` and the findings rather than expecting an error.
    pub async fn validate(
        &self,
        document: &Value,
        fixtures: Option<&[Value]>,
    ) -> Result<TypedPolicyValidation, AxonFlowError> {
        let request = TypedAuthoringDocumentRequest {
            document: document.clone(),
            fixtures: fixtures.map(<[Value]>::to_vec),
        };
        Ok(serde_json::from_value(
            self.post("/validate", &request).await?,
        )?)
    }

    /// Publish a document as a signed artifact, pinned by its digest.
    ///
    /// `fixtures` are the author-declared cases the publication gauntlet runs;
    /// a publication without any is refused, since no policy in the document
    /// has then been shown to do anything.
    ///
    /// # Errors
    ///
    /// [`AxonFlowError::TypedPolicyRefusal`]: `422` with the findings
    /// (`publication_refused`, or `document_refused` for the save-time checks;
    /// an edition boundary or `APPROVER_IS_AUTHOR` appears as a finding code),
    /// `402` `tier_limit`, `429` `artifact_cap`, or `400` for a malformed
    /// request.
    pub async fn publish(
        &self,
        document: &Value,
        fixtures: Option<&[Value]>,
    ) -> Result<TypedPolicyPublication, AxonFlowError> {
        let request = TypedAuthoringDocumentRequest {
            document: document.clone(),
            fixtures: fixtures.map(<[Value]>::to_vec),
        };
        Ok(serde_json::from_value(
            self.post("/publish", &request).await?,
        )?)
    }

    /// Promote a published digest to active. The activation is audited and
    /// names the caller. An empty or absent `reason` is not sent.
    ///
    /// # Errors
    ///
    /// [`AxonFlowError::TypedPolicyRefusal`] `409` `activation_refused` when
    /// the digest is not admitted, its version does not advance, its parent is
    /// not the active digest, or the caller may not activate it.
    pub async fn activate(
        &self,
        digest: &str,
        reason: Option<&str>,
    ) -> Result<TypedPolicyActivation, AxonFlowError> {
        let mut payload = Map::new();
        payload.insert("digest".into(), Value::String(digest.to_string()));
        if let Some(reason) = reason.filter(|r| !r.is_empty()) {
            payload.insert("reason".into(), Value::String(reason.to_string()));
        }
        Ok(serde_json::from_value(
            self.post("/activate", &payload).await?,
        )?)
    }

    /// The document in force, as the exact bytes that were signed, or `None`
    /// when nothing is active (the platform's `404`).
    ///
    /// A platform without the typed routes (before v11.0.0) also answers `404`
    /// here, because the route itself is missing; [`edition`](Self::edition)
    /// tells the two apart.
    pub async fn active(&self) -> Result<Option<ActiveTypedPolicy>, AxonFlowError> {
        let route = "/active";
        let response = self.client.raw_get_as(&self.url(route), None).await?;
        let status = response.status().as_u16();
        if status == 404 {
            return Ok(None);
        }
        let wait = retry_after(&response);
        let source = response.bytes().await?.to_vec();
        if status >= 400 {
            return Err(refusal(status, wait, &source, route));
        }
        match serde_json::from_slice::<Value>(&source) {
            Ok(document) if document.is_object() => Ok(Some(ActiveTypedPolicy { source, document })),
            _ => Err(AxonFlowError::ApiError {
                status,
                message: format!(
                    "{TYPED_POLICIES_PATH}{route} answered {status} with a body that is not an object"
                ),
            }),
        }
    }

    /// The platform's own controls, read-only: the shipped system corpus.
    pub async fn system(&self) -> Result<TypedPolicySystemCorpus, AxonFlowError> {
        let body = self.get("/system").await?;
        let system = match body.get("system") {
            Some(system @ Value::Object(_)) => system.clone(),
            _ => Value::Object(Map::new()),
        };
        Ok(serde_json::from_value(system)?)
    }
}
