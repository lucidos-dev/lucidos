use crate::core::Step;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// Re-exported so callers can use the bare names through `engine::*`
// (chained via `engine/mod.rs::pub use types::*`). Engine code references
// `ConversationSnapshot` etc. directly without the `core::` path.
pub use crate::core::{ConversationMessage, ConversationSnapshot, SessionMessage};

pub use crate::llm::api_usage::{ApiUsage, ModalityUsage};

pub type SharedEngine = std::sync::Arc<super::LucidosEngine>;

pub use super::thread_events::queue::{AppContext, UrlContext};
pub use super::thread_events::{ContextProducer, ContextPurpose, ContextRole, ContextSection};

/// An image as base64 bytes plus their mime type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatImage {
    pub base64: String,
    /// Optional on the wire. `chat_submit` ignores a client's value and uses
    /// the format the blob store sniffs.
    #[serde(default)]
    pub mime_type: String,
}

/// Maximum base64-encoded image size accepted by LLM APIs (Claude: 5 MB).
/// We target 4.5 MB to leave margin for encoding overhead.
pub const MAX_IMAGE_BASE64_BYTES: usize = 4_500_000;

/// Maximum dimension (width or height) for images sent to LLMs.
/// Larger images waste tokens without improving understanding.
pub const MAX_IMAGE_DIMENSION: u32 = 2048;

/// Longest image side Claude accepts. A longer side fails the whole request.
const PROVIDER_MAX_IMAGE_SIDE: u32 = 8000;

impl ChatImage {
    /// Always compress images for LLM consumption: re-encode as JPEG (quality 85)
    /// and cap dimensions at `MAX_IMAGE_DIMENSION`. This reduces token usage and
    /// ensures images stay under the API's 5 MB limit.
    pub fn compress(self) -> Self {
        use base64::Engine as _;
        use image::GenericImageView;

        let raw = match base64::engine::general_purpose::STANDARD.decode(&self.base64) {
            Ok(bytes) => bytes,
            Err(e) => {
                crate::log_legacy!("[Image] base64 decode failed, skipping compression: {}", e);
                return self;
            }
        };

        let img = match image::load_from_memory(&raw) {
            Ok(img) => img,
            Err(e) => {
                crate::log_legacy!("[Image] image decode failed, skipping compression: {}", e);
                return self;
            }
        };

        let img = crate::core::blobs::apply_exif_orientation(&raw, img);

        let (orig_w, orig_h) = img.dimensions();
        let orig_base64_len = self.base64.len();

        // Scale down if either dimension exceeds the cap
        let img = if orig_w > MAX_IMAGE_DIMENSION || orig_h > MAX_IMAGE_DIMENSION {
            img.resize(
                MAX_IMAGE_DIMENSION,
                MAX_IMAGE_DIMENSION,
                image::imageops::FilterType::Lanczos3,
            )
        } else {
            img
        };

        let (new_w, new_h) = img.dimensions();

        // Encode as JPEG — always, even if already JPEG (re-encoding normalizes quality)
        let mut jpeg_buf = std::io::Cursor::new(Vec::new());
        if let Err(e) = img.write_to(&mut jpeg_buf, image::ImageFormat::Jpeg) {
            crate::log_legacy!("[Image] JPEG encoding failed: {}", e);
            return self;
        }

        let mut jpeg_bytes = jpeg_buf.into_inner();
        let mut encoded = base64::engine::general_purpose::STANDARD.encode(&jpeg_bytes);

        // If still over the limit, progressively reduce dimensions
        let mut scale: f64 = 0.8;
        while encoded.len() > MAX_IMAGE_BASE64_BYTES && scale > 0.1 {
            let sw = ((new_w as f64) * scale).max(1.0) as u32;
            let sh = ((new_h as f64) * scale).max(1.0) as u32;
            let resized = img.resize(sw, sh, image::imageops::FilterType::Lanczos3);
            let mut buf = std::io::Cursor::new(Vec::new());
            if resized.write_to(&mut buf, image::ImageFormat::Jpeg).is_ok() {
                jpeg_bytes = buf.into_inner();
                encoded = base64::engine::general_purpose::STANDARD.encode(&jpeg_bytes);
            }
            scale *= 0.7;
        }

        crate::log_legacy!(
            "[Image] {}x{} ({} KB) → {}x{} ({} KB)",
            orig_w,
            orig_h,
            orig_base64_len / 1024,
            new_w,
            new_h,
            encoded.len() / 1024
        );

        ChatImage {
            base64: encoded,
            mime_type: "image/jpeg".to_string(),
        }
    }

