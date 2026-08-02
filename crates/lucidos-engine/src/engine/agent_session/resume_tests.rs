use super::*;
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{
    ActorMode, EventChannel, EventMeta, SessionEndReason, ThreadEvent,
};
use crate::test_support::{setup_test_db, teardown_test_db};
use uuid::Uuid;

fn cc_meta() -> EventMeta {
    EventMeta {
        channel: Some(EventChannel::ClaudeCode),
        ..EventMeta::NONE
    }
}

async fn emit(bus: &EventBus, thread_id: Uuid, event: ThreadEvent) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event,
        meta: cc_meta(),
    })
    .await
    .unwrap();
}

async fn seed_session_started(bus: &EventBus, thread_id: Uuid, session_id: &str, branch: &str) {
    emit(
        bus,
        thread_id,
        ThreadEvent::MessageReceived {
            text: "go".into(),
            user_image_hashes: vec![],
            device_id: None,
            device: None,
            image_description: None,
            parent_thread_id: None,
            spawning_event_id: None,
            mode: ActorMode::Human,
            model: None,
            reasoning_effort: None,
            origin: None,
        },
    )
    .await;
    emit(
        bus,
        thread_id,
        ThreadEvent::SessionStarted {
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            session_id: session_id.into(),
            branch: branch.into(),
            repo_id: None,
            coding_agent_kind: Default::default(),
            coding_agent_folder: String::new(),
            app_id: None,
        },
    )
    .await;
}

async fn seed_pending_change(bus: &EventBus, thread_id: Uuid, branch: &str) -> Uuid {
    let change_id = Uuid::new_v4();
    emit(
        bus,
        thread_id,
        ThreadEvent::ChangeProposed {
            change_id: change_id.to_string(),
            description: Some("work".into()),
            files: vec!["src/x.rs".to_string()],
            requires_restart: false,
            origin: None,
            commit_sha: None,
            branch_name: branch.to_string(),
            repo_root: "/tmp/repo".to_string(),
            hardened: true,
            incomplete: false,
            path: String::new(),
            diff: String::new(),
        },
    )
    .await;
    change_id
}

