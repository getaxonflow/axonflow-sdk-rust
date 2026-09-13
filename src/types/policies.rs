use serde::{Deserialize, Serialize};
use std::fmt;

/// Policy categories for organization and filtering.
///
/// The platform ships and extends its categories as data, so a category this
/// build does not name arrives as [`PolicyCategory::Unknown`], carrying the
/// platform's string verbatim, rather than failing the read it arrived in. It
/// re-serializes byte-identical. The known set is pinned to the categories the
/// platform's shipped posture uses (`testdata/shipped_posture_categories.json`),
/// because the spec's own enum is stale (getaxonflow/axonflow-enterprise#4224).
///
/// `#[non_exhaustive]` because `Unknown` does not make this enum additive for a
/// downstream crate: a match over the known variants plus `Unknown(_)` is
/// exhaustive today and would stop compiling the moment a category is added.
/// With the attribute, that match needs a `_` arm, and a new category is a
/// minor release rather than a breaking one.
///
/// Cross-SDK parity:
///   Go:     axonflow-sdk-go/policies.go (PolicyCategory)
///   Python: axonflow-sdk-python/axonflow/policies.py (PolicyCategory)
///   TS:     axonflow-sdk-typescript/src/types/policies.ts (PolicyCategory)
///   Java:   axonflow-sdk-java PolicyTypes.PolicyCategory
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(from = "String", into = "String")]
#[non_exhaustive]
pub enum PolicyCategory {
    /// `security-sqli`
    SecuritySqli,
    /// `security-admin`
    SecurityAdmin,
    /// `pii-global`
    PiiGlobal,
    /// `pii-us`
    PiiUs,
    /// `pii-eu`
    PiiEu,
    /// `pii-india`
    PiiIndia,
    /// `pii-singapore`
    PiiSingapore,
    /// `pii-indonesia`
    PiiIndonesia,
    /// `code-secrets`
    CodeSecrets,
    /// `code-unsafe`
    CodeUnsafe,
    /// `code-compliance`
    CodeCompliance,
    /// `sensitive-data`
    SensitiveData,
    /// `media-safety`
    MediaSafety,
    /// `media-biometric`
    MediaBiometric,
    /// `media-document`
    MediaDocument,
    /// `media-pii`
    MediaPii,
    /// `dynamic-risk`
    DynamicRisk,
    /// `dynamic-compliance`
    DynamicCompliance,
    /// `dynamic-security`
    DynamicSecurity,
    /// `dynamic-cost`
    DynamicCost,
    /// `dynamic-access`
    DynamicAccess,
    /// `security-dangerous`
    SecurityDangerous,
    /// `compliance-euaiact`
    ComplianceEuaiact,
    /// `dangerous_queries`
    DangerousQueries,
    /// `pii_detection`
    PiiDetection,
    /// `sql_injection`
    SqlInjection,
    /// A category this build does not know, carried verbatim.
    Unknown(String),
}

impl PolicyCategory {
    /// Every wire value this build knows.
    pub const KNOWN_WIRE_VALUES: &'static [&'static str] = &[
        "security-sqli",
        "security-admin",
        "pii-global",
        "pii-us",
        "pii-eu",
        "pii-india",
        "pii-singapore",
        "pii-indonesia",
        "code-secrets",
        "code-unsafe",
        "code-compliance",
        "sensitive-data",
        "media-safety",
        "media-biometric",
        "media-document",
        "media-pii",
        "dynamic-risk",
        "dynamic-compliance",
        "dynamic-security",
        "dynamic-cost",
        "dynamic-access",
        "security-dangerous",
        "compliance-euaiact",
        "dangerous_queries",
        "pii_detection",
        "sql_injection",
    ];

    /// The wire value.
    pub fn as_str(&self) -> &str {
        match self {
            Self::SecuritySqli => "security-sqli",
            Self::SecurityAdmin => "security-admin",
            Self::PiiGlobal => "pii-global",
            Self::PiiUs => "pii-us",
            Self::PiiEu => "pii-eu",
            Self::PiiIndia => "pii-india",
            Self::PiiSingapore => "pii-singapore",
            Self::PiiIndonesia => "pii-indonesia",
            Self::CodeSecrets => "code-secrets",
            Self::CodeUnsafe => "code-unsafe",
            Self::CodeCompliance => "code-compliance",
            Self::SensitiveData => "sensitive-data",
            Self::MediaSafety => "media-safety",
            Self::MediaBiometric => "media-biometric",
            Self::MediaDocument => "media-document",
            Self::MediaPii => "media-pii",
            Self::DynamicRisk => "dynamic-risk",
            Self::DynamicCompliance => "dynamic-compliance",
            Self::DynamicSecurity => "dynamic-security",
            Self::DynamicCost => "dynamic-cost",
            Self::DynamicAccess => "dynamic-access",
            Self::SecurityDangerous => "security-dangerous",
            Self::ComplianceEuaiact => "compliance-euaiact",
            Self::DangerousQueries => "dangerous_queries",
            Self::PiiDetection => "pii_detection",
            Self::SqlInjection => "sql_injection",
            Self::Unknown(v) => v.as_str(),
        }
    }

    /// Whether this is a value this build knows.
    ///
    /// A false result is not an error: a newer platform ships categories this
    /// SDK was built without. It IS a reason not to treat the value as
    /// equivalent to any known one.
    pub fn is_known(&self) -> bool {
        !matches!(self, Self::Unknown(_))
    }
}

