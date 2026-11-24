/// Event types that can be the originating event of a chat-channel turn —
/// the event whose id every event in the turn carries as `request_event_id`.
/// `ChildThreadCompleted` is included because a parent thread waking from a
/// finished child has the CTC's id stamped as `pre_emitted_origin` (see
/// `engine_impl::notify_parent_of_child_completion` → `process.rs`'s
/// `origin_id`). Used by every chat-side path that resolves "which turn was
/// being killed" — `abort_in_flight_for_restart`, `emit_stuck_thread_eviction_abort`,
/// the `chat/rerun.rs` Continue fallback, and the `recover_orphaned_threads`
/// SQL.
pub const CHAT_ORIGINATING_EVENT_TYPES: &[&str] =
    &["MessageReceived", "TriggerStarted", "ChildThreadCompleted"];

/// Event types that can originate Claude Code turns. Include injected
/// `CodingAgentUserMessageSent` for follow-ups. A parent woken by a child
/// anchors its turn to `ChildThreadCompleted` through `pre_emitted_origin`.
pub const CC_ORIGINATING_EVENT_TYPES: &[&str] = &[
    "MessageReceived",
    "CodingAgentUserMessageSent",
    "TriggerStarted",
    "ChildThreadCompleted",
];
