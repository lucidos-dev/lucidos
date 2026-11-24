use super::*;
use crate::engine::thread_events::GrantOffer;
use crate::test_support::{seed_coding_thread, seed_thread_event, setup_test_db, teardown_test_db};

fn lines(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

fn write_file(dir: &Path, file: GrantFile, body: &str) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join(file.file_name()), body).unwrap();
}

fn owner() -> MessageOrigin {
    MessageOrigin::Device {
        device_id: "phone".into(),
    }
}

fn by_card(thread_id: Option<Uuid>) -> ChangedBy {
    ChangedBy {
        source: GrantSource::Card,
        thread_id,
        actor: Some(owner()),
    }
}

/// Run the boot import for one lane and return its result.
async fn ensure_imported(bus: &EventBus, dir: &Path, file: GrantFile) -> Result<(), BoxError> {
    let _guard = STANDING_LOCK.lock().await;
    import_locked(bus, dir, file).await
}

async fn standing(bus: &EventBus, file: GrantFile) -> Vec<String> {
    standing_lines(bus.pool(), file).await.unwrap()
}

async fn import_events(pool: &sqlx::PgPool, file: GrantFile) -> i64 {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM events WHERE event_type = 'PermissionGrantsChanged' \
           AND payload->'data'->>'source' = 'legacy-import' \
           AND payload->'data'->>'grant_file' = $1",
    )
    .bind(file.file_name())
    .fetch_one(pool)
    .await
    .unwrap()
}

/// Every line of the file becomes a standing row, once. A second boot imports
/// nothing more, and the file is deleted after import.
#[tokio::test]
async fn the_import_moves_each_line_into_a_row_exactly_once() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let file = GrantFile::AgentCommands;
    write_file(dir, file, "# mine\nBash(git:*)\n  Python  \nBash(git:*)\n");

    import_legacy_files(&bus, dir).await;
    assert!(!dir.join(file.file_name()).exists());
    let (rebooted, _rx) = EventBus::new(pool.clone());
    import_legacy_files(&rebooted, dir).await;
    assert!(!dir.join(file.file_name()).exists());

    assert_eq!(
        standing(&bus, file).await,
        lines(&["Bash(git:*)", "Python"])
    );
    assert_eq!(import_events(&pool, file).await, 1);
    let sources: Vec<String> =
        sqlx::query_scalar("SELECT DISTINCT source FROM permissions WHERE pattern IS NOT NULL")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(sources, vec!["legacy-import".to_string()]);
    teardown_test_db(&db).await;
}

