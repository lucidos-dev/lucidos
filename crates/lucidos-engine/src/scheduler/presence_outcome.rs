use serde::{Deserialize, Serialize};

/// What the page already did with a notification when it answered. The page
/// acts first and reports after, so the push is suppressed only on its word
/// that the notification reached the reader in the app.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PresenceOutcome {
    /// The toast is on screen, alone or folded into the overflow toast.
    Shown,
    /// The source event is on screen, so the page marked the row read.
    AutoRead,
    /// Toasts are off, so the row waits on the bell badge.
    Silenced,
    /// The shell is not in front of the reader. The OS surface is theirs.
    NotShown,
}

impl PresenceOutcome {
    /// Strongest first: the order a shared connection merges its documents'
    /// answers in. The SDK reads it from the generated `engine-constants.ts`.
    pub const BY_STRENGTH: [Self; 4] =
        [Self::Shown, Self::AutoRead, Self::Silenced, Self::NotShown];

    pub fn suppresses_push(self) -> bool {
        !matches!(self, Self::NotShown)
    }
}
