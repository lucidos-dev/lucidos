//! `lucidos chatgpt-plan`, Sign in with ChatGPT from a terminal.
//!
//! The same routes Settings uses, so a sign-in started here shows there too.
//! The engine holds the tokens. This command only ever sees the ChatGPT page
//! the browser must open.

use std::time::Duration;

use crate::http::{client, send_expect_json};
use crate::workspace::{BoxError, Workspace};

/// How often `sign-in` asks whether the browser has come back yet.
const POLL_INTERVAL: Duration = Duration::from_secs(2);

fn status_url(ws: &Workspace) -> String {
    format!("{}/api/v1/providers/chatgpt-plan", ws.base_url())
}

fn fetch_status(ws: &Workspace) -> Result<serde_json::Value, BoxError> {
    let url = status_url(ws);
    send_expect_json("GET", &url, client()?.get(&url))
}

fn text<'a>(value: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    value[key].as_str()
}

fn signed_in_as(account: &serde_json::Value) -> &str {
    text(account, "email").unwrap_or("an account with no email")
}

/// `lucidos chatgpt-plan status`: who is signed in, any sign-in in progress,
/// and whether the plan's usage limit has paused requests.
pub(crate) fn cmd_status(ws: &Workspace, json: bool) -> Result<(), BoxError> {
    let status = fetch_status(ws)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&status)?);
        return Ok(());
    }
    let account = &status["account"];
    if account.is_object() {
        println!("Signed in with ChatGPT as {}.", signed_in_as(account));
    } else if status["sign_in_again"] == serde_json::Value::Bool(true) {
        println!("OpenAI ended the sign-in. Sign in again.");
    } else {
        println!("Not signed in with ChatGPT.");
    }
    if let Some(until) = text(&status, "paused_until") {
        println!("The plan's usage limit is reached. Lucidos pauses plan requests until {until}.");
    }
    let sign_in = &status["sign_in"];
    match text(sign_in, "state") {
        Some("pending") => println!(
            "A sign-in is waiting for the browser until {}.",
            text(sign_in, "expires_at").unwrap_or("?")
        ),
        Some("failed") => println!(
            "The last sign-in failed: {}",
            text(sign_in, "error").unwrap_or("no reason given")
        ),
        _ => {}
    }
    Ok(())
}

/// `lucidos chatgpt-plan sign-in`: start a sign-in, show the ChatGPT page to
/// open, then wait until the browser comes back or the sign-in ends.
pub(crate) fn cmd_sign_in(ws: &Workspace) -> Result<(), BoxError> {
    let start_url = format!("{}/sign-in", status_url(ws));
    let started = send_expect_json("POST", &start_url, client()?.post(&start_url))?;
    println!(
        "Open this page in a browser on this computer:\n{}",
        text(&started, "authorize_url").unwrap_or("?")
    );
    println!("Waiting for ChatGPT. Press Ctrl-C to stop waiting; the sign-in keeps running.");
    loop {
        std::thread::sleep(POLL_INTERVAL);
        let status = fetch_status(ws)?;
        let sign_in = &status["sign_in"];
        match text(sign_in, "state") {
            Some("pending") => continue,
            Some("failed") => {
                return Err(format!(
                    "The sign-in failed: {}",
                    text(sign_in, "error").unwrap_or("no reason given")
                )
                .into())
            }
            _ => {}
        }
        let account = &status["account"];
        return if account.is_object() {
            println!("Signed in with ChatGPT as {}.", signed_in_as(account));
            Ok(())
        } else {
            Err("The sign-in stopped before it finished. Run sign-in again.".into())
        };
    }
}

/// `lucidos chatgpt-plan sign-out [--forget-account]`: forget the sign-in,
/// and stop one in progress. `--forget-account` forgets the account too.
pub(crate) fn cmd_sign_out(ws: &Workspace, forget_account: bool) -> Result<(), BoxError> {
    let url = format!(
        "{}/sign-out?forget_account={forget_account}",
        status_url(ws)
    );
    let parsed = send_expect_json("POST", &url, client()?.post(&url))?;
    let signed_out = parsed["signed_out"].as_bool() == Some(true);
    let forgotten = parsed["account_forgotten"].as_bool() == Some(true);
    println!("{}", sign_out_sentence(signed_out, forgotten));
    Ok(())
}

fn sign_out_sentence(signed_out: bool, forgotten: bool) -> &'static str {
    match (signed_out, forgotten) {
        (true, true) => {
            "Signed out of ChatGPT and forgot the account. Any ChatGPT account may sign in next."
        }
        (false, true) => "Forgot the ChatGPT account. Any ChatGPT account may sign in next.",
        (true, false) => "Signed out of ChatGPT.",
        (false, false) => "Lucidos was not signed in with ChatGPT.",
    }
}