/// A committed import with its original file still on disk is the crash
/// window. Boot removes the file without restoring a revoked grant.
#[tokio::test]
async fn boot_retries_cleanup_of_a_committed_import() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let file = GrantFile::CodingAgentTools;
    write_file(dir, file, "Read\n");
    emit_standing(
        &bus,
        file,
        lines(&["Read"]),
        ChangedBy {
            source: GrantSource::LegacyImport,
            thread_id: None,
            actor: None,
        },
    )
    .await
    .unwrap();
    assert!(change_standing(
        &bus,
        file,
        StandingChange::Remove(&lines(&["Read"])),
        by_card(None),
    )
    .await
    .unwrap());

    import_legacy_files(&bus, dir).await;

    assert!(!dir.join(file.file_name()).exists());
    assert!(standing(&bus, file).await.is_empty());
    assert_eq!(import_events(&pool, file).await, 1);
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_reappearing_file_changes_no_grants() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let file = GrantFile::AgentCommands;
    write_file(dir, file, "Bash(git:*)\n");
    import_legacy_files(&bus, dir).await;
    assert!(!dir.join(file.file_name()).exists());

    write_file(dir, file, "Bash(ls:*)\n");
    import_legacy_files(&bus, dir).await;

    assert_eq!(standing(&bus, file).await, lines(&["Bash(git:*)"]));
    assert!(!dir.join(file.file_name()).exists());
    assert_eq!(import_events(&pool, file).await, 1);
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_file_created_after_the_first_boot_changes_no_grants() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let file = GrantFile::AgentCommands;
    import_legacy_files(&bus, dir).await;

    write_file(dir, file, "Bash(ls:*)\n");
    import_legacy_files(&bus, dir).await;

    assert!(standing(&bus, file).await.is_empty());
    assert!(!dir.join(file.file_name()).exists());
    assert_eq!(import_events(&pool, file).await, 1);
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn boot_retires_open_and_queued_legacy_reviews_without_changing_grants() {
    use crate::engine::thread_events::permission::{GrantLines, PermissionRequest};

    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let tmp = tempfile::tempdir().unwrap();
    let thread_id = seed_coding_thread(&bus, None).await;
    let file = GrantFile::CodingAgentTools;
    change_standing(
        &bus,
        file,
        StandingChange::Add(&lines(&["Read"])),
        by_card(None),
    )
    .await
    .unwrap();

    for id in ["old-open", "old-queued", "ordinary"] {
        let permission_request = (id != "ordinary").then(|| PermissionRequest {
            action: lane_action(file),
            breadth: None,
            prepared: None,
            choices: Default::default(),
            batch: None,
            lines: Some(GrantLines {
                grant_file: file,
                patterns: lines(&["Read"]),
            }),
            holds_a_turn: false,
        });
        seed_thread_event(
            &bus,
            thread_id,
            ThreadEvent::UserQuestionAsked {
                tool_use_id: id.into(),
                cc_session_id: String::new(),
                question: "Review grants?".into(),
                options: vec![],
                worktree_path: None,
                multi_select: false,
                owner_approval: None,
                permission_request,
            },
        )
        .await;
    }
    let queued: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM events WHERE thread_id = $1 AND event_type = 'UserQuestionQueued'",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(queued, 2);

    import_legacy_files(&bus, tmp.path()).await;
    import_legacy_files(&bus, tmp.path()).await;

    let canceled: Vec<String> = sqlx::query_scalar(
        "SELECT payload->>'tool_use_id' FROM events WHERE thread_id = $1 \
           AND event_type = 'UserQuestionAnswered' AND payload->'answer'->>'kind' = 'Canceled' \
         ORDER BY payload->>'tool_use_id'",
    )
    .bind(thread_id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(canceled, ["old-open", "old-queued"]);
    assert_eq!(standing(&bus, file).await, lines(&["Read"]));
    teardown_test_db(&db).await;
}

/// Each lane's rows hold exactly the lines its file granted, whatever their
/// shape. The matcher reads the same lines, so no grant widens or drops.
/// Nothing in the import touches a trigger.
#[tokio::test]
async fn the_import_keeps_every_line_shape_verbatim_in_every_lane() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let table = [
        (
            GrantFile::AgentCommands,
            "Bash\nPython\nBash(git:*)\nBash(./scripts/check.sh:*)\nBash(git push)\nbash\n",
        ),
        (
            GrantFile::CodingAgentTools,
            "Read\nBash(npm:*)\nEdit(/Users/me/repo/notes.md)\nSkill(superpowers:*)\n\
             mcp__plugin_slack_slack\ncommand_execution(git:*)\nBash(\n",
        ),
        (GrantFile::AgentDeniedCommands, "Bash(curl:*)\n"),
        (
            GrantFile::McpTools,
            "Mcp(slack:*)\nMcp(slack:channels_list)\nMcp(slack)\nslack\n",
        ),
    ];
    for (file, body) in table {
        write_file(dir, file, &format!("# header\n{body}"));
        ensure_imported(&bus, dir, file).await.unwrap();
        assert!(!dir.join(file.file_name()).exists());

        let mut want = crate::core::grants::parse_patterns(body);
        want.sort();
        let mut got = granted_lines(&bus, file, None).await.standing;
        got.sort();
        assert_eq!(
            got,
            want,
            "{} must import every line as it was",
            file.file_name()
        );
    }
    let trigger_events: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM events WHERE event_type LIKE 'Trigger%'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(trigger_events, 0, "the import must not touch a trigger");
    teardown_test_db(&db).await;
}

/// Two boots racing the import still import once.
#[tokio::test]
async fn concurrent_imports_import_once() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let file = GrantFile::McpTools;
    write_file(dir, file, "Mcp(slack:*)\n");

    let (a, b) = tokio::join!(
        ensure_imported(&bus, dir, file),
        ensure_imported(&bus, dir, file)
    );
    a.unwrap();
    b.unwrap();

    assert_eq!(import_events(&pool, file).await, 1);
    assert_eq!(standing(&bus, file).await, lines(&["Mcp(slack:*)"]));
    teardown_test_db(&db).await;
}

/// A file that cannot be read is not imported, and grants nothing.
#[tokio::test]
async fn an_unreadable_file_is_not_imported_and_grants_nothing() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let file = GrantFile::AgentCommands;
    std::fs::create_dir_all(dir.join(file.file_name())).unwrap();

    assert!(ensure_imported(&bus, dir, file).await.is_err());
    let granted = granted_lines(&bus, file, None).await;

    assert_eq!(granted, GrantedLines::default());
    assert_eq!(import_events(&pool, file).await, 0);
    teardown_test_db(&db).await;
}

