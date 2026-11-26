use super::*;

fn read_only_database_url(url: &str) -> Option<String> {
    lucidos_pg_url::PgUrl::parse(url)
        .map(|url| url.read_only_url())
        .ok()
}

#[test]
fn the_handle_is_the_agent_role_url_boot_recorded() {
    crate::test_support::install_agent_database_url();
    let (name, url) = lucidos_database_url_env().expect("boot recorded one");
    assert_eq!(name, LUCIDOS_DATABASE_URL);
    assert_eq!(Some(&url), AGENT_DATABASE_URL.get());
    assert_ne!(read_only_database_url(&database_url()), Some(url));
}

#[test]
fn sqlx_reads_the_packaged_socket_url_as_a_socket() {
    use std::str::FromStr as _;
    let options = sqlx::postgres::PgConnectOptions::from_str(
        "postgres:///lucidos_myws?host=%2FUsers%2Fme%20x%2F.lucidos%2Fprivate%2Frun%2Fpg-0123&port=5432&user=lucidos&password=s3cr%26t",
    )
    .unwrap();
    assert_eq!(
        options.get_socket(),
        Some(&std::path::PathBuf::from(
            "/Users/me x/.lucidos/private/run/pg-0123"
        ))
    );
    assert_eq!(options.get_port(), 5432);
    assert_eq!(options.get_username(), "lucidos");
    assert_eq!(options.get_database(), Some("lucidos_myws"));
}

#[test]
fn sqlx_reads_the_inlined_passfile_url() {
    use std::str::FromStr as _;
    let dir = tempfile::tempdir().unwrap();
    let passfile = dir.path().join("superuser.pgpass");
    std::fs::write(
        &passfile,
        lucidos_pg_url::passfile_line("lucidos", "s3cr&t"),
    )
    .unwrap();
    let url = format!(
        "postgres:///lucidos_myws?host=%2Frun%2Fpg-0123&port=5432&user=lucidos&passfile={}",
        urlencoding::encode(&passfile.to_string_lossy())
    );
    let (inlined, named) = lucidos_pg_url::inline_passfile(&url).unwrap();
    assert_eq!(named.as_deref(), Some(passfile.as_path()));
    assert!(!inlined.contains("passfile"), "{inlined}");
    let options = sqlx::postgres::PgConnectOptions::from_str(&inlined).unwrap();
    assert_eq!(
        options.get_socket(),
        Some(&std::path::PathBuf::from("/run/pg-0123"))
    );
    assert_eq!(options.get_username(), "lucidos");
    assert_eq!(
        lucidos_pg_url::PgUrl::parse(&inlined)
            .unwrap()
            .password
            .as_deref(),
        Some("s3cr&t")
    );
}

#[test]
fn sqlx_reads_the_read_only_socket_handle() {
    use std::str::FromStr as _;
    let handle = read_only_database_url(
        "postgres:///lucidos_myws?host=%2FUsers%2Fme%20x%2Frun%2Fpg-0123&user=lucidos",
    )
    .expect("renders");
    let options = sqlx::postgres::PgConnectOptions::from_str(&handle).unwrap();
    assert_eq!(
        options.get_socket(),
        Some(&std::path::PathBuf::from("/Users/me x/run/pg-0123"))
    );
    assert_eq!(options.get_username(), "lucidos");
    assert_eq!(options.get_database(), Some("lucidos_myws"));
    assert_eq!(
        options.get_options(),
        Some("-c default_transaction_read_only=on")
    );
}

#[test]
fn redact_postgres_secrets_masks_password_in_url() {
    assert_eq!(
        redact_postgres_secrets(
            "psql postgres://lucidos:lucidos@localhost:5432/lucidos -c 'select 1'"
        ),
        "psql postgres://lucidos:***@localhost:5432/lucidos -c 'select 1'"
    );
}

#[test]
fn redact_postgres_secrets_handles_postgresql_scheme() {
    assert_eq!(
        redact_postgres_secrets("postgresql://u:secret@h:5432/d"),
        "postgresql://u:***@h:5432/d"
    );
}

#[test]
fn redact_postgres_secrets_replaces_every_occurrence() {
    let s = "a postgres://u1:p1@h/d1 b postgres://u2:p2@h/d2";
    assert_eq!(
        redact_postgres_secrets(s),
        "a postgres://u1:***@h/d1 b postgres://u2:***@h/d2"
    );
}

#[test]
fn redact_postgres_secrets_leaves_innocuous_text_alone() {
    let s = "echo hello world";
    assert_eq!(redact_postgres_secrets(s), s);
}

#[test]
fn redact_postgres_secrets_does_not_match_passwordless_url() {
    let s = "postgres://lucidos@host/db";
    assert_eq!(redact_postgres_secrets(s), s);
}

#[test]
fn redact_secret_values_masks_each_secret_occurrence() {
    let secrets = vec![
        "ya29.super-secret-token".to_string(),
        "hunter2pw".to_string(),
    ];
    let text = "Authorization: Bearer ya29.super-secret-token (pw=hunter2pw, again hunter2pw)";
    assert_eq!(
        redact_secret_values(text, &secrets),
        "Authorization: Bearer [REDACTED] (pw=[REDACTED], again [REDACTED])"
    );
}

#[test]
fn redact_secret_values_skips_trivially_short_secrets() {
    let secrets = vec!["ab".to_string(), "x".to_string()];
    let text = "abc x ab xy";
    assert_eq!(redact_secret_values(text, &secrets), text);
}

#[test]
fn redact_secret_values_redacts_longest_first_no_partial_leftover() {
    let secrets = vec!["secret".to_string(), "secretvalue".to_string()];
    assert_eq!(
        redact_secret_values("token=secretvalue", &secrets),
        "token=[REDACTED]"
    );
}

#[test]
fn redact_secret_values_leaves_text_without_secrets_alone() {
    let secrets = vec!["never-present-token".to_string()];
    let text = "nothing sensitive here";
    assert_eq!(redact_secret_values(text, &secrets), text);
}

#[test]
fn redact_secret_bytes_masks_a_secret_inside_bytes_that_are_not_text() {
    let secrets = vec!["tok-1234".to_string()];
    let mut body = vec![0xff, 0x00, 0xfe];
    body.extend_from_slice(b"tok-1234");
    body.extend_from_slice(&[0x80, 0x81]);
    body.extend_from_slice(b"tok-1234");

    let mut expected = vec![0xff, 0x00, 0xfe];
    expected.extend_from_slice(REDACTED_MARKER.as_bytes());
    expected.extend_from_slice(&[0x80, 0x81]);
    expected.extend_from_slice(REDACTED_MARKER.as_bytes());
    assert_eq!(redact_secret_bytes(&body, &secrets), Some(expected));
}

#[test]
fn redact_secret_bytes_reports_no_change_when_no_secret_occurs() {
    let secrets = vec!["tok-1234".to_string(), "ab".to_string()];
    assert_eq!(redact_secret_bytes(b"ab tok-123 ab", &secrets), None);
}

#[test]
fn redact_secret_values_keeps_multibyte_text_around_a_secret() {
    let secrets = vec!["tøken-æ".to_string()];
    assert_eq!(
        redact_secret_values("før tøken-æ etter", &secrets),
        format!("før {REDACTED_MARKER} etter")
    );
}

#[test]
fn redact_postgres_secrets_in_json_walks_nested_strings() {
    let mut v = serde_json::json!({
        "command": "psql postgres://lucidos:lucidos@localhost:5432/lucidos -c 'select 1'",
        "timeout_secs": 60,
        "nested": {
            "url": "postgresql://u:p@h/d",
            "ok": "no postgres url here"
        },
        "list": [
            "postgres://a:b@c/d",
            42,
            "harmless"
        ]
    });
    redact_postgres_secrets_in_json(&mut v);
    assert_eq!(
        v["command"],
        "psql postgres://lucidos:***@localhost:5432/lucidos -c 'select 1'"
    );
    assert_eq!(v["nested"]["url"], "postgresql://u:***@h/d");
    assert_eq!(v["nested"]["ok"], "no postgres url here");
    assert_eq!(v["list"][0], "postgres://a:***@c/d");
    assert_eq!(v["list"][1], 42);
    assert_eq!(v["list"][2], "harmless");
    assert_eq!(v["timeout_secs"], 60);
}

#[test]
fn redact_postgres_secrets_in_json_masks_a_url_used_as_a_key() {
    let mut v = serde_json::json!({
        "postgres://u:p@h/d": 1,
        "nested": [{ "postgresql://a:b@c/d": "x" }],
    });
    redact_postgres_secrets_in_json(&mut v);
    assert_eq!(
        v,
        serde_json::json!({
            "postgres://u:***@h/d": 1,
            "nested": [{ "postgresql://a:***@c/d": "x" }],
        })
    );
}

