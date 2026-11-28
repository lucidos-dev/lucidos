use std::fmt;

use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderErrorKind {
    Internal,
    Overloaded,
    RateLimited,
    Timeout,
    RequestTooLarge,
    InvalidRequest,
    Authentication,
    Permission,
    NotFound,
    Unknown(String),
}

impl ProviderErrorKind {
    fn from_code(code: &str) -> Option<Self> {
        Some(match code {
            "api_error" | "server_error" | "internal_error" | "INTERNAL" => Self::Internal,
            "overloaded_error" | "overloaded" | "UNAVAILABLE" | "service_unavailable" => {
                Self::Overloaded
            }
            "rate_limit_error" | "rate_limit_exceeded" | "RESOURCE_EXHAUSTED" => Self::RateLimited,
            "timeout_error" | "DEADLINE_EXCEEDED" => Self::Timeout,
            "request_too_large" => Self::RequestTooLarge,
            "invalid_request_error" | "INVALID_ARGUMENT" | "FAILED_PRECONDITION" => {
                Self::InvalidRequest
            }
            "authentication_error" | "invalid_api_key" | "UNAUTHENTICATED" => Self::Authentication,
            "permission_error" | "PERMISSION_DENIED" => Self::Permission,
            "not_found_error" | "model_not_found" | "NOT_FOUND" => Self::NotFound,
            _ => return None,
        })
    }

    fn from_status(status: u16) -> Option<Self> {
        Some(match status {
            400 => Self::InvalidRequest,
            401 => Self::Authentication,
            403 => Self::Permission,
            404 => Self::NotFound,
            408 | 504 => Self::Timeout,
            413 => Self::RequestTooLarge,
            429 => Self::RateLimited,
            502 | 503 | 529 => Self::Overloaded,
            500..=599 => Self::Internal,
            _ => return None,
        })
    }

    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::Internal | Self::Overloaded | Self::RateLimited | Self::Timeout
        )
    }

    /// INVARIANT: One plain sentence on what the failure means and what to do about it.
    /// The retryable rows keep the phrases `is_transient_error` matches, so a
    /// rendered failure still reads as transient to the notification dedup.
    fn meaning(&self, provider: &str) -> String {
        match self {
            Self::Internal => format!(
                "{provider} had an internal error and is temporarily unavailable. \
                 Your request did not cause it."
            ),
            Self::Overloaded => format!("{provider} is overloaded right now."),
            Self::RateLimited => {
                format!("{provider} says this account hit its rate limit.")
            }
            Self::Timeout => format!("The request to {provider} timed out."),
            Self::RequestTooLarge => format!(
                "The request is larger than {provider} accepts. \
                 Start a new thread or attach less."
            ),
            Self::InvalidRequest => {
                format!("{provider} refused the request as invalid. Retrying will not help.")
            }
            Self::Authentication => format!(
                "{provider} did not accept the credential. \
                 Check its credential in Settings."
            ),
            Self::Permission => {
                format!("The credential has no permission for this request on {provider}.")
            }
            Self::NotFound => {
                format!("{provider} does not know the model or resource the request named.")
            }
            Self::Unknown(_) => format!(
                "Lucidos does not recognise this {provider} error. \
                 The text above is what {provider} sent."
            ),
        }
    }
}

/// INVARIANT: A model provider's failure: its own words, and what they mean.
///
/// Built through [`ProviderError::from_http`] or
/// [`ProviderError::from_stream_error`], so every provider reads its failures
/// the same way. The retry loops downcast to it and branch on
/// [`ProviderErrorKind::is_retryable`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderError {
    pub provider: String,
    pub kind: ProviderErrorKind,
    pub code: Option<String>,
    pub message: String,
    pub status: Option<u16>,
    pub attempts: u32,
}

impl ProviderError {
    pub fn from_http(provider: &str, status: u16, body: &str) -> Self {
        let mut error = match serde_json::from_str::<Value>(body) {
            Ok(value) => match error_object(&value) {
                Some(object) => Self::from_error_value(provider, object),
                None => Self::unparsed(provider, body),
            },
            Err(_) => Self::unparsed(provider, body),
        };
        error.status = Some(status);
        if matches!(error.kind, ProviderErrorKind::Unknown(_)) {
            if let Some(kind) = ProviderErrorKind::from_status(status) {
                error.kind = kind;
            } else if error.code.is_none() {
                error.kind = ProviderErrorKind::Unknown(format!("HTTP {status}"));
            }
        }
        error
    }

