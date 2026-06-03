use super::*;

#[test]
fn cc_allowed_tools_returns_default_when_user_dir_missing() {
    assert_eq!(cc_allowed_tools(None), DEFAULT_CC_ALLOWED_TOOLS.join(","));
}

#[test]
fn cc_allowed_tools_seeds_empty_default_file_on_first_use() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cc-allowed-tools");
    assert!(!path.exists());

    let result = cc_allowed_tools(Some(dir.path()));

    assert_eq!(result, "", "default allowlist must be empty");
    assert!(path.exists(), "seed file should have been written");
    let seeded = std::fs::read_to_string(&path).unwrap();
    assert_eq!(seeded, CC_ALLOWED_TOOLS_HEADER);
}

#[test]
fn derive_allow_pattern_skill_narrow_uses_plugin_glob() {
    let input = serde_json::json!({ "skill": "code-review:code-review" });
    assert_eq!(
        derive_allow_pattern("Skill", &input, AllowScope::Narrow).as_deref(),
        Some("Skill(code-review:*)"),
    );
}

#[test]
fn derive_allow_pattern_skill_narrow_with_no_colon_uses_full_name() {
    let input = serde_json::json!({ "skill": "loop" });
    assert_eq!(
        derive_allow_pattern("Skill", &input, AllowScope::Narrow).as_deref(),
        Some("Skill(loop:*)"),
    );
}

#[test]
fn derive_allow_pattern_skill_broad_returns_bare_tool_name() {
    let input = serde_json::json!({ "skill": "code-review:code-review" });
    assert_eq!(
        derive_allow_pattern("Skill", &input, AllowScope::Broad).as_deref(),
        Some("Skill"),
    );
}

#[test]
fn derive_allow_pattern_bash_narrow_uses_first_token() {
    let input = serde_json::json!({ "command": "git status --short" });
    assert_eq!(
        derive_allow_pattern("Bash", &input, AllowScope::Narrow).as_deref(),
        Some("Bash(git:*)"),
    );
}

#[test]
fn derive_allow_pattern_bash_broad_returns_bare_tool_name() {
    let input = serde_json::json!({ "command": "ls" });
    assert_eq!(
        derive_allow_pattern("Bash", &input, AllowScope::Broad).as_deref(),
        Some("Bash"),
    );
}

#[test]
fn derive_allow_pattern_other_tool_narrow_returns_none() {
    let input = serde_json::json!({ "file_path": "/tmp/x" });
    assert_eq!(
        derive_allow_pattern("Read", &input, AllowScope::Narrow),
        None
    );
}

#[test]
fn derive_allow_pattern_other_tool_broad_returns_bare_name() {
    let input = serde_json::json!({});
    assert_eq!(
        derive_allow_pattern("Read", &input, AllowScope::Broad).as_deref(),
        Some("Read"),
    );
}

#[test]
fn derive_allow_pattern_skill_missing_input_returns_none() {
    let input = serde_json::json!({});
    assert_eq!(
        derive_allow_pattern("Skill", &input, AllowScope::Narrow),
        None
    );
}

/// CC's `--permission-mode acceptEdits` routes parametric file-path tools
/// (Edit/Write/NotebookEdit) through `--permission-prompt-tool` for any
/// out-of-cwd path **regardless** of bare entries in `--allowedTools`.
/// Persisting bare `Edit` (etc.) silently does nothing for the very paths
/// that surfaced the prompt — so the engine must refuse to write them.
/// In-cwd paths never reach this card (acceptEdits auto-approves), so no
/// legitimate caller is denied by this guard.
#[test]
fn derive_allow_pattern_broad_returns_none_for_acceptedits_routed_tools() {
    let input = serde_json::json!({"file_path": "/x", "old_string": "a", "new_string": "b"});
    assert_eq!(
        derive_allow_pattern("Edit", &input, AllowScope::Broad),
        None,
        "broad Edit must be suppressed — bare entry doesn't bypass acceptEdits routing"
    );
    assert_eq!(
        derive_allow_pattern("Write", &input, AllowScope::Broad),
        None,
        "broad Write must be suppressed for the same reason"
    );
    assert_eq!(
        derive_allow_pattern("NotebookEdit", &input, AllowScope::Broad),
        None,
        "broad NotebookEdit must be suppressed for the same reason"
    );
}

