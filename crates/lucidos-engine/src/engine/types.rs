use crate::core::Step;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub use crate::core::{ConversationMessage, ConversationSnapshot, SessionMessage};

pub use crate::llm::api_usage::{ApiUsage, ModalityUsage};

pub type SharedEngine = std::sync::Arc<super::LucidosEngine>;

pub use super::thread_events::queue::{AppContext, UrlContext};
pub use super::thread_events::{ContextProducer, ContextPurpose, ContextRole, ContextSection};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatImage {
    pub base64: String,
    #[serde(default)]
    pub mime_type: String,
}

pub const MAX_IMAGE_BASE64_BYTES: usize = 4_500_000;

pub const MAX_IMAGE_DIMENSION: u32 = 2048;

const PROVIDER_MAX_IMAGE_SIDE: u32 = 8000;

impl ChatImage {
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

        let mut jpeg_buf = std::io::Cursor::new(Vec::new());
        if let Err(e) = img.write_to(&mut jpeg_buf, image::ImageFormat::Jpeg) {
            crate::log_legacy!("[Image] JPEG encoding failed: {}", e);
            return self;
        }

        let mut jpeg_bytes = jpeg_buf.into_inner();
        let mut encoded = base64::engine::general_purpose::STANDARD.encode(&jpeg_bytes);

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

    /// INVARIANT: Fit this image to what a model accepts. It runs [`compress`] when the
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
    pub response: String,
    pub steps: Vec<Step>,
    pub images: Vec<String>,
    #[serde(skip)]
    pub request_id: Uuid,
    #[serde(skip)]
    pub thread_id: Uuid,
    #[serde(skip)]
    pub proposed_change: bool,
    #[serde(skip)]
    pub auto_apply: bool,
    /// INVARIANT: Injections orphaned by the loop-exit / guard-drop race — caller
    /// must re-submit them as follow-up messages.
    #[serde(skip)]
    pub orphaned_injections: Vec<OrphanedInjection>,
}

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

    #[test]
    fn session_with_running_loop_is_live() {
        let (session, _msg_rx) = AgentSession::for_test();
        assert!(session.is_live());
    }

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

    #[test]
    fn session_with_exited_process_is_not_live() {
        let (mut session, _msg_rx) = AgentSession::for_test();
        session.process_exited = true;
        assert!(!session.is_live());
    }

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

pub struct CaptureResult {
    pub screenshot: String,
    pub dom: String,
}

