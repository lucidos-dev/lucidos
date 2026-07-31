//! The interpreter every engine-spawned shell command runs under, and the
//! typed outcome of a reaped child.
//!
//! ## Why this module exists: the pipeline-masking trap
//!
//! A POSIX shell reports the exit status of the **last** stage of a pipeline.
//! So `/bin/sh -c "cargo clippy … 2>&1 | tee build.log"` reports `tee`'s `0`
//! even when clippy exited `101`, and every downstream surface — the structured
//! `exit_code`, the `BackgroundBashCompleted` payload, and the completion
//! summary the LLM reads — faithfully propagates that `0`. A broken build is
//! then indistinguishable from a passing one.
//!
//! That is not hypothetical: the 2026-07-26 nightly hit it four times in one
//! pipeline (clippy `101`, e2e `101` and `1`), and every step had to write the
//! real status into a sidecar `.ec` file and cross-check it against the summary.
//!
//! `set -o pipefail` fixes exactly that: **a failing stage is never masked by a
//! later succeeding one.** Be precise about what it does and does not promise —
//! the pipeline's status becomes the status of the *rightmost stage that
//! failed*, and `0` only when every stage succeeded. It is NOT the *first*
//! failing stage: `sh -c 'exit 42' | sh -c 'exit 7'` reports `7`, not `42`. For
//! the shape that matters here — one real command piped into reporting stages
//! that succeed (`… | tee log`) — the real command is the only failing stage, so
//! its status is what surfaces. A caller that needs to attribute a failure in a
//! pipeline with several fallible stages still has to split them up.
//!
//! ## Why `bash` and not a `set -o pipefail;` prefix
//!
//! `pipefail` is not POSIX. macOS `/bin/sh` is bash-3.2 in sh-mode and supports
//! it, but `/bin/sh` on Linux is often dash, which only gained `pipefail` in
//! 0.5.12 (2023) — Ubuntu 22.04 still ships 0.5.11. Injecting
//! `set -o pipefail;` into the command string on such a shell prints an error
//! line to stderr and then *continues without pipefail*: the masking survives,
//! silently, plus every command's stderr gains noise. Resolving a real `bash`
//! once and passing `-o pipefail` as an argument is deterministic — either the
//! guarantee holds, or we fall back to `/bin/sh` and say so in the log.
//!
//! ## Accepted trade-off
//!
//! With `pipefail`, a producer killed by SIGPIPE surfaces the failure:
//! `yes | head -1` reports `141` instead of `0`. That is fail-loud where the
//! old behaviour was fail-silent.
//!
//! Note carefully *how* it surfaces. The shell is our direct child, so only the
//! shell's own death yields [`TaskOutcome::Signaled`]. A signal that kills a
//! stage *inside* its pipeline never reaches us as a signal at all — the shell
//! exits **normally** carrying `128 + signum`. So `yes | head -1` gives
//! `Exited(141)`, not `Signaled(13)`. [`TaskOutcome::describe`] decodes that
//! range (`exit code 141 (probable SIGPIPE)`) so the number is still readable
//! as what it is.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Candidate `bash` locations, in resolution order. Covers macOS (`/bin/bash`,
/// always present) and both merged-`/usr` and split-`/usr` Linux layouts. A
/// system with none of these falls back to `/bin/sh` without `pipefail`.
const BASH_CANDIDATES: [&str; 2] = ["/bin/bash", "/usr/bin/bash"];

/// POSIX shell used when no `bash` is resolvable. Also the historical
/// interpreter for all three call sites, so the fallback is exactly the old
/// behaviour.
const POSIX_SH: &str = "/bin/sh";

/// The resolved interpreter for engine-spawned shell commands.
///
/// Resolved once per process by [`command_shell`] — the resolution touches the
/// filesystem and logs, and neither should happen per spawn.
pub struct CommandShell {
    program: PathBuf,
    /// True when `program` is a `bash` we can pass `-o pipefail` to. False only
    /// on the degraded `/bin/sh` fallback, where a pipeline's failing stage is
    /// still masked — the condition the log warning names.
    pipefail: bool,
}

impl CommandShell {
    fn resolve() -> Self {
        for candidate in BASH_CANDIDATES {
            if Path::new(candidate).is_file() {
                return CommandShell {
                    program: PathBuf::from(candidate),
                    pipefail: true,
                };
            }
        }
        log!(
            "[Shell] no bash found in {:?} — falling back to {} WITHOUT pipefail. \
             A failing stage of a pipeline (e.g. `cmd | tee log`) will be masked \
             by the last stage's exit status.",
            BASH_CANDIDATES,
            POSIX_SH
        );
        CommandShell {
            program: PathBuf::from(POSIX_SH),
            pipefail: false,
        }
    }

