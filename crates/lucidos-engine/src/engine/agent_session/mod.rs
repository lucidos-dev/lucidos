mod apply_now;
mod cc_spawn_coalesce;
pub(crate) mod coding_agent_kind;
mod external_edits;
pub(crate) mod external_watchdog;
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
mod turn_gap;

pub(crate) use cc_spawn_coalesce::{
    combine_messages, queued_to_orphans, CcSpawnCoalescer, LeaderElection, QueuedMessage,
};
pub(crate) use coding_agent_kind::{
    classify_resolved_folder, err_no_lucidos_source, resolve_folder_input, CodingAgentKind,
    FolderClassification,
};

pub(crate) use apply_now::{probe_merge_conflicts, InPlaceMergeStart};
// Engine construction passes the hung-tool ceiling to the external watchdog;
// `lifecycle` is private, so re-export it here.
pub(crate) use external_edits::git_head_sha as external_edits_for_recovery_head_sha;
pub(crate) use lifecycle::WATCHDOG_HUNG_TOOL_CEILING_MS;
pub(crate) use parsing::{parse_ask_user_question_inputs, question_text};
pub(crate) use prompts::build_merge_prompt;
pub(crate) use reconstruct::prepend_reconstruction;
pub(crate) use resume::{
    change_description_fallback, latest_originating_event_id, lookup_latest_cc_session_id,
    lookup_pinned_cc_config_dir, CC_ORIGINATING_EVENT_TYPES, CHAT_ORIGINATING_EVENT_TYPES,
};

// Test-only re-exports — non-test callers reach these via super::* inside agent_session.
#[cfg(test)]
pub(crate) use resume::CC_TURN_CLOSER_EVENTS;
#[cfg(test)]
pub(crate) use spawn::generate_cc_branch_name;

// Phase 7 regression: cancel = turn boundary, not terminal event.
#[cfg(test)]
#[path = "cancel_lifecycle_tests.rs"]
mod cancel_lifecycle_tests;