impl From<String> for PolicyCategory {
    fn from(v: String) -> Self {
        match v.as_str() {
            "security-sqli" => Self::SecuritySqli,
            "security-admin" => Self::SecurityAdmin,
            "pii-global" => Self::PiiGlobal,
            "pii-us" => Self::PiiUs,
            "pii-eu" => Self::PiiEu,
            "pii-india" => Self::PiiIndia,
            "pii-singapore" => Self::PiiSingapore,
            "pii-indonesia" => Self::PiiIndonesia,
            "code-secrets" => Self::CodeSecrets,
            "code-unsafe" => Self::CodeUnsafe,
            "code-compliance" => Self::CodeCompliance,
            "sensitive-data" => Self::SensitiveData,
            "media-safety" => Self::MediaSafety,
            "media-biometric" => Self::MediaBiometric,
            "media-document" => Self::MediaDocument,
            "media-pii" => Self::MediaPii,
            "dynamic-risk" => Self::DynamicRisk,
            "dynamic-compliance" => Self::DynamicCompliance,
            "dynamic-security" => Self::DynamicSecurity,
            "dynamic-cost" => Self::DynamicCost,
            "dynamic-access" => Self::DynamicAccess,
            "security-dangerous" => Self::SecurityDangerous,
            "compliance-euaiact" => Self::ComplianceEuaiact,
            "dangerous_queries" => Self::DangerousQueries,
            "pii_detection" => Self::PiiDetection,
            "sql_injection" => Self::SqlInjection,
            _ => Self::Unknown(v),
        }
    }
}

impl From<PolicyCategory> for String {
    fn from(v: PolicyCategory) -> Self {
        match v {
            PolicyCategory::Unknown(s) => s,
            known => known.as_str().to_string(),
        }
    }
}

