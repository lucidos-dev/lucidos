//! Locating and asking the `tailscale` CLI.
//!
//! Reading state goes to the LocalAPI first (see the crate docs). The CLI is
//! the last resort for that, and the only way to run `tailscale serve`. Serve
//! turns a plain-HTTP tailnet address into `https://<machine>.<tailnet>.ts.net`,
//! and so into an installable PWA with push. Nothing else can configure it.

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use crate::process::{run_bounded, RunError};
use crate::Answer;

const STATUS_TIMEOUT: Duration = Duration::from_secs(3);

/// Absolute-path candidates for the Tailscale CLI, most likely first.
///
/// **Real CLI binaries only.** `/Applications/Tailscale.app/Contents/MacOS/
/// Tailscale` is deliberately NOT here: it is the GUI executable. Outside a GUI
/// session it prints "The Tailscale GUI failed to start ... (Tailscale.CLIError
/// error 3)" to stdout and **exits 0**, so a caller that trusts the exit code
/// reads it as a success that produced unparseable output. Since resolution
/// picks ONE candidate by existence with no fall-through, listing it would
/// poison every Mac that has the app, which is every Mac that has Tailscale at
/// all. That is not hypothetical: it shipped, and it is what made **Settings ->
/// Access** show a Sign in button that silently did nothing on a machine
/// already on its tailnet.
///
/// macOS users get a real CLI from Homebrew or from the app's own
/// `/usr/local/bin/tailscale` symlink. With neither, the actions are
/// unavailable and the UI says so, while state reporting is unaffected because
/// it does not use this at all.
pub const TAILSCALE_CANDIDATES: &[&str] = &[
    "/usr/local/bin/tailscale", // the macOS app's own CLI symlink, or Intel Homebrew
    "/opt/homebrew/bin/tailscale", // Homebrew, Apple Silicon
    "/usr/bin/tailscale",       // Linux distro package
    r"C:\Program Files\Tailscale\tailscale.exe", // Windows installer
];

/// The fallback when no candidate exists: resolved through `PATH`, if any.
pub(crate) const BARE_CLI: &str = "tailscale";

/// Pure resolution of the Tailscale CLI: an explicit override, else the first
/// candidate that exists, else the bare name.
///
/// Resolving by ABSOLUTE PATH is the whole point. A bare
/// `Command::new("tailscale")` works in dev (rich shell `PATH`) and fails in
/// every packaged process, which Finder/launchd start with **no `PATH` at all**.
/// Same shape as `runtime::claude_code::resolve_claude_binary`. The bare-name
/// fallback keeps a custom install working wherever a `PATH` does exist.
pub fn resolve_tailscale_binary(
    env_override: Option<&str>,
    candidates: &[&str],
    exists: impl Fn(&str) -> bool,
) -> String {
    if let Some(p) = env_override {
        let trimmed = p.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    candidates
        .iter()
        .find(|c| exists(c))
        .map(|c| (*c).to_string())
        .unwrap_or_else(|| BARE_CLI.to_string())
}

/// `tailscale status --json` through `bin`. `None` when it could not start.
pub(crate) fn status_via_cli(bin: &str) -> Option<Answer> {
    let mut cmd = Command::new(bin);
    cmd.args(["status", "--json", "--peers=false"]);
    let ran = match run_bounded(cmd, STATUS_TIMEOUT) {
        Ok(ran) => ran,
        Err(RunError::Spawn) => return None,
        Err(RunError::TimedOut) => return Some(Answer::Silent),
    };
    Some(classify_cli_output(ran.status.success(), &ran.stdout))
}

/// JSON is an answer whatever the exit code. Exit 0 without JSON is the macOS
/// GUI shim talking; any other failure means the daemon did not answer.
fn classify_cli_output(succeeded: bool, stdout: &str) -> Answer {
    if serde_json::from_str::<serde_json::Value>(stdout).is_ok_and(|v| v.is_object()) {
        return Answer::Status(stdout.to_string());
    }
    if !succeeded {
        return Answer::Silent;
    }
    let first_line = stdout.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    let shown = &first_line[..first_line.floor_char_boundary(160)];
    Answer::Garbled(if shown.is_empty() {
        "the Tailscale CLI printed nothing".to_string()
    } else {
        format!("the Tailscale CLI printed \"{}\"", shown.trim())
    })
}

/// [`resolve_tailscale_binary`] against the process env + the real filesystem.
pub fn tailscale_binary() -> String {
    resolve_tailscale_binary(
        std::env::var("LUCIDOS_TAILSCALE_BIN").ok().as_deref(),
        TAILSCALE_CANDIDATES,
        |p| Path::new(p).exists(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tailscale_binary_prefers_an_absolute_path_over_the_bare_name() {
        // The packaged-process case: no PATH, so an absolute path or nothing.
        let only_homebrew = |p: &str| p == "/opt/homebrew/bin/tailscale";
        assert_eq!(
            resolve_tailscale_binary(None, TAILSCALE_CANDIDATES, only_homebrew),
            "/opt/homebrew/bin/tailscale"
        );
        // With several present, the first listed CLI wins.
        assert_eq!(
            resolve_tailscale_binary(None, TAILSCALE_CANDIDATES, |_| true),
            "/usr/local/bin/tailscale"
        );
    }

    #[test]
    fn cli_output_is_json_or_a_reason_why_not() {
        let json = r#"{"BackendState":"NeedsLogin"}"#;
        assert!(matches!(classify_cli_output(true, json), Answer::Status(_)));
        // Logged-out CLIs may exit non-zero and still print status.
        assert!(matches!(
            classify_cli_output(false, json),
            Answer::Status(_)
        ));
        // The daemon is stopped: the CLI fails on stderr, stdout empty.
        assert!(matches!(classify_cli_output(false, ""), Answer::Silent));
        // The macOS GUI shim: exit 0, prose on stdout.
        let shim = "The Tailscale GUI failed to start (Tailscale.CLIError error 3)\n";
        assert!(matches!(
            classify_cli_output(true, shim),
            Answer::Garbled(d) if d.contains("GUI failed to start")
        ));
    }

    #[test]
    fn a_cli_that_is_not_there_is_no_answer_at_all() {
        assert!(status_via_cli("/nonexistent/lucidos-no-such-tailscale").is_none());
    }

    #[test]
    fn tailscale_binary_env_override_wins_over_every_candidate() {
        assert_eq!(
            resolve_tailscale_binary(Some("/custom/ts"), TAILSCALE_CANDIDATES, |_| true),
            "/custom/ts"
        );
        // Set-but-empty is not an override; fall through to the candidates.
        assert_eq!(
            resolve_tailscale_binary(Some("  "), TAILSCALE_CANDIDATES, |_| true),
            "/usr/local/bin/tailscale"
        );
    }

    #[test]
    fn tailscale_candidates_exclude_the_macos_gui_executable() {
        // See TAILSCALE_CANDIDATES: the GUI binary exits 0 without answering,
        // and resolution stops at the first candidate that EXISTS, so listing
        // it would shadow the working CLI next to it.
        assert!(
            !TAILSCALE_CANDIDATES
                .iter()
                .any(|c| c.contains("Tailscale.app")),
            "the macOS GUI executable must never be a CLI candidate"
        );
        // No known location present: keep a bare name so a custom install still
        // resolves wherever a PATH exists (dev shells, most Linux).
        assert_eq!(
            resolve_tailscale_binary(None, TAILSCALE_CANDIDATES, |_| false),
            "tailscale"
        );
    }
}
