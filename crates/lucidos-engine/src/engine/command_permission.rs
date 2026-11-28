use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Mutex;

use serde_json::Value;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::core::grants::GrantFile;
use crate::engine::card_queue;
use crate::engine::cc_permission::{DedupKey, PermissionAnswer, PermissionEntry, PermissionState};
use crate::engine::command_guard::{
    self, Catastrophe, CommandSite, GuardDecision, JudgeInput, JudgedClassification, RiskLane,
    SideEffectCategory, StaticVerdict, TrustedHeads,
};
use crate::engine::command_judge::{JudgeVerdict, JUDGE_FAILED_REASON};
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::git_ops::CheckpointEffects;
use crate::engine::permission::standing_deny::{self, CommandKind};
use crate::engine::permission::tool_grants::{self, AnsweredBy};
use crate::engine::thread_events::{AllowScope, GrantOffer, OfferWithheld};
use crate::engine::thread_events::{EventChannel, EventMeta, MessageOrigin, ThreadEvent};
use crate::engine::LucidosEngine;
use crate::llm::tool_names as tn;

pub const DENIAL_REASON: &str = "User denied";
pub const SUPERSEDED_REASON: &str = "Superseded by a new message";
pub const CANCELED_REASON: &str = "Canceled by user";
pub const MCP_SETUP_SUMMARY: &str = "Sets up an MCP server that runs this command now and each \
                                 time the server starts.";

pub const MCP_START_SUMMARY: &str = "Starts an MCP server whose command nobody has approved \
                                 yet. It runs this command now and each time the server starts. \
                                 `<stored value>` stands for a value kept sealed.";

const UNPLACEABLE_MCP_LAUNCH: &str = "Refused by the command guard: this MCP server was NOT \
                                      started. The engine's working directory is unknown, so \
                                      the guard cannot tell where the server would run.";

fn unfingerprintable_mcp_launch(e: impl std::fmt::Display) -> String {
    format!(
        "Refused by the command guard: this MCP server was NOT started. Its launch \
         approval cannot be checked: {e}"
    )
}

struct GuardedMcpLaunch {
    server_id: String,
    fingerprint: String,
    line: String,
    lead: &'static str,
}

async fn guarded_mcp_launch(
    pool: &sqlx::PgPool,
    root: &std::path::Path,
    spawn_cwd: Option<std::path::PathBuf>,
    guard_on: bool,
    tool_name: &str,
    input: &Value,
) -> Option<Result<GuardedMcpLaunch, String>> {
    let action = crate::engine::tool_arguments::dispatch_name(tool_name, input).ok()?;
    if action == tn::SETUP_MCP_SERVER {
        let line = match command_guard::mcp_setup_command_line(
            tool_name,
            input,
            spawn_cwd.as_deref(),
            root,
        )? {
            Ok(line) => line,
            Err(refusal) => return Some(Err(refusal)),
        };
        // INVARIANT: Rendering the line parsed the setup and placed its working
        // directory, so both hold here. Refuse rather than skip the guard.
        let (Ok(setup), Some(cwd)) = (
            crate::engine::tool_arguments::McpSetupRequest::parse(input),
            spawn_cwd,
        ) else {
            return Some(Err(UNPLACEABLE_MCP_LAUNCH.to_string()));
        };
        let fingerprint = match setup.launch_spec().fingerprint(&cwd, root) {
            Ok(fingerprint) => fingerprint,
            Err(e) => return Some(Err(unfingerprintable_mcp_launch(e))),
        };
        return Some(Ok(GuardedMcpLaunch {
            server_id: setup.id.to_string(),
            fingerprint,
            line,
            lead: MCP_SETUP_SUMMARY,
        }));
    }
    if !guard_on || !matches!(action, tn::ENABLE_MCP_SERVER | tn::START_MCP_SERVER) {
        return None;
    }
    let id = input["id"].as_str()?;
    let server = match crate::core::McpServerStore::get(pool, id).await {
        Ok(server) => server?,
        Err(e) => {
            return Some(Err(format!(
                "Refused by the command guard: MCP server '{id}' was NOT started. Its \
             registration could not be read: {e}"
            )))
        }
    };
    let Some(cwd) = spawn_cwd else {
        return Some(Err(UNPLACEABLE_MCP_LAUNCH.to_string()));
    };
    let spec = server.launch_spec();
    let fingerprint = match spec.fingerprint(&cwd, root) {
        Ok(fingerprint) => fingerprint,
        Err(e) => return Some(Err(unfingerprintable_mcp_launch(e))),
    };
    match crate::core::McpServerStore::is_launch_approved(pool, id, &fingerprint).await {
        Ok(true) => None,
        // INVARIANT: A row stored before setup refused `=` in a command. The line would
        // read that command as an assignment and classify the next word.
        Ok(false) if server.command.contains('=') => Some(Err(format!(
            "Refused by the command guard: MCP server '{id}' was NOT started. Its command \
             contains '=', so the guard cannot read what it runs. Ask the user to remove \
             the server and set it up again with a command path without '='."
        ))),
        Ok(false) => Some(Ok(GuardedMcpLaunch {
            server_id: server.id.clone(),
            fingerprint,
            line: spec.command_line(&cwd, root, crate::core::LaunchLineMode::Masked),
            lead: MCP_START_SUMMARY,
        })),
        Err(e) => Some(Err(format!(
            "Refused by the command guard: MCP server '{id}' was NOT started. Its \
             launch approval could not be read: {e}"
        ))),
    }
}

/// INVARIANT: The pattern to STORE when the user grants an "Always allow" / "Allow for
/// this thread" click, by scope. `None` for non-command tools.
///
///   * Bash `Broad`   → `Bash` (any bash command)
///   * Bash `Narrow`  → `Bash(<first-token>:*)`, e.g. `Bash(git:*)`
///   * Bash `Session` → the same narrow pattern, and nothing when no head
///     derives. A bare `Bash` fallback would turn one click on an unreadable
///     command into a grant for every command in the thread.
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
            AllowScope::Narrow | AllowScope::Session => bash_narrow_pattern(command),
        },
        tn::RUN_PYTHON | tn::RUN_PYTHON_BACKGROUND => Some("Python".to_string()),
        _ => None,
    }
}

/// INVARIANT: Whether the granted pattern set fully covers `command`, the auto-allow
/// (skip the card) check. `allowed` answers "is this exact pattern granted?"
/// over the union of the session set and the persisted allowlist.
///
/// Bash routes to [`command_guard::grant_covers_command`], the one predicate
/// the coding-agent session lane also uses, so the two cannot drift. Python
/// takes the coarse `Python` pattern: the python tool has no finer sub-scope.
/// It covers no secret read, which no grant covers.
#[cfg(test)]
pub fn command_is_allowed(
    tool_name: &str,
    command: &str,
    site: CommandSite<'_>,
    allowed: impl Fn(&str) -> bool,
) -> bool {
    command_is_allowed_with_paths(
        tool_name,
        command,
        site,
        &command_guard::SecretPaths::conservative(),
        allowed,
    )
}

fn command_is_allowed_with_paths(
    tool_name: &str,
    command: &str,
    site: CommandSite<'_>,
    paths: &command_guard::SecretPaths,
    allowed: impl Fn(&str) -> bool,
) -> bool {
    match tool_name {
        tn::RUN_BASH | tn::RUN_BASH_BACKGROUND => command_guard::grant_covers_command_with_paths(
            "Bash",
            command,
            site,
            &TrustedHeads::NONE,
            paths,
            allowed,
        ),
        tn::RUN_PYTHON | tn::RUN_PYTHON_BACKGROUND => {
            !paths.python(command)
                && command_guard::tool_catastrophe(tool_name, command, site.root()).is_none()
                && allowed("Python")
        }
        _ => false,
    }
}

#[cfg(test)]
fn tool_call_reads_secret(tool_name: &str, input: &Value) -> bool {
    command_guard::command_text(tool_name, input)
        .is_some_and(|text| command_guard::tool_text_reads_secret(tool_name, text))
}

fn bash_narrow_pattern(command: &str) -> Option<String> {
    command_guard::first_command_token(command).map(|head| format!("Bash({head}:*)"))
}

fn grant_source<'c>(tool_name: &str, command: &'c str, site: CommandSite<'_>) -> Cow<'c, str> {
    match tool_name {
        tn::RUN_BASH | tn::RUN_BASH_BACKGROUND => command_guard::grant_command(command, site),
        _ => Cow::Borrowed(command),
    }
}

fn grant_head(tool_name: &str, command: &str, site: CommandSite<'_>) -> Option<String> {
    match tool_name {
        tn::RUN_BASH | tn::RUN_BASH_BACKGROUND => {
            command_guard::first_command_token(&grant_source(tool_name, command, site))
        }
        _ => None,
    }
}

#[cfg(test)]
pub fn command_card_offer(tool_name: &str, command: &str, site: CommandSite<'_>) -> GrantOffer {
    command_card_offer_with_paths(
        tool_name,
        command,
        site,
        &command_guard::SecretPaths::conservative(),
    )
}

