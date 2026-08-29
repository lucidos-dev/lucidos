use super::*;

#[test]
fn pg_env_vars_extracts_standard_url() {
    let vars = pg_env_vars("postgres://lucidos:lucidos@localhost:5432/lucidos");
    let map: std::collections::HashMap<_, _> = vars.into_iter().collect();
    assert_eq!(map.get("PGUSER").map(String::as_str), Some("lucidos"));
    assert_eq!(map.get("PGPASSWORD").map(String::as_str), Some("lucidos"));
    assert_eq!(map.get("PGHOST").map(String::as_str), Some("localhost"));
    assert_eq!(map.get("PGPORT").map(String::as_str), Some("5432"));
    assert_eq!(map.get("PGDATABASE").map(String::as_str), Some("lucidos"));
}

#[test]
fn pg_env_vars_accepts_postgresql_scheme() {
    let vars = pg_env_vars("postgresql://u:p@h:1234/db");
    let map: std::collections::HashMap<_, _> = vars.into_iter().collect();
    assert_eq!(map.get("PGUSER").map(String::as_str), Some("u"));
    assert_eq!(map.get("PGPASSWORD").map(String::as_str), Some("p"));
    assert_eq!(map.get("PGHOST").map(String::as_str), Some("h"));
    assert_eq!(map.get("PGPORT").map(String::as_str), Some("1234"));
    assert_eq!(map.get("PGDATABASE").map(String::as_str), Some("db"));
}

#[test]
fn pg_env_vars_accepts_passwordless_trust_auth_url() {
    // The gateway's embedded (packaged) Postgres backend hands the engine a
    // passwordless URL (trust auth on loopback). It must parse so a picker
    // restore's `pg_restore`/`psql` get PG* env; PGPASSWORD is simply omitted.
    let vars = pg_env_vars("postgres://lucidos@127.0.0.1:5599/lucidos");
    let map: std::collections::HashMap<_, _> = vars.into_iter().collect();
    assert_eq!(map.get("PGUSER").map(String::as_str), Some("lucidos"));
    assert_eq!(map.get("PGHOST").map(String::as_str), Some("127.0.0.1"));
    assert_eq!(map.get("PGPORT").map(String::as_str), Some("5599"));
    assert_eq!(map.get("PGDATABASE").map(String::as_str), Some("lucidos"));
    assert!(
        !map.contains_key("PGPASSWORD"),
        "a passwordless URL must not emit PGPASSWORD"
    );
}

#[test]
fn pg_env_vars_explicit_empty_password_still_emits_pgpassword() {
    // `user:@host` carried an (empty) password segment; preserve prior behavior
    // of emitting an empty PGPASSWORD so the no-segment case stays distinct.
    let vars = pg_env_vars("postgres://u:@host:5432/db");
    let map: std::collections::HashMap<_, _> = vars.into_iter().collect();
    assert_eq!(map.get("PGPASSWORD").map(String::as_str), Some(""));
}

#[test]
fn pg_env_vars_defaults_port_when_omitted() {
    let vars = pg_env_vars("postgres://u:p@host/db");
    let map: std::collections::HashMap<_, _> = vars.into_iter().collect();
    assert_eq!(map.get("PGPORT").map(String::as_str), Some("5432"));
    assert_eq!(map.get("PGHOST").map(String::as_str), Some("host"));
}

#[test]
fn pg_env_vars_decodes_percent_encoded_password() {
    // password "p@ss/word" url-encoded
    let vars = pg_env_vars("postgres://u:p%40ss%2Fword@host:5432/db");
    let map: std::collections::HashMap<_, _> = vars.into_iter().collect();
    assert_eq!(map.get("PGPASSWORD").map(String::as_str), Some("p@ss/word"));
}

#[test]
fn pg_env_vars_strips_query_string_from_dbname() {
    let vars = pg_env_vars("postgres://u:p@host:5432/db?sslmode=require&application_name=engine");
    let map: std::collections::HashMap<_, _> = vars.into_iter().collect();
    assert_eq!(map.get("PGDATABASE").map(String::as_str), Some("db"));
}

#[test]
fn pg_env_vars_returns_empty_when_url_unparseable() {
    assert!(pg_env_vars("not a url at all").is_empty());
    assert!(pg_env_vars("http://wrong/scheme").is_empty());
    assert!(pg_env_vars("postgres://no-at-sign/db").is_empty());
}

#[test]
fn pg_env_vars_cached_returns_same_shape_as_direct_call() {
    // The cached variant reads from the process env on first access; in the
    // test process DATABASE_URL may or may not be set. Either way, the
    // shape (count + key set, or empty) must agree with a fresh call
    // against the same URL.
    let direct = pg_env_vars(&database_url());
    let cached = pg_env_vars_cached();
    assert_eq!(direct.len(), cached.len());
    for (a, b) in direct.iter().zip(cached.iter()) {
        assert_eq!(a.0, b.0);
        assert_eq!(a.1, b.1);
    }
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
    // No password segment → nothing to mask.
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
    // A 1–3 char "secret" is too generic to scrub without nuking real text.
    let secrets = vec!["ab".to_string(), "x".to_string()];
    let text = "abc x ab xy";
    assert_eq!(redact_secret_values(text, &secrets), text);
}

#[test]
fn redact_secret_values_redacts_longest_first_no_partial_leftover() {
    // "secretvalue" contains "secret" — scrubbing the longer one first means no
    // dangling partial match is left behind.
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
    // Empty question string falls back to the short header chip.
    let args = serde_json::json!({
        "questions": [{ "question": "", "header": "Approach", "options": [] }]
    });
    assert_eq!(
        describe_tool("ask_user_question", &args),
        "Asking: Approach"
    );
}

