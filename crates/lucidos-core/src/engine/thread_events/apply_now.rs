use serde::{Deserialize, Serialize};

/// How an Apply Now ended. Carried by `ApplyNowEnded`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApplyNowOutcome {
    /// The apply ran to its end. Whether it landed is `ChangeApplied` or
    /// `ChangeApplyFailed`.
    Finished,
    /// The engine stopped while the apply ran, so it ended with no result.
    Interrupted,
    /// A value a newer engine wrote. Never emit.
    #[serde(other)]
    Unknown,
}
