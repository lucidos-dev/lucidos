use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AffectedTrigger {
    pub trigger_id: String,
    pub trigger_name: String,
    pub paths: Vec<AffectedPath>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AffectedPath {
    pub path: String,
    pub reason: AffectedReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AffectedReason {
    NamedInRun,
    OwningApp,
    TriggerKnowhow,
}
