//! Stop hook subcommand: when CC tries to idle, nudge it to run `/harden` if
//! it has committed work that hasn't been reviewed yet. Soft reminder — the
//! model can ignore it and continue, or run `/harden` and stop again.
//!
//! Wired into `<workspace>/.lucidos/cc-settings.json` via the engine's
//! `cc_settings.rs`.

use std::path::Path;

use crate::hardened::{self, HardenedState};
use crate::workspace::{resolve_from_env, BoxError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StopDecision {
    Allow,
    Remind,
}

/// Pure decision: should this Stop attempt trigger a `/harden` reminder?
/// Sentinel handling lives in `run()` — the pre-filter is the sole enforcement
/// so this signature stays tight.
pub(crate) fn decide(commits_ahead: u32, state: HardenedState) -> StopDecision {
    if commits_ahead == 0 {
        return StopDecision::Allow;
    }
    match state {
        // Fresh/Stale = trusted per CLAUDE.md (follow-up commits don't re-trigger).
        HardenedState::Fresh | HardenedState::Stale => StopDecision::Allow,
        HardenedState::Missing => StopDecision::Remind,
    }
}

/// `cc-settings.json` is workspace-scoped, so this hook fires for
/// external-repo CC sessions too. `/harden` is only defined in the Lucidos
/// repo at `.claude/commands/harden.md` — the per-session filesystem check
/// keeps the reminder from pointing CC at a command that isn't there.
pub(crate) fn harden_command_available(cwd: &Path) -> bool {
    cwd.join(".claude/commands/harden.md").is_file()
}

pub(crate) const REMINDER_REASON: &str =
    "If you're done implementing, run /harden now. If you have more work to do, \
     ignore this and continue.";

pub(crate) fn build_reminder_json() -> String {
    serde_json::json!({
        "decision": "block",
        "reason": REMINDER_REASON,
    })
    .to_string()
}

pub(crate) fn run() -> Result<(), BoxError> {
    // Drain stdin so CC's writer doesn't see SIGPIPE if our exit beats its flush.
    let _ = std::io::copy(&mut std::io::stdin().lock(), &mut std::io::sink());

    let cwd = std::env::current_dir().map_err(|e| format!("cc-stop-reminder cwd: {}", e))?;

    // External-repo CC sessions reach this hook too (cc-settings.json is
    // workspace-scoped). Skip silently when /harden isn't available.
    if !harden_command_available(&cwd) {
        return Ok(());
    }

    // Fail-fast on read-only sessions: skip rev-parse + sentinel + HTTP.
    let commits_ahead = hardened::run_git(&cwd, &["rev-list", "--count", "main..HEAD"])
        .ok()
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(0);
    if commits_ahead == 0 {
        return Ok(());
    }

    let head_sha = hardened::run_git(&cwd, &["rev-parse", "HEAD"]).unwrap_or_default();
    if head_sha.is_empty() {
        return Ok(());
    }

    let sentinel = sentinel_path(&head_sha);
    if sentinel.exists() {
        return Ok(());
    }

    // No workspace = run from outside a Lucidos worktree. Allow stop silently.
    let Ok(ws) = resolve_from_env() else {
        return Ok(());
    };
    // Engine unreachable / transport error => Missing, which still reminds.
    // Better to nag than let unhardened code through unnoticed.
    let state = hardened::query_state(&ws).unwrap_or(HardenedState::Missing);

    match decide(commits_ahead, state) {
        StopDecision::Allow => Ok(()),
        StopDecision::Remind => {
            let _ = std::fs::write(&sentinel, b"");
            println!("{}", build_reminder_json());
            Ok(())
        }
    }
}

fn sentinel_path(head_sha: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("lucidos-cc-stop-reminder-{}", head_sha))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allow_when_no_commits_ahead() {
        assert_eq!(
            decide(0, HardenedState::Missing),
            StopDecision::Allow,
            "read-only sessions must not get a spurious harden reminder",
        );
    }

    #[test]
    fn allow_when_already_fresh() {
        assert_eq!(decide(5, HardenedState::Fresh), StopDecision::Allow);
    }

    #[test]
    fn allow_when_stale_marker_present() {
        // CLAUDE.md policy: once hardened, follow-up tweaks don't re-trigger.
        assert_eq!(decide(7, HardenedState::Stale), StopDecision::Allow);
    }

    #[test]
    fn remind_when_commits_exist_and_no_marker() {
        assert_eq!(decide(1, HardenedState::Missing), StopDecision::Remind);
    }

    #[test]
    fn harden_command_available_true_when_file_present() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".claude/commands")).unwrap();
        std::fs::write(dir.path().join(".claude/commands/harden.md"), b"# /harden").unwrap();
        assert!(
            harden_command_available(dir.path()),
            "Lucidos worktree ships .claude/commands/harden.md — must detect it",
        );
    }

    #[test]
    fn harden_command_available_false_when_file_missing() {
        let dir = tempfile::tempdir().unwrap();
        assert!(
            !harden_command_available(dir.path()),
            "external repos don't ship /harden — hook must not nudge for it",
        );
    }

    #[test]
    fn harden_command_available_false_when_path_is_directory() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".claude/commands/harden.md")).unwrap();
        assert!(
            !harden_command_available(dir.path()),
            "must require a regular file, not a directory of that name",
        );
    }

    #[test]
    fn reminder_json_uses_block_decision_with_permissive_reason() {
        let parsed: serde_json::Value =
            serde_json::from_str(&build_reminder_json()).unwrap();
        assert_eq!(parsed["decision"], "block");
        let reason = parsed["reason"].as_str().expect("reason must be a string");
        assert!(
            reason.contains("/harden"),
            "reason must name the skill so CC knows what to invoke",
        );
        assert!(
            reason.to_lowercase().contains("ignore"),
            "wording must be permissive — CC can ignore if not done",
        );
    }
}
