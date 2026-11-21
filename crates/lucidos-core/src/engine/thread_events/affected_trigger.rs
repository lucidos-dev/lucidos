use serde::{Deserialize, Serialize};

/// A trigger, and the removed paths it relies on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AffectedTrigger {
    pub trigger_id: String,
    pub trigger_name: String,
    pub paths: Vec<AffectedPath>,
}

/// One removed `data/`-relative path, and why the trigger relies on it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AffectedPath {
    pub path: String,
    pub reason: AffectedReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AffectedReason {
    /// The intent text or the script path names the path.
    NamedInRun,
    /// The path is knowhow under the trigger's owning app.
    OwningApp,
    /// The path is knowhow under the trigger's own `triggers/<slug>/` folder.
    TriggerKnowhow,
}