    /// Fit this image to what a model accepts. It runs [`compress`] when the
    /// base64 payload is over `MAX_IMAGE_BASE64_BYTES` or either side is over
    /// `PROVIDER_MAX_IMAGE_SIDE`. Otherwise it returns the image untouched, in
    /// its original format, with no quality loss.
    ///
    /// This is the single decision point for "can this image go to a model".
    /// Every LLM-bound image path routes through it: chat message blocks, the
    /// image-description pass, `read_file` and `view_image`. A full-page
    /// screenshot is often under the byte limit but over the side limit.
    ///
    /// [`compress`]: ChatImage::compress
    pub fn fit_for_llm(self) -> Self {
        if self.base64.len() > MAX_IMAGE_BASE64_BYTES || self.exceeds_provider_max_side() {
            self.compress()
        } else {
            self
        }
    }

    /// Reads only the image header. An image it cannot read counts as within
    /// bounds, because [`ChatImage::compress`] could not decode it either.
    fn exceeds_provider_max_side(&self) -> bool {
        use base64::Engine as _;

        let Ok(raw) = base64::engine::general_purpose::STANDARD.decode(&self.base64) else {
            return false;
        };
        image::ImageReader::new(std::io::Cursor::new(raw))
            .with_guessed_format()
            .ok()
            .and_then(|reader| reader.into_dimensions().ok())
            .is_some_and(|(w, h)| w.max(h) > PROVIDER_MAX_IMAGE_SIDE)
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_quiet_reply_replaces_the_agents_result_text() {
        let pick = |reply: Option<&str>| {
            Some(QuietPick {
                reply: reply.map(str::to_string),
            })
        };
        assert_eq!(
            QuietPick::terminal_text(pick(Some("Done.")), "ok", true),
            "Done."
        );
        assert_eq!(QuietPick::terminal_text(pick(None), "ok", true), "ok");
        assert_eq!(QuietPick::terminal_text(None, "ok", true), "ok");
    }

    #[test]
    fn a_stopped_or_aborted_turn_keeps_its_own_text_over_the_quiet_reply() {
        let pick = Some(QuietPick {
            reply: Some("Done.".to_string()),
        });
        assert_eq!(QuietPick::terminal_text(pick, "partial", false), "partial");
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

/// Result of an App UI capture from the frontend
pub struct CaptureResult {
    pub screenshot: String, // base64-encoded image (JPEG, or the requested save format; sniffed downstream)
    pub dom: String,        // condensed DOM text
}

/// A user message sent to a running Claude Code session, with optional image attachments.
/// `origin_event_id` is the UUID of the already-emitted `MessageReceived` (or other
/// exchange-starter like `ChildThreadCompleted`) event — used to re-process lost
/// follow-ups without emitting a duplicate exchange boundary.
pub struct AgentUserInput {
    pub text: String,
    pub images: Option<Vec<crate::engine::types::ChatImage>>,
    /// UUID of the exchange-starter event already emitted before routing
    /// (`MessageReceived` for user follow-ups; `ChildThreadCompleted` for
    /// engine re-entry follow-ups). `None` for auto-harden and other internally
    /// generated messages where no caller-side event exists.
    pub origin_event_id: Option<uuid::Uuid>,
    /// What kind of input this is. CC's `run_session` reads this to decide
    /// whether to emit `CodingAgentPromptSent`: `User` does, `ReentryFromEngine`
    /// doesn't (the `ChildThreadCompleted` event already on the parent's
    /// history is the exchange-starter; emitting another start event would
    /// split the response into a duplicate exchange).
    pub kind: AgentInputKind,
    /// Answered when the turn that read this input ends. A caller that must
    /// wait for its own prompt's turn holds the receiver. A session that ends
    /// first drops the sender, so the receiver reads the session as gone.
    pub completion: Option<tokio::sync::oneshot::Sender<InputAnswered>>,
}

/// How an input with a [`AgentUserInput::completion`] was answered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputAnswered {
    /// The turn that read the input reached its `Result`.
    TurnEnded,
    /// The agent rejected the input, so no turn will ever read it.
    Rejected,
}

impl AgentUserInput {
    /// The Continue a live run reads as a follow-up, anchored to its
    /// `ContinuationRequested`.
    pub fn continuation(text: String, continuation_requested: uuid::Uuid) -> Self {
        Self {
            text,
            images: None,
            origin_event_id: Some(continuation_requested),
            kind: AgentInputKind::Continuation,
            completion: None,
        }
    }

    /// The event of the user message behind this input, if a user sent one.
    /// A child wake or an engine-made prompt (auto-harden, apply) has none.
    pub fn user_origin(&self) -> Option<uuid::Uuid> {
        match self.kind {
            AgentInputKind::User
            | AgentInputKind::Continuation
            | AgentInputKind::Redelivered {
                input: RedeliveredInput::Message { .. },
                ..
            } => self.origin_event_id,
            AgentInputKind::ReentryFromEngine
            | AgentInputKind::CeilingReentry
            | AgentInputKind::Redelivered { .. } => None,
        }
    }
}

/// Discriminates a user-typed follow-up from an engine-synthesized re-entry.
/// CC's `run_session` and the chat fast-paths use this to suppress duplicate
/// exchange-starter events for re-entries (`ChildThreadCompleted` is the start
/// for the child case, the delivery anchor for an *event wait*).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentInputKind {
    /// User-typed follow-up. Emit `CodingAgentPromptSent` as the audit trail.
    User,
    /// A Continue the spawn dispatcher routed into a live run (ADR 0272),
    /// anchored to its `ContinuationRequested`. Forwards and renders like
    /// `User`. If unread when the run exits, a drain preserves its instructions
    /// and anchor as engine re-entry.
    Continuation,
    /// Engine-synthesized re-entry: a completed child thread, a resolved *event
    /// wait*, a continuation rerun's note. Suppress `CodingAgentPromptSent` so
    /// the response groups under the event that already started the exchange
    /// (`ChildThreadCompleted`, the delivery anchor). Every
    /// `PreEmittedOrigin::is_engine_reentry` origin lands here, which is why the
    /// name says engine rather than child.
    ReentryFromEngine,
    /// The *ceiling re-entry* (ADR 0629). A read after Result opens an
    /// engine turn; a mid-turn read preserves the user's read and wake decisions.
    CeilingReentry,
    /// An input a session dropped before its agent read it, sent again by the
    /// next session (`agent_session::stranded_follow_ups`). A message is like
    /// `User` and the rest like `ReentryFromEngine`, each with a note that it
    /// is late, who sent it and when.
    Redelivered {
        sent_at: chrono::DateTime<chrono::Utc>,
        input: RedeliveredInput,
    },
}

impl AgentInputKind {
    /// Whether forwarding this input records a `CodingAgentPromptSent`. Any
    /// other input already renders as the event that started it, and a prompt
    /// row would show it twice.
    pub fn writes_prompt_sent(self) -> bool {
        matches!(
            self,
            Self::User
                | Self::Continuation
                | Self::Redelivered {
                    input: RedeliveredInput::Message { .. },
                    ..
                }
        )
    }
}

/// What a redelivered input was, which decides its late note and whether it
/// writes a `CodingAgentPromptSent`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RedeliveredInput {
    /// A `MessageReceived`, from the user, Lucidos or another thread.
    Message {
        sender: crate::engine::thread_events::ActorMode,
    },
    /// A child's `ChildThreadCompleted` wake or `ChildReportFiled` report.
    ChildReport,
    /// A prompt Lucidos sent: a `CodingAgentPromptSent` or a `PromptInjected`.
    EnginePrompt,
}