    /// True when the resolved shell applies `pipefail`. Read by tests and by
    /// callers that want to describe the guarantee; the spawn helpers apply it
    /// themselves.
    pub fn has_pipefail(&self) -> bool {
        self.pipefail
    }

    /// Interpreter path, for docs and diagnostics.
    pub fn program(&self) -> &Path {
        &self.program
    }

    /// The interpreter with its options applied and no target yet. Both spawn
    /// helpers start here so the `pipefail` flag can only ever be applied in
    /// one place.
    fn base(&self) -> tokio::process::Command {
        let mut cmd = tokio::process::Command::new(&self.program);
        if self.pipefail {
            cmd.args(["-o", "pipefail"]);
        }
        cmd
    }

    /// `<shell> [-o pipefail] -c <command>` — the spawn used by `run_bash`,
    /// `run_bash_background`, and (via a generated `python <script>` string)
    /// `run_python_background`.
    pub fn command(&self, command: &str) -> tokio::process::Command {
        let mut cmd = self.base();
        cmd.arg("-c").arg(command);
        cmd
    }

    /// `<shell> [-o pipefail] <script>` — the spawn used for scheduled/trigger
    /// script files. Same masking applies to a pipeline inside the script, so
    /// it gets the same guarantee.
    pub fn script(&self, script_path: &Path) -> tokio::process::Command {
        let mut cmd = self.base();
        cmd.arg(script_path);
        cmd
    }
}

/// Process-lifetime resolved shell. The filesystem probe and the fallback
/// warning happen exactly once.
pub fn command_shell() -> &'static CommandShell {
    static SHELL: OnceLock<CommandShell> = OnceLock::new();
    SHELL.get_or_init(CommandShell::resolve)
}

/// How a reaped child process ended.
///
/// Replaces the bare `Option<i32>` exit code that used to be threaded through
/// the background-task machinery. That type could not distinguish four
/// different things — "exited 0", "exited non-zero", "died on a signal", and
/// "the engine never learned the status" — so two of them collapsed onto a
/// number the LLM reads as a normal exit. Making the states distinct is what
/// lets every surface refuse to invent a `0`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskOutcome {
    /// Child exited normally with this status code.
    Exited(i32),
    /// Child was terminated by this Unix signal (9 = SIGKILL, 13 = SIGPIPE, …).
    /// Carries no exit code — `code()` is `None` for a signalled child.
    Signaled(i32),
    /// `wait()` failed, or the platform reported neither a code nor a signal.
    /// The engine genuinely does not know how the child ended, and says so —
    /// this is never rendered as a number.
    Unknown,
}