/// The one step label that outlives its turn: the thread parks on it, so this
/// row is what the user reads for as long as the wait lasts. It has to say what
/// the agent is waiting FOR, in the agent's own words, not name the event types
/// (the engine's vocabulary) and not read as a generic "Executing …".
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

/// This label supplies the verb. `reason` is the model's free text, and it
/// opens with "waiting for" often enough that four guidance surfaces taught it.
///
/// The row that normally REPLACES this step strips the same phrase. A refused
/// `await_event` emits no `EventWaitStarted`, so nothing replaces the step, and
/// this label is then what the user is left reading.
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
    // Any run of whitespace, matching the TS twin's `\s+`. A literal
    // single-space phrase list stripped in the transcript and doubled here,
    // for the same reason, which is the hazard of having two implementations.
    assert_eq!(label("waiting  for  the lock"), "Waiting: the lock");
    // Only a LEADING phrase goes, and a reason that is nothing else is kept
    // whole rather than emptied into a dangling colon.
    assert_eq!(
        label("the lock another run is waiting for"),
        "Waiting: the lock another run is waiting for"
    );
    assert_eq!(label("waiting for"), "Waiting: waiting for");
    // The preposition must be a whole word.
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

/// Nearly every `describe_tool` arm ends in a trailing "..." as an in-progress
/// aesthetic. `truncate` appends its OWN "..." when it cuts, so an arm that
/// elides a value AND wraps it in a `"{}..."` format string emitted six dots
/// for any value long enough to truncate. The frontend accents only the
/// trailing three (`highlightEllipsis` + `.ellipsis-marker`), so the run
/// rendered as two separate markers: "Running: cd … && for ... ...".
/// Eliding arms middle-truncate instead, leaving exactly one "…" cut marker
/// and one trailing in-progress "...".
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

/// Middle-truncating a command keeps its tail, which is where the meaning
/// lives: the URL, the pipeline, the redirect. Head truncation spent the whole
/// budget on a `cd` prefix. Mirrors `describe_cc_tool`'s Bash arm.
#[test]
fn describe_tool_run_bash_keeps_the_tail_of_a_long_command() {
    let long = "cd /Users/me/workspaces/example && curl -s https://example.com/api/v1/items";
    let desc = describe_tool("run_bash", &serde_json::json!({ "command": long }));
    assert!(
        desc.ends_with("/api/v1/items..."),
        "the informative tail must survive: {desc}"
    );
}

/// A step label is one line of HTML, where a newline collapses to a space, so
/// a multi-line script condenses to its opening line instead of eliding across
/// newlines into a run-on. Both description paths share `first_command_line`.
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
    // The consolidated `triggers` / `trigger_groups` / `preferences` tools label
    // by the `action` discriminator (the flat-name arms above stay for aliases).
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
            &serde_json::json!({ "action": "set", "key": "theme" })
        ),
        "Updating theme setting..."
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
    // The fallback is reachable only for a name we do not ship: `tool_label`
    // says so, which is what the exhaustiveness guard below relies on.
    assert!(tool_label("some_future_tool", &serde_json::json!({})).is_none());
}

/// Every `pub const … : &str = "…";` value in `llm/tool_names.rs`, read out of
/// the file's own text (embedded by `include_str!`, parsed here at test time).
/// Reading the source beats naming the constants here: a new constant is
/// covered the moment it lands, with no second list to keep in sync.
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
    // A parser that silently stops matching (someone moves the constants behind
    // a macro, or reformats the declarations) would disarm the guard while the
    // test still passed. Hold a floor well under today's count so ordinary
    // additions and removals do not trip it.
    assert!(
        names.len() >= 100,
        "only {} tool-name constants parsed out of llm/tool_names.rs: the parser \
         in tool_name_constants no longer matches how they are declared",
        names.len()
    );
    names
}

