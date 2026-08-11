use crate::core::Step;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// Re-exported so callers can use the bare names through `engine::*`
// (chained via `engine/mod.rs::pub use types::*`). Engine code references
// `ConversationSnapshot` etc. directly without the `core::` path.
pub use crate::core::{ConversationMessage, ConversationSnapshot, SessionMessage};

pub use crate::api::AppContext;

#[derive(Clone, Debug, Serialize)]
pub struct ProcessResult {
    /// Response text in raw markdown (frontend converts to HTML)
    pub response: String,
    pub steps: Vec<Step>,
    /// Paths to images created during this response (relative to artifacts/)
    pub images: Vec<String>,
    /// The request ID this result belongs to
    #[serde(skip)]
    pub request_id: Uuid,
    /// The thread ID this result belongs to (groups related request/response exchanges)
    #[serde(skip)]
    pub thread_id: Uuid,
    /// Whether a pending change was proposed during this request (triggers SSE broadcast)
    #[serde(skip)]
    pub proposed_change: bool,
    /// Whether the user requested auto-apply before the session ended
    #[serde(skip)]
    pub auto_apply: bool,
    /// Injections orphaned by the loop-exit / guard-drop race — caller
    /// must re-submit them as follow-up messages.
    #[serde(skip)]
    pub orphaned_injections: Vec<OrphanedInjection>,
}

/// A follow-up message that was injected into a thread after the agentic loop
/// finished but before the ThreadGuard dropped — the race window where the
/// thread appears active but nobody reads the injection channel.
pub type OrphanedInjection = super::InjectedPrompt;

/// `content` is `Option` so the modal can render section *shape* (name +
/// `char_count`) without persisting the body when the `capture_context`
/// preference is off. Old DB rows always have content and deserialize as
/// `Some(_)`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContextSection {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    pub char_count: usize,
    /// API role this section is sent under. Defaults to "user" for backward
    /// compat with persisted ContextCaptured events from before this change —
    /// every previous section was actually part of either the system prompt
    /// (System Instructions) or the user message; the system row gets
    /// migrated by name in the viewer fallback.
    #[serde(default = "default_context_role")]
    pub role: ContextRole,
    /// Inner-group label used by the viewer to nest sections within the
    /// user-message role. None for system-role sections, prior-message rows,
    /// and legacy events.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
}

/// API role bucket a `ContextSection` belongs to. Mirrors the three buckets
/// in the LLM API call: the system prompt, prior messages (verbatim resume
/// tool blocks), and this turn's user message.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextRole {
    System,
    PriorMessage,
    User,
}

fn default_context_role() -> ContextRole {
    ContextRole::User
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_section_legacy_payload_deserializes_with_default_role_and_no_group() {
        // Pre-change ContextCaptured events have no `role` or `group` fields.
        // They must still deserialize cleanly: `role` defaults to `User`, and
        // `group` stays `None`.
        let json = r#"{"name":"X","char_count":42}"#;
        let section: ContextSection =
            serde_json::from_str(json).expect("legacy payload must deserialize");
        assert_eq!(section.name, "X");
        assert_eq!(section.char_count, 42);
        assert!(section.content.is_none());
        assert_eq!(section.role, ContextRole::User);
        assert!(section.group.is_none());
    }

    /// A session whose run loop still owns `msg_rx` is live.
    #[test]
    fn session_with_running_loop_is_live() {
        let (session, _msg_rx) = AgentSession::for_test();
        assert!(session.is_live());
    }

    /// Regression (2026-07-28): the run future was dropped rather than
    /// completed, so nothing set `process_exited` — but `msg_rx` went with it.
    /// The entry must read as dead anyway, because the loop that would ever
    /// service it is gone. This is the phantom that wedged thread `293f96d5`.
    #[test]
    fn session_whose_loop_was_dropped_is_not_live() {
        let (session, msg_rx) = AgentSession::for_test();
        drop(msg_rx);

        assert!(
            !session.process_exited,
            "precondition: a dropped future never gets to set this flag"
        );
        assert!(
            !session.is_live(),
            "a session whose receiver is gone must not read as live — \
             it fooled worktree cleanup, the chat fast path, and the resume guard"
        );
    }

    /// The normal exit path still works: the loop sets `process_exited` before
    /// the map entry is removed, and that alone marks it dead.
    #[test]
    fn session_with_exited_process_is_not_live() {
        let (mut session, _msg_rx) = AgentSession::for_test();
        session.process_exited = true;
        assert!(!session.is_live());
    }

    /// `is_in_flight` is layered on `is_live`, so a phantom mid-turn session
    /// (`is_waiting == false`) must not report an in-flight response either.
    #[test]
    fn phantom_mid_turn_session_is_not_in_flight() {
        let (mut session, msg_rx) = AgentSession::for_test();
        session.is_waiting = false;
        assert!(session.is_in_flight(), "live mid-turn session is in flight");

        drop(msg_rx);
        assert!(
            !session.is_in_flight(),
            "a phantom must never report an in-flight response"
        );
    }
}