/// CC always routes `ExitPlanMode` through `--permission-prompt-tool`
/// regardless of `--allowedTools` — plan-mode exit is the user's plan
/// approval step, not a regular tool call. Persisting bare `ExitPlanMode`
/// to the allowlist would mislead the user into thinking the prompt would
/// stop coming back; suppress so the UI hides the broad button.
#[test]
fn derive_allow_pattern_broad_returns_none_for_exit_plan_mode() {
    let input = serde_json::json!({ "plan": "Step 1: do thing" });
    assert_eq!(
        derive_allow_pattern("ExitPlanMode", &input, AllowScope::Broad),
        None,
        "broad ExitPlanMode must be suppressed — CC always prompts for plan approval"
    );
}

/// Narrow scope for the same tools is still None (no narrow pattern is
/// generated for path-tools today). Documented here so a future addition
/// of `Edit(<glob>)` patterns is an intentional change, not a side-effect.
#[test]
fn derive_allow_pattern_narrow_remains_none_for_path_tools() {
    let input = serde_json::json!({"file_path": "/x"});
    assert_eq!(
        derive_allow_pattern("Edit", &input, AllowScope::Narrow),
        None
    );
    assert_eq!(
        derive_allow_pattern("Write", &input, AllowScope::Narrow),
        None
    );
    assert_eq!(
        derive_allow_pattern("NotebookEdit", &input, AllowScope::Narrow),
        None
    );
}

/// `Bash rm -rf .../.claude/...` triggers the permission card even with
/// bare `Bash` in `--allowedTools` — so persisting a Broad (`Bash`) or
/// Narrow (`Bash(rm:*)`) grant from that card lies about future
/// suppression. The engine refuses to derive a pattern; the UI hides the
/// misleading buttons. Session scope is unaffected because the engine
/// intercepts before CC's gate. See `CC_PROTECTED_PATH_MARKERS` for the
/// probing notes.
#[test]
fn input_touches_protected_path_detects_dot_claude_in_bash_command() {
    let input = serde_json::json!({
        "command": "rm -rf /Users/me/.claude/skills/grill"
    });
    assert!(input_touches_protected_path("Bash", &input));
}

#[test]
fn input_touches_protected_path_detects_dot_git_in_bash_command() {
    let input = serde_json::json!({"command": "cat .git/HEAD"});
    assert!(input_touches_protected_path("Bash", &input));
}

#[test]
fn input_touches_protected_path_returns_false_for_unrelated_bash_command() {
    let input = serde_json::json!({"command": "git status --short"});
    assert!(!input_touches_protected_path("Bash", &input));
}

#[test]
fn input_touches_protected_path_does_not_match_substrings_without_trailing_slash() {
    // `.claude` (no slash) and `.git` (no slash) appear in unrelated
    // identifiers — `.claude_backup`, file extension `.gitignore`, etc.
    // The trailing slash anchors to the actual protected directory.
    let input = serde_json::json!({"command": "cat .gitignore"});
    assert!(!input_touches_protected_path("Bash", &input));
    let input = serde_json::json!({"command": "ls .claude_backup"});
    assert!(!input_touches_protected_path("Bash", &input));
}

#[test]
fn input_touches_protected_path_returns_false_for_non_bash_tools() {
    // Read / Edit / Glob / Grep / NotebookEdit on `~/.claude/skills/...`
    // all auto-approved silently under bare allowlist entries (see
    // `CC_PROTECTED_PATH_MARKERS` doc) — so the filter is Bash-only.
    // Widen the match arm in `input_touches_protected_path` if CC
    // tightens later.
    let edit_input = serde_json::json!({
        "file_path": "/Users/me/repo/.claude/commands/harden.md",
        "old_string": "x",
        "new_string": "y"
    });
    assert!(!input_touches_protected_path("Edit", &edit_input));
    assert!(!input_touches_protected_path("Write", &edit_input));
    assert!(!input_touches_protected_path("Read", &edit_input));

    let nb_input = serde_json::json!({"notebook_path": "/repo/.git/x.ipynb"});
    assert!(!input_touches_protected_path("NotebookEdit", &nb_input));

    let glob_input = serde_json::json!({"pattern": ".claude/skills/**/*.md"});
    assert!(!input_touches_protected_path("Glob", &glob_input));

    let grep_input = serde_json::json!({"path": "/repo/.git/", "pattern": "HEAD"});
    assert!(!input_touches_protected_path("Grep", &grep_input));

    let skill_input = serde_json::json!({"skill": "code-review:code-review"});
    assert!(!input_touches_protected_path("Skill", &skill_input));
}