/// Every tool name the engine knows must render a human-readable step label.
/// `describe_tool`'s fallback prints the raw identifier into the chat steps UI
/// ("Executing follow_up_child_thread..."), which leaks an internal name at the
/// one place the user is watching the agent work. `tool_label` answers `None`
/// for an unlabelled name, so this test can hold that line for every name we
/// register.
///
/// The four sources are unioned because a tool can enter the engine through any
/// of them: the flat default set, the manifest's grouped tools, a manifest
/// back-compat alias, or `tool_names.rs` on its own. That last source is the
/// belt-and-braces one: it fails as soon as the constant is declared, before the
/// tool is wired into any registry.
#[test]
fn every_known_tool_name_has_a_step_label() {
    let mut names: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for tool in crate::llm::tools::get_default_tools(&crate::llm::ToolCapabilities::all_open()) {
        names.insert(tool.name);
    }
    for tool in crate::capability_manifest::llm_tools() {
        names.insert(tool.name);
    }
    for domain in crate::capability_manifest::domains() {
        names.extend(domain.alias_names().into_iter().map(String::from));
    }
    names.extend(tool_name_constants());

    // Empty args on purpose: a label must survive a call whose optional
    // arguments are all absent, which is the shape the fallback used to swallow.
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

/// The cheap way to "fix" a missing label is to alias it back to the fallback
/// string, which changes nothing the user sees. Pin the eight names that were
/// hitting the fallback: each must render a real label that does not contain
/// its own snake_case identifier.
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

/// The grouped `triggers` tool dispatches on `action`, and an unrecognised
/// action falls to the list label. An off-schedule run is the opposite of a
/// list, so "Listing triggers..." for `action: "run"` was actively wrong, not
/// merely vague.
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
    // No message (a malformed call, or a replayed older payload) still reads as
    // a sentence rather than as a raw tool name.
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
    // The live emit path (agentic_loop) redacts args BEFORE building the
    // ToolCalled description, because the description renders in the steps UI
    // just like the args. This pins that composition: a hardcoded postgres
    // password must not survive into the bash-command preview.
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

/// The coding-agent twin of the test above. Codex reports a shell step as the
/// whole `/bin/zsh -lc "<script>"` invocation, and `shell_script_body` now puts
/// the script itself inside the 60-byte label budget, so a hardcoded password
/// that used to be pushed out by the wrapper is squarely inside it.
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

/// Redaction has to happen BEFORE the description is built, at BOTH tool-call
/// emit sites: a step row renders the description exactly as it renders the
/// args, so describing first leaves a hardcoded password in cleartext on
/// `description` while `args` is clean.
///
/// The two composition tests above prove the masking works on the way through.
/// This proves the call sites use it in the right ORDER, which is the half that
/// silently regressed: the coding-agent path described first and redacted after
/// until 2026-08-11, so every `psql postgres://user:pass@…` a coding agent ran
/// was persisted in cleartext on the event and broadcast over SSE.
#[test]
fn both_tool_call_emit_sites_redact_before_describing() {
    for (label, src, redact_call, describe_call) in [
        (
            "agent_session/run_session/run.rs",
            include_str!("../engine/agent_session/run_session/run.rs"),
            "redact_postgres_secrets_in_json(&mut input)",
            "describe_cc_tool(&name, &input)",
        ),
        (
            "agentic_loop/run.rs",
            include_str!("../engine/agentic_loop/run.rs"),
            "redact_postgres_secrets_in_json(&mut redacted_args)",
            "describe_tool(&tool_call.name, &redacted_args)",
        ),
        (
            "agentic_loop_special_tool.rs",
            include_str!("../engine/agentic_loop_special_tool.rs"),
            "redact_postgres_secrets_in_json(&mut redacted_args)",
            "describe_tool(&tc.name, &redacted_args)",
        ),
    ] {
        let find = |needle: &str| {
            src.find(needle).unwrap_or_else(|| {
                panic!("{label}: `{needle}` not found. If the call was renamed or moved, update this guard rather than deleting it.")
            })
        };
        assert!(
            find(redact_call) < find(describe_call),
            "{label}: the description is built from the UNREDACTED args, so a hardcoded postgres password lands in cleartext on the persisted `description`. Redact first."
        );
    }
}

#[test]
fn test_describe_tool_result_read_file() {
    let content = "hello world"; // 11 chars
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
    // Exact reproduction of the production panic:
    // byte index 120 is not a char boundary; it is inside 'æ' (bytes 119..121)
    let norwegian = "For å bestille et Buypass ID med høyt sikkerhetsnivå (nivå 4), må du vanligvis oppfylle visse krav, inkludert å være 13 år eller eldre";
    // This must not panic — truncate at 120 bytes falls inside 'æ' in 'være'
    let result = truncate(norwegian, 120);
    assert!(result.ends_with("..."));
    // Must not split a multi-byte char
    assert!(result.is_char_boundary(result.len()));
}

#[test]
fn test_truncate_multibyte_various() {
    // 'æ' is 2 bytes, 'ø' is 2 bytes, 'å' is 2 bytes
    assert_eq!(truncate("æøå", 2), "æ..."); // cut at byte 2 = after 'æ'
    assert_eq!(truncate("æøå", 3), "æ..."); // byte 3 is inside 'ø', back up to after 'æ'
    assert_eq!(truncate("æøå", 1), "..."); // byte 1 is inside 'æ', back up to start
}

#[test]
fn test_middle_truncate_within_limit() {
    assert_eq!(middle_truncate("hello", 10), "hello");
    assert_eq!(middle_truncate("", 10), "");
}

#[test]
fn test_middle_truncate_keeps_head_and_tail() {
    // 64 bytes; budget = 57 - 3("…") = 54 → head 27, tail 27.
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
    // Cuts must land on char boundaries; must not panic.
    let s = "For å bestille et Buypass ID med høyt sikkerhetsnivå (nivå 4), må du være 13 år eller eldre";
    let result = middle_truncate(s, 40);
    assert!(result.contains('…'), "got: {}", result);
    assert!(result.is_char_boundary(result.len()));
}

#[test]
fn test_middle_truncate_tiny_budget() {
    // Budget too small for both ends → just the ellipsis.
    assert_eq!(middle_truncate("hello world", 2), "…");
    assert_eq!(middle_truncate("hello world", 3), "…");
}

#[test]
fn test_describe_tool_result_failure_with_norwegian() {
    // The actual crash: web_search result with Norwegian text treated as failure
    let norwegian_error = "For å bestille et Buypass ID med høyt sikkerhetsnivå (nivå 4), må du vanligvis oppfylle visse krav, inkludert å være 13 år eller eldre og registrert i det norske folkeregisteret";
    // Must not panic
    let result = describe_tool_result("web_search", norwegian_error, false);
    assert!(result.is_some());
}

// --- describe_cc_tool tests ---

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
    // Multiline: picks first non-empty line
    let args2 = serde_json::json!({"command": "\n  git status\necho done"});
    assert_eq!(describe_cc_tool("Bash", &args2), "Run git status");
    assert_eq!(
        describe_cc_tool("Bash", &serde_json::json!({})),
        "Run command"
    );
}