    pub fn tools_unsupported(&self) -> bool {
        let message = self.message.to_ascii_lowercase();
        if matches!(self.status, Some(400 | 500))
            && matches!(
                self.kind,
                ProviderErrorKind::InvalidRequest | ProviderErrorKind::Internal
            )
            && message == "tools param requires --jinja flag"
        {
            return true;
        }
        if !matches!(self.status, Some(400 | 404))
            || !matches!(
                self.kind,
                ProviderErrorKind::InvalidRequest | ProviderErrorKind::NotFound
            )
        {
            return false;
        }
        [
            "not support tool",
            "tools not supported",
            "tools are not supported",
            "tools is not supported",
            "tool_choice is not supported",
            "tool_choice not supported",
            "unsupported param: tools",
            "unsupported parameter: tools",
            "unsupported tools",
            "unsupported tool_choice",
        ]
        .iter()
        .any(|phrase| message.contains(phrase))
            || (message.contains("no endpoints found")
                && (message.contains("support tool")
                    || (message.contains("support") && message.contains("tool_choice"))))
    }

    pub fn from_stream_error(provider: &str, error: &Value) -> Self {
        let mut parsed = Self::from_error_value(provider, error);
        if matches!(parsed.kind, ProviderErrorKind::Unknown(_)) {
            if let Some(kind) = parsed.status.and_then(ProviderErrorKind::from_status) {
                parsed.kind = kind;
            }
        }
        parsed
    }

    pub fn with_attempts(mut self, attempts: u32) -> Self {
        self.attempts = attempts;
        self
    }

    fn unparsed(provider: &str, body: &str) -> Self {
        let message = body.trim();
        Self {
            provider: provider.to_string(),
            kind: ProviderErrorKind::Unknown(String::new()),
            code: None,
            message: if message.is_empty() {
                "empty response body".to_string()
            } else {
                message.to_string()
            },
            status: None,
            attempts: 0,
        }
    }

    fn from_error_value(provider: &str, error: &Value) -> Self {
        let text = |key: &str| {
            error
                .get(key)
                .and_then(Value::as_str)
                .filter(|t| !t.is_empty())
        };
        let numeric_code = error.get("code").and_then(Value::as_u64);
        let status = numeric_code.and_then(|n| u16::try_from(n).ok());
        let code = [text("code"), text("type"), text("status")]
            .into_iter()
            .flatten()
            .next()
            .map(str::to_string)
            .or_else(|| numeric_code.map(|n| n.to_string()));
        let kind = [text("type"), text("code"), text("status")]
            .into_iter()
            .flatten()
            .find_map(ProviderErrorKind::from_code)
            .unwrap_or_else(|| ProviderErrorKind::Unknown(code.clone().unwrap_or_default()));
        let message = text("message")
            .map(str::trim)
            .filter(|m| !m.is_empty())
            .unwrap_or("no message given");
        Self {
            provider: provider.to_string(),
            kind,
            code,
            message: message.to_string(),
            status,
            attempts: 0,
        }
    }
}

fn error_object(value: &Value) -> Option<&Value> {
    let value = match value {
        Value::Array(items) => items.first()?,
        other => other,
    };
    value.get("error").filter(|e| e.is_object())
}

impl fmt::Display for ProviderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} error", self.provider)?;
        if let Some(status) = self.status {
            write!(f, " ({status})")?;
        }
        let status_text = self.status.map(|s| s.to_string());
        if let Some(code) = self
            .code
            .as_ref()
            .filter(|c| Some(*c) != status_text.as_ref())
        {
            write!(f, " [{code}]")?;
        }
        write!(
            f,
            ": {}. {}",
            self.message.trim_end_matches('.'),
            self.kind.meaning(&self.provider)
        )?;
        if self.attempts > 1 {
            write!(f, " (after {} attempts)", self.attempts)?;
        }
        Ok(())
    }
}

impl std::error::Error for ProviderError {}

#[cfg(test)]
#[path = "provider_error_tests.rs"]
mod tests;
