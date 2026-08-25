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
// `recovery`'s free predicates stay in that file next to the recovery pass
// that documents them; re-export the ones the teardown emit + its tests share
// (`engine_impl/shutdown.rs`), so both sides of the preserve/resume contract
// key on one definition.
pub(crate) use recovery::{
    preserve_question_park_at_shutdown, thread_has_unanswered_question,
    unanswered_question_exists_sql,
};
// "Does a boundary already cover this turn?", in its two strengths. The
// recovery pass asks the window-only form before emitting its own boundary; a
// coding-agent session that registered mid-teardown asks the anchored form
// before SKIPPING its own terminal (`agent_session::runtime_helpers`), because
// the cost of a wrong `true` differs: a duplicate panel versus a turn with no
// terminator at all. One place, so the two cannot drift on which turn an abort
// belongs to.
#[cfg(test)]
pub(crate) use recovery::boundary_abort_already_emitted;
pub(crate) use recovery::boundary_abort_covers_turn;
// The switch-vs-crash fingerprint, shared with the chat resume gate
// (`chat::recovery::switch_resume_candidates`) so the coding-agent and chat
// halves of the auto-resume contract cannot drift apart.
#[cfg(test)]
pub(crate) use recovery::switch_was_user_initiated;
// The running-vs-idle branch classifier, so its regression tests exercise the
// production SQL rather than a hand-copied paraphrase of it.
#[cfg(test)]
pub(crate) use recovery::BRANCH_CLASSIFICATION_SQL;
// The stale settle's two worktree actions, so the app-shape tests drive the
// production pair rather than a hand-copied replay of it. Which of the two runs
// is the whole property: a Discard removes the tree, everything else rescues it
// and leaves it to the cleanup worker (ADR 0035).
// `settle_stale_worktree` joins them because it holds the third answer: git
// could not say which tree has the branch, so neither action may run.
#[cfg(test)]
pub(crate) use recovery::{
    remove_discarded_stale_worktree, rescue_stale_worktree, settle_stale_worktree,
};
// "Does this branch still owe the user a recovery?", for the same reason: the
// scenario mirror calls it instead of re-spelling its `&&`, which is how a
// retired input sat in the mirror being asserted as correct.
#[cfg(test)]
pub(crate) use recovery::branch_awaits_recovery;
pub(crate) use recovery::{switch_abort_unsuperseded_sql, SWITCH_TEARDOWN_ABORT_SQL};
// The boot floor that withdraws a resume promise this boot could not keep, so a
// switch-interrupted thread never sits paused with no Continue button. `main.rs`
// reaches it through `LucidosEngine::settle_unresumed_switch_threads`; the free
// function is re-exported for the tests, which drive it against a seeded pool.
#[cfg(test)]
pub(crate) use recovery::settle_unresumed_switch_threads;

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
#[path = "../agent_recovery_tests/question_park.rs"]
mod question_park_tests;

#[cfg(test)]
#[path = "../agent_recovery_integration_tests/continuation.rs"]
mod integration_continuation_tests;

#[cfg(test)]
#[path = "../agent_recovery_integration_tests/recovery.rs"]
mod integration_recovery_tests;

#[cfg(test)]
#[path = "../agent_recovery_integration_tests/startup.rs"]
mod integration_startup_tests;