fn command_card_offer_with_paths(
    tool_name: &str,
    command: &str,
    site: CommandSite<'_>,
    paths: &command_guard::SecretPaths,
) -> GrantOffer {
    let source = grant_source(tool_name, command, site);
    let deny = derive_command_allow_pattern(tool_name, &source, AllowScope::Narrow)
        .and_then(|line| standing_deny::card_rule(line, command_kind(tool_name)?, command));
    if paths.tool(tool_name, command) {
        return GrantOffer {
            deny,
            ..GrantOffer::withheld(OfferWithheld::SecretRead)
        };
    }
    if matches!(
        command_guard::tool_catastrophe(tool_name, command, site.root()),
        Some(Catastrophe::Unproven(_))
    ) {
        return GrantOffer {
            deny,
            ..GrantOffer::withheld(OfferWithheld::UnprovenWorkspaceDestruction)
        };
    }
    let offered = |scope| {
        let pattern = derive_command_allow_pattern(tool_name, &source, scope)?;
        command_is_allowed_with_paths(tool_name, command, site, paths, |p| p == pattern)
            .then_some(pattern)
    };
    let broad = offered(AllowScope::Broad);
    GrantOffer {
        narrow: offered(AllowScope::Narrow).filter(|narrow| Some(narrow) != broad.as_ref()),
        session: offered(AllowScope::Session),
        broad,
        withheld: None,
        deny,
    }
}

fn command_kind(tool_name: &str) -> Option<CommandKind> {
    match tool_name {
        tn::RUN_BASH | tn::RUN_BASH_BACKGROUND => Some(CommandKind::Shell),
        tn::RUN_PYTHON | tn::RUN_PYTHON_BACKGROUND => Some(CommandKind::Python),
        _ => None,
    }
}

/// INVARIANT: The refusal a *standing deny* gives this tool call, if one covers it. The
/// chat and trigger lanes both read it here, after the floor and before any
/// allow or the judge.
async fn standing_deny_refusal(
    pool: &sqlx::PgPool,
    tool_name: &str,
    input: &Value,
) -> Option<String> {
    let kind = command_kind(tool_name)?;
    let command = command_guard::command_text(tool_name, input)?;
    standing_deny::refusal_for(pool, kind, command).await
}

pub async fn record_command_allow_grant(
    engine: &LucidosEngine,
    entry: &PermissionEntry,
    scope: AllowScope,
    answered_by: AnsweredBy,
) {
    let Some(pattern) = entry.grant_offer.pattern(scope).map(str::to_string) else {
        return;
    };
    if let Err(e) = tool_grants::record_card_allow(
        &engine.event_bus,
        GrantFile::AgentCommands,
        entry.thread_id,
        &entry.request_id,
        pattern.clone(),
        scope,
        answered_by,
    )
    .await
    {
        crate::log_legacy!("[CommandGuard] Failed to record allow pattern {pattern:?}: {e}");
    }
}

pub fn denial_refusal() -> String {
    "Refused by the command guard — the user denied permission to run this command, so it was NOT \
     run. Do not retry it. Either explain what you intended and ask the user how they'd like to \
     proceed, or choose a safe alternative that has no irreversible real-world side-effect."
        .to_string()
}

fn canceled_refusal() -> String {
    "The command was not run — the request was canceled before you got permission.".to_string()
}

fn withdrawn_refusal() -> String {
    "The command was NOT run: the permission card was withdrawn before anyone answered it, \
     usually because the user sent a new message. This is not a denial. Read the user's latest \
     message before deciding whether to retry."
        .to_string()
}

fn decision_from_answer(
    answer: Result<PermissionAnswer, tokio::sync::broadcast::error::RecvError>,
) -> GuardDecision {
    match answer {
        Ok(PermissionAnswer::Allowed) => GuardDecision::Proceed,
        Ok(PermissionAnswer::Denied) => GuardDecision::Refuse(denial_refusal()),
        Ok(PermissionAnswer::Refused(reason)) => GuardDecision::Refuse(reason),
        Ok(PermissionAnswer::Withdrawn(_)) | Err(_) => GuardDecision::Refuse(withdrawn_refusal()),
    }
}

const BLOCKED_COMMAND_EXCERPT_BYTES: usize = 400;

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
    let remedy = if category.is_grantable() {
        TRIGGER_GRANT_REMEDY
    } else {
        ungrantable_remedy(category)
    };
    msg.push_str(&format!("\n\n{remedy}"));
    // INVARIANT: Keep the command in the run transcript and out of the notification.
    if let Some(excerpt) = blocked_command_excerpt(tool_name, input) {
        let fence = code_fence_for(&excerpt);
        msg.push_str(&format!(
            "\n\nWhat it tried ({tool_name}):\n{fence}\n{excerpt}\n{fence}"
        ));
    }
    msg
}

const TRIGGER_GRANT_REMEDY: &str =
    "Review the command before granting it. A grant lets this trigger run the program again.";

fn ungrantable_remedy(category: SideEffectCategory) -> &'static str {
    match category {
        SideEffectCategory::UnprovenWorkspaceDestruction => {
            "No program grant can allow this. Run the command from a chat turn instead, where \
             you can allow it once after reviewing it."
        }
        _ => {
            "No program grant can allow this. Name a credential (CRED_*) in the command to \
             use a stored secret instead."
        }
    }
}

fn code_fence_for(excerpt: &str) -> String {
    let longest_run = excerpt.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    "`".repeat(longest_run.saturating_add(1).max(3))
}

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
            crate::log_legacy!(
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
        // INVARIANT: Best-effort unblock of a still-waiting loop. The std Mutex is released
        // before the `.await` below — never hold it across an await.
        {
            let mut state = pending.lock().unwrap();
            if let Some(entry) = state.take(&request_id) {
                let _ = entry
                    .tx
                    .send(PermissionAnswer::Withdrawn(SUPERSEDED_REASON));
            }
        }
        emit_command_permission_resolved(
            event_bus,
            thread_id,
            request_id,
            false,
            Some(SUPERSEDED_REASON.to_string()),
            None,
            None,
            EventMeta::with_actor(actor.clone()),
            "[CommandGuard] CommandPermissionResolved (superseded)",
        )
        .await;
    }
}

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
            crate::log_legacy!("[Recovery] orphan command permission query failed: {}", e);
            return;
        }
    };

    if rows.is_empty() {
        return;
    }

    crate::log_legacy!(
        "[Recovery] Auto-resolving {} orphan command permission request(s) (chat turn gone after restart)",
        rows.len()
    );

    for (thread_id, request_id) in rows {
        emit_command_permission_resolved(
            event_bus,
            thread_id,
            request_id,
            false,
            Some(crate::engine::cc_permission::RESTART_CLOSED_REASON.to_string()),
            None,
            None,
            EventMeta::NONE,
            "[Recovery] CommandPermissionResolved (orphan)",
        )
        .await;
    }
}

pub async fn resolve_command_permission(
    engine: &LucidosEngine,
    request_id: String,
    allowed: bool,
    persist_scope: Option<AllowScope>,
    always_deny: bool,
    answered_by: AnsweredBy,
    log_context: &str,
) -> Result<bool, String> {
    let entry = {
        let mut pending = engine.pending_command_permission.lock().unwrap();
        pending.take(&request_id)
    };
    let Some(entry) = entry else {
        return Ok(false);
    };
    let standing_deny = if always_deny {
        Some(standing_deny::record_card_rule(engine, &entry, &answered_by).await)
    } else {
        None
    };
    let blocked = if allowed {
        standing_deny_refusal(&engine.pool, &entry.tool_name, &entry.input).await
    } else {
        None
    };
    let allowed = allowed && blocked.is_none();
    let refusal = blocked.clone().or_else(|| {
        standing_deny
            .as_ref()
            .and_then(|result| result.as_ref().ok())
            .map(|line| standing_deny::refusal(line))
    });
    let answer = refusal.map_or_else(
        || PermissionAnswer::from_decision(allowed),
        PermissionAnswer::Refused,
    );
    let _ = entry.tx.send(answer);

    let reason = blocked.or_else(|| (!allowed).then(|| DENIAL_REASON.to_string()));
    let persist_scope =
        persist_scope.filter(|scope| allowed && entry.grant_offer.pattern(*scope).is_some());
    if let Some(scope) = persist_scope {
        record_command_allow_grant(engine, &entry, scope, answered_by.clone()).await;
    }
    emit_command_permission_resolved(
        &engine.event_bus,
        entry.thread_id,
        request_id,
        allowed,
        reason,
        persist_scope,
        standing_deny
            .as_ref()
            .and_then(|result| result.as_ref().ok())
            .cloned(),
        EventMeta::with_actor(answered_by.actor),
        log_context,
    )
    .await;
    if let Some(Err(error)) = standing_deny {
        return Err(format!(
            "The command was denied. Could not save the standing deny: {error}"
        ));
    }
    Ok(true)
}

