use crate::http::{client as http_client, send_and_print};
use crate::workspace::{BoxError, Workspace};

/// `POST /api/v1/changes/<id>/apply`. The CLI's HTTP client auto-forwards
/// `x-lucidos-agent-origin-token` when the engine-injected env var is
/// present, and that thread-bound token names the spawning thread itself, so
/// the resulting `ChangeApplied`
/// stamps `Api { mode: Agent, source_thread_id }` instead of bleeding through
/// as `Api { mode: Human }` (which the UI renders as "You"). That is the
/// reason this subcommand exists at all — hand-rolled urllib / curl from a
/// `run_python` / `run_bash` tool loses the headers and misattributes the
/// timeline. See `docs/apply-change-api.md` for the response shape and the
/// canonical caller workflow.
pub(crate) fn cmd_apply(ws: &Workspace, change_id: &str) -> Result<(), BoxError> {
    // Reject anything that isn't a hyphenated UUID. `parse_str` accepts
    // braced (`{…}`) and `urn:uuid:` prefixed forms too — the engine's
    // `Path<Uuid>` extractor would refuse those, but failing client-side
    // gives the script author an immediate, greppable error instead of a
    // 404 round-trip with a noisy URL. Also rejects the nil UUID, which
    // never names a real change.
    let uuid = uuid::Uuid::try_parse(change_id)
        .map_err(|_| format!("Invalid change id (must be a UUID): {}", change_id))?;
    if uuid.is_nil() {
        return Err(format!(
            "Invalid change id (nil UUID is never a real change): {}",
            change_id
        )
        .into());
    }
    // Re-serialize via Uuid::to_string so the path segment is the canonical
    // hyphenated lowercase form regardless of how the caller wrote it.
    let url = apply_url(ws, &uuid.to_string());
    send_and_print("POST", &url, http_client()?.post(&url))
}

fn apply_url(ws: &Workspace, change_id: &str) -> String {
    format!("{}/api/v1/changes/{}/apply", ws.base_url(), change_id)
}

/// `GET /api/v1/changes`. Echoes the engine's changes payload verbatim to
/// stdout: `{ pending, applied, total_pending, restart_required, … }`. The
/// `pending` array carries each change's `id`, which is the canonical way for
/// a CC subprocess / scheduled script to find the change id to feed
/// `lucidos changes apply <id>` — instead of guessing a `changes list`
/// subcommand that didn't exist and falling back to a raw `ChangeProposed`
/// event query.
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

/// The thread whose sub-threads' changes `list` narrows to, if any.
/// `--my-sub-threads` reads the calling thread's id off the environment, as
/// `threads list --my-children` does.
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
            "https://localhost:5173/api/v1/changes/00000000-0000-0000-0000-000000000001/apply"
        );
    }

    #[test]
    fn list_url_targets_changes_endpoint() {
        let url = list_url(&workspace(5173));
        assert_eq!(url, "https://localhost:5173/api/v1/changes");
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
        // Caught client-side before the HTTP round-trip — saves a 400
        // round-trip and gives the script author an immediate error
        // message they can grep for.
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

    /// The nil UUID parses as a `Uuid` but never names a real change. Reject
    /// it with the same client-side gate so a shell substitution like
    /// `${CHANGE_ID:-00000000-...}` doesn't reach the engine just to get a
    /// 400 — and so a script author sees "nil UUID is never a real change"
    /// instead of a generic engine error.
    #[test]
    fn cmd_apply_rejects_nil_uuid() {
        let err = cmd_apply(&workspace(0), "00000000-0000-0000-0000-000000000000").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("nil"), "error must mention nil: {msg}");
    }
}
