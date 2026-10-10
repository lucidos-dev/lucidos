//! Stop hook subcommand: when Claude Code ends its turn on a plaintext
//! question, send it back once to re-ask through AskUserQuestion. A soft block:
//! the model re-asks, or keeps its open-ended question and stops again.
//!
//! Asking for `/harden` belongs to the engine's turn-end gate, which reaches
//! both backends (ADR 0417).
//!
//! Wired into `<workspace>/.lucidos/cc-settings.json` via the engine's
//! `cc_settings.rs`.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::workspace::BoxError;

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

    // Only an interactive session can redirect: an unattended one
    // (conflict resolution) would hang waiting for an answer that never comes.
    // The engine sets the variable in `runtime/claude_code.rs::build_command`
    // from `SpawnArgs.interactive`.
    if std::env::var(SESSION_KIND_ENV).as_deref() != Ok(SESSION_KIND_INTERACTIVE) {
        return Ok(());
    }
    let question_uuid = parse_stop_hook_payload(&stdin_buf).ok().and_then(|p| {
        detect_plaintext_question(Path::new(&p.transcript_path))
            .ok()
            .flatten()
    });
    let Some(message_uuid) = question_uuid else {
        return Ok(());
    };
    // The sentinel means this message was already sent back once.
    let sentinel = question_sentinel_path(&message_uuid);
    if !sentinel.exists() {
        write_sentinel(&sentinel);
        println!("{}", build_question_redirect_json());
    }
    Ok(())
}

/// Writing the sentinel is the redirect's loop guard. If it silently failed,
/// the hook would redirect the same question on every Stop. Surface the failure on stderr (CC's hook
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

    /// The hook acts on a plaintext question and nothing else. Asking for
    /// `/harden` has one definition, the engine's turn-end gate (ADR 0417).
    #[test]
    fn the_hook_never_asks_for_harden() {
        assert!(!build_question_redirect_json().contains("/harden"));
    }
}
