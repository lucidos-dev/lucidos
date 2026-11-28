use super::*;
use crate::engine::coding_command_grants::{
    command_grant_head, derive_allow_pattern, input_touches_protected_path,
};
use crate::engine::command_guard::TrustedHeads;
use crate::engine::thread_events::AllowScope;

#[test]
fn allowed_tools_flag_allows_only_engine_tools_with_no_grants() {
    assert_eq!(
        allowed_tools_flag(Vec::new()),
        ENGINE_ALLOWED_TOOLS.join(","),
        "a workspace with no grants allows only the engine's own tools"
    );
}

#[test]
fn engine_allowed_tools_keep_the_lines_the_engine_asks_for() {
    for line in [
        "Bash(lucidos request-read:*)",
        "Bash(lucidos expect-wake:*)",
    ] {
        assert!(ENGINE_ALLOWED_TOOLS.contains(&line), "{line}");
    }
}

fn lines(body: &str) -> Vec<String> {
    crate::core::grants::parse_patterns(body)
}

#[test]
fn derive_allow_pattern_codex_persists_claude_codes_bash_lines() {
    let cmd_input = serde_json::json!({ "command": "/bin/zsh -lc 'git push'", "cwd": "/wt" });
    let fc_input = serde_json::json!({ "reason": "writes outside worktree" });
    assert_eq!(
        derive_allow_pattern(
            "command_execution",
            &cmd_input,
            AllowScope::Broad,
            &TrustedHeads::NONE
        )
        .as_deref(),
        Some("Bash")
    );
    assert_eq!(
        derive_allow_pattern(
            "command_execution",
            &cmd_input,
            AllowScope::Narrow,
            &TrustedHeads::NONE
        )
        .as_deref(),
        Some("Bash(git:*)")
    );
    let hook = serde_json::json!({ "command": "/bin/zsh -lc 'echo x > .git/hooks/pre-commit'" });
    assert_eq!(
        derive_allow_pattern(
            "command_execution",
            &hook,
            AllowScope::Broad,
            &TrustedHeads::NONE
        )
        .as_deref(),
        Some("Bash")
    );
    for scope in [AllowScope::Narrow, AllowScope::Session] {
        assert_eq!(
            derive_allow_pattern("command_execution", &hook, scope, &TrustedHeads::NONE),
            None
        );
    }
    for scope in [AllowScope::Broad, AllowScope::Narrow] {
        assert_eq!(
            derive_allow_pattern("file_change", &fc_input, scope, &TrustedHeads::NONE),
            None,
            "{scope:?} must not persist a codex file-change pattern"
        );
    }
}

#[test]
fn a_compound_read_derives_the_head_the_fast_path_does_not_settle() {
    let command = "/bin/zsh -lc 'cat data/pid; ps -p 1; rg --files docs; \
                   gh run rerun 4 --repo example-org/example-repo'";
    let input = serde_json::json!({ "command": command, "cwd": "/wt" });
    assert_eq!(
        command_grant_head(&input, &TrustedHeads::NONE).as_deref(),
        Some("gh")
    );
    let narrow = derive_allow_pattern(
        "command_execution",
        &input,
        AllowScope::Narrow,
        &TrustedHeads::NONE,
    );
    assert_eq!(narrow.as_deref(), Some("Bash(gh:*)"));
    let stored = narrow.unwrap();
    assert!(crate::engine::command_guard::grant_covers_command(
        "Bash",
        command,
        crate::engine::command_guard::CommandSite::TextOnly { root: None },
        &crate::engine::command_guard::TrustedHeads::NONE,
        |p| p == stored
    ));
    let reads = serde_json::json!({ "command": "cat a; ls" });
    assert_eq!(
        command_grant_head(&reads, &TrustedHeads::NONE).as_deref(),
        Some("cat")
    );
    let wrapped = serde_json::json!({ "command": "cat a; timeout 5 curl https://example.com" });
    assert_eq!(command_grant_head(&wrapped, &TrustedHeads::NONE), None);
}