#[tokio::test]
async fn pending_change_after_session_ended_resumes_branch() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let branch = "claude-code/pending";

    seed_session_started(&bus, thread_id, "sess-1", branch).await;
    seed_pending_change(&bus, thread_id, branch).await;
    emit(
        &bus,
        thread_id,
        ThreadEvent::SessionEnded {
            reason: SessionEndReason::Shutdown,
        },
    )
    .await;

    let (sid, resume_branch) =
        resolve_resume_context(&pool, bus.changes_projection(), thread_id, None, None).await;
    assert_eq!(sid, None);
    assert_eq!(resume_branch, Some(branch.to_string()));

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn pending_change_branch_with_idled_session_recovers_sid() {
    // Contract: when a pending change exists, the canonical branch wins for
    // branch selection (overriding any later SessionStarted on a different
    // branch), but the `cc_session_id` is recovered from the most recent
    // `CodingAgentIdled` event regardless of which branch produced it.
    // CC needs the sid to `--resume` the conversation; without it, the
    // revived subprocess starts with zero history.
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let canonical_branch = "claude-code/canonical";
    let wrong_branch = "claude-code/wrong";

    seed_session_started(&bus, thread_id, "real-session", canonical_branch).await;
    seed_pending_change(&bus, thread_id, canonical_branch).await;
    emit(
        &bus,
        thread_id,
        ThreadEvent::SessionEnded {
            reason: SessionEndReason::Shutdown,
        },
    )
    .await;

    emit(
        &bus,
        thread_id,
        ThreadEvent::SessionStarted {
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            session_id: "wrong-session".into(),
            branch: wrong_branch.into(),
            repo_id: None,
            coding_agent_kind: Default::default(),
            coding_agent_folder: String::new(),
            app_id: None,
        },
    )
    .await;
    emit(
        &bus,
        thread_id,
        ThreadEvent::CodingAgentIdled {
            has_changes: false,
            is_external_repo: false,
            requires_restart: false,
            cc_session_id: Some("wrong-session".into()),
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            reason: None,
            worktree_path: None,
            worktree_head_sha: None,
            bg_bash_pending: false,
        },
    )
    .await;

    let (sid, resume_branch) =
        resolve_resume_context(&pool, bus.changes_projection(), thread_id, None, None).await;
    assert_eq!(sid, Some("wrong-session".to_string()));
    assert_eq!(resume_branch, Some(canonical_branch.to_string()));

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn pending_change_with_idled_session_recovers_cc_session_id() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let branch = "claude-code/with-idled-sid";
    let session_id = "sess-recovered-abc";

    // Seed: SessionStarted, pending change, CodingAgentIdled WITH cc_session_id, SessionEnded
    seed_session_started(&bus, thread_id, session_id, branch).await;
    seed_pending_change(&bus, thread_id, branch).await;
    emit(
        &bus,
        thread_id,
        ThreadEvent::CodingAgentIdled {
            has_changes: true,
            is_external_repo: false,
            requires_restart: false,
            cc_session_id: Some(session_id.into()),
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            reason: None,
            worktree_path: None,
            worktree_head_sha: None,
            bg_bash_pending: false,
        },
    )
    .await;
    emit(
        &bus,
        thread_id,
        ThreadEvent::SessionEnded {
            reason: SessionEndReason::Shutdown,
        },
    )
    .await;

    let (sid, resume_branch) =
        resolve_resume_context(&pool, bus.changes_projection(), thread_id, None, None).await;
    assert_eq!(
        sid,
        Some(session_id.to_string()),
        "cc_session_id must be recovered when pending branch exists"
    );
    assert_eq!(resume_branch, Some(branch.to_string()));

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn applied_change_falls_through_to_fresh_start() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let branch = "claude-code/applied";

    seed_session_started(&bus, thread_id, "sess-1", branch).await;
    let change_id = seed_pending_change(&bus, thread_id, branch).await;
    emit(
        &bus,
        thread_id,
        ThreadEvent::SessionEnded {
            reason: SessionEndReason::Shutdown,
        },
    )
    .await;
    emit(
        &bus,
        thread_id,
        ThreadEvent::ChangeApplied {
            change_id: change_id.to_string(),
            requires_restart: false,
            client_update: false,
            commits: vec![],
            thread_title: None,
            actor: None,
            pre_merge_sha: None,
            post_merge_sha: None,
            path: String::new(),
        },
    )
    .await;

    let (sid, resume_branch) =
        resolve_resume_context(&pool, bus.changes_projection(), thread_id, None, None).await;
    assert_eq!(sid, None);
    assert_eq!(resume_branch, None);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Regression for the "Cancel spawns a fresh, amnesiac CC session" bug.
///
/// A user Cancel (Stop = Esc) emits `ResponseCanceled` then `CodingAgentIdled`
/// (carrying the `cc_session_id`) and — critically — does NOT delete the branch
/// or emit `SessionEnded` (the branch survives via `KeepCanceledBranch` in
/// `finalize`). So the most-recent turn closer is the idle, and the next message
/// resolves to the SAME session id + branch (a `--resume`), not a fresh spawn.
/// Before the fix, the cancel deleted the branch and dropped the sid, forcing a
/// brand-new session that re-asked everything.
#[tokio::test]
async fn cancel_then_idle_resumes_same_session() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let branch = "claude-code/canceled-turn";
    let session_id = "sess-canceled-resume";

    seed_session_started(&bus, thread_id, session_id, branch).await;
    emit(
        &bus,
        thread_id,
        ThreadEvent::ResponseCanceled {
            text: "partial work before the user hit Stop".into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
            cause: crate::engine::thread_events::CancelCause::UserStop,
        },
    )
    .await;
    emit_idled(&bus, thread_id, Some(session_id), None).await;

    // Auto-detect path (no caller sid): the most-recent closer is the idle, so
    // the resolver resumes the same session id on the SessionStarted branch.
    let (sid, resume_branch) =
        resolve_resume_context(&pool, bus.changes_projection(), thread_id, None, None).await;
    assert_eq!(
        sid,
        Some(session_id.to_string()),
        "a cancel must resume the same cc_session_id, not spawn fresh"
    );
    assert_eq!(resume_branch, Some(branch.to_string()));

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Reproduces the SessionEnded-after-CodingAgentIdled blind-spot.
///
/// In a clean (no-changes) CC turn the engine emits CodingAgentIdled and
/// then, on the post-loop cleanup or stale-resume retry path, emits
/// SessionEnded { Completed } / { StaleResume }. The auto-detect resolver
/// pulls the most-recent lifecycle event and returns no sid because the
/// row is SessionEnded, not CodingAgentIdled — even though a usable
/// CodingAgentIdled with the cc_session_id sits one row earlier.
///
/// The chat handler's job is to pre-resolve the sid via
/// `lookup_latest_cc_session_id` and pass it as `caller_session_id`. With
/// the caller-supplied sid the resolver short-circuits on the
/// `caller_session_id.is_some()` branch and CC resumes the conversation.
#[tokio::test]
async fn no_pending_change_with_session_ended_after_idle_recovers_via_caller_sid() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let branch = "claude-code/clean-turn";
    let session_id = "sess-clean-completed";

    // Clean turn: SessionStarted, CodingAgentIdled, SessionEnded { Completed }.
    // No pending change — this isolates the no-pending-change resolver path.
    seed_session_started(&bus, thread_id, session_id, branch).await;
    emit(
        &bus,
        thread_id,
        ThreadEvent::CodingAgentIdled {
            has_changes: false,
            is_external_repo: false,
            requires_restart: false,
            cc_session_id: Some(session_id.into()),
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            reason: None,
            worktree_path: None,
            worktree_head_sha: None,
            bg_bash_pending: false,
        },
    )
    .await;
    emit(
        &bus,
        thread_id,
        ThreadEvent::SessionEnded {
            reason: SessionEndReason::Shutdown,
        },
    )
    .await;

    // Bug repro: resolver with `None` returns `(None, None)` — the
    // auto-detect path sees SessionEnded as the latest lifecycle event
    // and refuses to resume.
    let (sid_without_caller, branch_without_caller) =
        resolve_resume_context(&pool, bus.changes_projection(), thread_id, None, None).await;
    assert_eq!(
        sid_without_caller, None,
        "auto-detect must return None when latest is SessionEnded — \
         this is the bug the chat handler must work around"
    );
    assert_eq!(branch_without_caller, None);

    // Fix: the chat handler looks up the prior cc_session_id and passes
    // it as `caller_session_id`. The resolver short-circuits and returns
    // the sid + the SessionStarted branch.
    let recovered_sid = lookup_latest_cc_session_id(&pool, thread_id).await;
    assert_eq!(
        recovered_sid,
        Some(session_id.to_string()),
        "lookup_latest_cc_session_id must surface the sid from CodingAgentIdled \
         so the chat handler has something to pass to the resolver"
    );

    let (sid_with_caller, branch_with_caller) = resolve_resume_context(
        &pool,
        bus.changes_projection(),
        thread_id,
        recovered_sid,
        None,
    )
    .await;
    assert_eq!(
        sid_with_caller,
        Some(session_id.to_string()),
        "caller-supplied sid must win: this is what preserves CC \
         conversation memory across the in-memory session being torn down"
    );
    assert_eq!(branch_with_caller, Some(branch.to_string()));

    pool.close().await;
    teardown_test_db(&db_name).await;
}

