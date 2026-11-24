fn label(command: &str) -> String {
    crate::core::describe_cc_tool(
        "command_execution",
        &serde_json::json!({ "command": command }),
    )
}

/// The label must unwrap every shell the permission guard unwraps.
/// Otherwise the row can hide the command the decision was made about.
#[test]
fn every_shell_the_guard_scans_is_one_the_label_can_name() {
    for shell in crate::engine::command_guard::GUARD_SHELLS {
        assert!(
            crate::core::WRAPPER_SHELLS.contains(&shell),
            "the guard unwraps `{shell}` but the label does not, so its payload is classified and then shown still wrapped"
        );
        let wrapped = format!("/bin/{shell} -lc 'ls -la'");
        assert_eq!(
            label(&wrapped),
            "Run ls -la",
            "{shell}: the label does not see through this wrapper"
        );
    }
}

/// POSIX `sh -c` runs `git` here and sets `$0=status`.
/// The guard may scan more text, but the label must name what actually ran.
#[test]
fn the_label_declines_the_unquoted_operand_the_guard_over_reads() {
    let unquoted = "zsh -lc git status";
    assert_eq!(label(unquoted), "Run zsh -lc git status");
    assert_eq!(
        crate::engine::command_guard::unwrap_shell_command(unquoted),
        "git status"
    );
}
