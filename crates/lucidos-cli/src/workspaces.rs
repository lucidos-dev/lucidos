//! Workspace listings, and the *workspace broker*'s verbs, for agents and
//! terminal users.
//!
//! Every verb asks this workspace's engine, which holds the local token and
//! asks the gateway (ADR 0671). Each mutating call needs the owner's
//! permission: without one the engine records a request and answers 403, and
//! this module prints how to ask the owner with it.

use crate::gateway;
use crate::http::{
    client, client_with_timeout, format_request_error, send_and_print, send_expect_json,
    ConnectRetry,
};
use crate::workspace::{BoxError, Workspace};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// The exit code for a call that waits on the owner's permission.
pub(crate) const EXIT_NEEDS_PERMISSION: u8 = 2;

/// How long a broker call may take. It outlasts the engine's minute for the
/// gateway call. Otherwise a slow create reads as failed while it completes.
const BROKER_CALL_DEADLINE: Duration = Duration::from_secs(120);

/// One row of `GET /api/v1/workspaces`, the engine's projection of its
/// gateway's listing. Asking the engine, not the gateway, is what lets a
/// sandboxed agent list workspaces with no local token (ADR 0645).
///
/// An engine older than this CLI sends no autostart, origin or machine-made
/// flag, so each reads as its safe default rather than failing the parse.
#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub(crate) struct ListedWorkspace {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) path: PathBuf,
    pub(crate) port: u16,
    pub(crate) health: String,
    #[serde(default)]
    pub(crate) autostart: bool,
    #[serde(default = "unknown_origin")]
    pub(crate) origin: String,
    #[serde(default)]
    pub(crate) machine_made: bool,
}

fn unknown_origin() -> String {
    "unknown".into()
}

pub(crate) fn fetch(ws: &Workspace) -> Result<Vec<ListedWorkspace>, BoxError> {
    let url = format!("{}/api/v1/workspaces", ws.base_url());
    let body = send_expect_json("GET", &url, client()?.get(&url))?;
    parse_listing(body)
}

fn parse_listing(body: serde_json::Value) -> Result<Vec<ListedWorkspace>, BoxError> {
    #[derive(Deserialize)]
    struct Listing {
        workspaces: Vec<ListedWorkspace>,
    }
    let listing: Listing =
        serde_json::from_value(body).map_err(|e| format!("Unexpected workspace listing: {e}"))?;
    Ok(listing.workspaces)
}

/// The row a `--to <name>` names: the display name first, then the slug.
pub(crate) fn find<'a>(rows: &'a [ListedWorkspace], name: &str) -> Option<&'a ListedWorkspace> {
    rows.iter()
        .find(|w| w.name == name)
        .or_else(|| rows.iter().find(|w| w.id == name))
}

/// `lucidos workspaces list`: one workspace per line, or the rows as JSON.
pub(crate) fn list(ws: &Workspace, as_json: bool) -> Result<(), BoxError> {
    let rows = fetch(ws)?;
    if as_json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
    } else {
        for row in &rows {
            println!("{}", list_line(row));
        }
    }
    Ok(())
}

/// Slug, name, port, health, autostart and origin, tab-separated.
fn list_line(row: &ListedWorkspace) -> String {
    let autostart = if row.autostart { "on" } else { "off" };
    format!(
        "{}\t{}\t{}\t{}\t{autostart}\t{}",
        row.id, row.name, row.port, row.health, row.origin
    )
}

/// One broker verb: the route under `/api/v1` it POSTs to, and its JSON body.
pub(crate) struct BrokerCall {
    path: String,
    body: serde_json::Value,
}

impl BrokerCall {
    pub(crate) fn create(name: &str, slug: Option<&str>, autostart: bool, reason: &str) -> Self {
        Self::post(
            "/workspaces".into(),
            json!({ "name": name, "slug": slug, "autostart": autostart, "reason": reason }),
        )
    }

    /// `stop`, `start` or `delete` on a workspace slug.
    pub(crate) fn verb(slug: &str, verb: &str, reason: &str) -> Self {
        Self::post(
            format!("/workspaces/{}/{verb}", segment(slug)),
            json!({ "reason": reason }),
        )
    }

    pub(crate) fn autostart(slug: &str, enabled: bool, reason: &str) -> Self {
        Self::post(
            format!("/workspaces/{}/autostart", segment(slug)),
            json!({ "enabled": enabled, "reason": reason }),
        )
    }