#[test]
fn a_key_masked_to_another_masked_key_keeps_both_values() {
    let mut v = serde_json::json!({ "a": 1, "b": 2 });
    mask_json_strings(&mut v, &|s| match s {
        "a" => Some("b".to_string()),
        "b" => Some("c".to_string()),
        _ => None,
    });
    assert_eq!(v, serde_json::json!({ "b": 1, "c": 2 }));
}

#[test]
fn test_describe_tool_ask_user_question_single_shows_question() {
    let args = serde_json::json!({
        "questions": [{ "question": "Should I proceed?", "options": [] }]
    });
    assert_eq!(
        describe_tool("ask_user_question", &args),
        "Asking: Should I proceed?"
    );
}

#[test]
fn test_describe_tool_ask_user_question_falls_back_to_header() {
    let args = serde_json::json!({
        "questions": [{ "question": "", "header": "Approach", "options": [] }]
    });
    assert_eq!(
        describe_tool("ask_user_question", &args),
        "Asking: Approach"
    );
}

#[test]
fn test_describe_tool_knowhow_without_an_id_names_no_id() {
    let empty = serde_json::json!({});
    assert_eq!(describe_tool("load_knowhow", &empty), "Loading know-how...");
    assert_eq!(
        describe_tool("unload_knowhow", &empty),
        "Unloading know-how..."
    );
    assert_eq!(
        describe_tool("load_knowhow", &serde_json::json!({"id": "ops/x"})),
        "Loading know-how 'ops/x'..."
    );
}

#[test]
fn test_describe_tool_await_event_leads_with_the_reason() {
    let args = serde_json::json!({
        "on": [{ "event_type": "ChangeProposed" }],
        "timeout_secs": 3600,
        "reason": "the release build to finish"
    });
    assert_eq!(
        describe_tool("await_event", &args),
        "Waiting: the release build to finish"
    );
}

#[test]
fn test_describe_tool_await_event_does_not_say_waiting_twice() {
    let label = |reason: &str| {
        describe_tool(
            "await_event",
            &serde_json::json!({ "on": [{ "event_type": "E2ELockReleased" }], "reason": reason }),
        )
    };
    assert_eq!(label("waiting for the e2e lock"), "Waiting: the e2e lock");
    assert_eq!(label("Waiting until tonight"), "Waiting: tonight");
    assert_eq!(label("waiting  for  the lock"), "Waiting: the lock");
    assert_eq!(
        label("the lock another run is waiting for"),
        "Waiting: the lock another run is waiting for"
    );
    assert_eq!(label("waiting for"), "Waiting: waiting for");
    assert_eq!(label("waiting formally"), "Waiting: waiting formally");
}

#[test]
fn test_describe_tool_await_event_without_a_reason_still_says_what_it_is() {
    let args = serde_json::json!({ "on": [{ "event_type": "ChangeProposed" }] });
    assert_eq!(
        describe_tool("await_event", &args),
        "Waiting for an event..."
    );
}

#[test]
fn test_describe_tool_ask_user_question_multiple() {
    let args = serde_json::json!({
        "questions": [
            { "question": "A?", "options": [] },
            { "question": "B?", "options": [] },
            { "question": "C?", "options": [] }
        ]
    });
    assert_eq!(
        describe_tool("ask_user_question", &args),
        "Asking 3 questions..."
    );
}

#[test]
fn test_describe_tool_ask_user_question_empty_falls_back() {
    let args = serde_json::json!({ "questions": [] });
    assert_eq!(
        describe_tool("ask_user_question", &args),
        "Asking a question..."
    );
}

#[test]
fn test_describe_tool_todo_write() {
    let writing = serde_json::json!({
        "todos": [{ "content": "Run tests", "active_form": "Running tests", "status": "pending" }]
    });
    assert_eq!(
        describe_tool("todo_write", &writing),
        "Updating todo list..."
    );
    let clearing = serde_json::json!({ "todos": [] });
    assert_eq!(
        describe_tool("todo_write", &clearing),
        "Clearing todo list..."
    );
    let reading = serde_json::json!({ "action": "read" });
    assert_eq!(
        describe_tool("todo_write", &reading),
        "Reading todo list..."
    );
}

#[test]
fn test_describe_tool_background_exec() {
    assert_eq!(
        describe_tool(
            "run_python_background",
            &serde_json::json!({ "code": "x = 1" })
        ),
        "Running Python in background..."
    );
    assert_eq!(
        describe_tool(
            "run_bash_background",
            &serde_json::json!({ "command": "sleep 99" })
        ),
        "Running in background: sleep 99..."
    );
    assert_eq!(
        describe_tool("bash_output", &serde_json::json!({ "task_id": "abc" })),
        "Checking background task output..."
    );
    assert_eq!(
        describe_tool("bash_kill", &serde_json::json!({ "task_id": "abc" })),
        "Stopping background task..."
    );
}

#[test]
fn describe_tool_never_doubles_the_trailing_ellipsis_on_an_elided_value() {
    let long = "cd /Users/me/workspaces/example && for f in *.json; do jq . \"$f\"; done";
    assert!(long.len() > 60, "fixture must exceed every elision budget");
    let args = serde_json::json!({ "command": long, "prompt": long, "message": long });

    for (tool, prefix) in [
        ("run_bash", "Running: "),
        ("run_bash_background", "Running in background: "),
        ("generate_image", "Generating image: "),
        ("run_thread", "Running thread: "),
        ("follow_up_child_thread", "Following up with child thread: "),
    ] {
        let desc = describe_tool(tool, &args);
        assert!(desc.starts_with(prefix), "{tool} lost its label: {desc}");
        assert!(
            desc.ends_with("..."),
            "{tool} lost the in-progress marker: {desc}"
        );
        assert!(
            !desc.ends_with("......"),
            "{tool} doubled the trailing ellipsis: {desc}"
        );
        assert_eq!(
            desc.matches('…').count(),
            1,
            "{tool} should carry exactly one cut marker: {desc}"
        );
    }
}

#[test]
fn describe_tool_run_bash_keeps_the_tail_of_a_long_command() {
    let long = "cd /Users/me/workspaces/example && curl -s https://example.com/api/v1/items";
    let desc = describe_tool("run_bash", &serde_json::json!({ "command": long }));
    assert!(
        desc.ends_with("/api/v1/items..."),
        "the informative tail must survive: {desc}"
    );
}

#[test]
fn describe_tool_condenses_a_multiline_command_to_its_first_line() {
    let script = "\n  git status\ncat <<'EOF' > out.txt\nbody\nEOF\n";
    let args = serde_json::json!({ "command": script });

    assert_eq!(describe_tool("run_bash", &args), "Running: git status...");
    assert_eq!(
        describe_tool("run_bash_background", &args),
        "Running in background: git status..."
    );
    assert_eq!(describe_cc_tool("Bash", &args), "Run git status");
    assert_eq!(
        describe_cc_tool("command_execution", &args),
        "Run git status"
    );
}

#[test]
fn test_describe_tool_trigger_groups_and_state() {
    assert_eq!(
        describe_tool("pause_trigger", &serde_json::json!({ "trigger_id": "t" })),
        "Pausing trigger..."
    );
    assert_eq!(
        describe_tool(
            "create_trigger_group",
            &serde_json::json!({ "name": "Morning" })
        ),
        "Creating trigger group 'Morning'..."
    );
    assert_eq!(
        describe_tool(
            "delete_trigger_group",
            &serde_json::json!({ "group_id": "g" })
        ),
        "Deleting trigger group..."
    );
}

#[test]
fn test_describe_grouped_tools_by_action() {
    assert_eq!(
        describe_tool(
            "triggers",
            &serde_json::json!({ "action": "create", "name": "Daily" })
        ),
        "Creating trigger 'Daily'..."
    );
    assert_eq!(
        describe_tool("triggers", &serde_json::json!({ "action": "pause" })),
        "Pausing trigger..."
    );
    assert_eq!(
        describe_tool("triggers", &serde_json::json!({ "action": "list" })),
        "Listing triggers..."
    );
    assert_eq!(
        describe_tool(
            "trigger_groups",
            &serde_json::json!({ "action": "create", "name": "Morning" })
        ),
        "Creating trigger group 'Morning'..."
    );
    assert_eq!(
        describe_tool(
            "preferences",
            &serde_json::json!({ "action": "set", "key": crate::core::prefs::THEME_MODE.key() })
        ),
        "Updating theme-mode setting..."
    );
    assert_eq!(
        describe_tool("preferences", &serde_json::json!({ "action": "get" })),
        "Reading preferences..."
    );
}

#[test]
fn test_describe_tool_count_events() {
    assert_eq!(
        describe_tool(
            "count_events",
            &serde_json::json!({ "event_type": "OuraSleepImported" })
        ),
        "Counting OuraSleepImported events..."
    );
    assert_eq!(
        describe_tool("count_events", &serde_json::json!({})),
        "Counting events..."
    );
}