impl fmt::Display for PolicyCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn posture() -> Value {
        serde_json::from_str(include_str!(
            "../../testdata/shipped_posture_categories.json"
        ))
        .expect("the vendored posture fixture")
    }

    #[test]
    fn pii_indonesia_serializes_to_wire_name() {
        let cat = PolicyCategory::PiiIndonesia;
        let json = serde_json::to_string(&cat).unwrap();
        assert_eq!(json, r#""pii-indonesia""#);
    }

    #[test]
    fn pii_indonesia_round_trips() {
        let cat = PolicyCategory::PiiIndonesia;
        let json = serde_json::to_string(&cat).unwrap();
        let back: PolicyCategory = serde_json::from_str(&json).unwrap();
        assert_eq!(back, PolicyCategory::PiiIndonesia);
    }

    #[test]
    fn all_categories_serialize_to_expected_wire_names() {
        let cases = vec![
            (PolicyCategory::SecuritySqli, "security-sqli"),
            (PolicyCategory::SecurityAdmin, "security-admin"),
            (PolicyCategory::PiiGlobal, "pii-global"),
            (PolicyCategory::PiiUs, "pii-us"),
            (PolicyCategory::PiiEu, "pii-eu"),
            (PolicyCategory::PiiIndia, "pii-india"),
            (PolicyCategory::PiiSingapore, "pii-singapore"),
            (PolicyCategory::PiiIndonesia, "pii-indonesia"),
            (PolicyCategory::CodeSecrets, "code-secrets"),
            (PolicyCategory::CodeUnsafe, "code-unsafe"),
            (PolicyCategory::CodeCompliance, "code-compliance"),
            (PolicyCategory::SensitiveData, "sensitive-data"),
            (PolicyCategory::MediaSafety, "media-safety"),
            (PolicyCategory::MediaBiometric, "media-biometric"),
            (PolicyCategory::MediaDocument, "media-document"),
            (PolicyCategory::MediaPii, "media-pii"),
            (PolicyCategory::DynamicRisk, "dynamic-risk"),
            (PolicyCategory::DynamicCompliance, "dynamic-compliance"),
            (PolicyCategory::DynamicSecurity, "dynamic-security"),
            (PolicyCategory::DynamicCost, "dynamic-cost"),
            (PolicyCategory::DynamicAccess, "dynamic-access"),
            (PolicyCategory::SecurityDangerous, "security-dangerous"),
            (PolicyCategory::ComplianceEuaiact, "compliance-euaiact"),
            (PolicyCategory::DangerousQueries, "dangerous_queries"),
            (PolicyCategory::PiiDetection, "pii_detection"),
            (PolicyCategory::SqlInjection, "sql_injection"),
        ];
        assert_eq!(cases.len(), PolicyCategory::KNOWN_WIRE_VALUES.len());
        for (variant, expected) in cases {
            let json = serde_json::to_string(&variant).unwrap();
            assert_eq!(json, format!(r#""{}""#, expected), "variant {:?}", variant);
            let back: PolicyCategory = serde_json::from_str(&json).unwrap();
            assert_eq!(back, variant, "{expected} parses back to its variant");
        }
    }

    #[test]
    fn every_known_wire_value_parses_to_a_known_variant_and_back() {
        for value in PolicyCategory::KNOWN_WIRE_VALUES {
            let category: PolicyCategory =
                serde_json::from_value(Value::String(value.to_string())).unwrap();
            assert!(category.is_known(), "{value} must be known");
            assert_eq!(category.as_str(), *value);
            assert_eq!(String::from(category), *value);
        }
    }

    /// The categories the platform's shipped posture uses that this enum
    /// lacked: `security-dangerous` failed a whole static-policy read in the
    /// other SDKs before they learned it.
    #[test]
    fn the_five_categories_the_platform_added_are_known() {
        for (value, variant) in [
            ("security-dangerous", PolicyCategory::SecurityDangerous),
            ("compliance-euaiact", PolicyCategory::ComplianceEuaiact),
            ("dangerous_queries", PolicyCategory::DangerousQueries),
            ("pii_detection", PolicyCategory::PiiDetection),
            ("sql_injection", PolicyCategory::SqlInjection),
        ] {
            let parsed: PolicyCategory = serde_json::from_str(&format!("\"{value}\"")).unwrap();
            assert_eq!(parsed, variant, "{value}");
        }
    }

    /// A category from a later platform is kept, not refused, and goes back
    /// out exactly as it came in.
    #[test]
    fn an_unknown_category_is_kept_and_re_serializes_byte_identical() {
        let wire = r#""a-category-from-a-later-platform""#;
        let parsed: PolicyCategory = serde_json::from_str(wire).unwrap();
        assert_eq!(
            parsed,
            PolicyCategory::Unknown("a-category-from-a-later-platform".into())
        );
        assert!(!parsed.is_known());
        assert_eq!(parsed.as_str(), "a-category-from-a-later-platform");
        assert_eq!(parsed.to_string(), "a-category-from-a-later-platform");
        assert_eq!(serde_json::to_string(&parsed).unwrap(), wire);
    }

    /// The read that failed in the other SDKs: an object carrying a category
    /// this enum did not name deserializes, and the category is kept.
    #[test]
    fn a_platform_object_with_an_unknown_category_deserializes() {
        #[derive(Deserialize)]
        struct Policy {
            category: PolicyCategory,
        }
        let known: Policy = serde_json::from_str(r#"{"category":"security-dangerous"}"#).unwrap();
        assert_eq!(known.category, PolicyCategory::SecurityDangerous);
        let unknown: Policy = serde_json::from_str(r#"{"category":"brand-new"}"#).unwrap();
        assert_eq!(
            unknown.category,
            PolicyCategory::Unknown("brand-new".into())
        );
    }

    /// Pinned to the platform's shipped posture, because the spec's enum is
    /// stale (getaxonflow/axonflow-enterprise#4224): a category the platform
    /// adds to its posture fails here, not as a read that loses its type.
    #[test]
    fn every_category_the_shipped_posture_uses_is_known() {
        let posture = posture();
        let missing: Vec<&str> = posture["categories"]
            .as_array()
            .expect("categories")
            .iter()
            .map(|c| c.as_str().expect("a category string"))
            .filter(|c| !PolicyCategory::KNOWN_WIRE_VALUES.contains(c))
            .collect();
        assert!(
            missing.is_empty(),
            "the platform's shipped posture at {} uses categories PolicyCategory lacks: \
             {missing:?} (getaxonflow/axonflow-enterprise#4224)",
            posture["platform_commit"]
        );
    }

    /// Every variant but `Unknown`, beside a match with no `_` arm: a variant
    /// added to the enum does not compile here until it is listed, and each
    /// listed variant is then held to `KNOWN_WIRE_VALUES` and `From<String>`.
    #[test]
    fn every_variant_is_in_the_known_set_and_parses_back() {
        use PolicyCategory::*;
        let all = [
            SecuritySqli,
            SecurityAdmin,
            PiiGlobal,
            PiiUs,
            PiiEu,
            PiiIndia,
            PiiSingapore,
            PiiIndonesia,
            CodeSecrets,
            CodeUnsafe,
            CodeCompliance,
            SensitiveData,
            MediaSafety,
            MediaBiometric,
            MediaDocument,
            MediaPii,
            DynamicRisk,
            DynamicCompliance,
            DynamicSecurity,
            DynamicCost,
            DynamicAccess,
            SecurityDangerous,
            ComplianceEuaiact,
            DangerousQueries,
            PiiDetection,
            SqlInjection,
        ];
        for c in &all {
            match c {
                SecuritySqli | SecurityAdmin | PiiGlobal | PiiUs | PiiEu | PiiIndia
                | PiiSingapore | PiiIndonesia | CodeSecrets | CodeUnsafe | CodeCompliance
                | SensitiveData | MediaSafety | MediaBiometric | MediaDocument | MediaPii
                | DynamicRisk | DynamicCompliance | DynamicSecurity | DynamicCost
                | DynamicAccess | SecurityDangerous | ComplianceEuaiact | DangerousQueries
                | PiiDetection | SqlInjection => {}
                Unknown(_) => unreachable!("the list names known variants only"),
            }
        }
        assert_eq!(all.len(), PolicyCategory::KNOWN_WIRE_VALUES.len());
        for c in all {
            let wire = c.as_str().to_string();
            assert!(
                PolicyCategory::KNOWN_WIRE_VALUES.contains(&wire.as_str()),
                "{wire}"
            );
            assert_eq!(PolicyCategory::from(wire.clone()), c, "{wire}");
        }
    }

    /// The fixture names where it came from, so a stale one is visible: the
    /// source path, a full platform commit, the source's sha256, and a sorted,
    /// de-duplicated category list (getaxonflow/axonflow-enterprise#4224).
    ///
    /// It is kept byte-identical to the other SDKs' copies, so its `_comment`
    /// names the Python test that reads it there; here, this test and
    /// `every_category_the_shipped_posture_uses_is_known` read it.
    #[test]
    fn the_posture_fixture_names_its_source() {
        let posture = posture();
        assert_eq!(
            posture["source"],
            "platform/decision/pdp/shipped_posture.json"
        );
        let commit = posture["platform_commit"].as_str().expect("commit");
        assert!(
            commit.len() == 40 && commit.bytes().all(|b| b.is_ascii_hexdigit()),
            "{commit}"
        );
        let sha = posture["source_sha256"].as_str().expect("sha256");
        assert!(
            sha.len() == 64 && sha.bytes().all(|b| b.is_ascii_hexdigit()),
            "{sha}"
        );
        let categories: Vec<&str> = posture["categories"]
            .as_array()
            .expect("categories")
            .iter()
            .map(|c| c.as_str().expect("a category string"))
            .collect();
        let mut canonical = categories.clone();
        canonical.sort_unstable();
        canonical.dedup();
        assert_eq!(categories, canonical);
    }
}
