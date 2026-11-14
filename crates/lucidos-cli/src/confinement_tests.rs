//! `lucidos confinement-tests run`: a coding agent's *confinement run*, the
//! confinement tests run by the engine outside the session sandbox.
//!
//! The engine owns the command and the permission check, over
//! `POST /api/v1/threads/<id>/confinement-tests`. With no permission it raises
//! the owner's card. Any later step on the thread overtakes that card and drops
//! its buttons. So this command holds its own tool call open, and asks again
//! with the card's id until the owner answers. Once the run starts it follows
//! the background task through the wait route and exits with its status.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::background_task::{calling_thread, tasks_url, WAIT_RESPONSE_DEADLINE};
use crate::http::{client as http_client, client_with_timeout, send_or_refusal};
use crate::workspace::{BoxError, Workspace};

/// `EX_TEMPFAIL`: nothing ran, and a later try may. No card answered, a card
/// closed, or no permission and no card asked for.
pub(crate) const NOT_RUN: u8 = 75;

/// How often a held call asks whether the owner answered the card.
const CARD_POLL_INTERVAL: Duration = Duration::from_secs(3);

/// What the route's answer says to do next.
#[derive(Debug, PartialEq)]
enum Step {
    Follow { task_id: String },
    Hold { card_id: String },
    Stop(u8),
}

fn run_body(card_id: Option<&str>, ask_owner: bool) -> Value {
    json!({ "card_id": card_id, "ask_owner": ask_owner })
}

fn step_for(answer: &Value) -> Result<Step, BoxError> {
    let field = |name: &str| {
        answer[name]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| format!("The engine answered without `{name}`: {answer}"))
    };
    Ok(match answer["state"].as_str() {
        Some("started") => Step::Follow {
            task_id: field("task_id")?,
        },
        Some("asked") => Step::Hold {
            card_id: field("card_id")?,
        },
        Some("closed" | "unpermitted") => Step::Stop(NOT_RUN),
        _ => return Err(format!("Unexpected answer from the engine: {answer}").into()),
    })
}

/// The task's own exit code, or 1 when it has none: killed, timed out, or
/// ended by a signal.
fn task_exit_code(record: &Value) -> u8 {
    match record["exit_code"].as_i64() {
        Some(0) => 0,
        Some(code) => u8::try_from(code).ok().filter(|c| *c != 0).unwrap_or(1),
        None => 1,
    }
}

fn parse_json(url: &str, text: &str) -> Result<Value, BoxError> {
    serde_json::from_str(text).map_err(|e| format!("Unexpected response from {url}: {e}").into())
}

/// `hold_secs` 0 raises no card: a call that cannot hold one open must not
/// leave the owner a card that dies under the next step.
pub(crate) fn cmd_run(ws: &Workspace, hold_secs: u64) -> Result<u8, BoxError> {
    let thread_id = calling_thread()?;
    let url = format!(
        "{}/api/v1/threads/{}/confinement-tests",
        ws.base_url(),
        thread_id
    );
    let hold_until = Instant::now() + Duration::from_secs(hold_secs);
    let mut waiting_on: Option<String> = None;
    loop {
        let body = run_body(waiting_on.as_deref(), hold_secs > 0);
        let request = http_client()?.post(&url).json(&body);
        let answer = match send_or_refusal("POST", &url, request)? {
            Ok(text) => parse_json(&url, &text)?,
            Err(refusal) => {
                eprintln!("lucidos: {refusal}");
                return Ok(1);
            }
        };
        let step = step_for(&answer)?;
        if waiting_on.is_none() || !matches!(step, Step::Hold { .. }) {
            println!("{}", answer["message"].as_str().unwrap_or_default());
        }
        match step {
            Step::Follow { task_id } => return follow(ws, &thread_id, &task_id),
            Step::Stop(code) => return Ok(code),
            Step::Hold { card_id } => {
                if Instant::now() >= hold_until {
                    println!(
                        "The owner has not answered card {card_id} yet, so nothing ran. \
                         Your next step closes it. Run `lucidos confinement-tests run` \
                         again to ask again."
                    );
                    return Ok(NOT_RUN);
                }
                waiting_on = Some(card_id);
                std::thread::sleep(CARD_POLL_INTERVAL);
            }
        }
    }
}