#[test]
fn test_describe_cc_tool_bash_middle_truncates() {
    // Long commands are middle-truncated so the meaningful tail (filename
    // / target) is preserved alongside the leading verb.
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
    // Long absolute glob patterns shouldn't show the full prefix at the
    // expense of the meaningful basename suffix.
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

/// Both coding-agent backends name an MCP tool `mcp__<server>__<tool>`: Claude
/// Code natively, and Codex because `runtime/codex_parse.rs` rebuilds its
/// `mcp_tool_call` item into that exact shape. Neither had an arm, so the raw
/// identifier (server prefix and all) WAS the step label.
#[test]
fn describe_cc_tool_names_an_mcp_tool_without_its_server_prefix() {
    let args = serde_json::json!({});
    assert_eq!(
        describe_cc_tool("mcp__example_server__create_issue", &args),
        "MCP: create_issue"
    );
    // A malformed name (no `__` after the server) keeps the whole identifier
    // rather than losing it, same as the engine-side arm.
    assert_eq!(describe_cc_tool("mcp__weird", &args), "MCP: mcp__weird");
    assert_eq!(
        describe_tool("mcp__example_server__create_issue", &args),
        "MCP: create_issue..."
    );
    assert_eq!(describe_tool("mcp__weird", &args), "MCP: mcp__weird...");
}

/// The two backends' plan tools are the same thing to the user, so they get the
/// same label. `TodoWrite` had no arm and rendered as the bare tool name.
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

/// The whole point of `shell_script_body`: the two backends running the same
/// command produce the same row. Codex reports the invocation its harness built
/// (`/bin/zsh -lc "<script>"`), Claude Code reports the script.
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

/// The wrapper is stripped BEFORE truncation, which is the reason it matters at
/// all: `/bin/zsh -lc "` is 14 of the 60-byte budget, so leaving it on spends a
/// quarter of the row on the harness and elides the command.
#[test]
fn describe_cc_tool_spends_the_label_budget_on_the_command_not_the_wrapper() {
    let script = "rg -n 'withScrollAnchor' crates/lucidos-app/src/components/chat/scrollState.ts";
    let args = serde_json::json!({ "command": format!("/bin/zsh -lc \"{script}\"") });
    let desc = describe_cc_tool("command_execution", &args);

    assert!(desc.starts_with("Run rg -n "), "got: {desc}");
    assert!(desc.ends_with("chat/scrollState.ts"), "got: {desc}");
    assert!(!desc.contains("zsh"), "wrapper survived: {desc}");
}

/// Unwrapping runs before `first_command_line`, so a wrapped multi-line script
/// condenses to the first line of the SCRIPT rather than of the invocation.
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

/// A Claude Code `Bash` call keeps whatever it was given. When that model asks
/// for a login shell it chose to, and the row must not rewrite the request.
#[test]
fn describe_cc_tool_never_unwraps_a_claude_code_bash_call() {
    let args = serde_json::json!({ "command": r#"bash -lc "make lint""# });
    assert_eq!(
        describe_cc_tool("Bash", &args),
        r#"Run bash -lc "make lint""#
    );
}

/// Everything the unwrap must decline, because the label has to stay a faithful
/// rendering of what ran. Each case would silently lose or re-punctuate part of
/// the command if the helper guessed.
#[test]
fn shell_script_body_declines_anything_it_cannot_read() {
    for untouched in [
        // Not a shell.
        "cargo test --locked",
        // First word merely CONTAINS a shell name.
        "shellcheck -x scripts/e2e.sh",
        "/usr/local/bin/pushd -c 'x'",
        // No script-carrying flag.
        "zsh --version",
        "bash -x script.sh",
        // Two quoted words and a pipeline, not one quoted script: stripping the
        // outer quotes here would claim `a" && "b` was the command.
        r#"zsh -lc "a" && "b""#,
        // Unterminated.
        r#"zsh -lc "git status"#,
        // Nothing after the flag.
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

/// The escapes each quoting style actually uses. A single-quoted script cannot
/// contain a quote, so the wrapper spells one as close-escape-reopen, and that
/// sequence must not read as the terminator.
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
    // A backslash before anything else inside double quotes is literal, and the
    // character after it is read normally.
    assert_eq!(
        shell_script_body(r#"zsh -lc "grep '\d+' f""#),
        r#"grep '\d+' f"#
    );
}

/// POSIX `sh -c` reads ONE operand as the script and assigns the rest to `$0`,
/// `$1`, ..., so an unquoted suffix is only the script when it is a single word.
/// `zsh -lc git status` runs `git` with `$0=status`; labelling it `Run git
/// status` would put a command in the row that never ran. Found by the Codex
/// reviewer, 2026-08-11.
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
    // A single unquoted word IS the whole script, and reads as one.
    assert_eq!(shell_script_body("zsh -lc ls"), "ls");
    // Trailing whitespace around the script is the wrapper's, not the command's.
    assert_eq!(shell_script_body(r#"zsh -lc "git status"  "#), "git status");
}

/// Every shell the permission guard unwraps is one the step-row label unwraps
/// too. That direction is the load-bearing one: a payload the guard scanned and
/// the label did not would be displayed still wrapped, so the row would not show
/// the command the decision was made about.
///
/// The containment is deliberately one-way, and `WRAPPER_SHELLS`'s doc says what
/// the extra entry costs and what taking it the other way would require.
#[test]
fn every_shell_the_guard_scans_is_one_the_label_can_name() {
    for shell in crate::engine::command_guard::GUARD_SHELLS {
        assert!(
            WRAPPER_SHELLS.contains(&shell),
            "the guard unwraps `{shell}` but the label does not, so its payload is classified and then shown still wrapped"
        );
        let wrapped = format!("/bin/{shell} -lc 'ls -la'");
        assert_eq!(
            shell_script_body(&wrapped),
            "ls -la",
            "{shell}: the label does not see through this wrapper"
        );
    }
}

/// Where the two unwrappers deliberately disagree, pinned so neither is later
/// "fixed" into the other. POSIX `sh -c` runs `git` here and sets `$0=status`,
/// so the guard reading it as `git status` scans strictly more (always safe)
/// while the label doing so would name a command that never ran.
#[test]
fn the_label_declines_the_unquoted_operand_the_guard_over_reads() {
    let unquoted = "zsh -lc git status";
    assert_eq!(shell_script_body(unquoted), unquoted);
    assert_eq!(
        crate::engine::command_guard::unwrap_shell_command(unquoted),
        "git status"
    );
}

/// Two real Codex payload shapes, both kept because only one of them is a win
/// and the other has to stay a deliberate decline.
///
/// The double-quoted one is the common case and the reason any of this exists:
/// nested `\"` escapes resolve and the row finally shows the command.
///
/// The second uses shell quote CONCATENATION (`'a'"b"'c'` is one word), which
/// this deliberately does not parse. Reading it would take a real tokenizer, and
/// a wrong guess would silently re-punctuate a command someone is reading to
/// find out what ran, so it declines and shows the invocation verbatim, exactly
/// as it did before the unwrap existed.
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

/// Codex's `file_change` used to say only how many files it touched. Claude Code
/// names the file, and so must this: the two land on the same verb set.
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
    // Older exec frames send the kind as a bare string.
    assert_eq!(
        label(serde_json::json!([change(
            serde_json::json!("update"),
            "/w/src/lib.rs"
        )])),
        "Edit lib.rs"
    );
    // The Claude Code row for the same edit reads the same way.
    assert_eq!(
        describe_cc_tool("Edit", &serde_json::json!({"file_path": "/w/src/lib.rs"})),
        "Edit lib.rs"
    );
}

/// A set of changes states its count, and claims one verb only when every change
/// agrees. A patch that both creates and deletes is a "change", the same honesty
/// rule the permission card applies to the same payload.
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
    // An unreadable kind claims nothing beyond "something changed".
    assert_eq!(
        label(serde_json::json!([
            serde_json::json!({ "path": "/w/a.ts" })
        ])),
        "Change a.ts"
    );
    // No path to name: state the verb and the count rather than inventing one.
    assert_eq!(
        label(serde_json::json!([
            serde_json::json!({ "kind": {"type": "add"} })
        ])),
        "Write 1 file"
    );
    // Nothing announced at all.
    assert_eq!(label(serde_json::json!([])), "Apply file changes");
    assert_eq!(
        describe_cc_tool("file_change", &serde_json::json!({})),
        "Apply file changes"
    );
}

#[test]
fn migrate_top_level_prompts_to_triggers() {
    let dir = std::env::temp_dir().join("lucidos_test_migrate_prompts");
    let _ = std::fs::remove_dir_all(&dir);
    let prompts_dir = dir.join("data/prompts");
    std::fs::create_dir_all(&prompts_dir).unwrap();
    std::fs::write(
        prompts_dir.join("sleep-reminder.md"),
        "---\nname: Sleep\n---\nGo to bed.",
    )
    .unwrap();
    std::fs::write(prompts_dir.join("notes.txt"), "not a markdown file").unwrap();

    migrate_prompts_to_intents(&dir);

    // .md moved into triggers/{stem}/
    assert!(dir
        .join("data/triggers/sleep-reminder/sleep-reminder.md")
        .exists());
    // non-.md left behind
    assert!(prompts_dir.join("notes.txt").exists());
    // prompts dir still exists (not empty due to notes.txt)
    assert!(prompts_dir.exists());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn migrate_app_prompts_to_intents() {
    let dir = std::env::temp_dir().join("lucidos_test_migrate_app_prompts");
    let _ = std::fs::remove_dir_all(&dir);
    let app_prompts = dir.join("data/apps/my-app/prompts");
    std::fs::create_dir_all(&app_prompts).unwrap();
    std::fs::write(
        app_prompts.join("workflow.md"),
        "---\nname: Workflow\n---\nDo things.",
    )
    .unwrap();

    migrate_prompts_to_intents(&dir);

    // prompts/ renamed to intents/
    assert!(dir.join("data/apps/my-app/intents/workflow.md").exists());
    assert!(!app_prompts.exists());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn migrate_skips_if_already_done() {
    let dir = std::env::temp_dir().join("lucidos_test_migrate_idempotent");
    let _ = std::fs::remove_dir_all(&dir);

    // Already-migrated state: intents/ exists, prompts/ also exists with a file
    let app_intents = dir.join("data/apps/my-app/intents");
    let app_prompts = dir.join("data/apps/my-app/prompts");
    std::fs::create_dir_all(&app_intents).unwrap();
    std::fs::create_dir_all(&app_prompts).unwrap();
    std::fs::write(app_prompts.join("old.md"), "old").unwrap();
    std::fs::write(app_intents.join("new.md"), "new").unwrap();

    migrate_prompts_to_intents(&dir);

    // prompts/ NOT overwritten — intents/ already existed
    assert!(app_prompts.exists());
    assert!(app_intents.join("new.md").exists());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn migrate_empty_prompts_dir_removed() {
    let dir = std::env::temp_dir().join("lucidos_test_migrate_empty_prompts");
    let _ = std::fs::remove_dir_all(&dir);
    let prompts_dir = dir.join("data/prompts");
    std::fs::create_dir_all(&prompts_dir).unwrap();
    std::fs::write(
        prompts_dir.join("only.md"),
        "---\nname: Only\n---\nContent.",
    )
    .unwrap();

    migrate_prompts_to_intents(&dir);

    // File moved, dir should be empty and removed
    assert!(!prompts_dir.exists());
    assert!(dir.join("data/triggers/only/only.md").exists());

    let _ = std::fs::remove_dir_all(&dir);
}

/// On a fresh workspace (no .gitignore), the file is created with all
/// engine-managed entries — that's the first-boot path that has been
/// shipping for years. Helper preserves that exact behavior.
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

/// The legacy two-line `.gitignore` (`.lucidos/\ndata/postgres/\n`) is
/// what every existing user's workspace looks like today. Adding a new
/// engine-managed entry like `data/blobs/` must self-heal: append the
/// missing line, leave the existing ones alone (no churn in git).
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
    // Legacy lines preserved verbatim — line equality matters for git diff stability.
    assert!(content.starts_with(".lucidos/\ndata/postgres/\n"));
    assert!(content.lines().any(|l| l.trim() == "data/blobs/"));
}

/// When every engine-managed entry is already present, the helper is a
/// pure no-op: returns `false`, doesn't rewrite the file. Without this
/// invariant every engine startup would re-touch the file and re-commit
/// it to the artifacts repo.
#[test]
fn ensure_workspace_gitignore_noop_when_all_entries_present() {
    let dir = tempfile::tempdir().unwrap();
    // Trailing-line variation + manually added user entries — must
    // be preserved without mutation.
    let original = ".lucidos/\ndata/postgres/\ndata/blobs/\ndata/.env\nmy-secret.env\n";
    std::fs::write(dir.path().join(".gitignore"), original).unwrap();
    let changed = ensure_workspace_gitignore_entries(dir.path()).expect("ensure ok");
    assert!(!changed, "all entries present must report changed=false");
    let after = std::fs::read_to_string(dir.path().join(".gitignore")).unwrap();
    assert_eq!(after, original, "file must not be rewritten when no-op");
}

/// Edge case: file exists but has no trailing newline. Appending must
/// add one before the new entry so the result stays parseable.
#[test]
fn ensure_workspace_gitignore_inserts_missing_trailing_newline() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(".gitignore"), ".lucidos/\ndata/postgres/").unwrap();
    ensure_workspace_gitignore_entries(dir.path()).expect("ensure ok");
    let content = std::fs::read_to_string(dir.path().join(".gitignore")).unwrap();
    assert!(content.contains(".lucidos/\ndata/postgres/\ndata/blobs/\n"));
}

/// The engine covers the launch that skips the gateway, so the helper has to
/// create `.lucidos/` before it marks it. A workspace with no state dir would
/// otherwise log an exclusion failure on every boot (ADR 0153).
#[test]
fn ensure_state_dir_excluded_creates_the_directory_it_marks() {
    let dir = tempfile::tempdir().unwrap();
    assert!(!dir.path().join(".lucidos").exists());

    ensure_state_dir_excluded_from_file_backup(dir.path()).expect("ensure ok");

    assert!(dir.path().join(".lucidos").is_dir());
}

/// Every boot re-checks the exclusion, so only the first call may report a
/// change. Otherwise the startup log carries the same line for ever.
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

/// Off macOS there is no exclusion to set. The call still succeeds and still
/// creates the directory, so no caller needs a `cfg` around it.
#[test]
#[cfg(not(target_os = "macos"))]
fn ensure_state_dir_excluded_is_a_quiet_no_op_off_macos() {
    let dir = tempfile::tempdir().unwrap();
    assert!(!ensure_state_dir_excluded_from_file_backup(dir.path()).expect("ensure ok"));
    assert!(dir.path().join(".lucidos").is_dir());
}

/// Brand-new workspace: helper writes `[ports]\nvite = N\n`, stages it,
/// and commits via the engine's "Lucidos <lucidos@local>" identity. The
/// return value is `true` so callers can log a one-liner about the pin.
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
    // The commit only touches lucidos.toml.
    let tree = head.tree().unwrap();
    assert!(tree.get_name("lucidos.toml").is_some());
}

/// If the commit phase fails after the file is written (here simulated
/// by calling the helper against a workspace dir with no `.git/`), the
/// helper MUST roll the file back. Otherwise lucidos.toml ends up on
/// disk untracked AND the engine_impl gate (`workspace_was_uninitialized`,
/// one-shot per workspace) prevents any future retry — leaving the
/// workspace permanently dirty, which is the exact thing this whole
/// change is meant to prevent.
#[test]
fn pin_workspace_vite_port_rolls_back_file_on_commit_failure() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    // Intentionally do NOT init the repo — Repository::open inside the
    // helper's commit phase will fail, triggering the rollback path.

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

/// If `lucidos.toml` exists (hand-written pin or prior auto-write), the
/// helper must NOT touch it AND must NOT create a commit. Pin files are
/// user-editable; surprising them with a rewrite would clobber overrides.
/// Returns `false` so callers know nothing changed.
#[test]
fn pin_workspace_vite_port_noop_when_file_exists() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    let mut opts = git2::RepositoryInitOptions::new();
    opts.initial_head("main");
    let repo = git2::Repository::init_opts(ws, &opts).unwrap();

    // Seed an initial commit so we can verify no NEW commit was made.
    // Mirrors the realistic case where engine startup has already committed
    // .gitignore before the lucidos.toml helper runs.
    std::fs::write(ws.join("seed"), "x").unwrap();
    let mut idx = repo.index().unwrap();
    idx.add_path(std::path::Path::new("seed")).unwrap();
    idx.write().unwrap();
    let seed_sha = commit_index(&repo, "seed").unwrap();

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

/// The HEAD compare-and-swap loss. Classified from the same helper as the index
/// lock so both contended shapes stay documented in one place.
#[test]
fn transient_contention_covers_the_lost_head_compare_and_swap() {
    let e = git2::Error::new(
        git2::ErrorCode::Modified,
        git2::ErrorClass::Reference,
        "old reference value does not match",
    );
    assert!(is_transient_repo_contention(&e));
}

/// Anything else (a missing path, a corrupt repo, a conflict) is the caller's
/// answer, not contention. Retrying it would delay a real failure by the whole
/// backoff budget on every write. The `Reference` / `NotFound` case matters
/// most: widening on CLASS alone would have swallowed it.
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

/// Retrying is only ever right for contention. Anything else is the caller's
/// answer and must come straight back.
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

/// A lock nobody ever releases (a crashed writer's stale `index.lock`) must
/// surface as the error it is rather than hang the caller forever.
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

/// Init a repo with one commit and return the handle. Shared by the HEAD-race
/// tests below.
fn repo_with_seed_commit(ws: &std::path::Path) -> git2::Repository {
    let mut opts = git2::RepositoryInitOptions::new();
    opts.initial_head("main");
    let repo = git2::Repository::init_opts(ws, &opts).unwrap();
    std::fs::write(ws.join("seed.txt"), "seed").unwrap();
    let mut index = repo.index().unwrap();
    index.add_path(std::path::Path::new("seed.txt")).unwrap();
    index.write().unwrap();
    commit_index(&repo, "seed").unwrap();
    repo
}

/// Write one file and stage it onto the current head.
fn stage_one(repo: &git2::Repository, name: &str) {
    let ws = repo.workdir().unwrap().to_path_buf();
    std::fs::write(ws.join(name), name).unwrap();
    let mut index = repo.index().unwrap();
    reset_index_to_head(repo, &mut index).unwrap();
    index.add_path(std::path::Path::new(name)).unwrap();
    index.write().unwrap();
}

/// Stage one file through `repo` and commit it, the way a competing writer
/// (another `Repository` handle, a `git` CLI process) lands on HEAD.
fn write_and_commit(repo: &git2::Repository, name: &str, message: &str) -> String {
    stage_one(repo, name);
    commit_index(repo, message).unwrap()
}

/// A competing FULL-TREE commit, the shape `ArtifactManager::commit_all_dirty`
/// has: staging everything picks up a deletion another writer was in the middle
/// of committing, which is what makes that writer's retry find nothing to stage.
fn commit_whole_tree(repo: &git2::Repository, message: &str) -> String {
    let mut index = repo.index().unwrap();
    reset_index_to_head(repo, &mut index).unwrap();
    index
        .add_all(["."], git2::IndexAddOption::DEFAULT, None)
        .unwrap();
    index.write().unwrap();
    commit_index(repo, message).unwrap()
}

/// Lose the HEAD compare-and-swap for real. Hold the reference HEAD resolved to
/// BEFORE `mover` runs, then try to write through it: that is exactly the swap
/// `repo.commit(Some("HEAD"), ...)` performs at the end of `commit_index`, with
/// the window widened enough for a test to drive it. Returns the genuine
/// libgit2 error, never a synthesized one.
fn lose_head_swap(repo: &git2::Repository, mover: impl FnOnce()) -> git2::Error {
    let mut stale_head = repo.head().expect("head");
    let stale_target = stale_head.target().expect("a direct ref");
    mover();
    stale_head
        .set_target(stale_target, "lose the race")
        .map(|_| ())
        .expect_err("the swap must fail once HEAD has moved")
}

/// Pins the classifier against what libgit2 ACTUALLY produces when a HEAD
/// compare-and-swap is lost, rather than against a hand-built error. If a git2
/// upgrade ever reclassified this, the fix would silently stop working and only
/// this test would notice.
#[test]
fn a_lost_head_compare_and_swap_is_classified_as_contention() {
    let dir = tempfile::tempdir().unwrap();
    let repo = repo_with_seed_commit(dir.path());
    // A competing writer moves HEAD out from under the ref we are holding.
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

/// The branch ref's lock file, held by whichever writer is moving `main` right
/// now. Writing it by hand is exactly what a competing `git` process or a second
/// libgit2 handle does for the microseconds its own ref update takes.
fn hold_main_ref_lock(ws: &std::path::Path) -> std::path::PathBuf {
    let lock = ws.join(".git/refs/heads/main.lock");
    std::fs::write(&lock, "").unwrap();
    lock
}

/// Pins the classifier against what libgit2 ACTUALLY produces when another
/// writer holds `refs/heads/main.lock`, the sibling of the HEAD-swap test above.
///
/// The regression this guards: the classifier keyed on `(class, code)` pairs and
/// knew only `Index` / `Locked`. libgit2 reports a held REF lock as `Os` /
/// `Locked`, so the retry helper read it as a real failure and returned it at
/// once. A trigger writing an artifact through `PUT /api/v1/data/artifacts/...`
/// got a 500 out of a lock that would have cleared in one backoff.
#[test]
fn a_held_branch_ref_lock_is_classified_as_contention() {
    let dir = tempfile::tempdir().unwrap();
    let repo = repo_with_seed_commit(dir.path());
    hold_main_ref_lock(dir.path());
    stage_one(&repo, "ours.txt");

    let err = commit_index(&repo, "ours").expect_err("a held ref lock must block the commit");

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

/// End to end: the write waits the ref lock out and lands, instead of failing
/// the request. The holder releases on the third attempt, so two genuine
/// libgit2 `Os` / `Locked` failures are retried.
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
        stage_one(&repo, "ours.txt");
        commit_index(&repo, "ours")
    })
    .expect("the write must land once the lock clears");

    assert_eq!(attempts, 3, "it must retry while the lock is held");
    let head = repo.head().unwrap().peel_to_commit().unwrap();
    assert_eq!(head.id().to_string(), ours);
    assert!(head.tree().unwrap().get_name("ours.txt").is_some());
}

/// The race itself, end to end: a competing commit lands on HEAD mid-attempt,
/// the attempt loses the compare-and-swap, and the retry must commit ONTO the
/// winner rather than beside it or over it. Both commits survive and the
/// winner's file is still in the final tree.
///
/// The interleave is driven from the test body because the real window is
/// inside one libgit2 call, between `repo.head()` and `repo.commit()` in
/// `commit_index`. The error the first attempt returns is a genuine libgit2
/// `Reference` / `Modified` from a lost swap, not a synthesized one.
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
        reset_index_to_head(&repo, &mut index)?;
        index.add_path(std::path::Path::new("ours.txt"))?;
        index.write()?;

        if attempts == 1 {
            // A competing commit lands on HEAD after this attempt read its
            // parent, so this attempt's swap is against a head that moved.
            return Err(lose_head_swap(&repo, || {
                write_and_commit(&other, "theirs.txt", "theirs");
            }));
        }
        commit_index(&repo, "ours")
    })
    .expect("the retry must commit onto the new head");

    assert_eq!(attempts, 2, "exactly one retry should have been needed");

    // History: ours on top of theirs on top of seed, linear, nothing lost.
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

    // The competing writer's file survived, which is the part a naive retry
    // (committing the pre-race tree onto the new head) would have clobbered.
    let tree = head.tree().unwrap();
    assert!(tree.get_name("theirs.txt").is_some(), "theirs.txt was lost");
    assert!(tree.get_name("ours.txt").is_some(), "ours.txt was lost");
    assert!(tree.get_name("seed.txt").is_some(), "seed.txt was lost");
}

