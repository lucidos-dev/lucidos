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
use std::path::Path;
use std::sync::Mutex;

use serde_json::Value;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::engine::cc_permission::{DedupKey, PermissionState};
use crate::engine::claude_code::AllowScope;
use crate::engine::command_guard::{
    self, GuardDecision, JudgeInput, JudgedClassification, RiskLane, SideEffectCategory,
    StaticVerdict,
};
use crate::engine::event_bus::{BusEvent, EventBus};
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

/// Persisted allowlist file under `~/.lucidos/` — the command-guard counterpart
/// of `cc-allowed-tools`. Separate file because chat command patterns
/// (`Bash(git:*)`, `Python`) are not CC `--allowedTools` patterns.
const AGENT_ALLOWED_COMMANDS_FILE: &str = "agent-allowed-commands";
const AGENT_ALLOWED_COMMANDS_HEADER: &str = "# Lucidos Agent command allowlist — one pattern per line. Lines starting with '#' are ignored.\n# Patterns: Bash(<head>:*) e.g. Bash(git:*) · Bash (any bash) · Python (any python).\n# A chained command (&&, |, ;) auto-allows only when EVERY segment's head is covered.\n";

/// Compiled-in default allowlist — empty so the feature ships dark; users build
/// their list via the per-prompt "Always allow" buttons (which append to the
/// file) or by editing `agent-allowed-commands` directly.
pub const DEFAULT_AGENT_ALLOWED_COMMANDS: &[&str] = &[];

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
            AllowScope::Session => bash_narrow_pattern(command).or_else(|| Some("Bash".to_string())),
        },
        tn::RUN_PYTHON | tn::RUN_PYTHON_BACKGROUND => Some("Python".to_string()),
        _ => None,
    }
}