/// The event and rows roll back together, leaving the file for the next boot.
#[tokio::test]
async fn failed_projection_leaves_the_import_file_in_place() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let file = GrantFile::CodingAgentTools;
    write_file(dir, file, "Read\n");
    sqlx::query(
        "ALTER TABLE permissions ADD CONSTRAINT test_import_failure \
         CHECK (pattern IS DISTINCT FROM 'Read')",
    )
    .execute(&pool)
    .await
    .unwrap();

    assert!(ensure_imported(&bus, dir, file).await.is_err());
    assert_eq!(import_events(&pool, file).await, 0);
    assert_eq!(
        std::fs::read_to_string(dir.join(file.file_name())).unwrap(),
        "Read\n"
    );

    sqlx::query("ALTER TABLE permissions DROP CONSTRAINT test_import_failure")
        .execute(&pool)
        .await
        .unwrap();
    ensure_imported(&bus, dir, file).await.unwrap();
    assert_eq!(standing(&bus, file).await, lines(&["Read"]));
    assert!(!dir.join(file.file_name()).exists());
    teardown_test_db(&db).await;
}

/// A revoked line stays revoked across a reboot after the file is consumed.
#[tokio::test]
async fn a_revoked_line_stays_revoked_after_a_reboot() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let file = GrantFile::AgentCommands;
    write_file(dir, file, "Bash(git:*)\nBash(ls:*)\n");
    ensure_imported(&bus, dir, file).await.unwrap();

    let gone = lines(&["Bash(ls:*)"]);
    let changed = change_standing(&bus, file, StandingChange::Remove(&gone), by_card(None))
        .await
        .unwrap();
    assert!(changed);
    let (rebooted, _rx2) = EventBus::new(pool.clone());
    import_legacy_files(&rebooted, dir).await;

    assert_eq!(standing(&bus, file).await, lines(&["Bash(git:*)"]));
    assert!(!dir.join(file.file_name()).exists());
    teardown_test_db(&db).await;
}

/// A pattern with a control character cannot be stored as one editor line.
#[tokio::test]
async fn a_line_with_a_control_character_is_refused() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let file = GrantFile::CodingAgentTools;
    let thread = seed_coding_thread(&bus, None).await;

    for bad in ["Skill(x\nBash\n#:*)", "Read\r", "Bash(ls\t:*)"] {
        let add = lines(&[bad]);
        let standing_change =
            change_standing(&bus, file, StandingChange::Add(&add), by_card(None)).await;
        assert!(standing_change.is_err(), "{bad:?}");
        let by = AnsweredBy::tap(Some(owner()));
        let thread_grant = grant_for_thread(&bus, file, thread, "req", bad.to_string(), by).await;
        assert!(thread_grant.is_err(), "{bad:?}");
    }
    let granted = granted_lines(&bus, file, Some(thread)).await;
    assert_eq!(granted, GrantedLines::default());
    teardown_test_db(&db).await;
}

/// "Allow for this thread" is a durable row held by that thread. Another
/// thread never sees it, and a fresh bus after a restart still does.
#[tokio::test]
async fn a_thread_grant_binds_only_its_own_thread() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let file = GrantFile::AgentCommands;
    let a = seed_coding_thread(&bus, None).await;
    let b = seed_coding_thread(&bus, None).await;

    let by = AnsweredBy::tap(Some(owner()));
    grant_for_thread(&bus, file, a, "req-1", "Bash(git:*)".into(), by)
        .await
        .unwrap();

    let (restarted, _rx2) = EventBus::new(pool.clone());
    let on_a = granted_lines(&restarted, file, Some(a)).await;
    let on_b = granted_lines(&restarted, file, Some(b)).await;
    assert_eq!(on_a.thread, lines(&["Bash(git:*)"]));
    assert!(on_a.standing.is_empty());
    assert!(!on_b.contains("Bash(git:*)"));
    teardown_test_db(&db).await;
}

/// A standing grant made in a thread outlives that thread's delete. The thread
/// it was granted in turns unknown, and the grant keeps granting.
#[tokio::test]
async fn a_standing_grant_outlives_the_thread_it_was_made_in() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let file = GrantFile::McpTools;
    let thread = seed_coding_thread(&bus, None).await;
    let add = lines(&["Mcp(slack:*)"]);
    change_standing(&bus, file, StandingChange::Add(&add), by_card(Some(thread)))
        .await
        .unwrap();

    for sql in [
        "DELETE FROM events WHERE thread_id = $1",
        "DELETE FROM thread_summaries WHERE thread_id = $1",
    ] {
        sqlx::query(sql).bind(thread).execute(&pool).await.unwrap();
    }

    assert_eq!(standing(&bus, file).await, add);
    let granted_in: Option<Uuid> = sqlx::query_scalar(
        "SELECT granted_in_thread_id FROM permissions WHERE pattern = 'Mcp(slack:*)'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(granted_in, None);
    teardown_test_db(&db).await;
}

