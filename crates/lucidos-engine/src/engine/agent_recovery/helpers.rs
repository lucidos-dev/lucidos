//! Recovery reason constants and free helper functions used across the
//! agent-recovery phases. Re-exported from mod.rs so existing
//! `agent_recovery::X` paths keep resolving.

use super::super::agent_session::CodingAgentKind;
use super::super::event_bus::{BusEvent, EventBus};
use super::super::git_ops::git_cmd;
use super::super::thread_events::{
    EngineReason, EventChannel, EventMeta, MessageOrigin, ThreadEvent,
};
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// Re-emit `ChangeProposed` flipping the row to `incomplete: true`. Called
/// by orphan recovery for branches that were mid-CC-turn at engine restart
/// — the existing pending row was populated by per-commit emits during the
/// dying turn, so its description / files reflect work the user never
/// confirmed. Apply must require explicit confirmation; the `incomplete`
/// flag is the existing signal for that (see commit `1e8736839`).
///
/// Caller passes the already-loaded `Change` so this avoids re-querying
/// what recover_orphaned_worktrees just fetched via `list_pending`. No-op
/// when the row is already flagged.
pub(crate) async fn mark_pending_change_incomplete(
    event_bus: &EventBus,
    thread_id: Uuid,
    change: &crate::core::changes::Change,
) {
    if change.incomplete {
        return;
    }
    event_bus
        .emit_or_log(
            BusEvent::Thread {
                thread_id,
                event: ThreadEvent::ChangeProposed {
                    change_id: change.id.to_string(),
                    description: Some(change.description.clone()),
                    files: change.files.clone(),
                    requires_restart: change.requires_restart,
                    origin: Some(MessageOrigin::engine(EngineReason::OrphanRecovery)),
                    commit_sha: None,
                    branch_name: change.branch_name.clone(),
                    repo_root: change.repo_root.clone(),
                    hardened: change.hardened,
                    incomplete: true,
                    path: String::new(),
                    diff: String::new(),
                },
                meta: EventMeta {
                    channel: Some(EventChannel::ClaudeCode),
                    ..EventMeta::NONE
                },
            },
            "[Recovery] ChangeProposed re-emit marking incomplete",
        )
        .await;
}

/// Tag stamped on `CodingAgentIdled.reason` when the engine surfaces a
/// mid-turn-crashed Claude Code session as "interrupted, click to continue" instead of
/// auto-resuming it. The frontend reads this constant to render the continue
/// affordance; the spawn dispatcher ignores `CodingAgentIdled` entirely so
/// the user's click is what produces the next spawn (via `ContinuationRequested`).
pub const ENGINE_RESTART_INTERRUPT_REASON: &str = "engine_restart_interrupt";

/// Tag stamped on `ContinuationRequested.reason` when the user clicks the
/// "click to continue" affordance after a mid-turn interrupt. The continue
/// endpoint emits with this reason; the spawn dispatcher classifies it as a
/// `SpawnTrigger::ContinuationRequested` and starts the next CC turn.
pub const USER_CLICKED_CONTINUE_REASON: &str = "user_clicked_continue";

/// Tag stamped on `ContinuationRequested.reason` when the user answers an
/// `AskUserQuestion` after the coding-agent subprocess has been torn down at
/// idle. `notify()` is a no-op in that window, so this signal makes the spawn
/// dispatcher boot a fresh `--resume` subprocess.
///
/// **The resume carries the answer itself** ([`continue_input_for_reason`]).
/// This constant used to promise that "the resumed agent re-runs the hook,
/// which reads the persisted answer from the DB", and that is not true: on
/// teardown Claude Code closes the pending `AskUserQuestion` in its OWN
/// transcript with a `tool_result` reading "the tool use was rejected", so the
/// resumed session has no dangling tool call to re-run, the hook never fires,
/// and `walk_question_batch`'s crash-recovery lookup never gets a chance. The
/// answer reached nobody, and the model was left with a rejection stamp next to
/// a bare "continue" (2026-08-10, thread `728de3cc`: it read the pair as
/// approval and started implementing).
pub const ANSWERED_AFTER_IDLE_REASON: &str = "answered_after_idle";

/// Tag stamped on `ContinuationRequested.reason` when the hung-subprocess watchdog
/// killed CC after silence past its inactivity limit. Same downstream
/// pipeline as `USER_CLICKED_CONTINUE_REASON` — only the boundary-exchange
/// gate (`continue_should_open_resume_exchange`) reads the reason — so the
/// user gets an automatic `--resume` without having to click Continue.
pub const AUTO_RECOVERY_AFTER_HANG_REASON: &str = "auto_recovery_after_hang";

/// Tag stamped on `ContinuationRequested.reason` when the backend ended a turn
/// on a TRANSIENT upstream API failure (its own `API Error: …` message, e.g. a
/// connection closed mid-response) and the engine resumes the session instead of
/// leaving the thread dead behind a red dot.
///
/// This closes an asymmetry, not a new policy: when the same network failure
/// manifests as SILENCE, the hung-subprocess watchdog already kills the
/// subprocess and auto-resumes via `AUTO_RECOVERY_AFTER_HANG_REASON`. Only the
/// case where the backend notices the drop first, and reports it, had no
/// recovery, so two unattended nightly runs sat dead for four and eight hours on
/// 2026-08-04 waiting for a human to type anything at all.
///
/// Unlike the watchdog reasons this one is BOUNDED
/// (`MAX_API_ERROR_AUTO_RESUMES`), because the trigger is a failure the backend
/// might reproduce immediately: a persistently broken upstream must surface,
/// not loop. See `auto_resume_after_api_error`.
pub const AUTO_RESUME_AFTER_API_ERROR_REASON: &str = "auto_resume_after_api_error";

/// Tag stamped on `ContinuationRequested.reason` when recovery auto-resumes an
/// in-flight coding-agent thread after a **user-initiated** *Switch to new
/// version* (detected by a device-attributed teardown `ResponseAborted`). A crash
/// leaves no such boundary → the thread gets the manual "Continue" affordance
/// instead, so work that may have crashed the engine can't loop. Opens a
/// "Resumed" boundary exchange like the other automatic-resume reasons.
pub const AUTO_RESUME_AFTER_SWITCH_REASON: &str = "auto_resume_after_switch";