/// CC can't expose its system prompt body or tool schemas via the
/// stream-json envelope, only their token cost — the frontend uses this
/// discriminant to know whether to expect a section breakdown body.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextProducer {
    MainLlm,
    ClaudeCode,
    Codex,
}

impl ContextProducer {
    /// Producer for a `ContextCaptured` emitted from a coding-agent session.
    pub fn from_coding_agent(agent: crate::runtime::CodingAgent) -> Self {
        match agent {
            crate::runtime::CodingAgent::ClaudeCode => Self::ClaudeCode,
            crate::runtime::CodingAgent::Codex => Self::Codex,
        }
    }
}

/// `cache_*_tokens` are Anthropic-only (zero elsewhere). `output_tokens`
/// may be zero on a snapshot emitted mid-stream before the final delta.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct ApiUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cache_read_tokens: u32,
    pub cache_creation_tokens: u32,
}

/// Result of an App UI capture from the frontend
pub struct CaptureResult {
    pub screenshot: String, // base64-encoded image (JPEG from html2canvas; format sniffed downstream)
    pub dom: String,        // condensed DOM text
}

/// A user message sent to a running Claude Code session, with optional image attachments.
/// `origin_event_id` is the UUID of the already-emitted `MessageReceived` (or other
/// exchange-starter like `ChildThreadCompleted`) event — used to re-process lost
/// follow-ups without emitting a duplicate exchange boundary.
pub struct AgentUserInput {
    pub text: String,
    pub images: Option<Vec<crate::api::ChatImage>>,
    /// UUID of the exchange-starter event already emitted before routing
    /// (`MessageReceived` for user follow-ups; `ChildThreadCompleted` for
    /// child-wake follow-ups). `None` for auto-harden and other internally
    /// generated messages where no caller-side event exists.
    pub origin_event_id: Option<uuid::Uuid>,
    /// What kind of input this is. CC's `run_session` reads this to decide
    /// whether to emit `CodingAgentPromptSent` — `User` does, `WakeFromChild`
    /// doesn't (the `ChildThreadCompleted` event already on the parent's
    /// history is the exchange-starter; emitting another start event would
    /// split the response into a duplicate exchange).
    pub kind: AgentInputKind,
}

/// Discriminates a user-typed follow-up from an engine-synthesized child-wake.
/// CC's `run_session` and the chat fast-paths use this to suppress duplicate
/// exchange-starter events for wakes (`ChildThreadCompleted` is the start).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentInputKind {
    /// User-typed follow-up. Emit `CodingAgentPromptSent` as the audit trail.
    User,
    /// Engine-synthesized wake from a completed child thread. Suppress
    /// `CodingAgentPromptSent` so the response groups under
    /// `ChildThreadCompleted` (the real exchange-starter).
    WakeFromChild,
}

/// Cached CC slash commands (builtin + skill).
/// Serializable so it can be cached to `.lucidos/cc-commands.json` and survive engine restarts.
/// Model/effort are NOT cached here — they are per-thread via CodingAgentSettingsChanged events.
#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CcCommandsInfo {
    pub builtin_commands: Vec<String>,
    pub skill_commands: Vec<String>,
}

/// Response from cc_categorized_commands / cc_commands_for_repo.
/// Commands come from the repo-level cache; model/effort come from per-thread events.
pub struct CcCommandsResult {
    pub info: CcCommandsInfo,
    pub has_active_session: bool,
    pub current_model: Option<String>,
    pub current_reasoning_effort: Option<String>,
}

