//! Trigger provenance shared by scheduling and event emission.

tokio::task_local! {
    /// Trigger UUID of the currently executing trigger. Used by the
    /// scheduling-tool guard to identify the running trigger.
    pub static ACTIVE_TRIGGER_ID: String;
    /// Current recursion depth for event-triggered chains.
    /// Set by `fire_matching_triggers` before spawning; read by `emit_domain_event`
    /// to propagate depth in the `DomainEvent` broadcast.
    pub static EVENT_TRIGGER_DEPTH: u32;
    /// Thread ID of the event that fired the currently executing trigger
    /// (set only when the trigger was matched on a `BusEvent::Thread`).
    /// Read by `execute_send_notification` so a push tap can deep-link back
    /// to the originating thread instead of the trigger's own LLM thread.
    pub static ORIGIN_THREAD_ID: uuid::Uuid;
}

/// How deep in an event-trigger chain the current task is running.
///
/// Zero outside any trigger fire, which is what an ordinary user turn, an HTTP
/// handler and a cron fire all are. Inside a fire it is the depth the queue
/// executor scoped. An event this task emits is therefore one link further
/// along the chain that produced it, and the scheduler's cap can see
/// that.
///
/// One reader, rather than one per site. A caller can only get this wrong one
/// way: by reading it from a spawned task, where the scope does not reach and
/// the answer silently becomes zero.
pub fn current_event_trigger_depth() -> u32 {
    EVENT_TRIGGER_DEPTH.try_with(|d| *d).unwrap_or(0)
}

/// Chain depth per thread, for background work whose events are emitted from
/// tasks no scope can reach.
///
/// The task-local above covers work that stays on one await chain. A
/// coding-agent session does not: its `CodingAgentIdled` comes from a spawn
/// tree several levels deep, and after a restart from `agent_recovery`. The one
/// identity every one of those emits holds is the thread id, so that is the
/// key.
///
/// **Keyed by thread, but owned per queue entry.** Two entries can name one
/// thread, and a plain `thread -> depth` map let the first to finish clear a
/// binding the other still needed.
///
/// A cache, never a source of truth. What survives a restart is the depth on
/// the persisted request. A row is dropped when its own queue entry completes,
/// so the depth belongs to the unit of background work rather than to the
/// thread. Nothing registers on the boot handoff path, where the entry
/// completes with no in-memory slot. See
/// `docs/adr/0138-event-trigger-depth-reaches-spawned-work.md`.
static CHAIN_DEPTHS: std::sync::LazyLock<std::sync::RwLock<ChainDepthOwners>> =
    std::sync::LazyLock::new(Default::default);

/// Thread id to the live `(queue entry, depth)` bindings on it.
type ChainDepthOwners = std::collections::HashMap<uuid::Uuid, Vec<(uuid::Uuid, u32)>>;

/// Bind `thread_id`'s events to `depth`, on behalf of queue entry `owner`.
///
/// Depth 0 registers nothing: it is the answer an unregistered thread already
/// gives, and a row per ordinary chat thread would grow without bound.
/// Re-registering the same owner replaces its row rather than stacking one.
pub fn register_chain_depth(thread_id: uuid::Uuid, owner: uuid::Uuid, depth: u32) {
    if depth == 0 {
        return;
    }
    if let Ok(mut map) = CHAIN_DEPTHS.write() {
        let owners = map.entry(thread_id).or_default();
        match owners.iter_mut().find(|(id, _)| *id == owner) {
            Some(row) => row.1 = depth,
            None => owners.push((owner, depth)),
        }
    }
}

/// Drop `owner`'s binding when the work that owned it finishes. Another entry's
/// binding on the same thread is left alone.
pub fn forget_chain_depth(thread_id: uuid::Uuid, owner: uuid::Uuid) {
    if let Ok(mut map) = CHAIN_DEPTHS.write() {
        let empty = match map.get_mut(&thread_id) {
            Some(owners) => {
                owners.retain(|(id, _)| *id != owner);
                owners.is_empty()
            }
            None => false,
        };
        if empty {
            map.remove(&thread_id);
        }
    }
}