#[test]
fn derive_allow_pattern_broad_returns_none_when_bash_command_touches_dot_claude() {
    let input = serde_json::json!({
        "command": "rm -rf /Users/me/.claude/skills/grill"
    });
    assert_eq!(
        derive_allow_pattern("Bash", &input, AllowScope::Broad),
        None,
        "broad Bash must be suppressed for `.claude/` commands — CC routes them through the prompt regardless of --allowedTools"
    );
}

#[test]
fn derive_allow_pattern_narrow_returns_none_when_bash_command_touches_dot_claude() {
    let input = serde_json::json!({
        "command": "rm -rf /Users/me/.claude/skills/grill"
    });
    assert_eq!(
        derive_allow_pattern("Bash", &input, AllowScope::Narrow),
        None,
        "narrow Bash(rm:*) must be suppressed for `.claude/` commands — same reason as broad"
    );
}

#[test]
fn derive_allow_pattern_narrow_returns_none_when_bash_command_touches_dot_git() {
    let input = serde_json::json!({"command": "git push --force HEAD:.git/refs/heads/x"});
    assert_eq!(
        derive_allow_pattern("Bash", &input, AllowScope::Narrow),
        None,
    );
}

#[test]
fn derive_allow_pattern_session_unaffected_by_protected_path() {
    // Session scope uses the engine's pre-prompt intercept, which fires
    // before CC's gate — so it works even for `.claude/` paths. Documented
    // in `api/internal.rs::permission_prompt`.
    let input = serde_json::json!({
        "command": "rm -rf /Users/me/.claude/skills/grill"
    });
    assert_eq!(
        derive_allow_pattern("Bash", &input, AllowScope::Session).as_deref(),
        Some("Bash(rm:*)"),
        "session scope must remain Some — engine-side intercept bypasses CC's protected-path routing"
    );
}

#[test]
fn derive_allow_pattern_broad_unaffected_for_unrelated_bash_command() {
    // Regression guard: the protected-path filter must NOT suppress
    // legitimate Broad/Narrow grants for ordinary commands.
    let input = serde_json::json!({"command": "git status"});
    assert_eq!(
        derive_allow_pattern("Bash", &input, AllowScope::Broad).as_deref(),
        Some("Bash"),
    );
    assert_eq!(
        derive_allow_pattern("Bash", &input, AllowScope::Narrow).as_deref(),
        Some("Bash(git:*)"),
    );
}

#[test]
fn derive_allow_pattern_session_edit_uses_per_file_scope() {
    let input = serde_json::json!({
        "file_path": "/Users/me/repo/.claude/commands/harden.md",
        "old_string": "x",
        "new_string": "y",
    });
    assert_eq!(
        derive_allow_pattern("Edit", &input, AllowScope::Session).as_deref(),
        Some("Edit(/Users/me/repo/.claude/commands/harden.md)"),
    );
}

#[test]
fn derive_allow_pattern_session_write_uses_per_file_scope() {
    let input = serde_json::json!({
        "file_path": "/tmp/new.txt",
        "content": "hello",
    });
    assert_eq!(
        derive_allow_pattern("Write", &input, AllowScope::Session).as_deref(),
        Some("Write(/tmp/new.txt)"),
    );
}

#[test]
fn derive_allow_pattern_session_notebookedit_uses_notebook_path() {
    let input = serde_json::json!({
        "notebook_path": "/tmp/nb.ipynb",
        "new_source": "print(1)",
    });
    assert_eq!(
        derive_allow_pattern("NotebookEdit", &input, AllowScope::Session).as_deref(),
        Some("NotebookEdit(/tmp/nb.ipynb)"),
    );
}

#[test]
fn derive_allow_pattern_session_two_edits_to_same_file_match() {
    // Different `old_string`/`new_string` payloads must derive the same
    // session pattern so the second prompt auto-resolves against the first.
    let a = serde_json::json!({"file_path": "/x", "old_string": "a", "new_string": "b"});
    let b = serde_json::json!({"file_path": "/x", "old_string": "c", "new_string": "d"});
    assert_eq!(
        derive_allow_pattern("Edit", &a, AllowScope::Session),
        derive_allow_pattern("Edit", &b, AllowScope::Session),
    );
}

#[test]
fn derive_allow_pattern_session_edit_missing_path_returns_none() {
    let input = serde_json::json!({"old_string": "x"});
    assert_eq!(
        derive_allow_pattern("Edit", &input, AllowScope::Session),
        None
    );
}

