use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use super::agent_sandbox::{Sandbox, SandboxedCommand};

const BASH_CANDIDATES: [&str; 2] = ["/bin/bash", "/usr/bin/bash"];

const POSIX_SH: &str = "/bin/sh";

pub struct CommandShell {
    program: PathBuf,
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
        log_legacy!(
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

    #[cfg(test)]
    pub fn has_pipefail(&self) -> bool {
        self.pipefail
    }

    pub fn program(&self) -> &Path {
        &self.program
    }

    /// INVARIANT: The interpreter with its options applied and no target yet. Both spawn
    /// helpers start here so the `pipefail` flag can only ever be applied in
    /// one place.
    fn base(&self, sandbox: &Sandbox) -> std::io::Result<SandboxedCommand> {
        let mut cmd = sandbox.command(&self.program)?;
        if self.pipefail {
            cmd.args(["-o", "pipefail"]);
        }
        Ok(cmd)
    }

    pub fn command(&self, sandbox: &Sandbox, command: &str) -> std::io::Result<SandboxedCommand> {
        let mut cmd = self.base(sandbox)?;
        cmd.arg("-c").arg(command);
        Ok(cmd)
    }

    pub fn script(
        &self,
        sandbox: &Sandbox,
        script_path: &Path,
    ) -> std::io::Result<SandboxedCommand> {
        let mut cmd = self.base(sandbox)?;
        cmd.arg(script_path);
        Ok(cmd)
    }
}

pub fn command_shell() -> &'static CommandShell {
    static SHELL: OnceLock<CommandShell> = OnceLock::new();
    SHELL.get_or_init(CommandShell::resolve)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskOutcome {
    Exited(i32),
    Signaled(i32),
    Unknown,
}

impl TaskOutcome {
    /// INVARIANT: Classify a reaped child's status.
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

    pub fn from_wait(result: std::io::Result<std::process::ExitStatus>) -> Self {
        match result {
            Ok(status) => Self::from_status(status),
            Err(_) => TaskOutcome::Unknown,
        }
    }

    pub fn from_persisted(exit_code: Option<i32>, signal: Option<i32>) -> Self {
        match (signal, exit_code) {
            (Some(sig), _) => TaskOutcome::Signaled(sig),
            (None, Some(code)) => TaskOutcome::Exited(code),
            (None, None) => TaskOutcome::Unknown,
        }
    }

    pub fn exit_code(&self) -> Option<i32> {
        match self {
            TaskOutcome::Exited(code) => Some(*code),
            TaskOutcome::Signaled(_) | TaskOutcome::Unknown => None,
        }
    }

    pub fn signal(&self) -> Option<i32> {
        match self {
            TaskOutcome::Signaled(sig) => Some(*sig),
            TaskOutcome::Exited(_) | TaskOutcome::Unknown => None,
        }
    }

    pub fn is_success(&self) -> bool {
        matches!(self, TaskOutcome::Exited(0))
    }

