use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::workspace::BoxError;

pub(crate) const SESSION_KIND_ENV: &str = "LUCIDOS_SESSION_KIND";

pub(crate) const SESSION_KIND_INTERACTIVE: &str = "interactive";

#[expect(
    clippy::disallowed_methods,
    reason = "an empty marker that must outlive this process; `write_sentinel` creates it \
              exclusively, so a planted symlink is refused"
)]
fn cc_sentinel_path(kind: &str, key: &str) -> PathBuf {
    std::env::temp_dir().join(format!("lucidos-cc-{}-{}", kind, key))
}

#[derive(Debug, Deserialize)]
pub(crate) struct StopHookPayload {
    pub(crate) transcript_path: String,
}

pub(crate) fn parse_stop_hook_payload(raw: &str) -> Result<StopHookPayload, BoxError> {
    serde_json::from_str(raw).map_err(Into::into)
}

pub(crate) fn detect_plaintext_question(
    transcript_path: &Path,
) -> Result<Option<String>, BoxError> {
    let file = std::fs::File::open(transcript_path)
        .map_err(|e| format!("open transcript {}: {}", transcript_path.display(), e))?;
    let reader = BufReader::new(file);

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

    let last_text_idx = content
        .iter()
        .enumerate()
        .rev()
        .find(|(_, b)| b.get("type").and_then(|t| t.as_str()) == Some("text"))
        .map(|(i, _)| i);
    let Some(idx) = last_text_idx else {
        return Ok(None);
    };

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

fn redirect_question(session_kind: Option<&str>, input: &str) -> Option<String> {
    if session_kind != Some(SESSION_KIND_INTERACTIVE) {
        return None;
    }
    let payload = parse_stop_hook_payload(input).ok()?;
    detect_plaintext_question(Path::new(&payload.transcript_path))
        .ok()
        .flatten()
}

pub(crate) fn run() -> Result<(), BoxError> {
    use std::io::Read;
    let mut stdin_buf = String::new();
    let _ = std::io::stdin().lock().read_to_string(&mut stdin_buf);

    let session_kind = std::env::var(SESSION_KIND_ENV).ok();
    let question_uuid = redirect_question(session_kind.as_deref(), &stdin_buf);
    let Some(message_uuid) = question_uuid else {
        return Ok(());
    };
    let sentinel = question_sentinel_path(&message_uuid);
    if !sentinel.exists() {
        write_sentinel(&sentinel);
        println!("{}", build_question_redirect_json());
    }
    Ok(())
}

fn write_sentinel(path: &Path) {
    let created = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path);
    if let Err(e) = created.map(drop).or_else(|e| match e.kind() {
        std::io::ErrorKind::AlreadyExists => Ok(()),
        _ => Err(e),
    }) {
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
    fn integrated_sub_thread_report_does_not_redirect_to_an_owner_card() {
        let (_dir, path) = write_transcript(&[serde_json::json!({
            "type": "assistant",
            "uuid": "parent-decision",
            "message": {
                "role": "assistant",
                "stop_reason": "end_turn",
                "content": [{"type": "text", "text": "Parent decision: should I use A or B?"}]
            }
        })]);
        let input = serde_json::json!({ "transcript_path": path }).to_string();

        assert_eq!(redirect_question(None, &input), None);
        assert_eq!(
            redirect_question(Some(SESSION_KIND_INTERACTIVE), &input).as_deref(),
            Some("parent-decision")
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
        let a = question_sentinel_path("aaaa");
        let b = question_sentinel_path("bbbb");
        assert_ne!(a, b);
        assert!(a.to_string_lossy().contains("aaaa"));
        assert!(b.to_string_lossy().contains("bbbb"));
    }

    #[test]
    fn the_hook_never_asks_for_harden() {
        assert!(!build_question_redirect_json().contains("/harden"));
    }
}