/// Should the SpawnConsumer's `Continue` handler emit `ContinuationStarted`
/// for a `ContinuationRequested` carrying this `reason`?
///
/// `ContinuationStarted` opens a new "Resumed after engine restart" exchange
/// in the timeline. That label is only honest when the continuation is in
/// response to an actual mid-turn engine restart (user clicked Continue) or
/// an engine-driven auto-resume after a hung-subprocess watchdog fire — both
/// cases the user should see as a fresh boundary in the timeline. For
/// `answered_after_idle` the user answered an `AskUserQuestion` after CC's
/// subprocess was torn down at idle — the follow-up CC events should attach
/// to the existing `UserQuestionAsked` exchange instead of being mislabeled
/// as a recovery.
///
/// `auto_resume_after_api_error` opts in for the same reason the watchdog does:
/// the turn genuinely ended (its `ResponseFailed` is already in the timeline),
/// and what follows is a new attempt at the same work, which the user should see
/// as its own boundary rather than as text appended to the failed answer.
///
/// Default-deny on unknown / missing reasons: a future `ContinuationRequested`
/// reason must opt-in explicitly rather than inheriting a "Resumed after
/// engine restart" boundary by accident.
pub fn continue_should_open_resume_exchange(reason: Option<&str>) -> bool {
    matches!(
        reason,
        Some(USER_CLICKED_CONTINUE_REASON)
            | Some(AUTO_RECOVERY_AFTER_HANG_REASON)
            | Some(AUTO_RESUME_AFTER_SWITCH_REASON)
            | Some(AUTO_RESUME_AFTER_API_ERROR_REASON)
    )
}

/// What the spawn consumer must do after a `SpawnRequest::Continue`'s
/// `run_direct_agent` returns. See [`continue_recovery`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContinueRecovery {
    /// The turn ran; the run loop emitted its own terminal. Nothing to do.
    Nothing,
    /// Stale resume on the FIRST attempt — re-run once with a fresh session
    /// (no resume sid) and the conversation reconstructed into the prompt.
    RetryFresh,
    /// Errored with no retry left. Settle the projection so the thread cannot
    /// sit at `running` with no live subprocess.
    Settle,
}

/// Decide the spawn consumer's next move for a continuation, from the error the
/// run returned (if any) and whether the fresh-session retry has already been
/// spent.
///
/// Extracted as a pure function because the consumer's async shell needs a full
/// `LucidosEngine` and cannot be exercised in a test — same reason
/// [`continue_should_open_resume_exchange`] and
/// `external_watchdog::external_watchdog_decision` are shaped this way. The
/// wiring bug this encodes was invisible precisely because the decision lived
/// inline in that untestable shell.
///
/// Two properties are load-bearing:
///
/// - **A stale resume MUST be retried, not just logged.** `run_session` bails on
///   a stale resume WITHOUT a terminal event — deliberately, so the projection
///   stays `running` across the retry window instead of flashing "Aborted". That
///   only holds if a retry actually follows. When it didn't, thread `cb503361`
///   wedged at `running` for 8 minutes (2026-07-29) with no live subprocess: the
///   stale-resume arm also drops the `agent_sessions` entry, and that map is the
///   only thing `ExternalWatchdog` scans.
/// - **The retry is one-shot.** `retried == true` settles even on another
///   `STALE_RESUME_ERROR`, so an engine-driven continuation can never loop. (It
///   is unreachable anyway — the retry passes no resume sid and
///   `is_stale_resume_signal` requires one — but crash-safety here is a floor,
///   not an inference.)
///
/// The ONE error that must NOT settle is `AGENT_ALREADY_RUNNING_ERROR`: the
/// spawn guard rejected us because a **live** session already owns the thread,
/// so this continuation owns nothing and the `running` projection is TRUE — it
/// belongs to the turn that won the race. Settling there would emit a terminal
/// against a working session and make the projection lie in the opposite
/// direction, which is strictly worse than the wedge this backstop exists to
/// prevent. Reachable whenever a continuation races a user message: the
/// consumer dispatches off an event subscriber with no lock on the thread.
///
/// Every other error settles: the failure modes that skip their own terminal are
/// exactly the ones we cannot enumerate, and settling is idempotent
/// (`settle_stuck_running_thread` re-checks `running` first).
pub fn continue_recovery(error: Option<&str>, retried: bool) -> ContinueRecovery {
    match error {
        None => ContinueRecovery::Nothing,
        Some(e) if e == crate::engine::claude_code::AGENT_ALREADY_RUNNING_ERROR => {
            ContinueRecovery::Nothing
        }
        Some(e) if !retried && e == crate::engine::claude_code::STALE_RESUME_ERROR => {
            ContinueRecovery::RetryFresh
        }
        Some(_) => ContinueRecovery::Settle,
    }
}

/// User message the spawn consumer hands to `run_direct_agent` when actuating
/// a `SpawnRequest::Continue`. **Must be non-empty.**
///
/// `claude --print --resume` reads stdin in stream-json mode and waits
/// indefinitely for at least one input line before emitting its `system/init`
/// event. The engine keeps the input channel open across the session lifetime,
/// so EOF never arrives on its own — without an explicit input, CC parks
/// forever, `events_rx.recv()` never resolves, and the thread sits "Running"
/// until the next engine restart tears the subprocess down.
///
/// The string mirrors the placeholder CC itself injects on `--resume` of an
/// unfinished tool_use (see `agent_session/run_session.rs` and
/// `agent_session/reconstruct.rs`), so CC ingests it as a plain user turn and
/// proceeds against the resumed conversation state. The richer recovery
/// payload (system-prompt override, pending-merge context, etc.) will replace
/// this call site later; until then this constant guarantees the non-empty
/// stdin precondition.
pub const CONTINUE_RESUME_USER_MESSAGE: &str = "Continue from where you left off.";

/// Opens the [`ANSWERED_AFTER_IDLE_REASON`] resume message. Its whole job is to
/// disarm what the agent is about to read in its own transcript.
///
/// On teardown Claude Code stamps a pending question tool call as
/// `"The user doesn't want to proceed with this tool use. The tool use was
/// rejected... STOP what you are doing"` with `toolDenialKind: "user-rejected"`.
/// That text is inside the CC binary and inside CC's private session JSONL, so
/// the engine can neither suppress it nor rewrite it. It can only refuse to
/// leave it as the sole account of what happened: an engine restart is not the
/// user declining anything.
const ANSWER_OUT_OF_BAND_NOTE: &str =
    "[Note from engine: the user answered your question, but your session was no longer \
     running when they did, so their answer could not come back to you as the tool result. \
     Your transcript may show that question tool call as rejected, denied or interrupted, \
     possibly with a line claiming the user did not want to proceed. That line is an engine \
     teardown artifact and it is false: the user declined nothing. They did not approve \
     anything either. Their actual answer follows.]";

/// Closes the [`ANSWERED_AFTER_IDLE_REASON`] resume message. Names the one
/// inference that has to be blocked: "the card is closed and I was told to
/// continue, so I may proceed with what I was about to do".
const ANSWER_OUT_OF_BAND_TRAILER: &str =
    "Continue the turn from that answer, and from nothing else. Do not re-ask the same \
     question. Do not read the interrupted tool call as approval, as a refusal, or as \
     permission to carry on with what you were doing before you asked.";

