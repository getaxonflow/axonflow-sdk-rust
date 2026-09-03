//! Read-path per-user identity and the platform's read-scope contract.
//!
//! Since platform #2922 the role-scoped read routes (audit / decisions /
//! overrides) answer from the identity the CALLER presents, not from the tenant
//! credential alone. The tenant credential in `Authorization` says which
//! organization is asking; it does not say WHO. A caller that presents no
//! per-user identity to an enterprise stack is not "a caller who sees
//! everything" and is not "a caller who sees nothing by coincidence" — it is a
//! caller the platform cannot scope, and every scoped read it makes returns
//! zero rows by construction.
//!
//! This module carries the whole surface:
//!
//! - the per-user identity itself ([`AxonFlowConfig::user_token`] for a
//!   client-wide identity, the `*_as` read methods for a per-call one, and
//!   [`AxonFlowClient::as_user`] for a process acting on behalf of several
//!   people), stamped as the `X-User-Token` header from exactly ONE site —
//!   `AxonFlowClient::dispatch`, which every request goes through. There is no
//!   per-method header plumbing, deliberately: the platform reads the header
//!   once in its own proxy middleware (`platform/agent/proxy.go`
//!   `proxyAuthMiddleware`), not per route, so a per-method sprinkle here would
//!   be a second, drifting copy of a decision the platform makes in one place.
//!
//! - the response side of the same contract: `X-Axonflow-Read-Scope`, which the
//!   platform stamps on every scoped read (`platform/orchestrator/read_scope.go`
//!   `applyReadScopeHeader`) to say which of the three scopes the answer was
//!   computed under. Without it, a 404 from explain and an empty list from
//!   `list_decisions` are indistinguishable from "the row is not there", which
//!   is how a governed read comes to report a confident, vacuous nothing.
//!
//! [`AxonFlowConfig::user_token`]: crate::AxonFlowConfig::user_token
//! [`AxonFlowClient::as_user`]: crate::AxonFlowClient::as_user

use std::fmt;

/// The request header carrying the per-user identity.
///
/// This constant is the SDK's only spelling of it. The header is set in exactly
/// one place (`AxonFlowClient::dispatch`); if you find yourself setting it in a
/// method, the method is the wrong altitude.
pub const HEADER_USER_TOKEN: &str = "X-User-Token";

/// The response header the platform stamps on scoped reads.
pub const HEADER_READ_SCOPE: &str = "X-Axonflow-Read-Scope";

/// The scope the platform computed a role-scoped read under, taken from the
/// `X-Axonflow-Read-Scope` response header.
///
/// Three named variants are the platform's closed set. Two states are NOT in it
/// and are deliberately distinct from each other and from the three:
///
/// - [`ReadScope::Absent`] — the response carried no such header. That is what a
///   pre-#2922 platform, a non-scoped route, or a proxy that dropped the header
///   looks like. It means "not stated", never "none": treating an absent header
///   as a scope of `none` would turn every older stack's perfectly good read
///   into a refusal.
///
/// - [`ReadScope::Other`] — a scope a newer platform names and this build does
///   not recognise. Its VALUE is preserved rather than folded into one of the
///   three, so a caller can see what it was — trimmed and lower-cased, like the
///   three named ones, because the same normalisation has to apply to every
///   value or the recognised set would depend on a proxy's header casing. It
///   never triggers a refusal:
///   this header is the platform's account of a decision it has ALREADY made and
///   applied, so an unrecognised value is a reporting gap on our side, not a
///   licence to invent an outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadScope {
    /// No `X-Axonflow-Read-Scope` header at all. Distinct from [`ReadScope::None`].
    Absent,
    /// Tenant-wide: a tenant-wide role (admin / owner / policy_admin), or a
    /// Community / Community-SaaS deployment where the whole tenant is the one
    /// operator.
    Tenant,
    /// Narrowed to the rows attributed to the identity presented. A miss under
    /// this scope means "not among yours", which is NOT the same statement as
    /// "not there" — see [`ReadScopeRefusal`].
    OwnRows,
    /// The platform RESOLVED no per-user identity and the caller holds no
    /// tenant-wide authority, so it returned zero rows by construction. Under
    /// this scope a read CANNOT have returned data, so its empty answer says
    /// nothing about what exists.
    ///
    /// "Resolved none" is wider than "presented none", and the difference is
    /// worth knowing before you go looking in the wrong place. A token that
    /// validates perfectly still resolves to no identity when its address is one
    /// the platform reserves for SHARED, non-personal identities — the whole of
    /// `@axonflow.local` and `@axonflow.internal`, plus the community and
    /// evaluator addresses. Those name a pool of callers rather than a person,
    /// and scoping a read to one would return the pool, so the platform
    /// deliberately censuses them to nothing. A per-user token minted with an
    /// address in one of those domains therefore reads exactly like no token at
    /// all. (Easy to hit: the platform's own `generate-jwt.sh` defaults to
    /// `demo-user@axonflow.local`.)
    None,
    /// A scope this build does not recognise, preserved (trimmed and
    /// lower-cased, as every value on this header is).
    Other(String),
}