/// The chain depth bound to `thread_id`, if it is background chain work.
///
/// The DEEPEST of the live bindings, matching how `EventBus::emit` ranks this
/// against the task-local. Under-counting re-opens the loop silently, and
/// over-counting suppresses a fire the cap notification names.
///
/// A poisoned lock answers `None`, which degrades to the ambient read. That
/// loses the cap for one emit rather than failing it.
pub fn chain_depth_for_thread(thread_id: uuid::Uuid) -> Option<u32> {
    CHAIN_DEPTHS
        .read()
        .ok()?
        .get(&thread_id)?
        .iter()
        .map(|(_, depth)| *depth)
        .max()
}

/// Which trigger's fire the current task is running, if any.
///
/// `None` outside a fire, which is what an ordinary user turn and an HTTP
/// handler both are. Inside one it is the id the queue executor scoped.
/// `EventBus::emit` stamps it onto every frame the fire emits, so the scheduler
/// can drop that trigger from the matches.
///
/// Same one-reader rule as [`current_event_trigger_depth`], and the same way to
/// get it wrong. Read it from a spawned task, where the scope does not reach,
/// and the answer silently becomes `None`. That direction is deliberate for
/// work a fire hands off (`docs/adr/0137-a-trigger-never-wakes-itself.md`). A
/// caller that must state the owner outright uses
/// [`crate::engine::event_bus::EventBus::emit_as_trigger`] instead.
pub fn current_trigger_id() -> Option<String> {
    ACTIVE_TRIGGER_ID.try_with(|id| id.clone()).ok()
}

/// True when the currently running trigger is deleting itself: the LLM
/// called `delete_trigger` with its own trigger ID. Config reconciliation
/// reads this event flag to skip nudging the thread performing the deletion.
/// Scheduler cancellation is independent: every deletion stops scheduling
/// while leaving any admitted run alone.
pub fn is_self_deleting_trigger(trigger_id: &str) -> bool {
    ACTIVE_TRIGGER_ID
        .try_with(|active| *active == trigger_id)
        .unwrap_or(false)
}

/// Namespace UUID for deriving trigger UUIDs via v5 (SHA-1).
/// The historical bytes spell "cognos-trigger-n" and form part of the hash input.
/// Keep them stable so existing trigger ids keep the same UUID.
const TRIGGER_UUID_NAMESPACE: uuid::Uuid = uuid::Uuid::from_bytes([
    0x63, 0x6f, 0x67, 0x6e, 0x6f, 0x73, 0x2d, 0x74, 0x72, 0x69, 0x67, 0x67, 0x65, 0x72, 0x2d, 0x6e,
]); // "cognos-trigger-n" — frozen for v5 namespace stability; see doc above.

/// Derive a deterministic UUID from a trigger ID string (uuid v5 / SHA-1).
pub fn trigger_id_to_uuid(trigger_id: &str) -> uuid::Uuid {
    uuid::Uuid::new_v5(&TRIGGER_UUID_NAMESPACE, trigger_id.as_bytes())
}

/// Outcome of a trigger's most recent completed firing. Surfaced on the
/// trigger row (OK / failed) so a threadless (script) trigger's health is
/// visible without opening its event stream. Wire values are `"ok"` /
/// `"failed"`; `None` on the config means "never run / legacy" (a
/// pre-status-field `TriggerExecuted` event carries no outcome).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TriggerRunStatus {
    Ok,
    Failed,
}

impl TriggerRunStatus {
    /// Map a run outcome (`execute_user_task`'s `Result`) to a status.
    pub fn from_success(success: bool) -> Self {
        if success {
            Self::Ok
        } else {
            Self::Failed
        }
    }

    /// Wire string used in the `TriggerExecuted` payload and the trigger-list API.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Failed => "failed",
        }
    }
}
