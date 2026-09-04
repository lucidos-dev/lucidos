//! Command-guard permission lane (ADR 0002, Phase 2) — the chat mirror of the
//! coding-agent permission model.
//!
//! When the command guard classifies a bash/python tool call as
//! [`RiskLane::IrreversibleDanger`](crate::engine::command_guard::RiskLane) on a
//! `Chat` channel, the agentic loop pauses and asks the user, exactly like the
//! coding agent's `PermissionCard`. The shared dedup / session-allow mechanism
//! lives in `engine::cc_permission` ([`PermissionState`]); the reason constants,
//! the command-flavored allow-pattern derivation, the persisted
//! `agent-allowed-commands` allowlist, the superseded / orphan-recovery sweeps,
//! and the in-process block orchestrator are here.
//!
//! Unlike the coding agent (a subprocess blocked over MCP), the Lucidos Agent
//! runs in-process: the loop blocks directly on the [`PermissionEntry`]'s
//! broadcast channel and resumes when `POST /api/v1/command-permission/consent`
//! sends the answer. Restart-safe by the same shape as the CC lane — a request
//! left unresolved across a restart is cleared by
//! [`recover_orphan_command_permission_requests`].

use std::collections::HashMap;
use std::sync::Mutex;

use serde_json::Value;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::core::grants::{self, GrantFile};
use crate::engine::cc_permission::{DedupKey, PermissionState};
use crate::engine::claude_code::AllowScope;
use crate::engine::command_guard::{
    self, GuardDecision, JudgeInput, JudgedClassification, RiskLane, SideEffectCategory,
    StaticVerdict,
};
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::git_ops::CheckpointEffects;
use crate::engine::thread_events::{EventChannel, EventMeta, MessageOrigin, ThreadEvent};
use crate::engine::LucidosEngine;
use crate::llm::tool_names as tn;

/// Reason on a resolution emitted because the user clicked Deny.
pub const DENIAL_REASON: &str = "User denied";
/// Reason on a resolution the engine emits because the user typed a new message
/// instead of answering the card.
pub const SUPERSEDED_REASON: &str = "Superseded by a new message";
/// Reason on a resolution the engine emits because the chat turn was canceled
/// (Stop button) while the card was on screen.
pub const CANCELED_REASON: &str = "Canceled by user";

// ---------------------------------------------------------------------------
// Allow-pattern derivation (reuses `AllowScope`)
// ---------------------------------------------------------------------------

/// The pattern to STORE when the user grants an "Always allow" / "Allow for
/// this thread" click, by scope. `None` for non-command tools.
///
///   * Bash `Broad`   → `Bash` (any bash command)
///   * Bash `Narrow`  → `Bash(<first-token>:*)`, e.g. `Bash(git:*)`
///   * Bash `Session` → narrow if derivable, else `Bash`
///   * Python (all scopes) → coarse `Python` (the python tool has no finer
///     sub-scope to key on)
pub fn derive_command_allow_pattern(
    tool_name: &str,
    command: &str,
    scope: AllowScope,
) -> Option<String> {
    match tool_name {
        tn::RUN_BASH | tn::RUN_BASH_BACKGROUND => match scope {
            AllowScope::Broad => Some("Bash".to_string()),
            AllowScope::Narrow => bash_narrow_pattern(command),
            AllowScope::Session => {
                bash_narrow_pattern(command).or_else(|| Some("Bash".to_string()))
            }
        },
        tn::RUN_PYTHON | tn::RUN_PYTHON_BACKGROUND => Some("Python".to_string()),
        _ => None,
    }
}

/// Whether the granted pattern set fully covers `command`, the auto-allow
/// (skip the card) check. `allowed` answers "is this exact pattern granted?"
/// over the union of the session set and the persisted allowlist.
///
/// Bash routes to [`command_guard::grant_covers_command`], the one predicate
/// the coding-agent session lane also uses, so the two cannot drift. Python
/// takes the coarse `Python` pattern: the python tool has no finer sub-scope.
pub fn command_is_allowed(tool_name: &str, command: &str, allowed: impl Fn(&str) -> bool) -> bool {
    match tool_name {
        tn::RUN_BASH | tn::RUN_BASH_BACKGROUND => {
            command_guard::grant_covers_command("Bash", command, allowed)
        }
        tn::RUN_PYTHON | tn::RUN_PYTHON_BACKGROUND => allowed("Python"),
        _ => false,
    }
}

fn bash_narrow_pattern(command: &str) -> Option<String> {
    command_guard::first_command_token(command).map(|head| format!("Bash({head}:*)"))
}

// ---------------------------------------------------------------------------
// Persisted allowlist file (`agent-allowed-commands`)
// ---------------------------------------------------------------------------

