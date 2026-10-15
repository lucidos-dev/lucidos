mod apply_now;
pub(crate) mod background_task;
pub(crate) mod coding_agent_kind;
mod external_edits;
pub(crate) mod external_watchdog;
mod input_ledger;
mod io_helpers;
pub(crate) mod lifecycle;
mod node_modules_setup;
mod parsing;
mod prompts;
mod reconstruct;
pub(crate) mod resume;
mod run_session;
mod runtime_helpers;
pub(crate) mod side_question;
mod spawn;
pub(crate) mod spawn_claim;
pub(crate) mod spawn_dispatcher;
pub(crate) mod start_failure;
mod text_buffer;
mod tool_output;
mod turn_gap;
pub(crate) mod withdraw;

pub(crate) use coding_agent_kind::{
    classify_resolved_folder, err_no_lucidos_source, resolve_folder_input, CodingAgentKind,
    FolderClassification,
};

#[cfg(test)]
pub(crate) use apply_now::{decide_in_place_merge_claim, release_change_claim, InPlaceMergeClaim};
pub(crate) use apply_now::{
    end_interrupted_applies, probe_merge_conflicts, send_awaited_prompt, ChangeClaimGuard,
    InPlaceMergeStart,
};
pub(crate) use external_edits::git_head_sha as external_edits_for_recovery_head_sha;
// Engine construction passes the hung-tool ceiling to the external watchdog;
// `lifecycle` is private, so re-export it here.
pub(crate) use lifecycle::WATCHDOG_HUNG_TOOL_CEILING_MS;
pub(crate) use parsing::{
    option_embed_problem, parse_ask_user_question_inputs, question_text, ParsedQuestion,
};
pub(crate) use prompts::{
    build_lane_catch_up_prompt, build_lane_thread_prompt, build_merge_prompt, LaneMemberBrief,
};
pub(crate) use reconstruct::prepend_reconstruction;
pub(crate) use resume::{
    change_description_fallback, latest_originating_event_id, lookup_latest_cc_session_id,
    lookup_pinned_cc_config_dir, CC_ORIGINATING_EVENT_TYPES, CHAT_ORIGINATING_EVENT_TYPES,
};
pub(crate) use run_session::{link_node_modules, provision_generated_files};

// Test-only re-exports — non-test callers reach these via super::* inside agent_session.
#[cfg(test)]
pub(crate) use resume::CC_TURN_CLOSER_EVENTS;

// Phase 7 regression: cancel = turn boundary, not terminal event.
#[cfg(test)]
#[path = "cancel_lifecycle_tests.rs"]
mod cancel_lifecycle_tests;