#[test]
fn derive_allow_pattern_codex_command_session_is_head_scoped() {
    let input = serde_json::json!({ "command": "git push origin main", "cwd": "/wt" });
    assert_eq!(
        derive_allow_pattern(
            "command_execution",
            &input,
            AllowScope::Session,
            &TrustedHeads::NONE
        )
        .as_deref(),
        Some("command_execution(git:*)"),
    );
    let rm = serde_json::json!({ "command": "rm -rf /tmp/x" });
    assert_eq!(
        derive_allow_pattern(
            "command_execution",
            &rm,
            AllowScope::Session,
            &TrustedHeads::NONE
        )
        .as_deref(),
        Some("command_execution(rm:*)"),
    );
}

#[test]
fn derive_allow_pattern_codex_command_session_reads_inside_the_wrapper() {
    let wrapped =
        |script: &str| serde_json::json!({ "command": format!("/bin/zsh -lc '{script}'") });
    assert_eq!(
        derive_allow_pattern(
            "command_execution",
            &wrapped("git status --short"),
            AllowScope::Session,
            &TrustedHeads::NONE
        )
        .as_deref(),
        Some("command_execution(git:*)"),
    );
    assert_eq!(
        derive_allow_pattern(
            "command_execution",
            &wrapped("rm -rf /tmp/x"),
            AllowScope::Session,
            &TrustedHeads::NONE
        )
        .as_deref(),
        Some("command_execution(rm:*)"),
    );
    assert_eq!(
        derive_allow_pattern(
            "Bash",
            &wrapped("cargo test"),
            AllowScope::Narrow,
            &TrustedHeads::NONE
        )
        .as_deref(),
        Some("Bash(cargo:*)"),
    );
}

#[test]
fn derive_allow_pattern_reads_through_a_nested_shell_and_never_stores_one() {
    let nested = serde_json::json!({ "command": "/bin/zsh -lc \"bash -c 'git push'\"" });
    let opaque = serde_json::json!({ "command": "/bin/zsh -lc 'bash x.sh'" });
    for tool in ["Bash", "command_execution"] {
        assert_eq!(
            derive_allow_pattern(tool, &nested, AllowScope::Narrow, &TrustedHeads::NONE).as_deref(),
            Some("Bash(git:*)"),
            "{tool}"
        );
        for scope in [AllowScope::Narrow, AllowScope::Session] {
            assert_eq!(
                derive_allow_pattern(tool, &opaque, scope, &TrustedHeads::NONE),
                None,
                "{tool} {scope:?}"
            );
        }
    }
}

#[test]
fn allowed_tools_flag_drops_a_line_that_names_a_shell_or_an_exec_wrapper() {
    let body = "Bash\nBash(git:*)\nBash(rm:*)\nBash(find:*)\nBash(python3:*)\nBash(bash:*)\nBash(zsh:*)\nBash(timeout:*)\nRead\n\
                 Bash(/bin/bash:*)\nBash(\\bash:*)\nBash(sudo rm:*)\nBash(eval:*)\nBash(env)\n";
    let result = allowed_tools_flag(lines(body));

    let mut want = ENGINE_ALLOWED_TOOLS.to_vec();
    want.push("Read");
    assert_eq!(result, want.join(","));
}

#[test]
fn allowed_tools_flag_drops_a_path_line() {
    let body = "Bash(./scripts/check.sh:*)\nBash(scripts/check.sh:*)\nBash(/tmp/x.sh:*)\n\
                 Bash(check.sh:*)\nBash(git:*)\n";
    let want = ENGINE_ALLOWED_TOOLS.to_vec();
    assert_eq!(allowed_tools_flag(lines(body)), want.join(","));
}