/// The user message the spawn consumer hands a `SpawnRequest::Continue`,
/// chosen by the `ContinuationRequested.reason` that produced it.
///
/// Only [`ANSWERED_AFTER_IDLE_REASON`] differs: that continuation exists
/// *because* the user answered a question, so the message carries the answer
/// (see [`crate::engine::agent_question::answered_question_recap`] for why the
/// normal in-band delivery cannot reach this agent). Every other reason, and a
/// thread with no answered question to recap, gets today's
/// [`CONTINUE_RESUME_USER_MESSAGE`] unchanged.
///
/// **Never returns an empty string**, whichever branch runs: an empty stdin
/// parks `claude --print --resume` forever and zombies the thread.
pub(crate) async fn continue_input_for_reason(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    reason: Option<&str>,
) -> String {
    if reason != Some(ANSWERED_AFTER_IDLE_REASON) {
        return CONTINUE_RESUME_USER_MESSAGE.to_string();
    }
    match crate::engine::agent_question::answered_question_recap(pool, thread_id).await {
        Some(recap) => {
            format!("{ANSWER_OUT_OF_BAND_NOTE}\n\n{recap}\n{ANSWER_OUT_OF_BAND_TRAILER}")
        }
        None => {
            // Nothing to recap on an answered-after-idle continuation means the
            // answer we were spawned for is not readable back. Say so: the
            // agent still resumes, but the silence lands in the log rather than
            // only in the model's guesswork.
            crate::log!(
                "[CCQuestion] answered_after_idle continuation for thread {} has no \
                 recoverable answer, resuming with the bare continue message",
                thread_id
            );
            CONTINUE_RESUME_USER_MESSAGE.to_string()
        }
    }
}

/// Input for the spawn consumer's **one-shot stale-resume retry** of a
/// `SpawnRequest::Continue`: the thread's conversation reconstructed from events,
/// followed by [`CONTINUE_RESUME_USER_MESSAGE`].
///
/// The retry runs with `resume_session_id: None` — the sid we just proved dead
/// cannot be reused — so the fresh subprocess starts with zero context. Without
/// the recap it would "continue from where you left off" with no idea where that
/// was. Same shape as the chat handler's stale-resume retry
/// (`chat::process_cc`), which is the path this one was missing.
///
/// The tail is [`continue_input_for_reason`], not the bare constant, so an
/// `answered_after_idle` retry still carries the user's answer. This path needs
/// it MORE than the ordinary resume does, not less: `reconstruct_summary` does
/// not project `UserQuestionAsked` / `UserQuestionAnswered` at all (see
/// `fetch_relevant_events`), and the retry has no session to resume, so without
/// the tail nothing anywhere in the fresh subprocess's context mentions that a
/// question was asked, let alone what the user replied. A question-parked thread
/// is also the likeliest one to hit a stale sid in the first place, having sat
/// idle long enough for the transcript to age out.
pub(crate) async fn continue_retry_input(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    reason: Option<&str>,
) -> String {
    let tail = continue_input_for_reason(pool, thread_id, reason).await;
    crate::engine::agent_session::prepend_reconstruction(pool, thread_id, &tail).await
}

/// Remove a stale (duplicate-recovery) worktree, deleting its branch ONLY when
/// fully merged (no unique commits). This is the duplicate-branch recovery path
/// (two branches map to one thread from stale-resume retries), and the
/// duplicate's unique commits must survive. It delegates to the cron's safe
/// helper, which keeps a branch with unique commits and keeps it on error too.
///
/// The dirtiness gate is the same one every `WorktreeCleanup` caller applies
/// before that helper, and it belongs here for the same reason. Which of the
/// two worktrees is stale is a guess. The helper opens with
/// `git worktree remove --force`, so an uncommitted edit in the loser goes with
/// it. `is_worktree_dirty` answers "dirty" when git could not say, so an
/// unreadable tree is kept. Keeping it costs one skipped cleanup, and the next
/// restart tries again.
pub(crate) async fn cleanup_stale_worktree(wt_path: &Path) {
    if crate::engine::worktree_cleanup::is_worktree_dirty(wt_path).await {
        log!(
            "[Recovery] Keeping duplicate worktree {}: it has uncommitted changes",
            wt_path.display()
        );
        return;
    }
    match crate::engine::worktree_cleanup::remove_worktree_and_optionally_delete_branch(
        wt_path, None,
    )
    .await
    {
        Some(outcome) => log!(
            "[Recovery] Removed duplicate worktree {} ({} bytes, branch_deleted={})",
            wt_path.display(),
            outcome.freed_bytes,
            outcome.branch_deleted
        ),
        None => log!(
            "[Recovery] Could not remove duplicate worktree {}: it is skipped until the next restart",
            wt_path.display()
        ),
    }
}

/// Resolve which thread an orphaned coding-agent worktree (found by the
/// recovery scan) should be surfaced under as a resumable Claude Code session.
/// Returns the thread that a recorded `SessionStarted` / pending change maps
/// `branch_name` to, or `None` when no thread owns the branch.
///
/// `None` means **skip**. A worktree whose originating thread is gone (its
/// events were pruned, leaving only the directory on disk) has no conversation
/// to continue. Recovery used to fabricate a fresh `Uuid` here and emit
/// `CodingAgentIdled` to it — but a thread with no projection row defaults to
/// `Chat` / `Archived` in the lifecycle gate, which rejects `CodingAgentIdled`
/// as CC-only. The emit failed on *every* engine restart (spamming the audit
/// log with `Thread lifecycle violation: 'CodingAgentIdled' is not valid for
/// Chat threads`) and never actually surfaced anything resumable. The orphaned
/// worktree is left for the worktree-cleanup worker to reclaim on its own
/// schedule instead.
///
/// Pure decision — exposed at module scope so the skip behaviour is unit-tested
/// without a live engine + git worktree (mirrors `classify_archive_decision`).
pub(crate) fn orphan_recovery_target(
    branch_to_thread: &std::collections::HashMap<String, Uuid>,
    branch_name: &str,
) -> Option<Uuid> {
    branch_to_thread.get(branch_name).copied()
}