/// Whether the granted pattern set fully covers `command` — the auto-allow
/// (skip the card) check. `allowed` answers "is this exact pattern granted?"
/// over the union of the session set and the persisted allowlist.
///
/// Bash: a broad `Bash` grant covers everything; otherwise EVERY command
/// segment's head must be covered by its `Bash(<head>:*)` pattern — matching
/// only the first head would let `git status && curl -X POST …` ride into a
/// `Bash(git:*)` grant. A command with no derivable head is never auto-allowed
/// (the card is shown). Python: the coarse `Python` pattern (the python tool
/// has no finer sub-scope).
pub fn command_is_allowed(
    tool_name: &str,
    command: &str,
    allowed: impl Fn(&str) -> bool,
) -> bool {
    match tool_name {
        tn::RUN_BASH | tn::RUN_BASH_BACKGROUND => {
            if allowed("Bash") {
                return true;
            }
            let heads = command_guard::segment_heads(command);
            !heads.is_empty() && heads.iter().all(|h| allowed(&format!("Bash({h}:*)")))
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

/// Patterns from `<user_dir>/agent-allowed-commands` (one per line, blanks and
/// `#` comments ignored). Empty on a missing file, a `None` user_dir, or an IO
/// error — a read failure must never auto-allow, so it degrades to "no
/// patterns" (the user re-approves once rather than the guard silently opening).
pub fn agent_allowed_commands_patterns(user_dir: Option<&Path>) -> Vec<String> {
    let Some(dir) = user_dir else {
        return DEFAULT_AGENT_ALLOWED_COMMANDS
            .iter()
            .map(|s| s.to_string())
            .collect();
    };
    let path = dir.join(AGENT_ALLOWED_COMMANDS_FILE);
    match std::fs::read_to_string(&path) {
        Ok(contents) => contents
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .map(|l| l.to_string())
            .collect(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => DEFAULT_AGENT_ALLOWED_COMMANDS
            .iter()
            .map(|s| s.to_string())
            .collect(),
        Err(e) => {
            crate::log!(
                "[CommandGuard] Failed to read {}: {} — treating as no allowlist",
                path.display(),
                e
            );
            Vec::new()
        }
    }
}

/// Append `pattern` to `<user_dir>/agent-allowed-commands` if not already
/// present. Creates the file (with the header comment) if it doesn't exist.
/// Atomic write via tmp + rename. No-op when `user_dir` is `None`.
pub fn append_agent_allowed_command(
    user_dir: Option<&Path>,
    pattern: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let Some(dir) = user_dir else {
        return Ok(());
    };
    let path = dir.join(AGENT_ALLOWED_COMMANDS_FILE);
    let existing = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            AGENT_ALLOWED_COMMANDS_HEADER.to_string()
        }
        Err(e) => return Err(e.into()),
    };
    if existing
        .lines()
        .map(str::trim)
        .any(|l| !l.is_empty() && !l.starts_with('#') && l == pattern)
    {
        return Ok(());
    }
    let mut next = existing;
    if !next.is_empty() && !next.ends_with('\n') {
        next.push('\n');
    }
    next.push_str(pattern);
    next.push('\n');
    std::fs::create_dir_all(dir)?;
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, &next)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// Read the raw contents of `<user_dir>/agent-allowed-commands` for the settings
/// UI. A missing file returns the seeded header (mirrors `read_allowed_tools_file`
/// in `engine::claude_code` for the CC allowlist), so the editor always opens
/// with the instructional comment even before the first "Always allow" grant.
pub fn read_agent_allowed_commands_file(
    user_dir: &Path,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let path = user_dir.join(AGENT_ALLOWED_COMMANDS_FILE);
    match std::fs::read_to_string(&path) {
        Ok(s) => Ok(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Ok(AGENT_ALLOWED_COMMANDS_HEADER.to_string())
        }
        Err(e) => Err(e.into()),
    }
}

/// Atomically overwrite `<user_dir>/agent-allowed-commands` with `contents`. The
/// command guard reads the file fresh on each prompt (see
/// [`agent_allowed_commands_patterns`]), so an edit takes effect on the next
/// gated command — no restart. Mirrors `write_allowed_tools_file` in
/// `engine::claude_code`.
pub fn write_agent_allowed_commands_file(
    user_dir: &Path,
    contents: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    std::fs::create_dir_all(user_dir)?;
    let path = user_dir.join(AGENT_ALLOWED_COMMANDS_FILE);
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, contents)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// Record a granted "Always allow" by scope: `Session` into the in-memory
/// per-thread allow set; `Narrow` / `Broad` into the persisted allowlist file.
/// No-op for scopes whose pattern doesn't derive. Mirrors the CC consent
/// endpoint's `record_allow_grant`.
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
            if let Err(e) = append_agent_allowed_command(engine.user_dir(), &pattern) {
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

/// The tool result fed back to the LLM when a trigger's command is blocked
/// because the firing trigger isn't granted the command's side-effect category
/// (ADR 0002, Phase 5). The agentic loop also emits a terminal `ResponseFailed`
/// and returns `Err`, so the scheduler's failure-notification path surfaces it.
fn trigger_block_refusal(category: SideEffectCategory) -> String {
    format!(
        "Blocked by the command guard — this trigger is not authorized to perform {reason}, so the \
         command was NOT run and the trigger failed. To allow it, grant the \"{label}\" side-effect \
         on the trigger (in the trigger's settings) and re-run.",
        reason = category.reason(),
        label = category.label(),
    )
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
            .resolve_command_lane(ctx, tool_name, input, meta.channel, cancel_token)
            .await
        else {
            return GuardDecision::Refuse(canceled_refusal());
        };

        match action_for_lane(lane, meta.channel, category, ctx.trigger_grant) {
            GuardAction::Proceed => GuardDecision::Proceed,
            GuardAction::Checkpoint => {
                // Snapshot the workspace before the in-workspace destruction so
                // the user can one-click Undo. A failed snapshot logs and still
                // proceeds (unguarded) — the pre-Phase-4 behavior — rather than
                // block a legitimate cleanup on a git hiccup.
                self.checkpoint_before_reversible_command(
                    thread_id,
                    tool_name,
                    input,
                    meta,
                    summary.as_deref(),
                )
                .await;
                GuardDecision::Proceed
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
                GuardDecision::FailTrigger(trigger_block_refusal(cat))
            }
        }
    }

    /// Snapshot the workspace before a `ReversibleDanger` command and emit
    /// `CommandCheckpointed` so the user can one-click Undo (ADR 0002, Phase 4).
    /// Best-effort: a failed snapshot logs and emits nothing, and the command
    /// still runs — in-workspace destruction was recoverable-in-principle before
    /// Phase 4 too, so a git hiccup must not block a legitimate cleanup. With no
    /// `CommandCheckpointed` event the UI simply shows no Undo affordance (never
    /// a button that can't actually restore).
    async fn checkpoint_before_reversible_command(
        &self,
        thread_id: Uuid,
        tool_name: &str,
        input: &Value,
        meta: &EventMeta,
        summary_override: Option<&str>,
    ) {
        let command = command_guard::command_text(tool_name, input)
            .unwrap_or_default()
            .to_string();
        let checkpoint_id = Uuid::new_v4().to_string();
        if let Err(e) =
            crate::engine::git_ops::create_command_checkpoint(self.workspace_path(), &checkpoint_id)
                .await
        {
            crate::log!(
                "[CommandGuard] checkpoint failed ({}); running the command without an undo point",
                e
            );
            return;
        }
        let summary = summary_override.map(str::to_string).unwrap_or_else(|| {
            "Deletes or overwrites files inside the workspace (recoverable).".to_string()
        });
        // Same postgres-URL redaction the agentic loop applies to ToolCalled.args
        // before the command text is persisted + SSE-broadcast.
        let command_for_event = crate::core::redact_postgres_secrets(&command);
        self.event_bus
            .emit_or_log(
                BusEvent::Thread {
                    thread_id,
                    event: ThreadEvent::CommandCheckpointed {
                        checkpoint_id,
                        command: command_for_event,
                        summary,
                    },
                    meta: meta.clone(),
                },
                "[CommandGuard] CommandCheckpointed",
            )
            .await;
    }

    /// Undo a command checkpoint (ADR 0002, Phase 4): restore the workspace
    /// working tree from the checkpoint ref, delete the ref, and emit
    /// `CommandCheckpointReverted`. The originating thread is resolved from the
    /// `CommandCheckpointed` event (so the revert lands on the right thread).
    /// Idempotent: a checkpoint already reverted (or whose ref is gone) is a
    /// no-op error-free return on the duplicate, an `Err` only on a genuinely
    /// unknown id or a failed git restore.
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

        crate::engine::git_ops::restore_command_checkpoint(self.workspace_path(), checkpoint_id)
            .await?;
        crate::engine::git_ops::delete_command_checkpoint_ref(self.workspace_path(), checkpoint_id)
            .await;

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
    /// `channel` is unused here now (both chat and triggers classify the middle)
    /// but kept on the signature for symmetry with the caller and future
    /// channel-specific routing.
    async fn resolve_command_lane(
        &self,
        ctx: &mut CommandGuardCtx<'_>,
        tool_name: &str,
        input: &Value,
        _channel: Option<EventChannel>,
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
        let persisted = agent_allowed_commands_patterns(self.user_dir());
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
            derive_command_allow_pattern(tn::RUN_BASH, "sudo /usr/bin/aws s3 rm x", AllowScope::Narrow),
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
        assert!(command_is_allowed(tn::RUN_BASH, "git pull", allowed_in(&["Bash"])));
    }

    #[test]
    fn compound_command_requires_every_head_covered() {
        // The permission-audit regression: a first-head grant must NOT cover a
        // chained command whose later segment has a different head.
        let cmd = "git status && curl -X POST https://api.example.com/pay";
        assert!(!command_is_allowed(tn::RUN_BASH, cmd, allowed_in(&["Bash(git:*)"])));
        // Covering every head (or going broad) does allow it.
        assert!(command_is_allowed(
            tn::RUN_BASH,
            cmd,
            allowed_in(&["Bash(git:*)", "Bash(curl:*)"])
        ));
        assert!(command_is_allowed(tn::RUN_BASH, cmd, allowed_in(&["Bash"])));
        // A command that runs nothing derivable is never auto-allowed.
        assert!(!command_is_allowed(tn::RUN_BASH, "", allowed_in(&["Bash(git:*)"])));
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
        assert!(!command_is_allowed("read_file", "x", allowed_in(&["Bash", "Python"])));
    }

    #[test]
    fn allowlist_roundtrip_append_then_read() {
        let dir = tempfile::tempdir().unwrap();
        let p = Some(dir.path());
        assert!(agent_allowed_commands_patterns(p).is_empty());
        append_agent_allowed_command(p, "Bash(git:*)").unwrap();
        append_agent_allowed_command(p, "Python").unwrap();
        // Duplicate append is a no-op.
        append_agent_allowed_command(p, "Bash(git:*)").unwrap();
        let patterns = agent_allowed_commands_patterns(p);
        assert!(patterns.contains(&"Bash(git:*)".to_string()));
        assert!(patterns.contains(&"Python".to_string()));
        assert_eq!(
            patterns.iter().filter(|p| *p == "Bash(git:*)").count(),
            1,
            "duplicate must not be appended twice"
        );
    }

    #[test]
    fn allowlist_missing_file_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert!(agent_allowed_commands_patterns(Some(dir.path())).is_empty());
        assert!(agent_allowed_commands_patterns(None).is_empty());
    }

    #[test]
    fn whole_file_read_missing_returns_header() {
        // The settings editor opens with the instructional header even before
        // the first grant creates the file.
        let dir = tempfile::tempdir().unwrap();
        let contents = read_agent_allowed_commands_file(dir.path()).unwrap();
        assert_eq!(contents, AGENT_ALLOWED_COMMANDS_HEADER);
    }

    #[test]
    fn whole_file_write_then_read_and_parse() {
        let dir = tempfile::tempdir().unwrap();
        let body = format!("{AGENT_ALLOWED_COMMANDS_HEADER}Bash(git:*)\nPython\n");
        write_agent_allowed_commands_file(dir.path(), &body).unwrap();
        // Round-trips verbatim …
        assert_eq!(read_agent_allowed_commands_file(dir.path()).unwrap(), body);
        // … and the guard's pattern reader sees the edited entries (a delete in
        // the editor removes a line; here we confirm what's written is read).
        let patterns = agent_allowed_commands_patterns(Some(dir.path()));
        assert_eq!(patterns, vec!["Bash(git:*)".to_string(), "Python".to_string()]);
    }

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
            action_for_lane(RiskLane::IrreversibleDanger, Some(EventChannel::Chat), cat, NO_GRANT),
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

    #[test]
    fn judge_off_path_uses_static_fallback_classifier() {
        // The wiring sanity check — full fallback behavior (side-effect shapes,
        // destruction scan, lanes) is tested in `command_guard::tests`.
        let ji = JudgeInput {
            tool_name: tn::RUN_BASH.to_string(),
            command: "curl -X POST https://api/charge".to_string(),
            out_of_workspace: false,
        };
        let c = command_guard::fallback_classify(&ji);
        assert_eq!(c.lane, RiskLane::IrreversibleDanger);
        assert_eq!(c.category, Some(SideEffectCategory::ExternalApi));
    }
}