/// Print the task's output as it arrives, until it finishes.
fn follow(ws: &Workspace, thread_id: &str, task_id: &str) -> Result<u8, BoxError> {
    println!(
        "If this call ends first, the run goes on: follow it with \
         `lucidos background-task wait {task_id}`."
    );
    let url = format!("{}/{}/wait", tasks_url(ws, thread_id), task_id);
    loop {
        let request = client_with_timeout(WAIT_RESPONSE_DEADLINE)?.get(&url);
        let record = match send_or_refusal("GET", &url, request)? {
            Ok(text) => parse_json(&url, &text)?,
            Err(refusal) => {
                eprintln!("lucidos: {refusal}");
                return Ok(1);
            }
        };
        print_output(&record);
        if record["finished"] == Value::Bool(true) {
            println!(
                "Confinement run {task_id} finished: {}",
                record["status"].as_str().unwrap_or("no exit status")
            );
            return Ok(task_exit_code(&record));
        }
    }
}

/// Each wait drains what arrived since the last one, so nothing repeats.
/// A child report the wait carried prints too: the wait consumed it.
fn print_output(record: &Value) {
    if let Some(out) = record["stdout"].as_str().filter(|s| !s.is_empty()) {
        print!("{out}");
    }
    if let Some(err) = record["stderr"].as_str().filter(|s| !s.is_empty()) {
        eprint!("{err}");
    }
    if let Some(reports) = record.get("child_reports") {
        println!("Child reports the wait carried: {reports}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Only a call that can hold the card open asks the owner.
    #[test]
    fn the_body_asks_the_owner_only_when_it_holds() {
        assert_eq!(
            run_body(None, true),
            json!({ "card_id": null, "ask_owner": true })
        );
        assert_eq!(run_body(None, false)["ask_owner"], false);
        assert_eq!(run_body(Some("card-1"), true)["card_id"], "card-1");
    }

    #[test]
    fn started_follows_the_task() {
        let answer = json!({ "state": "started", "task_id": "bg-1", "message": "" });
        assert_eq!(
            step_for(&answer).unwrap(),
            Step::Follow {
                task_id: "bg-1".into()
            }
        );
    }

    #[test]
    fn asked_holds_the_card() {
        let answer = json!({ "state": "asked", "card_id": "card-1", "message": "" });
        assert_eq!(
            step_for(&answer).unwrap(),
            Step::Hold {
                card_id: "card-1".into()
            }
        );
    }

    /// Nothing ran, so neither answer maps to success.
    #[test]
    fn closed_and_unpermitted_exit_not_run() {
        for state in ["closed", "unpermitted"] {
            let answer = json!({ "state": state, "card_id": "card-1", "message": "" });
            assert_eq!(step_for(&answer).unwrap(), Step::Stop(NOT_RUN), "{state}");
        }
    }

    #[test]
    fn an_unknown_or_incomplete_answer_is_an_error() {
        assert!(step_for(&json!({ "state": "maybe" })).is_err());
        assert!(step_for(&json!({ "state": "started" })).is_err());
        assert!(step_for(&json!({})).is_err());
    }

    /// A run that did not report success never exits 0.
    #[test]
    fn the_exit_code_follows_the_task() {
        assert_eq!(task_exit_code(&json!({ "exit_code": 0 })), 0);
        assert_eq!(task_exit_code(&json!({ "exit_code": 101 })), 101);
        assert_eq!(task_exit_code(&json!({ "exit_code": 256 })), 1);
        assert_eq!(task_exit_code(&json!({ "exit_code": -1 })), 1);
        assert_eq!(
            task_exit_code(&json!({ "exit_code": null, "killed": true })),
            1
        );
        assert_eq!(task_exit_code(&json!({ "timed_out": true })), 1);
    }
}