/// Every git repo the orphan-recovery sweep scans, deduplicated by canonical
/// path. The `String` is the registry id the worktree marker records, and is
/// `None` for the two repos that are in no registry.
///
/// Three sources, in scan order:
///
/// * The **Lucidos source repo**: where a `Lucidos`-kind thread works.
/// * The **workspace repo**: where an `App`-kind thread works. Both its
///   worktree and its branch belong to the workspace git, which no registry
///   lists. Omitting it hid every app thread from this sweep, so a user switch
///   never auto-resumed one
///   (`docs/plans/2026-08-21-app-threads-never-auto-resume-after-a-user-restart.md`).
/// * Every **registered external repo**: where an `External`-kind thread works.
///
/// Order is load-bearing. The lost-branch fallback takes the first repo whose
/// refs hold the branch, and a duplicate root would recover one worktree twice.
/// Either engine-owned root can ALSO sit in the registry, so a registered repo
/// that canonicalizes onto an earlier entry is dropped and logged.
///
/// Pure, so the composition is unit-tested without an engine or a DB.
pub(crate) fn recovery_repo_roots(
    lucidos_repo_root: &Path,
    workspace_root: &Path,
    external_repos: &[crate::core::repositories::Repository],
) -> Vec<(PathBuf, Option<String>)> {
    let canon = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());

    let mut seen: std::collections::HashSet<PathBuf> =
        std::collections::HashSet::from([canon(lucidos_repo_root)]);
    let mut roots: Vec<(PathBuf, Option<String>)> = vec![(lucidos_repo_root.to_path_buf(), None)];

    if seen.insert(canon(workspace_root)) {
        roots.push((workspace_root.to_path_buf(), None));
    }

    for repo in external_repos {
        let path = PathBuf::from(&repo.path);
        if !path.exists() {
            log!(
                "[Recovery] Skipping external repo '{}': path does not exist: {}",
                repo.name,
                repo.path
            );
            continue;
        }
        if !seen.insert(canon(&path)) {
            log!(
                "[Recovery] Skipping external repo '{}': same path as an already-scanned repo",
                repo.name
            );
            continue;
        }
        roots.push((path, Some(repo.id.to_string())));
    }

    roots
}

/// The app id an `App`-kind coding-agent thread edits, read from its newest
/// `SessionStarted`. `None` for every other kind, so a caller branches on
/// `Some` alone.
///
/// Recovery needs it to rebuild a lost worktree the way the spawn path built
/// it: a sparse cone over `data/apps/<id>/`, never the whole workspace tree.
/// The newest session decides, because that is the row naming the branch this
/// sweep is recovering.
///
/// A query that could not run answers `None`, which routes to the ordinary
/// `worktree_add`. That is the safe direction. An over-broad checkout only
/// wastes disk, where a wrongly-sparse one hides the very files a non-app
/// thread came to edit.
pub(crate) async fn lookup_app_spawn_id(pool: &sqlx::PgPool, thread_id: Uuid) -> Option<String> {
    let row: Option<(Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT payload->>'coding_agent_kind', payload->>'app_id' FROM events \
         WHERE event_type = 'SessionStarted' AND thread_id = $1 \
         ORDER BY sequence DESC LIMIT 1",
    )
    .bind(thread_id)
    .fetch_optional(pool)
    .await
    .unwrap_or_else(|e| {
        log!(
            "[Recovery] app-id lookup for thread {} failed: {}. \
             A lost worktree would be rebuilt in full rather than sparse",
            thread_id,
            e
        );
        None
    });
    match row {
        Some((Some(kind), Some(app_id))) if kind == "app" && !app_id.is_empty() => Some(app_id),
        _ => None,
    }
}

/// Rebuild the worktree of a lost coding-agent session at `wt_path`, on the
/// existing `branch`, in the shape its kind requires.
///
/// `app_spawn_id` is [`lookup_app_spawn_id`]'s answer, and it picks the shape.
/// `Some` builds the sparse cone over `data/apps/<id>/` that the spawn path
/// builds, so the resumed agent sees its own app and nothing else. A plain
/// `worktree_add` there would materialise the whole workspace tree, every other
/// app and every artifact included. `None` takes that full checkout, which is
/// right for a Lucidos-source or external-repo branch.
///
/// Both arms reuse the branch rather than creating one. This runs only when the
/// branch already exists and carries the session's committed work.
pub(crate) async fn recreate_lost_worktree(
    repo_root: &Path,
    branch: &str,
    wt_path: &Path,
    app_spawn_id: Option<&str>,
) -> Result<(), String> {
    match app_spawn_id {
        Some(app_id) => {
            crate::engine::git_ops::create_sparse_app_worktree(repo_root, app_id, branch, wt_path)
                .await
                .map(|_reused_or_created| ())
        }
        None => match crate::engine::git_ops::worktree_add(repo_root, wt_path, &[branch]).await {
            Ok(o) if o.status.success() => Ok(()),
            Ok(o) => Err(String::from_utf8_lossy(&o.stderr).trim().to_string()),
            Err(e) => Err(e),
        },
    }
}

/// True when `repo_root` is a **registered external** repo, rather than one of
/// the two repos the engine owns. The sweep stamps the answer onto its
/// `CodingAgentIdled`, so it must agree with a live spawn
/// (`run_session/run.rs`). An app spawn is never external, and nor is a
/// Lucidos one.
///
/// Derived from the repo root alone, so one rule serves every caller. Asking
/// only "is this the Lucidos repo?" read a workspace-repo branch as external,
/// which would strip an app thread of its Apply path.
pub(crate) fn recovery_branch_is_external_repo(
    repo_root: &Path,
    lucidos_repo_root: &Path,
    workspace_root: &Path,
) -> bool {
    crate::engine::git_ops::is_external_repo_path(repo_root, lucidos_repo_root)
        && crate::engine::git_ops::is_external_repo_path(repo_root, workspace_root)
}

/// Which git repo holds an ended coding-agent session's branch.
///
/// [`stale_session_repo`] derives it from the kind the thread's own
/// `SessionStarted` recorded, so the branch and the repo that owns it always
/// come from one row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StaleSessionRepo {
    /// A repo the engine owns and may act in: the workspace git for an `app`
    /// thread, the Lucidos source repo for a `lucidos` one.
    Owned(PathBuf),
    /// A registered external repo. The engine owns no proposal and no branch
    /// there, so the settle never looks the root up. A root nobody looked up
    /// can authorize nothing.
    External,
}