#[test]
fn describe_tool_labels_each_read_decision() {
    assert_eq!(
        describe_tool("request_read", &serde_json::json!({ "read": false })),
        "No read needed"
    );
    assert_eq!(
        describe_tool("request_read", &serde_json::json!({ "read": true })),
        "Asking you to read this reply..."
    );
}

#[test]
fn test_describe_tool_threads_and_changes() {
    assert_eq!(
        describe_tool("list_threads", &serde_json::json!({})),
        "Listing threads..."
    );
    assert_eq!(
        describe_tool("count_threads", &serde_json::json!({})),
        "Counting threads..."
    );
    assert_eq!(
        describe_tool("list_changes", &serde_json::json!({})),
        "Listing changes..."
    );
    assert_eq!(
        describe_tool("apply_change", &serde_json::json!({ "change_id": "c" })),
        "Applying change..."
    );
    assert_eq!(
        describe_tool(
            "save_thread_image",
            &serde_json::json!({ "image": "thread:1", "path": "photos/a.jpg" })
        ),
        "Saving image to photos/a.jpg..."
    );
    assert_eq!(
        describe_tool("run_coding_agent", &serde_json::json!({})),
        "Executing Claude Code..."
    );
    assert_eq!(
        describe_tool(
            "run_coding_agent",
            &serde_json::json!({ "coding_agent": "codex" })
        ),
        "Executing Codex..."
    );
    assert_eq!(
        describe_tool(
            "run_claude",
            &serde_json::json!({ "coding_agent": "codex" })
        ),
        "Executing Codex..."
    );
}

#[test]
fn test_describe_tool_unknown_falls_back_to_generic() {
    assert_eq!(
        describe_tool("some_future_tool", &serde_json::json!({})),
        "Executing some_future_tool..."
    );
    assert!(tool_label("some_future_tool", &serde_json::json!({})).is_none());
}

fn tool_name_constants() -> Vec<String> {
    const SRC: &str = include_str!("../llm/tool_names.rs");
    let mut names = Vec::new();
    for line in SRC.lines() {
        let Some(decl) = line.trim().strip_prefix("pub const ") else {
            continue;
        };
        if !decl.contains(": &str = ") {
            continue;
        }
        let Some(open) = decl.find('"') else { continue };
        let value = &decl[open + 1..];
        let Some(close) = value.find('"') else {
            continue;
        };
        names.push(value[..close].to_string());
    }
    assert!(
        names.len() >= 100,
        "only {} tool-name constants parsed out of llm/tool_names.rs: the parser \
         in tool_name_constants no longer matches how they are declared",
        names.len()
    );
    names
}

#[test]
fn every_known_tool_name_has_a_step_label() {
    let mut names: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for tool in crate::llm::tools::get_default_tools(&crate::llm::ToolCapabilities::all_open()) {
        names.insert(tool.name);
    }
    for tool in crate::capability_manifest::llm_tools_for(&crate::llm::ToolCapabilities::all_open())
    {
        names.insert(tool.name);
    }
    for domain in crate::capability_manifest::domains() {
        names.extend(domain.alias_names().into_iter().map(String::from));
    }
    names.extend(tool_name_constants());

    let args = serde_json::json!({});
    let unlabelled: Vec<&str> = names
        .iter()
        .filter(|name| tool_label(name, &args).is_none())
        .map(String::as_str)
        .collect();
    assert!(
        unlabelled.is_empty(),
        "these tool names render the raw fallback \"Executing <name>...\" in the \
         steps UI: {}. Add a match arm in core::mod::tool_label for each.",
        unlabelled.join(", ")
    );
}

#[test]
fn the_previously_unlabelled_tools_render_a_real_label() {
    for name in [
        "run_trigger",
        "get_backup_status",
        "follow_up_child_thread",
        "correct_memory_by_id",
        "list_thread_queue",
        "update_thread_queue_policy",
        "view_image",
        "register_plugin_marketplace",
    ] {
        let label = describe_tool(name, &serde_json::json!({}));
        assert!(!label.is_empty(), "{name} rendered an empty label");
        assert!(
            !label.contains(name),
            "{name} still renders its raw tool name: {label}"
        );
    }
}

#[test]
fn describe_tool_labels_every_changes_action() {
    for (action, expected) in [
        ("list", "Listing changes..."),
        ("apply", "Applying change..."),
        ("apply_when_settled", "Arming a standing apply..."),
        ("apply_as_they_settle", "Applying, and arming the rest..."),
        ("cancel_standing_apply", "Canceling a standing apply..."),
        ("set_aside", "Setting a change aside..."),
        ("bring_back", "Bringing a change back..."),
    ] {
        assert_eq!(
            describe_tool("changes", &serde_json::json!({ "action": action })),
            expected,
            "grouped `changes` action '{action}'"
        );
        let alias = crate::capability_manifest::domain_for_tool("changes")
            .and_then(|d| d.legacy_tool_for_action(action))
            .unwrap_or_else(|| panic!("no llm_alias for changes action '{action}'"));
        assert_eq!(describe_tool(alias, &serde_json::json!({})), expected);
    }
}

#[test]
fn every_grouped_action_renders_its_own_step_label() {
    const SHARED_BY_DESIGN: &[(&str, &[&str])] = &[("memory", &["correct", "correct_by_id"])];

    let mut checked = 0usize;
    let mut collisions: Vec<String> = Vec::new();
    for domain in crate::capability_manifest::domains() {
        let mut by_label: std::collections::BTreeMap<String, Vec<&str>> =
            std::collections::BTreeMap::new();
        for action in domain.actions() {
            checked += 1;
            let label = describe_tool(domain.tool_name, &serde_json::json!({ "action": action }));
            by_label.entry(label).or_default().push(action);
        }
        for (label, mut actions) in by_label {
            actions.sort_unstable();
            if actions.len() < 2
                || SHARED_BY_DESIGN.iter().any(|(tool, shared)| {
                    *tool == domain.tool_name && shared.iter().eq(actions.iter())
                })
            {
                continue;
            }
            collisions.push(format!(
                "{} actions {:?} all render \"{}\"",
                domain.tool_name, actions, label
            ));
        }
    }
    assert!(
        checked >= 40,
        "only {checked} grouped actions examined: capability_manifest::domains() \
         no longer reports what this guard walks"
    );
    assert!(
        collisions.is_empty(),
        "these grouped actions share a step label, so at least one is falling to \
         the default arm: {}. Add a match arm in core::mod::tool_label for each.",
        collisions.join("; ")
    );
}

#[test]
fn describe_tool_labels_an_off_schedule_trigger_run() {
    assert_eq!(
        describe_tool("triggers", &serde_json::json!({ "action": "run" })),
        "Running trigger now..."
    );
    assert_eq!(
        describe_tool("run_trigger", &serde_json::json!({ "trigger_id": "t" })),
        "Running trigger now..."
    );
}

#[test]
fn describe_tool_follow_up_child_thread_shows_the_message_not_the_uuid() {
    let args = serde_json::json!({
        "thread_id": "11111111-2222-3333-4444-555555555555",
        "message": "Fix the failing test first"
    });
    let label = describe_tool("follow_up_child_thread", &args);
    assert_eq!(
        label,
        "Following up with child thread: Fix the failing test first..."
    );
    assert!(
        !label.contains("11111111"),
        "the child's uuid must never reach the label: {label}"
    );
    assert_eq!(
        describe_tool(
            "follow_up_child_thread",
            &serde_json::json!({ "thread_id": "x" })
        ),
        "Following up with child thread..."
    );
}

#[test]
fn describe_tool_labels_the_remaining_previously_missing_tools() {
    assert_eq!(
        describe_tool("get_backup_status", &serde_json::json!({})),
        "Checking backup status..."
    );
    assert_eq!(
        describe_tool("correct_memory_by_id", &serde_json::json!({ "id": "m" })),
        "Updating memory..."
    );
    assert_eq!(
        describe_tool("list_thread_queue", &serde_json::json!({})),
        "Listing Thread Queue..."
    );
    assert_eq!(
        describe_tool("update_thread_queue_policy", &serde_json::json!({})),
        "Updating Thread Queue policy..."
    );
    assert_eq!(
        describe_tool("view_image", &serde_json::json!({ "image": "thread:2" })),
        "Viewing thread:2..."
    );
    assert_eq!(
        describe_tool("view_image", &serde_json::json!({})),
        "Viewing image..."
    );
    assert_eq!(
        describe_tool(
            "register_plugin_marketplace",
            &serde_json::json!({ "source": "example-org/plugins" })
        ),
        "Registering marketplace example-org/plugins..."
    );
}

#[test]
fn describe_tool_built_from_redacted_args_masks_postgres_password() {
    for tool in ["run_bash", "run_bash_background"] {
        let mut args = serde_json::json!({
            "command": "psql postgres://lucidos:topsecret@localhost:5432/db -c 'select 1'"
        });
        redact_postgres_secrets_in_json(&mut args);
        let desc = describe_tool(tool, &args);
        assert!(
            !desc.contains("topsecret"),
            "{tool} leaked password: {desc}"
        );
        assert!(
            desc.contains("***"),
            "{tool} description not redacted: {desc}"
        );
    }
}