#[allow(clippy::too_many_arguments)]
pub async fn emit_command_permission_resolved(
    event_bus: &EventBus,
    thread_id: Uuid,
    request_id: String,
    allowed: bool,
    reason: Option<String>,
    persist_scope: Option<AllowScope>,
    standing_deny: Option<String>,
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
                    standing_deny,
                },
                meta,
            },
            log_context,
        )
        .await;
}

pub struct CommandGuardCtx<'a> {
    /// INVARIANT: Enables the judge and reversible checks. The catastrophic floor and secret gate remain on.
    pub enabled: bool,
    pub judge_enabled: bool,
    pub judge_cache: &'a mut HashMap<String, JudgedClassification>,
}

#[derive(Debug, PartialEq, Eq)]
enum GuardAction {
    Proceed,
    Checkpoint,
    Refuse,
    Ask,
    FailTrigger(SideEffectCategory),
}

fn trigger_program_covers(
    grants: &[crate::triggers::config::TriggerProgramGrant],
    tool_name: &str,
    command: &str,
    site: CommandSite<'_>,
    paths: &command_guard::SecretPaths,
) -> bool {
    command_is_allowed_with_paths(tool_name, command, site, paths, |pattern| {
        grants
            .iter()
            .any(|grant| grant.legacy.is_none() && grant.pattern == pattern)
    })
}

pub(crate) fn trigger_program_pattern(
    tool_name: &str,
    command: &str,
    site: CommandSite<'_>,
    paths: &command_guard::SecretPaths,
) -> Option<String> {
    let offer = command_card_offer_with_paths(tool_name, command, site, paths);
    offer.narrow.or_else(|| {
        matches!(tool_name, tn::RUN_PYTHON | tn::RUN_PYTHON_BACKGROUND)
            .then_some(offer.broad)
            .flatten()
    })
}

/// INVARIANT: The lane an MCP server setup or start takes for its command's `lane`. A checkpoint
/// covers one run, but the server runs the command again on every start, with
/// no checkpoint. So reversible destruction is irreversible here.
fn mcp_launch_lane(lane: RiskLane) -> RiskLane {
    match lane {
        RiskLane::ReversibleDanger => RiskLane::IrreversibleDanger,
        lane => lane,
    }
}

