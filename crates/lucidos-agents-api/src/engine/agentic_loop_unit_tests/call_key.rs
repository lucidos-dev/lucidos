mod derive_call_key_tests {
    use super::super::derive_call_key;
    use crate::llm::tool_names as tn;
    use serde_json::json;

    #[test]
    fn run_bash_buckets_by_first_token() {
        let key = derive_call_key(tn::RUN_BASH, &json!({ "command": "git status" }));
        assert_eq!(key, "git");
    }

    #[test]
    fn run_bash_trims_leading_whitespace() {
        let key = derive_call_key(tn::RUN_BASH, &json!({ "command": "  git  add ." }));
        assert_eq!(key, "git");
    }

    #[test]
    fn run_bash_empty_command_falls_back_to_tool_name() {
        let key = derive_call_key(tn::RUN_BASH, &json!({ "command": "" }));
        assert_eq!(key, tn::RUN_BASH);
    }

    #[test]
    fn run_bash_whitespace_only_command_falls_back_to_tool_name() {
        let key = derive_call_key(tn::RUN_BASH, &json!({ "command": "   " }));
        assert_eq!(key, tn::RUN_BASH);
    }

    #[test]
    fn run_bash_missing_command_falls_back_to_tool_name() {
        let key = derive_call_key(tn::RUN_BASH, &json!({}));
        assert_eq!(key, tn::RUN_BASH);
    }

    #[test]
    fn run_bash_distinct_commands_bucket_separately() {
        let git = derive_call_key(tn::RUN_BASH, &json!({ "command": "git status" }));
        let cargo = derive_call_key(tn::RUN_BASH, &json!({ "command": "cargo test" }));
        let ls = derive_call_key(tn::RUN_BASH, &json!({ "command": "ls -la" }));
        assert_eq!(git, "git");
        assert_eq!(cargo, "cargo");
        assert_eq!(ls, "ls");
        assert_ne!(git, cargo);
        assert_ne!(cargo, ls);
    }

    #[test]
    fn run_bash_same_prefix_buckets_together() {
        let a = derive_call_key(tn::RUN_BASH, &json!({ "command": "git status" }));
        let b = derive_call_key(tn::RUN_BASH, &json!({ "command": "git add ." }));
        let c = derive_call_key(tn::RUN_BASH, &json!({ "command": "git commit -m x" }));
        assert_eq!(a, "git");
        assert_eq!(b, "git");
        assert_eq!(c, "git");
    }

    #[test]
    fn read_file_no_window_args_keys_by_path() {
        let key = derive_call_key(tn::READ_FILE, &json!({ "path": "src/main.rs" }));
        assert_eq!(key, "src/main.rs");
    }

    #[test]
    fn read_file_different_start_lines_bucket_separately() {
        let a = derive_call_key(
            tn::READ_FILE,
            &json!({ "path": "src/main.rs", "start_line": 1, "line_count": 100 }),
        );
        let b = derive_call_key(
            tn::READ_FILE,
            &json!({ "path": "src/main.rs", "start_line": 101, "line_count": 100 }),
        );
        let c = derive_call_key(
            tn::READ_FILE,
            &json!({ "path": "src/main.rs", "start_line": 201, "line_count": 100 }),
        );
        assert_ne!(a, b);
        assert_ne!(b, c);
        assert_ne!(a, c);
        assert!(a.starts_with("src/main.rs"));
    }

    #[test]
    fn read_file_same_args_share_bucket() {
        let a = derive_call_key(
            tn::READ_FILE,
            &json!({ "path": "src/main.rs", "start_line": 1, "line_count": 100 }),
        );
        let b = derive_call_key(
            tn::READ_FILE,
            &json!({ "path": "src/main.rs", "start_line": 1, "line_count": 100 }),
        );
        assert_eq!(a, b);
    }

    #[test]
    fn read_file_different_line_count_buckets_separately() {
        let a = derive_call_key(
            tn::READ_FILE,
            &json!({ "path": "src/main.rs", "line_count": 50 }),
        );
        let b = derive_call_key(
            tn::READ_FILE,
            &json!({ "path": "src/main.rs", "line_count": 100 }),
        );
        assert_ne!(a, b);
    }

    #[test]
    fn read_file_different_offset_buckets_separately() {
        let a = derive_call_key(
            tn::READ_FILE,
            &json!({ "path": "src/main.rs", "offset": 0 }),
        );
        let b = derive_call_key(
            tn::READ_FILE,
            &json!({ "path": "src/main.rs", "offset": 50000 }),
        );
        assert_ne!(a, b);
    }

    #[test]
    fn web_search_keys_by_query_unchanged() {
        let key = derive_call_key(tn::WEB_SEARCH, &json!({ "query": "rust async" }));
        assert_eq!(key, "rust async");
    }

    #[test]
    fn non_run_bash_with_command_arg_does_not_bucket_by_command() {
        let key = derive_call_key(tn::READ_FILE, &json!({ "command": "git status" }));
        assert_eq!(key, "");
    }

    #[test]
    fn non_run_bash_without_known_arg_returns_empty() {
        let key = derive_call_key(tn::LIST_FILES, &json!({}));
        assert_eq!(key, "");
    }

    #[test]
    fn run_python_time_sleep_verbatim_retry_shares_bucket_escalating_does_not() {
        let a = derive_call_key(
            tn::RUN_PYTHON,
            &json!({ "code": "import time\ntime.sleep(120)\nprint('done')" }),
        );
        let b = derive_call_key(
            tn::RUN_PYTHON,
            &json!({ "code": "import time\ntime.sleep(180)\nprint('done')" }),
        );
        assert!(
            a.starts_with("time.sleep("),
            "bucket key should start with the polling call: {a:?}"
        );
        assert_ne!(
            a, b,
            "escalating durations bucket separately — the integer is part of the actionable line ({a:?} vs {b:?})"
        );
        let c = derive_call_key(
            tn::RUN_PYTHON,
            &json!({ "code": "import time\ntime.sleep(120)\nprint('done')" }),
        );
        assert_eq!(a, c, "verbatim retry must collide: {a:?} vs {c:?}");
    }

    #[test]
    fn run_python_different_actionable_lines_bucket_separately() {
        let a = derive_call_key(
            tn::RUN_PYTHON,
            &json!({ "code": "import pandas as pd\ndf = pd.read_csv('a.csv')\nprint(df.head())" }),
        );
        let b = derive_call_key(
            tn::RUN_PYTHON,
            &json!({ "code": "import pandas as pd\ndf = pd.read_csv('b.csv')\nprint(df.head())" }),
        );
        assert_ne!(a, b, "different input files must bucket separately");
    }

    #[test]
    fn run_python_skips_imports_and_comments() {
        let a = derive_call_key(
            tn::RUN_PYTHON,
            &json!({
                "code": "# header\nimport os\nfrom pathlib import Path\n\n# body\nx = 1\nprint(x)"
            }),
        );
        let b = derive_call_key(
            tn::RUN_PYTHON,
            &json!({
                "code": "# header\nimport os\nfrom pathlib import Path\n\n# body\ny = 2\nprint(y)"
            }),
        );
        assert_eq!(
            a, "x = 1",
            "first actionable line must drive the key, got: {a:?}"
        );
        assert_eq!(b, "y = 2");
        assert_ne!(a, b);
    }

    #[test]
    fn run_python_pure_import_falls_back_to_tool_name() {
        let key = derive_call_key(
            tn::RUN_PYTHON,
            &json!({ "code": "import os\nimport sys\nfrom pathlib import Path\n# nothing else" }),
        );
        assert_eq!(key, tn::RUN_PYTHON);
    }

    #[test]
    fn run_python_empty_code_falls_back_to_tool_name() {
        let key = derive_call_key(tn::RUN_PYTHON, &json!({ "code": "" }));
        assert_eq!(key, tn::RUN_PYTHON);
    }

    #[test]
    fn run_python_missing_code_falls_back_to_tool_name() {
        let key = derive_call_key(tn::RUN_PYTHON, &json!({}));
        assert_eq!(key, tn::RUN_PYTHON);
    }

    #[test]
    fn run_python_background_uses_same_key_logic_as_run_python() {
        let code = "result = expensive_thing()\nprint(result)";
        let fg = derive_call_key(tn::RUN_PYTHON, &json!({ "code": code }));
        let bg = derive_call_key(tn::RUN_PYTHON_BACKGROUND, &json!({ "code": code }));
        assert_eq!(fg, bg);
        assert_eq!(fg, "result = expensive_thing()");
    }

    #[test]
    fn run_python_key_truncates_at_80_chars_with_hash_suffix() {
        let long = format!("x = {}", "a".repeat(200));
        let key = derive_call_key(tn::RUN_PYTHON, &json!({ "code": long }));
        assert!(key.starts_with("x = "));
        assert!(
            key.contains('#'),
            "long-line key must carry hash suffix: {key:?}"
        );
        assert!(
            key.chars().count() <= 89,
            "key was {} chars (expected ≤89): {key:?}",
            key.chars().count()
        );
    }

    #[test]
    fn run_python_long_lines_diverging_past_80_chars_bucket_separately() {
        let a_code = format!(
            "df = pd.read_csv('/very/long/absolute/path/to/data/folder/{}/file_A.csv')",
            "deep/nested/dir".repeat(3)
        );
        let b_code = format!(
            "df = pd.read_csv('/very/long/absolute/path/to/data/folder/{}/file_B.csv')",
            "deep/nested/dir".repeat(3)
        );
        assert!(
            a_code.len() > 90,
            "test setup wrong, line too short: len={}",
            a_code.len()
        );
        let a = derive_call_key(tn::RUN_PYTHON, &json!({ "code": a_code }));
        let b = derive_call_key(tn::RUN_PYTHON, &json!({ "code": b_code }));
        assert!(a.contains('#'), "expected hash suffix on long line: {a:?}");
        assert!(b.contains('#'), "expected hash suffix on long line: {b:?}");
        assert_ne!(
            a, b,
            "long-line scripts diverging past char 80 must bucket separately ({a:?} vs {b:?})"
        );
    }

    #[test]
    fn run_python_key_under_80_chars_no_hash_suffix() {
        let key = derive_call_key(tn::RUN_PYTHON, &json!({ "code": "x = compute()" }));
        assert_eq!(key, "x = compute()");
        assert!(!key.contains('#'));
    }
}