#[test]
fn describe_cc_tool_built_from_redacted_args_masks_postgres_password() {
    let leaky = "psql postgres://lucidos:topsecret@localhost:5432/db -c 'select 1'";
    for (tool, mut args) in [
        ("Bash", serde_json::json!({ "command": leaky })),
        (
            "command_execution",
            serde_json::json!({ "command": format!("/bin/zsh -lc \"{leaky}\"") }),
        ),
    ] {
        redact_postgres_secrets_in_json(&mut args);
        let desc = describe_cc_tool(tool, &args);
        assert!(
            !desc.contains("topsecret"),
            "{tool} leaked password: {desc}"
        );
        assert!(
            desc.contains("***"),
            "{tool} description not redacted: {desc}"
        );
    }
}

#[test]
fn test_describe_tool_result_read_file() {
    let content = "hello world";
    let result = describe_tool_result("read_file", content, true);
    assert_eq!(result, Some("11 chars".to_string()));
}

#[test]
fn test_describe_tool_result_list_files() {
    let result = describe_tool_result("list_files", "file1\nfile2\nfile3", true);
    assert_eq!(result, Some("3 items".to_string()));
}

#[test]
fn test_describe_tool_result_search_artifacts() {
    let result = describe_tool_result("search_artifacts", "match1\nmatch2", true);
    assert_eq!(result, Some("2 results".to_string()));
}

#[test]
fn test_describe_tool_result_failure() {
    let result = describe_tool_result(
        "read_file",
        "File not found: /foo/bar.txt\nsome stack trace",
        false,
    );
    assert_eq!(result, Some("File not found: /foo/bar.txt".to_string()));
}

#[test]
fn test_describe_tool_result_failure_truncates() {
    let long_err = "x".repeat(200);
    let result = describe_tool_result("any_tool", &long_err, false);
    let expected = format!("{}...", &"x".repeat(120));
    assert_eq!(result, Some(expected));
}

#[test]
fn a_retired_execute_intent_result_still_reads_done() {
    let result = describe_tool_result("execute_intent", "Calendar is clear.", true);
    assert_eq!(result, Some("Done".to_string()));
}

#[test]
fn test_describe_tool_result_run_python() {
    let result = describe_tool_result("run_python", "42\nsome debug output", true);
    assert_eq!(result, Some("42".to_string()));
}

#[test]
fn test_describe_tool_result_write_file() {
    assert_eq!(
        describe_tool_result("write_file", "OK", true),
        Some("Done".to_string())
    );
    assert_eq!(
        describe_tool_result("edit_file", "OK", true),
        Some("Done".to_string())
    );
    assert_eq!(
        describe_tool_result("create_app", "OK", true),
        Some("Done".to_string())
    );
}

#[test]
fn test_describe_tool_result_git_commit() {
    let result = describe_tool_result("git_commit", "[main abc123] feat: add feature", true);
    assert_eq!(result, Some("[main abc123] feat: add feature".to_string()));
}

#[test]
fn test_describe_tool_result_git_diff() {
    let diff = "--- a/file.rs\n+++ b/file.rs\n@@ -1,3 +1,4 @@\n+new line";
    let result = describe_tool_result("git_diff", diff, true);
    assert_eq!(result, Some("4 lines".to_string()));
}

#[test]
fn test_describe_tool_result_unknown_short() {
    let result = describe_tool_result("custom_tool", "short result", true);
    assert_eq!(result, Some("short result".to_string()));
}

#[test]
fn test_describe_tool_result_unknown_long() {
    let long_result = "x".repeat(100);
    let result = describe_tool_result("custom_tool", &long_result, true);
    assert_eq!(result, Some("100 chars".to_string()));
}

#[test]
fn test_truncate_within_limit() {
    assert_eq!(truncate("hello", 10), "hello");
}

#[test]
fn test_truncate_exceeds_limit() {
    assert_eq!(truncate("hello world", 5), "hello...");
}

#[test]
fn test_truncate_multibyte_char_boundary() {
    let norwegian = "For å bestille et Buypass ID med høyt sikkerhetsnivå (nivå 4), må du vanligvis oppfylle visse krav, inkludert å være 13 år eller eldre";
    let result = truncate(norwegian, 120);
    assert!(result.ends_with("..."));
    assert!(result.is_char_boundary(result.len()));
}

#[test]
fn test_truncate_multibyte_various() {
    assert_eq!(truncate("æøå", 2), "æ...");
    assert_eq!(truncate("æøå", 3), "æ...");
    assert_eq!(truncate("æøå", 1), "...");
}

#[test]
fn test_middle_truncate_within_limit() {
    assert_eq!(middle_truncate("hello", 10), "hello");
    assert_eq!(middle_truncate("", 10), "");
}

#[test]
fn test_middle_truncate_keeps_head_and_tail() {
    let s = "git log --oneline -30 crates/lucidos-app/src/hooks/useTooltip.ts";
    assert_eq!(s.len(), 64);
    let result = middle_truncate(s, 57);
    assert!(
        result.starts_with("git log --oneline -30 crate"),
        "got: {}",
        result
    );
    assert!(
        result.ends_with("app/src/hooks/useTooltip.ts"),
        "got: {}",
        result
    );
    assert!(result.contains('…'), "got: {}", result);
}

#[test]
fn test_middle_truncate_multibyte_safe() {
    let s = "For å bestille et Buypass ID med høyt sikkerhetsnivå (nivå 4), må du være 13 år eller eldre";
    let result = middle_truncate(s, 40);
    assert!(result.contains('…'), "got: {}", result);
    assert!(result.is_char_boundary(result.len()));
}

#[test]
fn test_middle_truncate_tiny_budget() {
    assert_eq!(middle_truncate("hello world", 2), "…");
    assert_eq!(middle_truncate("hello world", 3), "…");
}

#[test]
fn test_describe_tool_result_failure_with_norwegian() {
    let norwegian_error = "For å bestille et Buypass ID med høyt sikkerhetsnivå (nivå 4), må du vanligvis oppfylle visse krav, inkludert å være 13 år eller eldre og registrert i det norske folkeregisteret";
    let result = describe_tool_result("web_search", norwegian_error, false);
    assert!(result.is_some());
}

#[test]
fn test_describe_cc_tool_read() {
    let args = serde_json::json!({"file_path": "/home/user/src/main.rs"});
    assert_eq!(describe_cc_tool("Read", &args), "Read main.rs");
    assert_eq!(
        describe_cc_tool("Read", &serde_json::json!({})),
        "Read file"
    );
}

#[test]
fn test_describe_cc_tool_edit() {
    let args = serde_json::json!({"file_path": "/src/lib.rs"});
    assert_eq!(describe_cc_tool("Edit", &args), "Edit lib.rs");
    assert_eq!(describe_cc_tool("MultiEdit", &args), "Edit lib.rs");
}

#[test]
fn test_describe_cc_tool_write() {
    let args = serde_json::json!({"file_path": "/tmp/output.txt"});
    assert_eq!(describe_cc_tool("Write", &args), "Write output.txt");
}

#[test]
fn test_describe_cc_tool_glob() {
    let args = serde_json::json!({"pattern": "**/*.rs"});
    assert_eq!(describe_cc_tool("Glob", &args), "Find **/*.rs");
    assert_eq!(
        describe_cc_tool("Glob", &serde_json::json!({})),
        "Find files"
    );
}

#[test]
fn test_describe_cc_tool_grep() {
    let args = serde_json::json!({"pattern": "TODO"});
    assert_eq!(describe_cc_tool("Grep", &args), "Search 'TODO'");
}

#[test]
fn test_describe_cc_tool_bash() {
    let args = serde_json::json!({"command": "cargo test"});
    assert_eq!(describe_cc_tool("Bash", &args), "Run cargo test");
    let args2 = serde_json::json!({"command": "\n  git status\necho done"});
    assert_eq!(describe_cc_tool("Bash", &args2), "Run git status");
    assert_eq!(
        describe_cc_tool("Bash", &serde_json::json!({})),
        "Run command"
    );
}

#[test]
fn test_describe_cc_tool_bash_middle_truncates() {
    let cmd = "git log --oneline -30 crates/lucidos-app/src/hooks/useTooltip.ts";
    let args = serde_json::json!({"command": cmd});
    let result = describe_cc_tool("Bash", &args);
    assert!(
        result.starts_with("Run git log --oneline -30 "),
        "got: {}",
        result
    );
    assert!(result.ends_with("/hooks/useTooltip.ts"), "got: {}", result);
    assert!(result.contains('…'), "got: {}", result);
}

#[test]
fn test_describe_cc_tool_glob_middle_truncates() {
    let pat = "/Users/me/workspaces/dev/.lucidos/worktrees/thread-1523def0/crates/lucidos-app/src/hooks/useTooltip*.ts";
    let args = serde_json::json!({"pattern": pat});
    let result = describe_cc_tool("Glob", &args);
    assert!(result.starts_with("Find "), "got: {}", result);
    assert!(result.ends_with("/hooks/useTooltip*.ts"), "got: {}", result);
    assert!(result.contains('…'), "got: {}", result);
}