#[test]
fn derive_allow_pattern_command_with_no_head_has_no_session_pattern() {
    for command in ["", "   ", "2>/dev/null"] {
        let input = serde_json::json!({ "command": command });
        for tool in ["Bash", "command_execution"] {
            assert_eq!(
                derive_allow_pattern(tool, &input, AllowScope::Session, &TrustedHeads::NONE),
                None,
                "{tool} {command:?}"
            );
        }
    }
}

#[test]
fn derive_allow_pattern_codex_file_change_has_no_session_pattern() {
    let input = serde_json::json!({ "reason": "writes outside worktree", "grant_root": "/etc" });
    assert_eq!(
        derive_allow_pattern(
            "file_change",
            &input,
            AllowScope::Session,
            &TrustedHeads::NONE
        ),
        None,
    );
}

#[test]
fn derive_allow_pattern_skill_narrow_uses_plugin_glob() {
    let input = serde_json::json!({ "skill": "code-review:code-review" });
    assert_eq!(
        derive_allow_pattern("Skill", &input, AllowScope::Narrow, &TrustedHeads::NONE).as_deref(),
        Some("Skill(code-review:*)"),
    );
}

#[test]
fn derive_allow_pattern_skill_narrow_with_no_colon_uses_full_name() {
    let input = serde_json::json!({ "skill": "loop" });
    assert_eq!(
        derive_allow_pattern("Skill", &input, AllowScope::Narrow, &TrustedHeads::NONE).as_deref(),
        Some("Skill(loop:*)"),
    );
}

#[test]
fn derive_allow_pattern_skill_broad_returns_bare_tool_name() {
    let input = serde_json::json!({ "skill": "code-review:code-review" });
    assert_eq!(
        derive_allow_pattern("Skill", &input, AllowScope::Broad, &TrustedHeads::NONE).as_deref(),
        Some("Skill"),
    );
}

#[test]
fn derive_allow_pattern_bash_narrow_uses_the_command_head() {
    let input = serde_json::json!({ "command": "git status --short" });
    assert_eq!(
        derive_allow_pattern("Bash", &input, AllowScope::Narrow, &TrustedHeads::NONE).as_deref(),
        Some("Bash(git:*)"),
    );
}

#[test]
fn derive_allow_pattern_bash_broad_returns_bare_tool_name() {
    let input = serde_json::json!({ "command": "ls" });
    assert_eq!(
        derive_allow_pattern("Bash", &input, AllowScope::Broad, &TrustedHeads::NONE).as_deref(),
        Some("Bash"),
    );
}

#[test]
fn derive_allow_pattern_other_tool_narrow_returns_none() {
    let input = serde_json::json!({ "file_path": "/tmp/x" });
    assert_eq!(
        derive_allow_pattern("Read", &input, AllowScope::Narrow, &TrustedHeads::NONE),
        None
    );
}

#[test]
fn derive_allow_pattern_other_tool_broad_returns_bare_name() {
    let input = serde_json::json!({});
    assert_eq!(
        derive_allow_pattern("Read", &input, AllowScope::Broad, &TrustedHeads::NONE).as_deref(),
        Some("Read"),
    );
}

#[test]
fn derive_allow_pattern_skill_missing_input_returns_none() {
    let input = serde_json::json!({});
    assert_eq!(
        derive_allow_pattern("Skill", &input, AllowScope::Narrow, &TrustedHeads::NONE),
        None
    );
}

#[test]
fn derive_allow_pattern_broad_returns_none_for_acceptedits_routed_tools() {
    let input = serde_json::json!({"file_path": "/x", "old_string": "a", "new_string": "b"});
    assert_eq!(
        derive_allow_pattern("Edit", &input, AllowScope::Broad, &TrustedHeads::NONE),
        None,
        "broad Edit must be suppressed — bare entry doesn't bypass acceptEdits routing"
    );
    assert_eq!(
        derive_allow_pattern("Write", &input, AllowScope::Broad, &TrustedHeads::NONE),
        None,
        "broad Write must be suppressed for the same reason"
    );
    assert_eq!(
        derive_allow_pattern(
            "NotebookEdit",
            &input,
            AllowScope::Broad,
            &TrustedHeads::NONE
        ),
        None,
        "broad NotebookEdit must be suppressed for the same reason"
    );
}

