//! Agent-session recovery, split by phase.
//!
//! - [`helpers`] — recovery reason constants + free helper functions.
//! - [`recovery`] — the `impl LucidosEngine` block: stale-waiting-session
//!   settlement and orphaned-worktree recovery.
//! - [`has_diff`] — resume-dispatch / startup reconciliation free functions.
//!
//! Free items are re-exported here so existing `agent_recovery::X` callers (and
//! the `super::X` references in the test sibling modules) keep resolving.

mod has_diff;
mod helpers;
mod recovery;

pub use has_diff::*;
pub use helpers::*;

#[cfg(test)]
#[path = "../agent_recovery_tests/scenarios.rs"]
mod recovery_scenarios_tests;

#[cfg(test)]
#[path = "../agent_recovery_tests/startup_sweep.rs"]
mod startup_sweep_tests;

#[cfg(test)]
#[path = "../agent_recovery_tests/end_stale.rs"]
mod end_stale_tests;

#[cfg(test)]
#[path = "../agent_recovery_integration_tests/continuation.rs"]
mod integration_continuation_tests;

#[cfg(test)]
#[path = "../agent_recovery_integration_tests/recovery.rs"]
mod integration_recovery_tests;

#[cfg(test)]
#[path = "../agent_recovery_integration_tests/startup.rs"]
mod integration_startup_tests;
