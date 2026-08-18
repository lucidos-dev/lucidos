//! Hydrate the user's login-shell environment for a GUI launch (macOS).
//!
//! A process started by launchd or by LaunchServices inherits launchd's
//! environment, not the user's. `~/.zprofile` and `~/.zshrc` never ran, so
//! nothing exported there exists, and PATH is the bare
//! `/usr/bin:/bin:/usr/sbin:/sbin`. That breaks two things:
//!
//!  * **Provider discovery.** The engine and the coding agents it spawns
//!    resolve their credentials from environment variables. Keys in a shell
//!    profile hit the no-provider wall in the packaged app.
//!  * **Tool resolution.** `claude`, `codex`, `git` and `npx` installed via
//!    Homebrew, nvm, asdf or mise are all off launchd's PATH.
//!
//! **Where this runs, and why it is not the client.** In a packaged build the
//! engine is not a descendant of the GUI client. Every link from
//! [`crate::run_service`] down to the coding agents inherits its parent's
//! environment, so hydrating the service reaches all of them.
//!
//! The engine's `core::user_path` is a floor for the common install dirs. This
//! reads the PATH the user actually has, and the two compose.

use std::ffi::{OsStr, OsString};
use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// The variables carried over from the login shell, and the ONLY ones.
///
/// An allowlist rather than a wholesale copy, on purpose. This process is the
/// root of the packaged stack: the gateway, every workspace engine and every
/// coding agent inherit whatever lands here. Copying wholesale would let a
/// developer's `~/.zshrc` silently redefine the packaged app's topology.
///
/// So membership is narrow. A variable belongs here when it decides whether a
/// provider or a tool can be REACHED AT ALL. It must also be absent only
/// because the profile never ran. Credentials, the paths they live at, and PATH.
///
/// Deliberately absent:
///
///  * **Behavior selectors** such as `LUCIDOS_MODEL` or `HF_HOME`. They pick
///    behavior, not reachability, and they have a Settings UI.
///  * **Proxy variables.** Needed behind a corporate proxy, but a hydrated
///    proxy also sits in front of the gateway-to-engine loopback hops.
///
/// PATH is in the list but is NOT applied by the same rule: see [`merged_path`].
const HYDRATED_VARS: &[&str] = &[
    // Tool resolution for everything spawned below this process.
    "PATH",
    // Provider credentials the engine reads (`llm/`) and the ones the coding
    // agents read for themselves.
    "OPENAI_API_KEY",
    "ANTHROPIC_API_KEY",
    "LUCIDOS_OPENROUTER_API_KEY",
    "LUCIDOS_XAI_API_KEY",
    "LUCIDOS_LOCAL_BASE_URL",
    "LUCIDOS_LOCAL_API_KEY",
    "VERTEX_PROJECT_ID",
    "VERTEX_REGION",
    // Where a credential lives, rather than the credential itself: the gcloud
    // ADC file the Vertex provider reads, and the config dirs holding the two
    // coding agents' own logins.
    "GOOGLE_APPLICATION_CREDENTIALS",
    "CLOUDSDK_CONFIG",
    "CODEX_HOME",
    "CLAUDE_CONFIG_DIR",
];

/// Fallback login shell when `$SHELL` says nothing usable. macOS has defaulted
/// to zsh since Catalina.
const DEFAULT_SHELL: &str = "/bin/zsh";

/// Printed by the login shell immediately before the environment dump, so a
/// profile that greets the user cannot be mistaken for the payload. Everything
/// up to and including the first occurrence is discarded. See
/// [`env_after_marker`].
const MARKER: &str = "__LUCIDOS_SHELL_ENV__";

/// How long the login shell gets to answer.
///
/// A profile can hang outright: a slow nvm or mise init, a network call, a
/// prompt framework waiting on something. This runs before the gateway starts,
/// so a hung shell must cost a bounded delay and nothing else. Five seconds is
/// generous for a real profile and invisible against the gateway-health budget.
/// A shell that cannot answer within it is hung rather than slow.
const SHELL_TIMEOUT: Duration = Duration::from_secs(5);