#[test]
fn derive_allow_pattern_broad_returns_none_for_exit_plan_mode() {
    let input = serde_json::json!({ "plan": "Step 1: do thing" });
    assert_eq!(
        derive_allow_pattern(
            "ExitPlanMode",
            &input,
            AllowScope::Broad,
            &TrustedHeads::NONE
        ),
        None,
        "broad ExitPlanMode must be suppressed — CC always prompts for plan approval"
    );
}

#[test]
fn derive_allow_pattern_narrow_remains_none_for_path_tools() {
    let input = serde_json::json!({"file_path": "/x"});
    assert_eq!(
        derive_allow_pattern("Edit", &input, AllowScope::Narrow, &TrustedHeads::NONE),
        None
    );
    assert_eq!(
        derive_allow_pattern("Write", &input, AllowScope::Narrow, &TrustedHeads::NONE),
        None
    );
    assert_eq!(
        derive_allow_pattern(
            "NotebookEdit",
            &input,
            AllowScope::Narrow,
            &TrustedHeads::NONE
        ),
        None
    );
}

#[test]
fn input_touches_protected_path_detects_a_write_under_a_protected_dir() {
    for command in [
        "rm -rf /Users/me/.claude/skills/grill",
        "echo '{}' > .claude/settings.json",
        "cat a >> .git/config",
        "cp hook.sh .git/hooks/pre-commit",
        "mv x .claude/settings.local.json",
        "sed -i s/a/b/ .claude/rules/x.md",
        "tee .git/config < cfg",
        "mkdir -p .claude/hooks",
        "touch .git/hooks/post-merge",
        "git status && rm .git/index.lock",
        "find .claude/ -name '*.md' -delete",
        "find . -path '*/.git/*' -exec rm {} +",
        "sort -o .claude/x.md y",
        "sort -T .claude/ a",
        "sort -uo .claude/x a",
        "sort -k1 -T .claude/ a",
        "sort --output=.claude/x a",
        "sort --compress-program=sh .claude/x",
        "sort --unknown .claude/x",
        "git checkout .claude/settings.json",
        "git restore .claude/x",
        "git -c core.hooksPath=x commit -m m .claude/x",
        "git --git-dir=.claude/ add a",
        "GIT_INDEX_FILE=.claude/x git add a",
        "bash -c 'echo x > .claude/settings.json'",
        "python3 - <<'EOF'\nopen('.claude/settings.json', 'w').write('{}')\nEOF",
        "echo $'\\''\nrm -rf .claude/x\necho '\nx'",
        "echo \\$'a\\'\nrm -rf .claude/x\necho '\nx'",
        "cat <<EOF\n'\nEOF\nrm -rf .claude/x\necho '\nx'",
        "echo it\\'s; rm -rf .claude/x",
        "grep -rl foo .claude/rules | xargs sed -i 's/foo/bar/'",
        "find .claude/ -name '*.json' | xargs rm",
        "echo 'echo {} > .claude/settings.json' | sh",
        "N=.claude/settings.json; tee \"$N\" < cfg",
        "cat <<'EOF' | sh\necho '{}' > .claude/settings.local.json\nEOF",
        "python3 - <<EOF\nfile = open('.claude/settings.json', 'w')\nEOF",
        "cat > /tmp/f.sh <<'EOF'\necho x > .git/hooks/pre-commit\nEOF\nbash /tmp/f.sh",
        "sh -c 'cp \"$1\" \"$2\"' _ hook.sh .git/hooks/pre-commit",
        ">.claude/settings.json bash -c true",
        "X=\"$(cp cfg .claude/settings.json)\"; bash -c true",
        "echo hi #'\nrm -rf .claude/hooks\n# '",
        "mkdir -p \"/tmp/x; cat \"; cp cfg \"/tmp/x; cat /../..$PWD/.claude/settings.json\"; : '<<'",
        "ls() { rm -rf \"$@\"; }; ls .claude/hooks",
        "ls () ( rm -rf \"$@\" ); ls .claude/hooks",
        "psql -Atc 'select 1' | grep -o '\\.claude/[^ ]*'",
        "N=/tmp/n.md; cat > \"$N\" <<'EOF'\nSee .claude/rules/x.md.\nEOF\nlucidos successor start --narrative \"$N\"",
        "sed -i.bak s/a/b/ .claude/rules/x.md",
        "sed -i '' 's/a/b/' .claude/rules/x.md",
        "sed -Ei 's/a/b/' .claude/rules/x.md",
        "sed -n -i 's/a/b/' .claude/rules/x.md",
        "sed --in-place 's/a/b/' .claude/rules/x.md",
        "sed --in-place=.bak 's/a/b/' .claude/rules/x.md",
        "sed -I '' 's/a/b/' .claude/rules/x.md",
        "sed 's/a/b/w .claude/x' in.txt",
        "sed 's/a/b/gw .claude/x' in.txt",
        "sed -n '1w .git/hooks/pre-commit' hook.sh",
        "sed -n '/x/W .git/hooks/pre-commit' hook.sh",
        "sed -n 'p;w .claude/x' in.txt",
        "sed -e p -e 'w .claude/x' in.txt",
        "sed --expression='w .claude/x' in.txt",
        "rg -l x .claude/rules | sed 's/^/rm /e'",
        "sed '1e rm -rf .claude/x' in.txt",
        "sed '$e' .claude/list",
        "sed -f s.sed .claude/x",
        "sed --file=s.sed .claude/x",
        "S='w .claude/x'; sed \"$S\" in.txt",
        "sed '1{w .claude/x\n}' in.txt",
        "sed 'q;z;w .claude/x' in.txt",
        "sed 'r x\\\nw .claude/x' in.txt",
        "sed '# note\\\nw .claude/x' in.txt",
        "sed ':a;w .claude/x' in.txt",
        "sed 's/[/]/x/w .claude/x' in.txt",
        "sed s/a/b/ -i .claude/x",
        "sed --sandboxed 's/a/b/' .claude/x",
        "/tmp/bin/sed -n p .claude/x",
        "echo $(rm -rf .claude/x)",
        "x=$(sed -i s/a/b/ .claude/y)",
        "echo $(echo $(rm .git/index))",
        "x=$(echo $(rm .git/index))",
        "x=`rm .claude/x`",
        "x=\"$(cat .claude/a > .claude/b)\"",
        "diff <(rm .claude/a) b",
        "cat .claude/x > >(sh)",
        "cat <<EOF\n$(cat .claude/x)\nEOF",
        "sed -n \"$(echo 1)p\" .claude/x",
        ">$(printf .claude/x) cat a",
        "cat a > $(printf .claude/x)",
        "cat a > \"$(echo .claude/x)\"",
        "cd x; sort a > $(echo .claude/x)",
        "cp a \"$(echo .claude/x)\"",
        "ls <>$(echo .claude/x)",
        "PATH=./bin; cat .claude/x",
        "PATH=$(pwd)/bin cat .claude/x",
        "for f in .claude/*; do rm \"$f\"; done",
        "if true; then cp a .git/hooks/x; fi",
        "while read f; do rm \"$f\"; done < .claude/list",
        "for f in a; do cat \"$f\"; done > .claude/x",
        "for f in .claude/*; do cat \"$f\"; done | sh",
        "if rm .claude/x; then :; fi",
        "! rm .claude/x",
        "until rm .claude/x; do :; done",
        "if grep -q x a; then :; elif rm .claude/x; then :; fi",
        "for f in a; do\nrm .claude/x\ndone",
        "case .claude/x in *) ;; esac",
        "for f in $(rm .claude/x); do :; done",
        "# note\nrm .claude/x",
        "echo \"(\"; f() { rm .claude/x; }; f",
        "find .claude/ -name \\;# -o -delete",
        "find .claude/ -name ';#' -o -delete <<< x",
        "git log 'a;#' --output=.claude/settings.json; # x",
        "sort 'a;#' -o .claude/settings.json; # x",
        "cat .claude/rules/a >| .claude/settings.json",
        "cat .claude/rules/a >! .claude/settings.json",
        "cat a >| $(echo .claude/x)",
        "cat .claude/rules/a >&.claude/settings.json",
        "sed -n $' #\\nw .claude/settings.json' in.txt",
        "sort $'-o.claude/x' a",
        "echo `echo \\`touch .claude/x\\``",
        "sed -n 's/[/]/p;/w .claude/rules/p' in.txt",
        "sed -n 's/[[:alpha:]/]/p;/w .claude/rules/p' in.txt",
        "sed -l 's/a/b/w .claude/settings.json' p",
        "for PATH in ./bin; do cat .claude/x; done",
        "printf -v PATH ./bin; cat .claude/x",
        "TMPDIR=.claude/ sort big.txt",
        "for TMPDIR in .claude/; do sort big.txt; done",
    ] {
        let input = serde_json::json!({ "command": command });
        assert!(input_touches_protected_path("Bash", &input), "{command}");
    }
}