impl RedeliveredInput {
    /// The kind of an owed input's event row. The kinds match
    /// `stranded_follow_ups::OWED_KIND`, so a row it admits always has one.
    pub fn of_row(event_type: &str, payload: &serde_json::Value) -> Option<Self> {
        use crate::engine::thread_events::ActorMode;
        match event_type {
            "MessageReceived" => Some(Self::Message {
                sender: match payload.get("mode").and_then(|m| m.as_str()) {
                    Some("engine") => ActorMode::Engine,
                    Some("agent") => ActorMode::Agent,
                    _ => ActorMode::Human,
                },
            }),
            "ChildThreadCompleted" | "ChildReportFiled" => Some(Self::ChildReport),
            "CodingAgentPromptSent" | "PromptInjected" => Some(Self::EnginePrompt),
            _ => None,
        }
    }
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

impl StopReason {
    /// Whether the thread's work is being thrown away, so its running
    /// background tasks go too. An Apply keeps the work, and a Cancel keeps the
    /// session, so a task either of them started runs on.
    pub fn abandons_background_tasks(self) -> bool {
        matches!(self, StopReason::Discard | StopReason::Archive)
    }
}

/// Which operation holds a session's change claim (`AgentSession::change_claim`).
/// A refusal names it, so a Discard in progress never reads as an apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeClaim {
    /// Apply Now, or a Tier-1 in-place merge.
    Apply,
    /// An in-session Discard.
    Discard,
}

/// How a session carries a merge-conflict resolution. The two tiers put the
/// session in different worktrees, which the Diff button has to tell apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictBinding {
    /// Tier 2 / Tier 3: a session spawned for the merge, registered with the
    /// binding, working in the merge worktree on a temp branch.
    Detached { change_id: uuid::Uuid },
    /// Tier 1: the merge prompt went into a session that already existed. It
    /// merges in the thread's own worktree and outlives the resolution.
    InPlace { change_id: uuid::Uuid },
    /// A lane thread resolving its lane batch on the integration branch
    /// (ADR 0463). The lane holds every member's apply claim, so no member
    /// change names this session as its resolver.
    LaneBatch { batch_id: uuid::Uuid },
}