fn action_for_lane(
    lane: RiskLane,
    channel: Option<EventChannel>,
    category: Option<SideEffectCategory>,
) -> GuardAction {
    match lane {
        RiskLane::Safe => GuardAction::Proceed,
        RiskLane::ReversibleDanger => GuardAction::Checkpoint,
        RiskLane::Catastrophic => GuardAction::Refuse,
        RiskLane::IrreversibleDanger => {
            if channel == Some(EventChannel::Trigger) {
                GuardAction::FailTrigger(category.unwrap_or(SideEffectCategory::Other))
            } else {
                GuardAction::Ask
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DiscardReason {
    NothingCaptured,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CheckpointOutcome {
    Card { restores: u32, removes: u32 },
    Discard(DiscardReason),
    KeepRefsNoCard,
}

/// INVARIANT: Map the post-image attempt onto what to do about it.
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

fn judged(
    outcome: Result<JudgeVerdict, Box<dyn std::error::Error + Send + Sync>>,
) -> JudgedClassification {
    let verdict = outcome.unwrap_or_else(|e| JudgeVerdict::judge_failed(e.as_ref()));
    JudgedClassification {
        lane: verdict.lane,
        summary: Some(verdict.summary),
        category: verdict.category,
        judge_reason: Some(verdict.reason),
    }
}

fn decision_trace(layer: &str, input: Option<&JudgeInput>, reason: Option<&str>) -> String {
    let evidence = input.map_or_else(
        || "static_verdict=settled".to_string(),
        |input| {
            format!(
                "target_outside_workspace={} fast_path_refused={}",
                input.out_of_workspace, input.fast_path_refused
            )
        },
    );
    // INVARIANT: A provider error can echo the command, so a failure keeps its label only.
    let reason = match reason {
        Some(failed) if failed.starts_with(JUDGE_FAILED_REASON) => JUDGE_FAILED_REASON.to_string(),
        other => crate::core::redact_postgres_secrets(other.unwrap_or("not-called"))
            .replace(['\n', '\r'], " "),
    };
    format!("layer={layer} {evidence} judge={reason}")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandCheckpointOrigin {
    pub thread_id: Uuid,
    pub request_event_id: Option<Uuid>,
}

pub async fn command_checkpoint_origin(
    pool: &sqlx::PgPool,
    checkpoint_id: &str,
) -> Result<Option<CommandCheckpointOrigin>, String> {
    let row: Option<(Option<Uuid>, Option<String>)> = sqlx::query_as(
        "SELECT thread_id, payload->>'request_event_id' FROM events \
         WHERE event_type = 'CommandCheckpointed' \
           AND payload->>'checkpoint_id' = $1 \
         LIMIT 1",
    )
    .bind(checkpoint_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| format!("checkpoint lookup failed: {e}"))?;
    Ok(match row {
        Some((Some(thread_id), request_event_id)) => Some(CommandCheckpointOrigin {
            thread_id,
            request_event_id: request_event_id.and_then(|s| Uuid::parse_str(&s).ok()),
        }),
        _ => None,
    })
}

impl LucidosEngine {
    #[allow(clippy::too_many_arguments)]
    pub async fn command_guard_decision(
        &self,
        ctx: &mut CommandGuardCtx<'_>,
        tool_name: &str,
        input: &Value,
        thread_id: Uuid,
        tool_use_id: &str,
        meta: &EventMeta,
        cancel_token: &CancellationToken,
        paths: &command_guard::SecretPaths,
    ) -> GuardDecision {
        let mcp_launch = match guarded_mcp_launch(
            &self.pool,
            self.workspace_path(),
            std::env::current_dir().ok(),
            ctx.enabled,
            tool_name,
            input,
        )
        .await
        {
            Some(Err(refusal)) => {
                crate::log_legacy!("[CommandGuard] decision layer=mcp-launch result=refuse");
                return GuardDecision::Refuse(refusal);
            }
            launch => launch.and_then(Result::ok),
        };
        let shell_input;
        let (tool_name, input) = match &mcp_launch {
            None => (tool_name, input),
            Some(launch) => {
                shell_input = serde_json::json!({ "command": launch.line });
                (tn::RUN_BASH, &shell_input)
            }
        };
        // INVARIANT: The catastrophic floor is unconditional, including preference-off calls.
        let floor = command_guard::static_classify_with_paths(
            tool_name,
            input,
            CommandSite::Workspace(self.workspace_path()),
            &TrustedHeads::NONE,
            paths,
        );
        if floor == StaticVerdict::Settled(RiskLane::Catastrophic) {
            crate::log_legacy!("[CommandGuard] decision layer=floor lane=Catastrophic static_verdict=settled judge=not-called");
            return GuardDecision::Refuse(command_guard::catastrophic_refusal(tool_name, input));
        }
        // INVARIANT: The owner's own rule holds with the guard preference on or off.
        if let Some(refusal) = standing_deny_refusal(&self.pool, tool_name, input).await {
            return GuardDecision::Refuse(refusal);
        }
        let unproven_floor = floor
            == StaticVerdict::SettledIrreversible(SideEffectCategory::UnprovenWorkspaceDestruction);
        if mcp_launch.is_some() && unproven_floor {
            return GuardDecision::Refuse(
                concat!(
                    "Refused by the command guard: the MCP server was not started. ",
                    "Its launch may delete a Lucidos workspace or its database. ",
                    "MCP launch approval persists across starts, so Allow once is unavailable."
                )
                .to_string(),
            );
        }
        if !ctx.enabled
            && !unproven_floor
            && !command_guard::command_text(tool_name, input)
                .is_some_and(|text| paths.gates_tool(tool_name, text))
        {
            crate::log_legacy!(
                "[CommandGuard] decision layer=preference guard_enabled=false judge=not-called"
            );
            return GuardDecision::Proceed;
        }
        let Some((classification, mut trace)) = self
            .resolve_command_lane(ctx, tool_name, input, thread_id, cancel_token, paths, floor)
            .await
        else {
            crate::log_legacy!("[CommandGuard] decision layer=cancellation result=refuse");
            return GuardDecision::Refuse(canceled_refusal());
        };
        let JudgedClassification {
            lane,
            summary,
            category,
            ..
        } = classification;
        let lane = match mcp_launch {
            Some(_) => mcp_launch_lane(lane),
            None => lane,
        };
        let action = action_for_lane(lane, meta.channel, category);
        let (action, trigger_pattern) = match action {
            GuardAction::FailTrigger(category) => {
                self.trigger_program_action(category, tool_name, input, paths)
            }
            _ => (action, None),
        };
        trace.push_str(&format!(
            " lane={lane:?} category={category:?} action={action:?}"
        ));
        crate::log_legacy!("[CommandGuard] decision {}", trace);
        let gated_summary = match &mcp_launch {
            Some(launch) => Some(format!(
                "{} {}",
                launch.lead,
                summary
                    .clone()
                    .unwrap_or_else(|| command_guard::permission_summary(tool_name, input))
            )),
            None => summary.clone(),
        };

        let decision = match action {
            GuardAction::Proceed => GuardDecision::Proceed,
            GuardAction::Checkpoint => {
                match self
                    .checkpoint_before_reversible_command(tool_name, input, summary.as_deref())
                    .await
                {
                    Some(pending) => GuardDecision::ProceedCheckpointed(pending),
                    None => GuardDecision::Proceed,
                }
            }
            GuardAction::Refuse => {
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
                    gated_summary.as_deref(),
                    &trace,
                    paths,
                    unproven_floor,
                )
                .await
            }
            GuardAction::FailTrigger(cat) => {
                GuardDecision::FailTrigger(command_guard::TriggerCommandBlocked {
                    reason: format!(
                        "{}\nDecision trace: {trace}",
                        trigger_block_refusal(cat, tool_name, input, gated_summary.as_deref())
                    ),
                    pattern: trigger_pattern,
                    folder: None,
                    thread_id,
                })
            }
        };
        // INVARIANT: With the guard on, a launch it let through is approved. The start
        // gate in `McpManager` then lets it spawn now and on every later start.
        // An unrecorded approval refuses here, since the start gate would.
        if let (true, Some(launch), GuardDecision::Proceed) = (ctx.enabled, &mcp_launch, &decision)
        {
            if let Err(e) = crate::core::McpServerStore::approve_launch(
                &self.pool,
                &self.event_bus,
                &launch.server_id,
                &launch.fingerprint,
                Some(crate::engine::thread_events::agent_tool_actor(thread_id)),
            )
            .await
            {
                crate::log_legacy!(
                    "[CommandGuard] could not record the launch approval for MCP server '{}': {}",
                    launch.server_id,
                    e
                );
                return GuardDecision::Refuse(format!(
                    "Refused by the command guard: MCP server '{}' was NOT started. The \
                     approval could not be saved: {e}. Try again later.",
                    launch.server_id
                ));
            }
        }
        decision
    }

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
            crate::log_legacy!(
                "[CommandGuard] checkpoint failed ({}); running the command without an undo point",
                e
            );
            return None;
        }
        Some(command_guard::PendingCheckpoint {
            checkpoint_id,
            command: crate::core::redact_postgres_secrets(command),
            summary: summary_override.map(str::to_string).unwrap_or_else(|| {
                "Deletes or overwrites files inside the workspace (recoverable).".to_string()
            }),
        })
    }

    pub async fn finalize_command_checkpoint(
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
            crate::log_legacy!("[CommandGuard] checkpoint {} post image failed: {}", id, e);
        }
        match checkpoint_outcome(effects) {
            CheckpointOutcome::Discard(reason) => {
                crate::log_legacy!(
                    "[CommandGuard] checkpoint {} dropped: {}",
                    id,
                    reason.explain()
                );
                crate::engine::git_ops::delete_command_checkpoint_pair(workspace, id).await;
            }
            CheckpointOutcome::KeepRefsNoCard => crate::log_legacy!(
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

    pub async fn undo_command_checkpoint(
        &self,
        checkpoint_id: &str,
        origin: CommandCheckpointOrigin,
        actor: Option<MessageOrigin>,
    ) -> Result<(), String> {
        let CommandCheckpointOrigin {
            thread_id,
            request_event_id,
        } = origin;

        // INVARIANT: Already reverted: a second undo is a no-op, and must not re-apply
        // the restore over edits made since the first.
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

        crate::engine::git_ops::revert_command_effects(self.workspace_path(), checkpoint_id)
            .await?;
        // INVARIANT: The refs deliberately survive: they are what the card's diff viewer
        // reads, and a reverted card must still be able to show what happened.
        // `prune_expired_checkpoints` reclaims them once they age out.

        self.event_bus
            .emit_or_log(
                BusEvent::Thread {
                    thread_id,
                    event: ThreadEvent::CommandCheckpointReverted {
                        checkpoint_id: checkpoint_id.to_string(),
                    },
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

    /// INVARIANT: Consult this fire's trigger grants after the standing deny and static floor.
    fn trigger_program_action(
        &self,
        category: SideEffectCategory,
        tool_name: &str,
        input: &Value,
        paths: &command_guard::SecretPaths,
    ) -> (GuardAction, Option<String>) {
        use crate::scheduler::user_tasks::{current_trigger_id, TRIGGER_DEFINITION};

        let blocked = GuardAction::FailTrigger(category);
        let Some(command) =
            command_guard::command_text(tool_name, input).filter(|_| category.is_grantable())
        else {
            return (blocked, None);
        };
        let Some((id, digest)) =
            current_trigger_id().zip(TRIGGER_DEFINITION.try_with(Clone::clone).ok())
        else {
            return (blocked, None);
        };
        let site = CommandSite::Workspace(self.workspace_path());
        let configs = self.trigger_configs.read().unwrap();
        let Some(config) = configs
            .get(&id)
            .filter(|config| config.definition_digest() == digest)
        else {
            return (blocked, None);
        };
        let once = crate::scheduler::user_tasks::run_program_grants();
        if trigger_program_covers(&config.program_grants, tool_name, command, site, paths)
            || trigger_program_covers(&once, tool_name, command, site, paths)
        {
            return (GuardAction::Proceed, None);
        }
        (
            blocked,
            trigger_program_pattern(tool_name, command, site, paths),
        )
    }

    #[allow(clippy::too_many_arguments)]
    async fn resolve_command_lane(
        &self,
        ctx: &mut CommandGuardCtx<'_>,
        tool_name: &str,
        input: &Value,
        thread_id: Uuid,
        cancel_token: &CancellationToken,
        paths: &command_guard::SecretPaths,
        verdict: StaticVerdict,
    ) -> Option<(JudgedClassification, String)> {
        match verdict {
            StaticVerdict::Settled(lane) => Some((
                JudgedClassification {
                    lane,
                    summary: None,
                    category: None,
                    judge_reason: None,
                },
                decision_trace("static", None, None),
            )),
            StaticVerdict::SettledIrreversible(category) => Some((
                JudgedClassification {
                    lane: RiskLane::IrreversibleDanger,
                    summary: Some(format!("May perform {}.", category.reason())),
                    category: Some(category),
                    judge_reason: None,
                },
                decision_trace(
                    if command_guard::command_text(tool_name, input)
                        .is_some_and(|text| paths.tool(tool_name, text))
                    {
                        "static-secret-path"
                    } else {
                        "static-side-effect"
                    },
                    None,
                    None,
                ),
            )),
            StaticVerdict::NeedsJudge(ji) => {
                tokio::select! {
                    biased;
                    _ = cancel_token.cancelled() => None,
                    resolved = self.judge_or_fallback(ctx, ji.clone(), thread_id) => {
                        let trace = decision_trace(
                            if ctx.judge_enabled { "judge" } else { "static-fallback" },
                            Some(&ji), resolved.judge_reason.as_deref(),
                        );
                        Some((resolved, trace))
                    },
                }
            }
        }
    }

    /// INVARIANT: Resolve one ambiguous command via the judge, memoized in the per-turn
    /// cache, including the distribution for each decision trace. With the judge
    /// off, the static fallback decides. With it on but
    /// failing, the command asks: a failure must not weaken the guard.
    ///
    /// `thread_id` is the turn the judge's own spend is recorded against. A
    /// cache hit spends nothing; the caller still records its decision trace.
    async fn judge_or_fallback(
        &self,
        ctx: &mut CommandGuardCtx<'_>,
        ji: JudgeInput,
        thread_id: Uuid,
    ) -> JudgedClassification {
        let key = ji.cache_key();
        if let Some(hit) = ctx.judge_cache.get(&key) {
            return hit.clone();
        }
        let site = CommandSite::Workspace(self.workspace_path());
        let resolved = if ctx.judge_enabled {
            judged(self.judge_command(&ji, thread_id).await)
        } else {
            command_guard::fallback_classify(&ji, site)
        };
        ctx.judge_cache.insert(key, resolved.clone());
        resolved
    }

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
        trace: &str,
        paths: &command_guard::SecretPaths,
        unproven_floor: bool,
    ) -> GuardDecision {
        let command = command_guard::command_text(tool_name, input)
            .unwrap_or_default()
            .to_string();

        let site = CommandSite::Workspace(self.workspace_path());
        let offer = if unproven_floor {
            GrantOffer::withheld(OfferWithheld::UnprovenWorkspaceDestruction)
        } else {
            let granted = tool_grants::granted_lines(
                &self.event_bus,
                GrantFile::AgentCommands,
                Some(thread_id),
            )
            .await;
            if command_is_allowed_with_paths(tool_name, &command, site, paths, |p| {
                granted.contains(p)
            }) {
                return GuardDecision::Proceed;
            }
            command_card_offer_with_paths(tool_name, &command, site, paths)
        };

        let canonical_input = serde_json::to_string(input).unwrap_or_else(|_| "{}".to_string());
        let dedup_key: DedupKey = (thread_id, tool_name.to_string(), canonical_input);
        let (request_id, mut rx, is_canonical) = {
            let mut pending = self.pending_command_permission.lock().unwrap();
            pending.register_or_attach(
                dedup_key,
                thread_id,
                tool_name.to_string(),
                input.clone(),
                offer.clone(),
            )
        };

        if is_canonical {
            let summary = summary_override
                .map(str::to_string)
                .unwrap_or_else(|| command_guard::permission_summary(tool_name, input));
            let command_for_event = crate::core::redact_postgres_secrets(&command);
            let request = ThreadEvent::CommandPermissionRequested {
                request_id: request_id.clone(),
                tool_use_id: tool_use_id.to_string(),
                tool_name: tool_name.to_string(),
                command: command_for_event,
                summary,
                grant_head: grant_head(tool_name, &command, site),
                grant_offer: Some(offer),
                decision_trace: Some(trace.to_string()),
            };
            let posted = card_queue::post_permission_prompt(
                &self.event_bus,
                thread_id,
                request,
                meta.clone(),
                Some(cancel_token),
            )
            .await;
            match posted {
                Ok(card_queue::PromptPost::Posted) => {}
                Ok(card_queue::PromptPost::Canceled) => {
                    self.pending_command_permission
                        .lock()
                        .unwrap()
                        .take(&request_id);
                    return GuardDecision::Refuse(canceled_refusal());
                }
                Err(e) => {
                    crate::log_legacy!("[CommandGuard] CommandPermissionRequested emit failed: {e}")
                }
            }
        }

        // INVARIANT: Block until the user answers (no timeout — the user is the rate-
        // limiter, same as CC) or the turn is canceled. The paired
        // `CommandPermissionResolved` for an Allow/Deny click is emitted by the
        // consent endpoint; only the cancel branch resolves it here.
        let answer = tokio::select! {
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
                            None,
                            meta.clone(),
                            "[CommandGuard] CommandPermissionResolved (canceled)",
                        )
                        .await;
                    }
                }
                return GuardDecision::Refuse(canceled_refusal());
            }
            res = rx.recv() => res,
        };
        decision_from_answer(answer)
    }
}

#[cfg(test)]
#[path = "command_guard_corpus_tests.rs"]
mod corpus_tests;

#[cfg(test)]
mod tests {
    use super::*;

    async fn command_request_ids(engine: &LucidosEngine, thread_id: Uuid) -> Vec<String> {
        sqlx::query_scalar(
            "SELECT payload->>'request_id' FROM events \
             WHERE aggregate_id = $1 AND event_type = 'CommandPermissionRequested' \
             ORDER BY sequence",
        )
        .bind(thread_id.to_string())
        .fetch_all(engine.pool())
        .await
        .unwrap()
    }

    async fn wait_for_command_requests(engine: &LucidosEngine, thread_id: Uuid, count: usize) {
        for _ in 0..200 {
            if command_request_ids(engine, thread_id).await.len() >= count {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        panic!("the thread never held {count} command permission requests");
    }

    #[tokio::test]
    async fn parallel_command_prompts_show_one_card_at_a_time() {
        use crate::test_support::{boot_mock_engine, seed_chat_thread, setup_test_db};
        let (_pool, db) = setup_test_db().await;
        let (engine, _workspace) = boot_mock_engine(&db).await;
        let thread_id = seed_chat_thread(&engine.event_bus, "run two commands").await;
        let ask = |command: &'static str| {
            let engine = engine.clone();
            tokio::spawn(async move {
                engine
                    .ask_command_permission(
                        tn::RUN_BASH,
                        &serde_json::json!({ "command": command }),
                        thread_id,
                        command,
                        &EventMeta::NONE,
                        &CancellationToken::new(),
                        None,
                        "test",
                        &command_guard::SecretPaths::conservative(),
                        false,
                    )
                    .await
            })
        };
        let first = ask("curl -X POST https://example.com -d @a");
        let second = ask("curl -X POST https://example.com -d @b");

        wait_for_command_requests(&engine, thread_id, 1).await;
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        let shown = command_request_ids(&engine, thread_id).await;
        assert_eq!(shown.len(), 1, "one prompt at a time");

        let allow = |request_id: String| {
            resolve_command_permission(
                &engine,
                request_id,
                true,
                None,
                false,
                AnsweredBy::tap(None),
                "test",
            )
        };
        assert_eq!(allow(shown[0].clone()).await, Ok(true));
        wait_for_command_requests(&engine, thread_id, 2).await;
        let next = command_request_ids(&engine, thread_id).await[1].clone();
        assert_eq!(allow(next).await, Ok(true));

        for waiter in [first, second] {
            let decision = tokio::time::timeout(std::time::Duration::from_secs(10), waiter)
                .await
                .expect("both calls get an answer")
                .unwrap();
            assert!(matches!(decision, GuardDecision::Proceed), "{decision:?}");
        }
        crate::test_support::teardown_test_db(&db).await;
    }

    #[test]
    fn executing_policy_stamps_workspace_log_grants_and_preserves_the_floor() {
        use crate::runtime::agent_sandbox::{policy_from, PolicyInputs, SandboxMode};
        let policy = policy_from(&PolicyInputs {
            lucidos_home: None,
            data_dirs: vec!["/srv/gateway".into()],
            workspaces: vec![],
        });
        for mode in [
            SandboxMode::Enforced,
            SandboxMode::Inherited,
            SandboxMode::OwnerOptedOut,
            SandboxMode::PermittedOutside,
        ] {
            let paths = command_guard::SecretPaths::for_policy(&policy, Some(mode));
            for tool in [
                tn::RUN_BASH,
                tn::RUN_BASH_BACKGROUND,
                tn::RUN_PYTHON,
                tn::RUN_PYTHON_BACKGROUND,
            ] {
                let bash = matches!(tool, tn::RUN_BASH | tn::RUN_BASH_BACKGROUND);
                for (path, open) in [
                    ("/srv/gateway/workspaces/demo/log", true),
                    ("/srv/gateway//workspaces/./demo/log", true),
                    ("/srv/gateway/workspaces/demo/../../config/key", false),
                    ("/srv/gateway/config/key", false),
                    (".lucidos/private/local-token", false),
                ] {
                    let text = if bash {
                        format!("grep warning {path}")
                    } else {
                        format!("print(open('{path}').read())")
                    };
                    let expected = open && mode == SandboxMode::Enforced;
                    let offer = command_card_offer_with_paths(tool, &text, TEXT_ONLY, &paths);
                    assert_eq!(
                        offer.session.is_some(),
                        expected,
                        "{mode:?}: {tool}: {text}"
                    );
                    assert_eq!(
                        command_is_allowed_with_paths(tool, &text, TEXT_ONLY, &paths, |_| true),
                        expected
                    );
                    if expected && bash {
                        assert_eq!(offer.narrow.as_deref(), Some("Bash(grep:*)"));
                    }
                }
            }
        }
    }

    const TEXT_ONLY: CommandSite<'static> = CommandSite::TextOnly { root: None };

    #[test]
    fn only_a_human_deny_reads_as_a_denial() {
        use tokio::sync::broadcast::error::RecvError;
        assert_eq!(
            decision_from_answer(Ok(PermissionAnswer::Allowed)),
            GuardDecision::Proceed
        );
        assert_eq!(
            decision_from_answer(Ok(PermissionAnswer::Denied)),
            GuardDecision::Refuse(denial_refusal())
        );
        for nobody_decided in [
            Ok(PermissionAnswer::Withdrawn(SUPERSEDED_REASON)),
            Err(RecvError::Closed),
        ] {
            assert_eq!(
                decision_from_answer(nobody_decided),
                GuardDecision::Refuse(withdrawn_refusal())
            );
        }
    }

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

    #[test]
    fn a_failed_post_image_drops_the_pair() {
        assert_eq!(
            checkpoint_outcome(Err("git write-tree exploded".to_string())),
            CheckpointOutcome::Discard(DiscardReason::PostImageFailed)
        );
    }

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
    fn a_headless_command_derives_no_session_pattern() {
        let cmd = "> /tmp/out";
        assert_eq!(command_guard::first_command_token(cmd), None);
        assert_eq!(
            derive_command_allow_pattern(tn::RUN_BASH, cmd, AllowScope::Session),
            None
        );
    }

    #[test]
    fn derive_pattern_strips_privilege_prefix_and_path() {
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
    fn a_root_cd_grant_stores_the_head_the_card_names() {
        let dir = tempfile::tempdir().unwrap();
        let site = CommandSite::Workspace(dir.path());
        let cmd = format!("cd {} && python3 x.py", dir.path().display());
        let head = grant_head(tn::RUN_BASH, &cmd, site);
        assert_eq!(head.as_deref(), Some("python3"));
        for scope in [AllowScope::Narrow, AllowScope::Session] {
            let source = grant_source(tn::RUN_BASH, &cmd, site);
            assert_eq!(
                derive_command_allow_pattern(tn::RUN_BASH, &source, scope),
                Some("Bash(python3:*)".to_string())
            );
        }
        let moved = format!("cd {}/sub && python3 x.py", dir.path().display());
        assert_eq!(
            grant_head(tn::RUN_BASH, &moved, site).as_deref(),
            Some("cd")
        );
    }

    #[test]
    fn grant_head_is_optional_on_the_wire() {
        let old = serde_json::json!({
            "type": "CommandPermissionRequested",
            "request_id": "r", "tool_use_id": "t", "tool_name": "run_bash",
            "command": "ls", "summary": "s",
        });
        let event: ThreadEvent = serde_json::from_value(old.clone()).unwrap();
        assert!(matches!(
            event,
            ThreadEvent::CommandPermissionRequested {
                grant_head: None,
                grant_offer: None,
                ..
            }
        ));
        assert_eq!(serde_json::to_value(&event).unwrap(), old);
    }

    #[test]
    fn a_command_card_offers_only_grants_that_cover_its_command() {
        let dir = tempfile::tempdir().unwrap();
        let site = CommandSite::Workspace(dir.path());
        let offer = |tool: &str, command: &str| command_card_offer(tool, command, site);

        let git = offer(tn::RUN_BASH, "git status");
        assert_eq!(git.broad.as_deref(), Some("Bash"));
        assert_eq!(git.narrow.as_deref(), Some("Bash(git:*)"));
        assert_eq!(git.session.as_deref(), Some("Bash(git:*)"));

        let chained = offer(tn::RUN_BASH, "git status && make deploy");
        assert_eq!(chained.broad.as_deref(), Some("Bash"));
        assert_eq!(chained.narrow, None, "Bash(git:*) does not cover `make`");
        assert_eq!(chained.session, None);

        let python = offer(tn::RUN_PYTHON, "print(1)");
        assert_eq!(python.broad.as_deref(), Some("Python"));
        assert_eq!(python.narrow, None, "Python has one line, the broad one");

        for scope in [AllowScope::Broad, AllowScope::Narrow, AllowScope::Session] {
            for (tool, command) in [(tn::RUN_BASH, "git status"), (tn::RUN_PYTHON, "print(1)")] {
                if let Some(line) = offer(tool, command).pattern(scope) {
                    assert!(command_is_allowed(tool, command, site, |p| p == line));
                }
            }
        }
    }

    #[test]
    fn a_secret_read_card_offers_no_grant_and_says_why() {
        let dir = tempfile::tempdir().unwrap();
        let site = CommandSite::Workspace(dir.path());
        for (tool, command) in [
            (tn::RUN_BASH, "cat ~/.lucidos/secret-keys/db"),
            (
                tn::RUN_PYTHON,
                "open('/home/u/.lucidos/pg-shared-password')",
            ),
        ] {
            assert_eq!(
                command_card_offer(tool, command, site),
                GrantOffer {
                    deny: Some(
                        if tool == tn::RUN_BASH {
                            "Bash(cat:*)"
                        } else {
                            "Python"
                        }
                        .into()
                    ),
                    ..GrantOffer::withheld(OfferWithheld::SecretRead)
                },
                "{tool}: {command}"
            );
        }
    }

    #[test]
    fn a_root_cd_before_nothing_keeps_its_head() {
        let dir = tempfile::tempdir().unwrap();
        let site = CommandSite::Workspace(dir.path());
        let cmd = format!("cd {} && > notes.txt", dir.path().display());
        assert_eq!(grant_head(tn::RUN_BASH, &cmd, site).as_deref(), Some("cd"));
        let cd = |p: &str| p == "Bash(cd:*)";
        assert!(command_is_allowed(tn::RUN_BASH, &cmd, site, cd));
    }

    #[test]
    fn python_code_keeps_its_text_and_names_no_head() {
        let dir = tempfile::tempdir().unwrap();
        let site = CommandSite::Workspace(dir.path());
        let code = format!("cd {} && python3 x.py", dir.path().display());
        assert_eq!(grant_source(tn::RUN_PYTHON, &code, site), code);
        assert_eq!(grant_head(tn::RUN_PYTHON, &code, site), None);
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
        let stored =
            derive_command_allow_pattern(tn::RUN_BASH, "git push", AllowScope::Narrow).unwrap();
        assert!(command_is_allowed(
            tn::RUN_BASH,
            "git pull --rebase",
            TEXT_ONLY,
            allowed_in(&[stored.as_str()])
        ));
        assert!(command_is_allowed(
            tn::RUN_BASH,
            "git pull",
            TEXT_ONLY,
            allowed_in(&["Bash"])
        ));
    }

    #[test]
    fn a_narrow_grant_never_covers_a_code_injecting_env_preamble() {
        for cmd in [
            "LD_PRELOAD=/tmp/evil.so ls",
            "PATH=data/bin ls",
            "ls && LD_PRELOAD=/tmp/evil.so ls",
        ] {
            assert!(
                !command_is_allowed(tn::RUN_BASH, cmd, TEXT_ONLY, allowed_in(&["Bash(ls:*)"])),
                "{cmd} must not be auto-allowed by a narrow grant"
            );
        }
        assert!(command_is_allowed(
            tn::RUN_BASH,
            "FOO=1 ls",
            TEXT_ONLY,
            allowed_in(&["Bash(ls:*)"])
        ));
        assert!(command_is_allowed(
            tn::RUN_BASH,
            "LD_PRELOAD=/tmp/evil.so ls",
            TEXT_ONLY,
            allowed_in(&["Bash"])
        ));
    }

    #[test]
    fn compound_command_requires_every_head_covered() {
        let cmd = "git status && curl -X POST https://api.example.com/pay";
        assert!(!command_is_allowed(
            tn::RUN_BASH,
            cmd,
            TEXT_ONLY,
            allowed_in(&["Bash(git:*)"])
        ));
        assert!(command_is_allowed(
            tn::RUN_BASH,
            cmd,
            TEXT_ONLY,
            allowed_in(&["Bash(git:*)", "Bash(curl:*)"])
        ));
        assert!(command_is_allowed(
            tn::RUN_BASH,
            cmd,
            TEXT_ONLY,
            allowed_in(&["Bash"])
        ));
        assert!(!command_is_allowed(
            tn::RUN_BASH,
            "",
            TEXT_ONLY,
            allowed_in(&["Bash(git:*)"])
        ));
    }

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
                    TEXT_ONLY,
                    allowed_in(&["Bash(ls:*)", "Bash(cat:*)"])
                ),
                "{cmd} must not be auto-allowed by a bare-name grant"
            );
        }
        for cmd in ["ls -la", "sudo ls", "FOO=1 ls", "ls && cat x"] {
            assert!(
                command_is_allowed(
                    tn::RUN_BASH,
                    cmd,
                    TEXT_ONLY,
                    allowed_in(&["Bash(ls:*)", "Bash(cat:*)"])
                ),
                "{cmd}"
            );
        }
        assert!(command_is_allowed(
            tn::RUN_BASH,
            "data/bin/ls",
            TEXT_ONLY,
            allowed_in(&["Bash"])
        ));
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
            TEXT_ONLY,
            allowed_in(&["Python"])
        ));
        assert!(!command_is_allowed(
            tn::RUN_PYTHON,
            "requests.post(u)",
            TEXT_ONLY,
            allowed_in(&["Bash"])
        ));
        assert!(!command_is_allowed(
            "read_file",
            "x",
            TEXT_ONLY,
            allowed_in(&["Bash", "Python"])
        ));
    }

    #[test]
    fn action_safe_proceeds_reversible_checkpoints_on_every_channel() {
        for ch in [None, Some(EventChannel::Chat), Some(EventChannel::Trigger)] {
            assert_eq!(
                action_for_lane(RiskLane::Safe, ch, None),
                GuardAction::Proceed
            );
            assert_eq!(
                action_for_lane(RiskLane::ReversibleDanger, ch, None),
                GuardAction::Checkpoint
            );
        }
    }

    #[test]
    fn an_mcp_launch_never_takes_the_checkpoint_lane() {
        assert_eq!(
            mcp_launch_lane(RiskLane::ReversibleDanger),
            RiskLane::IrreversibleDanger
        );
        assert_eq!(
            action_for_lane(mcp_launch_lane(RiskLane::ReversibleDanger), None, None,),
            GuardAction::Ask
        );
        for lane in [
            RiskLane::Safe,
            RiskLane::IrreversibleDanger,
            RiskLane::Catastrophic,
        ] {
            assert_eq!(mcp_launch_lane(lane), lane);
        }
    }

    #[test]
    fn action_catastrophic_refuses_on_every_channel() {
        for ch in [None, Some(EventChannel::Chat), Some(EventChannel::Trigger)] {
            assert_eq!(
                action_for_lane(RiskLane::Catastrophic, ch, None),
                GuardAction::Refuse
            );
        }
    }

    #[test]
    fn action_irreversible_asks_on_chat() {
        for channel in [None, Some(EventChannel::Chat)] {
            assert_eq!(
                action_for_lane(
                    RiskLane::IrreversibleDanger,
                    channel,
                    Some(SideEffectCategory::Email)
                ),
                GuardAction::Ask
            );
        }
    }

    #[test]
    fn action_irreversible_on_trigger_requires_a_program_grant() {
        let channel = Some(EventChannel::Trigger);
        for category in SideEffectCategory::GRANTABLE
            .into_iter()
            .chain([SideEffectCategory::SecretRead])
        {
            assert_eq!(
                action_for_lane(RiskLane::IrreversibleDanger, channel, Some(category)),
                GuardAction::FailTrigger(category)
            );
        }
        assert_eq!(
            action_for_lane(RiskLane::IrreversibleDanger, channel, None),
            GuardAction::FailTrigger(SideEffectCategory::Other)
        );
    }

    #[test]
    fn no_stored_grant_covers_a_secret_read_on_the_chat_lane() {
        let all = |_: &str| true;
        for (tool, text) in [
            (tn::RUN_BASH, "cat ~/.lucidos/secret-keys/db"),
            (
                tn::RUN_BASH_BACKGROUND,
                "cat $HOME/.lucidos/pg-shared-password",
            ),
            (
                tn::RUN_PYTHON,
                "open('/Users/me/.lucidos/secret-keys/db').read()",
            ),
            (
                tn::RUN_PYTHON_BACKGROUND,
                "open('/Users/me/.lucidos/pg-shared-password')",
            ),
        ] {
            assert!(
                !command_is_allowed(tool, text, TEXT_ONLY, all),
                "{tool}: {text}"
            );
        }
        assert!(command_is_allowed(
            tn::RUN_PYTHON,
            "print(1)",
            TEXT_ONLY,
            all
        ));
    }

    #[test]
    fn a_secret_read_is_detected_on_every_command_tool() {
        assert!(tool_call_reads_secret(
            tn::RUN_BASH,
            &serde_json::json!({ "command": "cat ~/.lucidos/secret-keys/db" })
        ));
        assert!(tool_call_reads_secret(
            tn::RUN_PYTHON,
            &serde_json::json!({ "code": "open('/home/u/.lucidos/pg-shared-password')" })
        ));
        assert!(!tool_call_reads_secret(
            tn::RUN_BASH,
            &serde_json::json!({ "command": "ls -la" })
        ));
    }

    #[test]
    fn retired_category_invocation_grants_never_authorize_commands() {
        use crate::triggers::config::{LegacyInvocationGrant, TriggerProgramGrant};

        for category in SideEffectCategory::GRANTABLE {
            let grant = TriggerProgramGrant {
                pattern: "Bash(curl:*)".into(),
                legacy: Some(LegacyInvocationGrant {
                    fingerprint: "original".into(),
                    category,
                }),
            };
            assert!(!trigger_program_covers(
                &[grant],
                tn::RUN_BASH,
                "curl -X POST https://example.com",
                TEXT_ONLY,
                &command_guard::SecretPaths::conservative(),
            ));
        }
    }

    fn bash_input(cmd: &str) -> Value {
        serde_json::json!({ "command": cmd })
    }

    #[test]
    fn an_ungrantable_refusal_offers_no_grant() {
        for (category, command) in [
            (
                SideEffectCategory::SecretRead,
                "cat ~/.lucidos/secret-keys/db",
            ),
            (
                SideEffectCategory::UnprovenWorkspaceDestruction,
                "make clean",
            ),
        ] {
            let msg = trigger_block_refusal(category, tn::RUN_BASH, &bash_input(command), None);
            assert!(msg.contains(ungrantable_remedy(category)), "{msg}");
            assert!(!msg.contains(TRIGGER_GRANT_REMEDY), "{msg}");
            assert!(!msg.contains("grant the"), "{msg}");
        }
    }

    #[test]
    fn trigger_block_refusal_names_what_was_tried() {
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
        assert!(msg.contains("was NOT run"), "{msg}");
        assert!(msg.contains(TRIGGER_GRANT_REMEDY), "{msg}");
    }

    #[test]
    fn trigger_block_refusal_without_a_judge_summary_still_names_the_command() {
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
            "a long script must not flood the failure transcript: {} bytes",
            msg.len()
        );
        assert!(msg.contains("..."), "truncation must be visible: {msg}");
        assert!(msg.contains(SideEffectCategory::Other.reason()), "{msg}");
    }

    #[test]
    fn trigger_block_refusal_puts_the_raw_command_last() {
        let msg = trigger_block_refusal(
            SideEffectCategory::ExternalApi,
            tn::RUN_BASH,
            &bash_input("curl -X POST https://api.example.com/pay"),
            Some("Charges a payment endpoint."),
        );
        let why = msg.find("Why it was gated").expect("summary line");
        let remedy = msg.find(TRIGGER_GRANT_REMEDY).expect("remedy line");
        let tried = msg.find("What it tried").expect("command block");
        assert!(why < remedy && remedy < tried, "{msg}");
    }

    #[test]
    fn trigger_block_refusal_fences_a_command_containing_backticks() {
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
    fn a_failed_judge_call_leaves_no_provider_text_in_the_trace() {
        let err: Box<dyn std::error::Error + Send + Sync> =
            "400 bad request: rm -rf /home/u/private-token".into();
        let verdict = JudgeVerdict::judge_failed(err.as_ref());
        let trace = decision_trace("judge", None, Some(&verdict.reason));
        assert!(
            trace.ends_with(&format!("judge={JUDGE_FAILED_REASON}")),
            "{trace}"
        );
        assert!(!trace.contains("private-token"), "{trace}");
    }

    #[test]
    fn decision_trace_keeps_evidence_and_distribution_without_command_text() {
        let input = JudgeInput {
            tool_name: tn::RUN_BASH.to_string(),
            command: "curl https://example.test/private-token".to_string(),
            out_of_workspace: true,
            fast_path_refused: false,
            script_sources: vec![],
        };
        let classification = judged(Ok(JudgeVerdict {
            lane: RiskLane::Safe,
            category: None,
            summary: "Safe".to_string(),
            reason: "safe 0.90, reversible 0.05, irreversible 0.05".to_string(),
        }));
        let trace = decision_trace(
            "judge",
            Some(&input),
            classification.judge_reason.as_deref(),
        );
        assert!(trace.contains("layer=judge"));
        assert!(trace.contains("target_outside_workspace=true"));
        assert!(trace.contains("fast_path_refused=false"));
        assert!(trace.contains("safe 0.90"));
        assert!(!trace.contains("private-token"));
        let mut cache = HashMap::new();
        cache.insert(input.cache_key(), classification);
        let cached = cache.get(&input.cache_key()).unwrap();
        assert_eq!(
            decision_trace("judge", Some(&input), cached.judge_reason.as_deref()),
            trace
        );
    }

    #[test]
    fn decision_trace_survives_permission_event_roundtrip_and_old_events() {
        let old = serde_json::json!({
            "type": "CommandPermissionRequested",
            "request_id": "r", "tool_use_id": "t", "tool_name": "run_bash",
            "command": "ls", "summary": "s",
        });
        let event: ThreadEvent = serde_json::from_value(old.clone()).unwrap();
        assert_eq!(serde_json::to_value(event).unwrap(), old);
        let mut traced = old;
        traced["decision_trace"] = decision_trace("static", None, None).into();
        let event: ThreadEvent = serde_json::from_value(traced.clone()).unwrap();
        assert_eq!(serde_json::to_value(event).unwrap(), traced);
    }

    #[test]
    fn a_judge_that_never_answered_asks() {
        let c = judged(Err("network down".into()));
        assert_eq!(c.lane, RiskLane::IrreversibleDanger);
        assert_eq!(c.category, Some(SideEffectCategory::Other));
        assert!(c
            .judge_reason
            .as_deref()
            .is_some_and(|r| r.contains("network down")));
        assert!(
            c.summary
                .as_deref()
                .is_some_and(|s| s.contains("network down")),
            "{c:?}"
        );
    }

    #[test]
    fn judge_off_path_uses_static_fallback_classifier() {
        let ji = JudgeInput {
            tool_name: tn::RUN_BASH.to_string(),
            command: "curl -X POST https://api/charge".to_string(),
            out_of_workspace: false,
            fast_path_refused: false,
            script_sources: vec![],
        };
        let c = command_guard::fallback_classify(&ji, CommandSite::TextOnly { root: None });
        assert_eq!(c.lane, RiskLane::IrreversibleDanger);
        assert_eq!(c.category, Some(SideEffectCategory::ExternalApi));
    }

    #[tokio::test]
    async fn a_standing_deny_beats_a_standing_allow_on_the_agent_lanes() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = crate::test_support::seed_chat_thread(&bus, "post it").await;
        let root = tempfile::TempDir::new().unwrap();
        let site = CommandSite::Workspace(root.path());
        let command = "curl -X POST https://example.com";
        let offer = command_card_offer(tn::RUN_BASH, command, site);
        assert_eq!(offer.deny, offer.narrow, "one scope derivation for both");
        let line = offer.deny.expect("a curl command derives a deny line");

        let by = tool_grants::ChangedBy {
            source: crate::core::grants::GrantSource::Card,
            thread_id: Some(thread_id),
            actor: None,
        };
        tool_grants::change_standing(
            &bus,
            GrantFile::AgentCommands,
            tool_grants::StandingChange::Add(std::slice::from_ref(&line)),
            by,
        )
        .await
        .unwrap();
        let input = serde_json::json!({ "command": command });
        assert_eq!(
            standing_deny_refusal(&pool, tn::RUN_BASH, &input).await,
            None
        );

        standing_deny::record(&bus, thread_id, line.clone(), AnsweredBy::tap(None))
            .await
            .unwrap();
        for tool in [tn::RUN_BASH, tn::RUN_BASH_BACKGROUND] {
            let refusal = standing_deny_refusal(&pool, tool, &input)
                .await
                .unwrap_or_else(|| panic!("{tool} is denied"));
            assert!(refusal.contains(&line), "{refusal}");
        }
        let code = serde_json::json!({ "code": "print(1)" });
        assert_eq!(
            standing_deny_refusal(&pool, tn::RUN_PYTHON, &code).await,
            None
        );
        assert_eq!(
            standing_deny_refusal(&pool, tn::SEND_NOTIFICATION, &input).await,
            None
        );
        crate::test_support::teardown_test_db(&db_name).await;
    }

    #[test]
    fn a_secret_read_still_offers_always_deny() {
        let root = tempfile::TempDir::new().unwrap();
        let site = CommandSite::Workspace(root.path());
        let offer = command_card_offer(tn::RUN_BASH, "cat ~/.lucidos/private/pg-password", site);
        assert_eq!(offer.withheld, Some(OfferWithheld::SecretRead));
        assert_eq!(offer.narrow, None);
        assert_eq!(offer.deny.as_deref(), Some("Bash(cat:*)"));
    }

    #[test]
    fn an_unproven_floor_match_offers_no_allow() {
        let root = tempfile::TempDir::new().unwrap();
        let site = CommandSite::Workspace(root.path());
        for (tool, text) in [
            (
                tn::RUN_BASH,
                "cat > fixture.rs <<'EOF'\npg_ctl -D /tmp/lucidos_example stop\nEOF\ncargo fmt --all",
            ),
            (
                tn::RUN_PYTHON,
                "import os\nos.posix_spawn('curl', ['-X', 'POST', 'http://localhost/~/api/v1/control/cleanup/batches'], {})",
            ),
        ] {
            let offer = command_card_offer(tool, text, site);
            assert_eq!(
                offer.withheld,
                Some(OfferWithheld::UnprovenWorkspaceDestruction),
                "{tool}"
            );
            assert_eq!((offer.broad, offer.narrow, offer.session), (None, None, None));
            assert!(!command_is_allowed(tool, text, site, |_| true), "{tool}");
        }
    }

    #[tokio::test]
    async fn a_setup_maps_to_the_fingerprint_its_start_checks() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let root = tempfile::TempDir::new().unwrap();
        let cwd = std::env::current_dir().unwrap();
        let input = serde_json::json!({
            "id": "srv", "name": "Server", "command": "./server",
            "args": ["--port", "1"], "env": { "TOKEN": "s3cret-value" },
        });
        let launch = guarded_mcp_launch(
            &pool,
            root.path(),
            Some(cwd.clone()),
            false,
            tn::SETUP_MCP_SERVER,
            &input,
        )
        .await
        .expect("a setup is classified")
        .expect("a known working directory composes");
        assert_eq!(launch.lead, MCP_SETUP_SUMMARY);

        let env = HashMap::from([(
            "TOKEN".to_string(),
            crate::core::McpEnvValue::Plain("s3cret-value".into()),
        )]);
        let args = ["--port".to_string(), "1".to_string()];
        crate::core::McpServerStore::register(
            &pool, &bus, "srv", "Server", "./server", &args, &env, None,
        )
        .await
        .unwrap();
        let server = crate::core::McpServerStore::get(&pool, "srv")
            .await
            .unwrap()
            .unwrap();
        let manager = crate::mcp::McpManager::new(pool.clone(), bus, root.path().to_path_buf());
        assert_eq!(
            launch.fingerprint,
            manager.launch_fingerprint(&server).unwrap()
        );
        crate::test_support::teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn an_unapproved_agent_start_maps_to_its_masked_line() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let root = tempfile::TempDir::new().unwrap();
        let cwd = std::env::current_dir().unwrap();
        let env = HashMap::from([(
            "TOKEN".to_string(),
            crate::core::McpEnvValue::Plain("s3cret-value".into()),
        )]);
        crate::core::McpServerStore::register(&pool, &bus, "srv", "Server", "npx", &[], &env, None)
            .await
            .unwrap();
        let launch_of = |guard_on: bool, input: Value| {
            let (pool, root, cwd) = (pool.clone(), root.path().to_path_buf(), cwd.clone());
            async move { guarded_mcp_launch(&pool, &root, Some(cwd), guard_on, tn::MCP, &input).await }
        };
        let enable = serde_json::json!({ "action": "enable", "id": "srv" });
        let start = serde_json::json!({ "action": "start", "id": "srv" });

        for input in [&enable, &start] {
            let launch = launch_of(true, input.clone()).await.unwrap().unwrap();
            assert_eq!(launch.lead, MCP_START_SUMMARY);
            assert!(launch.line.contains("<stored value>"), "{}", launch.line);
            assert!(!launch.line.contains("s3cret-value"), "{}", launch.line);
        }
        assert!(launch_of(false, enable.clone()).await.is_none());
        let missing = serde_json::json!({ "action": "enable", "id": "nope" });
        assert!(launch_of(true, missing).await.is_none());

        let launch = launch_of(true, enable.clone()).await.unwrap().unwrap();
        crate::core::McpServerStore::approve_launch(
            &pool,
            &bus,
            &launch.server_id,
            &launch.fingerprint,
            None,
        )
        .await
        .unwrap();
        assert!(launch_of(true, enable).await.is_none());
        assert!(launch_of(true, start).await.is_none());
        crate::test_support::teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn an_unproven_mcp_launch_cannot_create_a_persistent_approval() {
        use crate::test_support::{boot_mock_engine, seed_chat_thread, setup_test_db};
        let (_pool, db) = setup_test_db().await;
        let (engine, _workspace) = boot_mock_engine(&db).await;
        let thread_id = seed_chat_thread(&engine.event_bus, "set up a server").await;
        let input = serde_json::json!({
            "action": "setup",
            "id": "srv",
            "name": "Server",
            "command": "fixture-server",
            "args": ["/~/api/v1/control/trash/empty"]
        });
        let mut ctx = CommandGuardCtx {
            enabled: true,
            judge_enabled: false,
            judge_cache: &mut HashMap::new(),
        };
        let decision = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            engine.command_guard_decision(
                &mut ctx,
                tn::MCP,
                &input,
                thread_id,
                "setup",
                &EventMeta::NONE,
                &CancellationToken::new(),
                &command_guard::SecretPaths::conservative(),
            ),
        )
        .await
        .expect("an unproven launch must refuse without a card");
        assert!(
            matches!(decision, GuardDecision::Refuse(ref reason) if reason.contains("Allow once")),
            "{decision:?}"
        );
        assert!(command_request_ids(&engine, thread_id).await.is_empty());
        crate::test_support::teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn an_unapproved_start_of_a_legacy_command_holding_equals_is_refused() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let root = tempfile::TempDir::new().unwrap();
        let cwd = std::env::current_dir().unwrap();
        crate::core::McpServerStore::register(
            &pool,
            &bus,
            "srv",
            "Server",
            "MODE=x/server",
            &[],
            &HashMap::new(),
            None,
        )
        .await
        .unwrap();
        let input = serde_json::json!({ "action": "start", "id": "srv" });
        let refusal = guarded_mcp_launch(&pool, root.path(), Some(cwd), true, tn::MCP, &input)
            .await
            .expect("an unapproved start is classified")
            .err()
            .expect("a command holding '=' is refused");
        assert!(refusal.contains("was NOT started"), "{refusal}");
        assert!(refusal.contains("set it up again"), "{refusal}");
        crate::test_support::teardown_test_db(&db_name).await;
    }
}
