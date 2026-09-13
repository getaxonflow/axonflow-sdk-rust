//! The PEP capability handshake: what this enforcement point can discharge.
//!
//! An enforcement point (a PEP) declares, on each governed call, the exact
//! obligation types and schema versions it can carry out. The declaration rides
//! the `X-Axonflow-PEP-Handshake` header as the unpadded base64url encoding of
//! a compact JSON document:
//!
//! ```text
//! {"profile_version":1,"pep_id":"...","audience":"...",
//!  "capabilities":[{"type":"field_redact","version":1}]}
//! ```
//!
//! Which edition refuses what: from platform v11.0.0, on every edition,
//! `decide` refuses with `unsupported_obligation` a mandatory obligation the
//! enforcement point cannot discharge, judged against the declared
//! capabilities. An organization's redact override is the shipped case, so
//! under one a Community caller that declares `field_mask` but not
//! `field_redact` is refused too. On an Enterprise deployment, in addition, any
//! allow carrying a mandatory obligation outside the declared set becomes a
//! deny, so the enforcement point is never handed an instruction it would drop.
//! A capability in a family the deployment's edition does not issue is dropped
//! from the declaration, counted and logged; the request proceeds.
//!
//! # Where it is sent
//!
//! Only on the calls whose route reads it:
//! [`decide`](crate::AxonFlowClient::decide) (`/api/v1/decide`), the AuthZEN
//! evaluation route ([`evaluate`](crate::AxonFlowClient::evaluate) and
//! [`evaluate_all`](crate::AxonFlowClient::evaluate_all)), and the MCP
//! check-input round-trip that [`fulfill_request`](crate::AxonFlowClient::fulfill_request)
//! and [`decide_and_fulfill`](crate::AxonFlowClient::decide_and_fulfill) make.
//! `proxy_llm_call` and `query_connector` (`/api/request`) do not read it, and
//! the client never sends it there, nor on any other route. It is never a
//! default header. The platform also reads it on the gateway pre-check
//! (`/api/policy/pre-check`) and on MCP `tools/call`; this SDK calls neither.
//!
//! # Absent is not empty
//!
//! A client with no declaration sends no header. There is no default
//! declaration: only the caller knows what its enforcement point can
//! discharge. The platform reads the declaration from v10.4.0; from v11.0.0,
//! `decide` under an organization's redact override refuses a caller that does
//! not declare redaction, so a client that sends none is refused there. An
//! empty capability list is a declaration that the enforcement point
//! discharges nothing, so every mandatory obligation is one it cannot
//! discharge.
//!
//! # The rules are the platform's
//!
//! A declaration [`PEPHandshake::new`] accepts is one the platform's decoder
//! accepts, and one it would refuse fails here, at construction, naming the
//! member, instead of as a `400` on the first governed call.

use crate::authzen::AuthZenObligationType;
use crate::error::AxonFlowError;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use serde::Serialize;
use std::fmt;

/// The request header the declaration rides on.
pub const PEP_HANDSHAKE_HEADER: &str = "X-Axonflow-PEP-Handshake";
/// The only handshake profile the platform reads. Matched exactly, never as a
/// floor.
pub const PEP_HANDSHAKE_PROFILE_V1: u32 = 1;
/// The longest header value the platform reads, in bytes of base64.
pub const MAX_PEP_HANDSHAKE_BYTES: usize = 4096;
/// The most capabilities one declaration may carry. A surplus is refused, not
/// truncated.
pub const MAX_PEP_HANDSHAKE_CAPABILITIES: usize = 64;
/// The longest `pep_id` and `audience` the platform reads, in bytes.
const MAX_IDENTIFIER_BYTES: usize = 128;

/// One obligation type, at one schema version, that the enforcement point can
/// discharge.
///
/// Matching is exact on both members. Values order by `(type, version)`, which
/// is the platform's canonical order. The type must be one of
/// [`AuthZenObligationType::KNOWN_WIRE_VALUES`] and the version must be
/// positive; [`PEPHandshake::new`] refuses anything else.
///
/// Build one with [`PEPCapability::new`]. The struct is non-exhaustive so a
/// member can be added without breaking a caller's struct literal.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub struct PEPCapability {
    /// The obligation type, for example `"field_redact"`.
    pub r#type: String,
    /// The obligation's schema version. Positive.
    pub version: u32,
}