/// Route a stale coding-agent settle to the repo its branch lives in.
///
/// `kind` is `payload->>'coding_agent_kind'` off the newest `SessionStarted`.
/// A missing or unrecognised value routes to the Lucidos repo, through
/// [`CodingAgentKind::parse`]: the `app` and `external` kinds postdate the rows
/// carrying no kind, so such a row really is Lucidos-source.
///
/// `projection_says_external` is
/// `thread_summaries.coding_agent_is_external_repo`. It catches a legacy
/// external thread whose event predates the kind field. Either signal alone
/// answers [`StaleSessionRepo::External`], including when the two disagree:
/// that is the side which touches nothing.
pub(crate) fn stale_session_repo(
    kind: Option<&str>,
    projection_says_external: bool,
    lucidos_repo_root: &Path,
    workspace_root: &Path,
) -> StaleSessionRepo {
    let kind = kind.map(CodingAgentKind::parse).unwrap_or_default();
    if projection_says_external || matches!(kind, CodingAgentKind::External) {
        return StaleSessionRepo::External;
    }
    match kind {
        CodingAgentKind::App => StaleSessionRepo::Owned(workspace_root.to_path_buf()),
        _ => StaleSessionRepo::Owned(lucidos_repo_root.to_path_buf()),
    }
}

/// The repo root a stale settle may run `git branch -D` in, or `None`.
///
/// Three facts authorize the delete and nothing less does: the user clicked
/// Discard, the root came from the thread's own kind, and the engine created
/// the branch name. `git branch -D` force-deletes unmerged commits, which
/// `.claude/rules/rust.md` puts in the class no unresolved answer may
/// authorize.
///
/// It hands back the root rather than a bool so an unauthorized caller has no
/// root to run the command in. Pure, so the gate is exercised without a repo on
/// disk.
pub(crate) fn stale_discard_branch_delete_root<'a>(
    discard: bool,
    repo: &'a StaleSessionRepo,
    branch: &str,
) -> Option<&'a Path> {
    if !discard || !crate::engine::git_ops::is_coding_agent_branch(branch) {
        return None;
    }
    match repo {
        StaleSessionRepo::Owned(root) => Some(root.as_path()),
        StaleSessionRepo::External => None,
    }
}

/// Did the most recent **Claude Code** turn for this thread end cleanly?
///
/// Returns `true` iff the latest CC-channel terminal event is
/// `ResponseGenerated`. Used by stale-session recovery to decide whether a
/// freshly-proposed change should carry `incomplete: true` (mid-turn crash /
/// abort / cancel) or `incomplete: false` (clean Generated, but the per-turn
/// `propose_change` never landed — e.g. the engine died between the idle and
/// the proposal). Without this distinction the recovery flags a clean change
/// as incomplete and the Apply UI surfaces a misleading "this came from a
/// failed turn" confirm dialog.
///
/// **Channel-scoped.** A thread can mix chat-agent, trigger, and CC
/// terminal events (verified empirically: `payload->>'channel'` takes
/// values `'claude_code'`, `'trigger'`, or NULL for chat). Without
/// `payload->>'channel' = 'claude_code'` filtering, a chat
/// `ResponseGenerated` arriving after the CC subprocess died (no
/// engine-restart-recovery synthesized `ResponseAborted` because the engine
/// itself didn't crash) would shadow the CC failure and falsely report
/// clean. Stale-session recovery is CC-only — the chat agent's exit state
/// has no bearing on whether a CC branch is mid-edit.
///
/// The four `Response*` terminal events are the source of truth on the CC
/// channel; any other state (or no CC terminal row at all) means the prior
/// CC turn never produced a clean Generated, which we treat as not-clean.
///
/// Pure DB query — exposed at module scope so tests can exercise it without
/// instantiating a full `LucidosEngine`.
pub(crate) async fn last_turn_ended_cleanly(pool: &sqlx::PgPool, thread_id: Uuid) -> bool {
    let row: Option<String> = sqlx::query_scalar(
        "SELECT event_type FROM events \
         WHERE thread_id = $1 \
           AND event_type IN ('ResponseGenerated', 'ResponseAborted', 'ResponseCanceled', 'ResponseFailed') \
           AND payload->>'channel' = 'claude_code' \
         ORDER BY sequence DESC LIMIT 1",
    )
    .bind(thread_id)
    .fetch_optional(pool)
    .await
    .unwrap_or_else(|e| {
        // Mirrors the recovery sweep's other best-effort queries
        // (`list_pending` in `recover_orphaned_worktrees`): log and fall
        // through. Returning
        // None here biases the caller toward `incomplete: true`, which is
        // the safe direction — Apply will require explicit confirmation
        // rather than auto-applying possibly-mid-turn work.
        log!(
            "[Recovery] last_turn_ended_cleanly({}): {} — defaulting to not-clean (incomplete=true)",
            thread_id,
            e
        );
        None
    });
    matches!(row.as_deref(), Some("ResponseGenerated"))
}