/// Why a stop signal fired. `Apply` / `Discard` / `Archive` flow through
/// `stop_agent` (the run_session loop's stop arm reads this to drive their
/// side effects; each has its own lifecycle terminator). `UserStop` does NOT
/// flow here — a real Cancel routes through `interrupt_agent` (CC's native
/// interrupt / Esc) so the session stays resumable (see `api::claude_code`).
/// The variant is retained because `interrupt_agent`'s no-live-session
/// fallback and the engine-shutdown sweep still surface a `UserStop`-shaped
/// cancel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    /// User clicked the Cancel/Stop button on an in-flight CC turn — Cancel =
    /// Esc. Routed through `interrupt_agent`, NOT `stop_agent`: the turn is
    /// interrupted but the session stays resumable. The interrupted `Result`
    /// classifies as `ResponseCanceled(UserStop)` + `CodingAgentIdled`; a
    /// no-op if CC had already gone idle (race window — previous turn done).
    UserStop,
    /// User clicked Apply Now — the resulting change auto-applies after
    /// cleanup. Post-loop reads this to set `ProcessResult.auto_apply=true`.
    /// `ChangeApplied` is the terminator; no `ResponseCanceled`.
    Apply,
    /// User clicked Discard — branch is deleted instead of proposed.
    /// `ChangeDiscarded` is the terminator; no `ResponseCanceled`.
    Discard,
    /// User clicked Archive — `ThreadArchived` is the terminator; no
    /// `ResponseCanceled`.
    Archive,
}