/// Record a granted "Always allow" by scope: `Session` into the in-memory
/// per-thread allow set; `Narrow` / `Broad` into the persisted allowlist file
/// ([`GrantFile::AgentCommands`]). No-op for scopes whose pattern doesn't
/// derive. Mirrors the CC consent endpoint's `record_allow_grant`.
pub fn record_command_allow_grant(
    engine: &LucidosEngine,
    thread_id: Uuid,
    tool_name: &str,
    command: &str,
    scope: AllowScope,
) {
    let Some(pattern) = derive_command_allow_pattern(tool_name, command, scope) else {
        return;
    };
    match scope {
        AllowScope::Session => {
            let mut pending = engine.pending_command_permission.lock().unwrap();
            pending.allow_session(thread_id, pattern);
        }
        AllowScope::Narrow | AllowScope::Broad => {
            if let Err(e) = grants::append(&engine.grants_dir(), GrantFile::AgentCommands, &pattern)
            {
                crate::log!(
                    "[CommandGuard] Failed to persist allow pattern {:?}: {}",
                    pattern,
                    e
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// LLM-facing refusal messages
// ---------------------------------------------------------------------------

/// The tool result fed back to the LLM when the user denies a command. Tells
/// the model not to retry and to route around the refusal — same shape as the
/// catastrophic refusal and the circuit-breaker STOP message.
pub fn denial_refusal() -> String {
    "Refused by the command guard — the user denied permission to run this command, so it was NOT \
     run. Do not retry it. Either explain what you intended and ask the user how they'd like to \
     proceed, or choose a safe alternative that has no irreversible real-world side-effect."
        .to_string()
}

/// The tool result fed back to the LLM when the chat turn was canceled while the
/// permission card was on screen. The turn ends as canceled immediately after.
fn canceled_refusal() -> String {
    "The command was not run — the request was canceled before you got permission.".to_string()
}

/// Byte cap on the command excerpt embedded in the trigger-block message. Long
/// enough to recognise which step of a multi-command script tripped the guard,
/// short enough that the failure notification stays readable.
const BLOCKED_COMMAND_EXCERPT_BYTES: usize = 400;

/// The tool result fed back to the LLM when a trigger's command is blocked
/// because the firing trigger isn't granted the command's side-effect category
/// (ADR 0002, Phase 5). The agentic loop also emits a terminal `ResponseFailed`
/// and returns `Err`, so the scheduler's failure-notification path surfaces this
/// string to the user verbatim as the notification body.
///
/// It has to name **what was tried**, because the user reads it out of context:
/// the judge's tailored summary of the side-effect (when it produced one) and an
/// excerpt of the command itself. The category reason alone is not enough for
/// [`SideEffectCategory::Other`], whose reason is the catch-all "an irreversible
/// real-world side-effect" and says nothing about which step of the trigger
/// failed.
fn trigger_block_refusal(
    category: SideEffectCategory,
    tool_name: &str,
    input: &Value,
    summary: Option<&str>,
) -> String {
    let mut msg = format!(
        "Blocked by the command guard: this trigger is not authorized to perform {reason}, so the \
         command was NOT run and the trigger failed.",
        reason = category.reason(),
    );
    if let Some(why) = summary.map(str::trim).filter(|s| !s.is_empty()) {
        msg.push_str(&format!("\n\nWhy it was gated: {why}"));
    }
    msg.push_str(&format!(
        "\n\nTo allow it, grant the \"{label}\" side-effect on the trigger (in the trigger's \
         settings) and re-run.",
        label = category.label(),
    ));
    // The verbatim command goes LAST, after the one-line summary and the remedy.
    // A push notification body is this same string, and the OS preview shows only
    // its first couple of lines: the summary and the remedy are what a glance
    // should carry, and the raw command (which can contain a token the postgres
    // scrub below does not know about) stays off the lock screen.
    if let Some(excerpt) = blocked_command_excerpt(tool_name, input) {
        let fence = code_fence_for(&excerpt);
        msg.push_str(&format!(
            "\n\nWhat it tried ({tool_name}):\n{fence}\n{excerpt}\n{fence}"
        ));
    }
    msg
}

/// A fence long enough that `excerpt` cannot break out of its code block: one
/// backtick more than the longest backtick run inside it (the CommonMark rule),
/// minimum three. The notification body is rendered as markdown, so a command
/// containing a ``` run would otherwise close the fence early and the rest of it
/// would render as markdown rather than as the code it is.
fn code_fence_for(excerpt: &str) -> String {
    let longest_run = excerpt.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    "`".repeat(longest_run.saturating_add(1).max(3))
}

/// The blocked call's command text, redacted and length-capped for embedding in
/// [`trigger_block_refusal`]. `None` when the tool carries no inspectable
/// command or the command is blank.
///
/// Redaction is [`crate::core::redact_postgres_secrets`] and nothing more, which
/// is deliberate: that is the codebase's single boundary scrub for command text,
/// already applied by `ToolCalled.args`, `CommandPermissionRequested.command` and
/// `CommandCheckpointed.command`. This very command is therefore ALREADY
/// persisted and SSE-broadcast at exactly this redaction level by the
/// `ToolCalled` event of the same tool call, so the excerpt adds no new class of
/// exposure. Widening the scrub is a codebase-wide change to that one helper, not
/// a special case here (a scrubber that only guards this one surface would read
/// as a guarantee the other three don't keep).
fn blocked_command_excerpt(tool_name: &str, input: &Value) -> Option<String> {
    let raw = command_guard::command_text(tool_name, input)?.trim();
    if raw.is_empty() {
        return None;
    }
    let redacted = crate::core::redact_postgres_secrets(raw);
    if redacted.len() <= BLOCKED_COMMAND_EXCERPT_BYTES {
        return Some(redacted);
    }
    let cut = redacted.floor_char_boundary(BLOCKED_COMMAND_EXCERPT_BYTES);
    Some(format!("{}...", &redacted[..cut]))
}

// ---------------------------------------------------------------------------
// Superseded / orphan recovery (mirror cc_permission)
// ---------------------------------------------------------------------------

/// Resolve every unresolved `CommandPermissionRequested` on `thread_id` as
/// denied, because the user typed a new message instead of clicking a button.
/// Mirrors `cc_permission::resolve_pending_permissions_as_superseded`: fans a
/// `false` to any in-process waiter and emits `CommandPermissionResolved` so the
/// card stops dangling and the thread status flips back to `running`.
pub async fn resolve_pending_command_permissions_as_superseded(
    pool: &sqlx::PgPool,
    event_bus: &EventBus,
    pending: &Mutex<PermissionState>,
    thread_id: Uuid,
    actor: Option<MessageOrigin>,
) {
    let rows: Vec<(Option<String>,)> = match sqlx::query_as(
        "SELECT e.payload->>'request_id' \
         FROM events e \
         WHERE e.event_type = 'CommandPermissionRequested' \
           AND e.thread_id = $1 \
           AND NOT EXISTS ( \
             SELECT 1 FROM events r \
             WHERE r.event_type = 'CommandPermissionResolved' \
               AND r.payload->>'request_id' = e.payload->>'request_id' \
           )",
    )
    .bind(thread_id)
    .fetch_all(pool)
    .await
    {
        Ok(r) => r,
        Err(e) => {
            crate::log!(
                "[CommandGuard] unresolved-permission query failed for {}: {}",
                thread_id,
                e
            );
            return;
        }
    };

    for (request_id,) in rows {
        let Some(request_id) = request_id.filter(|s| !s.is_empty()) else {
            continue;
        };
        // Best-effort unblock of a still-waiting loop. The std Mutex is released
        // before the `.await` below — never hold it across an await.
        {
            let mut state = pending.lock().unwrap();
            if let Some(entry) = state.take(&request_id) {
                let _ = entry.tx.send(false);
            }
        }
        emit_command_permission_resolved(
            event_bus,
            thread_id,
            request_id,
            false,
            Some(SUPERSEDED_REASON.to_string()),
            None,
            EventMeta::with_actor(actor.clone()),
            "[CommandGuard] CommandPermissionResolved (superseded)",
        )
        .await;
    }
}

/// Re-emit `CommandPermissionResolved` for every persisted
/// `CommandPermissionRequested` with no paired resolution. The in-memory
/// `pending_command_permission` is gone after a restart, so any loop that was
/// blocked on a card is dead; emitting the resolution clears the card buttons
/// and the projection flips status from `waiting_for_user_answer` to `running`
/// (the orphan running→idle reset then settles it). Mirrors
/// `agent_recovery::recover_orphan_cc_permission_requests`.
pub async fn recover_orphan_command_permission_requests(pool: &sqlx::PgPool, event_bus: &EventBus) {
    let rows: Vec<(Uuid, String)> = match sqlx::query_as(
        "SELECT e.thread_id, e.payload->>'request_id' AS request_id \
         FROM events e \
         WHERE e.event_type = 'CommandPermissionRequested' \
           AND e.thread_id IS NOT NULL \
           AND NOT EXISTS ( \
             SELECT 1 FROM events r \
             WHERE r.event_type = 'CommandPermissionResolved' \
               AND r.payload->>'request_id' = e.payload->>'request_id' \
           )",
    )
    .fetch_all(pool)
    .await
    {
        Ok(r) => r,
        Err(e) => {
            crate::log!("[Recovery] orphan command permission query failed: {}", e);
            return;
        }
    };

    if rows.is_empty() {
        return;
    }

    crate::log!(
        "[Recovery] Auto-resolving {} orphan command permission request(s) (chat turn gone after restart)",
        rows.len()
    );

    for (thread_id, request_id) in rows {
        emit_command_permission_resolved(
            event_bus,
            thread_id,
            request_id,
            false,
            Some("Chat turn ended before answering — request expired".to_string()),
            None,
            EventMeta::NONE,
            "[Recovery] CommandPermissionResolved (orphan)",
        )
        .await;
    }
}

/// Answer one command-guard permission card, whoever answered it.
///
/// **The single in-process path.** The consent endpoint and a spoken answer
/// both land here. So a card answered by voice is indistinguishable in the
/// event log from one answered on screen, bar its actor. Parity is structural
/// rather than something a test has to keep true.
///
/// Three effects, in order: fan the answer to the blocked agentic loop, record
/// any grant the scope asks for, emit the paired resolution.
///
/// `false` means no pending request carried that id, so nothing was answered.
/// It is already resolved (superseded, orphan-recovered, canceled) or was never
/// issued. The endpoint renders that as a 404.
pub async fn resolve_command_permission(
    engine: &LucidosEngine,
    request_id: String,
    allowed: bool,
    persist_scope: Option<AllowScope>,
    actor: Option<MessageOrigin>,
    log_context: &str,
) -> bool {
    let entry = {
        let mut pending = engine.pending_command_permission.lock().unwrap();
        pending.take(&request_id)
    };
    let Some(entry) = entry else {
        return false;
    };
    // Wake the blocked loop, and every deduped waiter on the same broadcast.
    let _ = entry.tx.send(allowed);

    let reason = if allowed {
        None
    } else {
        Some(DENIAL_REASON.to_string())
    };
    // A scope on a denial grants nothing: the click said no.
    let persist_scope = persist_scope.filter(|_| allowed);
    if let Some(scope) = persist_scope {
        let command =
            command_guard::command_text(&entry.tool_name, &entry.input).unwrap_or_default();
        record_command_allow_grant(engine, entry.thread_id, &entry.tool_name, command, scope);
    }
    emit_command_permission_resolved(
        &engine.event_bus,
        entry.thread_id,
        request_id,
        allowed,
        reason,
        persist_scope,
        EventMeta::with_actor(actor),
        log_context,
    )
    .await;
    true
}

/// Emit a `CommandPermissionResolved` via the bus. Shared by the consent
/// endpoint, the superseded sweep, the orphan-recovery sweep, and the cancel
/// branch of the in-process block.
#[allow(clippy::too_many_arguments)]
pub async fn emit_command_permission_resolved(
    event_bus: &EventBus,
    thread_id: Uuid,
    request_id: String,
    allowed: bool,
    reason: Option<String>,
    persist_scope: Option<AllowScope>,
    meta: EventMeta,
    log_context: &str,
) {
    event_bus
        .emit_or_log(
            BusEvent::Thread {
                thread_id,
                event: ThreadEvent::CommandPermissionResolved {
                    request_id,
                    allowed,
                    reason,
                    persist_scope,
                },
                meta,
            },
            log_context,
        )
        .await;
}

// ---------------------------------------------------------------------------
// In-process block orchestrator
// ---------------------------------------------------------------------------

/// Per-response judge configuration + verdict cache, built once by the agentic
/// loop and threaded through each bash/python tool call. The cache memoizes
/// judge verdicts by command (ADR 0002's "cache by command hash for the turn"),
/// so a re-emitted identical command within one response doesn't re-pay the
/// LLM call. Loop-local — dropped at the end of the response, no GC needed.
pub(crate) struct CommandGuardCtx<'a> {
    /// Master `command_guard` toggle. `false` → the guard is a no-op.
    pub enabled: bool,
    /// `command_guard_judge` sub-toggle. `false` → the ambiguous middle uses the
    /// static fallback list instead of the LLM judge.
    pub judge_enabled: bool,
    /// The model the judge runs on (`model_command_judge`).
    pub judge_model: &'a str,
    /// Turn-scoped cache: `JudgeInput::cache_key()` → resolved classification.
    pub judge_cache: &'a mut HashMap<String, JudgedClassification>,
    /// The firing trigger's declared **side-effect grant** (ADR 0002, Phase 5).
    /// Empty for chat turns (the ask lane never consults it) and for triggers
    /// that granted nothing. On a trigger, an `IrreversibleDanger` command runs
    /// only if its [`SideEffectCategory`] is in this set; otherwise the trigger
    /// fails.
    pub trigger_grant: &'a [SideEffectCategory],
}

/// What the guard does once the final [`RiskLane`] is resolved. Pure mapping —
/// unit-tested below; the interactive `Ask` block and the `Checkpoint` snapshot
/// are the only parts needing engine state.
#[derive(Debug, PartialEq, Eq)]
enum GuardAction {
    /// Run the command immediately (`Safe`).
    Proceed,
    /// In-workspace destruction (`ReversibleDanger`): snapshot the workspace on
    /// a safety ref + emit `CommandCheckpointed`, then run (ADR 0002, Phase 4).
    Checkpoint,
    /// Catastrophic — hard-block, feed the reason back to the LLM.
    Refuse,
    /// `IrreversibleDanger` on a chat channel — pause and ask the user.
    Ask,
    /// `IrreversibleDanger` on a trigger whose grant doesn't cover the command's
    /// [`SideEffectCategory`] — block and fail the trigger (ADR 0002, Phase 5).
    FailTrigger(SideEffectCategory),
}

/// Map a resolved lane (+ its side-effect category) + channel + trigger grant to
/// the guard's action. See [`LucidosEngine::command_guard_decision`] for the
/// channel-gate rationale (chat turns carry `channel == None`; only triggers
/// carry `Some(Trigger)`).
///
/// `category` is `Some` only for `IrreversibleDanger`; `grant` is the firing
/// trigger's declared side-effect grant (empty for chat).
fn action_for_lane(
    lane: RiskLane,
    channel: Option<EventChannel>,
    category: Option<SideEffectCategory>,
    grant: &[SideEffectCategory],
) -> GuardAction {
    match lane {
        RiskLane::Safe => GuardAction::Proceed,
        // Phase 4: in-workspace destruction is recoverable — snapshot first,
        // then run. Same on chat and triggers (the Undo affordance is available
        // whenever the user views the thread).
        RiskLane::ReversibleDanger => GuardAction::Checkpoint,
        RiskLane::Catastrophic => GuardAction::Refuse,
        RiskLane::IrreversibleDanger => {
            if channel == Some(EventChannel::Trigger) {
                // Unattended trigger: no human to ask. Run only if the command's
                // side-effect category is in the trigger's grant; else fail the
                // trigger (Phase 5). An irreversible command always has a
                // category — default `Other` if the judge didn't tag one.
                let cat = category.unwrap_or(SideEffectCategory::Other);
                if grant.contains(&cat) {
                    GuardAction::Proceed
                } else {
                    GuardAction::FailTrigger(cat)
                }
            } else {
                GuardAction::Ask
            }
        }
    }
}

/// Why a checkpoint's two refs are being dropped without ever showing a card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DiscardReason {
    /// The pre and post images are identical, so the command changed nothing
    /// git-visible. The usual cause is destruction inside a gitignored path
    /// (`.lucidos/`, `data/blobs/`), which `git add -A` never captured: undo
    /// could neither restore it nor find anything to remove.
    NothingCaptured,
    /// The post image could not be written or diffed, so there is no way to say
    /// what the command did.
    PostImageFailed,
}

impl DiscardReason {
    fn explain(self) -> &'static str {
        match self {
            Self::NothingCaptured => {
                "the command changed nothing git-visible (its target is gitignored, \
                 or it destroyed nothing)"
            }
            Self::PostImageFailed => "no post image, so what it changed is unknowable",
        }
    }
}

/// What [`LucidosEngine::finalize_command_checkpoint`] does once the post image
/// has been attempted.
///
/// Split out as a pure decision because the three no-card paths are the whole
/// point of the 2026-08-06 change and each is a different judgment. Only the
/// emit and the ref delete need an engine; deciding between them does not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CheckpointOutcome {
    /// The command changed something git-visible: show the card, with what
    /// Undo would put back and what it would remove.
    Card { restores: u32, removes: u32 },
    /// Nothing worth keeping. Drop both refs and emit no event.
    Discard(DiscardReason),
    /// The refs are fine but the diff could not be read. Keep them and show no
    /// card: the pre image is still a usable restore point, and the retention
    /// sweep reclaims it in its own time.
    KeepRefsNoCard,
}

/// Map the post-image attempt onto what to do about it.
///
/// The `Ok(None)` arm is the subtle one. `diff_checkpoint_effects` returns it
/// for a missing ref, but both refs were written moments ago here, so at this
/// call site it can only mean a ref probe that could not run. That is a reason
/// to say nothing, never a reason to delete the one snapshot standing between
/// the user and an unrecoverable command.
fn checkpoint_outcome(effects: Result<Option<CheckpointEffects>, String>) -> CheckpointOutcome {
    match effects {
        Err(_) => CheckpointOutcome::Discard(DiscardReason::PostImageFailed),
        Ok(None) => CheckpointOutcome::KeepRefsNoCard,
        Ok(Some(e)) if e.is_empty() => CheckpointOutcome::Discard(DiscardReason::NothingCaptured),
        Ok(Some(e)) => CheckpointOutcome::Card {
            restores: e.restores,
            removes: e.removes(),
        },
    }
}

impl LucidosEngine {
    /// The command guard's pre-dispatch decision for one bash/python tool call
    /// (ADR 0002). Always `Proceed` when the guard is off (`ctx.enabled` false),
    /// so the agentic loop can gate the whole feature on the `command_guard`
    /// preference without branching itself.
    ///
    /// Resolves the final [`RiskLane`] via the static fast-path and — for the
    /// ambiguous middle on a chat channel — the LLM judge (or the static
    /// fallback when the judge is off/unavailable), then:
    ///
    /// - `Safe` / `ReversibleDanger` → `Proceed`
    /// - `Catastrophic` → `Refuse` (deterministic hard-block)
    /// - `IrreversibleDanger` on the chat lane → pause and ask (this method
    ///   blocks until the user answers, the turn is canceled, or a restart
    ///   sweep resolves it)
    /// - `IrreversibleDanger` on a trigger → run if the command's side-effect
    ///   [`SideEffectCategory`] is in the trigger's grant (`ctx.trigger_grant`),
    ///   else `FailTrigger` (block + fail the run). Triggers fire unattended, so
    ///   a runtime prompt would deadlock them — the grant is pre-authorized at
    ///   trigger-creation time (ADR 0002, Phase 5).
    ///
    /// The agentic loop serves only the Lucidos Agent (chat + triggers), so
    /// `meta.channel` is either `Some(Trigger)` (a scheduled trigger) or `None`
    /// (an ordinary chat turn — the loop does not stamp `Chat` on its per-turn
    /// meta; only specific events like `MessageReceived` carry it). We therefore
    /// gate on "is this a trigger?" rather than "is this chat?", so the common
    /// `None` chat case correctly reaches the ask lane.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn command_guard_decision(
        &self,
        ctx: &mut CommandGuardCtx<'_>,
        tool_name: &str,
        input: &Value,
        thread_id: Uuid,
        tool_use_id: &str,
        meta: &EventMeta,
        cancel_token: &CancellationToken,
    ) -> GuardDecision {
        if !ctx.enabled {
            return GuardDecision::Proceed;
        }
        // `None` → the turn was canceled while the judge was running.
        let Some(JudgedClassification {
            lane,
            summary,
            category,
        }) = self
            .resolve_command_lane(ctx, tool_name, input, cancel_token)
            .await
        else {
            return GuardDecision::Refuse(canceled_refusal());
        };

        match action_for_lane(lane, meta.channel, category, ctx.trigger_grant) {
            GuardAction::Proceed => GuardDecision::Proceed,
            GuardAction::Checkpoint => {
                // Snapshot the workspace before the in-workspace destruction so
                // the user can one-click Undo. A failed snapshot logs and still
                // proceeds (unguarded), the pre-Phase-4 behavior, rather than
                // block a legitimate cleanup on a git hiccup.
                match self
                    .checkpoint_before_reversible_command(tool_name, input, summary.as_deref())
                    .await
                {
                    Some(pending) => GuardDecision::ProceedCheckpointed(pending),
                    None => GuardDecision::Proceed,
                }
            }
            GuardAction::Refuse => {
                crate::log!(
                    "[CommandGuard] Refused catastrophic command from {}",
                    tool_name
                );
                GuardDecision::Refuse(command_guard::catastrophic_refusal(tool_name, input))
            }
            GuardAction::Ask => {
                self.ask_command_permission(
                    tool_name,
                    input,
                    thread_id,
                    tool_use_id,
                    meta,
                    cancel_token,
                    summary.as_deref(),
                )
                .await
            }
            GuardAction::FailTrigger(cat) => {
                crate::log!(
                    "[CommandGuard] Trigger blocked — {} not in side-effect grant ({})",
                    cat.reason(),
                    tool_name
                );
                GuardDecision::FailTrigger(trigger_block_refusal(
                    cat,
                    tool_name,
                    input,
                    summary.as_deref(),
                ))
            }
        }
    }

    /// Take the **pre** image before a `ReversibleDanger` command runs (ADR
    /// 0002, Phase 4). Nothing is emitted here: what the command turns out to
    /// change is what decides whether a card is worth showing, and that is only
    /// knowable afterwards (`finalize_command_checkpoint`).
    ///
    /// Best-effort: a failed snapshot logs and returns `None`, and the command
    /// still runs. In-workspace destruction was recoverable-in-principle before
    /// Phase 4 too, so a git hiccup must not block a legitimate cleanup. With no
    /// `CommandCheckpointed` event the UI simply shows no Undo affordance (never
    /// a button that can't actually restore).
    async fn checkpoint_before_reversible_command(
        &self,
        tool_name: &str,
        input: &Value,
        summary_override: Option<&str>,
    ) -> Option<command_guard::PendingCheckpoint> {
        let command = command_guard::command_text(tool_name, input).unwrap_or_default();
        let checkpoint_id = Uuid::new_v4().to_string();
        if let Err(e) =
            crate::engine::git_ops::create_command_checkpoint(self.workspace_path(), &checkpoint_id)
                .await
        {
            crate::log!(
                "[CommandGuard] checkpoint failed ({}); running the command without an undo point",
                e
            );
            return None;
        }
        Some(command_guard::PendingCheckpoint {
            checkpoint_id,
            // Same postgres-URL redaction the agentic loop applies to
            // ToolCalled.args before the command text is persisted and
            // SSE-broadcast.
            command: crate::core::redact_postgres_secrets(command),
            summary: summary_override.map(str::to_string).unwrap_or_else(|| {
                "Deletes or overwrites files inside the workspace (recoverable).".to_string()
            }),
        })
    }

    /// Close the checkpoint bracket once the command has returned: write the
    /// **post** image, diff it against the pre image, and emit
    /// `CommandCheckpointed` only if the command actually changed something
    /// git-visible.
    ///
    /// The empty case is the one this exists for. A command whose destruction
    /// landed entirely in a gitignored path (`.lucidos/`, `data/blobs/`) leaves
    /// the two images identical, because `git add -A` never captured it. Before
    /// 2026-08-06 that still produced a card, whose Undo restored nothing,
    /// removed nothing, and then reported "Reverted". Now it produces no card
    /// and both refs are dropped.
    ///
    /// Best-effort throughout, on the same reasoning as the pre image: a git
    /// failure here costs the undo affordance, never the command's result.
    pub(crate) async fn finalize_command_checkpoint(
        &self,
        pending: command_guard::PendingCheckpoint,
        thread_id: Uuid,
        meta: &EventMeta,
    ) {
        let workspace = self.workspace_path();
        let id = &pending.checkpoint_id;
        let effects = match crate::engine::git_ops::create_command_post_image(workspace, id).await {
            Ok(()) => crate::engine::git_ops::diff_checkpoint_effects(workspace, id).await,
            Err(e) => Err(e),
        };
        if let Err(e) = &effects {
            crate::log!("[CommandGuard] checkpoint {} post image failed: {}", id, e);
        }
        match checkpoint_outcome(effects) {
            CheckpointOutcome::Discard(reason) => {
                crate::log!(
                    "[CommandGuard] checkpoint {} dropped: {}",
                    id,
                    reason.explain()
                );
                crate::engine::git_ops::delete_command_checkpoint_pair(workspace, id).await;
            }
            CheckpointOutcome::KeepRefsNoCard => crate::log!(
                "[CommandGuard] checkpoint {} effects unavailable; refs kept, no card shown",
                id
            ),
            CheckpointOutcome::Card { restores, removes } => {
                self.event_bus
                    .emit_or_log(
                        BusEvent::Thread {
                            thread_id,
                            event: ThreadEvent::CommandCheckpointed {
                                checkpoint_id: pending.checkpoint_id.clone(),
                                command: pending.command,
                                summary: pending.summary,
                                restores,
                                removes,
                            },
                            meta: meta.clone(),
                        },
                        "[CommandGuard] CommandCheckpointed",
                    )
                    .await;
            }
        }
    }

    /// Undo a command checkpoint (ADR 0002, Phase 4 and the 2026-08-06
    /// addendum): restore the workspace working tree from the pre image, remove
    /// the files the command created, and emit `CommandCheckpointReverted`. The
    /// originating thread is resolved from the `CommandCheckpointed` event (so
    /// the revert lands on the right thread).
    ///
    /// Idempotent: a checkpoint already reverted is an error-free no-op on the
    /// duplicate, an `Err` only on a genuinely unknown id or a failed git
    /// restore. The guard for that is the persisted `CommandCheckpointReverted`
    /// event rather than the absence of the ref, which is what lets the refs
    /// survive the undo for the card's diff viewer to read.
    pub async fn undo_command_checkpoint(
        &self,
        checkpoint_id: &str,
        actor: Option<MessageOrigin>,
    ) -> Result<(), String> {
        // Resolve the thread the checkpoint belongs to (also confirms it exists)
        // plus the originating turn's request_event_id, so the revert event
        // groups into the same exchange as the `CommandCheckpointed` card and the
        // frontend can render it as reverted (live and on reload).
        let row: Option<(Option<Uuid>, Option<String>)> = sqlx::query_as(
            "SELECT thread_id, payload->>'request_event_id' FROM events \
             WHERE event_type = 'CommandCheckpointed' \
               AND payload->>'checkpoint_id' = $1 \
             LIMIT 1",
        )
        .bind(checkpoint_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| format!("checkpoint lookup failed: {e}"))?;
        let Some((Some(thread_id), request_event_str)) = row else {
            return Err(format!("no command checkpoint with id {checkpoint_id}"));
        };
        let request_event_id = request_event_str.and_then(|s| Uuid::parse_str(&s).ok());

        // Already reverted → idempotent no-op (the ref was deleted on the first
        // undo; restoring again would error on the missing ref).
        let already: Option<Uuid> = sqlx::query_scalar(
            "SELECT thread_id FROM events \
             WHERE event_type = 'CommandCheckpointReverted' \
               AND payload->>'checkpoint_id' = $1 \
             LIMIT 1",
        )
        .bind(checkpoint_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| format!("revert lookup failed: {e}"))?
        .flatten();
        if already.is_some() {
            return Ok(());
        }

        let workspace = self.workspace_path();
        // Put back what the command deleted or overwrote …
        crate::engine::git_ops::restore_command_checkpoint(workspace, checkpoint_id).await?;
        // … then drop what it created. `Ok(None)` is a checkpoint with no post
        // image (written before 2026-08-06, reclaimed by the retention sweep, or
        // orphaned by a crash), which degrades to the restore-only behaviour
        // those checkpoints were taken under. A diff error does the same rather
        // than fail an undo whose restore half already landed.
        match crate::engine::git_ops::diff_checkpoint_effects(workspace, checkpoint_id).await {
            Ok(Some(effects)) => {
                let removed = crate::engine::git_ops::remove_created_files(
                    workspace,
                    checkpoint_id,
                    &effects.created,
                )
                .await;
                crate::log!(
                    "[CommandGuard] undo {}: restored {} file(s), removed {} of {} created",
                    checkpoint_id,
                    effects.restores,
                    removed,
                    effects.removes()
                );
            }
            Ok(None) => crate::log!(
                "[CommandGuard] undo {}: restore only (no post image for this checkpoint)",
                checkpoint_id
            ),
            Err(e) => crate::log!(
                "[CommandGuard] undo {}: restored, but the created-file diff failed ({})",
                checkpoint_id,
                e
            ),
        }
        // The refs deliberately survive: they are what the card's diff viewer
        // reads, and a reverted card must still be able to show what happened.
        // `prune_expired_checkpoints` reclaims them once they age out.

        self.event_bus
            .emit_or_log(
                BusEvent::Thread {
                    thread_id,
                    event: ThreadEvent::CommandCheckpointReverted {
                        checkpoint_id: checkpoint_id.to_string(),
                    },
                    // Carry the original turn's request_event_id so the revert
                    // groups into the same exchange as the checkpoint card.
                    meta: EventMeta {
                        request_event_id,
                        actor,
                        ..EventMeta::NONE
                    },
                },
                "[CommandGuard] CommandCheckpointReverted",
            )
            .await;
        Ok(())
    }

    /// Resolve the final [`RiskLane`] (and its side-effect category) for one
    /// tool call: the static fast-path, then — for the ambiguous middle — the
    /// judge or its static fallback. Returns `None` only when the turn is
    /// canceled while the judge is running — the loop then ends as canceled.
    ///
    /// Since Phase 5 triggers DO reach the judge: the trigger side-effect grant
    /// can only be enforced once the command's lane + category are known, so the
    /// ambiguous middle is classified on the trigger channel too (it was skipped
    /// in Phase 3, when triggers ran everything ambiguous).
    ///
    /// It therefore takes no channel: both channels classify the middle the
    /// same way. Add one back when a routing rule needs it.
    async fn resolve_command_lane(
        &self,
        ctx: &mut CommandGuardCtx<'_>,
        tool_name: &str,
        input: &Value,
        cancel_token: &CancellationToken,
    ) -> Option<JudgedClassification> {
        match command_guard::static_classify(tool_name, input) {
            // Settled lanes are only ever `Safe` / `Catastrophic` — neither
            // carries a side-effect category.
            StaticVerdict::Settled(lane) => Some(JudgedClassification {
                lane,
                summary: None,
                category: None,
            }),
            StaticVerdict::NeedsJudge(ji) => {
                tokio::select! {
                    biased;
                    _ = cancel_token.cancelled() => None,
                    resolved = self.judge_or_fallback(ctx, ji) => Some(resolved),
                }
            }
        }
    }

    /// Resolve one ambiguous command via the judge (or the static fallback when
    /// the judge is off / unavailable), memoized in the per-turn cache.
    async fn judge_or_fallback(
        &self,
        ctx: &mut CommandGuardCtx<'_>,
        ji: JudgeInput,
    ) -> JudgedClassification {
        let key = ji.cache_key();
        if let Some(hit) = ctx.judge_cache.get(&key) {
            return hit.clone();
        }
        let resolved = if ctx.judge_enabled {
            match self.judge_command(ctx.judge_model, &ji).await {
                Ok(verdict) => JudgedClassification {
                    lane: verdict.lane,
                    summary: Some(verdict.summary),
                    category: verdict.category,
                },
                Err(e) => {
                    crate::log!(
                        "[CommandGuard] judge failed ({}); falling back to the static classifier",
                        e
                    );
                    command_guard::fallback_classify(&ji)
                }
            }
        } else {
            command_guard::fallback_classify(&ji)
        };
        ctx.judge_cache.insert(key, resolved.clone());
        resolved
    }

    /// Emit a `CommandPermissionRequested`, block the loop until the user
    /// resolves it, and translate the answer into a [`GuardDecision`]. Auto-
    /// allows (no card) when a session or persisted allowlist pattern already
    /// covers the command. Mirrors `api::internal::permission_prompt`, but the
    /// waiter is this in-process loop rather than an MCP HTTP handler.
    ///
    /// `summary_override` is the judge's tailored card text when it produced one;
    /// `None` falls back to the static [`command_guard::permission_summary`].
    #[allow(clippy::too_many_arguments)]
    async fn ask_command_permission(
        &self,
        tool_name: &str,
        input: &Value,
        thread_id: Uuid,
        tool_use_id: &str,
        meta: &EventMeta,
        cancel_token: &CancellationToken,
        summary_override: Option<&str>,
    ) -> GuardDecision {
        let command = command_guard::command_text(tool_name, input)
            .unwrap_or_default()
            .to_string();

        // Auto-allow: prior "Allow for this thread" (session) and persisted
        // "Always allow" (allowlist file) grants are unioned, and EVERY segment
        // head of the command must be covered (see [`command_is_allowed`]) —
        // a dangerous trailing segment must not ride into a grant that only
        // names the harmless leading one.
        let session_patterns: std::collections::HashSet<String> = {
            let pending = self.pending_command_permission.lock().unwrap();
            pending
                .session_allows
                .get(&thread_id)
                .cloned()
                .unwrap_or_default()
        };
        let persisted = grants::patterns(&self.grants_dir(), GrantFile::AgentCommands);
        if command_is_allowed(tool_name, &command, |p| {
            session_patterns.contains(p) || persisted.iter().any(|x| x == p)
        }) {
            return GuardDecision::Proceed;
        }

        // Register (deduping identical concurrent requests) and emit the card.
        let canonical_input = serde_json::to_string(input).unwrap_or_else(|_| "{}".to_string());
        let dedup_key: DedupKey = (thread_id, tool_name.to_string(), canonical_input);
        let (request_id, mut rx, is_canonical) = {
            let mut pending = self.pending_command_permission.lock().unwrap();
            pending.register_or_attach(dedup_key, thread_id, tool_name.to_string(), input.clone())
        };

        if is_canonical {
            let summary = summary_override
                .map(str::to_string)
                .unwrap_or_else(|| command_guard::permission_summary(tool_name, input));
            // Scrub a hardcoded `postgres://user:pass@…` URL out of the command
            // text before it's persisted + SSE-broadcast — the same redaction
            // the agentic loop applies to `ToolCalled.args`. The `summary` is a
            // fixed risk phrase with no command text, so it needs no scrub.
            let command_for_event = crate::core::redact_postgres_secrets(&command);
            self.event_bus
                .emit_or_log(
                    BusEvent::Thread {
                        thread_id,
                        event: ThreadEvent::CommandPermissionRequested {
                            request_id: request_id.clone(),
                            tool_use_id: tool_use_id.to_string(),
                            tool_name: tool_name.to_string(),
                            command: command_for_event,
                            summary,
                        },
                        meta: meta.clone(),
                    },
                    "[CommandGuard] CommandPermissionRequested",
                )
                .await;
        }

        // Block until the user answers (no timeout — the user is the rate-
        // limiter, same as CC) or the turn is canceled. The paired
        // `CommandPermissionResolved` for an Allow/Deny click is emitted by the
        // consent endpoint; only the cancel branch resolves it here.
        let allowed = tokio::select! {
            biased;
            _ = cancel_token.cancelled() => {
                if is_canonical {
                    let had_entry = {
                        let mut pending = self.pending_command_permission.lock().unwrap();
                        pending.take(&request_id).is_some()
                    };
                    if had_entry {
                        emit_command_permission_resolved(
                            &self.event_bus,
                            thread_id,
                            request_id,
                            false,
                            Some(CANCELED_REASON.to_string()),
                            None,
                            meta.clone(),
                            "[CommandGuard] CommandPermissionResolved (canceled)",
                        )
                        .await;
                    }
                }
                return GuardDecision::Refuse(canceled_refusal());
            }
            res = rx.recv() => res.unwrap_or(false),
        };

        if allowed {
            GuardDecision::Proceed
        } else {
            GuardDecision::Refuse(denial_refusal())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    /// The reported bug, at the decision layer. A `run_python` step whose only
    /// destruction was an `rmtree` of a gitignored staging directory leaves the
    /// two images identical. Before 2026-08-06 that still drew a card, whose
    /// Undo restored nothing, removed nothing, and then said "Reverted".
    #[test]
    fn a_command_that_captured_nothing_gets_no_card_and_loses_its_refs() {
        assert_eq!(
            checkpoint_outcome(Ok(Some(CheckpointEffects::default()))),
            CheckpointOutcome::Discard(DiscardReason::NothingCaptured)
        );
    }

    #[test]
    fn a_command_that_changed_something_gets_a_card_carrying_both_counts() {
        let effects = CheckpointEffects {
            restores: 3,
            created: vec!["data/artifacts/out.zip".to_string()],
        };
        assert_eq!(
            checkpoint_outcome(Ok(Some(effects))),
            CheckpointOutcome::Card {
                restores: 3,
                removes: 1
            }
        );
    }

    /// A destruction with nothing created still earns a card: restoring is the
    /// original point of the lane.
    #[test]
    fn a_pure_destruction_still_gets_a_card() {
        let effects = CheckpointEffects {
            restores: 2,
            created: vec![],
        };
        assert_eq!(
            checkpoint_outcome(Ok(Some(effects))),
            CheckpointOutcome::Card {
                restores: 2,
                removes: 0
            }
        );
    }

    /// A failed post image means we cannot say what the command did, so there
    /// is nothing a card could honestly offer.
    #[test]
    fn a_failed_post_image_drops_the_pair() {
        assert_eq!(
            checkpoint_outcome(Err("git write-tree exploded".to_string())),
            CheckpointOutcome::Discard(DiscardReason::PostImageFailed)
        );
    }

    /// Both refs were written moments before this call, so `Ok(None)` here is a
    /// probe that could not run, not a missing pair. It must NOT be read as a
    /// licence to delete the pre image: that snapshot is the only thing
    /// standing between the user and an unrecoverable command.
    #[test]
    fn an_unreadable_diff_keeps_the_refs_rather_than_dropping_them() {
        assert_eq!(
            checkpoint_outcome(Ok(None)),
            CheckpointOutcome::KeepRefsNoCard
        );
    }

    #[test]
    fn derive_pattern_bash_scopes() {
        let cmd = "git push origin main";
        assert_eq!(
            derive_command_allow_pattern(tn::RUN_BASH, cmd, AllowScope::Broad),
            Some("Bash".to_string())
        );
        assert_eq!(
            derive_command_allow_pattern(tn::RUN_BASH, cmd, AllowScope::Narrow),
            Some("Bash(git:*)".to_string())
        );
        assert_eq!(
            derive_command_allow_pattern(tn::RUN_BASH, cmd, AllowScope::Session),
            Some("Bash(git:*)".to_string())
        );
    }

    #[test]
    fn derive_pattern_strips_privilege_prefix_and_path() {
        // sudo + absolute path → the real command head.
        assert_eq!(
            derive_command_allow_pattern(
                tn::RUN_BASH,
                "sudo /usr/bin/aws s3 rm x",
                AllowScope::Narrow
            ),
            Some("Bash(aws:*)".to_string())
        );
    }

    #[test]
    fn derive_pattern_python_is_coarse_for_all_scopes() {
        for scope in [AllowScope::Broad, AllowScope::Narrow, AllowScope::Session] {
            assert_eq!(
                derive_command_allow_pattern(tn::RUN_PYTHON, "requests.post(u)", scope),
                Some("Python".to_string())
            );
        }
    }

    #[test]
    fn derive_pattern_non_command_tool_is_none() {
        assert_eq!(
            derive_command_allow_pattern("read_file", "x", AllowScope::Broad),
            None
        );
    }

    fn allowed_in<'a>(grants: &'a [&'a str]) -> impl Fn(&str) -> bool + 'a {
        move |p| grants.contains(&p)
    }

    #[test]
    fn stored_narrow_grant_covers_same_head() {
        // A narrow grant stored from one command auto-allows the same head.
        let stored =
            derive_command_allow_pattern(tn::RUN_BASH, "git push", AllowScope::Narrow).unwrap();
        assert!(command_is_allowed(
            tn::RUN_BASH,
            "git pull --rebase",
            allowed_in(&[stored.as_str()])
        ));
        // A broad grant covers any bash command.
        assert!(command_is_allowed(
            tn::RUN_BASH,
            "git pull",
            allowed_in(&["Bash"])
        ));
    }

    /// The head walk skips a `VAR=value` preamble, so a narrow grant on the head
    /// would otherwise auto-allow arbitrary loaded code with no card. The Safe
    /// fast path already refuses these; this is the same refusal on the grant
    /// lane.
    #[test]
    fn a_narrow_grant_never_covers_a_code_injecting_env_preamble() {
        for cmd in [
            "LD_PRELOAD=/tmp/evil.so ls",
            "PATH=data/bin ls",
            "ls && LD_PRELOAD=/tmp/evil.so ls",
        ] {
            assert!(
                !command_is_allowed(tn::RUN_BASH, cmd, allowed_in(&["Bash(ls:*)"])),
                "{cmd} must not be auto-allowed by a narrow grant"
            );
        }
        // An ordinary assignment is unaffected.
        assert!(command_is_allowed(
            tn::RUN_BASH,
            "FOO=1 ls",
            allowed_in(&["Bash(ls:*)"])
        ));
        // A broad grant means "any command", so it still covers it.
        assert!(command_is_allowed(
            tn::RUN_BASH,
            "LD_PRELOAD=/tmp/evil.so ls",
            allowed_in(&["Bash"])
        ));
    }

    #[test]
    fn compound_command_requires_every_head_covered() {
        // The permission-audit regression: a first-head grant must NOT cover a
        // chained command whose later segment has a different head.
        let cmd = "git status && curl -X POST https://api.example.com/pay";
        assert!(!command_is_allowed(
            tn::RUN_BASH,
            cmd,
            allowed_in(&["Bash(git:*)"])
        ));
        // Covering every head (or going broad) does allow it.
        assert!(command_is_allowed(
            tn::RUN_BASH,
            cmd,
            allowed_in(&["Bash(git:*)", "Bash(curl:*)"])
        ));
        assert!(command_is_allowed(tn::RUN_BASH, cmd, allowed_in(&["Bash"])));
        // A command that runs nothing derivable is never auto-allowed.
        assert!(!command_is_allowed(
            tn::RUN_BASH,
            "",
            allowed_in(&["Bash(git:*)"])
        ));
    }

    /// The derivation basenames, so a click on `/usr/bin/git push` stores
    /// `Bash(git:*)`. Matching does not, so that grant cannot cover a binary
    /// the agent wrote inside the workspace and pointed at by path.
    #[test]
    fn a_stored_grant_never_covers_a_path_qualified_head() {
        for cmd in [
            "data/bin/ls",
            "./ls -la",
            "/tmp/ls",
            "sudo ./ls",
            "ls && ./cat x",
        ] {
            assert!(
                !command_is_allowed(
                    tn::RUN_BASH,
                    cmd,
                    allowed_in(&["Bash(ls:*)", "Bash(cat:*)"])
                ),
                "{cmd} must not be auto-allowed by a bare-name grant"
            );
        }
        // A bare name is still covered, privilege prefixes and ordinary
        // assignments included, so no stored grant loses its everyday use.
        for cmd in ["ls -la", "sudo ls", "FOO=1 ls", "ls && cat x"] {
            assert!(
                command_is_allowed(
                    tn::RUN_BASH,
                    cmd,
                    allowed_in(&["Bash(ls:*)", "Bash(cat:*)"])
                ),
                "{cmd}"
            );
        }
        // A broad grant means "any command", so it is unaffected.
        assert!(command_is_allowed(
            tn::RUN_BASH,
            "data/bin/ls",
            allowed_in(&["Bash"])
        ));
        // Derivation is unchanged: the stored pattern is still the basename.
        assert_eq!(
            derive_command_allow_pattern(tn::RUN_BASH, "/usr/bin/git push", AllowScope::Narrow),
            Some("Bash(git:*)".to_string())
        );
    }

    #[test]
    fn python_is_allowed_only_by_coarse_python_grant() {
        assert!(command_is_allowed(
            tn::RUN_PYTHON,
            "requests.post(u)",
            allowed_in(&["Python"])
        ));
        assert!(!command_is_allowed(
            tn::RUN_PYTHON,
            "requests.post(u)",
            allowed_in(&["Bash"])
        ));
        // Non-command tools never auto-allow.
        assert!(!command_is_allowed(
            "read_file",
            "x",
            allowed_in(&["Bash", "Python"])
        ));
    }

    // The allowlist file itself (append, read, overwrite, parse) is covered in
    // `core::grants`, which owns it for all three lanes.

    // --- action_for_lane: resolved-lane + channel gate + trigger grant ------

    const NO_GRANT: &[SideEffectCategory] = &[];

    #[test]
    fn action_safe_proceeds_reversible_checkpoints_on_every_channel() {
        for ch in [None, Some(EventChannel::Chat), Some(EventChannel::Trigger)] {
            assert_eq!(
                action_for_lane(RiskLane::Safe, ch, None, NO_GRANT),
                GuardAction::Proceed
            );
            // Phase 4: reversible is snapshotted before it runs, on every channel.
            assert_eq!(
                action_for_lane(RiskLane::ReversibleDanger, ch, None, NO_GRANT),
                GuardAction::Checkpoint
            );
        }
    }

    #[test]
    fn action_catastrophic_refuses_on_every_channel() {
        for ch in [None, Some(EventChannel::Chat), Some(EventChannel::Trigger)] {
            assert_eq!(
                action_for_lane(RiskLane::Catastrophic, ch, None, NO_GRANT),
                GuardAction::Refuse
            );
        }
    }

    #[test]
    fn action_irreversible_asks_on_chat_regardless_of_grant() {
        // Regression: an ordinary chat turn carries channel == None (the loop
        // does not stamp Chat on its per-turn meta). The ask lane MUST still
        // fire — gating on `== Some(Chat)` would have silently let it run. The
        // grant is never consulted on chat (the user is asked every time).
        let cat = Some(SideEffectCategory::Email);
        let full_grant = &[SideEffectCategory::Email];
        assert_eq!(
            action_for_lane(RiskLane::IrreversibleDanger, None, cat, full_grant),
            GuardAction::Ask
        );
        assert_eq!(
            action_for_lane(
                RiskLane::IrreversibleDanger,
                Some(EventChannel::Chat),
                cat,
                NO_GRANT
            ),
            GuardAction::Ask
        );
    }

    #[test]
    fn action_irreversible_on_trigger_gates_on_grant() {
        let ch = Some(EventChannel::Trigger);
        // Granted category → run.
        assert_eq!(
            action_for_lane(
                RiskLane::IrreversibleDanger,
                ch,
                Some(SideEffectCategory::Email),
                &[SideEffectCategory::Email, SideEffectCategory::CloudCli],
            ),
            GuardAction::Proceed
        );
        // Ungranted category → fail the trigger, carrying the blocked category.
        assert_eq!(
            action_for_lane(
                RiskLane::IrreversibleDanger,
                ch,
                Some(SideEffectCategory::ExternalApi),
                &[SideEffectCategory::Email],
            ),
            GuardAction::FailTrigger(SideEffectCategory::ExternalApi)
        );
        // No category on an irreversible trigger command → treated as Other,
        // which an empty grant doesn't cover → fail.
        assert_eq!(
            action_for_lane(RiskLane::IrreversibleDanger, ch, None, NO_GRANT),
            GuardAction::FailTrigger(SideEffectCategory::Other)
        );
        // A trigger that explicitly granted Other runs the uncategorized one.
        assert_eq!(
            action_for_lane(
                RiskLane::IrreversibleDanger,
                ch,
                Some(SideEffectCategory::Other),
                &[SideEffectCategory::Other],
            ),
            GuardAction::Proceed
        );
    }

    // --- trigger_block_refusal: the message the user reads out of context ---

    fn bash_input(cmd: &str) -> Value {
        serde_json::json!({ "command": cmd })
    }

    #[test]
    fn trigger_block_refusal_names_what_was_tried() {
        // The whole point of the message: a user reading only the failure
        // notification must learn which command was blocked and why, not just
        // that "an irreversible real-world side-effect" was refused.
        let msg = trigger_block_refusal(
            SideEffectCategory::Other,
            tn::RUN_BASH,
            &bash_input("pkill -x \"Google Chrome\""),
            Some("Kills processes outside the workspace."),
        );
        assert!(msg.contains("pkill -x \"Google Chrome\""), "{msg}");
        assert!(msg.contains(tn::RUN_BASH), "{msg}");
        assert!(
            msg.contains("Kills processes outside the workspace."),
            "{msg}"
        );
        // Still carries the block verdict and the remedy.
        assert!(msg.contains("was NOT run"), "{msg}");
        assert!(
            msg.contains(SideEffectCategory::Other.label()),
            "the remedy must name the grant to tick: {msg}"
        );
    }

    #[test]
    fn trigger_block_refusal_without_a_judge_summary_still_names_the_command() {
        // The judge-off / judge-failed path produces no tailored summary; the
        // command excerpt is then the only concrete detail, so it must survive.
        let msg = trigger_block_refusal(
            SideEffectCategory::CloudCli,
            tn::RUN_BASH,
            &bash_input("gh release delete v1.2.3"),
            None,
        );
        assert!(msg.contains("gh release delete v1.2.3"), "{msg}");
        assert!(!msg.contains("Why it was gated"), "{msg}");
    }

    #[test]
    fn trigger_block_refusal_truncates_a_long_command() {
        let long = format!("echo {}", "x".repeat(2_000));
        let msg = trigger_block_refusal(
            SideEffectCategory::Other,
            tn::RUN_BASH,
            &bash_input(&long),
            None,
        );
        assert!(
            msg.len() < 1_000,
            "a long script must not flood the notification: {} bytes",
            msg.len()
        );
        assert!(msg.contains("..."), "truncation must be visible: {msg}");
        assert!(msg.contains(SideEffectCategory::Other.label()), "{msg}");
    }

    #[test]
    fn trigger_block_refusal_puts_the_raw_command_last() {
        // The push notification body IS this string and the OS preview shows
        // only its first lines, so the summary and the remedy must precede the
        // raw command text.
        let msg = trigger_block_refusal(
            SideEffectCategory::ExternalApi,
            tn::RUN_BASH,
            &bash_input("curl -X POST https://api.example.com/pay"),
            Some("Charges a payment endpoint."),
        );
        let why = msg.find("Why it was gated").expect("summary line");
        let remedy = msg.find("To allow it").expect("remedy line");
        let tried = msg.find("What it tried").expect("command block");
        assert!(why < remedy && remedy < tried, "{msg}");
    }

    #[test]
    fn trigger_block_refusal_fences_a_command_containing_backticks() {
        // A command carrying its own ``` run must not close the fence early and
        // let the rest of it render as markdown in the notification body.
        let msg = trigger_block_refusal(
            SideEffectCategory::Other,
            tn::RUN_BASH,
            &bash_input("echo '```' && rm -rf /etc/x"),
            None,
        );
        assert!(
            msg.contains("\n````\necho '```' && rm -rf /etc/x\n````"),
            "{msg}"
        );
    }

    #[test]
    fn code_fence_grows_past_the_longest_backtick_run() {
        assert_eq!(code_fence_for("plain command"), "```");
        assert_eq!(code_fence_for("echo `date`"), "```");
        assert_eq!(code_fence_for("echo '```'"), "````");
        assert_eq!(code_fence_for("`````"), "``````");
    }

    #[test]
    fn trigger_block_refusal_redacts_a_postgres_password() {
        let msg = trigger_block_refusal(
            SideEffectCategory::ExternalApi,
            tn::RUN_BASH,
            &bash_input("psql postgres://u:hunter2@localhost/db -c 'delete from t'"),
            None,
        );
        assert!(!msg.contains("hunter2"), "{msg}");
        assert!(msg.contains("postgres://u:***@localhost/db"), "{msg}");
    }

    #[test]
    fn trigger_block_refusal_omits_an_empty_command_block() {
        // Defensive: a blank command yields no excerpt rather than an empty
        // fenced block.
        let msg = trigger_block_refusal(
            SideEffectCategory::Other,
            tn::RUN_BASH,
            &bash_input("   "),
            None,
        );
        assert!(!msg.contains("What it tried"), "{msg}");
        assert!(msg.contains("was NOT run"), "{msg}");
    }

    #[test]
    fn judge_off_path_uses_static_fallback_classifier() {
        // The wiring sanity check — full fallback behavior (side-effect shapes,
        // destruction scan, lanes) is tested in `command_guard::tests`.
        let ji = JudgeInput {
            tool_name: tn::RUN_BASH.to_string(),
            command: "curl -X POST https://api/charge".to_string(),
            out_of_workspace: false,
            fast_path_refused: false,
        };
        let c = command_guard::fallback_classify(&ji);
        assert_eq!(c.lane, RiskLane::IrreversibleDanger);
        assert_eq!(c.category, Some(SideEffectCategory::ExternalApi));
    }
}