async fn emit_settings_with_sid(bus: &EventBus, thread_id: Uuid, cc_session_id: Option<&str>) {
    emit(
        bus,
        thread_id,
        ThreadEvent::CodingAgentSettingsChanged {
            model: None,
            reasoning_effort: None,
            permission_mode: None,
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            cc_session_id: cc_session_id.map(String::from),
            claude_config_dir: None,
        },
    )
    .await;
}

async fn emit_settings_with_config_dir(
    bus: &EventBus,
    thread_id: Uuid,
    cc_session_id: Option<&str>,
    claude_config_dir: Option<&str>,
) {
    emit(
        bus,
        thread_id,
        ThreadEvent::CodingAgentSettingsChanged {
            model: None,
            reasoning_effort: None,
            permission_mode: None,
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            cc_session_id: cc_session_id.map(String::from),
            claude_config_dir: claude_config_dir.map(String::from),
        },
    )
    .await;
}

/// The config dir pinned at Init is what a later resume must re-inject so CC finds
/// the transcript under the same dir it was written in — the fix for dev/bf997e21.
#[tokio::test]
async fn lookup_finds_config_dir_from_init_settings() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    seed_session_started(&bus, thread_id, "sess-cfg", "claude-code/cfg").await;
    emit_settings_with_config_dir(&bus, thread_id, Some("sess-cfg"), Some("/home/u/.claude")).await;

    assert_eq!(
        lookup_pinned_cc_config_dir(&pool, thread_id).await,
        Some("/home/u/.claude".to_string()),
        "the config dir stamped at Init must be resolvable for a later resume"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A legacy thread whose events predate the field resolves `None` — the spawn then
/// falls back to the live env, and a failed resume is caught by the explicit
/// session-not-found recovery. Must never surface a phantom dir.
#[tokio::test]
async fn lookup_config_dir_is_none_for_legacy_thread() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    seed_session_started(&bus, thread_id, "sess-legacy", "claude-code/legacy").await;
    // Init settings with a sid but NO config dir (legacy row shape).
    emit_settings_with_sid(&bus, thread_id, Some("sess-legacy")).await;

    assert_eq!(
        lookup_pinned_cc_config_dir(&pool, thread_id).await,
        None,
        "a thread with no recorded config dir must resolve None, not a phantom value"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The thread's pin is the FIRST recorded config dir and never moves: a
/// settings-only emit with no dir doesn't shadow it, and a later respawn under a
/// DIFFERENT account must not override it — that immovability is what stops a
/// thread from switching provider after turn 1.
#[tokio::test]
async fn lookup_pinned_config_dir_prefers_first() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    seed_session_started(&bus, thread_id, "sess-1", "claude-code/multi-cfg").await;
    emit_settings_with_config_dir(
        &bus,
        thread_id,
        Some("sess-1"),
        Some("/home/u/.claude-personal"),
    )
    .await;
    // A settings-only change (model switch) carries no config dir — must not shadow.
    emit_settings_with_config_dir(&bus, thread_id, None, None).await;

    assert_eq!(
        lookup_pinned_cc_config_dir(&pool, thread_id).await,
        Some("/home/u/.claude-personal".to_string()),
        "a settings-only emit with no config dir must not shadow the pinned dir"
    );

    // A respawn under a DIFFERENT dir must NOT move the pin — the thread stays on
    // the account of its first session.
    emit_settings_with_config_dir(&bus, thread_id, Some("sess-2"), Some("/home/u/.claude")).await;
    assert_eq!(
        lookup_pinned_cc_config_dir(&pool, thread_id).await,
        Some("/home/u/.claude-personal".to_string()),
        "the FIRST recorded config dir is the permanent pin; a later account must not override it"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Reproduces the lost-session-id-on-mid-turn-restart bug.
///
/// A long CC turn interrupted by an engine restart *before* it ever reached a
/// `CodingAgentIdled` boundary never persisted its `cc_session_id` (idle is the
/// only event that historically carried it). The fix pins the id at `Init` via
/// `CodingAgentSettingsChanged { cc_session_id: Some(..) }`, so
/// `lookup_latest_cc_session_id` must surface it even when no `CodingAgentIdled`
/// row exists at all — otherwise the resumed session falls back to a fresh CC
/// process + reconstructed summary instead of a real `--resume`.
#[tokio::test]
async fn lookup_finds_session_id_from_init_settings_when_never_idled() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let session_id = "sess-init-only";

    // Session started, CC reported its id at Init, then the turn was
    // interrupted before any CodingAgentIdled was emitted.
    seed_session_started(&bus, thread_id, session_id, "claude-code/init-only").await;
    emit_settings_with_sid(&bus, thread_id, Some(session_id)).await;

    let recovered = lookup_latest_cc_session_id(&pool, thread_id).await;
    assert_eq!(
        recovered,
        Some(session_id.to_string()),
        "lookup_latest_cc_session_id must surface the sid pinned at Init by \
         CodingAgentSettingsChanged when the turn never reached an idle boundary"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The lookup reads the most recent non-null id across BOTH event types in
/// sequence order: a fresh respawn's Init id must win over an older idle id, a
/// later idle id must win over an older Init id, and a settings-only emit (no
/// sid) must never shadow a real id.
#[tokio::test]
async fn lookup_prefers_most_recent_session_id_across_event_types() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    seed_session_started(&bus, thread_id, "sess-1", "claude-code/multi").await;
    // First run idled with sess-1.
    emit_idled(&bus, thread_id, Some("sess-1"), None).await;
    // A settings-only change (e.g. model switch) carries no sid — must not shadow.
    emit_settings_with_sid(&bus, thread_id, None).await;
    // Respawn: Init pins sess-2.
    emit_settings_with_sid(&bus, thread_id, Some("sess-2")).await;

    assert_eq!(
        lookup_latest_cc_session_id(&pool, thread_id).await,
        Some("sess-2".to_string()),
        "the newest Init sid must win over an older idle sid and must not be \
         shadowed by an intervening settings-only (no-sid) emit"
    );

    // A later idle (sess-3) supersedes the Init sid.
    emit_idled(&bus, thread_id, Some("sess-3"), None).await;
    assert_eq!(
        lookup_latest_cc_session_id(&pool, thread_id).await,
        Some("sess-3".to_string()),
        "a later CodingAgentIdled sid must win over an earlier Init sid"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The account-scoped session lookup returns the newest session recorded UNDER a
/// given config dir, ignoring newer sessions under other accounts. This is what
/// lets a resume target the pinned account's session after a mid-thread flip.
#[tokio::test]
async fn lookup_session_id_for_config_dir_scopes_to_account() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    seed_session_started(&bus, thread_id, "sess-personal", "claude-code/acct").await;
    // Turn 1 under the personal account.
    emit_settings_with_config_dir(
        &bus,
        thread_id,
        Some("sess-personal"),
        Some("/home/u/.claude-personal"),
    )
    .await;
    // A LATER session recorded under a different account (the mis-pin flip).
    emit_settings_with_config_dir(&bus, thread_id, Some("sess-work"), Some("/home/u/.claude"))
        .await;

    assert_eq!(
        lookup_latest_cc_session_id_for_config_dir(&pool, thread_id, "/home/u/.claude-personal")
            .await,
        Some("sess-personal".to_string()),
        "must return the session under the requested account, not the globally-newest one"
    );
    assert_eq!(
        lookup_latest_cc_session_id_for_config_dir(&pool, thread_id, "/home/u/.claude").await,
        Some("sess-work".to_string()),
        "the other account resolves its own session"
    );
    assert_eq!(
        lookup_latest_cc_session_id_for_config_dir(&pool, thread_id, "/home/u/.claude-nope").await,
        None,
        "an account with no recorded session resolves None"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Regression for the "thread mis-pinned onto another account can't get back" bug
/// (thread 555ad2fb). A thread started under `.claude-personal` (session A), then
/// a later fresh spawn recorded session B under `.claude` (a toggle flip). With
/// the thread pinned to its FIRST account (`.claude-personal`), the auto-detect
/// resume must target session A — not the globally-newest session B — so the next
/// turn resumes the original conversation on the original account.
#[tokio::test]
async fn resolve_resume_context_resumes_session_under_pinned_account() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let branch = "claude-code/mis-pinned";

    // Turn 1: session A under the personal account, then idled.
    seed_session_started(&bus, thread_id, "sess-A", branch).await;
    emit_settings_with_config_dir(
        &bus,
        thread_id,
        Some("sess-A"),
        Some("/home/u/.claude-personal"),
    )
    .await;
    emit_idled(&bus, thread_id, Some("sess-A"), None).await;
    // Toggle flip: a later session B recorded under a DIFFERENT account, then idled
    // (this is the globally-newest session + lifecycle event).
    emit_settings_with_config_dir(&bus, thread_id, Some("sess-B"), Some("/home/u/.claude")).await;
    emit_idled(&bus, thread_id, Some("sess-B"), None).await;

    // The pin is the FIRST account.
    let pinned = lookup_pinned_cc_config_dir(&pool, thread_id).await;
    assert_eq!(pinned.as_deref(), Some("/home/u/.claude-personal"));

    // Auto-detect resume (no caller sid), scoped to the pin: resumes session A, not B.
    let (sid, resume_branch) = resolve_resume_context(
        &pool,
        bus.changes_projection(),
        thread_id,
        None,
        pinned.as_deref(),
    )
    .await;
    assert_eq!(
        sid,
        Some("sess-A".to_string()),
        "a mis-pinned thread must resume the session under its FIRST account, not the newest one"
    );
    assert_eq!(resume_branch, Some(branch.to_string()));

    // Sanity: without the pin (legacy), it would grab the globally-newest session B.
    let (legacy_sid, _) =
        resolve_resume_context(&pool, bus.changes_projection(), thread_id, None, None).await;
    assert_eq!(
        legacy_sid,
        Some("sess-B".to_string()),
        "with no pin, the resolver falls back to the globally-newest session"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

// -------------------- Phase 6.1 worktree-path tests --------------------

/// Helper: create a temp git repo with an initial commit on `main`.
async fn make_test_repo() -> (tempfile::TempDir, std::path::PathBuf) {
    use crate::engine::git_ops::git_cmd;
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().to_path_buf();
    let _ = git_cmd(&["init"], &repo).await;
    let _ = git_cmd(&["checkout", "-b", "main"], &repo).await;
    tokio::fs::write(repo.join("init.txt"), "initial")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &repo).await;
    let _ = git_cmd(&["commit", "-m", "initial commit"], &repo).await;
    (tmp, repo)
}

async fn emit_idled(
    bus: &EventBus,
    thread_id: Uuid,
    cc_session_id: Option<&str>,
    worktree_path: Option<&std::path::Path>,
) {
    emit(
        bus,
        thread_id,
        ThreadEvent::CodingAgentIdled {
            has_changes: false,
            is_external_repo: false,
            requires_restart: false,
            cc_session_id: cc_session_id.map(String::from),
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            reason: None,
            worktree_path: worktree_path.map(|p| p.to_string_lossy().into_owned()),
            worktree_head_sha: None,
            bg_bash_pending: false,
        },
    )
    .await;
}

#[test]
fn deterministic_path_is_short_thread_prefix_under_workspace_worktrees() {
    let workspace = std::path::PathBuf::from("/tmp/ws");
    let thread_id = Uuid::parse_str("01234567-89ab-cdef-0123-456789abcdef").unwrap();
    let path = deterministic_worktree_path(&workspace, thread_id);
    assert_eq!(
        path,
        std::path::PathBuf::from("/tmp/ws/.lucidos/worktrees/thread-01234567")
    );
}

#[tokio::test]
async fn first_turn_creates_deterministic_worktree_path() {
    let (pool, db_name) = setup_test_db().await;
    let (_repo_tmp, repo_root) = make_test_repo().await;
    let workspace = tempfile::tempdir().unwrap();
    let thread_id = Uuid::new_v4();

    let path = resolve_worktree_path(&pool, thread_id, workspace.path(), &repo_root, None).await;
    let expected_suffix = format!(
        "thread-{}",
        &thread_id.simple().to_string()[..THREAD_WORKTREE_ID_LEN]
    );
    assert!(
        path.ends_with(&expected_suffix),
        "expected suffix {} not present in {}",
        expected_suffix,
        path.display()
    );
    assert!(path.starts_with(workspace.path()));

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn subsequent_turns_reuse_recorded_worktree_path() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let (_repo_tmp, repo_root) = make_test_repo().await;
    let workspace = tempfile::tempdir().unwrap();
    let thread_id = Uuid::new_v4();

    // Promote the thread to CC lifecycle before emitting CodingAgentIdled.
    seed_session_started(&bus, thread_id, "sid-1", "claude-code/test").await;

    // Simulate Phase-6.1 idle that recorded a path.
    let recorded = workspace.path().join(".lucidos/worktrees/thread-deadbeef");
    emit_idled(&bus, thread_id, Some("sid-1"), Some(&recorded)).await;

    let resolved =
        resolve_worktree_path(&pool, thread_id, workspace.path(), &repo_root, None).await;
    assert_eq!(
        resolved, recorded,
        "second turn must reuse the path recorded on the prior CodingAgentIdled"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn legacy_threads_resolve_via_git_worktree_list_fallback() {
    use crate::engine::git_ops::git_cmd;

    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let (_repo_tmp, repo_root) = make_test_repo().await;
    let workspace = tempfile::tempdir().unwrap();
    let thread_id = Uuid::new_v4();
    let branch = "claude-code/legacy-feature";

    // Promote thread to CC lifecycle.
    seed_session_started(&bus, thread_id, "sid-legacy", branch).await;

    // Legacy CodingAgentIdled: no `worktree_path` field, but a branch
    // hint exists and the worktree is on disk.
    emit_idled(&bus, thread_id, Some("sid-legacy"), None).await;

    // Create a worktree on the branch outside the workspace dir to prove
    // the fallback returns the on-disk location, not the deterministic
    // workspace path.
    let wt_tmp = tempfile::tempdir().unwrap();
    let wt_path = wt_tmp.path().join("legacy-wt");
    let out = git_cmd(
        &["worktree", "add", wt_path.to_str().unwrap(), "-b", branch],
        &repo_root,
    )
    .await
    .unwrap();
    assert!(
        out.status.success(),
        "worktree add failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let resolved =
        resolve_worktree_path(&pool, thread_id, workspace.path(), &repo_root, Some(branch)).await;
    // `git worktree list` may canonicalize symlinks (macOS resolves
    // `/var/folders/...` → `/private/var/folders/...`), so compare
    // canonicalized paths to make the assertion symlink-tolerant.
    let canon = |p: &std::path::Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    assert_eq!(
        canon(&resolved),
        canon(&wt_path),
        "legacy thread with branch hint must resolve via git worktree list"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn legacy_thread_with_no_on_disk_worktree_falls_through_to_deterministic_path() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let (_repo_tmp, repo_root) = make_test_repo().await;
    let workspace = tempfile::tempdir().unwrap();
    let thread_id = Uuid::new_v4();

    seed_session_started(&bus, thread_id, "sid-stale", "claude-code/no-worktree").await;
    // Legacy idle without worktree_path, branch no longer has a worktree.
    emit_idled(&bus, thread_id, Some("sid-stale"), None).await;

    let resolved = resolve_worktree_path(
        &pool,
        thread_id,
        workspace.path(),
        &repo_root,
        Some("claude-code/no-worktree"),
    )
    .await;
    let expected = deterministic_worktree_path(workspace.path(), thread_id);
    assert_eq!(
        resolved, expected,
        "must fall through to deterministic path when branch has no worktree on disk"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn lookup_latest_worktree_path_returns_none_for_legacy_events() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    seed_session_started(&bus, thread_id, "sid-1", "claude-code/legacy").await;
    emit_idled(&bus, thread_id, Some("sid-1"), None).await;

    let path = lookup_latest_worktree_path(&pool, thread_id).await;
    assert!(path.is_none(), "legacy idle must yield None");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn lookup_latest_worktree_path_returns_recorded_value() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    seed_session_started(&bus, thread_id, "sid-1", "claude-code/wt").await;
    let p = std::path::PathBuf::from("/some/wt/path");
    emit_idled(&bus, thread_id, Some("sid-1"), Some(&p)).await;

    let got = lookup_latest_worktree_path(&pool, thread_id).await;
    assert_eq!(got, Some(p));

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn lookup_latest_worktree_path_picks_most_recent_idle() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    seed_session_started(&bus, thread_id, "sid-1", "claude-code/wt2").await;
    let earlier = std::path::PathBuf::from("/old/path");
    let later = std::path::PathBuf::from("/new/path");
    emit_idled(&bus, thread_id, Some("sid-1"), Some(&earlier)).await;
    emit_idled(&bus, thread_id, Some("sid-2"), Some(&later)).await;

    let got = lookup_latest_worktree_path(&pool, thread_id).await;
    assert_eq!(got, Some(later));

    pool.close().await;
    teardown_test_db(&db_name).await;
}

// -------------------- Phase 8.1 worktree_head_sha tests --------------------

/// Helper that fully populates the new Phase-8.1 field so we can assert
/// SHAs round-trip through serde + the projection lookup helper.
async fn emit_idled_with_sha(
    bus: &EventBus,
    thread_id: Uuid,
    cc_session_id: Option<&str>,
    worktree_path: Option<&std::path::Path>,
    head_sha: Option<&str>,
) {
    emit(
        bus,
        thread_id,
        ThreadEvent::CodingAgentIdled {
            has_changes: false,
            is_external_repo: false,
            requires_restart: false,
            cc_session_id: cc_session_id.map(String::from),
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            reason: None,
            worktree_path: worktree_path.map(|p| p.to_string_lossy().into_owned()),
            worktree_head_sha: head_sha.map(String::from),
            bg_bash_pending: false,
        },
    )
    .await;
}

/// Phase 8.1 contract: an idle event carrying `worktree_head_sha` must
/// persist the field through the EventBus → DB → projection round-trip
/// so the next spawn can diff against the recorded SHA.
#[tokio::test]
async fn idled_event_includes_worktree_head_sha() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let sha = "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef";

    seed_session_started(&bus, thread_id, "sid-1", "claude-code/sha").await;
    emit_idled_with_sha(
        &bus,
        thread_id,
        Some("sid-1"),
        Some(std::path::Path::new("/some/wt")),
        Some(sha),
    )
    .await;

    let got = lookup_latest_worktree_head_sha(&pool, thread_id).await;
    assert_eq!(
        got.as_deref(),
        Some(sha),
        "the SHA written to CodingAgentIdled must round-trip through the projection"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn lookup_latest_worktree_head_sha_returns_none_for_legacy_events() {
    // Legacy CodingAgentIdled (Phase 8 not yet shipped) → no SHA field.
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    seed_session_started(&bus, thread_id, "sid-1", "claude-code/legacy-sha").await;
    emit_idled(&bus, thread_id, Some("sid-1"), None).await;

    let got = lookup_latest_worktree_head_sha(&pool, thread_id).await;
    assert!(got.is_none(), "legacy idle must yield None");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn lookup_latest_worktree_head_sha_picks_most_recent_idle() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    seed_session_started(&bus, thread_id, "sid-1", "claude-code/sha2").await;
    let earlier = "1111111111111111111111111111111111111111";
    let later = "2222222222222222222222222222222222222222";
    emit_idled_with_sha(&bus, thread_id, Some("sid-1"), None, Some(earlier)).await;
    emit_idled_with_sha(&bus, thread_id, Some("sid-2"), None, Some(later)).await;

    let got = lookup_latest_worktree_head_sha(&pool, thread_id).await;
    assert_eq!(
        got.as_deref(),
        Some(later),
        "the most recent CodingAgentIdled must win"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

// -------------------- Phase 8.2 + 8.3 integration-style tests --------------------

/// Phase 8.2 contract: when a thread has a prior `CodingAgentIdled` with
/// a recorded `worktree_head_sha` AND the worktree on disk has moved
/// since, [`super::super::external_edits::compute_external_edit_note`] driven by
/// [`lookup_latest_worktree_head_sha`] produces a non-empty note for the
/// next spawn to inject. Exercises the lookup → helper handoff that
/// `run_direct_agent` performs at spawn time.
#[tokio::test]
async fn external_edits_produce_injected_note() {
    use crate::engine::git_ops::git_cmd;

    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let (_repo_tmp, repo_root) = make_test_repo().await;
    let thread_id = Uuid::new_v4();
    seed_session_started(&bus, thread_id, "sid-edits", "claude-code/edits").await;

    // Snapshot the SHA the agent saw on its prior idle.
    let _ = git_cmd(&["config", "user.email", "t@t"], &repo_root).await;
    let _ = git_cmd(&["config", "user.name", "t"], &repo_root).await;
    let head = git_cmd(&["rev-parse", "HEAD"], &repo_root).await.unwrap();
    let last_sha = String::from_utf8_lossy(&head.stdout).trim().to_string();
    emit_idled_with_sha(
        &bus,
        thread_id,
        Some("sid-edits"),
        Some(&repo_root),
        Some(&last_sha),
    )
    .await;

    // Simulate the user externally committing in the worktree.
    tokio::fs::write(repo_root.join("user.txt"), "user did this")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &repo_root).await;
    let _ = git_cmd(&["commit", "-m", "user commit between turns"], &repo_root).await;

    // Drive the same lookup → helper sequence run_direct_agent uses.
    let recorded_sha = lookup_latest_worktree_head_sha(&pool, thread_id).await;
    assert!(recorded_sha.is_some(), "test setup must record a SHA");
    let note = super::super::external_edits::compute_external_edit_note(
        &repo_root,
        recorded_sha.as_deref(),
        false,
    )
    .await
    .expect("non-empty diff against recorded SHA must produce a note");

    assert!(note.contains("user commit between turns"), "note: {}", note);
    assert!(note.starts_with("[Note from engine"));

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Phase 8.3 contract: when the user externally checks out a different
/// branch in the worktree, [`super::super::external_edits::verify_branch`] fails
/// loudly. `run_direct_agent` propagates the failure as a spawn refusal.
#[tokio::test]
async fn spawn_refuses_when_user_checked_out_different_branch() {
    use crate::engine::git_ops::git_cmd;

    let (_pool, _db_name) = setup_test_db().await;
    let (_repo_tmp, repo_root) = make_test_repo().await;

    // Engine expects to spawn on `claude-code/feature` …
    let expected_branch = "claude-code/feature";
    let _ = git_cmd(&["checkout", "-b", expected_branch], &repo_root).await;

    // … but the user externally jumped to a different branch.
    let _ = git_cmd(&["checkout", "-b", "user-detour"], &repo_root).await;

    let err = super::super::external_edits::verify_branch(&repo_root, expected_branch)
        .await
        .expect_err("branch mismatch must refuse the spawn");
    assert_eq!(err.expected, expected_branch);
    assert_eq!(err.found.as_deref(), Some("user-detour"));
    let msg = format!("{}", err);
    assert!(msg.contains("user-detour"));
    assert!(msg.contains(expected_branch));
    assert!(msg.contains("Resolve manually"));

    _pool.close().await;
    teardown_test_db(&_db_name).await;
}
