//! Thin snapshot client. Jobs are durable; status can be polled again after
//! a connection loss or an engine restart.
use crate::{
    http,
    workspace::{self, BoxError, Workspace},
};
use clap::{Args, Subcommand};
use serde_json::{json, Value};

#[path = "snapshot_restore.rs"]
mod restore;

#[derive(Args)]
pub(crate) struct Target {
    /// Target workspace name or absolute path; defaults to this workspace.
    #[arg(long)]
    workspace: Option<String>,
    #[arg(long)]
    json: bool,
}

#[derive(Subcommand)]
pub(crate) enum Action {
    /// Request another workspace's restore as the issuing agent. Never retries.
    Restore {
        handle: String,
        /// Exact gateway workspace slug, never a filesystem path.
        #[arg(long)]
        workspace: String,
        /// Must equal --workspace; binds admission to the intended workspace.
        #[arg(long)]
        expected_workspace_id: String,
        #[arg(long)]
        reason: Option<String>,
        /// Poll this restore's gateway journal until it reaches a terminal phase.
        #[arg(long)]
        wait: bool,
        #[arg(long)]
        port: Option<u16>,
        #[arg(long)]
        json: bool,
    },
    /// Read a restore journal even while its workspace engine is stopped.
    RestoreStatus {
        restore_id: uuid::Uuid,
        /// Exact gateway workspace slug, never a filesystem path.
        #[arg(long)]
        workspace: String,
        /// Gateway port; required when several gateways answer.
        #[arg(long)]
        port: Option<u16>,
        #[arg(long)]
        json: bool,
    },
    Create {
        #[arg(long)]
        label: Option<String>,
        #[arg(long)]
        keep: bool,
        #[arg(long)]
        wait: bool,
        #[command(flatten)]
        target: Target,
    },
    List {
        #[command(flatten)]
        target: Target,
    },
    Get {
        handle: String,
        #[command(flatten)]
        target: Target,
    },
    Check {
        handle: String,
        #[command(flatten)]
        target: Target,
    },
    Status {
        job_id: uuid::Uuid,
        #[arg(long)]
        wait: bool,
        #[command(flatten)]
        target: Target,
    },
    Delete {
        handle: String,
        #[command(flatten)]
        target: Target,
    },
}

fn target_workspace(target: &Target) -> Result<Workspace, BoxError> {
    match &target.workspace {
        None => workspace::resolve_from_env(),
        Some(name) => {
            let root = crate::spawn_thread::resolve_target(name)?;
            workspace::resolve(&root, Some(&root))
        }
    }
}

fn handle_path(handle: &str) -> Result<String, BoxError> {
    const ALPHABET: &[u8] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    let suffix = handle
        .strip_prefix("snap_")
        .ok_or("snapshot_handle_invalid")?;
    if suffix.len() != 26
        || !suffix.bytes().all(|b| ALPHABET.contains(&b))
        || suffix.bytes().next().is_none_or(|b| b > b'7')
    {
        return Err("snapshot_handle_invalid".into());
    }
    Ok(format!("/snapshots/{handle}"))
}

fn request(
    ws: &Workspace,
    method: reqwest::Method,
    path: &str,
    body: Option<&Value>,
) -> Result<(u8, Value), BoxError> {
    let client = http::client()?;
    let target = ws
        .root
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or("snapshot_workspace_invalid")?;
    let mut req = client
        .request(method, format!("{}/api/v1{path}", ws.base_url()))
        .header(http::HEADER_TARGET_WORKSPACE, target);
    if let Some(body) = body {
        req = req.json(body);
    }
    let response = req.send().map_err(|_| "snapshot_request_failed")?;
    let status = response.status();
    if status == reqwest::StatusCode::NO_CONTENT {
        return Ok((0, json!({"deleted":true})));
    }
    let body: Value = response.json().map_err(|_| "snapshot_response_failed")?;
    let code = if status.is_success() {
        0
    } else if status == reqwest::StatusCode::CONFLICT {
        75
    } else if status.is_client_error() {
        2
    } else {
        4
    };
    Ok((code, body))
}

