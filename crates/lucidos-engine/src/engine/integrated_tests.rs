use super::*;
use crate::engine::event_bus::{BusEvent, EventBus, SystemEvent};
use crate::engine::git_ops::record_hardened;
use crate::engine::thread_events::{
    ActorMode, EventChannel, EventMeta, ThreadEvent, UnproposedReason,
};
use crate::engine::thread_queue::ThreadQueueRequest;
use crate::test_support::{
    message_received, seed_thread_event, setup_test_db, start_cc_session, teardown_test_db,
};

/// Start a Lucidos-source session on `branch`, integrated or not.
async fn start_session(bus: &EventBus, thread_id: Uuid, branch: &str, integrated: bool) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::SessionStarted {
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            session_id: String::new(),
            branch: branch.into(),
            repo_id: None,
            coding_agent_kind: Default::default(),
            coding_agent_folder: String::new(),
            app_id: None,
            integrated,
            auto_compact_window: None,
            auto_compact_arm: None,
            continuity: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        },
    })
    .await
    .expect("start the session");
}

async fn set_parent(pool: &sqlx::PgPool, child: Uuid, parent: Uuid) {
    sqlx::query("UPDATE thread_summaries SET parent_thread_id = $2 WHERE thread_id = $1")
        .bind(child)
        .bind(parent)
        .execute(pool)
        .await
        .expect("link the child");
}

/// A `SessionStarted` written before integrated spawns existed carries no
/// flag. It reads as independent, and its JSON stays the old bytes.
#[test]
fn an_old_session_started_reads_as_independent() {
    let old = serde_json::json!({
        "type": "SessionStarted",
        "session_id": "s",
        "branch": "claude-code/x",
        "coding_agent": "claude-code",
    });
    let event: ThreadEvent = serde_json::from_value(old).expect("old row parses");
    assert!(matches!(
        event,
        ThreadEvent::SessionStarted {
            integrated: false,
            ..
        }
    ));
    let written = serde_json::to_value(&event).expect("serialize");
    assert!(written.get("integrated").is_none(), "{written}");
}

/// The flag rides the first session into the projection and stays for the
/// thread's life. A thread that never named it is independent, and so is one
/// the store does not know.
#[tokio::test]
async fn the_integrated_flag_is_recorded_and_kept() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let (child, independent) = (Uuid::new_v4(), Uuid::new_v4());
    start_session(&bus, child, "claude-code/child", true).await;
    start_cc_session(&bus, independent, "claude-code/independent", None).await;

    assert!(thread_is_integrated(&pool, child).await);
    assert!(!thread_is_integrated(&pool, independent).await);
    assert!(!thread_is_integrated(&pool, Uuid::new_v4()).await);

    // A later session, as a resume emits it, cannot clear the flag.
    start_session(&bus, child, "claude-code/child", false).await;
    assert!(thread_is_integrated(&pool, child).await);

    assert_eq!(
        expected_harden_tier(&pool, "claude-code/child").await,
        HardenTier::Fast
    );
    assert_eq!(
        expected_harden_tier(&pool, "claude-code/independent").await,
        HardenTier::Full
    );
    assert_eq!(
        expected_harden_tier(&pool, "claude-code/nobody").await,
        HardenTier::Full
    );

    pool.close().await;
    teardown_test_db(&db).await;
}

/// Set-aside work is applied as the child's own change, and Apply needs a
/// full marker. So once the branch has a change of its own, its `/harden`
/// runs the full tier, or Bring back would offer work nothing can apply.
#[tokio::test]
async fn an_integrated_branch_with_its_own_change_needs_the_full_tier() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let child = Uuid::new_v4();
    start_session(&bus, child, "claude-code/set-aside-child", true).await;
    assert_eq!(
        expected_harden_tier(&pool, "claude-code/set-aside-child").await,
        HardenTier::Fast
    );

    bus.emit(BusEvent::Thread {
        thread_id: child,
        event: ThreadEvent::ChangeProposed {
            change_id: Uuid::new_v4().to_string(),
            description: Some("work".into()),
            files: vec!["a.rs".into()],
            requires_restart: false,
            origin: None,
            commit_sha: None,
            branch_name: "claude-code/set-aside-child".into(),
            repo_root: "/tmp/repo".into(),
            hardened: false,
            incomplete: false,
            set_aside: true,
            path: String::new(),
            diff: String::new(),
        },
        meta: EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        },
    })
    .await
    .expect("set aside");

    assert!(thread_is_integrated(&pool, child).await, "the flag stays");
    assert_eq!(
        expected_harden_tier(&pool, "claude-code/set-aside-child").await,
        HardenTier::Full
    );

    pool.close().await;
    teardown_test_db(&db).await;
}