#[test]
fn input_touches_protected_path_ignores_a_mention_or_a_read() {
    for command in [
        "cat .git/HEAD",
        "grep -n x .claude/rules/a.md",
        "wc -l .claude/rules/frontend.md",
        "ls .claude/skills",
        "find . -name '*.ts' -not -path '*/.git/*'",
        "git log --oneline -- .claude/",
        "git diff main -- .claude/rules/x.md | head -20",
        "grep -o '\\.claude/[^ ]*\\|\\.git/[^ ]*' /tmp/q.txt | head",
        "N=\"${TMPDIR:-/tmp}/handoff.md\"; cat > \"$N\" <<'EOF'\n# Handoff\nSee `.claude/rules/dev-runtime.md`.\nEOF",
        "cat > /tmp/kh/msg.md <<'EOF'\nTouches .git/hooks and .claude/settings.json.\nEOF",
        "zsh -lc 'grep -rn TODO .claude/rules'",
        "/bin/zsh -lc 'bash -c \"cat .claude/rules/a.md\"'",
        "cd /repo && grep -rn x .claude/rules",
        "sed -n '64,67p' .claude/rules/x.md",
        "rg -n x .claude/rules | sed \"s|a||\"",
        "sed -n '1p;5p' .claude/rules/x.md",
        "sed -nE '/^## /,/^## /{/foo/p;}' .claude/rules/x.md",
        "sed -e 's/a/b/g' -e '/^$/d' .claude/rules/x.md",
        "sed 's/\\(w\\)/[\\1]/2' .claude/rules/x.md",
        "sed 's#\\#w#e#gI' .claude/rules/x.md",
        "sed -n '$p' .claude/rules/x.md",
        "sed '1i\\\nheader' .claude/rules/x.md",
        "sed '/x/a appended w text' .claude/rules/x.md",
        "sed 'y/abc/xyz/' .claude/rules/x.md",
        "sed -n '/start/,+3p;:a;N;ba' .claude/rules/x.md",
        "sed -n '0~4p' .claude/rules/x.md 2>/dev/null",
        "sed 'r .claude/rules/y.md' .claude/rules/x.md",
        "sed --quiet --regexp-extended -e '1,3p' -- .claude/rules/x.md",
        "x=$(sed -n 1p .claude/rules/a.md)",
        "x=\"$(cat .claude/rules/a.md)\"",
        "n=`wc -l < .claude/rules/a.md`",
        "x=$(grep -c x .claude/rules/a.md); echo \"$x\"",
        "x=$(y=$(cat .claude/rules/a.md))",
        "X=$(cat .claude/rules/a.md) grep -n y .claude/rules/b.md",
        "diff <(cat .claude/a) b",
        "diff <(git show main:.claude/rules/a.md) .claude/rules/a.md",
        "R=/repo; rg -n x .claude/rules | sed \"s|$R/||\"",
        "for f in a .claude/rules/x.md:84; do ln=${f##*:}; line=$(sed -n \"${ln}p\" \"$f\"); done",
        "for f in $(ls .claude/rules); do wc -l \"$f\"; done",
        "for f in a b; do grep -n x \"$f\"; done < .claude/list",
        "if grep -q x .claude/a; then cat .claude/b; else echo no; fi",
        "! grep -q x .claude/rules/a.md",
        "for f in .claude/rules/*.md; do\nhead -3 \"$f\"\ndone",
        "grep -E \"(a|b)\" .claude/rules/x.md",
        "echo \"see .claude/x (y)\"",
        "grep -n '{' .claude/settings.json",
        "cd \"$(git rev-parse --show-toplevel)\" && cat .claude/x",
        "readlink -f \"$(pwd)/.claude\"",
        "cat \"$(git rev-parse --show-toplevel)/.claude/x\"",
        "ls $(dirname .claude/rules/x.md)",
        "cat $(echo .claude/x)",
        "sort .claude/list",
        "sort -u -k2,2 -t: .claude/list",
        "grep x .claude/a | sort -rn | head",
        "sort --reverse --key=2 -- .claude/list",
        "# list the rules\nls .claude/rules",
        "cat .claude/x # example: echo x > .claude/y",
        "ls .claude/rules # don't touch",
        "cat .claude/x # example: echo $'x'",
        "sort .claude/list # not sort -o",
        "sed -n '/[0-9]/p' .claude/rules/x.md",
        "sed 's/[[:space:]]*$//' .claude/rules/x.md",
        "git check-ignore -v .claude/settings.local.json",
        "git for-each-ref refs/heads | grep .claude/",
        "git merge-base --is-ancestor HEAD main && cat .claude/x",
        "git add .claude/rules/x.md",
        "git add .claude/rules/x.md && git commit -m 'docs: x'",
        "git commit -m x .claude/rules/x.md",
    ] {
        let input = serde_json::json!({ "command": command });
        assert!(!input_touches_protected_path("Bash", &input), "{command}");
    }
}