fn wait_job(ws: &Workspace, mut body: Value) -> Result<(u8, Value), BoxError> {
    let id = body
        .get("job_id")
        .or_else(|| body.get("id"))
        .and_then(Value::as_str)
        .ok_or("snapshot_job_id_missing")?
        .parse::<uuid::Uuid>()?;
    while body.get("state").is_none() || body["state"]["phase"] == "running" {
        std::thread::sleep(std::time::Duration::from_secs(1));
        let (code, next) = request(
            ws,
            reqwest::Method::GET,
            &format!("/snapshots/jobs/{id}"),
            None,
        )?;
        if code != 0 {
            return Ok((code, next));
        }
        body = next;
    }
    let code = if body["state"]["phase"] == "completed" {
        0
    } else {
        4
    };
    Ok((code, body))
}

pub(crate) fn run(action: Action) -> Result<u8, BoxError> {
    let (target, method, path, body, wait) = match action {
        Action::Restore {
            handle,
            workspace,
            expected_workspace_id,
            reason,
            wait,
            port,
            json,
        } => {
            return restore::run(
                &handle,
                &workspace,
                &expected_workspace_id,
                reason.as_deref(),
                port,
                json,
                wait,
            );
        }
        Action::RestoreStatus {
            restore_id,
            workspace,
            port,
            json,
        } => {
            return restore_status(restore_id, &workspace, port, json);
        }
        Action::Create {
            label,
            keep,
            wait,
            target,
        } => (
            target,
            reqwest::Method::POST,
            "/snapshots".into(),
            Some(json!({"label":label,"keep":keep})),
            wait,
        ),
        Action::List { target } => (
            target,
            reqwest::Method::GET,
            "/snapshots".into(),
            None,
            false,
        ),
        Action::Get { handle, target } => (
            target,
            reqwest::Method::GET,
            handle_path(&handle)?,
            None,
            false,
        ),
        Action::Check { handle, target } => (
            target,
            reqwest::Method::POST,
            format!("{}/check", handle_path(&handle)?),
            None,
            true,
        ),
        Action::Status {
            job_id,
            wait,
            target,
        } => (
            target,
            reqwest::Method::GET,
            format!("/snapshots/jobs/{job_id}"),
            None,
            wait,
        ),
        Action::Delete { handle, target } => (
            target,
            reqwest::Method::DELETE,
            handle_path(&handle)?,
            None,
            false,
        ),
    };
    let ws = target_workspace(&target)?;
    let (mut code, mut result) = request(&ws, method, &path, body.as_ref())?;
    if code == 0 && wait {
        (code, result) = wait_job(&ws, result)?;
    }
    if code == 0 && result["state"]["phase"] == "failed" {
        code = 4;
    }
    if target.json {
        println!("{}", serde_json::to_string(&result)?);
    } else {
        println!("{}", serde_json::to_string_pretty(&result)?);
    }
    Ok(code)
}

fn restore_status(
    id: uuid::Uuid,
    workspace: &str,
    port: Option<u16>,
    json: bool,
) -> Result<u8, BoxError> {
    if !lucidos_gateway_constants::is_valid_workspace_id(workspace) {
        return Err("snapshot_target_invalid".into());
    }
    let probe = crate::gateway::probe_client()?;
    let base = crate::gateway::the_gateway(
        &probe,
        port,
        "Pass --port to name the gateway that owns this restore.",
    )?;
    let client = http::client()?;
    let result = read_restore_status(&client, &base, id, workspace)?;
    if json {
        println!("{}", serde_json::to_string(&result)?);
    } else {
        println!("{}", serde_json::to_string_pretty(&result)?);
    }
    Ok(restore_terminal_code(&result)?.unwrap_or(0))
}

fn read_restore_status(
    client: &reqwest::blocking::Client,
    base: &str,
    id: uuid::Uuid,
    workspace: &str,
) -> Result<Value, BoxError> {
    let response = client
        .get(format!(
            "{base}/~/api/v1/control/workspaces/{workspace}/snapshot-restore-status"
        ))
        .query(&[("restore_id", id.to_string())])
        .send()
        .map_err(|_| "snapshot_restore_status_unavailable")?;
    let success = response.status().is_success();
    let body: Value = response.json().map_err(|_| "snapshot_response_failed")?;
    if !success {
        return Err(body["error"]
            .as_str()
            .unwrap_or("snapshot_request_refused")
            .to_owned()
            .into());
    }
    Ok(matching_restore_status(&body, id, workspace)?.clone())
}