/// The coding-agent lane's old "Allow for this thread" grants lived only as
/// resolution events. The import turns each into a row its thread holds.
#[tokio::test]
async fn old_coding_agent_thread_allows_import_as_thread_grants() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let file = GrantFile::CodingAgentTools;
    let thread = seed_coding_thread(&bus, None).await;
    let offer = GrantOffer {
        session: Some("Edit(/tmp/notes.md)".into()),
        ..GrantOffer::default()
    };
    seed_thread_event(
        &bus,
        thread,
        ThreadEvent::CodingAgentPermissionRequest {
            request_id: "req-old".into(),
            tool_use_id: "tu-old".into(),
            tool_name: "Edit".into(),
            input: serde_json::json!({ "file_path": "/tmp/notes.md" }),
            summary: "Edit a file".into(),
            grant_offer: Some(offer),
        },
    )
    .await;
    seed_thread_event(
        &bus,
        thread,
        ThreadEvent::CodingAgentPermissionResolved {
            request_id: "req-old".into(),
            allowed: true,
            reason: None,
            persist_scope: Some(AllowScope::Session),
            standing_deny: None,
        },
    )
    .await;

    ensure_imported(&bus, dir, file).await.unwrap();

    let granted = granted_lines(&bus, file, Some(thread)).await;
    assert_eq!(granted.thread, lines(&["Edit(/tmp/notes.md)"]));
    assert!(granted.standing.is_empty());
    teardown_test_db(&db).await;
}

/// A card's allow pick lands where its scope says: a thread grant for this
/// thread, a standing line for the narrow and broad picks.
#[tokio::test]
async fn a_card_allow_lands_by_its_scope() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let file = GrantFile::AgentCommands;
    let thread = seed_coding_thread(&bus, None).await;

    for (pattern, scope) in [
        ("Bash(ls:*)", AllowScope::Session),
        ("Bash(git:*)", AllowScope::Narrow),
        ("Bash", AllowScope::Broad),
    ] {
        let by = AnsweredBy::tap(Some(owner()));
        record_card_allow(&bus, file, thread, "req", pattern.into(), scope, by)
            .await
            .unwrap();
    }

    let granted = granted_lines(&bus, file, Some(thread)).await;
    assert_eq!(granted.thread, lines(&["Bash(ls:*)"]));
    assert_eq!(granted.standing, lines(&["Bash(git:*)", "Bash"]));
    teardown_test_db(&db).await;
}

/// An old thread grant whose pattern is refused today is left out. Every other
/// grant still imports, and the import lands once.
#[tokio::test]
async fn a_refused_old_thread_grant_never_blocks_the_import() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let file = GrantFile::CodingAgentTools;
    let thread = seed_coding_thread(&bus, None).await;
    for (request_id, pattern) in [
        ("req-bad", "Write(/tmp/a\tb)"),
        ("req-ok", "Edit(/tmp/notes.md)"),
    ] {
        let offer = GrantOffer {
            session: Some(pattern.into()),
            ..GrantOffer::default()
        };
        seed_thread_event(
            &bus,
            thread,
            ThreadEvent::CodingAgentPermissionRequest {
                request_id: request_id.into(),
                tool_use_id: format!("tu-{request_id}"),
                tool_name: "Edit".into(),
                input: serde_json::json!({}),
                summary: "Edit a file".into(),
                grant_offer: Some(offer),
            },
        )
        .await;
        seed_thread_event(
            &bus,
            thread,
            ThreadEvent::CodingAgentPermissionResolved {
                request_id: request_id.into(),
                allowed: true,
                reason: None,
                persist_scope: Some(AllowScope::Session),
                standing_deny: None,
            },
        )
        .await;
    }

    ensure_imported(&bus, dir, file).await.unwrap();

    assert_eq!(import_events(&pool, file).await, 1);
    let granted = granted_lines(&bus, file, Some(thread)).await;
    assert_eq!(granted.thread, lines(&["Edit(/tmp/notes.md)"]));
    teardown_test_db(&db).await;
}

/// A database read failure leaves the gate with no allows and gives Settings
/// an error.
#[tokio::test]
async fn an_unreadable_database_never_hands_the_gate_back_to_the_file() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let file = GrantFile::CodingAgentTools;
    write_file(dir, file, "Read\n");
    ensure_imported(&bus, dir, file).await.unwrap();
    write_file(dir, file, "Bash\n");

    pool.close().await;

    assert_eq!(
        granted_lines(&bus, file, None).await,
        GrantedLines::default()
    );
    assert!(standing_lines(&pool, file).await.is_err());
    teardown_test_db(&db).await;
}