#[test]
fn derive_allow_pattern_session_bash_reuses_narrow_pattern() {
    let input = serde_json::json!({"command": "git push origin main"});
    assert_eq!(
        derive_allow_pattern("Bash", &input, AllowScope::Session).as_deref(),
        Some("Bash(git:*)"),
    );
}

#[test]
fn derive_allow_pattern_session_skill_reuses_narrow_pattern() {
    let input = serde_json::json!({"skill": "superpowers:test-driven-development"});
    assert_eq!(
        derive_allow_pattern("Skill", &input, AllowScope::Session).as_deref(),
        Some("Skill(superpowers:*)"),
    );
}

/// Bare-tool fallback for tools without a narrow sub-scope. Session scope
/// is engine-side, so `BROAD_ALLOW_INEFFECTIVE` does not apply — the user
/// gets to remember any prompt for the rest of the thread.
#[test]
fn derive_allow_pattern_session_other_tool_falls_back_to_bare_name() {
    let input = serde_json::json!({"pattern": "foo"});
    assert_eq!(
        derive_allow_pattern("Read", &input, AllowScope::Session).as_deref(),
        Some("Read"),
    );
}

#[test]
fn append_allowed_tool_pattern_creates_file_with_header() {
    let dir = tempfile::tempdir().unwrap();
    append_allowed_tool_pattern(Some(dir.path()), "Skill(code-review:*)").unwrap();
    let body = std::fs::read_to_string(dir.path().join("cc-allowed-tools")).unwrap();
    assert!(body.starts_with(CC_ALLOWED_TOOLS_HEADER));
    assert!(body.trim_end().ends_with("Skill(code-review:*)"));
}

#[test]
fn append_allowed_tool_pattern_skips_duplicate() {
    let dir = tempfile::tempdir().unwrap();
    append_allowed_tool_pattern(Some(dir.path()), "Skill").unwrap();
    append_allowed_tool_pattern(Some(dir.path()), "Skill").unwrap();
    let body = std::fs::read_to_string(dir.path().join("cc-allowed-tools")).unwrap();
    assert_eq!(body.matches("Skill\n").count(), 1);
}

#[test]
fn append_allowed_tool_pattern_appends_to_existing_file() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("cc-allowed-tools"),
        "# header\nBash\nRead\n",
    )
    .unwrap();
    append_allowed_tool_pattern(Some(dir.path()), "Skill(code-review:*)").unwrap();
    let body = std::fs::read_to_string(dir.path().join("cc-allowed-tools")).unwrap();
    assert_eq!(body, "# header\nBash\nRead\nSkill(code-review:*)\n");
}

#[test]
fn append_allowed_tool_pattern_treats_existing_pattern_as_present_even_with_indent() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("cc-allowed-tools"),
        "# header\n  Skill(code-review:*)  \n",
    )
    .unwrap();
    append_allowed_tool_pattern(Some(dir.path()), "Skill(code-review:*)").unwrap();
    let body = std::fs::read_to_string(dir.path().join("cc-allowed-tools")).unwrap();
    assert_eq!(body, "# header\n  Skill(code-review:*)  \n");
}

#[test]
fn append_allowed_tool_pattern_no_op_when_user_dir_none() {
    // Should not panic and should not error.
    append_allowed_tool_pattern(None, "Bash").unwrap();
}

#[test]
fn read_allowed_tools_file_returns_header_when_missing() {
    let dir = tempfile::tempdir().unwrap();
    let body = read_allowed_tools_file(dir.path()).unwrap();
    assert_eq!(body, CC_ALLOWED_TOOLS_HEADER);
}

#[test]
fn write_then_read_allowed_tools_file_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let payload = "# notes\nBash\nSkill(meta:*)\n";
    write_allowed_tools_file(dir.path(), payload).unwrap();
    assert_eq!(read_allowed_tools_file(dir.path()).unwrap(), payload);
}

#[test]
fn default_cc_allowed_tools_is_empty() {
    assert_eq!(DEFAULT_CC_ALLOWED_TOOLS, &[] as &[&str]);
}

#[test]
fn cc_allowed_tools_parses_user_file_strips_comments_and_blanks() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("cc-allowed-tools"),
        "# header comment\n\nBash\nRead\n  Edit  \n# inline comment\nSkill(superpowers:*)\n\n",
    )
    .unwrap();

    assert_eq!(
        cc_allowed_tools(Some(dir.path())),
        "Bash,Read,Edit,Skill(superpowers:*)",
    );
}