/// How long the shell's output may still be arriving AFTER the shell itself has
/// exited. Separate from [`SHELL_TIMEOUT`] on purpose: see the call site in
/// [`read_stdout_bounded`].
const DRAIN_GRACE: Duration = Duration::from_millis(250);

/// Resolve the login shell's environment and apply the allowlisted parts to
/// THIS process, so everything below it inherits them. Every failure is logged
/// and swallowed, since no shell problem may keep the app from starting.
///
/// # Safety contract
///
/// Calls `std::env::set_var`, so it must run while the process is still
/// single-threaded. Two things establish that, and both are load-bearing:
///
///  * The one call site is the first statement of `desktop::run_service`, which
///    `main` reaches before any Tauri, AppKit or thread setup.
///  * [`read_stdout_bounded`], which this calls first, does spawn a reader
///    thread. It hands back output only where it has joined that thread. Every
///    path that leaves the thread running returns `Err`, which returns from
///    here before any `set_var`.
///
/// Anything added between them has to preserve that, which is why it is spelled
/// out rather than left as "it is early in `main`".
pub fn hydrate_login_shell_env() {
    if !needs_hydration(std::env::var_os("SHLVL")) {
        eprintln!("[shell-env] started from a shell; keeping the inherited environment");
        return;
    }
    let shell = login_shell(std::env::var_os("SHELL"));
    let stdout = match read_login_shell_env(&shell, SHELL_TIMEOUT) {
        Ok(stdout) => stdout,
        Err(e) => {
            eprintln!("[shell-env] {e}; continuing without the login-shell environment");
            return;
        }
    };
    let Some(payload) = env_after_marker(&stdout) else {
        eprintln!(
            "[shell-env] {} produced no environment dump; continuing without it",
            shell.display()
        );
        return;
    };

    let shell_env = parse_null_delimited(payload);
    let to_apply = variables_to_apply(&shell_env, |name| std::env::var_os(name).is_some());

    let mut applied: Vec<&str> = Vec::new();
    for (name, value) in &to_apply {
        // SAFETY: single-threaded at this point. See the safety contract above.
        unsafe {
            std::env::set_var(name, value);
        }
        applied.push(name);
    }

    // PATH is the documented exception to the never-override rule: launchd
    // ALWAYS sets a minimal one, so leaving it to `variables_to_apply` would
    // mean it never hydrates at all. Merged rather than replaced so no
    // inherited directory is lost. See `merged_path`.
    if let Some((_, shell_path)) = shell_env.iter().find(|(name, _)| name == "PATH") {
        let merged = merged_path(OsStr::new(shell_path), std::env::var_os("PATH").as_deref());
        // SAFETY: as above.
        unsafe {
            std::env::set_var("PATH", &merged);
        }
        applied.push("PATH");
    }

    if applied.is_empty() {
        eprintln!(
            "[shell-env] {} contributed nothing new; continuing with the inherited environment",
            shell.display()
        );
    } else {
        // NAMES only. Several of these are API keys, and this line lands in the
        // service log under `<app-data>/logs/`.
        eprintln!(
            "[shell-env] hydrated from {}: {}",
            shell.display(),
            applied.join(", ")
        );
    }
}

/// Does this process need its environment hydrated from a login shell?
///
/// The signal is the absence of `SHLVL`: every POSIX shell sets and exports it,
/// and launchd sets nothing of the kind. A present `SHLVL` therefore means the
/// environment already came through a shell and must be left exactly as it is.
///
/// Both ways of being wrong are cheap. Treating a terminal run as a GUI launch
/// costs one shell spawn whose result overrides nothing. Treating a GUI launch
/// as a terminal run leaves the un-hydrated behaviour. Pure over its input, so
/// the decision is testable without touching process env.
fn needs_hydration(shlvl: Option<OsString>) -> bool {
    shlvl.is_none()
}