#[test]
fn test_describe_cc_tool_grep_middle_truncates() {
    let pat = "(some_very_long_function_name|another_very_long_function_name|yet_another_one)\\(";
    let args = serde_json::json!({"pattern": pat});
    let result = describe_cc_tool("Grep", &args);
    assert!(result.starts_with("Search '"), "got: {}", result);
    assert!(result.ends_with('\''), "got: {}", result);
    assert!(result.contains('…'), "got: {}", result);
}

#[test]
fn test_describe_cc_tool_web_search_middle_truncates() {
    let q = "what is the difference between rust floor_char_boundary and ceil_char_boundary in std";
    let args = serde_json::json!({"query": q});
    let result = describe_cc_tool("WebSearch", &args);
    assert!(result.starts_with("Search '"), "got: {}", result);
    assert!(result.ends_with('\''), "got: {}", result);
    assert!(result.contains('…'), "got: {}", result);
}

#[test]
fn test_describe_cc_tool_web_fetch() {
    let args = serde_json::json!({"url": "https://example.com/api/v1/data?q=1"});
    assert_eq!(
        describe_cc_tool("WebFetch", &args),
        "Fetch https://example.com"
    );
}

#[test]
fn test_describe_cc_tool_web_search() {
    let args = serde_json::json!({"query": "rust lifetimes"});
    assert_eq!(
        describe_cc_tool("WebSearch", &args),
        "Search 'rust lifetimes'"
    );
}

#[test]
fn test_describe_cc_tool_agent() {
    let args = serde_json::json!({"description": "Find all TODO comments"});
    assert_eq!(describe_cc_tool("Agent", &args), "Find all TODO comments");
    assert_eq!(
        describe_cc_tool("Agent", &serde_json::json!({})),
        "Run agent"
    );
}

#[test]
fn test_describe_cc_tool_skill() {
    let args = serde_json::json!({"skill": "commit"});
    assert_eq!(describe_cc_tool("Skill", &args), "Run skill: commit");
}

#[test]
fn test_describe_cc_tool_unknown() {
    assert_eq!(
        describe_cc_tool("CustomTool", &serde_json::json!({})),
        "CustomTool"
    );
}

#[test]
fn describe_cc_tool_names_an_mcp_tool_without_its_server_prefix() {
    let args = serde_json::json!({});
    assert_eq!(
        describe_cc_tool("mcp__example_server__create_issue", &args),
        "MCP: create_issue"
    );
    assert_eq!(describe_cc_tool("mcp__weird", &args), "MCP: mcp__weird");
    assert_eq!(
        describe_tool("mcp__example_server__create_issue", &args),
        "MCP: create_issue..."
    );
    assert_eq!(describe_tool("mcp__weird", &args), "MCP: mcp__weird...");
}

#[test]
fn describe_cc_tool_labels_both_backends_plan_tools_alike() {
    let args = serde_json::json!({});
    assert_eq!(describe_cc_tool("TodoWrite", &args), "Update plan");
    assert_eq!(describe_cc_tool("todo_list", &args), "Update plan");
    assert_eq!(
        describe_cc_tool("ExitPlanMode", &args),
        "Present plan for approval"
    );
}

#[test]
fn describe_cc_tool_reads_a_codex_shell_step_like_a_claude_code_one() {
    let bare = serde_json::json!({ "command": "git log --oneline -20" });
    for wrapped in [
        r#"/bin/zsh -lc "git log --oneline -20""#,
        r#"/bin/bash -lc 'git log --oneline -20'"#,
        "sh -c \"git log --oneline -20\"",
    ] {
        assert_eq!(
            describe_cc_tool(
                "command_execution",
                &serde_json::json!({ "command": wrapped })
            ),
            describe_cc_tool("Bash", &bare),
            "wrapper not seen through: {wrapped}"
        );
    }
}

#[test]
fn describe_cc_tool_spends_the_label_budget_on_the_command_not_the_wrapper() {
    let script = "rg -n 'withScrollAnchor' crates/lucidos-app/src/components/chat/scrollState.ts";
    let args = serde_json::json!({ "command": format!("/bin/zsh -lc \"{script}\"") });
    let desc = describe_cc_tool("command_execution", &args);

    assert!(desc.starts_with("Run rg -n "), "got: {desc}");
    assert!(desc.ends_with("chat/scrollState.ts"), "got: {desc}");
    assert!(!desc.contains("zsh"), "wrapper survived: {desc}");
}

#[test]
fn describe_cc_tool_condenses_a_wrapped_multiline_script_to_its_own_first_line() {
    let args = serde_json::json!({
        "command": "/bin/zsh -lc \"\n  git status\ncat <<'EOF' > out.txt\nbody\nEOF\n\"",
    });
    assert_eq!(
        describe_cc_tool("command_execution", &args),
        "Run git status"
    );
}

