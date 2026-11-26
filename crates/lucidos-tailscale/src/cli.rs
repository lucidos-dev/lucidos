use std::path::Path;
use std::process::Command;
use std::time::Duration;

use crate::process::{run_bounded, RunError};
use crate::Answer;

const STATUS_TIMEOUT: Duration = Duration::from_secs(3);

pub const TAILSCALE_CANDIDATES: &[&str] = &[
    "/usr/local/bin/tailscale",
    "/opt/homebrew/bin/tailscale",
    "/usr/bin/tailscale",
    r"C:\Program Files\Tailscale\tailscale.exe",
];

pub(crate) const BARE_CLI: &str = "tailscale";

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

fn classify_cli_output(succeeded: bool, stdout: &str) -> Answer {
    if serde_json::from_str::<serde_json::Value>(stdout).is_ok_and(|v| v.is_object()) {
        return Answer::Body(stdout.to_string());
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
        let only_homebrew = |p: &str| p == "/opt/homebrew/bin/tailscale";
        assert_eq!(
            resolve_tailscale_binary(None, TAILSCALE_CANDIDATES, only_homebrew),
            "/opt/homebrew/bin/tailscale"
        );
        assert_eq!(
            resolve_tailscale_binary(None, TAILSCALE_CANDIDATES, |_| true),
            "/usr/local/bin/tailscale"
        );
    }

    #[test]
    fn cli_output_is_json_or_a_reason_why_not() {
        let json = r#"{"BackendState":"NeedsLogin"}"#;
        assert!(matches!(classify_cli_output(true, json), Answer::Body(_)));
        assert!(matches!(classify_cli_output(false, json), Answer::Body(_)));
        assert!(matches!(classify_cli_output(false, ""), Answer::Silent));
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
        assert_eq!(
            resolve_tailscale_binary(Some("  "), TAILSCALE_CANDIDATES, |_| true),
            "/usr/local/bin/tailscale"
        );
    }

    #[test]
    fn tailscale_candidates_exclude_the_macos_gui_executable() {
        assert!(
            !TAILSCALE_CANDIDATES
                .iter()
                .any(|c| c.contains("Tailscale.app")),
            "the macOS GUI executable must never be a CLI candidate"
        );
        assert_eq!(
            resolve_tailscale_binary(None, TAILSCALE_CANDIDATES, |_| false),
            "tailscale"
        );
    }
}