impl ConflictBinding {
    /// The one change this session resolves. `None` for a lane batch.
    pub fn change_id(self) -> Option<uuid::Uuid> {
        match self {
            Self::Detached { change_id } | Self::InPlace { change_id } => Some(change_id),
            Self::LaneBatch { .. } => None,
        }
    }
}

/// Where a claimed run is in its life. A spawn inserts its entry `Starting`,
/// in the same `agent_sessions` lock hold that found the thread free
/// (`agent_session::spawn_claim`). So a second spawn always sees the first.
pub enum SessionPhase {
    /// Worktree setup, the startup semaphore and the process spawn. A Stop
    /// cancels `startup_cancel`, and the agent never takes its first turn.
    Starting {
        startup_cancel: tokio_util::sync::CancellationToken,
    },
    /// A Tier-2 apply or a late Discard holds the thread while it resets the
    /// worktree.
    /// No process exists. A spawn waits for the apply; the apply's own merge
    /// session takes the entry over as `Starting`.
    Applying,
    /// The agent process runs. `control_tx` reaches its runtime.
    Running {
        control_tx: tokio::sync::mpsc::UnboundedSender<crate::runtime::ControlRequest>,
    },
}

/// A *quiet pick* that ends a live turn, with the *quiet reply* the turn ends
/// on when the picked option carries one.
#[derive(Clone, Debug, PartialEq)]
pub struct QuietPick {
    pub reply: Option<String>,
}

impl QuietPick {
    /// The text a turn ends on. A turn that completed ends on the pick's
    /// *quiet reply*. A stopped, aborted or failed one keeps its own text.
    pub fn terminal_text(pick: Option<Self>, result_text: &str, turn_completed: bool) -> String {
        pick.filter(|_| turn_completed)
            .and_then(|pick| pick.reply)
            .unwrap_or_else(|| result_text.to_string())
    }
}

