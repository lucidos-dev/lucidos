#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ClaudeGatewaySignOutReason {
    SignedOut,
    UrlChanged,
    Rejected,
}

impl ClaudeGatewaySignOutReason {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::SignedOut => "signed-out",
            Self::UrlChanged => "url-changed",
            Self::Rejected => "rejected",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ChatGptPlanSignOutReason {
    SignedOut,
    Rejected,
    Forgotten,
}

impl ChatGptPlanSignOutReason {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::SignedOut => "signed-out",
            Self::Rejected => "rejected",
            Self::Forgotten => "forgotten",
        }
    }
}
