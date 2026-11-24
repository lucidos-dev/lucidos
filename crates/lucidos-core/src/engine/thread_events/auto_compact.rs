//! Claude Code auto-compact trial vocabulary.

use serde::{Deserialize, Serialize};

/// The window the trial's treatment arm runs with, in tokens.
pub const TRIAL_WINDOW_TOKENS: u64 = 400_000;

/// The values of `coding_agent_auto_compact_trial`, as the catalog lists them.
pub const TRIAL_MODES: &[&str] = &[
    TrialMode::Off.as_str(),
    TrialMode::Ab.as_str(),
    TrialMode::On.as_str(),
];

/// The `coding_agent_auto_compact_trial` preference, parsed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrialMode {
    Off,
    /// Half of all thread families get the window, by [`arm_for_family`].
    Ab,
    /// Every family gets the window.
    On,
}

impl TrialMode {
    pub const ALL: [Self; 3] = [Self::Off, Self::Ab, Self::On];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Ab => "ab",
            Self::On => "on",
        }
    }

    /// A value outside [`TRIAL_MODES`] reads as `off`, the catalog default.
    pub fn parse(value: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|mode| mode.as_str() == value)
            .unwrap_or(Self::Off)
    }
}

/// Which arm of the trial a session ran in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutoCompactArm {
    /// The engine set no window.
    Control,
    /// The engine set [`TRIAL_WINDOW_TOKENS`].
    Window,
    /// A workspace env var set the window, so the engine left it alone.
    Override,
}