/// State for a single active coding-agent session.
///
/// **Single agent per thread — deliberate.** The owning HashMap is keyed by
/// `Uuid` (thread_id) alone, so a thread hosts at most one live agent session.
/// This holds with both backends wired in (Claude Code + Codex): a thread's
/// backend holds until a handoff (`thread_summaries.coding_agent`, the newest
/// session's backend; `validate_thread_continuity` rejects a mismatched
/// follow-up with 409), and the session records its backend in
/// [`AgentSession::coding_agent`].
pub struct AgentSession {
    /// Captured at spawn; preference changes cannot change an existing process.
    pub execution_sandbox: Option<crate::runtime::agent_sandbox::Sandbox>,
    pub msg_tx: tokio::sync::mpsc::UnboundedSender<AgentUserInput>,
    /// The turn reached its boundary. Only the run loop writes it, because the
    /// loop keeps a local copy in step (`only_the_run_loop_writes_the_session_phase`).
    pub is_waiting: bool,
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
    /// Why the interrupt about to land is a redirect, not a Stop: a follow-up
    /// (`arm_followup_redirect`, `SupersededByFollowup`). The redirect analog
    /// of `cancel_actor`. The run_session interrupt arm drains it
    /// (`take_session_redirect_cause`) and feeds it to `classify_result`. The
    /// interrupted turn's `ResponseCanceled` then carries it (rendered
    /// neutrally) instead of `UserStop` (rendered "Canceled ✕"). A real Stop click via
    /// `interrupt_agent` leaves it `None`. Drained on read so a stale cause can't
    /// relabel the next turn on a resumed session.
    pub redirect_cause: Option<crate::engine::thread_events::CancelCause>,
    /// Also set by `arm_followup_redirect`, and deliberately NOT the same field as
    /// `redirect_cause` above: the two answer different questions and are
    /// drained by different arms. `redirect_cause` says "the interrupt about to
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
    /// every user-driven termination (Cancel, Apply, Discard, Archive). The
    /// engine shutdown sweep fires it directly: at once for a thread parked on a
    /// question, otherwise after its interrupt timeout. The stop arm reads `pending_stop` and
    /// `shutting_down` to decide what (if anything) to emit — only a real
    /// `UserStop` on an actively-working CC produces `ResponseCanceled`.
    pub stop: std::sync::Arc<tokio::sync::Notify>,
    /// Interrupt signal — sends control_request:interrupt to stop current work
    /// without killing the session (like pressing Esc in Claude Code terminal).
    pub interrupt: std::sync::Arc<tokio::sync::Notify>,
    /// Notified when the CC process enters idle/waiting state. The chat
    /// follow-up redirect waits on it for the interrupted turn's boundary.
    pub idle_notify: std::sync::Arc<tokio::sync::Notify>,
    /// The operation on this session's change that holds the session: Apply
    /// Now or a Tier-1 in-place merge (`Apply`), or an in-session Discard
    /// (`Discard`). Prevents two of them
    /// from running at once, which merges twice or resets a tree under a merge.
    /// It and `pending_stop` refuse each other: see `claude_code::stop_refusal`
    /// and `decide_in_place_merge_claim`. Release it with `release_change_claim`.
    pub change_claim: Option<ChangeClaim>,
    /// The merge-conflict resolution this session is carrying, if any. See
    /// [`ConflictBinding`] for where each variant is set.
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
    pub conflict: Option<ConflictBinding>,
    /// Set to true when the run loop decides the subprocess exits. Read through
    /// [`Self::is_live`].
    pub process_exited: bool,
    /// Path to the worktree this Claude Code session is working in.
    pub worktree_path: Option<std::path::PathBuf>,
    /// Branch name this Claude Code session is working on.
    pub branch_name: Option<String>,
    /// Root of the repo (for git operations during apply).
    pub repo_root: Option<std::path::PathBuf>,
    /// The backend's session ID, from its init event. Claude Code and Codex
    /// both write it. Used for `--resume` on follow-ups and engine restart.
    pub backend_session_id: Option<String>,
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
    /// Whether the run that claimed this thread has its agent process yet.
    /// See [`SessionPhase`].
    pub phase: SessionPhase,
    /// Withdraws for this session's run loop, which owns the input ledger.
    pub withdraw_tx: tokio::sync::mpsc::UnboundedSender<WithdrawInputRequest>,
    /// Child-report takes for the run loop, and how many reports wait unread.
    pub child_reports: crate::engine::child_report_channel::ChildReportsHandle,
    /// Follow-ups sent on `msg_tx` that the run loop has not forwarded yet,
    /// by origin event id. The chat fast path adds one before its send and the
    /// run loop removes it on receipt, both under the `agent_sessions` lock. A
    /// withdraw for an id still here waits until the message is forwarded, so
    /// it can never reach the agent ahead of it.
    pub unforwarded_inputs: std::collections::HashSet<Uuid>,
    /// Built-in CC commands (compact, clear, cost, etc.) — slash_commands minus skills.
    pub builtin_commands: Vec<String>,
    /// Skill commands (from plugins and user .claude/skills/).
    pub skill_commands: Vec<String>,
    /// Current model reported by CC (from the system init event's `model` field).
    pub current_model: Option<String>,
    /// The selected reasoning effort (low/medium/high), which the next spawn
    /// starts with. Seeded at spawn and updated by a control request, because
    /// CC's init event does not report it.
    pub current_reasoning_effort: Option<String>,
    /// The *effort in force*: the one this session's calls run at. Seeded when
    /// the process starts, and updated from each main-session call. A control
    /// request never sets it, so a terminal event never reports a pick no call
    /// ran at.
    pub reasoning_effort_in_force: Option<String>,
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
    /// The answer that woke this live turn was a *quiet pick*. The run loop
    /// takes it at the turn's `Result`: the turn ends on the pick's *quiet
    /// reply* when it has one, and an empty reply counts as finished.
    pub quiet_answer_pending: Option<QuietPick>,
    /// The session owes or has received its *ceiling re-entry*. A follow-up
    /// never interrupts it: input waits for the successor session.
    pub in_ceiling_reentry: bool,
    /// Paired ToolCalled / ToolResult counter, mirrored from the local
    /// counter inside `run_session`. Read by the external watchdog
    /// (`external_watchdog::tick`) — which lives outside `run_session`'s
    /// `select!` and cannot see local variables. The in-loop watchdog still
    /// reads the local copy; both observe the same `Arc<AtomicI32>` so the
    /// value can't drift. > 0 means a tool is mid-execution (Bash, Read,
    /// AskUserQuestion, …) — legitimate silence, no fire from either
    /// watchdog.
    pub tools_in_flight: std::sync::Arc<std::sync::atomic::AtomicI32>,
    /// The question tool uses the run loop has flushed, which a question
    /// card's gate waits on before its second look.
    pub question_flushes: std::sync::Arc<crate::engine::question_flushes::QuestionFlushes>,
    /// Backend driving this session, fixed at creation. Follow-up routing also
    /// uses the process capability negotiated at Init.
    pub coding_agent: crate::runtime::CodingAgent,
    /// Negotiated with this process; false until its Init proves support.
    pub can_steer: bool,
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
    /// A *cache keep-alive* run. Live, it counts as a hold rather than a live
    /// session: a real input waits the few seconds until it ends, and never
    /// joins the hidden turn.
    pub keep_alive: bool,
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

    pub fn is_starting(&self) -> bool {
        matches!(self.phase, SessionPhase::Starting { .. })
    }

    /// An agent process runs behind this entry, as opposed to a spawn still
    /// starting or an apply holding the thread.
    pub fn is_running(&self) -> bool {
        matches!(self.phase, SessionPhase::Running { .. })
    }