    pub fn as_reported(self, abandoned: bool) -> TaskOutcome {
        if abandoned {
            TaskOutcome::Unknown
        } else {
            self
        }
    }

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

fn probable_signal(code: i32) -> Option<&'static str> {
    if (129..=159).contains(&code) {
        signal_name(code - 128)
    } else {
        None
    }
}

pub fn signal_name(sig: i32) -> Option<&'static str> {
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

pub const MAX_OUTPUT_BYTES: usize = 100 * 1024;

pub const KEPT_BYTES_CAP: usize = 4 * MAX_OUTPUT_BYTES;

pub fn finalize_stream(bytes: &[u8], total: u64) -> String {
    let sanitized = super::sanitize_for_jsonb(&String::from_utf8_lossy(bytes));
    let dropped = total > bytes.len() as u64;
    let dropped_total = dropped.then(|| usize::try_from(total).unwrap_or(usize::MAX));
    truncate_output(&sanitized, MAX_OUTPUT_BYTES, dropped_total)
}

fn truncate_output(s: &str, max: usize, dropped_total: Option<usize>) -> String {
    if s.len() <= max && dropped_total.is_none() {
        return s.to_string();
    }
    let end = s.floor_char_boundary(max);
    let total = dropped_total.unwrap_or(0).max(s.len());
    format!("{}...\n[truncated: {} bytes total]", &s[..end], total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_capped_stream_that_sanitizes_to_nothing_is_still_marked() {
        let shown = finalize_stream(&[0u8; 16], 1_000_000);
        assert!(
            shown.ends_with("[truncated: 1000000 bytes total]"),
            "{shown:?}"
        );
        assert_eq!(finalize_stream(b"ok", 2), "ok");
    }

    #[test]
    fn complete_output_with_a_nul_byte_carries_no_marker() {
        assert_eq!(finalize_stream(b"a\0b", 3), "ab");
        assert!(finalize_stream(b"ab", 5).ends_with("[truncated: 5 bytes total]"));
    }

    #[test]
    fn truncate_output_short_string() {
        let s = "hello world";
        assert_eq!(truncate_output(s, 100, None), "hello world");
    }

    #[test]
    fn truncate_output_long_string() {
        let s = "a".repeat(200);
        let result = truncate_output(&s, 50, None);
        assert!(result.starts_with(&"a".repeat(50)));
        assert!(result.contains("[truncated"));
        assert!(result.contains("200 bytes total"));
    }

    #[test]
    fn truncate_output_multibyte_boundary() {
        let s = "ééééé";
        let result = truncate_output(s, 5, None);
        assert!(result.contains("[truncated"));
    }

    #[test]
    fn resolved_shell_is_reused_and_self_consistent() {
        let a = command_shell();
        let b = command_shell();
        assert!(std::ptr::eq(a, b), "shell must resolve once per process");
        assert!(a.program().is_file(), "resolved shell must exist on disk");
        if a.has_pipefail() {
            assert!(BASH_CANDIDATES.contains(&a.program().to_str().unwrap()));
        } else {
            assert_eq!(a.program(), Path::new(POSIX_SH));
        }
    }

    #[tokio::test]
    async fn command_does_not_mask_a_failing_stage_behind_later_successes() {
        if !command_shell().has_pipefail() {
            return;
        }
        let out = command_shell()
            .command(
                &Sandbox::unsandboxed(),
                "sh -c 'echo lints; exit 101' 2>&1 | tee /dev/null | cat",
            )
            .expect("unsandboxed command")
            .output()
            .await
            .expect("spawn");
        assert_eq!(
            TaskOutcome::from_status(out.status),
            TaskOutcome::Exited(101),
            "pipeline must report clippy's 101, not tee/cat's 0"
        );
    }

    #[tokio::test]
    async fn pipefail_reports_the_rightmost_failing_stage_not_the_first() {
        if !command_shell().has_pipefail() {
            return;
        }
        let out = command_shell()
            .command(
                &Sandbox::unsandboxed(),
                "sh -c 'exit 42' | sh -c 'exit 7' | cat",
            )
            .expect("unsandboxed command")
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
            .command(&Sandbox::unsandboxed(), "echo hi; exit 3")
            .expect("unsandboxed command")
            .output()
            .await
            .expect("spawn");
        assert_eq!(TaskOutcome::from_status(out.status), TaskOutcome::Exited(3));
        assert!(String::from_utf8_lossy(&out.stdout).contains("hi"));
    }

    #[tokio::test]
    async fn script_runs_a_file_and_applies_pipefail() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        let script = dir.join("piped.sh");
        std::fs::write(&script, "sh -c 'exit 101' | tee /dev/null\n").unwrap();

        let out = command_shell()
            .script(&Sandbox::unsandboxed(), &script)
            .expect("unsandboxed command")
            .output()
            .await
            .expect("spawn");
        let expected = if command_shell().has_pipefail() {
            TaskOutcome::Exited(101)
        } else {
            TaskOutcome::Exited(0)
        };
        assert_eq!(TaskOutcome::from_status(out.status), expected);
    }

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
        assert_eq!(TaskOutcome::Exited(141).exit_code(), Some(141));
        assert_eq!(TaskOutcome::Exited(141).signal(), None);
    }

    #[test]
    fn ordinary_exit_codes_get_no_signal_hint() {
        for code in [0, 1, 2, 101, 128, 160, 255] {
            let phrase = TaskOutcome::Exited(code).describe();
            assert_eq!(phrase, format!("exit code {code}"), "code {code}");
        }
        assert_eq!(TaskOutcome::Exited(144).describe(), "exit code 144");
    }

    #[tokio::test]
    async fn sigpipe_in_a_pipeline_surfaces_as_a_decoded_exit_code() {
        if !command_shell().has_pipefail() {
            return;
        }
        let out = command_shell()
            .command(&Sandbox::unsandboxed(), "yes | head -1 >/dev/null")
            .expect("unsandboxed command")
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
        assert_eq!(
            TaskOutcome::from_persisted(None, Some(9)),
            TaskOutcome::Signaled(9)
        );
        assert_eq!(
            TaskOutcome::from_persisted(Some(0), Some(9)),
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
        assert_eq!(
            TaskOutcome::from_persisted(None, None),
            TaskOutcome::Unknown
        );
    }

    #[tokio::test]
    async fn from_status_classifies_a_real_signal_death() {
        let out = command_shell()
            .command(&Sandbox::unsandboxed(), "kill -SEGV $$")
            .expect("unsandboxed command")
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