/// The `lucidos spawn-thread` request for a coding-agent child of `parent`.
fn agent_chat_spawn(child: Uuid, parent: Uuid, integrated: bool) -> ThreadQueueRequest {
    ThreadQueueRequest::AgentChat {
        depth: 0,
        message: "follow up on the sweep".into(),
        thread_id: child,
        event_id: None,
        image_hashes: vec![],
        device_id: None,
        model: None,
        reasoning_effort: None,
        provider: None,
        use_coding_agent: Some(true),
        repo_id: None,
        cc_model: None,
        coding_agent: None,
        title: None,
        mode: ActorMode::Agent,
        origin: None,
        parent_thread_id: Some(parent),
        spawning_event_id: None,
        wake_mode: None,
        app_id: None,
        integrated,
        spawning_trigger_id: None,
    }
}

/// Queue a child's spawn, as `ThreadQueue::submit` records it, then write its
/// first message. No session starts: the first run failed before one.
async fn spawn_child_without_a_session(
    bus: &EventBus,
    parent: Uuid,
    integrated: bool,
) -> (Uuid, Uuid) {
    let (entry_id, child) = (Uuid::new_v4(), Uuid::new_v4());
    let request = agent_chat_spawn(child, parent, integrated);
    bus.emit(BusEvent::System(SystemEvent::ThreadQueued {
        entry_id,
        kind: request.kind(),
        trigger_id: None,
        trigger_name: None,
        thread_id: Some(child),
        summary: request.summary(None),
        request: serde_json::to_value(&request).expect("serialize the request"),
        requeued: false,
        actor: None,
    }))
    .await
    .expect("queue the spawn");
    let mut first = message_received("follow up on the sweep");
    if let ThreadEvent::MessageReceived {
        parent_thread_id, ..
    } = &mut first
    {
        *parent_thread_id = Some(parent);
    }
    seed_thread_event(bus, child, first).await;
    (entry_id, child)
}

/// The child's first message records the flag. A first run that never reaches
/// `SessionStarted` still reads integrated, and so does a restart that hands
/// the queue entry off.
#[tokio::test]
async fn an_integrated_child_is_integrated_before_its_first_session() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let parent = Uuid::new_v4();
    start_cc_session(&bus, parent, "claude-code/parent", None).await;
    let (entry_id, child) = spawn_child_without_a_session(&bus, parent, true).await;
    let (_, independent) = spawn_child_without_a_session(&bus, parent, false).await;

    assert!(thread_is_integrated(&pool, child).await);
    assert!(!thread_is_integrated(&pool, independent).await);

    // The boot handoff deletes the queue row, and a follow-up names no parent.
    bus.emit(BusEvent::System(SystemEvent::ThreadQueueCompleted {
        entry_id,
    }))
    .await
    .expect("hand the entry off");
    seed_thread_event(&bus, child, message_received("and the docs")).await;
    assert!(thread_is_integrated(&pool, child).await);

    pool.close().await;
    teardown_test_db(&db).await;
}