    /// The Stop gesture: interrupt the running turn, or cancel a spawn that
    /// has no process yet. An interrupt permit stored before the run loop
    /// exists would otherwise land on the agent's first turn.
    pub fn interrupt_or_cancel_startup(&self) {
        match &self.phase {
            SessionPhase::Starting { startup_cancel } => startup_cancel.cancel(),
            SessionPhase::Running { .. } => self.interrupt.notify_one(),
            // `interrupt_route` reads an apply as having no turn to stop.
            SessionPhase::Applying => {}
        }
    }
}

/// Test-only `AgentSession` builder, colocated with the type so the field list
/// lives in one place instead of being re-spelled in every test module.
///
/// Hands the receiver back deliberately: an `AgentSession` whose `msg_rx` has
/// been dropped is a phantom (see [`AgentSession::is_live`]), so a builder that
/// dropped it internally would silently hand every test a dead session. Bind it
/// (`let (session, _rx) = …`) for the lifetime of the test.
#[cfg(any(test, feature = "test-support"))]
impl AgentSession {
    pub fn for_test() -> (Self, tokio::sync::mpsc::UnboundedReceiver<AgentUserInput>) {
        let (msg_tx, msg_rx) = tokio::sync::mpsc::unbounded_channel::<AgentUserInput>();
        (Self::for_test_with_sender(msg_tx), msg_rx)
    }

    /// Same, for a test that already owns the channel pair (and therefore keeps
    /// the receiver alive itself).
    pub fn for_test_with_sender(
        msg_tx: tokio::sync::mpsc::UnboundedSender<AgentUserInput>,
    ) -> Self {
        use std::sync::atomic::{AtomicBool, AtomicI32, AtomicI64};
        use std::sync::Arc;
        Self {
            execution_sandbox: None,
            msg_tx,
            is_waiting: true,
            pending_stop: None,
            cancel_actor: None,
            redirect_cause: None,
            redirect_followup_pending: false,
            stop: Arc::new(tokio::sync::Notify::new()),
            interrupt: Arc::new(tokio::sync::Notify::new()),
            idle_notify: Arc::new(tokio::sync::Notify::new()),
            change_claim: None,
            conflict: None,
            process_exited: false,
            worktree_path: None,
            branch_name: None,
            repo_root: None,
            backend_session_id: None,
            shutting_down: Arc::new(AtomicBool::new(false)),
            external_terminal_emitted: Arc::new(AtomicBool::new(false)),
            external_continuation_requested: Arc::new(AtomicBool::new(false)),
            phase: SessionPhase::Running {
                control_tx: tokio::sync::mpsc::unbounded_channel().0,
            },
            withdraw_tx: tokio::sync::mpsc::unbounded_channel().0,
            child_reports: crate::engine::child_report_channel::child_reports_channel().0,
            unforwarded_inputs: Default::default(),
            builtin_commands: vec![],
            skill_commands: vec![],
            current_model: None,
            current_reasoning_effort: None,
            reasoning_effort_in_force: None,
            last_event_at: Arc::new(AtomicI64::new(0)),
            question_resume_pending: false,
            quiet_answer_pending: None,
            in_ceiling_reentry: false,
            tools_in_flight: Arc::new(AtomicI32::new(0)),
            question_flushes: Default::default(),
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            can_steer: true,
            agent_cancel: tokio_util::sync::CancellationToken::new(),
            keep_alive: false,
        }
    }
}

/// Outcome class of an apply attempt.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ApplyStatus {
    /// Branch merged into main (or external-repo handoff completed).
    Applied,
    /// Branch had nothing to merge — already applied, or no commits exist.
    /// Distinct from `Applied` so callers don't infer a fresh merge happened.
    ///
    /// The default, as the only safe "no-info" value. Defaulting to `Applied`
    /// would let a forgotten field silently claim a successful merge.
    #[default]
    Noop,
    /// Hardening recovery session was spawned. The change will auto-apply
    /// after hardening completes — `review_thread_id` points at it.
    Hardening,
    /// Merge conflict — a Claude Code session was spawned (`conflict_thread_id`)
    /// or an in-place merge failed and the original session stays alive.
    Conflict,
    /// The change waits in the landing lane, or resolves in a lane batch.
    /// `ChangeApplied` reports its landing, `ChangeUnqueued` its leaving.
    Queued,
}

/// Result of applying a pending change. Serialized directly as the HTTP
/// response body — see `docs/apply-change-api.md`.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ApplyResult {
    pub status: ApplyStatus,
    pub change_id: Uuid,
    pub thread_id: Option<Uuid>,
    pub restart_required: bool,
    pub version_build: VersionBuild,
    pub deferred_coding_threads: i64,
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

/// Whether applying requested a new-version build.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum VersionBuild {
    #[default]
    None,
    Started,
    Deferred,
}

