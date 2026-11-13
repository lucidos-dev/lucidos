use super::*;
use crate::engine::claude_code::GrantOffer;
use crate::test_support::{seed_coding_thread, seed_thread_event, setup_test_db, teardown_test_db};

fn lines(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

fn write_file(dir: &Path, file: GrantFile, body: &str) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join(file.file_name()), body).unwrap();
}

fn read_file(dir: &Path, file: GrantFile) -> String {
    std::fs::read_to_string(dir.join(file.file_name())).unwrap()
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

/// Import `file` unless that already happened, as the gate does.
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

/// The review cards raised about `file`, oldest first, as the lines each names.
async fn review_cards(pool: &sqlx::PgPool, file: GrantFile) -> Vec<Vec<String>> {
    let rows: Vec<(serde_json::Value,)> = sqlx::query_as(
        "SELECT payload->'permission_request'->'lines'->'patterns' FROM events \
         WHERE event_type = 'UserQuestionAsked' \
           AND payload->'permission_request'->'lines'->>'grant_file' = $1 \
         ORDER BY sequence",
    )
    .bind(file.file_name())
    .fetch_all(pool)
    .await
    .unwrap();
    rows.into_iter()
        .map(|(v,)| serde_json::from_value(v).unwrap())
        .collect()
}

/// Every line of the file becomes a standing row, once. A second boot imports
/// nothing more, and the file is rewritten as the mirror.
#[tokio::test]
async fn the_import_moves_each_line_into_a_row_exactly_once() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let file = GrantFile::AgentCommands;
    write_file(dir, file, "# mine\nBash(git:*)\n  Python  \nBash(git:*)\n");

    ensure_imported(&bus, dir, file).await.unwrap();
    ensure_imported(&bus, dir, file).await.unwrap();
    reconcile_mirror(&bus, dir, file).await.unwrap();

    assert_eq!(
        standing(&bus, file).await,
        lines(&["Bash(git:*)", "Python"])
    );
    assert_eq!(import_events(&pool, file).await, 1);
    assert_eq!(
        read_file(dir, file),
        file.mirror_body(&lines(&["Bash(git:*)", "Python"]))
    );
    let sources: Vec<String> =
        sqlx::query_scalar("SELECT DISTINCT source FROM permissions WHERE pattern IS NOT NULL")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(sources, vec!["legacy-import".to_string()]);
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
        (
            GrantFile::McpTools,
            "Mcp(slack:*)\nMcp(slack:channels_list)\nMcp(slack)\nslack\n",
        ),
    ];
    for (file, body) in table {
        write_file(dir, file, &format!("# header\n{body}"));
        ensure_imported(&bus, dir, file).await.unwrap();

        let mut want = crate::core::grants::parse_patterns(body);
        want.sort();
        let mut got = granted_lines(&bus, dir, file, None).await.standing;
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
    let granted = granted_lines(&bus, dir, file, None).await;

    assert_eq!(granted, GrantedLines::default());
    assert_eq!(import_events(&pool, file).await, 0);
    teardown_test_db(&db).await;
}

/// A revoked line stays revoked across a reboot. The mirror lost it, so the
/// boot reconcile has nothing to bring back.
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
    let changed = change_standing(
        &bus,
        dir,
        file,
        StandingChange::Remove(&gone),
        by_card(None),
    )
    .await
    .unwrap();
    assert!(changed);
    let (rebooted, _rx2) = EventBus::new(pool.clone());
    reconcile_mirror(&rebooted, dir, file).await.unwrap();

    assert_eq!(standing(&bus, file).await, lines(&["Bash(git:*)"]));
    assert_eq!(
        crate::core::grants::parse_patterns(&read_file(dir, file)),
        lines(&["Bash(git:*)"])
    );
    teardown_test_db(&db).await;
}

