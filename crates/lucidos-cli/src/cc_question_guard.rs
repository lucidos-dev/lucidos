//! PreToolUse hook on every Claude Code tool: while the agent's own question
//! is open, its other tool calls are refused (ADR 0803).
//!
//! The question is open from its `tool_use` until its `tool_result` or the
//! turn's end, read from the session transcript. Claude Code writes each block
//! to the transcript before it runs that block's tool. So a sibling that runs
//! beside the question always finds the question there, and no engine call is
//! needed. A sibling streamed before the question ran before it was asked.
//!
//! The engine passes the refusal and the question tool names as arguments, so
//! each keeps one definition. Fails OPEN on any parse or I/O error: a hook bug
//! must not brick every tool call.

use serde::Deserialize;
use std::collections::HashSet;
use std::io::{Read, Seek, SeekFrom};

use crate::workspace::BoxError;

/// How much of the transcript's end the guard reads. A turn's open question
/// sits in its last few blocks, far inside this.
const TAIL_BYTES: u64 = 512 * 1024;

#[derive(Debug, Deserialize)]
struct HookPayload {
    #[serde(default)]
    tool_name: String,
    #[serde(default)]
    tool_use_id: String,
    #[serde(default)]
    transcript_path: Option<String>,
}

pub(crate) fn run(refusal: &str, question_tools: &[String]) -> Result<(), BoxError> {
    let mut raw = String::new();
    std::io::stdin().read_to_string(&mut raw)?;
    let Ok(payload) = serde_json::from_str::<HookPayload>(&raw) else {
        return Ok(());
    };
    if question_tools.contains(&payload.tool_name) {
        return Ok(());
    }
    let Some(path) = payload.transcript_path.as_deref() else {
        return Ok(());
    };
    let Ok(tail) = read_tail(path) else {
        return Ok(());
    };
    if question_open(&tail, question_tools, &payload.tool_use_id) {
        println!("{}", deny_json(refusal));
    }
    Ok(())
}

fn read_tail(path: &str) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let len = file.metadata()?.len();
    let start = len.saturating_sub(TAIL_BYTES);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    let text = String::from_utf8_lossy(&bytes).into_owned();
    // A cut first line is a fragment, never a whole block.
    Ok(match (start > 0, text.split_once('\n')) {
        (true, Some((_, rest))) => rest.to_string(),
        _ => text,
    })
}

/// Whether a question tool call in `transcript` has no result yet, in the
/// current turn, other than `own_id`. A typed user prompt starts a new turn,
/// which closes every earlier question.
fn question_open(transcript: &str, question_tools: &[String], own_id: &str) -> bool {
    let mut open: HashSet<String> = HashSet::new();
    for line in transcript.lines() {
        let Ok(row) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let content = &row["message"]["content"];
        match row["type"].as_str() {
            Some("assistant") => {
                for block in content.as_array().into_iter().flatten() {
                    let named = block["name"]
                        .as_str()
                        .is_some_and(|n| question_tools.iter().any(|q| q == n));
                    if block["type"] == "tool_use" && named {
                        if let Some(id) = block["id"].as_str() {
                            open.insert(id.to_string());
                        }
                    }
                }
            }
            Some("user") => {
                let blocks = content.as_array();
                let results: Vec<&str> = blocks
                    .into_iter()
                    .flatten()
                    .filter(|b| b["type"] == "tool_result")
                    .filter_map(|b| b["tool_use_id"].as_str())
                    .collect();
                if !results.is_empty() {
                    for id in results {
                        open.remove(id);
                    }
                } else if row["isMeta"] != true && (content.is_string() || blocks.is_some()) {
                    open.clear();
                }
            }
            _ => {}
        }
    }
    open.remove(own_id);
    !open.is_empty()
}