#[test]
fn describe_cc_tool_never_unwraps_a_claude_code_bash_call() {
    let args = serde_json::json!({ "command": r#"bash -lc "make lint""# });
    assert_eq!(
        describe_cc_tool("Bash", &args),
        r#"Run bash -lc "make lint""#
    );
}

#[test]
fn shell_script_body_declines_anything_it_cannot_read() {
    for untouched in [
        "cargo test --locked",
        "shellcheck -x scripts/e2e.sh",
        "/usr/local/bin/pushd -c 'x'",
        "zsh --version",
        "bash -x script.sh",
        r#"zsh -lc "a" && "b""#,
        r#"zsh -lc "git status"#,
        "zsh -lc",
        "zsh",
        "",
    ] {
        assert_eq!(
            shell_script_body(untouched),
            untouched,
            "should have passed through untouched"
        );
    }
}

#[test]
fn shell_script_body_unescapes_the_quoting_style_it_stripped() {
    assert_eq!(
        shell_script_body(r#"/bin/zsh -lc 'echo '\''hi'\'' > out'"#),
        "echo 'hi' > out"
    );
    assert_eq!(
        shell_script_body(r#"/bin/zsh -lc "rg -n \"pat\" src""#),
        r#"rg -n "pat" src"#
    );
    assert_eq!(
        shell_script_body(r#"/bin/zsh -lc "printf '%s\\n' x""#),
        r#"printf '%s\n' x"#
    );
    assert_eq!(
        shell_script_body(r#"zsh -lc "grep '\d+' f""#),
        r#"grep '\d+' f"#
    );
}

#[test]
fn shell_script_body_keeps_the_wrapper_on_a_multiword_unquoted_script() {
    assert_eq!(
        shell_script_body("zsh -lc git status"),
        "zsh -lc git status"
    );
    assert_eq!(
        describe_cc_tool(
            "command_execution",
            &serde_json::json!({ "command": "zsh -lc git status" })
        ),
        "Run zsh -lc git status"
    );
    assert_eq!(shell_script_body("zsh -lc ls"), "ls");
    assert_eq!(shell_script_body(r#"zsh -lc "git status"  "#), "git status");
}

#[test]
fn shell_script_body_on_the_two_real_codex_shapes() {
    assert_eq!(
        shell_script_body(
            r#"/bin/zsh -lc "rg -n \"detailsExpanded|stepsExpanded\" src -g '*.ts' | head -40; git status --short""#
        ),
        r#"rg -n "detailsExpanded|stepsExpanded" src -g '*.ts' | head -40; git status --short"#
    );

    let concatenated = r#"/bin/zsh -lc 'if [ -e "$LOCK" ]; then sed -n '"'1,20p' \""'$LOCK"; else echo MISSING; fi'"#;
    assert_eq!(shell_script_body(concatenated), concatenated);
}

#[test]
fn describe_cc_tool_names_the_file_a_codex_change_touches() {
    let change =
        |kind: serde_json::Value, path: &str| serde_json::json!({ "kind": kind, "path": path });
    let label = |changes: serde_json::Value| {
        describe_cc_tool("file_change", &serde_json::json!({ "changes": changes }))
    };

    assert_eq!(
        label(serde_json::json!([change(
            serde_json::json!({"type": "add"}),
            "/w/src/new.ts"
        )])),
        "Write new.ts"
    );
    assert_eq!(
        label(serde_json::json!([change(
            serde_json::json!({"type": "update", "move_path": null}),
            "/w/src/lib.rs"
        )])),
        "Edit lib.rs"
    );
    assert_eq!(
        label(serde_json::json!([change(
            serde_json::json!({"type": "delete"}),
            "/w/e2e/probe.spec.ts"
        )])),
        "Delete probe.spec.ts"
    );
    assert_eq!(
        label(serde_json::json!([change(
            serde_json::json!("update"),
            "/w/src/lib.rs"
        )])),
        "Edit lib.rs"
    );
    assert_eq!(
        describe_cc_tool("Edit", &serde_json::json!({"file_path": "/w/src/lib.rs"})),
        "Edit lib.rs"
    );
}

#[test]
fn describe_cc_tool_only_claims_one_verb_when_the_whole_patch_agrees() {
    let change =
        |kind: &str, path: &str| serde_json::json!({ "kind": {"type": kind}, "path": path });
    let label = |changes: serde_json::Value| {
        describe_cc_tool("file_change", &serde_json::json!({ "changes": changes }))
    };

    assert_eq!(
        label(serde_json::json!([
            change("update", "/w/a.ts"),
            change("update", "/w/b.ts"),
            change("update", "/w/c.ts"),
        ])),
        "Edit 3 files"
    );
    assert_eq!(
        label(serde_json::json!([
            change("add", "/w/a.ts"),
            change("delete", "/w/b.ts")
        ])),
        "Change 2 files"
    );
    assert_eq!(
        label(serde_json::json!([
            serde_json::json!({ "path": "/w/a.ts" })
        ])),
        "Change a.ts"
    );
    assert_eq!(
        label(serde_json::json!([
            serde_json::json!({ "kind": {"type": "add"} })
        ])),
        "Write 1 file"
    );
    assert_eq!(label(serde_json::json!([])), "Apply file changes");
    assert_eq!(
        describe_cc_tool("file_change", &serde_json::json!({})),
        "Apply file changes"
    );
}

#[test]
fn migrate_top_level_prompts_to_triggers() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().to_path_buf();
    let prompts_dir = dir.join("data/prompts");
    std::fs::create_dir_all(&prompts_dir).unwrap();
    std::fs::write(
        prompts_dir.join("sleep-reminder.md"),
        "---\nname: Sleep\n---\nGo to bed.",
    )
    .unwrap();
    std::fs::write(prompts_dir.join("notes.txt"), "not a markdown file").unwrap();

    migrate_prompts_to_intents(&dir);

    assert!(dir
        .join("data/triggers/sleep-reminder/sleep-reminder.md")
        .exists());
    assert!(prompts_dir.join("notes.txt").exists());
    assert!(prompts_dir.exists());
}

#[test]
fn migrate_app_prompts_to_intents() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().to_path_buf();
    let app_prompts = dir.join("data/apps/my-app/prompts");
    std::fs::create_dir_all(&app_prompts).unwrap();
    std::fs::write(
        app_prompts.join("workflow.md"),
        "---\nname: Workflow\n---\nDo things.",
    )
    .unwrap();

    migrate_prompts_to_intents(&dir);

    assert!(dir.join("data/apps/my-app/intents/workflow.md").exists());
    assert!(!app_prompts.exists());
}

#[test]
fn migrate_skips_if_already_done() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().to_path_buf();

    let app_intents = dir.join("data/apps/my-app/intents");
    let app_prompts = dir.join("data/apps/my-app/prompts");
    std::fs::create_dir_all(&app_intents).unwrap();
    std::fs::create_dir_all(&app_prompts).unwrap();
    std::fs::write(app_prompts.join("old.md"), "old").unwrap();
    std::fs::write(app_intents.join("new.md"), "new").unwrap();

    migrate_prompts_to_intents(&dir);

    assert!(app_prompts.exists());
    assert!(app_intents.join("new.md").exists());
}

#[test]
fn migrate_empty_prompts_dir_removed() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().to_path_buf();
    let prompts_dir = dir.join("data/prompts");
    std::fs::create_dir_all(&prompts_dir).unwrap();
    std::fs::write(
        prompts_dir.join("only.md"),
        "---\nname: Only\n---\nContent.",
    )
    .unwrap();

    migrate_prompts_to_intents(&dir);

    assert!(!prompts_dir.exists());
    assert!(dir.join("data/triggers/only/only.md").exists());
}

#[test]
fn ensure_workspace_gitignore_creates_file_with_all_entries() {
    let dir = tempfile::tempdir().unwrap();
    let changed = ensure_workspace_gitignore_entries(dir.path()).expect("ensure ok");
    assert!(changed, "fresh workspace must report changed=true");
    let content = std::fs::read_to_string(dir.path().join(".gitignore")).unwrap();
    for entry in WORKSPACE_GITIGNORE_ENTRIES {
        assert!(
            content.lines().any(|l| l.trim() == *entry),
            "missing entry {entry:?} in: {content}"
        );
    }
}

#[test]
fn ensure_workspace_gitignore_appends_missing_entry_to_legacy_file() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(".gitignore"), ".lucidos/\ndata/postgres/\n").unwrap();
    let changed = ensure_workspace_gitignore_entries(dir.path()).expect("ensure ok");
    assert!(
        changed,
        "legacy file missing data/blobs/ must report changed=true"
    );
    let content = std::fs::read_to_string(dir.path().join(".gitignore")).unwrap();
    assert!(content.starts_with(".lucidos/\ndata/postgres/\n"));
    assert!(content.lines().any(|l| l.trim() == "data/blobs/"));
}

#[test]
fn ensure_workspace_gitignore_noop_when_all_entries_present() {
    let dir = tempfile::tempdir().unwrap();
    let original =
        ".lucidos/\ndata/postgres/\ndata/blobs/\ndata/.env\ndata/plugin-media/\nmy-secret.env\n";
    std::fs::write(dir.path().join(".gitignore"), original).unwrap();
    let changed = ensure_workspace_gitignore_entries(dir.path()).expect("ensure ok");
    assert!(!changed, "all entries present must report changed=false");
    let after = std::fs::read_to_string(dir.path().join(".gitignore")).unwrap();
    assert_eq!(after, original, "file must not be rewritten when no-op");
}

#[test]
fn ensure_workspace_gitignore_inserts_missing_trailing_newline() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(".gitignore"), ".lucidos/\ndata/postgres/").unwrap();
    ensure_workspace_gitignore_entries(dir.path()).expect("ensure ok");
    let content = std::fs::read_to_string(dir.path().join(".gitignore")).unwrap();
    assert!(content.contains(".lucidos/\ndata/postgres/\ndata/blobs/\n"));
}

#[test]
fn ensure_state_dir_excluded_creates_the_directory_it_marks() {
    let dir = tempfile::tempdir().unwrap();
    assert!(!dir.path().join(".lucidos").exists());

    ensure_state_dir_excluded_from_file_backup(dir.path()).expect("ensure ok");

    assert!(dir.path().join(".lucidos").is_dir());
}

#[test]
#[cfg(target_os = "macos")]
fn ensure_state_dir_excluded_reports_the_change_once() {
    let dir = tempfile::tempdir().unwrap();

    assert!(
        ensure_state_dir_excluded_from_file_backup(dir.path()).expect("first boot"),
        "the first call sets the exclusion"
    );
    assert!(
        !ensure_state_dir_excluded_from_file_backup(dir.path()).expect("second boot"),
        "the re-check on the next boot must be silent"
    );
}

#[test]
#[cfg(not(target_os = "macos"))]
fn ensure_state_dir_excluded_is_a_quiet_no_op_off_macos() {
    let dir = tempfile::tempdir().unwrap();
    assert!(!ensure_state_dir_excluded_from_file_backup(dir.path()).expect("ensure ok"));
    assert!(dir.path().join(".lucidos").is_dir());
}

#[test]
fn pin_workspace_vite_port_writes_and_commits_when_missing() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    let mut opts = git2::RepositoryInitOptions::new();
    opts.initial_head("main");
    git2::Repository::init_opts(ws, &opts).unwrap();

    let written = pin_workspace_vite_port(ws, 5174).expect("pin ok");
    assert!(written, "missing file must report written=true");

    let content = std::fs::read_to_string(ws.join("lucidos.toml")).unwrap();
    assert_eq!(content, "[ports]\nvite = 5174\n");

    let repo = git2::Repository::open(ws).unwrap();
    let head = repo.head().unwrap().peel_to_commit().unwrap();
    assert_eq!(head.message().unwrap(), "chore: pin workspace vite port");
    assert_eq!(head.author().name().unwrap(), "Lucidos");
    assert_eq!(head.author().email().unwrap(), "lucidos@local");
    let tree = head.tree().unwrap();
    assert!(tree.get_name("lucidos.toml").is_some());
}

#[test]
fn pin_workspace_vite_port_rolls_back_file_on_commit_failure() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();

    let result = pin_workspace_vite_port(ws, 5174);
    assert!(
        result.is_err(),
        "expected error when commit phase fails (no .git)"
    );
    assert!(
        !ws.join("lucidos.toml").exists(),
        "file must be removed on commit failure so the working tree stays clean"
    );
}