/// State for a single active coding-agent session.
///
/// **Single agent per thread — deliberate.** The owning HashMap is keyed by
/// `Uuid` (thread_id) alone, so a thread hosts at most one live agent session.
/// This holds with both backends wired in (Claude Code + Codex): a thread's
/// backend is locked at its first session (`thread_summaries.coding_agent`;
/// `validate_thread_continuity` rejects a mismatched follow-up with 409), and
/// the session records its backend in [`AgentSession::coding_agent`].
pub struct AgentSession {
    pub msg_tx: tokio::sync::mpsc::UnboundedSender<AgentUserInput>,
    pub is_waiting: bool,
    pub has_changes: bool,
    pub requires_restart: bool,
    /// Reason the most recent `stop_agent` call fired, or `None` when the
    /// session has never been stopped (or the stop signal came from the
    /// engine-shutdown direct-notify path, which sets `shutting_down` instead).
    /// Set by `stop_agent` BEFORE `stop.notify_one()`; read by the run_session
    /// stop arm to decide whether to suppress `ResponseCanceled`, and by the
    /// post-loop cleanup to drive `Apply` / `Discard` side effects. Stays
    /// mutually exclusive by construction — three `bool` fields here used to
    /// drift apart and the `auto_apply || discard || archiving` shape was a
    /// repeated source of bugs.
    pub pending_stop: Option<StopReason>,
    /// Device that clicked **Cancel** on a live (non-waiting) session, stamped
    /// by `interrupt_agent` before it fires the `interrupt` notify. The
    /// run_session interrupt arm drains it (`take_session_cancel_actor`) and
    /// merges it into the emitted `ResponseCanceled.actor` so the Initiator
    /// popover shows which device cancelled — the live-interrupt analog of the
    /// chat path's `ThreadHandle.cancel_actor` slot. `None` for engine-internal
    /// interrupts (those pre-emit their own boundary events with an explicit
    /// actor upstream). Drained on read so a stale device can't carry into the
    /// next turn on a resumed session.
    pub cancel_actor: Option<crate::engine::thread_events::MessageOrigin>,
    /// Set by `arm_followup_redirect` when a follow-up interrupts a mid-turn
    /// coding-agent turn (the redirect path). The redirect analog of
    /// `cancel_actor`. The
    /// run_session interrupt arm drains it (`take_session_redirect_followup`)
    /// alongside `cancel_actor` and feeds it to `classify_result` so the
    /// interrupted turn's `ResponseCanceled` carries
    /// `CancelCause::SupersededByFollowup` (rendered neutrally) instead of
    /// `UserStop` (rendered "Canceled ✕"). A real Stop click via `interrupt_agent`
    /// leaves it `false`. Drained on read so a stale flag can't relabel the next
    /// turn on a resumed session.
    pub redirect_followup: bool,
    /// Also set by `arm_followup_redirect`, and deliberately NOT the same flag as
    /// `redirect_followup` above: the two answer different questions and are
    /// drained by different arms. `redirect_followup` says "the interrupt about to
    /// land is a redirect, not a Stop" and the interrupt arm takes it before the
    /// turn's `Result` is even parsed. This one says "a follow-up has been promised
    /// to this session but not yet routed", and it has to survive until the idle
    /// decision, which runs after that.
    ///
    /// It covers the one window `msg_rx` cannot: `arm_followup_redirect` fires the
    /// interrupt and returns, and its caller only sends the message after waiting
    /// for the interrupted turn to reach a boundary, so at the idle decision the
    /// channel is legitimately empty and the subprocess must still be kept.
    ///
    /// **Taken (read-and-clear) by the idle decision**, which gives it exactly one
    /// turn of grace. That bound is the point: an arming caller that dies before
    /// routing costs one kept-alive idle, not a subprocess pinned until the engine
    /// restarts.
    pub redirect_followup_pending: bool,
    /// Generic stop signal for the run_session loop. Fired by `stop_agent` for
    /// every user-driven termination (Cancel, Apply, Discard, Archive) and by
    /// the engine shutdown timeout. The stop arm reads `pending_stop` and
    /// `shutting_down` to decide what (if anything) to emit — only a real
    /// `UserStop` on an actively-working CC produces `ResponseCanceled`.
    pub stop: std::sync::Arc<tokio::sync::Notify>,
    /// Interrupt signal — sends control_request:interrupt to stop current work
    /// without killing the session (like pressing Esc in Claude Code terminal).
    pub interrupt: std::sync::Arc<tokio::sync::Notify>,
    /// Notified when the CC process enters idle/waiting state.
    /// Used by `apply_now` to wait for review/conflict resolution to complete.
    pub idle_notify: std::sync::Arc<tokio::sync::Notify>,
    /// When true, an `apply_now` task is already running for this thread.
    /// Prevents concurrent apply_now calls from causing duplicate merges.
    pub apply_now_in_progress: bool,
    /// The change whose merge-conflict resolution this session is carrying, set
    /// where the resolution binds to the session: at registration for the
    /// detached Tier-2 / Tier-3 spawns (they carry a `conflict_change_id`), and
    /// in `cc_assisted_merge_then_ff` for the Tier-1 in-place merge, which
    /// injects the merge prompt into a session that already existed.
    ///
    /// Read by the merge-ownership guard (ADR 0060) as the liveness half of
    /// "is a resolver working on this change right now". Descriptive, NOT a
    /// claim: nothing has to clear it, because the guard's other half is the
    /// durable `MergeConflictDetected` pairing and the binding dies with the
    /// session. A Tier-1 session deliberately outlives its own resolution, so a
    /// lingering binding here is expected and harmless.
    ///
    /// Naming the resolver is what keeps the guard from misreading an ORDINARY
    /// live session as one: a pairing stranded by a crash plus a later
    /// unrelated turn on the same thread would otherwise refuse every Apply for
    /// the length of that turn.
    pub conflict_change_id: Option<uuid::Uuid>,
    /// Set to true when the CC process exits. Checked by `apply_now_inner`
    /// after waking from `idle_notify` to detect CC death vs normal idle.
    pub process_exited: bool,
    /// Path to the worktree this Claude Code session is working in.
    pub worktree_path: Option<std::path::PathBuf>,
    /// Branch name this Claude Code session is working on.
    pub branch_name: Option<String>,
    /// Root of the repo (for git operations during apply).
    pub repo_root: Option<std::path::PathBuf>,
    /// Claude Code's session ID (from the "system" init event).
    /// Used for `--resume` on follow-ups and engine restart.
    pub cc_session_id: Option<String>,
    /// Epoch millis of the last event received from this Claude Code session.
    /// Used by `apply_now` for liveness-based timeout instead of fixed wall-clock.
    pub last_event_at: std::sync::Arc<std::sync::atomic::AtomicI64>,
    /// Set during engine shutdown so CC cleanup emits SessionEnded with
    /// reason SessionEndReason::Shutdown — the frontend uses this to show "Aborted"
    /// instead of "Done" for engine-interrupted exchanges.
    ///
    /// **Do not read this alone to classify a turn's terminal.** It is a
    /// SNAPSHOT signal: `shutdown_agent_sessions` sets it on the sessions
    /// present in `agent_sessions` when its pass ran, so a session inserted
    /// after that pass carries `false` through a teardown it is very much part
    /// of. Ask [`crate::engine::LucidosEngine::session_is_shutting_down`], which
    /// ORs the durable engine-global flag; its doc records the two incidents a
    /// bare read caused.
    ///
    /// `run_session/entry_guard.rs` is the one deliberate bare reader, and it is
    /// asking a different question: not "how should this turn be classified" but
    /// "does the shutdown sweep own this thread's terminal, so my drop-guard
    /// must not invent one". It holds only this `Arc`, never the engine.
    pub shutting_down: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Set by `abort_in_flight_for_restart` when it pre-emits a `ResponseAborted`
    /// on `/api/v1/restart` for this thread. The run_session loop's Result-classify
    /// and safety-net paths read this flag and skip their own terminal emit so
    /// the user only sees ONE "Response interrupted" panel (with the device
    /// actor) instead of two (one device, one system). Without this flag, CC's
    /// graceful interrupt → Result event → classify_result(is_shutdown=true)
    /// path emits a duplicate `ResponseAborted` 3s after the pre-emit.
    ///
    /// Covers one ordering only: a boundary emitted while this session was
    /// already in the map. The pre-emit iterates a snapshot, so a session still
    /// spawning gets no flag set at all, and the opposite ordering (boundary
    /// first, session second) is covered by the DB arm of
    /// `agent_session::runtime_helpers::external_terminal_already_emitted`.
    /// Read through that function rather than loading this directly.
    pub external_terminal_emitted: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Set by the external watchdog (alongside `external_terminal_emitted`,
    /// before it cancels the session) when the "terminal" it emitted is a
    /// recovery `ContinuationRequested{auto_recovery_after_hang}` — i.e. an
    /// auto-recovery continuation of this very turn is in flight. The
    /// suppression flag alone is ambiguous (a restart abort or a concurrent
    /// cancel also sets it), and a conflict-resolution session must
    /// distinguish: the wedged loop's completion reads this to HAND OFF the
    /// merge duty (`ConflictResolutionCleanupAction::HandOff`) instead of
    /// aborting the apply and tearing down the merge worktree underneath the
    /// continuation the watchdog just dispatched.
    pub external_continuation_requested: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Channel for sending control requests (set_model, set_permission_mode, etc.)
    /// from outside the event loop. The event loop forwards them to the runtime.
    pub control_tx: tokio::sync::mpsc::UnboundedSender<crate::runtime::ControlRequest>,
    /// Built-in CC commands (compact, clear, cost, etc.) — slash_commands minus skills.
    pub builtin_commands: Vec<String>,
    /// Skill commands (from plugins and user .claude/skills/).
    pub skill_commands: Vec<String>,
    /// Current model reported by CC (from the system init event's `model` field).
    pub current_model: Option<String>,
    /// Current reasoning effort level (low/medium/high).
    /// Not reported in CC's init event — only set via control request.
    pub current_reasoning_effort: Option<String>,
    /// Inputs the run loop has **forwarded to the agent driver** that the driver
    /// has not yet answered with a `Result`. Seeded to 1 when the spawn carries
    /// real content (that input is the one `Result` the first turn owes) and 0 for
    /// a silent resume / warm-up, then incremented in the `msg_rx.recv()` arm.
    ///
    /// Incrementing at the FORWARD site rather than at each `msg_tx.send` is what
    /// keeps this disjoint from the channel itself: a message still sitting in
    /// `msg_rx` is covered by `msg_rx.is_empty()`, this counter starts where that
    /// stops, and no message is in both. It also means every sender is counted by
    /// construction, including the three that never touched the old send-site
    /// counter (`apply_now`'s hardening prompt, the `run_bash_background`
    /// auto-wake, `change_ops::propose`).
    ///
    /// **Settled per backend at each `Result`**, by
    /// `lifecycle::settle_inputs_awaiting_result`, because the backends make
    /// different promises about how many Results an input earns. Claude Code merges
    /// back-to-back stdin inputs into a SINGLE Result, so one Result answers
    /// everything forwarded so far and the counter zeroes. The Codex app-server
    /// driver runs one child per accepted input and emits one Result EACH
    /// (`TurnOutcome::Continue` keeps queued inputs across an interrupt on exactly
    /// that promise), so a Result answers one and the counter decrements.
    ///
    /// A non-zero remainder after the settle keeps the subprocess alive at idle: the
    /// driver still owes a turn, and killing it would drop work the user already
    /// sent. Applying the Codex rule to Claude Code is what made a merged
    /// three-message turn report two phantom follow-ups, keep a dead session alive,
    /// and swallow the API-drop auto-resume (2026-08-07; see
    /// `docs/plans/2026-08-07-api-drop-resume-suppressed-by-phantom-followup-count.md`).
    pub inputs_awaiting_result: std::sync::Arc<std::sync::atomic::AtomicU32>,
    /// Set by `answer_pending_question` when a user answers an `AskUserQuestion`
    /// on a *live* subprocess (CC continues its turn in-place once the blocked
    /// PreToolUse hook is woken). That resume path does NOT go through `msg_tx`,
    /// so it never reaches the `msg_rx` arm's `reset_per_turn_flags` — the only
    /// place the run loop's per-turn `emitted_terminal_event` flag clears. Without
    /// this signal a turn whose flag was armed (every `Result` sets it, even when
    /// the terminal emit is suppressed) would drop all of CC's continued output as
    /// "post-terminal stragglers", stranding the thread on "thinking…". The run
    /// loop reads-and-clears this on the per-event lock it already takes and, if
    /// set while `emitted_terminal_event` is true, re-arms emission before
    /// matching the event. Set under the `agent_sessions` lock *before* waking the
    /// hook; default `false` so a genuine trailing straggler still drops. See
    /// docs/plans/2026-06-27-cc-question-answer-resume-straggler-drop.md.
    pub question_resume_pending: bool,
    /// Paired ToolCalled / ToolResult counter, mirrored from the local
    /// counter inside `run_session`. Read by the external watchdog
    /// (`external_watchdog::tick`) — which lives outside `run_session`'s
    /// `select!` and cannot see local variables. The in-loop watchdog still
    /// reads the local copy; both observe the same `Arc<AtomicI32>` so the
    /// value can't drift. > 0 means a tool is mid-execution (Bash, Read,
    /// AskUserQuestion, …) — legitimate silence, no fire from either
    /// watchdog.
    pub tools_in_flight: std::sync::Arc<std::sync::atomic::AtomicI32>,
    /// Which coding-agent backend drives this session. Locked at session
    /// creation. Read by the follow-up fast-path to decide how a mid-turn
    /// follow-up is delivered: Claude Code steers the live turn via stdin, so
    /// the message is forwarded as-is; Codex's app-server/exec protocols only
    /// accept input at a turn boundary, so a mid-turn follow-up first
    /// interrupts the running turn (see the Codex interrupt-and-redirect path
    /// in `chat::process`).
    pub coding_agent: crate::runtime::CodingAgent,
    /// Clone of `run_session`'s `agent_cancel` `CancellationToken` — the lever
    /// that tears the coding-agent subprocess down. `run_session` and its
    /// in-loop watchdog / stale-resume paths already hold the original and
    /// cancel it directly; this clone exists so the **external watchdog**
    /// (`external_watchdog::tick`), which lives OUTSIDE the per-thread loop and
    /// only sees this map, can cancel it too. Cancelling it makes the
    /// independent `driver_task` run its reap-safe `graceful_kill_child_process_group`
    /// teardown — the only pid-recycle-safe way to kill a child whose owning
    /// `run_session` `select!` has wedged (without it, an externally-recovered
    /// thread leaves the original subprocess alive, so the `--resume` spawns a
    /// second concurrent agent on the same worktree — the 2026-07-02
    /// demo-director double-process bug).
    pub agent_cancel: tokio_util::sync::CancellationToken,
}

impl AgentSession {
    /// Snapshot current slash commands for cache persistence.
    pub fn to_commands_info(&self) -> CcCommandsInfo {
        CcCommandsInfo {
            builtin_commands: self.builtin_commands.clone(),
            skill_commands: self.skill_commands.clone(),
        }
    }

