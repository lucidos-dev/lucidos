use super::*;

fn background_tasks_switch(cmd: &tokio::process::Command) -> Option<std::ffi::OsString> {
    collect_envs(cmd)
        .get(std::ffi::OsStr::new("CLAUDE_CODE_DISABLE_BACKGROUND_TASKS"))
        .cloned()
}

#[test]
fn every_session_runs_with_claude_codes_background_tasks_off() {
    let p = std::path::Path::new("/tmp");
    let cmd = session_command(&test_spawn_args(p, p, uuid::Uuid::new_v4()), None);
    assert_eq!(
        background_tasks_switch(&cmd).as_deref(),
        Some(std::ffi::OsStr::new("1")),
    );
}

#[test]
fn a_user_env_var_cannot_turn_claude_codes_background_tasks_back_on() {
    let p = std::path::Path::new("/tmp");
    let user_env = vec![(
        "CLAUDE_CODE_DISABLE_BACKGROUND_TASKS".to_string(),
        "0".to_string(),
    )];
    let mut args = test_spawn_args(p, p, uuid::Uuid::new_v4());
    args.user_env_vars = &user_env;
    let cmd = session_command(&args, None);
    assert_eq!(
        background_tasks_switch(&cmd).as_deref(),
        Some(std::ffi::OsStr::new("1")),
        "the engine-owned switch must win over a user env var",
    );
}