impl PEPCapability {
    /// A capability for `r#type` at `version`. Validated when it is declared in
    /// a [`PEPHandshake`].
    pub fn new(r#type: impl Into<String>, version: u32) -> Self {
        Self {
            r#type: r#type.into(),
            version,
        }
    }
}

/// A declaration the platform would refuse, found before anything was sent.
///
/// [`pointer`](Self::pointer) names the member at fault the way the platform
/// reports it: `/pep_id`, `/audience` or `/capabilities`, or empty when the
/// whole document encodes past the header's size limit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PEPHandshakeError {
    pointer: &'static str,
    message: String,
}

impl PEPHandshakeError {
    fn new(pointer: &'static str, message: String) -> Self {
        Self { pointer, message }
    }

    /// The JSON Pointer of the member at fault, or `""` for the whole document.
    pub fn pointer(&self) -> &str {
        self.pointer
    }

    /// What is wrong with it.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for PEPHandshakeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.pointer.is_empty() {
            write!(f, "{PEP_HANDSHAKE_HEADER}: {}", self.message)
        } else {
            write!(
                f,
                "{PEP_HANDSHAKE_HEADER}: {}: {}",
                self.pointer, self.message
            )
        }
    }
}

impl std::error::Error for PEPHandshakeError {}

/// A declaration is built before a client exists, so its error is a value
/// error of its own. Inside a function that returns [`AxonFlowError`], `?`
/// turns it into the existing configuration error rather than a new variant.
impl From<PEPHandshakeError> for AxonFlowError {
    fn from(e: PEPHandshakeError) -> Self {
        AxonFlowError::ConfigError(e.to_string())
    }
}

/// A capability declaration, validated and encoded once.
///
/// Set it on [`AxonFlowConfig::pep_handshake`](crate::AxonFlowConfig::pep_handshake)
/// to declare it on every call to a plane that reads it, or derive a client that
/// presents a different one with
/// [`AxonFlowClient::with_pep_handshake`](crate::AxonFlowClient::with_pep_handshake).
/// One process can be two enforcement points (a request path and a response
/// path discharging different obligations), and the derived client is how each
/// presents its own.
///
/// ```
/// use axonflow_sdk_rust::{PEPCapability, PEPHandshake};
///
/// let declared = PEPHandshake::new(
///     "gateway",
///     "https://pep.example.test",
///     [PEPCapability::new("field_redact", 1)],
/// )?;
/// assert_eq!(declared.capabilities().len(), 1);
/// # Ok::<(), axonflow_sdk_rust::PEPHandshakeError>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PEPHandshake {
    pep_id: String,
    audience: String,
    capabilities: Vec<PEPCapability>,
    header_value: String,
}