impl TaskOutcome {
    /// Classify a reaped child's status.
    ///
    /// The `signal()` check must come first: on Unix a signalled child has
    /// `code() == None`, so keying off `code()` alone collapses a signal death
    /// into the same `None` as a failed `wait()`.
    pub fn from_status(status: std::process::ExitStatus) -> Self {
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            if let Some(sig) = status.signal() {
                return TaskOutcome::Signaled(sig);
            }
        }
        match status.code() {
            Some(code) => TaskOutcome::Exited(code),
            None => TaskOutcome::Unknown,
        }
    }

    /// Classify the result of awaiting a child. A `wait()` error means the
    /// status is genuinely unavailable — [`TaskOutcome::Unknown`], never a
    /// synthesized code.
    pub fn from_wait(result: std::io::Result<std::process::ExitStatus>) -> Self {
        match result {
            Ok(status) => Self::from_status(status),
            Err(_) => TaskOutcome::Unknown,
        }
    }

    /// Rebuild an outcome from the persisted `(exit_code, signal)` pair on a
    /// `BackgroundBashCompleted` payload, so the event-store fallback in
    /// `bash_output` renders exactly what the in-memory drain rendered.
    ///
    /// Rows written before `signal` existed carry `signal: None`; a legacy row
    /// whose `exit_code` is also null (watchdog timeout / `bash_kill`) becomes
    /// `Unknown`, which is the honest reading of "we recorded no status".
    pub fn from_persisted(exit_code: Option<i32>, signal: Option<i32>) -> Self {
        match (signal, exit_code) {
            (Some(sig), _) => TaskOutcome::Signaled(sig),
            (None, Some(code)) => TaskOutcome::Exited(code),
            (None, None) => TaskOutcome::Unknown,
        }
    }

    /// Structured exit code — `Some` **only** for a normal exit. A signal death
    /// and an unknown status are both `None`: never `0`, never a synthesized
    /// `-1`, never `128 + signum`.
    pub fn exit_code(&self) -> Option<i32> {
        match self {
            TaskOutcome::Exited(code) => Some(*code),
            TaskOutcome::Signaled(_) | TaskOutcome::Unknown => None,
        }
    }

    /// Structured signal number — `Some` only for a signal death.
    pub fn signal(&self) -> Option<i32> {
        match self {
            TaskOutcome::Signaled(sig) => Some(*sig),
            TaskOutcome::Exited(_) | TaskOutcome::Unknown => None,
        }
    }

    /// True only for a clean `exit 0`. A signal death and an unknown status are
    /// **not** success — the whole point of the type.
    pub fn is_success(&self) -> bool {
        matches!(self, TaskOutcome::Exited(0))
    }

    /// The human-readable phrase every LLM-facing surface uses. One
    /// implementation so the summary line, the sync `run_bash` result, and the
    /// `bash_output` JSON can never disagree about what happened.
    ///
    /// Exit codes in `129..=159` get a `(probable SIGNAME)` hint. That range is
    /// the `128 + signum` convention, and it is not a curiosity here — it is how
    /// a signal *inside* the command reaches us. The shell is our direct child,
    /// so only the shell's own death produces [`TaskOutcome::Signaled`]; when a
    /// stage of its pipeline is signalled the shell exits **normally** carrying
    /// `128 + signum`. Enabling `pipefail` made that common: `yes | head -1` now
    /// surfaces as `Exited(141)`, and without the hint "exit code 141" is
    /// exactly the bare number-that-reads-like-a-normal-exit this type exists to
    /// eliminate. "probable", not certain, because a program may legitimately
    /// choose such a code — same wording and range as
    /// `runtime::claude_code::format_exit_status`.
    pub fn describe(&self) -> String {
        match self {
            TaskOutcome::Exited(code) => match probable_signal(*code) {
                Some(name) => format!("exit code {} (probable {})", code, name),
                None => format!("exit code {}", code),
            },
            TaskOutcome::Signaled(sig) => match signal_name(*sig) {
                Some(name) => format!("killed by {} (signal {})", name, sig),
                None => format!("killed by signal {}", sig),
            },
            TaskOutcome::Unknown => "exit code unknown".to_string(),
        }
    }
}

/// Name for the signals worth spelling out. Anything unmapped falls through to
/// a bare number in [`TaskOutcome::describe`], so the phrase never silently
/// drops information.
///
/// Shared with `runtime::claude_code::format_exit_status`, which renders the
/// same names for coding-agent subprocess deaths — one table so the two
/// renderings can't drift apart.
///
/// `SIGPIPE` earns its place because enabling `pipefail` makes it user-visible:
/// a producer whose consumer closes the pipe surfaces as `141`, and naming it
/// is the difference between an actionable report and a mystery number.
/// Decode an exit code in the `128 + signum` range to the signal it most likely
/// stands for. `None` outside `129..=159` or for an unmapped signal, so an
/// ordinary status is never dressed up as a signal.
fn probable_signal(code: i32) -> Option<&'static str> {
    if (129..=159).contains(&code) {
        signal_name(code - 128)
    } else {
        None
    }
}

