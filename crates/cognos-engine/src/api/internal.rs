use super::*;
use crate::engine::cc_permission::{CcPermissionEntry, CcPermissionState, DedupKey, DENIAL_REASON};
use crate::engine::event_bus::BusEvent;
use crate::engine::thread_events::{EventMeta, ThreadEvent};

#[derive(Deserialize)]
pub(super) struct PermissionPromptRequest {
    pub thread_id: String,
    pub tool_use_id: String,
    pub tool_name: String,
    pub input: serde_json::Value,
}

#[derive(Serialize)]
struct PermissionPromptResponse {
    allowed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
}

/// POST /api/internal/permission-prompt — invoked by the cognos-cli
/// `mcp-permission-server` subprocess (spawned by CC) when CC asks for
/// tool-call permission.
///
/// Behaves like `AskUserQuestion`: the engine emits a persisted event,
/// renders an inline card, and waits **indefinitely** for the user's answer.
/// No timeout on this handler — a timed-out denial would just push CC's
/// model into a retry that surfaces another card. CC's `MCP_TOOL_TIMEOUT`
/// env var (set very high in `runtime::claude_code`) is the only practical
/// bound.
///
/// Concurrent identical requests (same `thread_id` + `tool_name` + `input`)
/// dedup onto one canonical entry: the first emits the event, every
/// subsequent identical request subscribes to the same broadcast. One click
/// answers them all.
pub(super) async fn permission_prompt(
    State(state): State<AppState>,
    Json(body): Json<PermissionPromptRequest>,
) -> impl IntoResponse {
    let thread_id = match Uuid::parse_str(&body.thread_id) {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "Invalid thread_id").into_response(),
    };

    let canonical_input =
        serde_json::to_string(&body.input).unwrap_or_else(|_| "{}".to_string());
    let dedup_key: DedupKey = (thread_id, body.tool_name.clone(), canonical_input);
    let summary = build_summary(&body.tool_name, &body.input);

    let (request_id, mut rx, is_canonical) = {
        let mut pending = state.engine.pending_cc_permission.lock().unwrap();
        register_or_attach(&mut pending, dedup_key, thread_id)
    };

    if is_canonical {
        state
            .engine
            .event_bus
            .emit_or_log(
                BusEvent::Thread {
                    thread_id,
                    event: ThreadEvent::CodingAgentPermissionRequest {
                        request_id: request_id.clone(),
                        tool_use_id: body.tool_use_id,
                        tool_name: body.tool_name,
                        input: body.input,
                        summary,
                    },
                    meta: EventMeta::NONE,
                },
                "[Internal] CodingAgentPermissionRequest",
            )
            .await;
    }

    // Wait forever for the user. The paired `CodingAgentPermissionResolved`
    // is emitted by `submit_mcp_consent` (so it fires once per click, not
    // once per deduped listener).
    let allowed = rx.recv().await.unwrap_or(false);
    let reason = if allowed {
        None
    } else {
        Some(DENIAL_REASON.to_string())
    };

    Json(PermissionPromptResponse { allowed, reason }).into_response()
}

/// Look up `dedup_key`. If a canonical entry already exists (a duplicate
/// concurrent request), subscribe and reuse its `request_id`. Otherwise
/// create a fresh entry, register both indexes, and return its receiver.
///
/// Returns `(request_id, receiver, is_canonical)`. The caller emits the
/// `CodingAgentPermissionRequest` event only when `is_canonical` is true.
fn register_or_attach(
    state: &mut CcPermissionState,
    dedup_key: DedupKey,
    thread_id: Uuid,
) -> (String, tokio::sync::broadcast::Receiver<bool>, bool) {
    // Opportunistic sweep: each new prompt is a chance to evict orphans
    // whose HTTP handlers were canceled (CC died, MCP request aborted) and
    // would otherwise leak until engine restart.
    state.gc_dead_entries();
    if let Some(entry) = state.by_dedup_key.get(&dedup_key) {
        return (entry.request_id.clone(), entry.tx.subscribe(), false);
    }
    let request_id = Uuid::new_v4().to_string();
    let (tx, rx) = tokio::sync::broadcast::channel(1);
    state.by_dedup_key.insert(
        dedup_key.clone(),
        CcPermissionEntry {
            thread_id,
            request_id: request_id.clone(),
            tx,
        },
    );
    state.by_request_id.insert(request_id.clone(), dedup_key);
    (request_id, rx, true)
}