/// The user's login shell: `$SHELL` when it says something usable, else
/// [`DEFAULT_SHELL`]. Blank and whitespace-only values are treated as unset,
/// since a launchd environment can carry an empty one.
fn login_shell(shell: Option<OsString>) -> PathBuf {
    shell
        .filter(|s| !s.to_string_lossy().trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_SHELL))
}

/// The command that asks a login shell for its environment.
///
/// `-i` as well as `-l`: `~/.zshrc` and `~/.bashrc` are sourced only for
/// INTERACTIVE shells, which is where most people put their exports. A
/// login-only shell would miss the common case entirely.
///
/// `env -0` rather than a bare `env`, because environment values legitimately
/// contain newlines. Splitting on them would corrupt every multi-line value and
/// invent junk entries from the pieces. `/usr/bin/env` by absolute path, so a
/// profile's `env` function or alias cannot stand in for it.
///
/// stdin and stderr are `/dev/null`. stdin because a profile that reads from it
/// would block against a pipe nobody writes to. stderr because its content is
/// the user's profile noise, which we must not risk logging.
fn login_shell_command(shell: &Path) -> Command {
    let mut cmd = Command::new(shell);
    cmd.arg("-ilc")
        .arg(format!("printf '%s' '{MARKER}'; /usr/bin/env -0"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    cmd
}

/// Run the login shell and return its raw stdout, or an error message naming
/// what went wrong.
fn read_login_shell_env(shell: &Path, timeout: Duration) -> Result<Vec<u8>, String> {
    read_stdout_bounded(
        login_shell_command(shell),
        &shell.display().to_string(),
        timeout,
    )
}

/// SIGKILL an entire process group. `pid` must be the group leader, which
/// `process_group(0)` in [`read_stdout_bounded`] guarantees for the child it
/// spawns.
///
/// Signalling the GROUP rather than the one process is what makes the deadline
/// real: see the pipe-lifetime paragraph on [`read_stdout_bounded`].
fn kill_process_group(pid: u32) {
    // SAFETY: a negative pid is POSIX for "every process in group |pid|". The
    // group is the one we created for our own child, so nothing outside this
    // subtree can be in it. An already-dead group is a harmless ESRCH.
    unsafe {
        libc::kill(-(pid as i32), libc::SIGKILL);
    }
}

/// Run `cmd` to completion and return its stdout, killing it if it overruns
/// `timeout`. Split out from [`read_login_shell_env`] so the bound can be
/// tested against a child that really hangs.
///
/// **stdout is drained on a thread WHILE the child runs**, not after it exits.
/// A full environment can exceed the 64 KiB pipe buffer, and a child blocked
/// writing into a pipe nobody reads never exits. That is why `mobile.rs`'s
/// `output_with_timeout` is not reused here: it waits for exit first.
///
/// **Nothing waits on that thread without a deadline, and the child gets its
/// own process group.** Both exist for the same subtle reason. A pipe reaches
/// EOF only when the LAST write end closes, and every descendant the shell
/// spawned inherited one. So a profile that backgrounds anything leaves the
/// read blocked long after the shell is gone, and a plain `join()` would sail
/// past the deadline. `process_group(0)` lets the timeout take the whole tree
/// down, and the result comes back over a channel.
///
/// `label` names the child in every error message. It must never be built from
/// anything the environment dump contained.
fn read_stdout_bounded(
    mut cmd: Command,
    label: &str,
    timeout: Duration,
) -> Result<Vec<u8>, String> {
    let deadline = Instant::now() + timeout;
    // Make the child its own process-group leader, so `kill_process_group` can
    // reach whatever it spawned.
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
        // The receiver is gone on the timeout path; that send failing is the
        // normal way this thread ends there.
        let _ = done.send(buf);
    });

    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() >= deadline => {
                // The group kill already covers the child, since it leads the
                // group. Signalling it directly as well is what makes the
                // `wait` below unconditionally safe: `wait` blocks until the
                // child is gone, so it must never be reachable with the child
                // still alive, whatever the group call did.
                let _ = child.kill();
                kill_process_group(pid);
                let _ = child.wait();
                // `{:?}` rather than `as_secs()`, which truncates a sub-second
                // budget to "within 0s". That reads as a bug in the caller
                // rather than a hung shell.
                return Err(format!(
                    "{label} did not answer within {timeout:?} and was stopped"
                ));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => return Err(format!("could not wait for {label}: {e}")),
        }
    }

    // The shell exited, so its output is already written and the reader has
    // normally finished draining it before we get here. It still gets a bound,
    // for the case where a background descendant is holding the pipe open: we
    // would rather launch un-hydrated than wait on a daemon the user's profile
    // started. Deliberately NOT a group kill here, unlike the path above: the
    // shell answered, so anything still running is something the profile meant
    // to leave running.
    //
    // [`DRAIN_GRACE`] rather than what is left of `deadline`, because the child
    // can exit in the last poll tick and leave nothing at all. Draining a pipe
    // whose writer has exited is a memcpy, so the grace never makes a launch
    // slow. A zero budget would blame a descendant for a shell that answered.
    match output.recv_timeout(DRAIN_GRACE) {
        // Its blocking read is done and it has already sent, so this join only
        // waits for the thread to return and exit. That matters beyond
        // tidiness: `hydrate_login_shell_env` calls `std::env::set_var` on what
        // we return, which is sound only while the process is single-threaded,
        // so this is where that promise is actually kept.
        Ok(buf) => {
            let _ = reader.join();
            Ok(buf)
        }
        // Abandoned rather than joined. The thread is parked on a read a live
        // descendant can hold open indefinitely. Joining it is exactly the
        // unbounded wait this function exists to prevent. It costs one parked
        // thread and its buffer, once per service start. No `set_var` follows
        // an `Err`, so the single-threaded promise above still holds.
        Err(_) => Err(format!(
            "{label} answered but something it started still held its output open"
        )),
    }
}