fn applied_message(restart_required: bool) -> String {
    if restart_required {
        "Change applied. This change requires the engine to restart.".to_string()
    } else {
        "Change applied.".to_string()
    }
}

impl ApplyResult {
    pub(crate) fn with_version_build(
        mut self,
        request: super::build_deferral::BuildRequest,
    ) -> Self {
        use super::build_deferral::BuildRequest;
        match request {
            BuildRequest::None => {}
            BuildRequest::Started => self.version_build = VersionBuild::Started,
            BuildRequest::Deferred(count) => {
                self.version_build = VersionBuild::Deferred;
                self.deferred_coding_threads = count;
                let noun = if count == 1 { "thread" } else { "threads" };
                self.message =
                    format!("Applied. The new version waits for {count} coding {noun} to finish.");
            }
        }
        self
    }

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

    pub fn queued(change_id: Uuid, thread_id: Option<Uuid>, message: impl Into<String>) -> Self {
        Self {
            status: ApplyStatus::Queued,
            change_id,
            thread_id,
            message: message.into(),
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

/// A withdraw on its way to a session's run loop.
#[derive(Debug)]
pub struct WithdrawInputRequest {
    /// The event that carried the input: a follow-up's `MessageReceived`, or
    /// a child report's `ChildThreadCompleted`.
    pub input_event_id: Uuid,
    /// Who asked, for the tombstone.
    pub actor: Option<crate::engine::thread_events::MessageOrigin>,
    pub purpose: WithdrawPurpose,
    pub reply: tokio::sync::oneshot::Sender<crate::runtime::InputWithdrawal>,
}

/// Why the input is taken back, which decides what a success records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WithdrawPurpose {
    /// The user removed a queued message: a `QueuedMessageRemoved` tombstone.
    Remove,
    /// The agent took a child report mid-turn: a `ChildReportTaken`.
    TakeChildReport,
}

/// A turn's exchange-starter event that the CALLER already persisted, so
/// `process_message_with_steps_internal` must not emit its own
/// `MessageReceived`.
///
/// The variant identifies what to inject into a live turn. A user's message
/// uses `UserText`, emits `PromptInjected` and can interrupt when required. An
/// engine re-entry uses the silent `ReentryFromEngine` path.
///
/// An event ID alone cannot distinguish a user message from a re-entry. A
/// caller can emit the user's queued message before injecting it. It still
/// needs the usual visible acknowledgment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreEmittedOrigin {
    /// Real input from a person or an agent, whose starter event the caller
    /// persisted before dispatch: the chat API boundary's emit-before-ack, the
    /// Thread Queue executor's eager child `MessageReceived`, or a call's
    /// `WorkDelegated` (ADR 0201). Routes exactly like a message this function
    /// emitted itself, which is the point: the words are the user's, so they
    /// must not reach a live turn framed as engine text.
    Message(Uuid),
    /// An engine-internal re-entry on an existing thread, anchored to an event
    /// the UI already renders: a child thread's completion re-opening its parent
    /// (`notify_parent_of_child_completion`), a continuation rerun's note
    /// (`chat::rerun`), or the re-processing of an orphaned injection. Not
    /// something the person just typed, so it must not surface as one.
    EngineReentry(Uuid),
    /// An **event-wait re-entry** (`engine::event_wait`). Keep its own
    /// injection variant so the log attributes the wake to the subscription
    /// rather than a child.
    ///
    /// Anchored to the `PromptInjected` `emit_resolution` wrote beside the
    /// resolution, which carries the payload as prose. One shape, always: a
    /// subscription does not hold its thread's turn, so there is never a
    /// dangling tool call for the delivery to land in instead.
    WaitReentry(Uuid),
    /// A *cache keep-alive* (`engine::event_wait::keep_alive`), anchored to its
    /// own hidden `PromptInjected`. It only ever starts a fresh `--resume` turn:
    /// it never enters a live session, waits on a hold, or supersedes a card.
    KeepAlive(Uuid),
}

impl PreEmittedOrigin {
    /// The `events.id` of the already-persisted event.
    pub fn event_id(self) -> Uuid {
        match self {
            PreEmittedOrigin::Message(id)
            | PreEmittedOrigin::EngineReentry(id)
            | PreEmittedOrigin::WaitReentry(id)
            | PreEmittedOrigin::KeepAlive(id) => id,
        }
    }

    /// True for an engine-internal re-entry, false for a real message.
    pub fn is_engine_reentry(self) -> bool {
        matches!(
            self,
            PreEmittedOrigin::EngineReentry(_)
                | PreEmittedOrigin::WaitReentry(_)
                | PreEmittedOrigin::KeepAlive(_)
        )
    }

    pub fn is_keep_alive(self) -> bool {
        matches!(self, PreEmittedOrigin::KeepAlive(_))
    }

