use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PresenceOutcome {
    Shown,
    AutoRead,
    Silenced,
    NotShown,
}

impl PresenceOutcome {
    /// INVARIANT: Strongest first: the order a shared connection merges its documents'
    /// answers in. The SDK reads it from the generated `engine-constants.ts`.
    pub const BY_STRENGTH: [Self; 4] =
        [Self::Shown, Self::AutoRead, Self::Silenced, Self::NotShown];

    pub fn suppresses_push(self) -> bool {
        !matches!(self, Self::NotShown)
    }
}