/// The environment payload: everything after the FIRST [`MARKER`], or `None`
/// when the marker never appeared (a shell that failed to start, a profile that
/// exited early).
///
/// First occurrence rather than last. Only profile chatter is guaranteed to
/// precede the real marker, while a marker-shaped string inside a later value
/// is not something to guess about. A profile that printed the marker itself
/// leaves noise in the payload, which parses into entries with no `=`.
fn env_after_marker(stdout: &[u8]) -> Option<&[u8]> {
    let marker = MARKER.as_bytes();
    let start = stdout
        .windows(marker.len())
        .position(|w| w == marker)?
        .checked_add(marker.len())?;
    Some(&stdout[start..])
}

/// Parse `env -0` output into `(name, value)` pairs.
///
/// Splitting on NUL is what makes this safe. A value containing a NEWLINE
/// survives whole, and so does one containing `=`, since the name is taken up
/// to the FIRST `=` only. Entries that are not valid UTF-8 are dropped rather
/// than lossily mangled, as are entries with no `=` or an empty name.
///
/// Only NUL-TERMINATED entries count. `env -0` terminates every entry it emits,
/// including the last, so an unterminated tail means the dump was cut short.
/// Dropping it stops half an API key being applied as if it were whole. That
/// would fail authentication in a way that looks nothing like a truncated read.
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

/// Which of the shell's variables to apply, given what is already set here.
///
/// Two filters, in this order:
///
///  * **The allowlist** ([`HYDRATED_VARS`]), so nothing else in the user's shell
///    can reach the packaged stack.
///  * **Never override.** A name already present in this process wins. An
///    explicitly passed variable, a plist entry or a Lucidos-managed value is
///    a deliberate decision, which a shell profile may not quietly outrank.
///
/// PATH is excluded here and handled by [`merged_path`] instead: launchd always
/// sets one, so the never-override rule alone would mean PATH never hydrates.
/// Empty values are dropped, since an exported-but-empty key is not a
/// credential and would only mask the fallbacks below it.
///
/// `already_set` is injected so the rule is testable without mutating real
/// process env.
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

