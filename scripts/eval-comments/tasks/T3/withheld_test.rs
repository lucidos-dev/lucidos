use super::*;
use serde_json::json;

fn bash(cmd: &str) -> StaticVerdict {
    static_classify(tn::RUN_BASH, &json!({ "command": cmd }))
}

#[test]
fn ordinary_env_names_and_arguments_keep_the_read_only_fast_path() {
    for cmd in [
        "ENVIRONMENT=staging cat data/config.yaml",
        "ENV_FILE=.env ls",
        "IFS_MODE=x echo hi",
        "grep NODE_OPTIONS= data/f.txt",
        "echo LD_PRELOAD=x",
    ] {
        assert_eq!(bash(cmd), StaticVerdict::Settled(RiskLane::Safe), "{cmd}");
    }
}

#[test]
fn code_loading_assignments_still_leave_the_fast_path() {
    for cmd in [
        "ENV=/tmp/rc bash -c 'echo hi'",
        "IFS=, cat data/f.txt",
        "LD_PRELOAD=/tmp/evil.so ls",
    ] {
        assert!(
            !matches!(bash(cmd), StaticVerdict::Settled(RiskLane::Safe)),
            "{cmd} must not settle Safe"
        );
    }
}
