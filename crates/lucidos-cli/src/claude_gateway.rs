//! `lucidos claude-gateway`, the Claude gateway sign-in from a terminal.
//!
//! The same routes Settings uses, so a sign-in started here shows there too.
//! The engine holds the tokens. This command only ever sees the user code.

use std::time::Duration;

use crate::http::{client, send_expect_json};
use crate::workspace::{BoxError, Workspace};

/// How often `sign-in` asks whether the user has approved yet.
const POLL_INTERVAL: Duration = Duration::from_secs(2);

fn status_url(ws: &Workspace) -> String {
    format!("{}/api/v1/providers/claude-gateway", ws.base_url())
}

fn fetch_status(ws: &Workspace) -> Result<serde_json::Value, BoxError> {
    let url = status_url(ws);
    send_expect_json("GET", &url, client()?.get(&url))
}

fn text<'a>(value: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    value[key].as_str()
}

/// `lucidos claude-gateway status`: the URL, who is signed in, and any sign-in
/// in progress.
pub(crate) fn cmd_status(ws: &Workspace, json: bool) -> Result<(), BoxError> {
    let status = fetch_status(ws)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&status)?);
        return Ok(());
    }
    match (text(&status, "url"), text(&status, "suggested_url")) {
        (Some(url), _) => println!("Gateway URL: {url}"),
        (None, Some(suggested)) => println!(
            "No Claude gateway URL is set. Claude Code's managed settings name {suggested}: \
             sign in with --url {suggested} to use it."
        ),
        (None, None) => println!("No Claude gateway URL is set."),
    }
    let account = &status["account"];
    match text(account, "issuer") {
        Some(issuer) => {
            println!(
                "Signed in to {issuer} as {}.",
                text(account, "email").unwrap_or("an account with no email")
            );
            if status["telemetry"] == serde_json::Value::Bool(true) {
                println!(
                    "Your organisation's gateway collects usage telemetry for your account: \
                     model, tokens, cost and timing per thread, never content."
                );
            }
        }
        None if status["sign_in_again"] == serde_json::Value::Bool(true) => {
            println!("The gateway ended the sign-in. Sign in again.")
        }
        None => println!("Not signed in."),
    }
    let sign_in = &status["sign_in"];
    match text(sign_in, "state") {
        Some("pending") => print_code(sign_in),
        Some("failed") => println!(
            "The last sign-in failed: {}",
            text(sign_in, "error").unwrap_or("no reason given")
        ),
        _ => {}
    }
    Ok(())
}

fn print_code(pending: &serde_json::Value) {
    let page = text(pending, "verification_uri_complete")
        .or_else(|| text(pending, "verification_uri"))
        .unwrap_or("?");
    println!(
        "Open {page} and enter the code {}.",
        text(pending, "user_code").unwrap_or("?")
    );
}

/// `lucidos claude-gateway sign-in`: start a device sign-in, storing `url` as
/// the gateway URL first when given. Shows the code, then waits until the
/// user approves or the sign-in ends.
pub(crate) fn cmd_sign_in(ws: &Workspace, url: Option<&str>) -> Result<(), BoxError> {
    let start_url = format!("{}/sign-in", status_url(ws));
    let req = client()?
        .post(&start_url)
        .json(&serde_json::json!({ "url": url }));
    let pending = send_expect_json("POST", &start_url, req)?;
    print_code(&pending);
    println!("Waiting for approval. Press Ctrl-C to stop waiting; the sign-in keeps running.");
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
        return match text(&status["account"], "issuer") {
            Some(issuer) => {
                println!("Signed in to {issuer}.");
                Ok(())
            }
            None => Err("The sign-in stopped before it finished. Run sign-in again.".into()),
        };
    }
}

/// `lucidos claude-gateway sign-out`: forget the sign-in, and stop one in
/// progress.
pub(crate) fn cmd_sign_out(ws: &Workspace) -> Result<(), BoxError> {
    let url = format!("{}/sign-out", status_url(ws));
    let parsed = send_expect_json("POST", &url, client()?.post(&url))?;
    match parsed["signed_out"].as_bool() {
        Some(true) => println!("Signed out of the Claude gateway."),
        _ => println!("Lucidos was not signed in to a Claude gateway."),
    }
    Ok(())
}
