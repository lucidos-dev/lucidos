use crate::http::{client as http_client, send_and_print, send_expect_json};
use crate::workspace::{BoxError, Workspace};

pub(crate) fn cmd_apply(ws: &Workspace, change_id: &str) -> Result<(), BoxError> {
    let uuid = uuid::Uuid::try_parse(change_id)
        .map_err(|_| format!("Invalid change id (must be a UUID): {}", change_id))?;
    if uuid.is_nil() {
        return Err(format!(
            "Invalid change id (nil UUID is never a real change): {}",
            change_id
        )
        .into());
    }
    let url = apply_url(ws, &uuid.to_string());
    send_and_print("POST", &url, http_client()?.post(&url))
}

fn apply_url(ws: &Workspace, change_id: &str) -> String {
    format!("{}/api/v1/changes/{}/apply", ws.base_url(), change_id)
}

pub(crate) fn cmd_list(ws: &Workspace, sub_threads_of: Option<&str>) -> Result<(), BoxError> {
    let url = list_url(ws);
    let mut req = http_client()?.get(&url);
    if let Some(root) = sub_threads_of {
        let root = uuid::Uuid::try_parse(root)
            .map_err(|_| format!("Invalid --sub-threads-of (must be a thread UUID): {root}"))?;
        req = req.query(&[("sub_threads_of", root.to_string())]);
    }
    send_and_print("GET", &url, req)
}

pub(crate) fn resolve_sub_threads_of(
    sub_threads_of: Option<String>,
    my_sub_threads: bool,
    env_thread_id: Option<String>,
) -> Result<Option<String>, BoxError> {
    if !my_sub_threads {
        return Ok(sub_threads_of);
    }
    match env_thread_id.map(|t| t.trim().to_string()) {
        Some(t) if !t.is_empty() => Ok(Some(t)),
        _ => Err(format!(
            "--my-sub-threads needs a calling thread, and {} is not set. \
             It only works from inside a Lucidos thread. Pass --sub-threads-of <uuid> instead.",
            crate::threads::ENV_SOURCE_THREAD_ID
        )
        .into()),
    }
}

pub(crate) fn cmd_landing_setting(ws: &Workspace) -> Result<(), BoxError> {
    let url = list_url(ws);
    let snapshot = send_expect_json("GET", &url, http_client()?.get(&url))?;
    let setting = snapshot["landing_lane"]["setting"]
        .as_str()
        .ok_or("The engine did not report a landing setting.")?;
    println!("{setting}");
    Ok(())
}

fn list_url(ws: &Workspace) -> String {
    format!("{}/api/v1/changes", ws.base_url())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn workspace(port: u16) -> Workspace {
        Workspace {
            root: PathBuf::from("/tmp/test-ws"),
            api_port: port,
            proto: "https".to_string(),
            api_base_override: None,
        }
    }

    #[test]
    fn apply_url_targets_changes_endpoint() {
        let url = apply_url(&workspace(5173), "00000000-0000-0000-0000-000000000001");
        assert_eq!(
            url,
            "https://127.0.0.1:5173/api/v1/changes/00000000-0000-0000-0000-000000000001/apply"
        );
    }

    #[test]
    fn list_url_targets_changes_endpoint() {
        let url = list_url(&workspace(5173));
        assert_eq!(url, "https://127.0.0.1:5173/api/v1/changes");
    }

    #[test]
    fn my_sub_threads_reads_the_calling_thread() {
        let own = "11111111-1111-1111-1111-111111111111".to_string();
        assert_eq!(
            resolve_sub_threads_of(None, true, Some(own.clone())).unwrap(),
            Some(own)
        );
        assert_eq!(resolve_sub_threads_of(None, false, None).unwrap(), None);
        let err = resolve_sub_threads_of(None, true, None).unwrap_err();
        assert!(err.to_string().contains("LUCIDOS_THREAD_ID"), "{err}");
    }

    #[test]
    fn cmd_list_rejects_a_non_uuid_sub_threads_of() {
        let err = cmd_list(&workspace(0), Some("current")).unwrap_err();
        assert!(err.to_string().contains("UUID"), "{err}");
    }

    #[test]
    fn cmd_apply_rejects_non_uuid_change_id() {
        let err = cmd_apply(&workspace(0), "not-a-uuid").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("UUID"), "error must mention UUID: {msg}");
        assert!(
            msg.contains("not-a-uuid"),
            "error must echo the bad id: {msg}"
        );
    }

    #[test]
    fn cmd_apply_rejects_empty_change_id() {
        let err = cmd_apply(&workspace(0), "").unwrap_err();
        assert!(err.to_string().contains("UUID"));
    }

    #[test]
    fn cmd_apply_rejects_nil_uuid() {
        let err = cmd_apply(&workspace(0), "00000000-0000-0000-0000-000000000000").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("nil"), "error must mention nil: {msg}");
    }
}
