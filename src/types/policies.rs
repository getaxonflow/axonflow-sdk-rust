use serde::{Deserialize, Serialize};

/// Policy categories for organization and filtering.
///
/// Cross-SDK parity:
///   Go:     axonflow-sdk-go/policies.go (PolicyCategory)
///   Python: axonflow-sdk-python/axonflow/policies.py (PolicyCategory)
///   TS:     axonflow-sdk-typescript/src/types/policies.ts (PolicyCategory)
///   Java:   axonflow-sdk-java PolicyTypes.PolicyCategory
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub enum PolicyCategory {
    #[serde(rename = "security-sqli")]
    SecuritySqli,
    #[serde(rename = "security-admin")]
    SecurityAdmin,
    #[serde(rename = "pii-global")]
    PiiGlobal,
    #[serde(rename = "pii-us")]
    PiiUs,
    #[serde(rename = "pii-eu")]
    PiiEu,
    #[serde(rename = "pii-india")]
    PiiIndia,
    #[serde(rename = "pii-singapore")]
    PiiSingapore,
    #[serde(rename = "pii-indonesia")]
    PiiIndonesia,
    #[serde(rename = "code-secrets")]
    CodeSecrets,
    #[serde(rename = "code-unsafe")]
    CodeUnsafe,
    #[serde(rename = "code-compliance")]
    CodeCompliance,
    #[serde(rename = "sensitive-data")]
    SensitiveData,
    #[serde(rename = "media-safety")]
    MediaSafety,
    #[serde(rename = "media-biometric")]
    MediaBiometric,
    #[serde(rename = "media-document")]
    MediaDocument,
    #[serde(rename = "media-pii")]
    MediaPii,
    #[serde(rename = "dynamic-risk")]
    DynamicRisk,
    #[serde(rename = "dynamic-compliance")]
    DynamicCompliance,
    #[serde(rename = "dynamic-security")]
    DynamicSecurity,
    #[serde(rename = "dynamic-cost")]
    DynamicCost,
    #[serde(rename = "dynamic-access")]
    DynamicAccess,
}

#[cfg(test)]
mod tests {
    use super::*;

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
        ];
        for (variant, expected) in cases {
            let json = serde_json::to_string(&variant).unwrap();
            assert_eq!(json, format!(r#""{}""#, expected), "variant {:?}", variant);
        }
    }
}