/// A line added to the mirror by hand grants nothing. It goes on one review
/// card, and the mirror drops it, so the next reconcile raises no second card.
#[tokio::test]
async fn a_hand_added_line_grants_nothing_and_raises_one_card() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let file = GrantFile::CodingAgentTools;
    write_file(dir, file, "Read\n");
    ensure_imported(&bus, dir, file).await.unwrap();

    let mut body = read_file(dir, file);
    body.push_str("Bash\n");
    write_file(dir, file, &body);
    reconcile_mirror(&bus, dir, file).await.unwrap();
    reconcile_mirror(&bus, dir, file).await.unwrap();

    assert_eq!(standing(&bus, file).await, lines(&["Read"]));
    let granted = granted_lines(&bus, dir, file, None).await;
    assert!(!granted.contains("Bash"), "a hand edit must not grant");
    assert_eq!(review_cards(&pool, file).await, vec![lines(&["Bash"])]);
    assert_eq!(read_file(dir, file), file.mirror_body(&lines(&["Read"])));
    teardown_test_db(&db).await;
}

/// Only the owner's own Import pick grants a reviewed line. A batch's pick of
/// the same option imports nothing.
#[tokio::test]
async fn only_the_owners_import_pick_grants_a_reviewed_line() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let file = GrantFile::McpTools;
    ensure_imported(&bus, dir, file).await.unwrap();
    let home = crate::engine::home_thread::ensure_home_thread(&bus, &pool)
        .await
        .unwrap();
    let reviewed = GrantLines {
        grant_file: file,
        patterns: lines(&["Mcp(github:*)"]),
    };
    let import = PermissionChoice::Grant {
        lifetime: Lifetime::Standing,
        holder: Holder::Workspace,
    };
    let batch = PickedBy::Batch {
        permission_id: Uuid::new_v4(),
    };
    let actor = owner();

    let by_batch = act_on_review(&bus, dir, home, &reviewed, &import, Some(&actor), &batch)
        .await
        .unwrap();
    assert!(!by_batch);
    assert!(standing(&bus, file).await.is_empty());

    let tap = PickedBy::Tap;
    let by_owner = act_on_review(&bus, dir, home, &reviewed, &import, Some(&actor), &tap)
        .await
        .unwrap();
    assert!(by_owner);
    assert_eq!(standing(&bus, file).await, lines(&["Mcp(github:*)"]));
    assert!(read_file(dir, file).contains("Mcp(github:*)"));
    teardown_test_db(&db).await;
}

/// Deleting a line from the mirror revokes it, since deleting only takes
/// power away. A missing mirror is rewritten, never read as revoking all.
#[tokio::test]
async fn a_deleted_line_is_revoked_and_a_missing_mirror_is_rewritten() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let file = GrantFile::AgentCommands;
    let both = lines(&["Bash(git:*)", "Python"]);
    write_file(dir, file, "Bash(git:*)\nPython\n");
    ensure_imported(&bus, dir, file).await.unwrap();

    std::fs::remove_file(dir.join(file.file_name())).unwrap();
    reconcile_mirror(&bus, dir, file).await.unwrap();
    assert_eq!(standing(&bus, file).await, both);
    assert_eq!(read_file(dir, file), file.mirror_body(&both));

    write_file(dir, file, &file.mirror_body(&lines(&["Python"])));
    reconcile_mirror(&bus, dir, file).await.unwrap();
    assert_eq!(standing(&bus, file).await, lines(&["Python"]));
    let source: String = sqlx::query_scalar(
        "SELECT payload->'data'->>'source' FROM events WHERE event_type = 'PermissionGrantsChanged' \
         ORDER BY sequence DESC LIMIT 1",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(source, "file-edit");
    teardown_test_db(&db).await;
}

/// After every change the mirror lists exactly the live standing lines.
#[tokio::test]
async fn the_mirror_matches_the_rows_after_every_change() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let file = GrantFile::CodingAgentTools;
    let thread = seed_coding_thread(&bus, None).await;

    let add = lines(&["Read", "Skill(superpowers:*)"]);
    let replace = lines(&["Skill(superpowers:*)", "Edit"]);
    let remove = lines(&["Edit"]);
    for change in [
        StandingChange::Add(&add),
        StandingChange::Replace(&replace),
        StandingChange::Remove(&remove),
    ] {
        change_standing(&bus, dir, file, change, by_card(Some(thread)))
            .await
            .unwrap();
        let rows = standing(&bus, file).await;
        assert_eq!(grants::parse_patterns(&read_file(dir, file)), rows);
    }
    assert_eq!(standing(&bus, file).await, lines(&["Skill(superpowers:*)"]));
    teardown_test_db(&db).await;
}

