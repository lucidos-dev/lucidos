use crate::gateway;
use crate::http::{
    client, client_with_timeout, format_request_error, send_and_print, send_expect_json,
    ConnectRetry,
};
use crate::permission_request::{
    permission_text, reply, send_act, Reply, EXIT_NEEDS_PERMISSION, WAITING_FOR_OWNER_TEXT,
};
use crate::workspace::{BoxError, Workspace};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::PathBuf;
use std::time::{Duration, Instant};

const BROKER_CALL_DEADLINE: Duration = Duration::from_secs(120);

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

pub(crate) fn find<'a>(rows: &'a [ListedWorkspace], name: &str) -> Option<&'a ListedWorkspace> {
    rows.iter()
        .find(|w| w.name == name)
        .or_else(|| rows.iter().find(|w| w.id == name))
}

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

fn list_line(row: &ListedWorkspace) -> String {
    let autostart = if row.autostart { "on" } else { "off" };
    format!(
        "{}\t{}\t{}\t{}\t{autostart}\t{}\t{}",
        row.id,
        row.name,
        row.port,
        row.health,
        row.origin,
        row.path.display()
    )
}

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

pub(crate) fn segment(raw: &str) -> String {
    raw.bytes()
        .map(|b| match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' => (b as char).to_string(),
            other => format!("%{other:02X}"),
        })
        .collect()
}

pub(crate) fn run(ws: &Workspace, call: BrokerCall) -> Result<u8, BoxError> {
    let url = format!("{}/api/v1{}", ws.base_url(), call.path);
    send_act(
        client_with_timeout(BROKER_CALL_DEADLINE)?,
        reqwest::Method::POST,
        &url,
        &call.body,
    )
}

pub(crate) fn device_session(
    ws: &Workspace,
    slug: &str,
    device: &str,
    ttl_minutes: Option<u64>,
    reason: &str,
    out: &std::path::Path,
) -> Result<u8, BoxError> {
    let ttl_secs = ttl_minutes
        .map(|minutes| {
            minutes
                .checked_mul(60)
                .ok_or("Device session lifetime overflow")
        })
        .transpose()?;
    let url = format!(
        "{}/api/v1/workspaces/{}/device-session",
        ws.base_url(),
        segment(slug)
    );
    let started = Instant::now();
    let response = client_with_timeout(BROKER_CALL_DEADLINE)?
        .post(&url)
        .json(&json!({"device_id": device, "ttl_secs": ttl_secs, "reason": reason}))
        .send_with_connect_retry()
        .map_err(|e| format_request_error("POST", &url, &e, started.elapsed()))?;
    let status = response.status().as_u16();
    let body = response
        .text()
        .map_err(|_| "Failed to read device session response")?;
    match reply("POST", &url, status, &body)? {
        Reply::NeedsPermission(id) => {
            println!("{}", permission_text(&id));
            Ok(EXIT_NEEDS_PERMISSION)
        }
        Reply::WaitingForOwner => {
            println!("{WAITING_FOR_OWNER_TEXT}");
            Ok(EXIT_NEEDS_PERMISSION)
        }
        Reply::Done(body) => {
            println!("{}", save_device_session(&body, out)?);
            Ok(0)
        }
    }
}

fn save_device_session(body: &str, out: &std::path::Path) -> Result<String, BoxError> {
    use std::io::Write as _;
    let body: serde_json::Value =
        serde_json::from_str(body).map_err(|_| "Invalid device session response")?;
    let session = body["session"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or("Device session response has no session")?;
    let expires = body["expires_at"]
        .as_u64()
        .ok_or("Device session response has no expiry")?;
    let parent = out
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| std::path::Path::new("."));
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    file.write_all(session.as_bytes())?;
    file.as_file().sync_all()?;
    file.persist(out).map_err(|e| e.error)?;
    Ok(format!(
        "Device session saved; expires at Unix time {expires}."
    ))
}

pub(crate) fn trash(ws: &Workspace) -> Result<(), BoxError> {
    let url = format!("{}/api/v1/workspaces/trash", ws.base_url());
    send_and_print("GET", &url, client()?.get(&url))
}

pub(crate) fn classify(
    ws: &Workspace,
    slugs: &[String],
    origin: &str,
    evidence: &str,
) -> Result<(), BoxError> {
    crate::permission_request::cmd_batch(
        ws,
        json!([{
            "kind": "workspace_classification",
            "workspaces": slugs,
            "origin": origin,
            "evidence": evidence,
        }]),
    )
}

pub(crate) fn delete(ws: &Workspace, slugs: &[String]) -> Result<(), BoxError> {
    crate::permission_request::cmd_batch(
        ws,
        json!([{ "kind": "workspace_deletion", "workspaces": slugs }]),
    )
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
    fn a_list_line_appends_the_gateway_reported_directory() {
        let rows = rows();
        assert_eq!(
            list_line(&rows[0]),
            "dev\tdevelopment\t5173\thealthy\ton\tuser\t/home/user/workspaces/dev"
        );
        assert_eq!(
            list_line(&rows[1]),
            "myws\tdev\t5174\tunhealthy\toff\tdemo\t/home/user/elsewhere/myws"
        );
    }

    #[test]
    fn json_listing_keeps_the_gateway_reported_path() {
        let json = serde_json::to_value(rows()).unwrap();
        assert_eq!(json[1]["path"], "/home/user/elsewhere/myws");
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
    #[test]
    fn session_output_is_private_and_never_prints_the_session() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("session");
        std::fs::write(&out, "old").unwrap();
        std::fs::set_permissions(&out, std::fs::Permissions::from_mode(0o644)).unwrap();
        let body = json!({"session": "test-session-value", "expires_at": 12345}).to_string();
        let printed = save_device_session(&body, &out).unwrap();
        assert!(!printed.contains("test-session-value"));
        assert!(printed.contains("12345"));
        assert_eq!(std::fs::read_to_string(&out).unwrap(), "test-session-value");
        assert_eq!(
            std::fs::metadata(&out).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(save_device_session("{}", &out).is_err());
        assert_eq!(std::fs::read_to_string(&out).unwrap(), "test-session-value");
    }
}
