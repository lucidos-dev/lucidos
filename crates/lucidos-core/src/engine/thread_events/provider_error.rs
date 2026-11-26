use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderError {
    pub code: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProviderErrorCode {
    MisalignmentPolicyViolation,
    CyberPolicy,
    BioPolicy,
    ContextWindowExceeded,
    UsageLimitExceeded,
    RateLimitExceeded,
    ModelNotFound,
    PolicyCapacityUnavailable,
    FlexUnavailable,
    SessionBudgetExceeded,
    Other(String),
}

impl ProviderErrorCode {
    fn from_wire(code: &str) -> Self {
        match code {
            "misalignment_policy_violation" => Self::MisalignmentPolicyViolation,
            "cyber_policy" => Self::CyberPolicy,
            "bio_policy" => Self::BioPolicy,
            "context_window_exceeded" => Self::ContextWindowExceeded,
            "usage_limit_exceeded" => Self::UsageLimitExceeded,
            "rate_limit_exceeded" => Self::RateLimitExceeded,
            "model_not_found" => Self::ModelNotFound,
            "policy_capacity_unavailable" => Self::PolicyCapacityUnavailable,
            "flex_unavailable" => Self::FlexUnavailable,
            "session_budget_exceeded" => Self::SessionBudgetExceeded,
            other => Self::Other(other.to_string()),
        }
    }

    pub fn is_policy_block(&self) -> bool {
        self.policy_name().is_some()
    }

    pub fn policy_name(&self) -> Option<&'static str> {
        match self {
            Self::MisalignmentPolicyViolation => Some("potentially unintended activity"),
            Self::CyberPolicy => Some("cybersecurity policy"),
            Self::BioPolicy => Some("biosecurity policy"),
            _ => None,
        }
    }
}

impl ProviderError {
    pub fn kind(&self) -> ProviderErrorCode {
        ProviderErrorCode::from_wire(&self.code)
    }

    pub fn is_policy_block(&self) -> bool {
        self.kind().is_policy_block()
    }

    pub fn policy_name(&self) -> Option<&'static str> {
        self.kind().policy_name()
    }

    pub fn from_codex_error(error: &serde_json::Value) -> Option<Self> {
        let info = error
            .get("codexErrorInfo")
            .or_else(|| error.get("codex_error_info"))?;
        let raw = match info {
            serde_json::Value::String(s) => s.as_str(),
            serde_json::Value::Object(o) if o.len() == 1 => o.keys().next()?.as_str(),
            _ => return None,
        };
        let code = to_snake_case(raw.trim());
        if code.is_empty() {
            return None;
        }
        Some(Self {
            code,
            detail: codex_detail(error),
        })
    }
}

fn codex_detail(error: &serde_json::Value) -> Option<String> {
    let details = error
        .get("additionalDetails")
        .or_else(|| error.get("additional_details"))?;
    let text = details
        .get("detailedExplanation")
        .or_else(|| details.get("detailed_explanation"))?
        .as_str()?
        .trim();
    (!text.is_empty()).then(|| text.to_string())
}

fn to_snake_case(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 4);
    for (i, c) in name.chars().enumerate() {
        if c.is_ascii_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_app_server_camel_case_code_is_a_policy_block() {
        let error = json!({
            "message": "This request was blocked by our safety systems.",
            "codexErrorInfo": "misalignmentPolicyViolation",
        });
        let provider_error = ProviderError::from_codex_error(&error).unwrap();
        assert_eq!(provider_error.code, "misalignment_policy_violation");
        assert_eq!(
            provider_error.kind(),
            ProviderErrorCode::MisalignmentPolicyViolation
        );
        assert!(provider_error.is_policy_block());
    }

    #[test]
    fn the_rollout_snake_case_code_reads_the_same() {
        let error = json!({ "codex_error_info": "cyber_policy" });
        let provider_error = ProviderError::from_codex_error(&error).unwrap();
        assert_eq!(provider_error.kind(), ProviderErrorCode::CyberPolicy);
        assert!(provider_error.is_policy_block());
    }

    #[test]
    fn a_single_key_object_names_the_code_by_its_key() {
        let error =
            json!({ "codexErrorInfo": { "httpConnectionFailed": { "httpStatusCode": 502 } } });
        let provider_error = ProviderError::from_codex_error(&error).unwrap();
        assert_eq!(provider_error.code, "http_connection_failed");
        assert!(!provider_error.is_policy_block());
    }

    #[test]
    fn an_unknown_code_is_kept_verbatim_and_never_blocks() {
        let error = json!({ "codexErrorInfo": "brandNewFailure" });
        let provider_error = ProviderError::from_codex_error(&error).unwrap();
        assert_eq!(provider_error.code, "brand_new_failure");
        assert_eq!(
            provider_error.kind(),
            ProviderErrorCode::Other("brand_new_failure".to_string())
        );
        assert!(!provider_error.is_policy_block());
    }

    #[test]
    fn no_code_means_no_provider_error() {
        let error = json!({ "message": "This request was blocked by our safety systems." });
        assert_eq!(ProviderError::from_codex_error(&error), None);
        assert_eq!(
            ProviderError::from_codex_error(&json!({ "codexErrorInfo": null })),
            None
        );
    }

    #[test]
    fn the_detailed_explanation_is_carried_as_detail() {
        let error = json!({
            "codexErrorInfo": "misalignmentPolicyViolation",
            "additionalDetails": { "errorType": "x", "detailedExplanation": "  Why.  " },
        });
        let provider_error = ProviderError::from_codex_error(&error).unwrap();
        assert_eq!(provider_error.detail.as_deref(), Some("Why."));
    }

    #[test]
    fn only_a_policy_code_has_a_policy_name() {
        let named = |code: &str| {
            ProviderError {
                code: code.to_string(),
                detail: None,
            }
            .policy_name()
        };
        assert_eq!(
            named("misalignment_policy_violation"),
            Some("potentially unintended activity")
        );
        assert_eq!(named("cyber_policy"), Some("cybersecurity policy"));
        assert_eq!(named("bio_policy"), Some("biosecurity policy"));
        assert_eq!(named("rate_limit_exceeded"), None);
        assert_eq!(named("brand_new_failure"), None);
    }

    #[test]
    fn an_old_payload_without_detail_deserializes() {
        let provider_error: ProviderError =
            serde_json::from_value(json!({ "code": "cyber_policy" })).unwrap();
        assert_eq!(provider_error.detail, None);
        assert_eq!(
            serde_json::to_value(&provider_error).unwrap(),
            json!({ "code": "cyber_policy" })
        );
    }
}