impl ReadScope {
    /// The scope a response header names.
    ///
    /// Trimmed and lower-cased, for the same reason the platform's own header
    /// helpers are: a proxy that normalises header casing or appends whitespace
    /// must not silently change the answer. The cost of getting that wrong is
    /// one-sided and quiet — a scope spelled `None` would fall to
    /// [`ReadScope::Other`] and the vacuous empty page it describes would come
    /// back as data again.
    pub fn parse(header: Option<&str>) -> Self {
        match header {
            None => ReadScope::Absent,
            Some(raw) => match raw.trim().to_ascii_lowercase().as_str() {
                "" => ReadScope::Absent,
                "tenant" => ReadScope::Tenant,
                "own-rows" => ReadScope::OwnRows,
                "none" => ReadScope::None,
                other => ReadScope::Other(other.to_string()),
            },
        }
    }

    /// The scope a response reports.
    pub fn of(response: &reqwest::Response) -> Self {
        Self::parse(
            response
                .headers()
                .get(HEADER_READ_SCOPE)
                .and_then(|v| v.to_str().ok()),
        )
    }
}

impl fmt::Display for ReadScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReadScope::Absent => write!(f, ""),
            ReadScope::Tenant => write!(f, "tenant"),
            ReadScope::OwnRows => write!(f, "own-rows"),
            ReadScope::None => write!(f, "none"),
            ReadScope::Other(value) => write!(f, "{value}"),
        }
    }
}

/// A role-scoped read whose answer was decided by the caller's identity scope
/// rather than by the data.
///
/// Carried by [`AxonFlowError::ReadScope`], which exists because "no rows" and
/// "no identity" are the same bytes on the wire. The platform distinguishes them
/// in the `X-Axonflow-Read-Scope` header; this is that distinction made visible,
/// so a read that could not have succeeded reports a cause instead of a
/// confident nothing.
///
/// Two shapes, told apart by [`ReadScopeRefusal::identity_missing`]:
///
/// - [`ReadScope::None`] — no identity was RESOLVED; the read returned zero rows
///   by construction and says nothing about what exists.
/// - [`ReadScope::OwnRows`] — an identity WAS resolved, and the row is not among
///   the ones attributed to it. That does NOT mean the row exists and belongs to
///   somebody else: the platform answers "not attributed to you" and "not there
///   at all" with the identical 404, deliberately, so that a miss cannot be used
///   to probe for another user's rows. This reports the scope, not a claim about
///   what exists.
///
/// The presented token is never included in the message: it is safe to log,
/// which is the point of putting the diagnosis in a type rather than in a string
/// the caller assembles from the credential.
///
/// [`AxonFlowError::ReadScope`]: crate::AxonFlowError::ReadScope
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadScopeRefusal {
    /// What was read, e.g. `"decision"`.
    pub resource: String,
    /// The identifier that was read; `None` for a list read.
    pub identifier: Option<String>,
    /// The scope the platform reported.
    pub scope: ReadScope,
    /// The HTTP status the platform answered with (404 for a scoped miss, 200
    /// for a scoped-empty page).
    pub status: u16,
}

impl ReadScopeRefusal {
    /// Whether the read failed because no per-user identity was RESOLVED, as
    /// opposed to one being resolved and not matching.
    pub fn identity_missing(&self) -> bool {
        self.scope == ReadScope::None
    }

    fn subject(&self) -> String {
        match &self.identifier {
            Some(id) => format!("{} {:?}", self.resource, id),
            None => self.resource.clone(),
        }
    }
}

impl fmt::Display for ReadScopeRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.identity_missing() {
            write!(
                f,
                "HTTP {}: {}: the platform resolved no per-user identity for this read \
                 ({HEADER_READ_SCOPE}: {}), so it returned zero rows by construction and the \
                 empty answer says nothing about what exists. Either no identity was presented \
                 — set AxonFlowConfig::user_token, use the *_as read methods, or derive a client \
                 with as_user(..) — or the one presented carries an address the platform \
                 reserves for shared identities (@axonflow.local, @axonflow.internal), which \
                 resolves to nobody. (platform #2922)",
                self.status,
                self.subject(),
                self.scope,
            )
        } else {
            write!(
                f,
                "HTTP {}: {} was not found among the rows this identity can see: the platform \
                 reports {HEADER_READ_SCOPE}: {}, so the read was narrowed to the identity's own \
                 rows. It is either not attributed to this identity or not there at all — the \
                 platform answers both the same way ON PURPOSE, so that a miss cannot be used to \
                 probe for the existence of another user's rows, and this SDK cannot tell them \
                 apart either. A tenant-wide role (admin, owner or policy_admin) reads the whole \
                 tenant. (platform #2922)",
                self.status,
                self.subject(),
                self.scope,
            )
        }
    }
}