/// Merge the login shell's PATH over the inherited one: the shell's directories
/// first, then any inherited directory not already among them, in order.
///
/// PATH is the one allowlisted variable that overrides what the process already
/// has, and it has to be. launchd hands every packaged process a minimal PATH,
/// so it is never absent and a strict never-override rule would skip it.
///
/// Merged rather than replaced so the launchd floor cannot be lost: a login
/// shell that somehow omits `/usr/bin` still leaves the engine able to find
/// `git`. Duplicates keep their first position, and empty components are
/// dropped because an empty PATH element means the current directory. Same
/// shape as the engine's `core::user_path::augmented_user_path`, which this
/// crate cannot reuse because it links neither the engine nor the gateway.
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

    // ── When hydration runs at all ──────────────────────────────────────────

    #[test]
    fn a_launchd_launch_has_no_shlvl_and_is_hydrated() {
        assert!(
            needs_hydration(None),
            "no SHLVL means launchd/LaunchServices started us and the profile never ran"
        );
    }

    #[test]
    fn a_shell_started_run_is_left_alone() {
        assert!(
            !needs_hydration(Some(OsString::from("1"))),
            "SHLVL present means a shell already gave us its environment"
        );
        assert!(!needs_hydration(Some(OsString::from("3"))));
    }

    // ── Which shell we ask ──────────────────────────────────────────────────

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
    fn login_shell_falls_back_to_zsh_when_shell_says_nothing() {
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

    // ── Finding the payload ─────────────────────────────────────────────────

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

    // ── Parsing the payload ─────────────────────────────────────────────────

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
        // No '=', an empty name, an empty trailing chunk, and invalid UTF-8.
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

    // ── Choosing what to apply ──────────────────────────────────────────────

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

    // ── PATH ────────────────────────────────────────────────────────────────

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

    // ── The bounded read ────────────────────────────────────────────────────

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

    /// A child that never returns, standing in for a wedged `.zshrc`.
    fn a_child_that_hangs() -> Command {
        let mut cmd = Command::new("/bin/sleep");
        cmd.arg("30").stdin(Stdio::null()).stdout(Stdio::piped());
        cmd
    }

    /// A `/bin/sh` running `script`, wired up the way the real login shell is.
    fn a_shell_running(script: &str) -> Command {
        let mut cmd = Command::new("/bin/sh");
        cmd.arg("-c")
            .arg(script)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        cmd
    }

    /// A path in the temp dir that no other test uses, and no leftover file.
    fn a_fresh_temp_path(name: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("lucidos-shell-env-{name}-{}", std::process::id()));
        let _ = std::fs::remove_file(&path);
        path
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
        // A profile that backgrounds something and then hangs. The background
        // child would touch the marker file half a second in, so its absence
        // afterwards proves the whole process GROUP was killed. Killing only
        // the shell would leave that child holding the stdout pipe.
        let marker = a_fresh_temp_path("descendant");
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
        // The shell answers and exits immediately, but something it started in
        // the background still holds the write end, so the pipe never reaches
        // EOF. Reading to EOF is then unbounded, and the launch must not be.
        //
        // The timeout here is deliberately GENEROUS, and is not what is under
        // test. The subject is the drain grace that runs after the shell exits,
        // which is only reached when the shell exits before the deadline. A
        // tight deadline races the shell's own startup instead. On a loaded
        // machine the deadline arm then wins, and the assertion fails against
        // "did not answer" rather than the descendant. `SHELL_TIMEOUT` is the
        // real budget this call gets in production, so use exactly that.
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
        // The whole point: bounded by the drain grace, NOT by the shell timeout
        // the descendant would otherwise run out. Comfortably under
        // `SHELL_TIMEOUT` so the assertion distinguishes the two paths.
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