pub(crate) fn signal_name(sig: i32) -> Option<&'static str> {
    match sig {
        1 => Some("SIGHUP"),
        2 => Some("SIGINT"),
        3 => Some("SIGQUIT"),
        6 => Some("SIGABRT"),
        9 => Some("SIGKILL"),
        11 => Some("SIGSEGV"),
        13 => Some("SIGPIPE"),
        15 => Some("SIGTERM"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolved_shell_is_reused_and_self_consistent() {
        let a = command_shell();
        let b = command_shell();
        assert!(std::ptr::eq(a, b), "shell must resolve once per process");
        assert!(a.program().is_file(), "resolved shell must exist on disk");
        // pipefail is on iff we landed on a bash candidate; the fallback is the
        // POSIX shell. Asserting the implication (not the platform) keeps this
        // honest on a hypothetical bash-less box.
        if a.has_pipefail() {
            assert!(BASH_CANDIDATES.contains(&a.program().to_str().unwrap()));
        } else {
            assert_eq!(a.program(), Path::new(POSIX_SH));
        }
    }

    /// The whole reason the module exists: a failing stage must not be masked
    /// by the succeeding stages after it. Without pipefail this reports `0` —
    /// the exact masking the nightly hit four times.
    #[tokio::test]
    async fn command_does_not_mask_a_failing_stage_behind_later_successes() {
        if !command_shell().has_pipefail() {
            // No bash on this box — the guarantee genuinely doesn't hold and
            // the resolver already logged why. Don't assert a lie.
            return;
        }
        let out = command_shell()
            .command("sh -c 'echo lints; exit 101' 2>&1 | tee /dev/null | cat")
            .output()
            .await
            .expect("spawn");
        assert_eq!(
            TaskOutcome::from_status(out.status),
            TaskOutcome::Exited(101),
            "pipeline must report clippy's 101, not tee/cat's 0"
        );
    }

    /// Pin what `pipefail` actually promises, so no doc or tool description
    /// drifts back into claiming "first failing stage". Bash reports the
    /// RIGHTMOST non-zero status; with two failing stages the later one wins.
    /// Knowing this matters: an agent debugging a multi-fallible pipeline must
    /// not assume the reported code came from the leftmost failure.
    #[tokio::test]
    async fn pipefail_reports_the_rightmost_failing_stage_not_the_first() {
        if !command_shell().has_pipefail() {
            return;
        }
        let out = command_shell()
            .command("sh -c 'exit 42' | sh -c 'exit 7' | cat")
            .output()
            .await
            .expect("spawn");
        assert_eq!(
            TaskOutcome::from_status(out.status),
            TaskOutcome::Exited(7),
            "pipefail reports the rightmost non-zero stage (7), not the first (42)"
        );
    }

    #[tokio::test]
    async fn command_still_reports_plain_exit_codes() {
        let out = command_shell()
            .command("echo hi; exit 3")
            .output()
            .await
            .expect("spawn");
        assert_eq!(TaskOutcome::from_status(out.status), TaskOutcome::Exited(3));
        assert!(String::from_utf8_lossy(&out.stdout).contains("hi"));
    }

    #[tokio::test]
    async fn script_runs_a_file_and_applies_pipefail() {
        let dir = std::env::temp_dir().join("lucidos_test_shell_script_pipefail");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("piped.sh");
        std::fs::write(&script, "sh -c 'exit 101' | tee /dev/null\n").unwrap();

        let out = command_shell()
            .script(&script)
            .output()
            .await
            .expect("spawn");
        let expected = if command_shell().has_pipefail() {
            TaskOutcome::Exited(101)
        } else {
            TaskOutcome::Exited(0)
        };
        assert_eq!(TaskOutcome::from_status(out.status), expected);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A signal that kills a pipeline stage reaches us as the shell's own
    /// `128 + signum` EXIT code, not as a signal — so `describe` must decode it.
    /// Undecoded, "exit code 141" is precisely the bare-number-that-reads-like-
    /// a-normal-exit this type exists to eliminate.
    #[test]
    fn exit_code_in_the_128_plus_signum_range_is_decoded() {
        assert_eq!(
            TaskOutcome::Exited(141).describe(),
            "exit code 141 (probable SIGPIPE)"
        );
        assert_eq!(
            TaskOutcome::Exited(137).describe(),
            "exit code 137 (probable SIGKILL)"
        );
        assert_eq!(
            TaskOutcome::Exited(143).describe(),
            "exit code 143 (probable SIGTERM)"
        );
        // Still a normal exit as far as the structured field is concerned — the
        // hint is a reading aid, not a reclassification.
        assert_eq!(TaskOutcome::Exited(141).exit_code(), Some(141));
        assert_eq!(TaskOutcome::Exited(141).signal(), None);
    }

    /// Ordinary statuses must NOT be dressed up as signals — including the
    /// boundary values and the in-range codes we have no name for.
    #[test]
    fn ordinary_exit_codes_get_no_signal_hint() {
        for code in [0, 1, 2, 101, 128, 160, 255] {
            let phrase = TaskOutcome::Exited(code).describe();
            assert_eq!(phrase, format!("exit code {code}"), "code {code}");
        }
        // 129+16 = SIGURG on Linux, unmapped in our tiny table → no hint
        // rather than a wrong guess.
        assert_eq!(TaskOutcome::Exited(144).describe(), "exit code 144");
    }

    /// The engine's own `yes | head` case, end to end through the real shell:
    /// bash exits 141 normally, and the phrase makes that readable.
    #[tokio::test]
    async fn sigpipe_in_a_pipeline_surfaces_as_a_decoded_exit_code() {
        if !command_shell().has_pipefail() {
            return;
        }
        let out = command_shell()
            .command("yes | head -1 >/dev/null")
            .output()
            .await
            .expect("spawn");
        let outcome = TaskOutcome::from_status(out.status);
        assert_eq!(
            outcome,
            TaskOutcome::Exited(141),
            "the SHELL exits 141; only the shell's own death would be Signaled"
        );
        assert_eq!(outcome.describe(), "exit code 141 (probable SIGPIPE)");
        assert!(!outcome.is_success());
    }

    #[test]
    fn exited_exposes_code_and_no_signal() {
        let o = TaskOutcome::Exited(101);
        assert_eq!(o.exit_code(), Some(101));
        assert_eq!(o.signal(), None);
        assert!(!o.is_success());
        assert_eq!(o.describe(), "exit code 101");
    }

    #[test]
    fn zero_exit_is_the_only_success() {
        assert!(TaskOutcome::Exited(0).is_success());
        assert!(!TaskOutcome::Exited(1).is_success());
        assert!(!TaskOutcome::Signaled(9).is_success());
        assert!(!TaskOutcome::Unknown.is_success());
    }

    /// A signal death must never present as an exit code — not `0`, not `-1`,
    /// not `128 + signum`. It reports the signal by name.
    #[test]
    fn signaled_reports_the_signal_and_never_an_exit_code() {
        let o = TaskOutcome::Signaled(9);
        assert_eq!(o.exit_code(), None, "a signal death has no exit code");
        assert_eq!(o.signal(), Some(9));
        assert_eq!(o.describe(), "killed by SIGKILL (signal 9)");

        assert_eq!(
            TaskOutcome::Signaled(11).describe(),
            "killed by SIGSEGV (signal 11)"
        );
        assert_eq!(
            TaskOutcome::Signaled(13).describe(),
            "killed by SIGPIPE (signal 13)"
        );
    }

    #[test]
    fn unmapped_signal_falls_through_to_a_bare_number() {
        let o = TaskOutcome::Signaled(31);
        assert_eq!(o.describe(), "killed by signal 31");
        assert_eq!(o.signal(), Some(31));
        assert_eq!(o.exit_code(), None);
    }

    /// The invariant the whole change exists to enforce: an unavailable status
    /// is words, never a number the reader can mistake for a clean exit.
    #[test]
    fn unknown_never_renders_as_a_number() {
        let o = TaskOutcome::Unknown;
        assert_eq!(o.exit_code(), None);
        assert_eq!(o.signal(), None);
        assert_eq!(o.describe(), "exit code unknown");
        assert!(
            !o.describe().contains('0') && !o.describe().contains("-1"),
            "unknown must not render any number: {}",
            o.describe()
        );
    }

    #[test]
    fn from_wait_error_is_unknown_not_a_synthesized_code() {
        let outcome = TaskOutcome::from_wait(Err(std::io::Error::other("wait failed")));
        assert_eq!(outcome, TaskOutcome::Unknown);
        assert_eq!(outcome.exit_code(), None);
    }

    #[test]
    fn from_persisted_round_trips_every_variant() {
        // Signal wins when both are present — a signalled child has no code, so
        // an exit_code alongside a signal can only come from a malformed row.
        assert_eq!(
            TaskOutcome::from_persisted(None, Some(9)),
            TaskOutcome::Signaled(9)
        );
        assert_eq!(
            TaskOutcome::from_persisted(Some(101), None),
            TaskOutcome::Exited(101)
        );
        assert_eq!(
            TaskOutcome::from_persisted(Some(0), None),
            TaskOutcome::Exited(0)
        );
        // Legacy row (pre-`signal`) with a null exit_code: watchdog timeout or
        // bash_kill. "We recorded no status" reads as Unknown, not as success.
        assert_eq!(
            TaskOutcome::from_persisted(None, None),
            TaskOutcome::Unknown
        );
    }

    #[tokio::test]
    async fn from_status_classifies_a_real_signal_death() {
        let out = command_shell()
            .command("kill -SEGV $$")
            .output()
            .await
            .expect("spawn");
        assert_eq!(
            TaskOutcome::from_status(out.status),
            TaskOutcome::Signaled(11),
            "a child that dies on SIGSEGV must classify as Signaled(11)"
        );
    }
}
