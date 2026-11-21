pub(crate) mod apply_now;
pub(crate) mod background_task;
pub(crate) mod child_reports;
pub(crate) mod coding_agent_kind;
pub(crate) mod confinement_run;
mod continuity;
mod external_edits;
pub(crate) mod external_watchdog;
mod input_ledger;
mod io_helpers;
pub(crate) mod lifecycle;
#[cfg(test)]
mod node_modules_setup;
mod prompts;
mod reconstruct;
pub(crate) mod resume;
pub(crate) mod run_session;
pub(crate) mod runtime_helpers;
pub(crate) mod side_question;
mod spawn;
pub(crate) mod spawn_claim;
pub(crate) mod spawn_dispatcher;
pub(crate) mod spawn_gate;
#[cfg(test)]
mod start_failure;
pub(crate) mod stranded_follow_ups;
mod text_buffer;
mod tool_output;
mod turn_gap;
pub(crate) mod withdraw;

pub(crate) use coding_agent_kind::{
    classify_resolved_folder, err_no_lucidos_source, resolve_folder_input, FolderClassification,
};

#[cfg(test)]
pub(crate) use crate::engine::in_place_merge::{decide_in_place_merge_claim, InPlaceMergeClaim};
pub(crate) use apply_now::{probe_merge_conflicts, wait_for_prompt_turn, ChangeClaimGuard};
pub(crate) use external_edits::git_head_sha as external_edits_for_recovery_head_sha;
pub(crate) use external_edits::worktree_serves_branch;
pub(crate) use reconstruct::prepend_reconstruction;
pub(crate) use resume::{blocking_policy, lookup_pinned_cc_config_dir};

// Test-only re-exports — non-test callers reach these via super::* inside agent_session.
#[cfg(test)]
pub(crate) use resume::CC_TURN_CLOSER_EVENTS;

// Phase 7 regression: cancel = turn boundary, not terminal event.
#[cfg(test)]
#[path = "cancel_lifecycle_tests.rs"]
mod cancel_lifecycle_tests;