pub struct AgentUserInput {
    pub text: String,
    pub images: Option<Vec<crate::engine::types::ChatImage>>,
    pub origin_event_id: Option<uuid::Uuid>,
    pub kind: AgentInputKind,
    /// INVARIANT: Answered when the turn that read this input ends. A caller that must
    /// wait for its own prompt's turn holds the receiver. A session that ends
    /// first drops the sender, so the receiver reads the session as gone.
    pub completion: Option<tokio::sync::oneshot::Sender<InputAnswered>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputAnswered {
    TurnEnded,
    Rejected,
}

impl AgentUserInput {
    pub fn continuation(text: String, continuation_requested: uuid::Uuid) -> Self {
        Self {
            text,
            images: None,
            origin_event_id: Some(continuation_requested),
            kind: AgentInputKind::Continuation,
            completion: None,
        }
    }

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentInputKind {
    User,
    Continuation,
    ReentryFromEngine,
    CeilingReentry,
    Redelivered {
        sent_at: chrono::DateTime<chrono::Utc>,
        input: RedeliveredInput,
    },
}

impl AgentInputKind {
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RedeliveredInput {
    Message {
        sender: crate::engine::thread_events::ActorMode,
    },
    ChildReport,
    EnginePrompt,
}

impl RedeliveredInput {
    /// INVARIANT: The kind of an owed input's event row. The kinds match
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

#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CcCommandsInfo {
    pub builtin_commands: Vec<String>,
    pub skill_commands: Vec<String>,
}

pub struct CcCommandsResult {
    pub info: CcCommandsInfo,
    pub has_active_session: bool,
    pub current_model: Option<String>,
    pub current_reasoning_effort: Option<String>,
}

/// INVARIANT: Why a stop signal fired. `Apply` / `Discard` / `Archive` flow through
/// `stop_agent` (the run_session loop's stop arm reads this to drive their
/// side effects; each has its own lifecycle terminator). `UserStop` does NOT
/// flow here — a real Cancel routes through `interrupt_agent` (CC's native
/// interrupt / Esc) so the session stays resumable (see `api::claude_code`).
/// The variant is retained because `interrupt_agent`'s no-live-session
/// fallback and the engine-shutdown sweep still surface a `UserStop`-shaped
/// cancel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    /// INVARIANT: User clicked the Cancel/Stop button on an in-flight CC turn — Cancel =
    /// Esc. Routed through `interrupt_agent`, NOT `stop_agent`: the turn is
    /// interrupted but the session stays resumable. The interrupted `Result`
    /// classifies as `ResponseCanceled(UserStop)` + `CodingAgentIdled`; a
    /// no-op if CC had already gone idle (race window — previous turn done).
    UserStop,
    Apply,
    Discard,
    Archive,
}

impl StopReason {
    pub fn abandons_background_tasks(self) -> bool {
        matches!(self, StopReason::Discard | StopReason::Archive)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeClaim {
    Apply,
    Discard,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictBinding {
    Detached { change_id: uuid::Uuid },
    InPlace { change_id: uuid::Uuid },
    LaneBatch { batch_id: uuid::Uuid },
}

impl ConflictBinding {
    pub fn change_id(self) -> Option<uuid::Uuid> {
        match self {
            Self::Detached { change_id } | Self::InPlace { change_id } => Some(change_id),
            Self::LaneBatch { .. } => None,
        }
    }
}

/// INVARIANT: Where a claimed run is in its life. A spawn inserts its entry `Starting`,
/// in the same `agent_sessions` lock hold that found the thread free
/// (`agent_session::spawn_claim`). So a second spawn always sees the first.
pub enum SessionPhase {
    /// INVARIANT: Worktree setup, the startup semaphore and the process spawn. A Stop
    /// cancels `startup_cancel`, and the agent never takes its first turn.
    Starting {
        startup_cancel: tokio_util::sync::CancellationToken,
    },
    /// INVARIANT: A Tier-2 apply or a late Discard holds the thread while it resets the
    /// worktree.
    /// No process exists. A spawn waits for the apply; the apply's own merge
    /// session takes the entry over as `Starting`.
    Applying,
    /// INVARIANT: The agent process runs. `control_tx` reaches its runtime.
    Running {
        control_tx: tokio::sync::mpsc::UnboundedSender<crate::runtime::ControlRequest>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct QuietPick {
    pub reply: Option<String>,
}

impl QuietPick {
    pub fn terminal_text(pick: Option<Self>, result_text: &str, turn_completed: bool) -> String {
        pick.filter(|_| turn_completed)
            .and_then(|pick| pick.reply)
            .unwrap_or_else(|| result_text.to_string())
    }
}

pub struct AgentSession {
    pub sign_in_context: Option<super::sign_in_contexts::SignInContext>,
    pub execution_sandbox: Option<crate::runtime::agent_sandbox::Sandbox>,
    pub msg_tx: tokio::sync::mpsc::UnboundedSender<AgentUserInput>,
    pub is_waiting: bool,
    /// INVARIANT: Reason the most recent `stop_agent` call fired, or `None` when the
    /// session has never been stopped (or the stop signal came from the
    /// engine-shutdown direct-notify path, which sets `shutting_down` instead).
    /// Set by `stop_agent` BEFORE `stop.notify_one()`; read by the run_session
    /// stop arm to decide whether to suppress `ResponseCanceled`, and by the
    /// post-loop cleanup to drive `Apply` / `Discard` side effects. Stays
    /// mutually exclusive by construction — three `bool` fields here used to
    /// drift apart and the `auto_apply || discard || archiving` shape was a
    /// repeated source of bugs.
    pub pending_stop: Option<StopReason>,
    /// INVARIANT: Device that clicked **Cancel** on a live (non-waiting) session, stamped
    /// by `interrupt_agent` before it fires the `interrupt` notify. The
    /// run_session interrupt arm drains it (`take_session_cancel_actor`) and
    /// merges it into the emitted `ResponseCanceled.actor` so the Initiator
    /// popover shows which device cancelled — the live-interrupt analog of the
    /// chat path's `ThreadHandle.cancel_actor` slot. `None` for engine-internal
    /// interrupts (those pre-emit their own boundary events with an explicit
    /// actor upstream). Drained on read so a stale device can't carry into the
    /// next turn on a resumed session.
    pub cancel_actor: Option<crate::engine::thread_events::MessageOrigin>,
    /// INVARIANT: Why the interrupt about to land is a redirect, not a Stop: a follow-up
    /// (`arm_followup_redirect`, `SupersededByFollowup`). The redirect analog
    /// of `cancel_actor`. The run_session interrupt arm drains it
    /// (`take_session_redirect_cause`) and feeds it to `classify_result`. The
    /// interrupted turn's `ResponseCanceled` then carries it (rendered
    /// neutrally) instead of `UserStop` (rendered "Canceled ✕"). A real Stop click via
    /// `interrupt_agent` leaves it `None`. Drained on read so a stale cause can't
    /// relabel the next turn on a resumed session.
    pub redirect_cause: Option<crate::engine::thread_events::CancelCause>,
    /// INVARIANT: Also set by `arm_followup_redirect`, and deliberately NOT the same field as
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
    /// Set when the user types a terminal suggestion into this thread's
    /// terminal. A sign-in run there reaches only a fresh subprocess, so new
    /// input goes to one.
    pub respawn_after_sign_in: bool,
    pub stop: std::sync::Arc<tokio::sync::Notify>,
    pub interrupt: std::sync::Arc<tokio::sync::Notify>,
    pub idle_notify: std::sync::Arc<tokio::sync::Notify>,
    /// INVARIANT: The operation on this session's change that holds the session: Apply
    /// Now or a Tier-1 in-place merge (`Apply`), or an in-session Discard
    /// (`Discard`). Prevents two of them
    /// from running at once, which merges twice or resets a tree under a merge.
    /// It and `pending_stop` refuse each other: see `claude_code::stop_refusal`
    /// and `decide_in_place_merge_claim`. Release it with `release_change_claim`.
    pub change_claim: Option<ChangeClaim>,
    pub conflict: Option<ConflictBinding>,
    pub process_exited: bool,
    pub worktree_path: Option<std::path::PathBuf>,
    pub branch_name: Option<String>,
    pub repo_root: Option<std::path::PathBuf>,
    pub backend_session_id: Option<String>,
    pub last_event_at: std::sync::Arc<std::sync::atomic::AtomicI64>,
    /// INVARIANT: Set during engine shutdown so CC cleanup emits SessionEnded with
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
    /// INVARIANT: Set by `abort_in_flight_for_restart` when it pre-emits a `ResponseAborted`
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
    /// INVARIANT: Set by the external watchdog (alongside `external_terminal_emitted`,
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
    pub phase: SessionPhase,
    pub withdraw_tx: tokio::sync::mpsc::UnboundedSender<WithdrawInputRequest>,
    pub child_reports: crate::engine::child_report_channel::ChildReportsHandle,
    /// INVARIANT: Follow-ups sent on `msg_tx` that the run loop has not forwarded yet,
    /// by origin event id. The chat fast path adds one before its send and the
    /// run loop removes it on receipt, both under the `agent_sessions` lock. A
    /// withdraw for an id still here waits until the message is forwarded, so
    /// it can never reach the agent ahead of it.
    pub unforwarded_inputs: std::collections::HashSet<Uuid>,
    pub builtin_commands: Vec<String>,
    pub skill_commands: Vec<String>,
    pub current_model: Option<String>,
    pub current_reasoning_effort: Option<String>,
    /// INVARIANT: The *effort in force*: the one this session's calls run at. Seeded when
    /// the process starts, and updated from each main-session call. A control
    /// request never sets it, so a terminal event never reports a pick no call
    /// ran at.
    pub reasoning_effort_in_force: Option<String>,
    /// INVARIANT: Set by `answer_pending_question` when a user answers an `AskUserQuestion`
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
    pub quiet_answer_pending: Option<QuietPick>,
    pub in_ceiling_reentry: bool,
    /// INVARIANT: Paired ToolCalled / ToolResult counter, mirrored from the local
    /// counter inside `run_session`. Read by the external watchdog
    /// (`external_watchdog::tick`) — which lives outside `run_session`'s
    /// `select!` and cannot see local variables. The in-loop watchdog still
    /// reads the local copy; both observe the same `Arc<AtomicI32>` so the
    /// value can't drift. > 0 means a tool is mid-execution (Bash, Read,
    /// AskUserQuestion, …) — legitimate silence, no fire from either
    /// watchdog.
    pub tools_in_flight: std::sync::Arc<std::sync::atomic::AtomicI32>,
    pub question_flushes: std::sync::Arc<crate::engine::question_flushes::QuestionFlushes>,
    pub coding_agent: crate::runtime::CodingAgent,
    pub can_steer: bool,
    /// INVARIANT: Clone of `run_session`'s `agent_cancel` `CancellationToken` — the lever
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
    pub keep_alive: bool,
}

impl AgentSession {
    pub fn to_commands_info(&self) -> CcCommandsInfo {
        CcCommandsInfo {
            builtin_commands: self.builtin_commands.clone(),
            skill_commands: self.skill_commands.clone(),
        }
    }

    pub fn is_in_flight(&self) -> bool {
        self.is_live() && !self.is_waiting
    }

    /// INVARIANT: True when this entry still has a running session loop behind it.
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

    pub fn is_running(&self) -> bool {
        matches!(self.phase, SessionPhase::Running { .. })
    }

    /// INVARIANT: The Stop gesture: interrupt the running turn, or cancel a spawn that
    /// has no process yet. An interrupt permit stored before the run loop
    /// exists would otherwise land on the agent's first turn.
    pub fn interrupt_or_cancel_startup(&self) {
        match &self.phase {
            SessionPhase::Starting { startup_cancel } => startup_cancel.cancel(),
            SessionPhase::Running { .. } => self.interrupt.notify_one(),
            // INVARIANT: `interrupt_route` reads an apply as having no turn to stop.
            SessionPhase::Applying => {}
        }
    }
}

#[cfg(any(test, feature = "test-support"))]
impl AgentSession {
    pub fn for_test() -> (Self, tokio::sync::mpsc::UnboundedReceiver<AgentUserInput>) {
        let (msg_tx, msg_rx) = tokio::sync::mpsc::unbounded_channel::<AgentUserInput>();
        (Self::for_test_with_sender(msg_tx), msg_rx)
    }

    pub fn for_test_with_sender(
        msg_tx: tokio::sync::mpsc::UnboundedSender<AgentUserInput>,
    ) -> Self {
        use std::sync::atomic::{AtomicBool, AtomicI32, AtomicI64};
        use std::sync::Arc;
        Self {
            execution_sandbox: None,
            sign_in_context: None,
            msg_tx,
            is_waiting: true,
            pending_stop: None,
            cancel_actor: None,
            redirect_cause: None,
            redirect_followup_pending: false,
            respawn_after_sign_in: false,
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ApplyStatus {
    Applied,
    /// INVARIANT: Branch had nothing to merge — already applied, or no commits exist.
    /// Distinct from `Applied` so callers don't infer a fresh merge happened.
    ///
    /// The default, as the only safe "no-info" value. Defaulting to `Applied`
    /// would let a forgotten field silently claim a successful merge.
    #[default]
    Noop,
    Hardening,
    Conflict,
    Queued,
}

/// INVARIANT: Result of applying a pending change. Serialized directly as the HTTP
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conflict_thread_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub review_thread_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub applied_commit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_commit: Option<String>,
    pub commits_applied: usize,
    pub files_changed: usize,
}

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

#[derive(Debug)]
pub struct WithdrawInputRequest {
    pub input_event_id: Uuid,
    pub actor: Option<crate::engine::thread_events::MessageOrigin>,
    pub purpose: WithdrawPurpose,
    pub reply: tokio::sync::oneshot::Sender<crate::runtime::InputWithdrawal>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WithdrawPurpose {
    Remove,
    TakeChildReport,
}

/// INVARIANT: A turn's exchange-starter event that the CALLER already persisted, so
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
    Message(Uuid),
    EngineReentry(Uuid),
    WaitReentry(Uuid),
    /// INVARIANT: A *cache keep-alive* (`engine::event_wait::keep_alive`), anchored to its
    /// own hidden `PromptInjected`. It only ever starts a fresh `--resume` turn:
    /// it never enters a live session, waits on a hold, or supersedes a card.
    KeepAlive(Uuid),
}

impl PreEmittedOrigin {
    pub fn event_id(self) -> Uuid {
        match self {
            PreEmittedOrigin::Message(id)
            | PreEmittedOrigin::EngineReentry(id)
            | PreEmittedOrigin::WaitReentry(id)
            | PreEmittedOrigin::KeepAlive(id) => id,
        }
    }

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FollowUpUrgency {
    #[default]
    Normal,
    Urgent,
}

impl FollowUpUrgency {
    pub fn is_urgent(self) -> bool {
        matches!(self, Self::Urgent)
    }

    pub fn from_flag(urgent: Option<bool>) -> Self {
        if urgent.unwrap_or(false) {
            Self::Urgent
        } else {
            Self::Normal
        }
    }

    /// INVARIANT: Parse the LLM tool's raw argument, which has no schema enforcement
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

pub struct SpawnAgentThreadParams {
    pub hook_args: crate::core::event_hooks::HookArgs,
    pub prompt: String,
    pub user_images: Option<Vec<crate::engine::types::ChatImage>>,
    pub device_id: Option<String>,
    pub parent_thread_id: Option<Uuid>,
    pub spawning_event_id: Option<Uuid>,
    pub wake_mode: Option<crate::engine::thread_events::WakeMode>,
    pub repo_id: Option<String>,
    pub caller_title: Option<String>,
    /// INVARIANT: When `Some`, spawn an **app coding-agent thread** instead of a
    /// Lucidos / external-repo one: sparse-checkout worktree of the
    /// workspace git narrowed to `data/apps/<id>/`. `repo_id` MUST be
    /// `None` when this is set — apps are not in the repo registry.
    pub app_id: Option<String>,
    pub coding_agent: crate::runtime::CodingAgent,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    pub origin: Option<crate::engine::thread_events::MessageOrigin>,
}
