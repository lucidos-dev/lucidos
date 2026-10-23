//! `lucidos main-check status`: where the *main check* stands (ADR 0464).
//! Wraps `GET /api/v1/main-check`.

use crate::http::{client as http_client, send_expect_json};
use crate::workspace::{BoxError, Workspace};
use serde_json::Value;

/// Print the main check's state. `json` prints the engine's snapshot as is.
pub(crate) fn cmd_status(ws: &Workspace, json: bool) -> Result<(), BoxError> {
    let url = format!("{}/api/v1/main-check", ws.base_url());
    let snapshot = send_expect_json("GET", &url, http_client()?.get(&url))?;
    if json {
        println!("{snapshot}");
        return Ok(());
    }
    let workspace = ws
        .root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    print!("{}", status_text(&snapshot, &workspace));
    Ok(())
}

/// The snapshot in words, one fact per line.
fn status_text(snapshot: &Value, workspace: &str) -> String {
    let field = |name: &str| snapshot.get(name).and_then(Value::as_str).unwrap_or("");
    let range = format!("{}..{}", short(field("from_sha")), short(field("to_sha")));
    let mut lines = match field("state") {
        "running" => vec![format!(
            "Main check: running on {}, covering {range}.",
            short(field("to_sha"))
        )],
        "passed" => vec![format!(
            "Main check: passing at {}.",
            short(field("to_sha"))
        )],
        "red" => vec![
            format!("Main is red: {}", field("summary")),
            format!("Range: {range}"),
        ],
        "unavailable" => vec![format!(
            "Main check: unavailable here, {}.",
            field("summary")
        )],
        _ => vec!["Main check: idle. No check runs and main is not red.".to_string()],
    };
    if !field("run_url").is_empty() {
        lines.push(format!("Run: {}", field("run_url")));
    }
    if field("state") == "red" && !field("fix_thread_id").is_empty() {
        lines.push(format!(
            "Fix thread: [Fix main](thread:{workspace}/{})",
            field("fix_thread_id")
        ));
    }
    lines.iter().map(|l| format!("{l}\n")).collect()
}

/// A sha as a reader scans it.
fn short(sha: &str) -> &str {
    &sha[..sha.len().min(12)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn red_names_the_summary_range_run_and_fix_thread() {
        let text = status_text(
            &json!({
                "state": "red",
                "check_id": "c",
                "from_sha": "aaaaaaaaaaaaaaaa",
                "to_sha": "bbbbbbbbbbbbbbbb",
                "run_url": "https://example.invalid/runs/1",
                "fix_thread_id": "6fa459ea-ee8a-3ca4-894e-db77e160355e",
                "summary": "Failed: engine.",
            }),
            "myws",
        );
        assert_eq!(
            text,
            "Main is red: Failed: engine.\n\
             Range: aaaaaaaaaaaa..bbbbbbbbbbbb\n\
             Run: https://example.invalid/runs/1\n\
             Fix thread: [Fix main](thread:myws/6fa459ea-ee8a-3ca4-894e-db77e160355e)\n"
        );
    }

    #[test]
    fn every_other_state_reads_as_one_line_with_its_run() {
        let running = status_text(
            &json!({"state": "running", "from_sha": "a1", "to_sha": "b2", "run_url": "https://r"}),
            "myws",
        );
        assert_eq!(
            running,
            "Main check: running on b2, covering a1..b2.\nRun: https://r\n"
        );
        assert_eq!(
            status_text(&json!({"state": "passed", "to_sha": "b2"}), "myws"),
            "Main check: passing at b2.\n"
        );
        assert_eq!(
            status_text(
                &json!({"state": "unavailable", "summary": "there is no 'lucidos' remote"}),
                "myws"
            ),
            "Main check: unavailable here, there is no 'lucidos' remote.\n"
        );
        assert_eq!(
            status_text(&json!({"state": "idle"}), "myws"),
            "Main check: idle. No check runs and main is not red.\n"
        );
    }
}