fn deny_json(reason: &str) -> String {
    serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "deny",
            "permissionDecisionReason": reason,
        }
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tools() -> Vec<String> {
        vec!["AskUserQuestion".into(), "mcp__lucidos__ask".into()]
    }

    fn assistant_uses(uses: &[(&str, &str)]) -> String {
        let blocks: Vec<_> = uses
            .iter()
            .map(|(id, name)| serde_json::json!({"type": "tool_use", "id": id, "name": name}))
            .collect();
        serde_json::json!({"type": "assistant", "message": {"content": blocks}}).to_string()
    }

    fn result_for(id: &str) -> String {
        serde_json::json!({"type": "user", "message": {"content": [
            {"type": "tool_result", "tool_use_id": id, "content": "ok"}
        ]}})
        .to_string()
    }

    fn prompt(text: &str) -> String {
        serde_json::json!({"type": "user", "message": {"content": text}}).to_string()
    }

    fn transcript(lines: &[String]) -> String {
        lines.join("\n")
    }

    /// The sibling race: the question streamed first and runs, then a sibling
    /// streams and starts beside it. Its hook finds the question unanswered.
    #[test]
    fn a_sibling_after_an_unanswered_question_is_refused() {
        let t = transcript(&[
            prompt("go"),
            assistant_uses(&[("q1", "AskUserQuestion")]),
            assistant_uses(&[("r1", "Read")]),
        ]);
        assert!(question_open(&t, &tools(), "r1"));
    }

    /// Each block is its own transcript line in one message, or one line with
    /// both blocks: either way the sibling is refused.
    #[test]
    fn a_sibling_in_the_same_line_is_refused() {
        let t = transcript(&[assistant_uses(&[
            ("q1", "mcp__lucidos__ask"),
            ("r1", "Grep"),
        ])]);
        assert!(question_open(&t, &tools(), "r1"));
    }

    /// A sibling streamed before the question started before it was asked.
    #[test]
    fn a_sibling_before_the_question_runs() {
        let t = transcript(&[prompt("go"), assistant_uses(&[("r1", "Read")])]);
        assert!(!question_open(&t, &tools(), "r1"));
    }

    #[test]
    fn an_answered_question_closes() {
        let t = transcript(&[
            assistant_uses(&[("q1", "AskUserQuestion")]),
            result_for("q1"),
            assistant_uses(&[("b1", "Bash")]),
        ]);
        assert!(!question_open(&t, &tools(), "b1"));
    }

    /// A question left unanswered by an ended turn closes with that turn.
    #[test]
    fn a_new_turn_closes_an_earlier_question() {
        let t = transcript(&[
            assistant_uses(&[("q1", "AskUserQuestion")]),
            prompt("next"),
            assistant_uses(&[("b1", "Bash")]),
        ]);
        assert!(!question_open(&t, &tools(), "b1"));
    }

    /// A meta row Claude Code adds mid-turn is not the owner's prompt.
    #[test]
    fn a_meta_row_keeps_the_question_open() {
        let meta = serde_json::json!({"type": "user", "isMeta": true,
            "message": {"content": "reminder"}})
        .to_string();
        let t = transcript(&[assistant_uses(&[("q1", "AskUserQuestion")]), meta]);
        assert!(question_open(&t, &tools(), "b1"));
    }

    #[test]
    fn the_question_itself_is_never_refused() {
        let t = transcript(&[assistant_uses(&[("q1", "AskUserQuestion")])]);
        assert!(!question_open(&t, &tools(), "q1"));
    }

    #[test]
    fn a_cut_first_line_is_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.jsonl");
        let filler = "x".repeat(TAIL_BYTES as usize);
        let body = format!(
            "{filler}\n{}\n",
            assistant_uses(&[("q1", "AskUserQuestion")])
        );
        std::fs::write(&path, body).unwrap();
        let tail = read_tail(path.to_str().unwrap()).unwrap();
        assert!(question_open(&tail, &tools(), "r1"));
    }

    #[test]
    fn the_refusal_is_a_valid_deny() {
        let parsed: serde_json::Value = serde_json::from_str(&deny_json("wait")).unwrap();
        assert_eq!(parsed["hookSpecificOutput"]["permissionDecision"], "deny");
        assert_eq!(
            parsed["hookSpecificOutput"]["permissionDecisionReason"],
            "wait"
        );
    }
}