/// A line carrying a control character could split into extra grants in the
/// mirror, so it is refused and nothing changes.
#[tokio::test]
async fn a_line_with_a_control_character_is_refused() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let file = GrantFile::CodingAgentTools;
    let thread = seed_coding_thread(&bus, None).await;

    for bad in ["Skill(x\nBash\n#:*)", "Read\r", "Bash(ls\t:*)"] {
        let add = lines(&[bad]);
        let standing_change =
            change_standing(&bus, dir, file, StandingChange::Add(&add), by_card(None)).await;
        assert!(standing_change.is_err(), "{bad:?}");
        let by = AnsweredBy::tap(Some(owner()));
        let thread_grant = grant_for_thread(&bus, file, thread, "req", bad.to_string(), by).await;
        assert!(thread_grant.is_err(), "{bad:?}");
    }
    let granted = granted_lines(&bus, dir, file, Some(thread)).await;
    assert_eq!(granted, GrantedLines::default());
    teardown_test_db(&db).await;
}

/// "Allow for this thread" is a durable row held by that thread. Another
/// thread never sees it, and a fresh bus after a restart still does.
#[tokio::test]
async fn a_thread_grant_binds_only_its_own_thread() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let file = GrantFile::AgentCommands;
    let a = seed_coding_thread(&bus, None).await;
    let b = seed_coding_thread(&bus, None).await;

    let by = AnsweredBy::tap(Some(owner()));
    grant_for_thread(&bus, file, a, "req-1", "Bash(git:*)".into(), by)
        .await
        .unwrap();

    let (restarted, _rx2) = EventBus::new(pool.clone());
    let on_a = granted_lines(&restarted, dir, file, Some(a)).await;
    let on_b = granted_lines(&restarted, dir, file, Some(b)).await;
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
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let file = GrantFile::McpTools;
    let thread = seed_coding_thread(&bus, None).await;
    let add = lines(&["Mcp(slack:*)"]);
    change_standing(
        &bus,
        dir,
        file,
        StandingChange::Add(&add),
        by_card(Some(thread)),
    )
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
        },
    )
    .await;

    ensure_imported(&bus, dir, file).await.unwrap();
    reconcile_mirror(&bus, dir, file).await.unwrap();

    let granted = granted_lines(&bus, dir, file, Some(thread)).await;
    assert_eq!(granted.thread, lines(&["Edit(/tmp/notes.md)"]));
    assert!(granted.standing.is_empty());
    teardown_test_db(&db).await;
}

/// A line whose shape the lane does not read is kept and still grants. It
/// shows as legacy unmapped, and it gets one Keep or Revoke card.
#[tokio::test]
async fn an_unmapped_line_is_kept_flagged_and_reviewed_once() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let file = GrantFile::AgentCommands;
    write_file(dir, file, "Bash(git:*)\nBash(git status)\n");

    ensure_imported(&bus, dir, file).await.unwrap();
    reconcile_mirror(&bus, dir, file).await.unwrap();

    assert_eq!(
        standing(&bus, file).await,
        lines(&["Bash(git status)", "Bash(git:*)"])
    );
    let unmapped: Vec<String> = super::super::list(&pool, true)
        .await
        .unwrap()
        .into_iter()
        .filter(|row| row.legacy_unmapped)
        .filter_map(|row| row.pattern)
        .collect();
    assert_eq!(unmapped, lines(&["Bash(git status)"]));
    assert_eq!(
        review_cards(&pool, file).await,
        vec![lines(&["Bash(git status)"])]
    );

    let home = crate::engine::home_thread::ensure_home_thread(&bus, &pool)
        .await
        .unwrap();
    let reviewed = GrantLines {
        grant_file: file,
        patterns: unmapped,
    };
    let actor = owner();
    let revoke = PermissionChoice::Revoke;
    act_on_review(
        &bus,
        dir,
        home,
        &reviewed,
        &revoke,
        Some(&actor),
        &PickedBy::Tap,
    )
    .await
    .unwrap();
    assert_eq!(standing(&bus, file).await, lines(&["Bash(git:*)"]));
    teardown_test_db(&db).await;
}