/// The delete helpers are the one shape the retry alone cannot rescue. They
/// remove from the working tree BEFORE the closure, so the writer that wins the
/// race can commit the very same deletion (`commit_all_dirty` stages all of
/// `data/`, picking our removal up as its own). The retry then resets onto a
/// head that no longer tracks the path and has nothing left to stage. That must
/// settle as done, not fail a request whose deletion demonstrably happened.
#[test]
fn a_deletion_the_race_winner_already_committed_settles_as_done() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    let repo = repo_with_seed_commit(ws);
    let other = git2::Repository::open(ws).unwrap();
    write_and_commit(&repo, "doomed.txt", "add doomed.txt");

    // The caller removes the file from the working tree, outside the retry.
    std::fs::remove_file(ws.join("doomed.txt")).unwrap();

    let mut attempts = 0;
    let reported = retry_while_repo_contended(|| {
        attempts += 1;
        let mut index = repo.index()?;
        reset_index_to_head(&repo, &mut index)?;
        let _ = index.remove_path(std::path::Path::new("doomed.txt"));
        index.write()?;

        if attempts == 1 {
            // The competitor sweeps the whole tree, committing OUR deletion,
            // and we lose the swap to it.
            return Err(lose_head_swap(&repo, || {
                commit_whole_tree(&other, "theirs: sweep the working tree");
            }));
        }
        commit_index_unless_unchanged(&repo, "delete doomed.txt")
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

/// The ordinary delete still commits. Guards against the tolerance above
/// swallowing a real deletion into a no-op that reports the unchanged head.
#[test]
fn an_uncontended_deletion_still_makes_its_own_commit() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    let repo = repo_with_seed_commit(ws);
    let before = write_and_commit(&repo, "doomed.txt", "add doomed.txt");
    std::fs::remove_file(ws.join("doomed.txt")).unwrap();

    let mut index = repo.index().unwrap();
    reset_index_to_head(&repo, &mut index).unwrap();
    let _ = index.remove_path(std::path::Path::new("doomed.txt"));
    index.write().unwrap();
    let sha = commit_index_unless_unchanged(&repo, "delete doomed.txt").unwrap();

    assert_ne!(sha, before, "a real deletion must produce a new commit");
    let head = repo.head().unwrap().peel_to_commit().unwrap();
    assert_eq!(head.message().unwrap(), "delete doomed.txt");
    assert!(head.tree().unwrap().get_name("doomed.txt").is_none());
    assert!(head.tree().unwrap().get_name("seed.txt").is_some());
}
