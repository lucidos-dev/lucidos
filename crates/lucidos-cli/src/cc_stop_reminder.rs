//! Stop hook subcommand: when CC tries to idle, nudge it to run `/harden` if
//! it has committed work that hasn't been reviewed yet. Soft reminder — the
//! model can ignore it and continue, or run `/harden` and stop again.
//!
//! Wired into `<workspace>/.lucidos/cc-settings.json` via the engine's
//! `cc_settings.rs`.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::hardened::{self, HardenedState};
use crate::workspace::{resolve_from_env, BoxError};

/// Env var the engine sets when spawning a CC session that's interactive
/// (chat / recovery / external-repo). Absent for unattended sessions
/// (conflict-resolution). Read by `run()` to gate the AskUserQuestion redirect.
pub(crate) const SESSION_KIND_ENV: &str = "LUCIDOS_SESSION_KIND";

/// Value of [`SESSION_KIND_ENV`] that means "user is at the keyboard, safe
/// to redirect plaintext questions to AskUserQuestion".
pub(crate) const SESSION_KIND_INTERACTIVE: &str = "interactive";

fn cc_sentinel_path(kind: &str, key: &str) -> PathBuf {
    std::env::temp_dir().join(format!("lucidos-cc-{}-{}", kind, key))
}

/// Subset of CC's Stop-hook stdin payload. CC sends additional fields
/// (`session_id`, `cwd`, `permission_mode`, `effort`, etc.) that we ignore —
/// keep this `#[derive(Deserialize)]` permissive (no `deny_unknown_fields`)
/// so a future CC release adding new fields can't break the hook.
#[derive(Debug, Deserialize)]
pub(crate) struct StopHookPayload {
    pub(crate) transcript_path: String,
}

pub(crate) fn parse_stop_hook_payload(raw: &str) -> Result<StopHookPayload, BoxError> {
    serde_json::from_str(raw).map_err(Into::into)
}