#[test]
fn input_touches_protected_path_returns_false_for_unrelated_bash_command() {
    let input = serde_json::json!({"command": "git status --short"});
    assert!(!input_touches_protected_path("Bash", &input));
}

#[test]
fn input_touches_protected_path_does_not_match_substrings_without_trailing_slash() {
    let input = serde_json::json!({"command": "cat .gitignore"});
    assert!(!input_touches_protected_path("Bash", &input));
    let input = serde_json::json!({"command": "ls .claude_backup"});
    assert!(!input_touches_protected_path("Bash", &input));
}

#[test]
fn input_touches_protected_path_returns_false_for_non_bash_tools() {
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
fn derive_allow_pattern_broad_offers_bash_when_bash_command_touches_dot_claude() {
    let input = serde_json::json!({
        "command": "rm -rf /Users/me/.claude/skills/grill"
    });
    assert_eq!(
        derive_allow_pattern("Bash", &input, AllowScope::Broad, &TrustedHeads::NONE).as_deref(),
        Some("Bash"),
        "the bare Bash line covers a write under `.claude/`"
    );
}

#[test]
fn derive_allow_pattern_narrow_returns_none_when_bash_command_touches_dot_claude() {
    let input = serde_json::json!({
        "command": "rm -rf /Users/me/.claude/skills/grill"
    });
    assert_eq!(
        derive_allow_pattern("Bash", &input, AllowScope::Narrow, &TrustedHeads::NONE),
        None,
        "no persisted Bash(rm:*) line covers a write under `.claude/`"
    );
}

#[test]
fn derive_allow_pattern_narrow_returns_none_when_bash_command_touches_dot_git() {
    let input = serde_json::json!({"command": "git push --force HEAD:.git/refs/heads/x"});
    assert_eq!(
        derive_allow_pattern("Bash", &input, AllowScope::Narrow, &TrustedHeads::NONE),
        None,
    );
}

#[test]
fn derive_allow_pattern_session_returns_none_when_bash_command_touches_dot_claude() {
    let input = serde_json::json!({
        "command": "rm -rf /Users/me/.claude/skills/grill"
    });
    assert_eq!(
        derive_allow_pattern("Bash", &input, AllowScope::Session, &TrustedHeads::NONE),
        None,
    );
}

#[test]
fn derive_allow_pattern_broad_unaffected_for_unrelated_bash_command() {
    let input = serde_json::json!({"command": "git status"});
    assert_eq!(
        derive_allow_pattern("Bash", &input, AllowScope::Broad, &TrustedHeads::NONE).as_deref(),
        Some("Bash"),
    );
    assert_eq!(
        derive_allow_pattern("Bash", &input, AllowScope::Narrow, &TrustedHeads::NONE).as_deref(),
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
        derive_allow_pattern("Edit", &input, AllowScope::Session, &TrustedHeads::NONE).as_deref(),
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
        derive_allow_pattern("Write", &input, AllowScope::Session, &TrustedHeads::NONE).as_deref(),
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
        derive_allow_pattern(
            "NotebookEdit",
            &input,
            AllowScope::Session,
            &TrustedHeads::NONE
        )
        .as_deref(),
        Some("NotebookEdit(/tmp/nb.ipynb)"),
    );
}

#[test]
fn derive_allow_pattern_session_two_edits_to_same_file_match() {
    let a = serde_json::json!({"file_path": "/x", "old_string": "a", "new_string": "b"});
    let b = serde_json::json!({"file_path": "/x", "old_string": "c", "new_string": "d"});
    assert_eq!(
        derive_allow_pattern("Edit", &a, AllowScope::Session, &TrustedHeads::NONE),
        derive_allow_pattern("Edit", &b, AllowScope::Session, &TrustedHeads::NONE),
    );
}

#[test]
fn derive_allow_pattern_session_edit_missing_path_returns_none() {
    let input = serde_json::json!({"old_string": "x"});
    assert_eq!(
        derive_allow_pattern("Edit", &input, AllowScope::Session, &TrustedHeads::NONE),
        None
    );
}

#[test]
fn derive_allow_pattern_session_bash_reuses_narrow_pattern() {
    let input = serde_json::json!({"command": "git push origin main"});
    assert_eq!(
        derive_allow_pattern("Bash", &input, AllowScope::Session, &TrustedHeads::NONE).as_deref(),
        Some("Bash(git:*)"),
    );
}

#[test]
fn derive_allow_pattern_session_skill_reuses_narrow_pattern() {
    let input = serde_json::json!({"skill": "superpowers:test-driven-development"});
    assert_eq!(
        derive_allow_pattern("Skill", &input, AllowScope::Session, &TrustedHeads::NONE).as_deref(),
        Some("Skill(superpowers:*)"),
    );
}

#[test]
fn derive_allow_pattern_session_other_tool_falls_back_to_bare_name() {
    let input = serde_json::json!({"pattern": "foo"});
    assert_eq!(
        derive_allow_pattern("Read", &input, AllowScope::Session, &TrustedHeads::NONE).as_deref(),
        Some("Read"),
    );
}

#[test]
fn allowed_tools_flag_keeps_grants_in_order_after_the_engine_tools() {
    let body =
        "# header comment\n\nBash\nRead\n  Edit  \n# inline comment\nSkill(superpowers:*)\n\n";

    assert_eq!(
        allowed_tools_flag(lines(body)),
        format!(
            "{},Read,Edit,Skill(superpowers:*)",
            ENGINE_ALLOWED_TOOLS.join(",")
        ),
    );
}