    /// True when the session has an in-flight response: process alive and
    /// not waiting at a turn boundary.
    pub fn is_in_flight(&self) -> bool {
        self.is_live() && !self.is_waiting
    }

    /// True when this entry still has a running session loop behind it.
    ///
    /// **Membership in `agent_sessions` is not liveness.** `run_session` owns
    /// the receiving half of [`Self::msg_tx`], so a run future that is dropped
    /// rather than completed — a cancelled caller, an aborted task — closes the
    /// channel the instant it goes away, while the map entry it inserted
    /// survives untouched with `process_exited == false` (nothing on the
    /// cancellation path clears it). That leftover is a *phantom session*.
    ///
    /// One fooled three independent readers at once on 2026-07-28: worktree
    /// cleanup skipped the thread forever ("live agent session active"), the
    /// chat fast path sent a follow-up into a dead channel, and the resume
    /// guard refused every follow-up with "A coding agent is already running
    /// for this thread" — wedging the thread until the engine restarted. All
    /// three had asked `!process_exited`, which only the *loop* ever sets.
    ///
    /// `msg_tx.is_closed()` is the self-maintaining half of the answer: it is
    /// already correct with no cleanup having run at all. `run_session`'s
    /// drop-guard still reaps the entry, but liveness must never depend on that
    /// having happened yet — the guard's cleanup is asynchronous, so there is
    /// always a window where the entry outlives its loop.
    pub fn is_live(&self) -> bool {
        !self.process_exited && !self.msg_tx.is_closed()
    }
}

/// Test-only `AgentSession` builder, colocated with the type so the field list
/// lives in one place instead of being re-spelled in every test module.
///
/// Hands the receiver back deliberately: an `AgentSession` whose `msg_rx` has
/// been dropped is a phantom (see [`AgentSession::is_live`]), so a builder that
/// dropped it internally would silently hand every test a dead session. Bind it
/// (`let (session, _rx) = …`) for the lifetime of the test.
#[cfg(test)]
impl AgentSession {
    pub(crate) fn for_test() -> (Self, tokio::sync::mpsc::UnboundedReceiver<AgentUserInput>) {
        let (msg_tx, msg_rx) = tokio::sync::mpsc::unbounded_channel::<AgentUserInput>();
        (Self::for_test_with_sender(msg_tx), msg_rx)
    }