#[test]
fn pin_workspace_vite_port_noop_when_file_exists() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    let mut opts = git2::RepositoryInitOptions::new();
    opts.initial_head("main");
    let repo = git2::Repository::init_opts(ws, &opts).unwrap();

    std::fs::write(ws.join("seed"), "x").unwrap();
    let mut idx = repo.index().unwrap();
    let base = reset_index_to_head(&repo, &mut idx).unwrap();
    idx.add_path(std::path::Path::new("seed")).unwrap();
    idx.write().unwrap();
    let seed_sha = commit_index(&repo, "seed", base).unwrap();

    let existing = "# my custom pin\n[ports]\nvite = 9999\n";
    std::fs::write(ws.join("lucidos.toml"), existing).unwrap();

    let written = pin_workspace_vite_port(ws, 5174).expect("pin ok");
    assert!(!written, "existing file must report written=false");

    let after = std::fs::read_to_string(ws.join("lucidos.toml")).unwrap();
    assert_eq!(after, existing, "existing file must not be touched");

    let head_sha = repo
        .head()
        .unwrap()
        .peel_to_commit()
        .unwrap()
        .id()
        .to_string();
    assert_eq!(
        head_sha, seed_sha,
        "no new commit should be made when file exists"
    );
}

fn index_locked_error() -> git2::Error {
    git2::Error::new(
        git2::ErrorCode::Locked,
        git2::ErrorClass::Index,
        "the index is locked",
    )
}

#[test]
fn transient_contention_covers_the_index_lock() {
    assert!(is_transient_repo_contention(&index_locked_error()));
}

#[test]
fn transient_contention_covers_the_lost_head_compare_and_swap() {
    let e = git2::Error::new(
        git2::ErrorCode::Modified,
        git2::ErrorClass::Reference,
        "old reference value does not match",
    );
    assert!(is_transient_repo_contention(&e));
}

#[test]
fn transient_contention_rejects_unrelated_errors() {
    for (code, class, msg) in [
        (
            git2::ErrorCode::NotFound,
            git2::ErrorClass::Index,
            "no such path",
        ),
        (
            git2::ErrorCode::NotFound,
            git2::ErrorClass::Reference,
            "no such reference",
        ),
        (
            git2::ErrorCode::Modified,
            git2::ErrorClass::Index,
            "the file was modified",
        ),
        (
            git2::ErrorCode::Conflict,
            git2::ErrorClass::Merge,
            "merge conflict",
        ),
    ] {
        let e = git2::Error::new(code, class, msg);
        assert!(
            !is_transient_repo_contention(&e),
            "{:?}/{:?} must not be treated as contention",
            class,
            code
        );
    }
}

#[test]
fn retry_while_repo_contended_waits_out_a_transient_lock() {
    let mut attempts = 0;
    let out = retry_while_repo_contended(|| {
        attempts += 1;
        if attempts < 3 {
            Err(index_locked_error())
        } else {
            Ok("committed")
        }
    });
    assert_eq!(out.unwrap(), "committed");
    assert_eq!(attempts, 3, "it must retry until the lock clears");
}

#[test]
fn retry_while_repo_contended_returns_any_other_error_at_once() {
    let mut attempts = 0;
    let out: Result<(), git2::Error> = retry_while_repo_contended(|| {
        attempts += 1;
        Err(git2::Error::new(
            git2::ErrorCode::NotFound,
            git2::ErrorClass::Index,
            "no such path",
        ))
    });
    assert_eq!(out.unwrap_err().code(), git2::ErrorCode::NotFound);
    assert_eq!(attempts, 1, "a non-contended error must not be retried");
}

#[test]
fn retry_while_repo_contended_gives_up_and_reports_the_lock() {
    let mut attempts = 0;
    let out: Result<(), git2::Error> = retry_while_repo_contended(|| {
        attempts += 1;
        Err(index_locked_error())
    });
    assert_eq!(out.unwrap_err().code(), git2::ErrorCode::Locked);
    assert!(attempts > 1, "it must have retried before giving up");
}

fn repo_with_seed_commit(ws: &std::path::Path) -> git2::Repository {
    let mut opts = git2::RepositoryInitOptions::new();
    opts.initial_head("main");
    let repo = git2::Repository::init_opts(ws, &opts).unwrap();
    std::fs::write(ws.join("seed.txt"), "seed").unwrap();
    let mut index = repo.index().unwrap();
    let base = reset_index_to_head(&repo, &mut index).unwrap();
    index.add_path(std::path::Path::new("seed.txt")).unwrap();
    index.write().unwrap();
    commit_index(&repo, "seed", base).unwrap();
    repo
}

fn stage_one(repo: &git2::Repository, name: &str) -> IndexBase {
    let ws = repo.workdir().unwrap().to_path_buf();
    std::fs::write(ws.join(name), name).unwrap();
    let mut index = repo.index().unwrap();
    let base = reset_index_to_head(repo, &mut index).unwrap();
    index.add_path(std::path::Path::new(name)).unwrap();
    index.write().unwrap();
    base
}

fn write_and_commit(repo: &git2::Repository, name: &str, message: &str) -> String {
    let base = stage_one(repo, name);
    commit_index(repo, message, base).unwrap()
}

fn commit_whole_tree(repo: &git2::Repository, message: &str) -> String {
    let mut index = repo.index().unwrap();
    let base = reset_index_to_head(repo, &mut index).unwrap();
    index
        .add_all(["."], git2::IndexAddOption::DEFAULT, None)
        .unwrap();
    index.write().unwrap();
    commit_index(repo, message, base).unwrap()
}

fn lose_head_swap(repo: &git2::Repository, mover: impl FnOnce()) -> git2::Error {
    let mut stale_head = repo.head().expect("head");
    let stale_target = stale_head.target().expect("a direct ref");
    mover();
    stale_head
        .set_target(stale_target, "lose the race")
        .map(|_| ())
        .expect_err("the swap must fail once HEAD has moved")
}

#[test]
fn a_lost_head_compare_and_swap_is_classified_as_contention() {
    let dir = tempfile::tempdir().unwrap();
    let repo = repo_with_seed_commit(dir.path());
    let other = git2::Repository::open(dir.path()).unwrap();
    let err = lose_head_swap(&repo, || {
        write_and_commit(&other, "theirs.txt", "theirs");
    });

    assert_eq!(err.class(), git2::ErrorClass::Reference);
    assert_eq!(err.code(), git2::ErrorCode::Modified);
    assert!(
        err.message().contains("old reference value does not match"),
        "the production symptom string changed: {}",
        err.message()
    );
    assert!(
        is_transient_repo_contention(&err),
        "the real libgit2 error must be retried"
    );
}

fn hold_main_ref_lock(ws: &std::path::Path) -> std::path::PathBuf {
    let lock = ws.join(".git/refs/heads/main.lock");
    std::fs::write(&lock, "").unwrap();
    lock
}

#[test]
fn a_held_branch_ref_lock_is_classified_as_contention() {
    let dir = tempfile::tempdir().unwrap();
    let repo = repo_with_seed_commit(dir.path());
    hold_main_ref_lock(dir.path());
    let base = stage_one(&repo, "ours.txt");

    let err = commit_index(&repo, "ours", base).expect_err("a held ref lock must block the commit");

    assert_eq!(err.class(), git2::ErrorClass::Os);
    assert_eq!(err.code(), git2::ErrorCode::Locked);
    assert!(
        err.message().contains("main.lock"),
        "the production symptom string changed: {}",
        err.message()
    );
    assert!(
        is_transient_repo_contention(&err),
        "the real libgit2 error must be retried"
    );
}

#[test]
fn a_write_waits_out_a_held_branch_ref_lock() {
    let dir = tempfile::tempdir().unwrap();
    let repo = repo_with_seed_commit(dir.path());
    let lock = hold_main_ref_lock(dir.path());

    let mut attempts = 0;
    let ours = retry_while_repo_contended(|| {
        attempts += 1;
        if attempts == 3 {
            std::fs::remove_file(&lock).unwrap();
        }
        let base = stage_one(&repo, "ours.txt");
        commit_index(&repo, "ours", base)
    })
    .expect("the write must land once the lock clears");

    assert_eq!(attempts, 3, "it must retry while the lock is held");
    let head = repo.head().unwrap().peel_to_commit().unwrap();
    assert_eq!(head.id().to_string(), ours);
    assert!(head.tree().unwrap().get_name("ours.txt").is_some());
}

