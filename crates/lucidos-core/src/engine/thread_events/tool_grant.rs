#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AllowScope {
    Narrow,
    Broad,
    Session,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GrantOffer {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deny: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub broad: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub narrow: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub withheld: Option<OfferWithheld>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OfferWithheld {
    SecretRead,
    StandingDenyUnavailable,
    StandingDenyUnavailableSecretRead,
    UnprovenWorkspaceDestruction,
}

pub const UNPROVEN_WORKSPACE_DESTRUCTION_NOTE: &str = "The command guard can’t rule out that this deletes a Lucidos workspace or its database. Review it before allowing it once.";

impl OfferWithheld {
    pub fn standing_deny_unavailable(self) -> bool {
        matches!(
            self,
            Self::StandingDenyUnavailable | Self::StandingDenyUnavailableSecretRead
        )
    }
}

impl GrantOffer {
    pub fn withheld(reason: OfferWithheld) -> Self {
        Self {
            withheld: Some(reason),
            ..Self::default()
        }
    }

    pub fn pattern(&self, scope: AllowScope) -> Option<&str> {
        match scope {
            AllowScope::Broad => self.broad.as_deref(),
            AllowScope::Narrow => self.narrow.as_deref(),
            AllowScope::Session => self.session.as_deref(),
        }
    }
}
