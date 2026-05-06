mod apply_now;
mod cc_spawn_coalesce;
mod commit_hook;
mod external_edits;
mod io_helpers;
mod lifecycle;
mod node_modules_setup;
mod parsing;
mod prompts;
mod reconstruct;
pub(crate) mod resume;
mod run_session;
mod runtime_helpers;
mod spawn;
pub(crate) mod spawn_dispatcher;

pub(crate) use cc_spawn_coalesce::{
    combine_messages, queued_to_orphans, CcSpawnCoalescer, LeaderElection, QueuedMessage,
};

pub(crate) use external_edits::git_head_sha as external_edits_for_recovery_head_sha;
pub(crate) use parsing::parse_ask_user_question_input;
pub(crate) use prompts::build_merge_prompt;
pub(crate) use reconstruct::prepend_reconstruction;
pub(crate) use resume::{change_description_fallback, lookup_latest_cc_session_id};

// Test-only re-exports — non-test callers reach these via super::* inside agent_session.
#[cfg(test)]
pub(crate) use resume::CC_TURN_CLOSER_EVENTS;
#[cfg(test)]
pub(crate) use spawn::generate_cc_branch_name;

// Phase 7 regression: cancel = turn boundary, not terminal event.
#[cfg(test)]
#[path = "cancel_lifecycle_tests.rs"]
mod cancel_lifecycle_tests;