    pub(crate) fn rename(slug: &str, name: &str, reason: &str) -> Self {
        Self::post(
            format!("/workspaces/{}/rename", segment(slug)),
            json!({ "name": name, "reason": reason }),
        )
    }

    pub(crate) fn restore(trash_id: &str, reason: &str) -> Self {
        Self::post(
            format!("/workspaces/trash/{}/restore", segment(trash_id)),
            json!({ "reason": reason }),
        )
    }

    fn post(path: String, body: serde_json::Value) -> Self {
        Self { path, body }
    }
}

/// A slug or Trash id as one path segment. The engine validates the value, so
/// this only keeps a stray `/` or `?` from naming another route.
fn segment(raw: &str) -> String {
    raw.bytes()
        .map(|b| match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' => (b as char).to_string(),
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// What a broker call came to.
#[derive(Debug, PartialEq)]
enum Reply {
    Done(String),
    NeedsPermission(String),
}

/// Run one broker verb. Prints the engine's answer, or how to ask the owner,
/// and returns the exit code.
pub(crate) fn run(ws: &Workspace, call: BrokerCall) -> Result<u8, BoxError> {
    let url = format!("{}/api/v1{}", ws.base_url(), call.path);
    let start = Instant::now();
    let resp = client_with_timeout(BROKER_CALL_DEADLINE)?
        .post(&url)
        .json(&call.body)
        .send_with_connect_retry()
        .map_err(|e| format_request_error("POST", &url, &e, start.elapsed()))?;
    let status = resp.status().as_u16();
    let text = resp
        .text()
        .map_err(|e| format!("Failed to read response body: {e}"))?;
    match reply(&url, status, &text)? {
        Reply::Done(body) => {
            println!("{}", if body.is_empty() { "Done." } else { &body });
            Ok(0)
        }
        Reply::NeedsPermission(id) => {
            println!("{}", permission_text(&id));
            Ok(EXIT_NEEDS_PERMISSION)
        }
    }
}

/// Read a broker answer. A refusal shows only the engine's `error` text, never
/// the rest of the body.
fn reply(url: &str, status: u16, text: &str) -> Result<Reply, BoxError> {
    if (200..300).contains(&status) {
        return Ok(Reply::Done(text.trim().to_owned()));
    }
    let body: serde_json::Value = serde_json::from_str(text).unwrap_or_default();
    if status == 403 && body["reason"] == "permission_required" {
        if let Some(id) = body["request_id"].as_str() {
            return Ok(Reply::NeedsPermission(id.to_owned()));
        }
    }
    let error = body["error"].as_str().unwrap_or("no error text");
    Err(format!("POST {url} returned {status}: {error}").into())
}

fn permission_text(request_id: &str) -> String {
    format!(
        "This needs the owner's permission. {}\n\
         The act is this same command: run it again once the owner allows it.",
        crate::permission_request::outcome_text(request_id)
    )
}

/// `lucidos workspaces trash`: the Trash, as the engine projects it.
pub(crate) fn trash(ws: &Workspace) -> Result<(), BoxError> {
    let url = format!("{}/api/v1/workspaces/trash", ws.base_url());
    send_and_print("GET", &url, client()?.get(&url))
}

/// `lucidos workspaces classify`: a batch with one `workspace_classification`
/// item. The owner confirms it on the batch card.
pub(crate) fn classify(
    ws: &Workspace,
    slugs: &[String],
    origin: &str,
    evidence: &str,
) -> Result<(), BoxError> {
    let url = format!("{}/api/v1/permissions/batches", ws.base_url());
    let body = json!({ "items": [{
        "kind": "workspace_classification",
        "workspaces": slugs,
        "origin": origin,
        "evidence": evidence,
    }]});
    send_and_print("POST", &url, client()?.post(&url).json(&body))
}

fn pending_message(result: &serde_json::Value) -> Option<&'static str> {
    if result
        .get("scanned_at")
        .is_none_or(serde_json::Value::is_null)
    {
        Some("Workspace review is pending. Run this command again after the gateway finishes scanning; empty rows do not yet mean there are no workspaces.")
    } else {
        None
    }
}

pub fn review(port: Option<u16>) -> Result<(), BoxError> {
    let client = gateway::client()?;
    let base = gateway::the_gateway(
        &client,
        port,
        "Pass --port to select the gateway whose workspaces you want to review",
    )?;
    let token = lucidos_local_token::read().ok_or("no local token found")?;
    let result: serde_json::Value = client
        .get(format!("{base}/~/api/v1/control/cleanup"))
        .header(lucidos_local_token::HEADER_LOCAL_TOKEN, token)
        .send()?
        .error_for_status()?
        .json()?;
    if let Some(message) = pending_message(&result) {
        eprintln!("{message}");
    }
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_empty_scan_is_pending_but_completed_empty_scan_is_not() {
        assert!(pending_message(
            &serde_json::json!({"scanned_at": null, "refreshing": true, "rows": []})
        )
        .is_some());
        assert!(pending_message(&serde_json::json!({"scanned_at": "2026-01-01T00:00:00Z", "refreshing": false, "rows": []})).is_none());
    }

    fn rows() -> Vec<ListedWorkspace> {
        parse_listing(serde_json::json!({"workspaces": [
            {"id": "dev", "name": "development", "path": "/home/user/workspaces/dev",
             "port": 5173, "health": "healthy", "autostart": true, "origin": "user",
             "machine_made": false},
            {"id": "myws", "name": "dev", "path": "/home/user/elsewhere/myws",
             "port": 5174, "health": "unhealthy", "autostart": false, "origin": "demo",
             "machine_made": true},
        ]}))
        .unwrap()
    }

    #[test]
    fn a_list_line_is_slug_name_port_health_autostart_origin() {
        let rows = rows();
        assert_eq!(
            list_line(&rows[0]),
            "dev\tdevelopment\t5173\thealthy\ton\tuser"
        );
        assert_eq!(list_line(&rows[1]), "myws\tdev\t5174\tunhealthy\toff\tdemo");
    }

    #[test]
    fn a_permission_refusal_yields_its_request_id() {
        let body = r#"{"error": "Ask the owner.", "reason": "permission_required",
                       "request_id": "permission-request:abc"}"#;
        assert_eq!(
            reply("u", 403, body).unwrap(),
            Reply::NeedsPermission("permission-request:abc".into())
        );
        let text = permission_text("permission-request:abc");
        assert!(text.contains("`permission-request:abc`"), "{text}");
        assert!(text.contains("run it again"), "{text}");
    }

    /// A refusal prints the engine's `error` text and nothing else from the
    /// body, so no other field can reach the agent's output.
    #[test]
    fn a_refusal_shows_only_the_error_text() {
        let body = r#"{"error": "That workspace is the user's.", "detail": "x-secret-value"}"#;
        let err = reply("http://h/api/v1/workspaces/dev/delete", 403, body)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("returned 403: That workspace is the user's."),
            "{err}"
        );
        assert!(!err.contains("x-secret-value"), "{err}");
        let err = reply("u", 405, "not json").unwrap_err().to_string();
        assert!(!err.contains("not json"), "{err}");
    }

    #[test]
    fn a_success_with_no_body_is_done() {
        assert_eq!(reply("u", 204, "").unwrap(), Reply::Done(String::new()));
    }

    #[test]
    fn a_target_stays_one_path_segment() {
        assert_eq!(segment("demo-ws_1"), "demo-ws_1");
        assert_eq!(segment("../x?y"), "%2E%2E%2Fx%3Fy");
        assert_eq!(
            BrokerCall::verb("a/b", "delete", "r").path,
            "/workspaces/a%2Fb/delete"
        );
    }

    #[test]
    fn a_display_name_wins_over_another_rows_slug() {
        assert_eq!(find(&rows(), "dev").unwrap().id, "myws");
        assert_eq!(find(&rows(), "development").unwrap().id, "dev");
        assert!(find(&rows(), "absent").is_none());
    }

    #[test]
    fn an_older_engines_row_reads_as_unknown_and_protected() {
        let rows = parse_listing(serde_json::json!({"workspaces": [
            {"id": "dev", "name": "dev", "path": "/home/user/workspaces/dev",
             "port": 5173, "health": "healthy"},
        ]}))
        .unwrap();
        assert_eq!(rows[0].origin, "unknown");
        assert!(!rows[0].machine_made && !rows[0].autostart);
    }

    #[test]
    fn a_listing_without_the_workspaces_array_is_an_error() {
        assert!(parse_listing(serde_json::json!({"error": "nope"})).is_err());
    }
}