    /// Same, for a test that already owns the channel pair (and therefore keeps
    /// the receiver alive itself).
    pub(crate) fn for_test_with_sender(
        msg_tx: tokio::sync::mpsc::UnboundedSender<AgentUserInput>,
    ) -> Self {
        use std::sync::atomic::{AtomicBool, AtomicI32, AtomicI64, AtomicU32};
        use std::sync::Arc;
        Self {
            msg_tx,
            is_waiting: true,
            has_changes: false,
            requires_restart: false,
            pending_stop: None,
            cancel_actor: None,
            redirect_followup: false,
            redirect_followup_pending: false,
            stop: Arc::new(tokio::sync::Notify::new()),
            interrupt: Arc::new(tokio::sync::Notify::new()),
            idle_notify: Arc::new(tokio::sync::Notify::new()),
            apply_now_in_progress: false,
            conflict_change_id: None,
            process_exited: false,
            worktree_path: None,
            branch_name: None,
            repo_root: None,
            cc_session_id: None,
            shutting_down: Arc::new(AtomicBool::new(false)),
            external_terminal_emitted: Arc::new(AtomicBool::new(false)),
            external_continuation_requested: Arc::new(AtomicBool::new(false)),
            control_tx: tokio::sync::mpsc::unbounded_channel().0,
            builtin_commands: vec![],
            skill_commands: vec![],
            current_model: None,
            current_reasoning_effort: None,
            last_event_at: Arc::new(AtomicI64::new(0)),
            inputs_awaiting_result: Arc::new(AtomicU32::new(0)),
            question_resume_pending: false,
            tools_in_flight: Arc::new(AtomicI32::new(0)),
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            agent_cancel: tokio_util::sync::CancellationToken::new(),
        }
    }
}

/// Outcome class of an apply attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ApplyStatus {
    /// Branch merged into main (or external-repo handoff completed).
    Applied,
    /// Branch had nothing to merge — already applied, or no commits exist.
    /// Distinct from `Applied` so callers don't infer a fresh merge happened.
    Noop,
    /// Hardening recovery session was spawned. The change will auto-apply
    /// after hardening completes — `review_thread_id` points at it.
    Hardening,
    /// Merge conflict — a Claude Code session was spawned (`conflict_thread_id`)
    /// or an in-place merge failed and the original session stays alive.
    Conflict,
}