/// Last-resort recovery for an actively-running coding-agent branch whose ref
/// vanished from every repo (`recover_orphaned_worktrees`'s "branch not found
/// in any repo" arm) while its deterministic worktree directory survived on
/// disk. Recreates the branch ref at the worktree's recorded `HEAD` so the
/// session can be `--resume`d on the original branch instead of being torn down
/// and restarted from a fresh branch (which drops the `cc_session_id` and the
/// thread's conversation continuity — the thread-9e37697e failure mode).
///
/// Returns `Some((repo_root, worktree_path))` when the branch was recreated and
/// the session can resume; `None` (caller falls back to ending the stuck
/// session — the genuine last resort) when:
///   - the worktree directory is absent (nothing recoverable),
///   - its `HEAD` does not resolve to a real commit (a dangling symref left by
///     out-of-band ref deletion — we never *guess* a SHA, which could mis-point
///     a commit-bearing branch onto the wrong history), or
///   - the repo root can't be resolved or the `git branch` create fails.
///
/// Pure git/filesystem — no engine/DB dependency — so it is unit-testable
/// against a real worktree without instantiating a `LucidosEngine`.
pub(crate) async fn recover_branch_ref_from_worktree(
    workspace_path: &Path,
    thread_id: Uuid,
    branch_name: &str,
) -> Option<(PathBuf, PathBuf)> {
    let wt = crate::engine::agent_session::resume::deterministic_worktree_path(
        workspace_path,
        thread_id,
    );
    if !matches!(tokio::fs::try_exists(&wt).await, Ok(true)) {
        return None;
    }
    // Resolve the worktree's HEAD to a concrete commit. `--verify` makes git
    // fail (non-zero) rather than echo the literal "HEAD" when the symref
    // dangles, so a deleted-branch-but-symref-still-points-at-it worktree
    // (unrecoverable without fsck) falls through to the last-resort path
    // instead of recreating the branch at a bogus value.
    let head_sha = match git_cmd(&["rev-parse", "--verify", "HEAD"], &wt).await {
        Ok(o) if o.status.success() => {
            let sha = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if sha.is_empty() {
                return None;
            }
            sha
        }
        _ => return None,
    };
    let repo_root = crate::engine::worktree_cleanup::resolve_repo_root_from_worktree(&wt).await?;
    match git_cmd(&["branch", branch_name, &head_sha], &repo_root).await {
        Ok(o) if o.status.success() => {
            log!(
                "[Recovery] Recreated missing branch {} at {} from surviving worktree {} — session resumable",
                branch_name,
                &head_sha[..head_sha.floor_char_boundary(8)],
                wt.display()
            );
            Some((repo_root, wt))
        }
        Ok(o) => {
            log!(
                "[Recovery] Could not recreate branch {} from worktree {}: {}",
                branch_name,
                wt.display(),
                String::from_utf8_lossy(&o.stderr).trim()
            );
            None
        }
        Err(e) => {
            log!(
                "[Recovery] git branch create for {} errored: {}",
                branch_name,
                e
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::git_ops::{git_cmd, worktrees_dir};
    use std::collections::HashMap;

    /// Build a fresh git repo at `workspace` with one commit on `main` and an
    /// empty `.lucidos/worktrees/`. Returns nothing — the caller uses
    /// `workspace` as both repo root and workspace root.
    async fn init_repo(workspace: &Path) {
        git_cmd(&["init", "--initial-branch=main"], workspace)
            .await
            .unwrap();
        git_cmd(&["config", "user.email", "recover@test"], workspace)
            .await
            .unwrap();
        git_cmd(&["config", "user.name", "Recover Test"], workspace)
            .await
            .unwrap();
        tokio::fs::write(workspace.join("seed.txt"), "seed")
            .await
            .unwrap();
        git_cmd(&["add", "."], workspace).await.unwrap();
        git_cmd(&["commit", "-m", "seed"], workspace).await.unwrap();
        let _ = worktrees_dir(workspace);
    }

    async fn rev_parse(repo: &Path, rev: &str) -> Option<String> {
        let o = git_cmd(&["rev-parse", "--verify", rev], repo).await.ok()?;
        o.status
            .success()
            .then(|| String::from_utf8_lossy(&o.stdout).trim().to_string())
    }

    /// Regression for the thread-9e37697e recovery half: when a resumable
    /// coding-agent branch's ref has vanished but its deterministic worktree
    /// directory survives with a resolvable HEAD, recovery recreates the branch
    /// from that HEAD (so the session can `--resume`) instead of ending the
    /// stuck session and dropping the session id.
    #[tokio::test]
    async fn recover_branch_ref_recreates_missing_branch_from_surviving_worktree() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path().to_path_buf();
        init_repo(&workspace).await;

        let thread_id = Uuid::new_v4();
        let branch = "claude-code/recover-test";
        let wt = crate::engine::agent_session::resume::deterministic_worktree_path(
            &workspace, thread_id,
        );
        git_cmd(
            &[
                "worktree",
                "add",
                "-b",
                branch,
                &wt.to_string_lossy(),
                "main",
            ],
            &workspace,
        )
        .await
        .unwrap();
        let head_before = rev_parse(&wt, "HEAD")
            .await
            .expect("worktree HEAD resolves");

        // Simulate the branch-ref loss while the worktree dir survives: detach
        // the worktree's HEAD (so it still resolves to a commit), then delete
        // the branch ref. This is exactly the "branch not found in any repo,
        // worktree survives" state recovery's last-resort arm guards against.
        git_cmd(&["checkout", "--detach"], &wt).await.unwrap();
        git_cmd(&["branch", "-D", branch], &workspace)
            .await
            .unwrap();
        assert!(
            rev_parse(&workspace, &format!("refs/heads/{}", branch))
                .await
                .is_none(),
            "precondition: branch ref must be gone"
        );

        let recovered = recover_branch_ref_from_worktree(&workspace, thread_id, branch).await;
        let (repo_root, wt_path) =
            recovered.expect("must recover the branch from the surviving worktree");
        assert_eq!(wt_path, wt, "returns the recovered worktree path");
        // `resolve_repo_root_from_worktree` canonicalizes (macOS resolves
        // `/var` → `/private/var`), so compare canonical forms.
        let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
        assert_eq!(
            canon(&repo_root),
            canon(&workspace),
            "resolves the worktree's repo root"
        );

        let head_after = rev_parse(&workspace, &format!("refs/heads/{}", branch))
            .await
            .expect("branch ref must be recreated");
        assert_eq!(
            head_after, head_before,
            "recreated branch must point at the worktree's recorded HEAD, not a guessed commit"
        );
    }

    /// No worktree on disk → nothing to recover from → `None` so the caller
    /// falls back to ending the stuck session (the genuine last resort).
    #[tokio::test]
    async fn recover_branch_ref_returns_none_when_worktree_absent() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path().to_path_buf();
        init_repo(&workspace).await;
        let _ = worktrees_dir(&workspace);

        let recovered =
            recover_branch_ref_from_worktree(&workspace, Uuid::new_v4(), "claude-code/missing")
                .await;
        assert!(
            recovered.is_none(),
            "absent worktree must not recover — caller ends the stuck session"
        );
    }

    /// A dangling HEAD symref (branch ref deleted out-of-band, worktree HEAD
    /// still says `ref: refs/heads/<branch>`) is unrecoverable without fsck —
    /// the helper must refuse rather than guess a SHA and mis-point the branch.
    #[tokio::test]
    async fn recover_branch_ref_returns_none_on_dangling_head() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path().to_path_buf();
        init_repo(&workspace).await;

        let thread_id = Uuid::new_v4();
        let branch = "claude-code/dangling-test";
        let wt = crate::engine::agent_session::resume::deterministic_worktree_path(
            &workspace, thread_id,
        );
        git_cmd(
            &[
                "worktree",
                "add",
                "-b",
                branch,
                &wt.to_string_lossy(),
                "main",
            ],
            &workspace,
        )
        .await
        .unwrap();
        // Delete the branch ref directly, leaving the worktree HEAD as a
        // dangling `ref: refs/heads/<branch>` symref — `rev-parse HEAD` fails.
        let ref_path = workspace
            .join(".git/refs/heads")
            .join(branch.strip_prefix("claude-code/").unwrap());
        let _ = tokio::fs::remove_file(workspace.join(format!(".git/refs/heads/{}", branch))).await;
        let _ = tokio::fs::remove_file(&ref_path).await;
        git_cmd(&["pack-refs", "--all"], &workspace).await.ok();
        let _ = tokio::fs::remove_file(workspace.join(format!(".git/refs/heads/{}", branch))).await;

        let recovered = recover_branch_ref_from_worktree(&workspace, thread_id, branch).await;
        assert!(
            recovered.is_none(),
            "dangling HEAD symref is unrecoverable — must NOT guess a SHA"
        );
    }

    fn registered_repo(name: &str, path: &Path) -> crate::core::repositories::Repository {
        crate::core::repositories::Repository {
            id: Uuid::new_v4(),
            name: name.to_string(),
            path: path.to_string_lossy().into_owned(),
            description: None,
            root_commit_sha: None,
            created_at: chrono::Utc::now(),
        }
    }

    fn root_paths(roots: &[(PathBuf, Option<String>)]) -> Vec<PathBuf> {
        roots.iter().map(|(p, _)| p.clone()).collect()
    }

    /// The bug this fixes. An app coding-agent thread's worktree and branch
    /// live in the workspace git, which no registry lists. A sweep over the
    /// Lucidos repo plus the registered externals never saw one, so a user
    /// switch fell through to `end_stuck_session` instead of auto-resuming.
    #[test]
    fn recovery_repo_roots_scans_the_workspace_repo() {
        let lucidos = PathBuf::from("/src/lucidos");
        let workspace = PathBuf::from("/home/u/workspaces/dev");

        let roots = recovery_repo_roots(&lucidos, &workspace, &[]);

        assert_eq!(
            root_paths(&roots),
            vec![lucidos, workspace],
            "the workspace repo must be scanned, after the Lucidos source repo"
        );
        assert!(
            roots.iter().all(|(_, id)| id.is_none()),
            "neither engine-owned repo is in the registry, so neither carries an id"
        );
    }

    /// Order is load-bearing: the lost-branch fallback takes the first repo
    /// whose refs hold the branch, so the two engine-owned roots come first.
    #[test]
    fn recovery_repo_roots_puts_registered_repos_last() {
        let tmp = tempfile::tempdir().unwrap();
        let external = tmp.path().join("external-repo");
        std::fs::create_dir_all(&external).unwrap();
        let lucidos = PathBuf::from("/src/lucidos");
        let workspace = PathBuf::from("/home/u/workspaces/dev");
        let repos = vec![registered_repo("example-repo", &external)];

        let roots = recovery_repo_roots(&lucidos, &workspace, &repos);

        assert_eq!(root_paths(&roots), vec![lucidos, workspace, external]);
        assert!(
            roots[2].1.is_some(),
            "a registered repo carries the id its worktree marker records"
        );
    }

    /// A workspace that IS the Lucidos checkout must not be scanned twice. A
    /// duplicate root recovers one worktree under two entries, which trips the
    /// duplicate-thread guard on every ordinary boot.
    #[test]
    fn recovery_repo_roots_dedupes_a_workspace_inside_the_lucidos_repo() {
        let tmp = tempfile::tempdir().unwrap();
        let both = tmp.path().to_path_buf();

        let roots = recovery_repo_roots(&both, &both, &[]);

        assert_eq!(root_paths(&roots), vec![both]);
    }

    /// Same dedupe from the other side: a user who also registered their
    /// workspace as an external repo gets one entry, the workspace's.
    #[test]
    fn recovery_repo_roots_dedupes_a_workspace_registered_as_an_external_repo() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path().to_path_buf();
        let lucidos = PathBuf::from("/src/lucidos");
        let repos = vec![registered_repo("my-workspace", &workspace)];

        let roots = recovery_repo_roots(&lucidos, &workspace, &repos);

        assert_eq!(root_paths(&roots), vec![lucidos, workspace]);
        assert!(
            roots[1].1.is_none(),
            "the workspace entry wins, so the duplicate registry id is dropped"
        );
    }

    /// The dedupe case that predates the workspace entry: a Lucidos checkout
    /// the user also registered as a repo. Scanning it twice would recover
    /// every Lucidos-source worktree under two entries.
    #[test]
    fn recovery_repo_roots_dedupes_a_registered_lucidos_repo() {
        let tmp = tempfile::tempdir().unwrap();
        let lucidos = tmp.path().to_path_buf();
        let workspace = PathBuf::from("/home/u/workspaces/dev");
        let repos = vec![registered_repo("Lucidos", &lucidos)];

        let roots = recovery_repo_roots(&lucidos, &workspace, &repos);

        assert_eq!(root_paths(&roots), vec![lucidos, workspace]);
    }

    /// A registered repo the user has since deleted from disk is skipped. Its
    /// `git worktree list` would only fail and log.
    #[test]
    fn recovery_repo_roots_skips_a_registered_repo_that_is_gone() {
        let lucidos = PathBuf::from("/src/lucidos");
        let workspace = PathBuf::from("/home/u/workspaces/dev");
        let repos = vec![registered_repo(
            "example-repo",
            Path::new("/nonexistent/example-repo"),
        )];

        let roots = recovery_repo_roots(&lucidos, &workspace, &repos);

        assert_eq!(root_paths(&roots), vec![lucidos, workspace]);
    }

    /// The recovery `CodingAgentIdled` stamps
    /// `thread_summaries.coding_agent_is_external_repo`, and an app thread that
    /// wrongly reads external loses its Apply path. Only a registered repo is
    /// external; neither engine-owned root is.
    #[test]
    fn recovery_branch_is_external_repo_excludes_both_engine_owned_roots() {
        let tmp = tempfile::tempdir().unwrap();
        let lucidos = tmp.path().join("lucidos");
        let workspace = tmp.path().join("workspace");
        let external = tmp.path().join("example-repo");
        for p in [&lucidos, &workspace, &external] {
            std::fs::create_dir_all(p).unwrap();
        }

        assert!(
            !recovery_branch_is_external_repo(&lucidos, &lucidos, &workspace),
            "a Lucidos-source branch is not external"
        );
        assert!(
            !recovery_branch_is_external_repo(&workspace, &lucidos, &workspace),
            "an app branch lives in the workspace repo and is not external"
        );
        assert!(
            recovery_branch_is_external_repo(&external, &lucidos, &workspace),
            "a registered repo's branch is external"
        );
    }

    fn lucidos_root() -> PathBuf {
        PathBuf::from("/src/lucidos")
    }

    fn workspace_root() -> PathBuf {
        PathBuf::from("/home/u/workspaces/dev")
    }

    fn repo_for(kind: Option<&str>, projection_says_external: bool) -> StaleSessionRepo {
        stale_session_repo(
            kind,
            projection_says_external,
            &lucidos_root(),
            &workspace_root(),
        )
    }

    /// The bug this fixes. An app thread's branch lives in the workspace git.
    /// A settle that asks the Lucidos repo finds nothing and proposes nothing,
    /// leaving the user no Apply card for committed work.
    #[test]
    fn stale_session_repo_routes_an_app_thread_to_the_workspace_git() {
        assert_eq!(
            repo_for(Some("app"), false),
            StaleSessionRepo::Owned(workspace_root())
        );
    }

    /// A `lucidos` thread, an unrecognised kind and a legacy row carrying no
    /// kind all take the Lucidos repo. The `app` and `external` kinds postdate
    /// those rows, so a row with no kind really is Lucidos-source.
    #[test]
    fn stale_session_repo_routes_every_other_kind_to_the_lucidos_repo() {
        for kind in [Some("lucidos"), Some("something-new"), None] {
            assert_eq!(
                repo_for(kind, false),
                StaleSessionRepo::Owned(lucidos_root()),
                "kind {:?} must take the Lucidos repo",
                kind
            );
        }
    }

    /// Either external signal alone is enough. The event kind covers a modern
    /// thread; the projection flag covers a legacy one whose `SessionStarted`
    /// predates the field.
    #[test]
    fn stale_session_repo_reads_external_from_either_signal() {
        assert_eq!(
            repo_for(Some("external"), false),
            StaleSessionRepo::External
        );
        assert_eq!(repo_for(None, true), StaleSessionRepo::External);
    }

    /// A disagreement between the two signals resolves to `External`, the only
    /// answer that touches nothing. Routing a contradiction into an owned repo
    /// would hand a `git branch -D` a root nobody can vouch for.
    #[test]
    fn stale_session_repo_resolves_a_contradiction_to_external() {
        assert_eq!(repo_for(Some("app"), true), StaleSessionRepo::External);
    }

    /// `git branch -D` force-deletes unmerged commits, so it needs all three
    /// facts. Dropping any one leaves the branch, which is recoverable. A wrong
    /// delete is not.
    #[test]
    fn stale_discard_deletes_a_branch_only_on_all_three_facts() {
        let owned = StaleSessionRepo::Owned(workspace_root());
        let branch = "lucidos-claude-code-app-habit-tracker-add-streaks-ae6846f4";

        assert_eq!(
            stale_discard_branch_delete_root(true, &owned, branch),
            Some(workspace_root().as_path()),
            "a Discard on an app thread deletes its branch in the workspace git"
        );
        assert_eq!(
            stale_discard_branch_delete_root(false, &owned, branch),
            None,
            "an Apply or Stop deletes nothing: only Discard asks for the work to go"
        );
        assert_eq!(
            stale_discard_branch_delete_root(true, &StaleSessionRepo::External, branch),
            None,
            "an external root was never resolved, so it authorizes no delete"
        );
        assert_eq!(
            stale_discard_branch_delete_root(true, &owned, "main"),
            None,
            "a branch the engine did not create is not this settle's to delete"
        );
    }

    /// Build a workspace git holding one app, a sibling app, and an artifact,
    /// with `branch` created and no worktree on it. That is the state the
    /// lost-worktree recovery arm rebuilds from.
    async fn workspace_with_lost_app_branch(workspace: &Path, branch: &str) {
        init_repo(workspace).await;
        for dir in [
            "data/apps/habit-tracker",
            "data/apps/other",
            "data/artifacts",
        ] {
            tokio::fs::create_dir_all(workspace.join(dir))
                .await
                .unwrap();
        }
        tokio::fs::write(
            workspace.join("data/apps/habit-tracker/index.html"),
            "<h1>h</h1>",
        )
        .await
        .unwrap();
        tokio::fs::write(workspace.join("data/apps/other/index.html"), "<h1>o</h1>")
            .await
            .unwrap();
        tokio::fs::write(workspace.join("data/artifacts/report.md"), "private")
            .await
            .unwrap();
        git_cmd(&["add", "."], workspace).await.unwrap();
        git_cmd(&["commit", "-m", "scaffold"], workspace)
            .await
            .unwrap();
        git_cmd(&["branch", branch], workspace).await.unwrap();
    }

    /// An app thread's worktree is a sparse cone over its own app folder. A
    /// rebuild that materialised the whole workspace tree would show the
    /// resumed agent every other app and every artifact.
    #[tokio::test]
    async fn recreate_lost_worktree_rebuilds_an_app_worktree_sparsely() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path().to_path_buf();
        let branch = "lucidos-claude-code-app-habit-tracker-add-streaks-ae6846f4";
        workspace_with_lost_app_branch(&workspace, branch).await;
        let wt = worktrees_dir(&workspace).join("thread-ae6846f4");

        recreate_lost_worktree(&workspace, branch, &wt, Some("habit-tracker"))
            .await
            .expect("an app worktree must rebuild on its surviving branch");

        assert!(
            wt.join("data/apps/habit-tracker/index.html").exists(),
            "the thread's own app folder must be materialised"
        );
        assert!(
            !wt.join("data/apps/other/index.html").exists(),
            "a sibling app must stay outside the sparse cone"
        );
        assert!(
            !wt.join("data/artifacts/report.md").exists(),
            "artifacts must stay outside the sparse cone"
        );
    }

    /// Every other kind keeps the full checkout, so a Lucidos-source or
    /// external-repo thread still finds the files it came to edit.
    #[tokio::test]
    async fn recreate_lost_worktree_rebuilds_a_non_app_worktree_in_full() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path().to_path_buf();
        let branch = "lucidos-claude-code-repo-fix-oauth-urls-ae6846f4";
        workspace_with_lost_app_branch(&workspace, branch).await;
        let wt = worktrees_dir(&workspace).join("thread-ae6846f4");

        recreate_lost_worktree(&workspace, branch, &wt, None)
            .await
            .expect("a non-app worktree must rebuild on its surviving branch");

        // `worktree_add` passes `--no-checkout`, so the spawn that follows does
        // the checkout. Git registering the worktree on our branch, unnarrowed,
        // is the fact this arm owns.
        let head = git_cmd(&["rev-parse", "--abbrev-ref", "HEAD"], &wt)
            .await
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&head.stdout).trim(), branch);
        let narrowed = git_cmd(&["config", "--get", "core.sparseCheckout"], &wt)
            .await
            .unwrap();
        assert!(
            !narrowed.status.success(),
            "a non-app rebuild must not narrow the checkout"
        );
    }

    #[test]
    fn orphan_recovery_target_surfaces_known_branch_and_skips_unknown() {
        let tid = Uuid::new_v4();
        let mut branch_to_thread = HashMap::new();
        branch_to_thread.insert("claude-code/known".to_string(), tid);

        // A branch a SessionStarted maps to a thread surfaces under it.
        assert_eq!(
            orphan_recovery_target(&branch_to_thread, "claude-code/known"),
            Some(tid)
        );

        // A branch with no originating thread (events pruned, worktree
        // orphaned) MUST skip — never fabricate a phantom thread. Fabricating
        // one made recovery emit `CodingAgentIdled` to a row-less thread, which
        // the lifecycle gate rejects as CC-only (row-less threads default to
        // Chat/Archived), failing on every engine restart.
        assert_eq!(
            orphan_recovery_target(&branch_to_thread, "claude-code/orphaned"),
            None
        );
    }
}
