//! Main check state and repair vocabulary carried by system events.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The durable reason a main repair started.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MainRepairStartedBy {
    Auto,
    FixNow,
}

/// Why a red main check closed, on `MainCheckRedClosed`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RedCloseReason {
    /// A change from its fix thread landed.
    FixLanded,
    /// The maintainer closed it.
    Closed,
    /// A newer main check passed.
    PassedOnNewerMain,
}

/// The *main-check setting* (ADR 0694): which workspace checks `main`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MainCheckSetting {
    /// This workspace checks every new `main` and repairs a red one.
    Here,
    /// Another workspace checks `main`, so nothing starts here.
    AnotherHost,
    /// No main check covers this workspace.
    Off,
}

/// The stored spellings of [`MainCheckSetting`], for the preference catalog.
pub const SETTING_VALUES: [&str; 3] = ["here", "another-host", "off"];

impl MainCheckSetting {
    /// The setting a stored value names. An unset or unknown value defaults to
    /// `here` where this checkout can run the check, else `off`.
    pub fn resolve(stored: Option<&str>, available: bool) -> Self {
        match stored {
            Some("here") => Self::Here,
            Some("another-host") => Self::AnotherHost,
            Some("off") => Self::Off,
            _ if available => Self::Here,
            _ => Self::Off,
        }
    }

    /// Why no check starts here under this setting, or `None` under `here`.
    pub fn idle_reason(self) -> Option<&'static str> {
        match self {
            Self::Here => None,
            Self::AnotherHost => Some("another host checks main (main-check setting)"),
            Self::Off => Some("the main-check setting is off"),
        }
    }
}

/// One failed test, as the check script names it: the suite or browser phase it
/// ran in, its name, and the head of its error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FailedTest {
    pub suite: String,
    pub name: String,
    #[serde(default)]
    pub detail: String,
}

/// Where the main check stands.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MainCheckState {
    /// Nothing runs, no red is open, and the last check did not pass.
    #[default]
    Idle,
    Running,
    /// The last check passed, and nothing has landed since.
    Passed,
    /// The last check failed, and its fix thread owns it.
    Red,
    /// This checkout cannot run the check. `summary` says why.
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

/// The repair's admission state. A waiting repair has no thread yet.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MainRepairSnapshot {
    pub state: RepairState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fix_thread_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_by: Option<MainRepairStartedBy>,
    pub waiting_on: Vec<String>,
}

/// The main check right now. Fields a state has nothing for are absent.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MainCheckSnapshot {
    pub state: MainCheckState,
    /// The main-check setting the driver runs under. Absent before restore.
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
    /// The open flaky fix thread, whatever state the check is in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flaky_fix_thread_id: Option<Uuid>,
    /// What failed, for a red check; why the check cannot run, when
    /// unavailable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}
