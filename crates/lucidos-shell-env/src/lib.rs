use std::ffi::{OsStr, OsString};
use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const HYDRATED_VARS: &[&str] = &[
    "PATH",
    "OPENAI_API_KEY",
    "ANTHROPIC_API_KEY",
    "LUCIDOS_OPENROUTER_API_KEY",
    "LUCIDOS_XAI_API_KEY",
    "LUCIDOS_LOCAL_BASE_URL",
    "LUCIDOS_LOCAL_API_KEY",
    "VERTEX_PROJECT_ID",
    "VERTEX_REGION",
    "GOOGLE_APPLICATION_CREDENTIALS",
    "CLOUDSDK_CONFIG",
    "CODEX_HOME",
    "CLAUDE_CONFIG_DIR",
];

#[cfg(target_os = "macos")]
const DEFAULT_SHELL: &str = "/bin/zsh";
#[cfg(not(target_os = "macos"))]
const DEFAULT_SHELL: &str = "/bin/sh";

const MARKER: &str = "__LUCIDOS_SHELL_ENV__";

const SHELL_TIMEOUT: Duration = Duration::from_secs(5);

const DRAIN_GRACE: Duration = Duration::from_millis(250);

pub fn hydrate_login_shell_env(log: impl Fn(String)) {
    let packaged = std::env::var_os("LUCIDOS_PACKAGED");
    let shlvl = std::env::var_os("SHLVL");
    if !should_hydrate(packaged.as_deref(), shlvl.as_deref()) {
        if shlvl.is_some() && is_packaged(packaged.as_deref()) {
            log("[shell-env] started from a shell; keeping the inherited environment".to_owned());
        }
        return;
    }
    let shell = login_shell(std::env::var_os("SHELL"));
    let stdout = match read_login_shell_env(&shell, SHELL_TIMEOUT) {
        Ok(stdout) => stdout,
        Err(e) => {
            log(format!(
                "[shell-env] {e}; continuing without the login-shell environment"
            ));
            return;
        }
    };
    let Some(payload) = env_after_marker(&stdout) else {
        log(format!(
            "[shell-env] {} produced no environment dump; continuing without it",
            shell.display()
        ));
        return;
    };

    let plan = hydration_plan(&parse_null_delimited(payload), |name| {
        std::env::var_os(name)
    });
    for (name, value) in &plan {
        // SAFETY: single-threaded at this point. See the safety contract above.
        unsafe {
            std::env::set_var(name, value);
        }
    }

    if plan.is_empty() {
        log(format!(
            "[shell-env] {} contributed nothing new; continuing with the inherited environment",
            shell.display()
        ));
    } else {
        let names: Vec<&str> = plan.iter().map(|(name, _)| name.as_str()).collect();
        log(format!(
            "[shell-env] hydrated from {}: {}",
            shell.display(),
            names.join(", ")
        ));
    }
}

fn should_hydrate(packaged: Option<&OsStr>, shlvl: Option<&OsStr>) -> bool {
    is_packaged(packaged) && shlvl.is_none()
}

fn is_packaged(packaged: Option<&OsStr>) -> bool {
    packaged == Some(OsStr::new("1"))
}

fn login_shell(shell: Option<OsString>) -> PathBuf {
    shell
        .filter(|s| !s.to_string_lossy().trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_SHELL))
}

fn login_shell_command(shell: &Path) -> Command {
    let mut cmd = Command::new(shell);
    cmd.arg("-ilc")
        .arg(format!("printf '%s' '{MARKER}'; /usr/bin/env -0"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    cmd
}

fn read_login_shell_env(shell: &Path, timeout: Duration) -> Result<Vec<u8>, String> {
    read_stdout_bounded(
        login_shell_command(shell),
        &shell.display().to_string(),
        timeout,
    )
}

fn kill_process_group(pid: u32) {
    // SAFETY: a negative pid is POSIX for "every process in group |pid|". The
    // group is the one we created for our own child, so nothing outside this
    // subtree can be in it. An already-dead group is a harmless ESRCH.
    unsafe {
        libc::kill(-(pid as i32), libc::SIGKILL);
    }
}

fn read_stdout_bounded(
    mut cmd: Command,
    label: &str,
    timeout: Duration,
) -> Result<Vec<u8>, String> {
    let deadline = Instant::now() + timeout;
    cmd.process_group(0);
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("could not run {label}: {e}"))?;
    let pid = child.id();

    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| format!("{label} gave no stdout pipe"))?;
    let (done, output) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf);
        let _ = done.send(buf);
    });

    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                kill_process_group(pid);
                let _ = child.wait();
                return Err(format!(
                    "{label} did not answer within {timeout:?} and was stopped"
                ));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => return Err(format!("could not wait for {label}: {e}")),
        }
    }

    match output.recv_timeout(DRAIN_GRACE) {
        Ok(buf) => {
            let _ = reader.join();
            Ok(buf)
        }
        Err(_) => Err(format!(
            "{label} answered but something it started still held its output open"
        )),
    }
}