/// The typed refusal for a scoped read that came back with nothing, or `None`
/// when the scope does not explain the result.
///
/// `None` for [`ReadScope::Tenant`] (the caller could see the whole tenant and it
/// still was not there — a genuine miss), for [`ReadScope::Absent`] (the platform
/// did not state a scope; see [`ReadScope`] for why absent is not none), and for
/// [`ReadScope::Other`] (a newer platform's; reporting a cause we cannot actually
/// read would be a confident wrong diagnosis).
pub fn read_scope_refusal(
    resource: &str,
    identifier: Option<&str>,
    scope: ReadScope,
    status: u16,
) -> Option<ReadScopeRefusal> {
    match scope {
        ReadScope::None | ReadScope::OwnRows => Some(ReadScopeRefusal {
            resource: resource.to_string(),
            identifier: identifier.map(str::to_string),
            scope,
            status,
        }),
        _ => None,
    }
}

/// The typed refusal for a scoped read that came back EMPTY under a scope that
/// could not have returned a row; `None` in every other case.
///
/// One helper rather than a check at each read, because "the page is empty and
/// the scope is none" is one rule and the reads that need it decode their body
/// on more than one path each. A rule copied per return site is a rule that ends
/// up applied on some of them.
///
/// The emptiness guard is as load-bearing as the scope guard: a non-empty page
/// is never turned into an error, whatever the header says. And only
/// [`ReadScope::None`] refuses — an own-rows or tenant-wide read that
/// legitimately found nothing is a real answer, and replacing it with an error
/// would swap one wrong report for another.
pub fn refuse_vacuous_scoped_page(
    resource: &str,
    scope: ReadScope,
    status: u16,
    rows: usize,
) -> Option<ReadScopeRefusal> {
    if rows > 0 || scope != ReadScope::None {
        return None;
    }
    Some(ReadScopeRefusal {
        resource: resource.to_string(),
        identifier: None,
        scope,
        status,
    })
}

/// The diagnosis for a per-user identity that cannot be an HTTP header value.
///
/// Reported rather than dropped. Dropping it is the worst of the three outcomes
/// available: the read then goes out unidentified, the platform answers with a
/// scoped-empty page, and the SDK tells the caller "no identity was presented"
/// — which is true of the wire and false of what they did. A caller who set a
/// token and gets told they set none has been sent to look in the wrong place.
///
/// The token is NEVER in the message. The offending byte's index and CLASS are,
/// because those are what you need to find it in your own minting code, and
/// neither is a fragment of the credential. The byte's value is withheld too:
/// for a non-ASCII byte it is one eighth of the secret.
pub(crate) fn unusable_token(token: &str) -> String {
    let offender = token
        .bytes()
        .position(|b| !(0x20..=0x7e).contains(&b) && b != b'\t');
    let detail = match offender.map(|i| (i, token.as_bytes()[i])) {
        Some((index, byte)) if byte.is_ascii() => format!(
            "an ASCII control character at byte {index} (an embedded newline or carriage \
             return is the usual cause; a LEADING or TRAILING one is trimmed on the way in, \
             so this one is inside the value)"
        ),
        Some((index, _)) => {
            format!("a non-ASCII byte at byte {index} (an HTTP header value is visible ASCII only)")
        }
        // Unreachable via HeaderValue::from_str, whose rejection set is exactly
        // the bytes tested above. Named rather than asserted: a future header
        // crate with a wider rule must not make this arm claim a cause it did
        // not observe.
        None => "a byte HTTP header values do not admit".to_string(),
    };
    format!(
        "the per-user identity is not a usable HTTP header value: it contains {detail}. \
         The token itself is deliberately omitted from this message. It was NOT sent, and the \
         read was NOT attempted — an unsendable identity is reported rather than dropped, \
         because a dropped one would have made this read silently unidentified. \
         (length {} bytes)",
        token.len()
    )
}

/// Whether two URLs are the same origin: scheme, host AND port.
///
/// Subdomains are NOT trusted, deliberately: this header is an identity
/// assertion, not a session cookie, and "close enough" is not a property an
/// identity should have.
pub(crate) fn same_origin(a: &url::Url, b: &url::Url) -> bool {
    a.scheme() == b.scheme()
        && a.host_str() == b.host_str()
        && a.port_or_known_default() == b.port_or_known_default()
}