#[derive(Deserialize)]
pub(super) struct MarkHardenedRequest {
    pub repo_root: String,
    pub branch_name: String,
    pub head_sha: String,
}

/// POST /api/internal/mark-hardened — invoked by `cognos hardened mark` from
/// the `mark-harden.sh` hook after Claude Code finishes `/harden`. Replaces
/// the prior worktree-keyed file marker, which was lost when stale-session
/// recovery removed the worktree before the apply check ran.
pub(super) async fn mark_hardened(
    State(state): State<AppState>,
    Json(body): Json<MarkHardenedRequest>,
) -> impl IntoResponse {
    let repo_root = std::path::PathBuf::from(&body.repo_root);
    match state
        .engine
        .record_hardened(&repo_root, &body.branch_name, &body.head_sha)
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => {
            crate::log!(
                "[Internal] record_hardened failed for {}: {}",
                body.branch_name,
                e
            );
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("record_hardened: {}", e),
            )
                .into_response()
        }
    }
}

fn build_summary(tool_name: &str, input: &serde_json::Value) -> String {
    let arg = [
        "file_path",
        "path",
        "command",
        "notebook_path",
        "skill",
        "url",
        "pattern",
    ]
    .iter()
    .find_map(|k| input.get(k).and_then(|v| v.as_str()))
    .unwrap_or("");
    let display_name = match tool_name {
        "Skill" => "skill",
        _ => tool_name,
    };
    if arg.is_empty() {
        display_name.to_string()
    } else {
        format!("{} {}", display_name, arg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_summary_uses_file_path() {
        let s = build_summary(
            "Edit",
            &serde_json::json!({ "file_path": "/tmp/foo.md", "old_string": "x" }),
        );
        assert_eq!(s, "Edit /tmp/foo.md");
    }

    #[test]
    fn build_summary_falls_back_to_command() {
        let s = build_summary("Bash", &serde_json::json!({ "command": "ls -la" }));
        assert_eq!(s, "Bash ls -la");
    }

    #[test]
    fn build_summary_returns_tool_name_when_no_arg_field() {
        let s = build_summary("WeirdTool", &serde_json::json!({ "foo": 1 }));
        assert_eq!(s, "WeirdTool");
    }

    #[test]
    fn build_summary_uses_skill_for_skill_tool() {
        let s = build_summary("Skill", &serde_json::json!({ "skill": "update-config" }));
        assert_eq!(s, "skill update-config");
    }

    #[test]
    fn build_summary_uses_url_for_webfetch() {
        let s = build_summary(
            "WebFetch",
            &serde_json::json!({ "url": "https://example.com", "prompt": "x" }),
        );
        assert_eq!(s, "WebFetch https://example.com");
    }

    #[test]
    fn register_or_attach_creates_canonical_entry_first_time() {
        let mut state = CcPermissionState::default();
        let key: DedupKey = (Uuid::nil(), "Edit".into(), "{}".into());
        let (request_id, _rx, is_canonical) = register_or_attach(&mut state, key.clone(), Uuid::nil());
        assert!(is_canonical, "first request must be canonical");
        assert!(state.by_dedup_key.contains_key(&key));
        assert!(state.by_request_id.contains_key(&request_id));
    }

    #[test]
    fn register_or_attach_returns_existing_request_id_for_duplicate() {
        let mut state = CcPermissionState::default();
        let key: DedupKey = (Uuid::nil(), "Edit".into(), "{}".into());
        let (first_id, _rx1, first_canonical) =
            register_or_attach(&mut state, key.clone(), Uuid::nil());
        let (second_id, _rx2, second_canonical) =
            register_or_attach(&mut state, key.clone(), Uuid::nil());
        assert!(first_canonical);
        assert!(!second_canonical, "duplicate must not be canonical");
        assert_eq!(
            first_id, second_id,
            "duplicate must reuse the canonical request_id"
        );
    }

    #[tokio::test]
    async fn duplicate_subscribers_both_receive_the_answer() {
        let mut state = CcPermissionState::default();
        let key: DedupKey = (Uuid::nil(), "Edit".into(), "{}".into());
        let (id, mut rx1, _) = register_or_attach(&mut state, key.clone(), Uuid::nil());
        let (_, mut rx2, _) = register_or_attach(&mut state, key.clone(), Uuid::nil());

        // Resolve via the same path the consent endpoint uses.
        let entry = state.take(&id).expect("entry must be present");
        let _ = entry.tx.send(true);

        assert!(rx1.recv().await.unwrap());
        assert!(rx2.recv().await.unwrap());
    }
}