fn env_after_marker(stdout: &[u8]) -> Option<&[u8]> {
    let marker = MARKER.as_bytes();
    let start = stdout
        .windows(marker.len())
        .position(|w| w == marker)?
        .checked_add(marker.len())?;
    Some(&stdout[start..])
}

fn parse_null_delimited(payload: &[u8]) -> Vec<(String, String)> {
    payload
        .split_inclusive(|b| *b == 0)
        .filter_map(|entry| entry.strip_suffix(&[0]))
        .filter_map(|entry| std::str::from_utf8(entry).ok())
        .filter_map(|entry| entry.split_once('='))
        .filter(|(name, _)| !name.is_empty())
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect()
}

fn variables_to_apply(
    shell_env: &[(String, String)],
    already_set: impl Fn(&str) -> bool,
) -> Vec<(String, String)> {
    shell_env
        .iter()
        .filter(|(name, _)| name != "PATH")
        .filter(|(name, _)| HYDRATED_VARS.contains(&name.as_str()))
        .filter(|(_, value)| !value.is_empty())
        .filter(|(name, _)| !already_set(name))
        .cloned()
        .collect()
}

fn hydration_plan(
    shell_env: &[(String, String)],
    inherited: impl Fn(&str) -> Option<OsString>,
) -> Vec<(String, OsString)> {
    let mut plan: Vec<(String, OsString)> =
        variables_to_apply(shell_env, |name| inherited(name).is_some())
            .into_iter()
            .map(|(name, value)| (name, OsString::from(value)))
            .collect();
    if let Some((_, shell_path)) = shell_env.iter().find(|(name, _)| name == "PATH") {
        let merged = merged_path(OsStr::new(shell_path), inherited("PATH").as_deref());
        plan.push(("PATH".to_string(), merged));
    }
    plan
}