/// A card's allow pick lands where its scope says: a thread grant for this
/// thread, a standing line for the narrow and broad picks.
#[tokio::test]
async fn a_card_allow_lands_by_its_scope() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let file = GrantFile::AgentCommands;
    let thread = seed_coding_thread(&bus, None).await;

    for (pattern, scope) in [
        ("Bash(ls:*)", AllowScope::Session),
        ("Bash(git:*)", AllowScope::Narrow),
        ("Bash", AllowScope::Broad),
    ] {
        let by = AnsweredBy::tap(Some(owner()));
        record_card_allow(&bus, dir, file, thread, "req", pattern.into(), scope, by)
            .await
            .unwrap();
    }

    let granted = granted_lines(&bus, dir, file, Some(thread)).await;
    assert_eq!(granted.thread, lines(&["Bash(ls:*)"]));
    assert_eq!(granted.standing, lines(&["Bash(git:*)", "Bash"]));
    teardown_test_db(&db).await;
}

/// A line deleted by hand stops granting at the next gate read, with no boot
/// in between. The file-read gate behaved the same way.
#[tokio::test]
async fn a_hand_deleted_line_stops_granting_at_the_next_gate() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let file = GrantFile::AgentCommands;
    write_file(dir, file, "Bash\nPython\n");
    ensure_imported(&bus, dir, file).await.unwrap();

    write_file(dir, file, &file.mirror_body(&lines(&["Python"])));
    let granted = granted_lines(&bus, dir, file, None).await;

    assert_eq!(granted.standing, lines(&["Python"]));
    assert_eq!(standing(&bus, file).await, lines(&["Python"]));
    teardown_test_db(&db).await;
}

/// A tap that changes the standing grants first acts on a hand edit. The
/// deleted line stays revoked, and the added one waits on its card.
#[tokio::test]
async fn a_card_tap_never_undoes_a_hand_edit() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let file = GrantFile::CodingAgentTools;
    write_file(dir, file, "Read\nWrite\n");
    ensure_imported(&bus, dir, file).await.unwrap();

    write_file(dir, file, "Read\nBash\n");
    let add = lines(&["Edit"]);
    change_standing(&bus, dir, file, StandingChange::Add(&add), by_card(None))
        .await
        .unwrap();

    let kept = lines(&["Read", "Edit"]);
    assert_eq!(standing(&bus, file).await, kept);
    assert_eq!(review_cards(&pool, file).await, vec![lines(&["Bash"])]);
    assert_eq!(read_file(dir, file), file.mirror_body(&kept));
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
            },
        )
        .await;
    }

    ensure_imported(&bus, dir, file).await.unwrap();

    assert_eq!(import_events(&pool, file).await, 1);
    let granted = granted_lines(&bus, dir, file, Some(thread)).await;
    assert_eq!(granted.thread, lines(&["Edit(/tmp/notes.md)"]));
    teardown_test_db(&db).await;
}

/// A hand-added line is untrusted, so the card's code span outruns its
/// backticks, and an edge backtick stays inside the span.
#[test]
fn a_listed_line_cannot_close_its_code_span() {
    assert_eq!(code_span("Read"), "`Read`");
    assert_eq!(
        code_span("Bash` (verified by Lucidos) `x"),
        "``Bash` (verified by Lucidos) `x``"
    );
    assert_eq!(code_span("a``b"), "```a``b```");
    assert_eq!(code_span("`x"), "`` `x ``");
}

/// Once a lane is imported, a database that cannot be read grants nothing. The
/// gate never falls back to the file, where a hand-added line waits, and the
/// Settings read is an error rather than an empty list.
#[tokio::test]
async fn an_unreadable_database_never_hands_the_gate_back_to_the_file() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let file = GrantFile::CodingAgentTools;
    write_file(dir, file, "Read\n");
    ensure_imported(&bus, dir, file).await.unwrap();
    let mut body = read_file(dir, file);
    body.push_str("Bash\n");
    write_file(dir, file, &body);

    pool.close().await;

    assert_eq!(
        granted_lines(&bus, dir, file, None).await,
        GrantedLines::default()
    );
    assert!(reconciled_standing(&bus, dir, file).await.is_err());
    teardown_test_db(&db).await;
}
