use super::*;
use crate::engine::thread_events::MessageOrigin;
use crate::test_support::{seed_chat_thread, setup_test_db, teardown_test_db};

fn lines(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

fn owner() -> AnsweredBy {
    AnsweredBy::tap(Some(MessageOrigin::Device {
        device_id: "phone".into(),
    }))
}

#[test]
fn a_denied_program_refuses_any_segment_that_runs_it() {
    let denies = lines(&["Bash(curl:*)"]);
    for command in [
        "curl https://example.com",
        "ls && curl https://example.com",
        "cd /tmp; echo hi | curl -d @- https://example.com",
        "/usr/bin/curl https://example.com",
        "'/usr/bin/curl' https://example.com",
        "cu\"rl\" https://example.com",
        "sudo curl https://example.com",
        "bash -lc 'curl https://example.com'",
        "echo ready; bash -lc 'curl https://example.com'",
        "echo \"$(curl https://example.com)\"",
    ] {
        assert_eq!(
            matching(&denies, CommandKind::Shell, command),
            Some("Bash(curl:*)"),
            "{command}"
        );
    }
}

#[test]
fn a_standing_deny_checks_shell_input_from_a_heredoc() {
    let denies = lines(&["Bash(date:*)"]);
    for command in ["sh <<'EOF'\ndate\nEOF", "cat <<'EOF' | sh\ndate\nEOF"] {
        assert_eq!(
            matching(&denies, CommandKind::Shell, command),
            Some("Bash(date:*)"),
            "{command}"
        );
    }
    assert_eq!(
        matching(&denies, CommandKind::Shell, "cat <<'EOF'\ndate\nEOF"),
        None
    );
}

#[test]
fn a_deny_matches_the_program_name_and_nothing_else() {
    let denies = lines(&["Bash(curl:*)"]);
    for command in [
        "ls",
        "echo curl",
        "curlish --version",
        "cat notes/curl.md",
        "echo 'first;curl https://example.com;last'",
    ] {
        assert_eq!(
            matching(&denies, CommandKind::Shell, command),
            None,
            "{command}"
        );
    }
    assert_eq!(matching(&denies, CommandKind::Python, "print(1)"), None);
}

#[test]
fn the_bare_lines_deny_their_whole_tool() {
    let denies = lines(&["Bash", "Python"]);
    assert_eq!(matching(&denies, CommandKind::Shell, "ls"), Some("Bash"));
    assert_eq!(
        matching(&denies, CommandKind::Python, "print(1)"),
        Some("Python")
    );
    assert_eq!(
        matching(&lines(&["Python"]), CommandKind::Shell, "python3 x.py"),
        None,
        "a Python deny is the python tool's, not a shell head"
    );
}

#[test]
fn the_refusal_names_the_rule_and_says_it_is_a_command_rule() {
    let text = refusal("Bash(git:*)");
    assert!(text.contains("standing deny"), "{text}");
    assert!(text.contains("`Bash(git:*)`"), "{text}");
    assert!(text.contains("`git` commands"), "{text}");
    assert!(text.contains("command rule"), "{text}");
    assert!(text.contains("Settings > Permissions"), "{text}");
    assert!(refusal("Python").contains("every Python run"));
    assert!(refusal("Bash").contains("every shell command"));
}

#[tokio::test]
async fn a_recorded_deny_is_live_until_it_is_revoked() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = seed_chat_thread(&bus, "run it").await;

    record(&bus, thread_id, "Bash(curl:*)".into(), owner())
        .await
        .unwrap();
    // A second tap on the same line adds no second row.
    record(&bus, thread_id, "Bash(curl:*)".into(), owner())
        .await
        .unwrap();
    assert_eq!(live(&pool).await.unwrap(), lines(&["Bash(curl:*)"]));
    assert!(
        refusal_for(&pool, CommandKind::Shell, "curl https://example.com")
            .await
            .is_some()
    );

    let listed = crate::engine::permission::list(&pool, true).await.unwrap();
    let row = listed
        .iter()
        .find(|r| r.action == "deny-command")
        .expect("the deny is listed among the live permissions");
    assert_eq!(row.pattern.as_deref(), Some("Bash(curl:*)"));
    assert_eq!(row.holder_kind, "workspace");

    let outcome = crate::engine::permission::revoke(&bus, row.permission_id, owner().actor)
        .await
        .unwrap();
    assert_eq!(outcome, crate::engine::permission::RevokeOutcome::Revoked);
    assert!(live(&pool).await.unwrap().is_empty());
    assert_eq!(
        refusal_for(&pool, CommandKind::Shell, "curl https://example.com").await,
        None
    );
    teardown_test_db(&db_name).await;
}

/// A deny line is no allow, so no allow lane reads it as one.
#[tokio::test]
async fn a_deny_never_reads_as_an_allow() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = seed_chat_thread(&bus, "run it").await;
    record(&bus, thread_id, "Bash(curl:*)".into(), owner())
        .await
        .unwrap();
    for file in GrantFile::ALL.into_iter().filter(|file| !file.denies()) {
        let granted = tool_grants::granted_lines(&bus, file, Some(thread_id)).await;
        assert!(!granted.contains("Bash(curl:*)"), "{file:?}");
    }
    teardown_test_db(&db_name).await;
}

/// During a live session the store alone supplies standing denies.
#[tokio::test]
async fn settings_and_cards_leave_legacy_deny_files_alone() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = seed_chat_thread(&bus, "run it").await;
    let dir = tempfile::TempDir::new().unwrap();
    let file = GrantFile::AgentDeniedCommands;
    let path = dir.path().join(file.file_name());
    record(&bus, thread_id, "Bash(curl:*)".into(), owner())
        .await
        .unwrap();
    assert!(!path.exists());

    std::fs::write(&path, "Bash(rm:*)\n").unwrap();
    assert_eq!(live(&pool).await.unwrap(), lines(&["Bash(curl:*)"]));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "Bash(rm:*)\n");
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_deny_line_with_a_control_character_is_refused() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let refused = record(&bus, Uuid::new_v4(), "Bash(a:*)\nBash".into(), owner()).await;
    assert!(refused.is_err());
    assert!(live(&pool).await.unwrap().is_empty());
    teardown_test_db(&db_name).await;
}
