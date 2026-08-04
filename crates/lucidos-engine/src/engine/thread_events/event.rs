use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::runtime::CodingAgent;

use super::{
    AbortCause, ActorMode, AnswerKind, CancelCause, ChildCompletionStatus, MessageOrigin,
    QuestionOption, SessionEndReason, TodoItem, TriggerInvocation,
};

/// Replay default for events persisted before the `agent` field existed —
/// all such events were Claude Code (the only agent at the time).
fn default_coding_agent_claude_code() -> CodingAgent {
    CodingAgent::ClaudeCode
}

fn is_empty_str(s: &str) -> bool {
    s.is_empty()
}
fn is_false(b: &bool) -> bool {
    !b
}
/// Skip-serializing predicate for the default `CodingAgentKind` (`Lucidos`)
/// — keeps `SessionStarted` rows that have no app/external context wire-quiet
/// so the diff against legacy rows stays minimal.
fn is_default_coding_agent_kind(k: &crate::engine::agent_session::CodingAgentKind) -> bool {
    matches!(k, crate::engine::agent_session::CodingAgentKind::Lucidos)
}
fn default_session_ended_reason() -> SessionEndReason {
    // Legacy DB rows persisted before `reason` was a required wire field lack
    // the field entirely. Removed non-terminal reasons that DO carry a value
    // deserialize via `#[serde(other)]` into `LegacyNonTerminal`; this default
    // covers the missing-field case for the same row class.
    SessionEndReason::LegacyNonTerminal
}

/// Backward-compat default for `MessageReceived.mode` on DB rows persisted
/// before the `mode` field existed. New emissions MUST set `mode` explicitly
/// — the API layer enforces this on incoming requests.
fn default_mode_human() -> ActorMode {
    ActorMode::Human
}

fn default_cancel_cause() -> CancelCause {
    CancelCause::Unknown
}

fn default_abort_cause() -> AbortCause {
    AbortCause::Unknown
}

fn default_true() -> bool {
    true
}

fn default_inject_mode() -> ActorMode {
    // Historical UserPromptInjected rows pre-date the mode field. The only
    // emit site at the time was the user-correction path, so defaulting to
    // Human keeps legacy rows attributed correctly.
    ActorMode::Human
}

