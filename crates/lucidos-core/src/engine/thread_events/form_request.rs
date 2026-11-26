use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FormRequestOutcome {
    Completed,
    Canceled,
    Superseded,
    Expired,
    /// INVARIANT: A value a newer engine wrote. Never emit.
    #[serde(other)]
    Unknown,
}