/// The parent's review skips what its integrated children's runs reviewed: it
/// lists their recorded heads, of either tier, and never an independent
/// child's or another parent's.
#[tokio::test]
async fn a_parent_lists_its_integrated_childrens_hardened_heads() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let repo = tempfile::tempdir().expect("repo dir");
    let (parent, fast, full, unmarked, independent, stranger) = (
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
    );
    start_cc_session(&bus, parent, "claude-code/parent", None).await;
    for (child, branch, integrated) in [
        (fast, "claude-code/fast", true),
        (full, "claude-code/full", true),
        (unmarked, "claude-code/unmarked", true),
        (independent, "claude-code/independent", false),
    ] {
        start_session(&bus, child, branch, integrated).await;
        set_parent(&pool, child, parent).await;
    }
    start_session(&bus, stranger, "claude-code/stranger", true).await;
    for (branch, sha, tier) in [
        ("claude-code/fast", "aaa1", HardenTier::Fast),
        ("claude-code/full", "bbb2", HardenTier::Full),
        ("claude-code/independent", "ccc3", HardenTier::Full),
        ("claude-code/stranger", "ddd4", HardenTier::Fast),
    ] {
        record_hardened(&pool, repo.path(), branch, sha, tier)
            .await
            .expect("record a marker");
    }

    assert_eq!(
        integrated_child_heads(&pool, "claude-code/parent")
            .await
            .expect("heads"),
        vec!["aaa1".to_string(), "bbb2".to_string()]
    );
    assert!(integrated_child_heads(&pool, "claude-code/nobody")
        .await
        .expect("heads")
        .is_empty());

    pool.close().await;
    teardown_test_db(&db).await;
}

/// The report names the child's branch and head only once its work waits as
/// handed up. Unhardened work hands nothing up.
#[tokio::test]
async fn a_child_hands_up_its_branch_only_once_its_work_waits_for_the_parent() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let repo = tempfile::tempdir().expect("repo dir");
    let child = Uuid::new_v4();
    let branch = "claude-code/hand-up";
    start_session(&bus, child, branch, true).await;
    record_hardened(&pool, repo.path(), branch, "feed1", HardenTier::Fast)
        .await
        .expect("record the fast marker");
    let withhold = |reason| {
        crate::engine::change_ops::emit_proposal_withheld(&bus, child, branch, &[], reason)
    };

    withhold(UnproposedReason::HardeningMissing).await;
    assert_eq!(handed_up_branch(&pool, child).await, None);

    withhold(UnproposedReason::IntegratedIntoParent).await;
    assert_eq!(
        handed_up_branch(&pool, child).await,
        Some(IntegratedBranch {
            branch_name: branch.into(),
            head_sha: "feed1".into(),
        })
    );

    pool.close().await;
    teardown_test_db(&db).await;
}

fn spawn() -> IntegratedSpawn {
    IntegratedSpawn {
        has_parent: true,
        coding_agent: true,
        lucidos_source: true,
        parent_is_lucidos_coding_agent: true,
        thread_exists: false,
        human_mode: false,
    }
}

/// Only a new Lucidos-source coding-agent child of a Lucidos-source
/// coding-agent parent may be integrated. Each refusal says what to change.
#[test]
fn an_integrated_spawn_needs_a_lucidos_coding_agent_child_and_parent() {
    assert_eq!(integrated_spawn_refusal(&spawn()), None);
    let refusal = |s: IntegratedSpawn| integrated_spawn_refusal(&s).expect("refused");
    let cases = [
        (
            IntegratedSpawn {
                thread_exists: true,
                ..spawn()
            },
            "starts a new thread",
        ),
        (
            IntegratedSpawn {
                human_mode: true,
                ..spawn()
            },
            "--mode human",
        ),
        (
            IntegratedSpawn {
                has_parent: false,
                ..spawn()
            },
            "--relation child",
        ),
        (
            IntegratedSpawn {
                coding_agent: false,
                ..spawn()
            },
            "--coding-agent",
        ),
        (
            IntegratedSpawn {
                lucidos_source: false,
                ..spawn()
            },
            "Lucidos-source child",
        ),
        (
            IntegratedSpawn {
                parent_is_lucidos_coding_agent: false,
                ..spawn()
            },
            "coding-agent parent",
        ),
    ];
    for (unfit, names) in cases {
        let refused = refusal(unfit);
        assert!(refused.contains(names), "{refused}");
    }
}