/// Walk the CC transcript JSONL and return the UUID of the last assistant
/// message iff its FINAL text block ends with `?`, `stop_reason == "end_turn"`,
/// and no `tool_use` block follows that text. This is the "CC ended its turn
/// with a plaintext question" signal that the Stop hook turns into an
/// AskUserQuestion redirect.
///
/// Returns `Ok(None)` when the transcript is empty, the last assistant message
/// doesn't match the pattern, or individual lines are malformed (skipped).
/// Returns `Err` only when the file can't be opened — caller treats that as
/// "no signal" so a missing transcript can't break the harden path.
pub(crate) fn detect_plaintext_question(
    transcript_path: &Path,
) -> Result<Option<String>, BoxError> {
    let file = std::fs::File::open(transcript_path)
        .map_err(|e| format!("open transcript {}: {}", transcript_path.display(), e))?;
    let reader = BufReader::new(file);

    // Streaming forward and overwriting `last` is simpler than reverse-seeking
    // and good enough — transcripts cap at a few MB even for long sessions.
    // Cheap substring check before the JSON parse rejects the bulk (user /
    // tool_result / queue-operation / attachment lines) without allocating a
    // `serde_json::Value` tree per line.
    let mut last_assistant: Option<serde_json::Value> = None;
    for line in reader.lines() {
        let Ok(line) = line else { continue };
        if !line.contains("\"type\":\"assistant\"") {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        if v.get("type").and_then(|t| t.as_str()) == Some("assistant") {
            last_assistant = Some(v);
        }
    }
    let Some(msg) = last_assistant else {
        return Ok(None);
    };

    let stop_reason = msg
        .pointer("/message/stop_reason")
        .and_then(|s| s.as_str())
        .unwrap_or("");
    if stop_reason != "end_turn" {
        return Ok(None);
    }

    let content = msg
        .pointer("/message/content")
        .and_then(|c| c.as_array())
        .cloned()
        .unwrap_or_default();

    // Find the index of the LAST text block.
    let last_text_idx = content
        .iter()
        .enumerate()
        .rev()
        .find(|(_, b)| b.get("type").and_then(|t| t.as_str()) == Some("text"))
        .map(|(i, _)| i);
    let Some(idx) = last_text_idx else {
        return Ok(None);
    };

    // Reject if any tool_use sits AFTER the last text — CC was working,
    // not asking.
    let tool_after = content[idx + 1..]
        .iter()
        .any(|b| b.get("type").and_then(|t| t.as_str()) == Some("tool_use"));
    if tool_after {
        return Ok(None);
    }

    let text = content[idx]
        .get("text")
        .and_then(|t| t.as_str())
        .unwrap_or("");
    if !text.trim_end().ends_with('?') {
        return Ok(None);
    }

    let uuid = msg.get("uuid").and_then(|u| u.as_str()).unwrap_or("");
    if uuid.is_empty() {
        return Ok(None);
    }
    Ok(Some(uuid.to_string()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StopDecision {
    Allow,
    Remind,
}

/// Outcome of the Stop hook orchestration. Three states because the hook now
/// covers two unrelated concerns: harden enforcement and AskUserQuestion
/// redirect. `run()` maps each variant to one stdout JSON.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StopAction {
    Allow,
    RemindHarden,
    RedirectQuestion { message_uuid: String },
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

/// Orchestrates the Stop-hook priority: a detected plaintext question wins
/// over the harden reminder (the user must answer first; harden makes no
/// sense until the question is resolved). Each sentinel-present flag means
/// "we already nudged for this key" — guards against infinite redirect loops
/// (question) and re-nagging on the same commit (harden). Pure function —
/// `run()` does the IO and feeds it the inputs.
pub(crate) fn decide_stop_action(
    question_uuid: Option<String>,
    question_sentinel_present: bool,
    commits_ahead: u32,
    state: HardenedState,
    harden_sentinel_present: bool,
) -> StopAction {
    if let Some(uuid) = question_uuid {
        if !question_sentinel_present {
            return StopAction::RedirectQuestion { message_uuid: uuid };
        }
    }
    if harden_sentinel_present {
        return StopAction::Allow;
    }
    match decide(commits_ahead, state) {
        StopDecision::Allow => StopAction::Allow,
        StopDecision::Remind => StopAction::RemindHarden,
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

pub(crate) const QUESTION_REDIRECT_REASON: &str =
    "You ended your turn with a plaintext question. Re-issue it via the \
     AskUserQuestion tool so the user can click options instead of typing. \
     Reserve plaintext questions for genuinely open-ended ones \
     (e.g. \"what name should I use?\") where pre-baked options would be guesses.";

pub(crate) fn build_question_redirect_json() -> String {
    serde_json::json!({
        "decision": "block",
        "reason": QUESTION_REDIRECT_REASON,
    })
    .to_string()
}

fn question_sentinel_path(message_uuid: &str) -> PathBuf {
    cc_sentinel_path("question-redirect", message_uuid)
}

pub(crate) fn run() -> Result<(), BoxError> {
    use std::io::Read;
    let mut stdin_buf = String::new();
    let _ = std::io::stdin().lock().read_to_string(&mut stdin_buf);

    let cwd = std::env::current_dir().map_err(|e| format!("cc-stop-reminder cwd: {}", e))?;

    // External-repo CC sessions reach this hook too (cc-settings.json is
    // workspace-scoped). Skip silently when /harden isn't available.
    if !harden_command_available(&cwd) {
        return Ok(());
    }

    // AskUserQuestion redirect only fires for interactive sessions —
    // unattended ones (conflict-resolution) would hang waiting for an answer
    // that's not coming. Env var is set by the engine in
    // `runtime/claude_code.rs::build_command` from `SpawnArgs.interactive`.
    let question_uuid =
        if std::env::var(SESSION_KIND_ENV).as_deref() == Ok(SESSION_KIND_INTERACTIVE) {
            parse_stop_hook_payload(&stdin_buf).ok().and_then(|p| {
                detect_plaintext_question(Path::new(&p.transcript_path))
                    .ok()
                    .flatten()
            })
        } else {
            None
        };

    let commits_ahead = hardened::run_git(&cwd, &["rev-list", "--count", "main..HEAD"])
        .ok()
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(0);

    if question_uuid.is_none() && commits_ahead == 0 {
        return Ok(());
    }

    let head_sha = hardened::run_git(&cwd, &["rev-parse", "HEAD"]).unwrap_or_default();
    let harden_sentinel_present = !head_sha.is_empty() && sentinel_path(&head_sha).exists();

    let state = if commits_ahead == 0 || harden_sentinel_present {
        HardenedState::Fresh
    } else {
        let Ok(ws) = resolve_from_env() else {
            return Ok(());
        };
        // Engine unreachable / transport error => Missing, which still reminds.
        // Better to nag than let unhardened code through unnoticed.
        hardened::query_state(&ws).unwrap_or(HardenedState::Missing)
    };

    let question_sentinel_present = question_uuid
        .as_ref()
        .map(|u| question_sentinel_path(u).exists())
        .unwrap_or(false);

    match decide_stop_action(
        question_uuid,
        question_sentinel_present,
        commits_ahead,
        state,
        harden_sentinel_present,
    ) {
        StopAction::Allow => Ok(()),
        StopAction::RemindHarden => {
            if !head_sha.is_empty() {
                write_sentinel(&sentinel_path(&head_sha));
            }
            println!("{}", build_reminder_json());
            Ok(())
        }
        StopAction::RedirectQuestion { message_uuid } => {
            write_sentinel(&question_sentinel_path(&message_uuid));
            println!("{}", build_question_redirect_json());
            Ok(())
        }
    }
}

fn sentinel_path(head_sha: &str) -> PathBuf {
    cc_sentinel_path("stop-reminder", head_sha)
}

/// Writing the sentinel is the loop guard for both the harden reminder and
/// the question redirect — if it silently fails we'd re-nag every Stop or
/// infinite-loop the redirect. Surface the failure on stderr (CC's hook
/// stderr is preserved) instead of `let _ = `.
fn write_sentinel(path: &Path) {
    if let Err(e) = std::fs::write(path, b"") {
        eprintln!(
            "[cc-stop-reminder] sentinel write failed at {}: {}",
            path.display(),
            e
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_transcript(lines: &[serde_json::Value]) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.jsonl");
        let body: String = lines
            .iter()
            .map(|v| format!("{}\n", v))
            .collect::<Vec<_>>()
            .concat();
        std::fs::write(&path, body).unwrap();
        (dir, path)
    }

    #[test]
    fn parse_stop_hook_payload_extracts_transcript_path() {
        let raw = r#"{
            "session_id": "sid-1",
            "transcript_path": "/tmp/cc-transcript.jsonl",
            "cwd": "/tmp/wt",
            "hook_event_name": "Stop"
        }"#;
        let parsed = parse_stop_hook_payload(raw).expect("valid payload");
        assert_eq!(parsed.transcript_path, "/tmp/cc-transcript.jsonl");
    }

    #[test]
    fn parse_stop_hook_payload_tolerates_extra_fields() {
        // CC adds fields over time (permission_mode, effort, etc) — must not break.
        let raw = r#"{
            "session_id": "sid-2",
            "transcript_path": "/tmp/x.jsonl",
            "cwd": "/tmp/wt",
            "hook_event_name": "Stop",
            "permission_mode": "acceptEdits",
            "effort": {"level": "high"}
        }"#;
        let parsed = parse_stop_hook_payload(raw).expect("must tolerate extra fields");
        assert_eq!(parsed.transcript_path, "/tmp/x.jsonl");
    }

    #[test]
    fn parse_stop_hook_payload_rejects_missing_required_fields() {
        let raw = r#"{"session_id": "sid-3"}"#;
        assert!(parse_stop_hook_payload(raw).is_err());
    }

    #[test]
    fn detect_plaintext_question_returns_uuid_when_last_text_ends_with_question_mark() {
        let (_d, path) = write_transcript(&[serde_json::json!({
            "type": "assistant",
            "uuid": "msg-uuid-1",
            "message": {
                "role": "assistant",
                "stop_reason": "end_turn",
                "content": [
                    {"type": "text", "text": "Sure, here's what I think.\n\nWant me to add the rule?"}
                ]
            }
        })]);
        assert_eq!(
            detect_plaintext_question(&path).unwrap(),
            Some("msg-uuid-1".to_string())
        );
    }

    #[test]
    fn detect_plaintext_question_ignores_when_tool_use_follows_text() {
        let (_d, path) = write_transcript(&[serde_json::json!({
            "type": "assistant",
            "uuid": "msg-uuid-2",
            "message": {
                "role": "assistant",
                "stop_reason": "tool_use",
                "content": [
                    {"type": "text", "text": "Let me check. What does this look like?"},
                    {"type": "tool_use", "name": "Read", "id": "t1", "input": {}}
                ]
            }
        })]);
        assert_eq!(detect_plaintext_question(&path).unwrap(), None);
    }

    #[test]
    fn detect_plaintext_question_ignores_when_stop_reason_is_not_end_turn() {
        let (_d, path) = write_transcript(&[serde_json::json!({
            "type": "assistant",
            "uuid": "msg-uuid-3",
            "message": {
                "role": "assistant",
                "stop_reason": "stop_sequence",
                "content": [{"type": "text", "text": "Wait, what?"}]
            }
        })]);
        assert_eq!(detect_plaintext_question(&path).unwrap(), None);
    }

    #[test]
    fn detect_plaintext_question_ignores_when_text_does_not_end_with_question_mark() {
        let (_d, path) = write_transcript(&[serde_json::json!({
            "type": "assistant",
            "uuid": "msg-uuid-4",
            "message": {
                "role": "assistant",
                "stop_reason": "end_turn",
                "content": [{"type": "text", "text": "Done. Ready for review."}]
            }
        })]);
        assert_eq!(detect_plaintext_question(&path).unwrap(), None);
    }

    #[test]
    fn detect_plaintext_question_handles_trailing_whitespace_after_question_mark() {
        let (_d, path) = write_transcript(&[serde_json::json!({
            "type": "assistant",
            "uuid": "msg-uuid-5",
            "message": {
                "role": "assistant",
                "stop_reason": "end_turn",
                "content": [{"type": "text", "text": "Should I proceed?\n\n  "}]
            }
        })]);
        assert_eq!(
            detect_plaintext_question(&path).unwrap(),
            Some("msg-uuid-5".to_string())
        );
    }

    #[test]
    fn detect_plaintext_question_uses_only_the_last_assistant_message() {
        let (_d, path) = write_transcript(&[
            serde_json::json!({
                "type": "assistant", "uuid": "early",
                "message": {"role":"assistant","stop_reason":"end_turn",
                            "content": [{"type":"text","text":"Should I do X?"}]}
            }),
            serde_json::json!({
                "type": "assistant", "uuid": "late",
                "message": {"role":"assistant","stop_reason":"end_turn",
                            "content": [{"type":"text","text":"Done."}]}
            }),
        ]);
        assert_eq!(detect_plaintext_question(&path).unwrap(), None);
    }

    #[test]
    fn detect_plaintext_question_returns_none_for_empty_transcript() {
        let (_d, path) = write_transcript(&[]);
        assert_eq!(detect_plaintext_question(&path).unwrap(), None);
    }

    #[test]
    fn detect_plaintext_question_skips_malformed_lines() {
        // write_transcript only takes JSON values, so build the file by hand
        // here to mix a malformed line in alongside a valid one.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.jsonl");
        let good = serde_json::json!({
            "type":"assistant","uuid":"u1",
            "message":{"role":"assistant","stop_reason":"end_turn",
                       "content":[{"type":"text","text":"Want X?"}]}
        });
        std::fs::write(&path, format!("not json\n{}\n", good)).unwrap();
        assert_eq!(
            detect_plaintext_question(&path).unwrap(),
            Some("u1".to_string())
        );
    }

    #[test]
    fn detect_plaintext_question_returns_err_when_path_missing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("does-not-exist.jsonl");
        assert!(detect_plaintext_question(&path).is_err());
    }

    #[test]
    fn redirect_reason_names_askuserquestion_and_explains_why() {
        let json = build_question_redirect_json();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["decision"], "block");
        let reason = parsed["reason"].as_str().expect("reason must be a string");
        assert!(
            reason.contains("AskUserQuestion"),
            "reason must name the tool so CC knows what to invoke",
        );
        assert!(
            reason.to_lowercase().contains("plaintext"),
            "reason must explain what was wrong",
        );
    }

    #[test]
    fn question_sentinel_path_is_keyed_on_message_uuid() {
        // Two different message UUIDs must produce two different paths,
        // so a redirect for one message doesn't suppress the next.
        let a = question_sentinel_path("aaaa");
        let b = question_sentinel_path("bbbb");
        assert_ne!(a, b);
        assert!(a.to_string_lossy().contains("aaaa"));
        assert!(b.to_string_lossy().contains("bbbb"));
    }

    #[test]
    fn stop_action_redirects_question_when_detected_and_sentinel_absent() {
        let action = decide_stop_action(
            Some("uuid-1".to_string()),
            false, // question sentinel absent
            5,     // commits_ahead — irrelevant when question wins
            HardenedState::Missing,
            false, // harden sentinel absent
        );
        assert_eq!(
            action,
            StopAction::RedirectQuestion {
                message_uuid: "uuid-1".to_string()
            }
        );
    }

    #[test]
    fn stop_action_falls_through_to_harden_when_question_sentinel_present() {
        let action = decide_stop_action(
            Some("uuid-1".to_string()),
            true, // question sentinel present — already redirected once
            5,
            HardenedState::Missing,
            false,
        );
        assert_eq!(action, StopAction::RemindHarden);
    }

    #[test]
    fn stop_action_falls_through_to_harden_when_no_question() {
        let action = decide_stop_action(None, false, 5, HardenedState::Missing, false);
        assert_eq!(action, StopAction::RemindHarden);
    }

    #[test]
    fn stop_action_allows_when_no_question_and_no_commits() {
        let action = decide_stop_action(None, false, 0, HardenedState::Missing, false);
        assert_eq!(action, StopAction::Allow);
    }

    #[test]
    fn stop_action_allows_when_no_question_and_already_hardened() {
        let action = decide_stop_action(None, false, 5, HardenedState::Fresh, false);
        assert_eq!(action, StopAction::Allow);
    }

    #[test]
    fn stop_action_allows_when_harden_sentinel_present_even_if_state_missing() {
        // Sentinel encodes "we already nudged for this commit" — second Stop
        // attempt for the same HEAD must allow even if the engine still
        // reports Missing (e.g. user hasn't run /harden yet, but we already
        // reminded them once).
        let action = decide_stop_action(None, false, 5, HardenedState::Missing, true);
        assert_eq!(action, StopAction::Allow);
    }

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