/// Persisted thread events — stored in the DB with thread_id + sequence.
/// Names are past tense, matching the `event_type` column.
///
/// Every field needed for persistence AND SSE broadcast lives here.
/// New fields use `#[serde(default)]` so old DB rows deserialize safely.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ThreadEvent {
    // Chat
    MessageReceived {
        text: String,
        /// Content-addressed sha256 hashes of user-attached image blobs.
        /// Bytes live exactly once under `data/blobs/<hh>/<hash>.<ext>`;
        /// the LLM call resolves hashes to bytes at send time. Old DB rows
        /// migrate from `images: [{base64, mime_type}, ...]` via the
        /// startup migration in `core::image_migration`.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        user_image_hashes: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        device_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        device: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        image_description: Option<String>,
        /// Set when this thread was spawned by another thread. Required when
        /// `mode != Human` and the spawn originated from a parent thread.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent_thread_id: Option<uuid::Uuid>,
        /// Event in the parent thread that triggered this spawn (e.g. the
        /// `ToolCalled` event for a `run_thread` invocation). Only set when
        /// `mode != Human`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        spawning_event_id: Option<uuid::Uuid>,
        /// Semantic mode of the actor that emitted this message. Required for
        /// new emissions (enforced at the API layer). The serde default exists
        /// only to replay old DB rows persisted before the `mode` field existed.
        #[serde(default = "default_mode_human")]
        mode: ActorMode,
        /// Model the engine will use to answer this message. Stamped at request
        /// time so the route tooltip can display it before ResponseGenerated
        /// fires. Coding-agent sessions still rely on CodingAgentSettingsChanged.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        /// Reasoning effort the engine will use to answer this message.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reasoning_effort: Option<String>,
        /// Structured origin — captured from HTTP headers / device lookup at
        /// the API boundary. Optional on the wire so old DB rows deserialize
        /// cleanly; the frontend's `legacyOrigin()` synthesizes from the
        /// legacy `device_id` / `parent_thread_id` fields when this is None.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        origin: Option<MessageOrigin>,
    },
    /// User removed a queued chat follow-up before the agentic loop ingested it.
    /// The original `MessageReceived` stays in the append-only log; renderers
    /// hide it while it is still stepless, and the agentic loop skips the
    /// matching injected prompt when it drains the queue.
    QueuedMessageRemoved {
        removed_message_id: uuid::Uuid,
    },
    TextStreamed {
        text: String,
    },
    #[serde(alias = "Thinking")]
    ThoughtStreamed {
        text: String,
    },
    /// One captured context per LLM call. `usage` is None pre-call and on
    /// providers that don't report it (OpenAI, Gemini); when present it
    /// reflects the real prompt-token cost from the provider's `usage`
    /// block. `estimated_total_tokens` is the engine's pre-call estimate
    /// (`estimate_tokens_from_chars` — 1.5 chars/token, matching the trim
    /// budget); the modal renders both so the user can spot estimator
    /// drift.
    ContextCaptured {
        producer: crate::engine::ContextProducer,
        model: String,
        context_window: usize,
        sections: Vec<crate::engine::ContextSection>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        tools: Vec<String>,
        estimated_total_tokens: usize,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        usage: Option<crate::engine::ApiUsage>,
        #[serde(default, skip_serializing_if = "is_false")]
        trimmed: bool,
    },
    MemorySearched {
        #[serde(default)]
        results: usize,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        queries: Vec<String>,
    },
    ToolCalled {
        name: String,
        args: Value,
        #[serde(default, skip_serializing_if = "is_empty_str")]
        description: String,
    },
    ToolResult {
        name: String,
        result: String,
        #[serde(default)]
        images: Vec<String>,
        #[serde(default = "default_true")]
        success: bool,
        /// Event id of the originating `ToolCalled`. Stamped on every live
        /// emit by the chat agentic loop and on synthetic backfills by the
        /// recovery sweep (`recover_orphan_tool_calls`); the frontend's
        /// `groupIntoExchanges` uses it (via `chatToolCallOwners`) to route
        /// the result back to the call's exchange. Without explicit pairing,
        /// an `ask_user_question` result followed the post-`UserQuestionAsked`
        /// request_id redirect into the question divider and left the
        /// original exchange's "Executing …" spinner pending forever.
        /// Server-side resume-block synthesis (LLM-message pairing) still
        /// uses chronological name matching — see
        /// `collect_tool_pairs_chronological`. Optional for legacy DB rows
        /// (pre-field): callers that fall back to chronological pairing
        /// continue to work when absent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool_called_event_id: Option<uuid::Uuid>,
    },
    /// Lucidos Agent's current todo list, replaced wholesale on every
    /// `todo_write` tool call. Sticky UI panel reads the latest of these per
    /// thread; the empty list means the agent cleared the list. Not emitted
    /// from coding-agent threads — CC's own `TodoWrite` continues to render
    /// inline on the tool-call step.
    TodoListWritten {
        items: Vec<TodoItem>,
    },
    /// Background task spawned via `run_bash_background` (shell command) OR
    /// `run_python_background` (venv-rooted python script — the engine
    /// wraps it as `bash -o pipefail -c "<venv-python> <script>"` and routes through
    /// the same `BackgroundBashRegistry`). The `command` field captures
    /// the exact shell invocation, so a reader can tell which spawning
    /// tool produced the row. Paired with a later `BackgroundBashCompleted`.
    /// The two events are the durable audit trail of every long-running
    /// background task — `bash_output` reads from the in-memory registry
    /// while a task runs and for the few minutes its completion is retained
    /// there, then falls back to the `BackgroundBashCompleted` payload,
    /// regardless of which tool spawned it.
    BackgroundBashStarted {
        task_id: String,
        command: String,
        timeout_secs: u64,
        started_at: chrono::DateTime<chrono::Utc>,
    },
    BackgroundBashCompleted {
        task_id: String,
        /// Truncated to 200 chars for log readability; full command lives
        /// on the paired `BackgroundBashStarted`.
        command: String,
        /// The child's normal exit status. `None` whenever there isn't one:
        /// the child died on a signal (see `signal`), or the engine failed to
        /// reap it at all. Never a stand-in number — a reader that sees
        /// `exit_code: 0` can trust the command really exited 0.
        exit_code: Option<i32>,
        /// Unix signal that terminated the child, when one did (9 = SIGKILL
        /// from the watchdog timeout or `bash_kill`, 11 = SIGSEGV, 13 =
        /// SIGPIPE from a `pipefail` pipeline, …). Mutually exclusive with
        /// `exit_code`; both `None` means the status was unavailable.
        /// Absent on rows written before the field existed.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signal: Option<i32>,
        stdout: String,
        stderr: String,
        started_at: chrono::DateTime<chrono::Utc>,
        finished_at: chrono::DateTime<chrono::Utc>,
        /// Watchdog killed the task because `timeout_secs` elapsed.
        #[serde(default, skip_serializing_if = "is_false")]
        timed_out: bool,
        /// `bash_kill` killed the task explicitly.
        #[serde(default, skip_serializing_if = "is_false")]
        killed: bool,
    },
    ResponseGenerated {
        #[serde(default, skip_serializing_if = "is_empty_str")]
        text: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        images: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reasoning_effort: Option<String>,
    },
    ResponseCanceled {
        #[serde(default, skip_serializing_if = "is_empty_str")]
        text: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        images: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reasoning_effort: Option<String>,
        #[serde(default = "default_cancel_cause")]
        cause: CancelCause,
    },
    ResponseAborted {
        #[serde(default, skip_serializing_if = "is_empty_str")]
        text: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        images: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reasoning_effort: Option<String>,
        #[serde(default = "default_abort_cause")]
        cause: AbortCause,
    },
    ResponseFailed {
        error: String,
    },

    // Resume-after-abort boundary. Opens a new exchange in the timeline whose
    // body is the rerun (chat: SessionRecovered's predecessor → engine note +
    // re-LLM call; CC: --resume into the same cc_session_id). Past name was
    // SessionRecovered (and SessionResumed before that) — kept as serde aliases
    // so older DB rows still deserialize. Renamed to ContinuationStarted
    // because (a) "session" was ambiguous between chat and CC, and (b) the
    // resumed response is actually a *new* response continuing the prior
    // attempt, not the prior response coming back to life.
    #[serde(alias = "SessionRecovered", alias = "SessionResumed")]
    ContinuationStarted {
        #[serde(default, skip_serializing_if = "is_empty_str")]
        branch: String,
        /// Engine-stamped origin so the route popover can render
        /// "Engine · Auto-resumed after restart" for recovered sessions.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        origin: Option<MessageOrigin>,
        /// Why the continuation opened — forwarded from the originating
        /// `ContinuationRequested.reason`. The frontend reads it to label the
        /// resume honestly: `user_clicked_continue` is genuinely a resume
        /// after an engine restart, but `auto_recovery_after_hang` fires for a
        /// hung subprocess OR a stray signal-kill (e.g. a cross-workspace
        /// `cargo check` broad-kill) where NO engine restart happened —
        /// labeling those "Resumed after engine restart" misattributes a local
        /// interruption to a restart. `None` for legacy rows and the chat
        /// rerun path (which carries its own engine note instead).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    SessionStarted {
        session_id: String,
        #[serde(default, skip_serializing_if = "is_empty_str")]
        branch: String,
        /// External repository ID this session is bound to.
        /// Persisted in thread_summaries so follow-ups reuse the same repo.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        repo_id: Option<String>,
        /// What kind of coding-agent thread this is: `lucidos` (default,
        /// edits Lucidos source), `app` (edits a `data/apps/<id>/` folder
        /// in the workspace), or `external` (edits a registered external
        /// repo). Default applies to legacy rows persisted before this
        /// field existed; those are interpreted via `repo_id` (NULL ⇒
        /// `lucidos`, set ⇒ `external`).
        #[serde(default, skip_serializing_if = "is_default_coding_agent_kind")]
        coding_agent_kind: crate::engine::agent_session::CodingAgentKind,
        /// Canonical folder the user (or LLM) picked when spawning. For
        /// `app` threads this is `<workspace>/data/apps/<id>/`. For
        /// `lucidos` and `external` it equals the repo root. Empty for
        /// legacy rows persisted before this field existed.
        #[serde(default, skip_serializing_if = "is_empty_str")]
        coding_agent_folder: String,
        /// App id when `coding_agent_kind == "app"`; `None` otherwise.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        app_id: Option<String>,
        /// Which backend drives this thread (`claude-code` | `codex`).
        /// Locked in at first SessionStarted via the thread_summaries
        /// projection (COALESCE keeps the existing value) so follow-ups and
        /// recovery resume on the same backend. Default covers legacy rows —
        /// all Claude Code.
        #[serde(default = "default_coding_agent_claude_code")]
        coding_agent: CodingAgent,
    },
    SessionEnded {
        /// Why the session ended. Always serialized — the frontend reads
        /// `reason` to distinguish a normal completion from a system abort,
        /// and `Completed` must reach the wire as `"completed"`. The default
        /// applies only when reading old DB rows persisted before `reason`
        /// existed.
        #[serde(default = "default_session_ended_reason")]
        reason: SessionEndReason,
    },
    #[serde(alias = "ClaudeCodeTextStreamed")]
    CodingAgentTextStreamed {
        text: String,
        #[serde(default = "default_coding_agent_claude_code", alias = "agent")]
        coding_agent: CodingAgent,
    },
    /// Streamed reasoning/thinking from a coding agent — the coding-agent mirror
    /// of the chat agent's `ThoughtStreamed`. Carries plaintext reasoning the
    /// agent emitted before its visible output (CC's `thinking_delta`; Codex's
    /// `item/reasoning/*Delta` / `reasoning` item). Per-token streamed, persisted,
    /// rendered as the "Thinking" step's live content so a long reasoning pass
    /// shows progress instead of a silent "Working" gap.
    #[serde(alias = "ClaudeCodeThoughtStreamed")]
    CodingAgentThoughtStreamed {
        text: String,
        #[serde(default = "default_coding_agent_claude_code", alias = "agent")]
        coding_agent: CodingAgent,
    },
    #[serde(alias = "ClaudeCodeToolCalled")]
    CodingAgentToolCalled {
        name: String,
        args: Value,
        #[serde(default, skip_serializing_if = "is_empty_str")]
        description: String,
        #[serde(default = "default_coding_agent_claude_code", alias = "agent")]
        coding_agent: CodingAgent,
        /// Agent-issued identifier for this tool invocation, persisted so the
        /// matching `CodingAgentToolResult` can be paired with the call even
        /// when an `EXCHANGE_START_TYPES` event (e.g. permission prompt)
        /// splits them across exchanges. Empty for legacy DB rows.
        #[serde(default, skip_serializing_if = "is_empty_str")]
        tool_use_id: String,
    },
    #[serde(alias = "ClaudeCodeToolResult")]
    CodingAgentToolResult {
        name: String,
        result: String,
        #[serde(default = "default_coding_agent_claude_code", alias = "agent")]
        coding_agent: CodingAgent,
        /// Matches the originating `CodingAgentToolCalled.tool_use_id`.
        /// Empty for legacy DB rows.
        #[serde(default, skip_serializing_if = "is_empty_str")]
        tool_use_id: String,
    },
    #[serde(alias = "ClaudeCodeUserMessageSent")]
    CodingAgentUserMessageSent {
        text: String,
        #[serde(default = "default_coding_agent_claude_code", alias = "agent")]
        coding_agent: CodingAgent,
    },
    /// Automated prompt sent to a coding agent (e.g., conflict resolution, hardening).
    /// Persisted for audit trail but not rendered in the chat UI.
    #[serde(alias = "ClaudeCodePromptSent")]
    CodingAgentPromptSent {
        text: String,
        #[serde(default = "default_coding_agent_claude_code", alias = "agent")]
        coding_agent: CodingAgent,
        /// Engine-stamped origin for prompts the engine itself synthesized
        /// (orphan recovery, hardening retrigger, merge conflict). Surfaced in
        /// the route popover so users can distinguish engine-driven prompts
        /// from agent-driven ones. `None` for legacy DB rows and for prompts
        /// that already carry their origin elsewhere in the chain.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        origin: Option<MessageOrigin>,
    },
    /// Emitted when the engine detects that a coding-agent session ended without
    /// running the required hardening. A recovery hardening session is spawned
    /// automatically. This is NOT a completion event — the thread stays active
    /// until hardening finishes.
    MissingHardeningDetected {
        /// Engine-stamped origin (always `MessageOrigin::Engine { reason:
        /// EngineReason::MissingHardening }` for new emits). Optional on the
        /// wire so legacy DB rows decode cleanly.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        origin: Option<MessageOrigin>,
    },
    #[serde(alias = "ClaudeCodeIdled")]
    CodingAgentIdled {
        #[serde(default, skip_serializing_if = "is_false")]
        has_changes: bool,
        #[serde(default, skip_serializing_if = "is_false")]
        is_external_repo: bool,
        #[serde(default, skip_serializing_if = "is_false")]
        requires_restart: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cc_session_id: Option<String>,
        #[serde(default = "default_coding_agent_claude_code", alias = "agent")]
        coding_agent: CodingAgent,
        /// Optional short tag describing why this idle was emitted. Most idles
        /// have no `reason` (the agent simply finished its turn). Recovery
        /// emits `Some("engine_restart_interrupt")` when a mid-turn-crashed
        /// session is surfaced to the UI as "interrupted, click to continue"
        /// instead of being auto-spawned. The frontend reads this field to
        /// render the continue affordance.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
        /// Absolute filesystem path of the worktree the agent ran in for this
        /// turn. Phase 6.1 of the CC resume architecture: persisted so that
        /// follow-up turns can look up the deterministic per-thread worktree
        /// directly from the events stream instead of scanning
        /// `git worktree list`. `None` on legacy rows that predate the field
        /// and on out-of-band idles emitted without a worktree (e.g. the
        /// "no branch" recovery path).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        worktree_path: Option<String>,
        /// `git rev-parse HEAD` in the worktree at the moment the agent went
        /// idle. Phase 8.1 of the CC resume architecture: persisted so that
        /// the next spawn can diff against this SHA + check `git status` to
        /// detect external user edits made between turns and inject a note
        /// into the resumed prompt. `None` on legacy rows, on idles emitted
        /// without a worktree, or when `git rev-parse` fails (e.g. branch
        /// has zero commits yet).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        worktree_head_sha: Option<String>,
        /// True iff the agent went idle while the chat-agent's
        /// `run_bash_background` tool still had a task running for this thread.
        /// **Recorded history only** — as of the bg-bash-gate removal this no
        /// longer drives any projection or UI. The change proposes at idle
        /// regardless of background bash (correctness is covered by
        /// harden-at-apply), and the chat-agent bash auto-resumes CC via
        /// `spawn_bash_completion_watcher`. Kept on the event so the timeline
        /// still records that an idle overlapped a background task.
        #[serde(default, skip_serializing_if = "is_false")]
        bg_bash_pending: bool,
    },
    /// Continuation requested — emitted when a CC turn that was interrupted
    /// (engine restart mid-turn, Q9a recovery path) needs to be resumed
    /// without a new user message. Four emit sites: HTTP `/continue`
    /// (user clicks Continue), the *in-loop watchdog* (10-min silence with
    /// the gate open), the *external watchdog* (12-min wedged `select!`),
    /// and the missing-hardening recovery sweep. Picked up by the spawn
    /// dispatcher (Phase 5, Task 5.2), which re-spawns CC via `--resume`
    /// with no new input. The dispatcher uses the event's id as the
    /// idempotency key so a single `ContinuationRequested` produces exactly
    /// one spawn. Past name was `ContinueSignal`; the rename migration
    /// (`20260518212540_rename_continue_signal_to_continuation_requested`)
    /// rewrote existing rows, and the serde alias remains as a safety net
    /// for any in-flight JSON not yet persisted. Renamed to past-tense
    /// per the events-only model
    /// (`AppUiRefreshRequested` not `RefreshAppUI`) and to pair with the
    /// already-past-tense `ContinuationStarted` terminal that opens the
    /// resulting exchange.
    ///
    /// `reason` is a short tag describing why a continuation was needed
    /// (e.g. `"engine_restart_interrupt"`); it is purely informational and
    /// surfaced for debugging / route-popover context.
    #[serde(alias = "ContinueSignal")]
    ContinuationRequested {
        #[serde(default, skip_serializing_if = "is_empty_str")]
        reason: String,
    },

    // Thread lifecycle
    ThreadTitleGenerated {
        title: String,
    },
    ThreadTitleRenamed {
        title: String,
    },
    ThreadSaved,
    ThreadUnsaved,
    ThreadArchived,
    /// A thread was created in `composing` state. Emitted by the first
    /// successful POST /threads (debounced first user input — keystroke,
    /// image attach, or mode toggle on a fresh compose). The thread can
    /// be addressed by id immediately after this event lands.
    ThreadStarted {
        /// Initial mode the user opened compose with. Mutable while the
        /// thread is `Composing`; locked on first `MessageReceived`.
        mode: String,
        /// Stamped by `api::actor::user_actor_resolved` so the timeline
        /// shows which device started the draft thread.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    /// A thread in `composing` state was explicitly discarded. Emitted by
    /// DELETE /threads/:id. Terminal — the state-machine guard rejects
    /// every subsequent compose PUT and message POST with 410 Gone, which
    /// is the "make impossible states impossible" lever that replaces the
    /// old LWW + tombstone machinery.
    ThreadDiscarded {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    /// A user attached an image to this thread's compose draft. Emitted by
    /// POST /api/v1/threads/:id/blobs after the bytes are content-addressed
    /// to disk under `data/blobs/<hh>/<hash>.<ext>`. The `hash` is the sole
    /// identity used by every downstream consumer (compose payload, message
    /// payload, LLM call); `mime` and `byte_size` are convenience fields so
    /// SSE subscribers can render the upload entry without fetching the
    /// blob. Same blob attached to two threads = two events, one per thread
    /// (the disk write is a no-op the second time, but the per-thread fact
    /// stays distinct).
    ImageUploaded {
        hash: String,
        mime: String,
        byte_size: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },
    TriggerStarted {
        #[serde(alias = "task_id")]
        trigger_id: String,
        #[serde(default, alias = "task_name", skip_serializing_if = "Option::is_none")]
        trigger_name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        prompt: Option<String>,
        /// Which path fired this run. `None` only on legacy DB rows persisted
        /// before this field existed — new emissions always set it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        invocation: Option<TriggerInvocation>,
        /// Engine-stamped origin for scheduler-fired triggers — always set to
        /// `Engine { Scheduler { trigger_id, trigger_name } }` so the route
        /// popover can render "Engine · Scheduled · <name>". `None` only on
        /// legacy DB rows persisted before this field existed.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        origin: Option<MessageOrigin>,
        /// Snapshot of the firing trigger's `go_to_review` flag. When true,
        /// the section transition logic treats this trigger thread as
        /// top-level so its terminal event surfaces it in REVIEW. Defaults
        /// to false for backward compat with pre-flag DB rows.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        go_to_review: bool,
    },
    TriggerCompleted {
        #[serde(alias = "task_id")]
        trigger_id: String,
        #[serde(default, alias = "task_name", skip_serializing_if = "Option::is_none")]
        trigger_name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        result_summary: Option<String>,
    },

    // Changes — change_id is the primary identifier
    ChangeProposed {
        #[serde(default, skip_serializing_if = "is_empty_str")]
        change_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        files: Vec<String>,
        #[serde(default, skip_serializing_if = "is_false")]
        requires_restart: bool,
        /// Engine-stamped origin for change proposals from engine-internal
        /// recovery paths (stale-session cleanup, orphan worktree cleanup).
        /// `None` for proposals authored by a live agent session — those
        /// inherit their origin from the surrounding `MessageReceived`.
        /// Surfaced in the route popover so users can render
        /// "Engine · Stale session cleanup" etc.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        origin: Option<MessageOrigin>,
        /// Legacy: was set by the deleted post-commit git hook on per-commit
        /// emits (empty `change_id`). Live aggregate emits always set `None`.
        /// Historical rows still carry it; the projection treats per-commit
        /// shape (empty `change_id`, `commit_sha: Some(_)`) as inert in
        /// `thread_summaries` and UPDATE-only on `changes`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        commit_sha: Option<String>,
        /// Branch the change lives on. Stamped by the first
        /// `ChangeProposed` per `change_id`; used by the projection to
        /// reconstruct the row without consulting the legacy `changes` table.
        #[serde(default, skip_serializing_if = "is_empty_str")]
        branch_name: String,
        /// Repo root the change targets. Same lifetime/intent as `branch_name`.
        #[serde(default, skip_serializing_if = "is_empty_str")]
        repo_root: String,
        /// `true` if the originating commit landed on a hardened HEAD; surfaced
        /// in the projection so the apply UI can short-circuit re-hardening.
        /// Stamped from `is_harden_marker_present` at end-of-turn aggregate
        /// emit; engine-internal recovery emits resolve it the same way.
        #[serde(default, skip_serializing_if = "is_false")]
        hardened: bool,
        /// `true` only when engine-internal recovery proposes commits whose
        /// originating turn was killed before closure (stale-session sweep,
        /// orphan worktree recovery). The frontend reads this to confirm
        /// before Apply so the user knows they're landing recovered work.
        /// Live end-of-turn aggregate emits always stamp `false` because
        /// `may_touch_change_state_at_idle` refuses to emit on non-`Generated`
        /// terminals.
        #[serde(default, skip_serializing_if = "is_false")]
        incomplete: bool,
        // Legacy fields — kept for backward compat with old DB rows
        #[serde(default, skip_serializing_if = "is_empty_str")]
        path: String,
        #[serde(default, skip_serializing_if = "is_empty_str")]
        diff: String,
    },
    ChangeApplied {
        #[serde(default, skip_serializing_if = "is_empty_str")]
        change_id: String,
        #[serde(default, skip_serializing_if = "is_false")]
        requires_restart: bool,
        #[serde(default, skip_serializing_if = "is_false")]
        client_update: bool,
        /// Commit subjects merged to main, oldest first. Empty for no-op applies.
        /// Surfaced in the restart-required toast grouped by thread.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        commits: Vec<String>,
        /// Title of the originating thread, included so the restart toast can
        /// group entries without an extra lookup.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thread_title: Option<String>,
        /// Who applied the change — always the human/HTTP/SDK initiator
        /// stamped by `api/actor::build_message_origin` from the originating
        /// HTTP call. None on legacy DB rows or when the conflict-resolution
        /// ff-merge path runs (the original applier's actor is dropped across
        /// the async gap; popover then renders "Unknown").
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
        /// SHA of `main` before the merge — paired with `post_merge_sha` to
        /// give Revert the exact commit range to drop. `None` for events
        /// emitted before the projection rewrite.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pre_merge_sha: Option<String>,
        /// SHA of `main` after the merge.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        post_merge_sha: Option<String>,
        // Legacy
        #[serde(default, skip_serializing_if = "is_empty_str")]
        path: String,
    },
    ChangeDiscarded {
        #[serde(default, skip_serializing_if = "is_empty_str")]
        change_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
        // Legacy
        #[serde(default, skip_serializing_if = "is_empty_str")]
        path: String,
    },
    ChangeReverted {
        #[serde(default, skip_serializing_if = "is_empty_str")]
        change_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
        // Legacy
        #[serde(default, skip_serializing_if = "is_empty_str")]
        path: String,
    },
    ChangeApplyFailed {
        #[serde(default, skip_serializing_if = "is_empty_str")]
        change_id: String,
        #[serde(default, skip_serializing_if = "is_empty_str")]
        error: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },

    // Merge conflict resolution
    MergeConflictDetected {
        #[serde(default, skip_serializing_if = "is_empty_str")]
        change_id: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        files: Vec<String>,
        /// Engine-stamped origin (always `MessageOrigin::Engine { reason:
        /// EngineReason::MergeConflict }` for new emits). Optional on the wire
        /// so legacy DB rows decode cleanly.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        origin: Option<MessageOrigin>,
    },
    /// A merge-resolution worktree was set up for a change with conflicts.
    /// The projection treats this as the change's `merge_worktree_path` /
    /// `merge_temp_branch` until a `MergeResolutionCleared` arrives. Survives
    /// restart so startup cleanup can find dangling worktrees.
    MergeResolutionStarted {
        #[serde(default, skip_serializing_if = "is_empty_str")]
        change_id: String,
        #[serde(default, skip_serializing_if = "is_empty_str")]
        worktree_path: String,
        #[serde(default, skip_serializing_if = "is_empty_str")]
        temp_branch: String,
    },
    /// The merge-resolution worktree was torn down (cleanup finished).
    MergeResolutionCleared {
        #[serde(default, skip_serializing_if = "is_empty_str")]
        change_id: String,
    },
    /// A change's working tree was hardened (`/harden` marker stamped on
    /// HEAD). Idempotent: re-emitting after an unrelated commit is fine,
    /// the projection treats only the latest event per `change_id`.
    /// Downgraded to false implicitly when a fresh `ChangeProposed` arrives
    /// with `hardened: false`.
    ChangeHardened {
        #[serde(default, skip_serializing_if = "is_empty_str")]
        change_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageOrigin>,
    },

    /// Coding-agent session settings changed mid-session (model, reasoning effort,
    /// or permission mode). Persisted per-thread so settings survive idle exit
    /// and respawn.
    ///
    /// Also carries `cc_session_id`: the agent emits this event at `Init` (the
    /// first moment CC reports its session id) with `cc_session_id: Some(..)`,
    /// so the resume/recovery lookups can find the id even when the session is
    /// interrupted by an engine restart *before* it ever reaches a
    /// `CodingAgentIdled` boundary (the only other event that carries it). The
    /// id is CC's authoritative Init fact, exactly like `model`. Settings-only
    /// emits (startup persist, mid-session model/effort/permission changes) pass
    /// `None`; the lookups read the most recent non-null value across both
    /// event types.
    #[serde(alias = "CCSettingsChanged")]
    CodingAgentSettingsChanged {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reasoning_effort: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        permission_mode: Option<String>,
        #[serde(default = "default_coding_agent_claude_code", alias = "agent")]
        coding_agent: CodingAgent,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cc_session_id: Option<String>,
        /// Absolute `CLAUDE_CONFIG_DIR` the session was created under, stamped
        /// alongside `cc_session_id` at the Init emit — the authoritative
        /// session↔config-dir pairing. CC keys each session's transcript on this
        /// dir (`$CLAUDE_CONFIG_DIR/projects/<cwd>/<sid>.jsonl`), so a follow-up
        /// resume re-injects it (see `lookup_pinned_cc_config_dir` +
        /// `SpawnArgs::claude_config_dir`) so a mid-flight user toggle of the env
        /// var can't strand the session (dev/bf997e21). `None` on legacy rows,
        /// on the pre-Init settings emit, and on mid-session settings-only emits
        /// (model/effort/permission changes) — the Init emit is the carrier.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        claude_config_dir: Option<String>,
    },

    // Mid-flight injection — a user correction OR a system message (e.g. parent
    // thread receiving a child's [Child thread completed] callback) sent into
    // the agentic loop. `mode` and `origin` describe the actor; both default
    // for back-compat with old DB rows that pre-date these fields.
    UserPromptInjected {
        text: String,
        #[serde(default = "default_inject_mode")]
        mode: ActorMode,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        origin: Option<MessageOrigin>,
        // None for engine-driven injections (resume notes from chat/rerun.rs)
        // and for legacy DB rows that pre-date this field.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        injected_message_id: Option<uuid::Uuid>,
    },

    // Interactive (persisted)
    CredentialRequested {
        provider: String,
    },
    McpConsentRequested {
        tool: String,
        args: Value,
    },

    // Interactive question card with selectable options. Emitted by both
    // CC's built-in `AskUserQuestion` tool and the chat agent's
    // `ask_user_question` LLM tool. `meta.channel` distinguishes which
    // agent raised it (`claude_code` / `chat`). Resume goes through
    // POST /api/v1/threads/{thread_id}/answer-question, which emits
    // UserQuestionAnswered and dispatches the channel-specific resume:
    // CC respawns its subprocess with --resume + a matching tool_result;
    // chat wakes its in-process tool waiting on the question wait registry.
    //
    // `cc_session_id` is the Claude Code session id at intercept time, used to pin
    // `--resume` to the right CLI conversation. **Empty string** when the
    // chat agent raises the question — chat is in-process and has no
    // resume token.
    //
    // `worktree_path` is the absolute path of the CC worktree at intercept
    // time. CC stores session JSONLs keyed by CWD
    // (`~/.claude/projects/<encoded-cwd>/<sid>.jsonl`), so resume MUST
    // start the new subprocess in the same directory or `--resume` returns
    // "No conversation found". Branch lookup is unreliable here because CC
    // is free to `git checkout -b ...` inside the worktree — see
    // `engine/agent_session/run_session.rs` for how this is used on resume.
    // **`None`** for chat-channel questions.
    UserQuestionAsked {
        tool_use_id: String,
        cc_session_id: String,
        question: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        options: Vec<QuestionOption>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        worktree_path: Option<String>,
        #[serde(default)]
        multi_select: bool,
    },
    UserQuestionAnswered {
        tool_use_id: String,
        answer: AnswerKind,
    },

    /// CC requested permission for a tool call (e.g. Edit/Write/Bash on a path
    /// outside cwd or under `.claude/`). Renders as a `PermissionCard` in the
    /// thread; the user's answer resolves the deduped entry keyed by
    /// `request_id` in `Engine.pending_cc_permission`. Persisted so the card
    /// survives reload.
    CodingAgentPermissionRequest {
        request_id: String,
        tool_use_id: String,
        tool_name: String,
        input: Value,
        summary: String,
    },
    /// User answered (or system timed out) a `CodingAgentPermissionRequest`.
    /// Emitted by the permission-prompt handler immediately after the oneshot
    /// resolves, before returning to the MCP subprocess.
    ///
    /// `persist_scope` records which scope the user picked when granting an
    /// "Always allow"-style click (`narrow` / `broad` / `session`). `None`
    /// covers Allow-once, Deny, and the recovery-emitted orphan resolution
    /// (engine doesn't know what the user *would* have picked). The frontend
    /// uses it to render the answered card with a check on the chosen button
    /// and strike-through on the rest, so reload reproduces the same view.
    CodingAgentPermissionResolved {
        request_id: String,
        allowed: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        persist_scope: Option<crate::engine::claude_code::AllowScope>,
    },

    /// The Lucidos Agent's command guard (ADR 0002) paused a bash/python tool
    /// call to ask the user for permission — the `IrreversibleDanger` lane on a
    /// `Chat` channel. The chat counterpart of `CodingAgentPermissionRequest`:
    /// it renders the same `PermissionCard`, but the agent loop blocks
    /// in-process on `Engine.pending_command_permission` (no MCP subprocess).
    /// `tool_name` is one of the bash/python tools; `command` is the inspected
    /// command text (the bash `command` or python `code`); `summary` is the
    /// card's one-line description. Persisted so the card survives reload.
    CommandPermissionRequested {
        request_id: String,
        tool_use_id: String,
        tool_name: String,
        command: String,
        summary: String,
    },
    /// User resolved a `CommandPermissionRequested` (or the engine resolved it
    /// as superseded / orphaned). Chat counterpart of
    /// `CodingAgentPermissionResolved`; `persist_scope` records the
    /// "Always allow" / "Allow for this thread" scope the user picked so reload
    /// reproduces the answered card. `None` covers Allow-once, Deny, and the
    /// engine-emitted superseded/orphan resolutions.
    CommandPermissionResolved {
        request_id: String,
        allowed: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        persist_scope: Option<crate::engine::claude_code::AllowScope>,
    },

    /// The Lucidos Agent (chat) paused an MCP server tool call to ask the user
    /// for permission — the chat counterpart of `CommandPermissionRequested` for
    /// MCP tools. Renders the same `PermissionCard`; the agentic loop blocks
    /// in-process on `Engine.pending_mcp_permission` until the user resolves it.
    /// `server_id` is the MCP server registry key (stable, used to derive the
    /// persisted `Mcp(server:tool)` / `Mcp(server:*)` grant pattern);
    /// `server_name` is the human label shown on the card; `tool_name` is the
    /// bare MCP tool; `arguments_summary` is the pretty-printed (truncated) args.
    /// Persisted so the card survives reload. Replaces the legacy transient
    /// `McpConsentPromptRequested` + `showConfirm` modal. Auto-approved silently
    /// (no event) in non-interactive trigger threads and when the server's
    /// `auto_approve` flag is set.
    McpPermissionRequested {
        request_id: String,
        tool_use_id: String,
        server_id: String,
        server_name: String,
        tool_name: String,
        arguments_summary: String,
    },
    /// User resolved an `McpPermissionRequested` (or the engine resolved it as
    /// superseded / orphaned). Chat counterpart of `CommandPermissionResolved`;
    /// `persist_scope` records the "Always allow this tool" (`narrow` →
    /// `Mcp(server:tool)`) / "Always allow this server" (`broad` →
    /// `Mcp(server:*)`) / "Allow for this thread" (`session`) choice the user
    /// picked so reload reproduces the answered card. `None` covers Allow-once,
    /// Deny, and the engine-emitted superseded/orphan resolutions.
    McpPermissionResolved {
        request_id: String,
        allowed: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        persist_scope: Option<crate::engine::claude_code::AllowScope>,
    },

    /// The command guard snapshotted the workspace's git-tracked `data/` before
    /// running a `ReversibleDanger` command (in-workspace destruction) — ADR
    /// 0002, Phase 4. The snapshot lives on the safety ref
    /// `refs/lucidos/command-checkpoints/<checkpoint_id>`; `command` is the
    /// command about to run and `summary` is the one-line card text. Persisted
    /// so the one-click Undo affordance survives reload. Emitted only after the
    /// snapshot succeeds (a failed snapshot lets the command run unguarded, no
    /// event — same as the pre-Phase-4 behavior).
    CommandCheckpointed {
        checkpoint_id: String,
        command: String,
        summary: String,
    },
    /// The user clicked Undo on a `CommandCheckpointed` card (or the engine
    /// resolved it). The workspace working tree was restored from the checkpoint
    /// ref and the ref deleted; the frontend renders the card as reverted.
    /// Persisted so the reverted state survives reload.
    CommandCheckpointReverted {
        checkpoint_id: String,
    },

    /// Background worktree cleanup happened on this thread (Phase 10.2).
    /// `tier=1` means build artifacts (`target/`, `node_modules/`,
    /// `.lucidos/cache/`) were stripped from a long-idle worktree; the
    /// worktree itself is still on disk. `tier=2` means the entire worktree
    /// directory was removed (long-idle, clean, unsaved). `freed_bytes` is
    /// a best-effort sum of file sizes reclaimed; on filesystems where
    /// metadata is partially unavailable it may be `0` even if real space was
    /// freed. `branch_deleted` is `true` when Tier 2 also dropped a
    /// fully-merged branch (Phase 10.3).
    WorktreeCleaned {
        tier: u8,
        freed_bytes: u64,
        #[serde(default, skip_serializing_if = "is_false")]
        branch_deleted: bool,
    },

    /// A child thread spawned by `run_thread` / `run_coding_agent` finished its turn.
    /// Emitted on the **parent** thread by the EventBus fan-out fan-in path
    /// when the child reaches a terminal event (CC: `CodingAgentIdled` or
    /// `SessionEnded`; chat: `ResponseGenerated` / `ResponseFailed`).
    ///
    /// Replaces the previous prose-only `[Child thread completed] ...` user
    /// message that was injected via the `parent_callback_tx` channel. The
    /// channel still wakes the parent — the payload is now the typed event id
    /// rather than a prebuilt string.
    ChildThreadCompleted {
        child_thread_id: uuid::Uuid,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        child_thread_title: Option<String>,
        status: ChildCompletionStatus,
        /// Free-form summary — prose body of the child's final
        /// `ResponseGenerated` (truncated to 2000 chars), or the failure error
        /// for `Failure`. Indexed by [`ThreadEvent::indexable_text`].
        summary: String,
        /// IDs of changes the child left in `pending` state. Empty for chat
        /// children and for CC children that ended without proposing anything.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pending_change_ids: Vec<String>,
    },

    /// The agent (LLM) explicitly asked to drop a prior `ToolCalled` (and its
    /// matching `ToolResult`) or `ChildThreadCompleted` from future resume
    /// context. Emitted by the `dismiss_from_context` tool handler after it
    /// validates that `dismissed_event_id` is a real, dismissible event in the
    /// same thread. The resume helper (`build_resume_tool_blocks_with_skip_ids`)
    /// honours these on every subsequent assembly.
    ContextDismissed {
        dismissed_event_id: uuid::Uuid,
    },

    /// A background Flash call produced a text description for one of the
    /// images attached to a `MessageReceived`. Emitted from the agentic loop
    /// after iteration 1 of a chat turn — one event per attached image hash,
    /// all carrying the same description text. Gated by
    /// `is_bad_image_description` so non-descriptions ("I don't see any
    /// image") never persist.
    ///
    /// Consumers (history projection, title generator) join by
    /// `source_event_id` to attach the description back to the original user
    /// message. `MessageReceived.image_description` survives as a serde
    /// fallback for legacy DB rows; new emissions never set it.
    ImageDescribed {
        /// The `MessageReceived` event id this description applies to.
        source_event_id: uuid::Uuid,
        /// Sha256 hash of the described blob (matches one entry in
        /// `MessageReceived.user_image_hashes`).
        hash: String,
        /// Flash output, post `is_bad_image_description` filter.
        description: String,
        /// The model that produced the description (e.g. `claude-haiku-4-5`,
        /// `gemini-2.5-flash`). Backfilled rows carry the literal `"backfill"`
        /// because the original model identity wasn't recorded.
        model: String,
    },

    // ---- Transient — never persisted ----
    // Every transient variant is past-tense (events-only model — no command
    // concept). Aliases preserve replay of any legacy persisted rows that
    // slipped in before the projection's transient match arm caught them.
    // Streaming state
    #[serde(alias = "TextStreaming")]
    CumulativeTextUpdated {
        text: String,
    },
    #[serde(alias = "Retrying")]
    LlmCallRetried {
        reason: String,
    },
    #[serde(alias = "PreambleCompleting")]
    PreambleCompleted,
    // Request events — trigger frontend modals/actions. Past-tense framing
    // (a request was made; the frontend chooses whether to act). The persisted
    // sibling `CredentialRequested` is a distinct audit-log entry; this
    // transient one carries the JSON payload that drives the modal. (Chat MCP
    // consent moved off this pattern to the persisted in-thread
    // `McpPermissionRequested` permission card.)
    #[serde(alias = "CredentialRequest")]
    CredentialPromptRequested {
        payload: String,
    },
    /// Plugin install request awaiting user confirmation. Carries the JSON
    /// preview emitted by `install_plugin` (manifest, file list, overwrites,
    /// optional `setup`) so the frontend can render the install panel.
    /// Resolved by `POST /api/v1/plugins/install/{install_id}/{confirm|cancel}`.
    #[serde(alias = "PluginInstallRequest")]
    PluginInstallRequested {
        payload: String,
    },
    /// Plugin uninstall request awaiting user confirmation. Carries the JSON
    /// preview emitted by `uninstall_plugin` (plugin name + version, file
    /// list partitioned into still-on-disk vs already-missing) so the
    /// frontend can render the uninstall panel. Resolved by
    /// `POST /api/v1/plugins/uninstall/{uninstall_id}/{confirm|cancel}`.
    #[serde(alias = "PluginUninstallRequest")]
    PluginUninstallRequested {
        payload: String,
    },
    #[serde(alias = "EmailConfirmRequest")]
    EmailConfirmRequested {
        payload: String,
    },
    #[serde(alias = "PushNotificationRequest")]
    PushNotificationRequested,
    #[serde(alias = "RefreshFile")]
    FileRefreshRequested {
        path: String,
    },
    #[serde(alias = "RefreshAppUI")]
    AppUiRefreshRequested {
        app_id: String,
    },
    #[serde(alias = "CaptureAppUI")]
    AppUiCaptureRequested {
        app_id: String,
        request_id: String,
    },
    NavigationRequested {
        payload: String,
    },
    #[serde(alias = "CcThreadSpawned")]
    CodingAgentThreadSpawned {
        cc_thread_id: String,
        title: String,
        #[serde(default = "default_coding_agent_claude_code", alias = "agent")]
        coding_agent: CodingAgent,
    },
    CodingAgentDiffChanged {
        has_diff: bool,
    },
    ChildrenCountChanged {
        active: i64,
        total: i64,
    },
}