impl Default for ApplyStatus {
    /// `Noop` is the only safe "no-info" default. Defaulting to `Applied`
    /// would let a forgotten field silently claim a successful merge —
    /// the very ambiguity this struct exists to prevent.
    fn default() -> Self {
        Self::Noop
    }
}

/// Result of applying a pending change. Serialized directly as the HTTP
/// response body — see `docs/apply-change-api.md`.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ApplyResult {
    pub status: ApplyStatus,
    pub change_id: Uuid,
    pub thread_id: Option<Uuid>,
    pub restart_required: bool,
    pub message: String,
    /// Set on `Conflict` — frontend should focus this thread to resolve.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conflict_thread_id: Option<Uuid>,
    /// Set on `Hardening` — frontend should track as "applying" until done.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub review_thread_id: Option<Uuid>,
    /// SHA of main HEAD after the merge — absent when no real merge happened.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub applied_commit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_commit: Option<String>,
    pub commits_applied: usize,
    pub files_changed: usize,
}

fn applied_message(restart_required: bool) -> String {
    if restart_required {
        "Change applied. This change requires the engine to restart.".to_string()
    } else {
        "Change applied.".to_string()
    }
}

impl ApplyResult {
    pub fn applied(change_id: Uuid, thread_id: Option<Uuid>, restart_required: bool) -> Self {
        Self {
            status: ApplyStatus::Applied,
            change_id,
            thread_id,
            restart_required,
            message: applied_message(restart_required),
            ..Self::default()
        }
    }