impl PEPHandshake {
    /// Validates a declaration by the platform's rules and encodes it.
    ///
    /// - `pep_id` names this enforcement point within the client's credential:
    ///   lower-case letters, digits, `.`, `_` and `-`, starting with a letter or
    ///   digit, at most 128 bytes. The platform prefixes it with the
    ///   authenticated credential, so it cannot name another client's
    ///   enforcement point, and `:` (that prefix's separator) is refused.
    /// - `audience` is the audience a decision proof is bound to: letters,
    ///   digits, `.`, `_`, `:`, `/` and `-`, starting with a letter or digit, at
    ///   most 128 bytes. A URI is the usual form. It is recorded and bound, and
    ///   authorises nothing.
    /// - `capabilities` is the exact set this enforcement point can discharge:
    ///   at most 64, each a known obligation type at a positive version, none
    ///   repeated. It is stored in canonical order, so two declarations of the
    ///   same set in a different order are equal and encode to the same bytes.
    ///   An empty set is a declaration that it discharges nothing.
    ///
    /// # Errors
    ///
    /// [`PEPHandshakeError`] naming the member the platform would refuse, or
    /// with an empty pointer when the whole document encodes past 4096 bytes.
    pub fn new(
        pep_id: impl Into<String>,
        audience: impl Into<String>,
        capabilities: impl IntoIterator<Item = PEPCapability>,
    ) -> Result<Self, PEPHandshakeError> {
        let pep_id = pep_id.into();
        let audience = audience.into();
        if !well_formed(&pep_id, is_identifier_start, is_identifier_byte) {
            return Err(PEPHandshakeError::new(
                "/pep_id",
                format!(
                    "{pep_id:?} is not of the form [a-z0-9][a-z0-9._-]* with at most \
                     {MAX_IDENTIFIER_BYTES} bytes"
                ),
            ));
        }
        if !well_formed(&audience, is_audience_start, is_audience_byte) {
            return Err(PEPHandshakeError::new(
                "/audience",
                format!(
                    "{audience:?} is not of the form [A-Za-z0-9][A-Za-z0-9._:/-]* with at most \
                     {MAX_IDENTIFIER_BYTES} bytes"
                ),
            ));
        }
        let capabilities = canonical_capabilities(capabilities.into_iter().collect())?;
        let header_value = encode(&pep_id, &audience, &capabilities);
        if header_value.len() > MAX_PEP_HANDSHAKE_BYTES {
            return Err(PEPHandshakeError::new(
                "",
                format!(
                    "encodes to {} bytes; the header carries at most {MAX_PEP_HANDSHAKE_BYTES}",
                    header_value.len()
                ),
            ));
        }
        Ok(Self {
            pep_id,
            audience,
            capabilities,
            header_value,
        })
    }

    /// The enforcement point's name within the client's credential.
    pub fn pep_id(&self) -> &str {
        &self.pep_id
    }

    /// The audience a decision proof is bound to.
    pub fn audience(&self) -> &str {
        &self.audience
    }

    /// The declared capabilities, in canonical `(type, version)` order.
    pub fn capabilities(&self) -> &[PEPCapability] {
        &self.capabilities
    }

    /// The `X-Axonflow-PEP-Handshake` value this declaration is sent as.
    pub fn header_value(&self) -> &str {
        &self.header_value
    }
}

// The two identifier grammars, as byte classes. The platform matches them with
// anchored RE2 patterns, where `$` is the end of the text, so a trailing
// newline is refused there and must be refused here; a byte-class walk over the
// WHOLE string cannot match a prefix. Every admitted byte is ASCII, so the byte
// length bounded below is the length the platform bounds.
fn is_identifier_start(b: u8) -> bool {
    b.is_ascii_lowercase() || b.is_ascii_digit()
}

fn is_identifier_byte(b: u8) -> bool {
    is_identifier_start(b) || matches!(b, b'.' | b'_' | b'-')
}

fn is_audience_start(b: u8) -> bool {
    b.is_ascii_alphanumeric()
}

fn is_audience_byte(b: u8) -> bool {
    is_audience_start(b) || matches!(b, b'.' | b'_' | b':' | b'/' | b'-')
}

fn well_formed(value: &str, start: fn(u8) -> bool, rest: fn(u8) -> bool) -> bool {
    match value.as_bytes() {
        [] => false,
        bytes if bytes.len() > MAX_IDENTIFIER_BYTES => false,
        [first, tail @ ..] => start(*first) && tail.iter().all(|b| rest(*b)),
    }
}

/// Validates and sorts the declared set, in the platform's order of checks: the
/// count first, then each capability in canonical order.
fn canonical_capabilities(
    mut capabilities: Vec<PEPCapability>,
) -> Result<Vec<PEPCapability>, PEPHandshakeError> {
    if capabilities.len() > MAX_PEP_HANDSHAKE_CAPABILITIES {
        return Err(PEPHandshakeError::new(
            "/capabilities",
            format!(
                "declares {} capabilities; the platform reads at most \
                 {MAX_PEP_HANDSHAKE_CAPABILITIES}",
                capabilities.len()
            ),
        ));
    }
    capabilities.sort();
    for (i, c) in capabilities.iter().enumerate() {
        if !AuthZenObligationType::KNOWN_WIRE_VALUES.contains(&c.r#type.as_str()) {
            return Err(PEPHandshakeError::new(
                "/capabilities",
                format!(
                    "names obligation type {:?}, which is not one of {:?}",
                    c.r#type,
                    AuthZenObligationType::KNOWN_WIRE_VALUES
                ),
            ));
        }
        // Matching is exact, so a capability at version 0 would match only an
        // obligation whose version was never set.
        if c.version == 0 {
            return Err(PEPHandshakeError::new(
                "/capabilities",
                format!(
                    "declares {:?} at version 0; a version is a positive integer",
                    c.r#type
                ),
            ));
        }
        // Sorted, so a repeat is always adjacent to its first occurrence.
        if i > 0 && capabilities[i - 1] == *c {
            return Err(PEPHandshakeError::new(
                "/capabilities",
                format!(
                    "declares {:?} at version {} more than once; the platform refuses a \
                     repeated capability",
                    c.r#type, c.version
                ),
            ));
        }
    }
    Ok(capabilities)
}

