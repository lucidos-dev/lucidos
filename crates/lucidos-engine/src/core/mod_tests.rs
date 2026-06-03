use super::*;

/// Serializes the `load_workspace_env` tests: each mutates process-global env
/// (via `set_var` / the override load), so they must not interleave.
static ENV_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

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
    let pat = "/Users/kenneth/workspaces/dev/.lucidos/worktrees/thread-1523def0/crates/lucidos-app/src/hooks/useTooltip*.ts";
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

/// `load_workspace_env` reads `<workspace>/data/.env` and applies it with
/// override semantics: a value in the per-workspace file wins over the value
/// already present in the process env (which models the global `.env` +
/// inherited shell env). It returns the path it loaded.
#[test]
fn load_workspace_env_loads_and_overrides_when_present() {
    let _guard = ENV_TEST_LOCK.lock().unwrap();
    let key = "LUCIDOS_TEST_WS_ENV_OVERRIDE";
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("data")).unwrap();
    std::fs::write(dir.path().join("data/.env"), format!("{key}=from-workspace\n")).unwrap();

    // SAFETY: process-wide env mutation gated by ENV_TEST_LOCK.
    unsafe {
        std::env::set_var(key, "from-global");
    }

    let loaded = load_workspace_env(dir.path());
    assert_eq!(
        loaded.as_deref(),
        Some(dir.path().join("data/.env").as_path()),
        "returns the loaded path"
    );
    assert_eq!(
        std::env::var(key).ok().as_deref(),
        Some("from-workspace"),
        "per-workspace value must override the pre-existing process env value"
    );

    unsafe {
        std::env::remove_var(key);
    }
}

/// No `data/.env` → `load_workspace_env` is a no-op: returns `None` and leaves
/// the existing process env untouched.
#[test]
fn load_workspace_env_noop_when_absent() {
    let _guard = ENV_TEST_LOCK.lock().unwrap();
    let key = "LUCIDOS_TEST_WS_ENV_ABSENT";
    let dir = tempfile::tempdir().unwrap();
    // data/ exists but carries no .env.
    std::fs::create_dir_all(dir.path().join("data")).unwrap();

    // SAFETY: process-wide env mutation gated by ENV_TEST_LOCK.
    unsafe {
        std::env::set_var(key, "from-global");
    }

    let loaded = load_workspace_env(dir.path());
    assert!(loaded.is_none(), "absent .env must return None");
    assert_eq!(
        std::env::var(key).ok().as_deref(),
        Some("from-global"),
        "process env must be untouched when no per-workspace .env exists"
    );

    unsafe {
        std::env::remove_var(key);
    }
}