fn merged_path(shell_path: &OsStr, inherited: Option<&OsStr>) -> OsString {
    let mut dirs: Vec<PathBuf> = Vec::new();
    let mut seen: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    let inherited = inherited.unwrap_or(OsStr::new(""));
    for dir in std::env::split_paths(shell_path).chain(std::env::split_paths(inherited)) {
        if dir.as_os_str().is_empty() || !seen.insert(dir.clone()) {
            continue;
        }
        dirs.push(dir);
    }
    std::env::join_paths(&dirs).unwrap_or_else(|_| shell_path.to_os_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os(s: &str) -> Option<&OsStr> {
        Some(OsStr::new(s))
    }

    #[test]
    fn a_packaged_service_launch_is_hydrated() {
        assert!(
            should_hydrate(os("1"), None),
            "LUCIDOS_PACKAGED=1 and no SHLVL is launchd or systemd starting a shipped install"
        );
    }

    #[test]
    fn a_packaged_gateway_started_from_a_shell_is_left_alone() {
        assert!(
            !should_hydrate(os("1"), os("1")),
            "SHLVL present means a shell already gave us its environment"
        );
        assert!(!should_hydrate(os("1"), os("3")));
    }

    #[test]
    fn a_dev_gateway_is_never_hydrated() {
        assert!(!should_hydrate(None, os("2")), "a dev gateway from a shell");
        assert!(
            !should_hydrate(None, None),
            "even with no SHLVL, a gateway that is not a shipped install keeps its env"
        );
    }

    #[test]
    fn only_the_exact_packaged_value_counts() {
        assert!(!should_hydrate(os("0"), None));
        assert!(!should_hydrate(os(""), None));
        assert!(!should_hydrate(os("true"), None));
    }

    #[test]
    fn the_fallback_shell_exists_on_this_platform() {
        assert!(
            Path::new(DEFAULT_SHELL).exists(),
            "{DEFAULT_SHELL} is the fallback here, so it has to be installed"
        );
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn linux_falls_back_to_sh_not_zsh() {
        assert_eq!(DEFAULT_SHELL, "/bin/sh", "zsh is not a Linux default");
    }

    #[test]
    fn login_shell_uses_the_users_shell() {
        assert_eq!(
            login_shell(Some(OsString::from("/opt/homebrew/bin/fish"))),
            PathBuf::from("/opt/homebrew/bin/fish")
        );
        assert_eq!(
            login_shell(Some(OsString::from("/bin/bash"))),
            PathBuf::from("/bin/bash")
        );
    }

    #[test]
    fn login_shell_falls_back_when_shell_says_nothing() {
        assert_eq!(login_shell(None), PathBuf::from(DEFAULT_SHELL));
        assert_eq!(
            login_shell(Some(OsString::from(""))),
            PathBuf::from(DEFAULT_SHELL)
        );
        assert_eq!(
            login_shell(Some(OsString::from("   "))),
            PathBuf::from(DEFAULT_SHELL),
            "a whitespace-only SHELL is not a shell"
        );
    }

    #[test]
    fn marker_discards_whatever_the_profile_printed_first() {
        let raw = format!("Welcome back!\nnvm: v20.11.0\n{MARKER}A=1\0");
        let payload = env_after_marker(raw.as_bytes()).expect("marker is present");
        assert_eq!(
            parse_null_delimited(payload),
            vec![("A".to_string(), "1".to_string())],
            "a chatty profile must not become environment entries"
        );
    }

    #[test]
    fn no_marker_means_no_payload() {
        assert!(
            env_after_marker(b"zsh: command not found: something\n").is_none(),
            "a shell that never reached the dump must not look like an empty environment"
        );
    }

    #[test]
    fn a_value_containing_a_newline_survives_whole() {
        let payload = b"KEY=line one\nline two\nline three\0NEXT=b\0";
        assert_eq!(
            parse_null_delimited(payload),
            vec![
                (
                    "KEY".to_string(),
                    "line one\nline two\nline three".to_string()
                ),
                ("NEXT".to_string(), "b".to_string()),
            ],
            "newline-splitting is exactly what null-delimiting exists to avoid"
        );
    }

    #[test]
    fn a_value_containing_equals_survives_whole() {
        let payload = b"LUCIDOS_LOCAL_BASE_URL=http://h/v1?a=1&b=2\0";
        assert_eq!(
            parse_null_delimited(payload),
            vec![(
                "LUCIDOS_LOCAL_BASE_URL".to_string(),
                "http://h/v1?a=1&b=2".to_string()
            )],
            "the name ends at the FIRST '=', the rest is all value"
        );
    }

    #[test]
    fn junk_entries_are_dropped_rather_than_guessed_at() {
        let mut payload: Vec<u8> = b"no equals here\0=orphan value\0GOOD=y\0".to_vec();
        payload.extend_from_slice(b"BAD=\xff\xfe\0");
        assert_eq!(
            parse_null_delimited(&payload),
            vec![("GOOD".to_string(), "y".to_string())]
        );
    }

    #[test]
    fn an_empty_payload_parses_to_nothing() {
        assert!(parse_null_delimited(b"").is_empty());
    }

    #[test]
    fn an_unterminated_tail_is_a_truncated_dump_and_is_dropped() {
        assert_eq!(
            parse_null_delimited(b"VERTEX_REGION=europe-west1\0OPENAI_API_KEY=sk-trunc"),
            env(&[("VERTEX_REGION", "europe-west1")]),
            "half a key applied as if it were whole would fail auth for no visible reason"
        );
    }

    fn env(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(n, v)| (n.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn only_allowlisted_names_are_applied() {
        let shell_env = env(&[
            ("OPENAI_API_KEY", "sk-test"),
            ("LUCIDOS_BIND_ALL", "1"),
            ("DATABASE_URL", "postgres://x"),
            ("AWS_SECRET_ACCESS_KEY", "nope"),
            ("CODEX_HOME", "/home/u/.codex"),
        ]);
        let applied = variables_to_apply(&shell_env, |_| false);
        assert_eq!(
            applied,
            env(&[
                ("OPENAI_API_KEY", "sk-test"),
                ("CODEX_HOME", "/home/u/.codex"),
            ]),
            "a shell profile must not be able to repoint the packaged stack's topology"
        );
    }

    #[test]
    fn a_name_already_set_here_is_never_overridden() {
        let shell_env = env(&[
            ("OPENAI_API_KEY", "from-profile"),
            ("ANTHROPIC_API_KEY", "from-profile"),
        ]);
        let applied = variables_to_apply(&shell_env, |name| name == "OPENAI_API_KEY");
        assert_eq!(
            applied,
            env(&[("ANTHROPIC_API_KEY", "from-profile")]),
            "an explicitly-passed value or a plist entry outranks the profile"
        );
    }

    #[test]
    fn an_exported_but_empty_value_is_not_applied() {
        let shell_env = env(&[("OPENAI_API_KEY", ""), ("VERTEX_REGION", "europe-west1")]);
        assert_eq!(
            variables_to_apply(&shell_env, |_| false),
            env(&[("VERTEX_REGION", "europe-west1")]),
            "an empty key would only mask the fallbacks below it"
        );
    }

    #[test]
    fn path_is_not_applied_by_the_never_override_rule() {
        let shell_env = env(&[("PATH", "/opt/homebrew/bin"), ("VERTEX_REGION", "eu")]);
        assert_eq!(
            variables_to_apply(&shell_env, |_| false),
            env(&[("VERTEX_REGION", "eu")]),
            "PATH goes through merged_path instead; see its docs"
        );
    }

    #[test]
    fn the_plan_merges_path_after_the_allowlisted_variables() {
        let shell_env = env(&[
            ("PATH", "/home/u/.nvm/versions/node/v22/bin:/usr/bin"),
            ("ANTHROPIC_API_KEY", "sk-profile"),
        ]);
        let plan = hydration_plan(&shell_env, |name| {
            (name == "PATH").then(|| OsString::from("/usr/bin:/bin"))
        });
        assert_eq!(
            plan,
            vec![
                (
                    "ANTHROPIC_API_KEY".to_string(),
                    OsString::from("sk-profile")
                ),
                (
                    "PATH".to_string(),
                    OsString::from("/home/u/.nvm/versions/node/v22/bin:/usr/bin:/bin")
                ),
            ]
        );
    }

    #[test]
    fn a_shell_with_no_path_leaves_path_out_of_the_plan() {
        let plan = hydration_plan(&env(&[("VERTEX_REGION", "eu")]), |_| {
            Some(OsString::from("/usr/bin"))
        });
        assert!(
            plan.is_empty(),
            "VERTEX_REGION is already set and no PATH was reported: {plan:?}"
        );
    }

    #[test]
    fn merged_path_puts_the_shells_dirs_first_and_keeps_the_inherited_tail() {
        let merged = merged_path(
            OsStr::new("/Users/me/.nvm/versions/node/v20/bin:/opt/homebrew/bin"),
            Some(OsStr::new("/usr/bin:/bin:/usr/sbin:/sbin")),
        );
        assert_eq!(
            std::env::split_paths(&merged).collect::<Vec<PathBuf>>(),
            vec![
                PathBuf::from("/Users/me/.nvm/versions/node/v20/bin"),
                PathBuf::from("/opt/homebrew/bin"),
                PathBuf::from("/usr/bin"),
                PathBuf::from("/bin"),
                PathBuf::from("/usr/sbin"),
                PathBuf::from("/sbin"),
            ],
            "the launchd floor must survive the merge"
        );
    }

    #[test]
    fn merged_path_dedupes_and_drops_empty_components() {
        let merged = merged_path(
            OsStr::new("/opt/homebrew/bin::/usr/bin"),
            Some(OsStr::new("/usr/bin:/bin:/opt/homebrew/bin")),
        );
        assert_eq!(
            std::env::split_paths(&merged).collect::<Vec<PathBuf>>(),
            vec![
                PathBuf::from("/opt/homebrew/bin"),
                PathBuf::from("/usr/bin"),
                PathBuf::from("/bin"),
            ],
            "a repeated dir keeps its first position, and an empty component is 'cwd'"
        );
    }

    #[test]
    fn merged_path_handles_a_missing_inherited_path() {
        let merged = merged_path(OsStr::new("/opt/homebrew/bin"), None);
        assert_eq!(
            std::env::split_paths(&merged).collect::<Vec<PathBuf>>(),
            vec![PathBuf::from("/opt/homebrew/bin")]
        );
    }

    #[test]
    fn a_real_shell_answers_with_a_marked_null_delimited_environment() {
        let stdout = read_login_shell_env(Path::new("/bin/sh"), Duration::from_secs(10))
            .expect("/bin/sh answers");
        let payload = env_after_marker(&stdout).expect("the marker is printed before the dump");
        let parsed = parse_null_delimited(payload);
        assert!(
            parsed.iter().any(|(name, _)| name == "PATH"),
            "a real environment dump has a PATH; got {} entries",
            parsed.len()
        );
    }

    #[test]
    fn a_login_shell_with_a_version_manager_on_path_hydrates_the_plan() {
        let nvm_bin = "/home/u/.nvm/versions/node/v22.3.0/bin";
        let home_dir = tempfile::tempdir().expect("temp HOME");
        let home = home_dir.path();
        std::fs::write(
            home.join(".profile"),
            format!(
                "export PATH=\"{nvm_bin}:$PATH\"\n\
                 export ANTHROPIC_API_KEY=sk-from-profile\n\
                 export OPENAI_API_KEY=sk-profile-loses\n\
                 export LUCIDOS_BIND_ALL=1\n"
            ),
        )
        .expect("write the profile");
        let mut cmd = login_shell_command(Path::new("/bin/sh"));
        cmd.env("HOME", home).env_remove("ENV");
        let stdout = read_stdout_bounded(cmd, "the test shell", Duration::from_secs(10));
        let stdout = stdout.expect("/bin/sh answers");
        let shell_env = parse_null_delimited(env_after_marker(&stdout).expect("marker"));

        let plan = hydration_plan(&shell_env, |name| match name {
            "OPENAI_API_KEY" => Some(OsString::from("sk-from-the-unit-file")),
            "PATH" => Some(OsString::from("/usr/bin:/bin:/usr/sbin:/sbin")),
            _ => None,
        });
        let value = |name: &str| plan.iter().find(|(n, _)| n == name).map(|(_, v)| v);

        assert_eq!(
            value("ANTHROPIC_API_KEY"),
            Some(&OsString::from("sk-from-profile"))
        );
        assert_eq!(
            value("OPENAI_API_KEY"),
            None,
            "a value the service already has outranks the profile"
        );
        assert!(
            plan.iter()
                .all(|(name, _)| HYDRATED_VARS.contains(&name.as_str())),
            "nothing off the allowlist may be applied: {plan:?}"
        );
        let path: Vec<PathBuf> =
            std::env::split_paths(value("PATH").expect("PATH is hydrated")).collect();
        assert!(
            path.contains(&PathBuf::from(nvm_bin)),
            "the version manager's bin dir reaches PATH: {path:?}"
        );
        for floor in ["/usr/bin", "/bin", "/usr/sbin", "/sbin"] {
            assert!(
                path.contains(&PathBuf::from(floor)),
                "{floor} survives: {path:?}"
            );
        }
    }

    fn a_child_that_hangs() -> Command {
        let mut cmd = Command::new("/bin/sleep");
        cmd.arg("30").stdin(Stdio::null()).stdout(Stdio::piped());
        cmd
    }

    fn a_shell_running(script: &str) -> Command {
        let mut cmd = Command::new("/bin/sh");
        cmd.arg("-c")
            .arg(script)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        cmd
    }

    #[test]
    fn a_hung_child_is_killed_at_the_deadline_and_reported() {
        let started = Instant::now();
        let err = read_stdout_bounded(
            a_child_that_hangs(),
            "the test shell",
            Duration::from_millis(200),
        )
        .expect_err("a shell that never answers must not be waited on forever");
        assert!(
            err.contains("did not answer"),
            "the failure must say what happened: {err}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the deadline is the point; took {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn the_timeout_takes_the_shells_descendants_with_it() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("descendant");
        let script = format!("( sleep 0.5; touch '{}' ) & sleep 30", marker.display());
        let started = Instant::now();
        read_stdout_bounded(
            a_shell_running(&script),
            "the test shell",
            Duration::from_millis(100),
        )
        .expect_err("the shell never answers");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the deadline still has to hold with a descendant on the pipe; took {:?}",
            started.elapsed()
        );

        std::thread::sleep(Duration::from_millis(1200));
        assert!(
            !marker.exists(),
            "the backgrounded descendant outlived the process-group kill"
        );
    }

    #[test]
    fn a_descendant_holding_the_pipe_after_the_shell_exits_cannot_outlast_the_deadline() {
        let started = Instant::now();
        let err = read_stdout_bounded(
            a_shell_running("printf hello; sleep 30 & exit 0"),
            "the test shell",
            SHELL_TIMEOUT,
        )
        .expect_err("EOF never comes while the descendant lives");
        assert!(
            err.contains("held its output open"),
            "the failure must name the actual cause: {err}"
        );
        assert!(
            started.elapsed() < SHELL_TIMEOUT,
            "an exited shell with a live descendant must not stall the launch; took {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn a_shell_that_is_not_there_is_an_error_not_a_panic() {
        let err = read_login_shell_env(Path::new("/nonexistent/shell"), Duration::from_millis(200))
            .expect_err("a missing shell cannot be run");
        assert!(err.contains("could not run"), "unexpected message: {err}");
    }
}