#[test]
fn a_lost_head_race_retries_onto_the_new_head_and_keeps_both_commits() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    let repo = repo_with_seed_commit(ws);
    let other = git2::Repository::open(ws).unwrap();
    std::fs::write(ws.join("ours.txt"), "ours").unwrap();

    let mut attempts = 0;
    let ours = retry_while_repo_contended(|| {
        attempts += 1;
        let mut index = repo.index()?;
        let base = reset_index_to_head(&repo, &mut index)?;
        index.add_path(std::path::Path::new("ours.txt"))?;
        index.write()?;

        if attempts == 1 {
            return Err(lose_head_swap(&repo, || {
                write_and_commit(&other, "theirs.txt", "theirs");
            }));
        }
        commit_index(&repo, "ours", base)
    })
    .expect("the retry must commit onto the new head");

    assert_eq!(attempts, 2, "exactly one retry should have been needed");

    let head = repo.head().unwrap().peel_to_commit().unwrap();
    assert_eq!(head.id().to_string(), ours, "our commit must be HEAD");
    let messages: Vec<String> = repo
        .revwalk()
        .map(|mut walk| {
            walk.push_head().unwrap();
            walk.filter_map(|id| repo.find_commit(id.unwrap()).ok())
                .map(|c| c.message().unwrap().to_string())
                .collect()
        })
        .unwrap();
    assert_eq!(messages, vec!["ours", "theirs", "seed"]);
    assert_eq!(head.parent_count(), 1, "history must stay linear");

    let tree = head.tree().unwrap();
    assert!(tree.get_name("theirs.txt").is_some(), "theirs.txt was lost");
    assert!(tree.get_name("ours.txt").is_some(), "ours.txt was lost");
    assert!(tree.get_name("seed.txt").is_some(), "seed.txt was lost");
}

#[test]
fn a_deletion_the_race_winner_already_committed_settles_as_done() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    let repo = repo_with_seed_commit(ws);
    let other = git2::Repository::open(ws).unwrap();
    write_and_commit(&repo, "doomed.txt", "add doomed.txt");

    std::fs::remove_file(ws.join("doomed.txt")).unwrap();

    let mut attempts = 0;
    let reported = retry_while_repo_contended(|| {
        attempts += 1;
        let mut index = repo.index()?;
        let base = reset_index_to_head(&repo, &mut index)?;
        let _ = index.remove_path(std::path::Path::new("doomed.txt"));
        index.write()?;

        if attempts == 1 {
            return Err(lose_head_swap(&repo, || {
                commit_whole_tree(&other, "theirs: sweep the working tree");
            }));
        }
        commit_index_unless_unchanged(&repo, "delete doomed.txt", base)
    })
    .expect("an already-committed deletion must not fail the request");

    assert_eq!(attempts, 2, "exactly one retry should have been needed");

    let head = repo.head().unwrap().peel_to_commit().unwrap();
    assert_eq!(
        reported,
        head.id().to_string(),
        "it must report the head that records the deletion"
    );
    assert_eq!(
        head.message().unwrap(),
        "theirs: sweep the working tree",
        "no empty commit should have been stacked on the winner"
    );
    assert!(
        head.tree().unwrap().get_name("doomed.txt").is_none(),
        "the deletion must actually be recorded at the reported commit"
    );
}

#[test]
fn an_uncontended_deletion_still_makes_its_own_commit() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    let repo = repo_with_seed_commit(ws);
    let before = write_and_commit(&repo, "doomed.txt", "add doomed.txt");
    std::fs::remove_file(ws.join("doomed.txt")).unwrap();

    let mut index = repo.index().unwrap();
    let base = reset_index_to_head(&repo, &mut index).unwrap();
    let _ = index.remove_path(std::path::Path::new("doomed.txt"));
    index.write().unwrap();
    let sha = commit_index_unless_unchanged(&repo, "delete doomed.txt", base).unwrap();

    assert_ne!(sha, before, "a real deletion must produce a new commit");
    let head = repo.head().unwrap().peel_to_commit().unwrap();
    assert_eq!(head.message().unwrap(), "delete doomed.txt");
    assert!(head.tree().unwrap().get_name("doomed.txt").is_none());
    assert!(head.tree().unwrap().get_name("seed.txt").is_some());
}

#[test]
fn a_head_that_moved_after_the_reset_refuses_the_commit() {
    let dir = tempfile::tempdir().unwrap();
    let repo = repo_with_seed_commit(dir.path());
    let other = git2::Repository::open(dir.path()).unwrap();

    let base = stage_one(&repo, "ours.txt");
    let theirs = write_and_commit(&other, "theirs.txt", "theirs");

    let err = commit_index(&repo, "ours", base).expect_err("a moved HEAD must refuse the commit");
    assert_eq!(err.class(), git2::ErrorClass::Object);
    assert_eq!(err.code(), git2::ErrorCode::Modified);
    assert!(
        is_transient_repo_contention(&err),
        "the real libgit2 error must be retried: {}",
        err.message()
    );

    let head = repo.head().unwrap().peel_to_commit().unwrap();
    assert_eq!(
        head.id().to_string(),
        theirs,
        "HEAD must stay on the winner"
    );
}

#[test]
fn a_commit_between_reset_and_commit_is_kept_by_the_retry() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    let repo = repo_with_seed_commit(ws);
    let other = git2::Repository::open(ws).unwrap();
    std::fs::write(ws.join("ours.txt"), "ours").unwrap();

    let mut attempts = 0;
    let ours = retry_while_repo_contended(|| {
        attempts += 1;
        let mut index = repo.index()?;
        let base = reset_index_to_head(&repo, &mut index)?;
        index.add_path(std::path::Path::new("ours.txt"))?;
        index.write()?;
        if attempts == 1 {
            write_and_commit(&other, "theirs.txt", "theirs");
        }
        commit_index(&repo, "ours", base)
    })
    .expect("the retry must commit onto the new head");

    assert_eq!(attempts, 2, "the first attempt must be refused, not land");
    let head = repo.head().unwrap().peel_to_commit().unwrap();
    assert_eq!(head.id().to_string(), ours);
    assert_eq!(head.parent(0).unwrap().message().unwrap(), "theirs");
    let tree = head.tree().unwrap();
    assert!(
        tree.get_name("theirs.txt").is_some(),
        "theirs.txt was reverted"
    );
    assert!(tree.get_name("ours.txt").is_some(), "ours.txt was lost");
}

#[test]
fn a_deletion_landing_mid_full_tree_commit_stays_deleted() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    let repo = repo_with_seed_commit(ws);
    let other = git2::Repository::open(ws).unwrap();
    write_and_commit(&repo, "doomed.txt", "add doomed");
    std::fs::write(ws.join("dirty.txt"), "dirty").unwrap();

    let mut attempts = 0;
    retry_while_repo_contended(|| {
        attempts += 1;
        let mut index = repo.index()?;
        let base = reset_index_to_head(&repo, &mut index)?;
        index.add_all(["."], git2::IndexAddOption::DEFAULT, None)?;
        index.write()?;
        if attempts == 1 {
            std::fs::remove_file(ws.join("doomed.txt")).unwrap();
            let mut theirs = other.index().unwrap();
            let theirs_base = reset_index_to_head(&other, &mut theirs).unwrap();
            theirs
                .remove_path(std::path::Path::new("doomed.txt"))
                .unwrap();
            theirs.write().unwrap();
            commit_index(&other, "delete doomed", theirs_base).unwrap();
        }
        commit_index(&repo, "sweep the working tree", base)
    })
    .expect("the retry must commit onto the deletion");

    assert_eq!(attempts, 2, "the first attempt must be refused, not land");
    let head = repo.head().unwrap().peel_to_commit().unwrap();
    assert_eq!(head.parent(0).unwrap().message().unwrap(), "delete doomed");
    let tree = head.tree().unwrap();
    assert!(
        tree.get_name("doomed.txt").is_none(),
        "the deletion was reverted"
    );
    assert!(tree.get_name("dirty.txt").is_some(), "dirty.txt was lost");
}

#[tokio::test]
async fn the_database_handle_refuses_writes_until_a_transaction_opts_in() {
    use sqlx::Connection as _;
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    pool.close().await;
    let handle = read_only_database_url(&crate::test_support::test_db_url(&db_name))
        .expect("the test URL renders");
    let mut conn = sqlx::PgConnection::connect(&handle)
        .await
        .expect("connect through the handle");

    for ddl in [
        "CREATE TABLE handle_probe (id int)",
        "DROP SCHEMA public CASCADE",
    ] {
        let err = sqlx::raw_sql(ddl)
            .execute(&mut conn)
            .await
            .expect_err("a write through the handle must fail");
        assert!(
            err.to_string().contains("read-only transaction"),
            "{ddl}: {err}"
        );
    }
    let one: i32 = sqlx::query_scalar("SELECT 1")
        .fetch_one(&mut conn)
        .await
        .expect("a read works");
    assert_eq!(one, 1);
    sqlx::raw_sql("BEGIN READ WRITE; CREATE TABLE handle_probe (id int); COMMIT")
        .execute(&mut conn)
        .await
        .expect("an opted-in write works");

    conn.close().await.expect("close");
    crate::test_support::teardown_test_db(&db_name).await;
}

#[test]
fn an_artifacts_prefix_is_the_data_root_once() {
    assert_eq!(
        artifact_relative_path("artifacts/shots/a.png"),
        "shots/a.png"
    );
    assert_eq!(artifact_relative_path("shots/a.png"), "shots/a.png");
    assert_eq!(
        artifact_relative_path("artifacts/artifacts/a.png"),
        "artifacts/a.png"
    );
    assert_eq!(
        artifact_relative_path("artifactsx/a.png"),
        "artifactsx/a.png"
    );
}
