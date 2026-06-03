use super::*;
pub(super) use crate::engine::event_bus::{BusEvent, EventBus};
pub(super) use crate::engine::thread_events::{EventChannel, EventMeta, ThreadEvent};
pub(super) use crate::test_support::{setup_test_db, teardown_test_db};

pub(super) fn aggregate_proposed(change_id: Uuid, branch: &str, repo_root: &str) -> ThreadEvent {
    ThreadEvent::ChangeProposed {
        change_id: change_id.to_string(),
        description: Some("aggregate description".to_string()),
        files: vec!["a.rs".to_string(), "b.rs".to_string()],
        requires_restart: false,
        origin: None,
        commit_sha: None,
        branch_name: branch.to_string(),
        repo_root: repo_root.to_string(),
        hardened: true,
        incomplete: false,
        path: String::new(),
        diff: String::new(),
    }
}

pub(super) fn per_commit_proposed(
    branch: &str,
    sha: &str,
    subject: &str,
    requires_restart: bool,
) -> ThreadEvent {
    ThreadEvent::ChangeProposed {
        change_id: String::new(),
        description: Some(subject.to_string()),
        files: vec!["c.rs".to_string()],
        requires_restart,
        origin: None,
        commit_sha: Some(sha.to_string()),
        branch_name: branch.to_string(),
        repo_root: String::new(),
        hardened: false,
        incomplete: false,
        path: String::new(),
        diff: String::new(),
    }
}

pub(super) fn applied_event(change_id: Uuid, commits: &[&str], requires_restart: bool) -> ThreadEvent {
    ThreadEvent::ChangeApplied {
        change_id: change_id.to_string(),
        requires_restart,
        client_update: false,
        commits: commits.iter().map(|s| s.to_string()).collect(),
        thread_title: None,
        actor: None,
        pre_merge_sha: None,
        post_merge_sha: None,
        path: String::new(),
    }
}

pub(super) fn discarded_event(change_id: Uuid) -> ThreadEvent {
    ThreadEvent::ChangeDiscarded {
        change_id: change_id.to_string(),
        actor: None,
        path: String::new(),
    }
}

pub(super) async fn start_cc_thread(bus: &EventBus, thread_id: Uuid) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::SessionStarted {
            session_id: format!("cc-{thread_id}"),
            branch: String::new(),
            repo_id: None,
            coding_agent_kind: Default::default(),
            coding_agent_folder: String::new(),
            app_id: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
}

pub(super) async fn emit(bus: &EventBus, thread_id: Uuid, event: ThreadEvent) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event,
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
}

/// Buffer to put around a cutoff so the surrounding `resolved_at`
/// timestamps land strictly on the expected side. Has to absorb scheduler
/// jitter and Docker network round-trips on macOS, where 1ms is too tight.
pub(super) const CUTOFF_GAP_MS: u64 = 50;

/// Returns Postgres `NOW()` so cutoffs are in the same clock as
/// `resolved_at` (set via SQL `NOW()`). Mixing Rust `Utc::now()` with
/// Postgres NOW() flakes under clock drift between host and the Postgres
/// container (notably after laptop sleep/wake on macOS Docker).
pub(super) async fn pg_now(pool: &PgPool) -> DateTime<Utc> {
    sqlx::query_scalar("SELECT NOW()")
        .fetch_one(pool)
        .await
        .expect("pg_now: SELECT NOW() failed")
}

pub(super) fn proposed_with_files(
    change_id: Uuid,
    branch: &str,
    repo_root: &str,
    files: Vec<&str>,
) -> ThreadEvent {
    ThreadEvent::ChangeProposed {
        change_id: change_id.to_string(),
        description: Some("d".into()),
        files: files.iter().map(|s| s.to_string()).collect(),
        requires_restart: false,
        origin: None,
        commit_sha: None,
        branch_name: branch.into(),
        repo_root: repo_root.into(),
        hardened: true,
        incomplete: false,
        path: String::new(),
        diff: String::new(),
    }
}