/// The compact JSON document, members in the platform's order, then unpadded
/// base64url.
///
/// A private struct rather than a map: `serde_json`'s map sorts its keys, and
/// the platform's encoder emits `profile_version, pep_id, audience,
/// capabilities` in that order. Go's encoder also escapes `<`, `>` and `&`; no
/// identifier or obligation type can contain one, so the bytes agree.
fn encode(pep_id: &str, audience: &str, capabilities: &[PEPCapability]) -> String {
    #[derive(Serialize)]
    struct Document<'a> {
        profile_version: u32,
        pep_id: &'a str,
        audience: &'a str,
        capabilities: Vec<Capability<'a>>,
    }
    #[derive(Serialize)]
    struct Capability<'a> {
        r#type: &'a str,
        version: u32,
    }
    let document = Document {
        profile_version: PEP_HANDSHAKE_PROFILE_V1,
        pep_id,
        audience,
        capabilities: capabilities
            .iter()
            .map(|c| Capability {
                r#type: &c.r#type,
                version: c.version,
            })
            .collect(),
    };
    // Serializing borrowed strings and integers cannot fail.
    let raw = serde_json::to_vec(&document).expect("the handshake document always serializes");
    URL_SAFE_NO_PAD.encode(raw)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn caps(pairs: &[(&str, u32)]) -> Vec<PEPCapability> {
        pairs
            .iter()
            .map(|(t, v)| PEPCapability::new(*t, *v))
            .collect()
    }

    fn refused(
        pep_id: &str,
        audience: &str,
        capabilities: Vec<PEPCapability>,
    ) -> PEPHandshakeError {
        PEPHandshake::new(pep_id, audience, capabilities)
            .expect_err("the platform refuses this declaration, so construction must too")
    }

    fn decoded(value: &str) -> Vec<u8> {
        URL_SAFE_NO_PAD.decode(value).expect("unpadded base64url")
    }

    // ---- parity with the platform's encoder -------------------------------

    /// The vectors the platform's reference encoder produced (and its decoder
    /// accepted), vendored with their source in `testdata/`.
    #[test]
    fn the_bytes_are_the_platform_encoders() {
        let golden: Value =
            serde_json::from_str(include_str!("../testdata/pep_handshake_golden.json"))
                .expect("golden file");
        let vectors = golden["vectors"].as_array().expect("vectors");
        assert_eq!(vectors.len(), 4, "every vendored vector is checked");
        for v in vectors {
            let pairs: Vec<PEPCapability> = v["capabilities"]
                .as_array()
                .expect("capabilities")
                .iter()
                .map(|c| {
                    PEPCapability::new(
                        c["type"].as_str().expect("type"),
                        u32::try_from(c["version"].as_u64().expect("version")).expect("u32"),
                    )
                })
                .collect();
            let declared = PEPHandshake::new(
                v["pep_id"].as_str().expect("pep_id"),
                v["audience"].as_str().expect("audience"),
                pairs,
            )
            .expect("a platform-encoded declaration is accepted");
            assert_eq!(
                declared.header_value(),
                v["header"].as_str().expect("header"),
                "vector {}",
                v["id"]
            );
        }
    }

    /// The platform's construction of the 64-capability vector: every declared
    /// type in canonical order at version 1, then 2, and so on, stopping at the
    /// count cap.
    ///
    /// Built from this build's vocabulary on purpose, as Python's test builds
    /// it: when the vocabulary gains a type this fails, and that is exactly
    /// when the golden vector must be regenerated from the platform.
    #[test]
    fn sixty_four_capabilities_are_built_the_way_the_platform_built_them() {
        let mut kinds = AuthZenObligationType::KNOWN_WIRE_VALUES.to_vec();
        kinds.sort_unstable();
        let pairs: Vec<PEPCapability> = (1..=5)
            .flat_map(|v| kinds.iter().map(move |k| PEPCapability::new(*k, v)))
            .take(MAX_PEP_HANDSHAKE_CAPABILITIES)
            .collect();
        let declared = PEPHandshake::new("p", "a", pairs).expect("64 is the cap, not over it");
        let golden: Value =
            serde_json::from_str(include_str!("../testdata/pep_handshake_golden.json"))
                .expect("golden file");
        let sixty_four = golden["vectors"]
            .as_array()
            .expect("vectors")
            .iter()
            .find(|v| v["id"] == "sixty-four")
            .expect("the sixty-four vector");
        assert_eq!(
            declared.header_value(),
            sixty_four["header"].as_str().unwrap()
        );
        assert_eq!(declared.header_value().len(), 3364);
    }

    #[test]
    fn the_header_is_unpadded_base64url_of_the_canonical_document() {
        let declared = PEPHandshake::new(
            "gw",
            "https://pep.example.test",
            caps(&[("field_redact", 2), ("field_redact", 1)]),
        )
        .unwrap();
        let value = declared.header_value();
        assert!(!value.contains(['=', '+', '/']), "{value}");
        assert_eq!(
            decoded(value),
            br#"{"profile_version":1,"pep_id":"gw","audience":"https://pep.example.test","capabilities":[{"type":"field_redact","version":1},{"type":"field_redact","version":2}]}"#
        );
    }

    #[test]
    fn the_order_of_declaration_does_not_change_the_bytes() {
        let given = [
            ("notification", 3),
            ("field_redact", 2),
            ("approval_challenge", 1),
        ];
        let mut reversed = given;
        reversed.reverse();
        let one = PEPHandshake::new("gw", "a", caps(&given)).unwrap();
        let other = PEPHandshake::new("gw", "a", caps(&reversed)).unwrap();
        assert_eq!(one.header_value(), other.header_value());
        assert_eq!(one, other);
        let mut sorted = caps(&given);
        sorted.sort();
        assert_eq!(one.capabilities(), sorted.as_slice());
    }

    /// Absent is not empty: an empty set is a declaration and encodes as `[]`.
    #[test]
    fn an_empty_declaration_is_a_declaration() {
        let declared = PEPHandshake::new("gw", "a", Vec::new()).unwrap();
        let document: Value = serde_json::from_slice(&decoded(declared.header_value())).unwrap();
        assert_eq!(document["capabilities"], serde_json::json!([]));
        assert_eq!(document["profile_version"], 1);
    }

    // ---- pep_id -------------------------------------------------------------

    #[test]
    fn pep_id_at_127_and_128_bytes_is_accepted() {
        PEPHandshake::new("g".repeat(127), "a", Vec::new()).expect("127 bytes");
        PEPHandshake::new("g".repeat(128), "a", Vec::new()).expect("128 bytes");
    }

    #[test]
    fn pep_id_at_129_bytes_is_refused() {
        assert_eq!(
            refused(&"g".repeat(129), "a", Vec::new()).pointer(),
            "/pep_id"
        );
    }

    #[test]
    fn pep_id_empty_is_refused() {
        assert_eq!(refused("", "a", Vec::new()).pointer(), "/pep_id");
    }

    #[test]
    fn pep_id_grammar_is_the_platforms() {
        for bad in [
            "Gateway",   // upper case
            "client:gw", // the credential separator
            "-gw",       // must start with a letter or digit
            ".gw",       // must start with a letter or digit
            "_gw",       // must start with a letter or digit
            "gw\n",      // RE2's `$` is the end of the text
            "gw/1",      // `/` is an audience byte, not an identifier byte
            "g w",       // no space
            "café",      // non-ASCII
        ] {
            assert_eq!(
                refused(bad, "a", Vec::new()).pointer(),
                "/pep_id",
                "{bad:?}"
            );
        }
        for good in ["a", "0", "gw.request-1", "gw_1", "9-a.b_c"] {
            PEPHandshake::new(good, "a", Vec::new()).unwrap_or_else(|e| panic!("{good:?}: {e}"));
        }
    }

    // ---- audience -----------------------------------------------------------

    #[test]
    fn audience_at_127_and_128_bytes_is_accepted() {
        PEPHandshake::new("gw", "A".repeat(127), Vec::new()).expect("127 bytes");
        PEPHandshake::new("gw", "A".repeat(128), Vec::new()).expect("128 bytes");
    }

    #[test]
    fn audience_at_129_bytes_is_refused() {
        assert_eq!(
            refused("gw", &"a".repeat(129), Vec::new()).pointer(),
            "/audience"
        );
    }

    #[test]
    fn audience_empty_is_refused() {
        assert_eq!(refused("gw", "", Vec::new()).pointer(), "/audience");
    }

    #[test]
    fn audience_grammar_is_the_platforms() {
        for bad in [
            "/aud",
            ":aud",
            "-aud",
            "a b",
            "aud\n",
            "aud?x",
            "aud#x",
            "ünïcode",
        ] {
            assert_eq!(
                refused("gw", bad, Vec::new()).pointer(),
                "/audience",
                "{bad:?}"
            );
        }
        for good in [
            "A",
            "https://api.example.com/v1",
            "urn:example:aud",
            "Mixed.Case_aud-1",
        ] {
            PEPHandshake::new("gw", good, Vec::new()).unwrap_or_else(|e| panic!("{good:?}: {e}"));
        }
    }

    /// The pep_id is checked before the audience, as the platform checks it.
    #[test]
    fn pep_id_is_reported_before_audience() {
        assert_eq!(refused("", "", Vec::new()).pointer(), "/pep_id");
    }

    // ---- capabilities -------------------------------------------------------

    fn distinct(n: u32) -> Vec<PEPCapability> {
        (1..=n)
            .map(|v| PEPCapability::new("field_redact", v))
            .collect()
    }

    #[test]
    fn zero_one_and_sixty_four_capabilities_are_accepted() {
        for n in [0, 1, 64] {
            let declared = PEPHandshake::new("gw", "a", distinct(n))
                .unwrap_or_else(|e| panic!("{n} capabilities: {e}"));
            assert_eq!(declared.capabilities().len(), n as usize);
        }
    }

    #[test]
    fn sixty_five_capabilities_are_refused() {
        let e = refused("gw", "a", distinct(65));
        assert_eq!(e.pointer(), "/capabilities");
        assert!(e.message().contains("65"), "{e}");
    }

    /// The count is checked before any capability, as the platform checks it:
    /// 65 entries with an unknown type are reported as too many.
    #[test]
    fn the_count_is_checked_before_the_entries() {
        let mut over = distinct(64);
        over.push(PEPCapability::new("redact_pii", 1));
        let e = refused("gw", "a", over);
        assert!(e.message().contains("at most 64"), "{e}");
    }

    #[test]
    fn a_repeated_capability_is_refused() {
        let e = refused(
            "gw",
            "a",
            caps(&[("field_redact", 1), ("field_mask", 1), ("field_redact", 1)]),
        );
        assert_eq!(e.pointer(), "/capabilities");
        assert!(e.message().contains("more than once"), "{e}");
    }

    #[test]
    fn the_same_type_at_two_versions_is_not_a_repeat() {
        PEPHandshake::new("gw", "a", caps(&[("field_redact", 1), ("field_redact", 2)]))
            .expect("two versions are two capabilities");
    }

    #[test]
    fn version_zero_is_refused() {
        let e = refused("gw", "a", caps(&[("field_redact", 0)]));
        assert_eq!(e.pointer(), "/capabilities");
        assert!(e.message().contains("version 0"), "{e}");
    }

    #[test]
    fn version_one_is_accepted() {
        PEPHandshake::new("gw", "a", caps(&[("field_redact", 1)])).expect("version 1");
    }

    #[test]
    fn an_obligation_type_the_platform_cannot_match_is_refused() {
        for bad in ["redact_pii", "Field_Redact", "field_redact ", ""] {
            let e = refused("gw", "a", caps(&[(bad, 1)]));
            assert_eq!(e.pointer(), "/capabilities", "{bad:?}");
            assert!(e.message().contains("not one of"), "{bad:?}: {e}");
        }
    }

    #[test]
    fn every_obligation_type_this_build_declares_is_accepted() {
        for kind in AuthZenObligationType::KNOWN_WIRE_VALUES {
            PEPHandshake::new("gw", "a", caps(&[(*kind, 1)]))
                .unwrap_or_else(|e| panic!("{kind}: {e}"));
        }
    }

    /// The vocabulary is the generated AuthZEN surface's, and it must be the
    /// platform's own list: `contract.AllObligationTypes()` at platform commit
    /// 857455033b2ea4686835eac037fca41f948e4515
    /// (`platform/decision/contract/obligation.go:113`, the keys of
    /// `obligationFamilies`, sorted).
    #[test]
    fn the_vocabulary_is_the_platforms_obligation_types() {
        let platform = [
            "approval_challenge",
            "field_annotate",
            "field_hash",
            "field_mask",
            "field_redact",
            "field_remove",
            "field_tokenize",
            "immutable_audit",
            "notification",
            "quota_reservation",
            "response_filter",
            "route_restriction",
            "schema_transform",
            "step_up_authentication",
        ];
        let mut known = AuthZenObligationType::KNOWN_WIRE_VALUES.to_vec();
        known.sort_unstable();
        assert_eq!(known, platform);
    }

    // ---- the whole-document byte cap ---------------------------------------

    /// A declaration whose document is `json_len` bytes, found by search so the
    /// test states the boundary rather than a hand-tuned fixture.
    fn declaration_with_document_length(json_len: usize) -> (String, Vec<PEPCapability>) {
        for count in 1..=MAX_PEP_HANDSHAKE_CAPABILITIES as u32 {
            let capabilities: Vec<PEPCapability> = (0..count)
                .map(|i| PEPCapability::new("step_up_authentication", 1_000_000_000 + i))
                .collect();
            for audience_len in 1..=MAX_IDENTIFIER_BYTES {
                let audience = "a".repeat(audience_len);
                let len = decoded(&encode("p", &audience, &capabilities)).len();
                if len == json_len {
                    return (audience, capabilities);
                }
            }
        }
        panic!("no declaration found with a {json_len}-byte document");
    }

    /// Unpadded base64 of n bytes is never 1 mod 4 long, so the cap's
    /// neighbours are 4096 (a 3072-byte document, accepted) and 4098 (3073
    /// bytes, refused as a whole: the pointer is empty).
    #[test]
    fn the_byte_cap_is_4096_and_refuses_the_whole_document() {
        let (audience, capabilities) = declaration_with_document_length(3072);
        let at_cap = PEPHandshake::new("p", audience, capabilities).expect("4096 is the cap");
        assert_eq!(at_cap.header_value().len(), 4096);

        let (audience, capabilities) = declaration_with_document_length(3073);
        assert_eq!(encode("p", &audience, &capabilities).len(), 4098);
        let e = refused("p", &audience, capabilities);
        assert_eq!(e.pointer(), "", "the document is at fault, not a member");
        assert!(e.message().contains("4098"), "{e}");
    }

    // ---- the error ----------------------------------------------------------

    #[test]
    fn the_error_names_the_header_and_the_member() {
        let e = refused("Gateway", "a", Vec::new());
        assert!(
            e.to_string()
                .starts_with("X-Axonflow-PEP-Handshake: /pep_id: "),
            "{e}"
        );
        let whole = PEPHandshakeError::new("", "encodes to 4098 bytes".into());
        assert_eq!(
            whole.to_string(),
            "X-Axonflow-PEP-Handshake: encodes to 4098 bytes"
        );
    }

    #[test]
    fn the_error_converts_into_the_existing_configuration_error() {
        let e = refused("Gateway", "a", Vec::new());
        let rendered = e.to_string();
        match AxonFlowError::from(e) {
            AxonFlowError::ConfigError(message) => assert_eq!(message, rendered),
            other => panic!("expected ConfigError, got {other:?}"),
        }
    }
}