    /// How a LIVE thread injects this re-entry. `Message` is not an engine
    /// re-entry at all, so it routes as ordinary user text.
    pub fn inject_kind(self) -> crate::engine::InjectedPromptKind {
        match self {
            PreEmittedOrigin::Message(_) => crate::engine::InjectedPromptKind::UserText,
            PreEmittedOrigin::EngineReentry(_) | PreEmittedOrigin::KeepAlive(_) => {
                crate::engine::InjectedPromptKind::ReentryFromEngine
            }
            PreEmittedOrigin::WaitReentry(_) => crate::engine::InjectedPromptKind::ReentryFromWait,
        }
    }
}

/// Whether the caller is willing to end the child's current turn to be read now.
///
/// Use named states so callers can see whether the follow-up interrupts work.
/// The default, `Normal`, queues behind current work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FollowUpUrgency {
    /// Queue behind the child's current work. Claude Code and Lucidos Agent
    /// deliver the message at the next internal boundary. A long tool call can
    /// delay it until the call returns or times out.
    #[default]
    Normal,
    /// Preempt the child's in-flight work, accepting that its current turn ends
    /// as `ResponseCanceled { SupersededByFollowup }` and the follow-up runs as
    /// the next turn. For the messages that cannot wait for a tool timeout: a
    /// cancellation, a "stop, you are working from a wrong assumption".
    ///
    /// Codex app-servers with steer support preserve ordinary turns too.
    /// Older app-servers and exec retain interrupt-and-redirect. See
    /// `process_helpers::should_redirect_followup`.
    Urgent,
}

impl FollowUpUrgency {
    pub fn is_urgent(self) -> bool {
        matches!(self, Self::Urgent)
    }

    /// Parse the HTTP spelling, where serde has already rejected anything that
    /// is not a boolean. Absent means `Normal`, so every caller that predates
    /// the flag keeps its behaviour.
    pub fn from_flag(urgent: Option<bool>) -> Self {
        if urgent.unwrap_or(false) {
            Self::Urgent
        } else {
            Self::Normal
        }
    }

    /// Parse the LLM tool's raw argument, which has no schema enforcement
    /// behind it: the model hands us whatever JSON it emitted.
    ///
    /// Absent or `null` urgency defaults to `Normal`. Reject any other
    /// non-boolean input. Silently defaulting an invalid urgent request could
    /// leave the child working when the caller expects it to stop. HTTP already
    /// enforces this through serde.
    pub fn from_tool_arg(urgent: Option<&serde_json::Value>) -> Result<Self, String> {
        match urgent {
            None | Some(serde_json::Value::Null) => Ok(Self::Normal),
            Some(serde_json::Value::Bool(b)) => Ok(Self::from_flag(Some(*b))),
            Some(other) => Err(format!(
                "`urgent` must be a boolean (true or false), got {other}. It is not \
                 defaulted, because reading a malformed urgent as 'not urgent' would \
                 report the follow-up as sent while the child kept working."
            )),
        }
    }
}

/// Parameters for spawning a new CC thread.
///
/// `caller_title`: Some(non-empty) becomes the thread's name and keeps the
/// title model out. None leaves the naming to the model, and the parent's
/// sub-thread row shows the prompt's opening words until it lands.
pub struct SpawnAgentThreadParams {
    pub prompt: String,
    pub user_images: Option<Vec<crate::engine::types::ChatImage>>,
    pub device_id: Option<String>,
    pub parent_thread_id: Option<Uuid>,
    pub spawning_event_id: Option<Uuid>,
    /// The child's wake mode, beside its parent link. `None` for a top spawn.
    pub wake_mode: Option<crate::engine::thread_events::WakeMode>,
    pub repo_id: Option<String>,
    pub caller_title: Option<String>,
    /// When `Some`, spawn an **app coding-agent thread** instead of a
    /// Lucidos / external-repo one: sparse-checkout worktree of the
    /// workspace git narrowed to `data/apps/<id>/`. `repo_id` MUST be
    /// `None` when this is set — apps are not in the repo registry.
    pub app_id: Option<String>,
    /// Backend to launch for this coding-agent thread.
    pub coding_agent: crate::runtime::CodingAgent,
    /// Model the session runs on, validated against `coding_agent`'s own picker
    /// at the tool boundary. `None` inherits the backend default.
    pub model: Option<String>,
    /// Thinking budget for the session. `None` inherits the backend default.
    pub reasoning_effort: Option<String>,
    /// Who launched this thread, stamped on its first `MessageReceived` for the
    /// message route popover. Independent of `parent_thread_id` above: a
    /// `relation: "top"` spawn records its spawning thread here while carrying no
    /// callback linkage. `None` falls back to the linkage-derived origin in
    /// `synthesize_legacy_origin`.
    pub origin: Option<crate::engine::thread_events::MessageOrigin>,
}
