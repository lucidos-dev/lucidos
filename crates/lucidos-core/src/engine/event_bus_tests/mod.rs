// Tests for [`super::EventBus`]. Split from the former single
// `event_bus_tests.rs` into concern modules (see the `mod` declarations
// below). Shared setup helpers live here so every concern module can reach
// them via `use super::*`.

use super::*;
pub(crate) use crate::engine::thread_events::{
    ActorMode, AnswerKind, EventChannel, EventMeta, MessageOrigin, SessionEndReason, ThreadEvent,
    WakeMode,
};
pub(crate) use crate::engine::thread_lifecycle::CodingAgentChangeState;
pub(crate) use crate::test_support::{
    read_change_state, read_change_state_kind, seed_change_state, setup_test_db, start_cc_session,
    teardown_test_db, wait_until_blocked_on_locks,
};

include!("lower_fixtures.rs");
include!("fixtures.rs");
use crate::engine::event_bus_test_support::*;

mod active_children_count;
mod ancestor_rebroadcast;
mod atomic_emit;
mod awaited_children_count;
mod blocking_attention_counts;
mod branch_work_and_actor;
mod card_slot;
mod child_report_taken;
mod committed_horizon;
mod crash_cut_child;
mod enabled_mcp_servers;
mod fan_out_callback;
mod filed_report;
mod has_response;
mod idle_reconcile_counts;
mod initiator_actor;
mod parked_question_delivery;
mod parked_thread_archive;
mod per_thread_commit_order;
mod pinned_is_never_archived;
mod proposal_follows_changes;
mod read_request;
mod redirect_status;
mod repo_names_projection;
mod review_presence_status;
mod section_review;
mod serialization_persistence;
mod serialization_sse;
mod session_lifecycle;
mod side_question_projection;
mod stopped_child;
mod trigger_provenance;
mod voice_session_projection;
mod waiting_child;
mod wake_decision;
mod wake_mode;