    pub fn applied_with_merge(
        change_id: Uuid,
        thread_id: Option<Uuid>,
        restart_required: bool,
        previous_commit: String,
        applied_commit: String,
        commits: &[String],
        files_changed: usize,
    ) -> Self {
        Self {
            status: ApplyStatus::Applied,
            change_id,
            thread_id,
            restart_required,
            message: applied_message(restart_required),
            applied_commit: Some(applied_commit),
            previous_commit: Some(previous_commit),
            commits_applied: commits.len(),
            files_changed,
            ..Self::default()
        }
    }

    pub fn noop(
        change_id: Uuid,
        thread_id: Option<Uuid>,
        files_changed: usize,
        message: impl Into<String>,
    ) -> Self {
        Self {
            status: ApplyStatus::Noop,
            change_id,
            thread_id,
            message: message.into(),
            files_changed,
            ..Self::default()
        }
    }

    pub fn hardening(
        change_id: Uuid,
        review_thread_id: Uuid,
        files_changed: usize,
        message: impl Into<String>,
    ) -> Self {
        Self {
            status: ApplyStatus::Hardening,
            change_id,
            thread_id: Some(review_thread_id),
            message: message.into(),
            review_thread_id: Some(review_thread_id),
            files_changed,
            ..Self::default()
        }
    }

    pub fn conflict(
        change_id: Uuid,
        conflict_thread_id: Uuid,
        files_changed: usize,
        message: impl Into<String>,
    ) -> Self {
        Self {
            status: ApplyStatus::Conflict,
            change_id,
            thread_id: Some(conflict_thread_id),
            message: message.into(),
            conflict_thread_id: Some(conflict_thread_id),
            files_changed,
            ..Self::default()
        }
    }
}
