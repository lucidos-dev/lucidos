use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MainRepairStartedBy {
    Auto,
    FixNow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RedCloseReason {
    FixLanded,
    Closed,
    PassedOnNewerMain,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MainCheckSetting {
    Here,
    AnotherHost,
    Off,
}

pub const SETTING_VALUES: [&str; 3] = ["here", "another-host", "off"];

impl MainCheckSetting {
    pub fn resolve(stored: Option<&str>, available: bool) -> Self {
        match stored {
            Some("here") => Self::Here,
            Some("another-host") => Self::AnotherHost,
            Some("off") => Self::Off,
            _ if available => Self::Here,
            _ => Self::Off,
        }
    }

    pub fn idle_reason(self) -> Option<&'static str> {
        match self {
            Self::Here => None,
            Self::AnotherHost => Some("another host checks main (main-check setting)"),
            Self::Off => Some("the main-check setting is off"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FailedTest {
    pub suite: String,
    pub name: String,
    #[serde(default)]
    pub detail: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MainCheckState {
    #[default]
    Idle,
    Running,
    Passed,
    Red,
    Unavailable,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RepairState {
    #[default]
    None,
    Waiting,
    Running,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MainRepairSnapshot {
    pub state: RepairState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fix_thread_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_by: Option<MainRepairStartedBy>,
    pub waiting_on: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MainCheckSnapshot {
    pub state: MainCheckState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setting: Option<MainCheckSetting>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repair: Option<MainRepairSnapshot>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failures: Vec<FailedTest>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub flaky: Vec<FailedTest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_sha: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_sha: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fix_thread_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flaky_fix_thread_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}