fn restore_terminal_code(status: &Value) -> Result<Option<u8>, BoxError> {
    match status["phase"].as_str() {
        Some("completed") => Ok(Some(0)),
        Some("failed-unchanged" | "rolled-back" | "needs-attention" | "refused") => Ok(Some(4)),
        Some(
            "requested" | "recovery-capture" | "staging" | "db-restore" | "file-swap"
            | "files-restored" | "rolling-back",
        ) => Ok(None),
        _ => Err("snapshot_restore_status_unavailable".into()),
    }
}

fn matching_restore_status<'a>(
    body: &'a Value,
    id: uuid::Uuid,
    workspace: &str,
) -> Result<&'a Value, BoxError> {
    let status = &body["restore"];
    if status["restore_id"]
        .as_str()
        .and_then(|s| s.parse::<uuid::Uuid>().ok())
        != Some(id)
        || status["workspace_id"].as_str() != Some(workspace)
    {
        return Err("snapshot_restore_status_unavailable".into());
    }
    Ok(status)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn restore_accepts_wait_with_an_explicit_target() {
        use clap::Parser;
        #[derive(Parser)]
        struct Probe {
            #[command(subcommand)]
            action: Action,
        }
        let probe = Probe::try_parse_from([
            "probe",
            "restore",
            "snap_00000000000000000000000000",
            "--workspace",
            "example",
            "--expected-workspace-id",
            "example",
            "--wait",
        ])
        .unwrap();
        assert!(matches!(probe.action, Action::Restore { wait: true, .. }));
    }

    #[test]
    fn restore_wait_requires_verified_boot_completion() {
        for phase in [
            "requested",
            "recovery-capture",
            "staging",
            "db-restore",
            "file-swap",
            "files-restored",
            "rolling-back",
        ] {
            assert_eq!(
                restore_terminal_code(&json!({"phase":phase})).unwrap(),
                None
            );
        }
        assert_eq!(
            restore_terminal_code(&json!({"phase":"completed"})).unwrap(),
            Some(0)
        );
        for phase in [
            "failed-unchanged",
            "rolled-back",
            "needs-attention",
            "refused",
        ] {
            assert_eq!(
                restore_terminal_code(&json!({"phase":phase})).unwrap(),
                Some(4)
            );
        }
        assert!(restore_terminal_code(&json!({"phase":"future-phase"})).is_err());
        assert!(restore_terminal_code(&json!({})).is_err());
    }

    #[test]
    fn restore_status_requires_the_exact_journal_and_workspace() {
        let id = uuid::Uuid::new_v4();
        let body = json!({"restore":{"restore_id":id.to_string(),"workspace_id":"example","phase":"file-swap"}});
        assert!(matching_restore_status(&body, id, "example").is_ok());
        assert!(matching_restore_status(&body, uuid::Uuid::new_v4(), "example").is_err());
        assert!(matching_restore_status(&body, id, "other").is_err());
        assert!(matching_restore_status(&json!({"restore":null}), id, "example").is_err());
    }
    #[test]
    fn handles_cannot_escape_the_snapshot_route() {
        for invalid in [
            "../keys",
            "snap_0000000000000000000000000/",
            "snap_Z0000000000000000000000000",
        ] {
            assert!(handle_path(invalid).is_err());
        }
        assert!(handle_path("snap_00000000000000000000000000").is_ok());
    }
}

#[cfg(test)]
mod http_tests {
    use super::*;
    use std::io::{Read, Write};

    #[test]
    fn waits_for_the_job_returned_by_create_and_asserts_target_workspace() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let id = uuid::Uuid::new_v4();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 8192];
            let count = stream.read(&mut request).unwrap();
            let request = String::from_utf8_lossy(&request[..count]);
            assert!(request.starts_with(&format!("GET /api/v1/snapshots/jobs/{id} ")));
            assert!(request
                .to_lowercase()
                .contains("x-lucidos-target-workspace: example-workspace"));
            let body = json!({"id":id.to_string(),"state":{"phase":"completed","manifest":{"handle":"snap_00000000000000000000000000"}}}).to_string();
            write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body).unwrap();
        });
        let ws = Workspace {
            root: std::path::PathBuf::from("/example-workspace"),
            api_port: address.port(),
            proto: "http".into(),
            api_base_override: Some(format!("http://{address}")),
        };
        let (code, job) = wait_job(
            &ws,
            json!({"job_id":id.to_string(),"handle":"snap_00000000000000000000000000"}),
        )
        .unwrap();
        assert_eq!(code, 0);
        assert_eq!(
            job["state"]["manifest"]["handle"],
            "snap_00000000000000000000000000"
        );
        server.join().unwrap();
    }
}
