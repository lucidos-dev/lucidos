//! Engine-side tool dispatch: `handle_special_tool` covers tools that need
//! more than `execute_tool()` can give them (capture/refresh app UI,
//! run_thread, resume_thread, run_coding_agent, …) and `run_intent_loop` runs
//! the intent sub-loop for the open-ended verbs.

use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::{build_intent_tools, derive_call_key};
use crate::engine::context::trim_context_if_needed;
use crate::engine::event_bus::BusEvent;
use crate::engine::http::workspace_client::cross_workspace_http_client;
use crate::engine::thread_events::{ActorMode, EventChannel, EventMeta, ThreadEvent};
use crate::engine::LucidosEngine;
use crate::llm::tool_names as tn;
use crate::llm::{ContentBlock, Message, MessageContent};
use crate::triggers::{normalize_route_setting, validate_trigger_reasoning_effort};

/// How a thread spawned by `run_thread` / `run_coding_agent` relates to the
/// thread that issued the spawn. Drives both the spawn linkage (parent /
/// spawning event ids) and the result-text branch.
///
/// `Child` (default) preserves today's callback semantics: the spawning
/// thread's `notify_parent_if_child` fires when the spawned thread reaches
/// a terminal event, and `active_children_count` is incremented while the
/// spawned thread is in flight. The wire string is `"child"`; `"sub"` is
/// accepted as a back-compat alias from before the glossary settled on
/// *child thread* (direct descendant) vs *sub-thread* (transitive).
///
/// `Top` drops the linkage entirely — no callback, no count bump. The
/// spawned thread appears as an independent top-level thread in the list.
/// Use when the spawn is for the user to follow, not for the spawning
/// thread to resume on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Relation {
    Child,
    Top,
}

/// The originating tool call's coordinates for a spawn: the thread that issued
/// it and the `ToolCalled` event id (when known). Bundled because the two
/// always travel together into the cross-workspace `WorkspaceCallCtx` as
/// `source_thread_id` / `source_event_id`.
#[derive(Debug, Clone, Copy)]
struct SpawnSource {
    thread_id: Uuid,
    tool_called_event_id: Option<Uuid>,
}

/// Parse the optional `relation` argument from a tool call. Defaults to
/// [`Relation::Child`] when absent so existing prompts keep working unchanged.
/// Accepts `"child"` (canonical) and `"sub"` (back-compat alias from the
/// pre-glossary wire name). Returns `Err` for unknown values or non-string
/// types so the LLM sees the contract violation instead of silently getting
/// `Child`.
pub(crate) fn parse_relation(args: &serde_json::Value) -> Result<Relation, String> {
    match args.get("relation") {
        None | Some(serde_json::Value::Null) => Ok(Relation::Child),
        Some(serde_json::Value::String(s)) => match s.as_str() {
            "child" | "sub" => Ok(Relation::Child),
            "top" => Ok(Relation::Top),
            other => Err(format!(
                "relation must be \"child\" or \"top\", got {:?}",
                other
            )),
        },
        Some(other) => Err(format!(
            "relation must be \"child\" or \"top\", got {}",
            other
        )),
    }
}

/// Parse the optional `coding_agent` argument from `run_coding_agent`.
/// Defaults to Claude Code for existing callers. Accepts the canonical
/// kebab-case values plus the common underscore alias for Claude Code; returns
/// an explicit error for typos so a request for Codex never silently starts CC.
pub(crate) fn parse_coding_agent(
    args: &serde_json::Value,
) -> Result<crate::runtime::CodingAgent, String> {
    match args.get("coding_agent") {
        None | Some(serde_json::Value::Null) => Ok(crate::runtime::CodingAgent::ClaudeCode),
        Some(serde_json::Value::String(s)) => match s.as_str() {
            "claude-code" | "claude_code" => Ok(crate::runtime::CodingAgent::ClaudeCode),
            "codex" => Ok(crate::runtime::CodingAgent::Codex),
            other => Err(format!(
                "coding_agent must be \"claude-code\" or \"codex\", got {:?}",
                other
            )),
        },
        Some(other) => Err(format!(
            "coding_agent must be \"claude-code\" or \"codex\", got {}",
            other
        )),
    }
}

/// Resolve the model and reasoning effort a `run_thread` child runs on: a
/// caller-supplied pin wins, otherwise the account chat preference. The two
/// resolve independently, so pinning one leaves the other inherited.
///
/// Validation follows the trigger route pins, the other place a chat model and
/// effort are pinned per spawn. The model id passes through unchecked
/// (`normalize_route_setting`). A registry row can be disabled long after the
/// caller read it, and a bad id surfaces as an ordinary thread failure. The
/// tier vocabulary is ours, so a typo there is rejected by name
/// (`validate_trigger_reasoning_effort`), never swapped for the default.
pub(crate) fn resolve_sub_thread_model_and_effort(
    args: &serde_json::Value,
    pref_model: Option<String>,
    pref_effort: Option<String>,
) -> Result<(Option<String>, Option<String>), String> {
    let model = normalize_route_setting(optional_string_arg(args, "model")?).or(pref_model);
    let effort = validate_trigger_reasoning_effort(optional_string_arg(args, "reasoning_effort")?)?
        .or(pref_effort);
    Ok((model, effort))
}

/// Read an optional string tool argument, rejecting a non-string instead of
/// reading it as absent. A caller that sent the wrong type must learn its pin
/// did not apply, rather than silently getting the account default.
fn optional_string_arg<'a>(
    args: &'a serde_json::Value,
    key: &str,
) -> Result<Option<&'a str>, String> {
    match args.get(key) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::String(s)) => Ok(Some(s.as_str())),
        Some(other) => Err(format!("{} must be a string, got {}", key, other)),
    }
}

/// The ATTRIBUTION half of a spawn: the `MessageOrigin` to stamp on the spawned
/// thread's first `MessageReceived` so the message route popover names and links
/// the thread that launched it.
///
/// Takes no [`Relation`], and that is the invariant rather than an oversight:
/// both relations were launched by the same thread and the popover should say so
/// either way. Only the CALLBACK half varies, in [`Relation::spawn_linkage`].
/// For `Child` this is exactly what `synthesize_legacy_origin` would have built
/// from the linkage, so a child spawn's persisted payload is unchanged; for
/// `Top` it is the same origin carried *without* linkage, which is the fix.
///
/// `direction: Parent` because from the spawned thread's point of view the
/// linked thread is the one that started it, whatever the relation. Origin
/// without linkage is an established shape here, not a new one:
/// `build_follow_up_message` (`chat/child_follow_up.rs`) already emits a
/// `MessageReceived` this way so a follow-up attributes to the parent without
/// re-counting the child.
pub(crate) fn spawn_origin(
    spawning_thread_id: Uuid,
    tool_called_event_id: Option<Uuid>,
) -> Option<crate::engine::thread_events::MessageOrigin> {
    Some(crate::engine::thread_events::MessageOrigin::ThreadLink {
        thread_id: spawning_thread_id,
        title: None,
        spawning_event_id: tool_called_event_id,
        mode: ActorMode::Agent,
        direction: crate::engine::thread_events::ThreadDirection::Parent,
    })
}

fn coding_agent_label(agent: crate::runtime::CodingAgent) -> &'static str {
    match agent {
        crate::runtime::CodingAgent::ClaudeCode => "Claude Code",
        crate::runtime::CodingAgent::Codex => "Codex",
    }
}

impl Relation {
    /// Resolve the `(parent_thread_id, spawning_event_id)` pair to pass
    /// into the spawn helpers. `Child` carries the spawning thread + tool
    /// call event through; `Top` drops both so the spawned thread has no
    /// parent linkage and `notify_parent_if_child` early-returns.
    ///
    /// This is the CALLBACK half of a spawn, and it is deliberately not the
    /// same thing as [`spawn_origin`]: the linkage answers "do I report back to
    /// them" (callback + `active_children_count` + the projection's
    /// `parent_thread_id`), the origin answers "who launched me" (display
    /// attribution). Collapsing the two is what made a `Top` spawn render its
    /// Origin as "Unknown".
    pub(crate) fn spawn_linkage(
        self,
        spawning_thread_id: Uuid,
        tool_called_event_id: Option<Uuid>,
    ) -> (Option<Uuid>, Option<Uuid>) {
        match self {
            Self::Child => (Some(spawning_thread_id), tool_called_event_id),
            Self::Top => (None, None),
        }
    }

    /// Tool-result text returned for `run_thread`. The LLM reads this to
    /// decide whether to expect a callback, so each branch must state the
    /// callback contract explicitly. `workspace` is stamped into the body and
    /// the link prefix so the LLM cannot hallucinate which workspace the
    /// thread landed in (the link is workspace-qualified per the `thread:`
    /// renderer in `renderMarkdown.ts`).
    pub(crate) fn run_thread_success_text(self, child_thread_id: Uuid, workspace: &str) -> String {
        match self {
            Self::Child => format!(
                "Thread started in workspace '{ws}' (thread_id: {tid}). When the child \
                 thread finishes, this conversation will automatically resume with its \
                 results — you don't need to check on it. Continue with other work or \
                 tell the user you've delegated this subtask in the {ws} workspace: \
                 [Open thread](thread:{ws}/{tid})",
                ws = workspace,
                tid = child_thread_id
            ),
            Self::Top => format!(
                "Top-level thread started in workspace '{ws}' (thread_id: {tid}). It \
                 runs independently and will NOT report back to this conversation. Tell \
                 the user you've started a separate thread in the {ws} workspace for \
                 them to follow: [Open thread](thread:{ws}/{tid})",
                ws = workspace,
                tid = child_thread_id
            ),
        }
    }

    /// Tool-result text returned when a spawn lands in the *Thread Queue*
    /// instead of starting immediately (the system is at background
    /// capacity). `label` names the spawn flavor ("Thread" / "Claude Code
    /// session"). The callback contract per relation is unchanged — it just
    /// starts later — so each branch restates it, mirroring the success
    /// texts.
    pub(crate) fn queued_spawn_text(self, label: &str, thread_id: Uuid, position: usize) -> String {
        let base = format!(
            "{label} queued at position {position} in the Thread Queue — Lucidos is at \
             background capacity. It starts automatically when a slot frees; its \
             thread_id will be {thread_id}."
        );
        match self {
            Self::Child => format!(
                "{base} When it eventually finishes, this conversation will \
                 automatically resume with its results — you don't need to check on it."
            ),
            Self::Top => format!(
                "{base} It runs independently and will NOT report back to this \
                 conversation."
            ),
        }
    }

    pub(crate) fn run_coding_agent_success_text(
        self,
        cc_thread_id: Uuid,
        workspace: &str,
        coding_agent: crate::runtime::CodingAgent,
    ) -> String {
        let label = coding_agent_label(coding_agent);
        match self {
            Self::Child => format!(
                "{label} session started in workspace '{ws}' (thread_id: {tid}). \
                 Tell the user you've started a new {label} thread in the {ws} \
                 workspace and include this link: [Open thread](thread:{ws}/{tid})",
                label = label,
                ws = workspace,
                tid = cc_thread_id
            ),
            Self::Top => format!(
                "Top-level {label} session started in workspace '{ws}' (thread_id: \
                 {tid}). It runs independently and will NOT report back to this \
                 conversation. Include the link in your reply so the user can follow it: \
                 [Open thread](thread:{ws}/{tid})",
                label = label,
                ws = workspace,
                tid = cc_thread_id
            ),
        }
    }
}

impl LucidosEngine {
    /// Handle tool calls that need special processing outside of `execute_tool`.
    /// Returns `Some(result)` if the tool was handled, `None` if it should fall through
    /// to `execute_tool()`.
    // Per-parameter rationale lives in the inline comments below; a bundle
    // struct would just rename the same set of fields.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn handle_special_tool(
        &self,
        tool_name: &str,
        tool_args: &serde_json::Value,
        thread_id: Uuid,
        user_images: Option<&[crate::api::ChatImage]>,
        device_id: Option<&str>,
        // Event_id of the just-emitted ToolCalled event. Forwarded as
        // `spawning_event_id` for spawn-style tools (run_thread,
        // run_coding_agent)
        // so the new thread can be traced back to the exact tool call that
        // started it.
        tool_called_event_id: Option<Uuid>,
        // Outer batch id `ask_user_question` keys its per-question wait
        // registry sub_ids on (see `synth_question_id`).
        tool_use_id: &str,
        // Per-turn event meta — carries the channel (`Some(Trigger)` for a
        // scheduled trigger, `None` for chat) so the MCP permission gate can
        // auto-approve silently in unattended trigger threads, and the actor for
        // the permission card. Cloned onto any emitted event.
        meta: &EventMeta,
        // Cancel token for the response — lets a blocking MCP permission card
        // resolve as canceled when the user clicks Stop, instead of dangling.
        cancel_token: &tokio_util::sync::CancellationToken,
    ) -> Option<String> {
        if tool_name == tn::ASK_USER_QUESTION {
            return Some(
                self.handle_chat_ask_user_question(thread_id, tool_use_id, tool_args)
                    .await,
            );
        }
        if tool_name == tn::CAPTURE_APP || tool_name == tn::REFRESH_APP {
            let app_id = tool_args
                .get("app_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            if tool_name == tn::REFRESH_APP {
                self.event_bus
                    .emit_or_log(
                        BusEvent::Thread {
                            thread_id,
                            event: ThreadEvent::AppUiRefreshRequested {
                                app_id: app_id.clone(),
                            },
                            meta: EventMeta::NONE,
                        },
                        "[AgenticLoop] AppUiRefreshRequested (refresh_app tool)",
                    )
                    .await;
            }

            let skip_capture = tool_args
                .get("skip_capture")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            if skip_capture {
                Some("App UI refreshed.".to_string())
            } else {
                if tool_name == tn::REFRESH_APP {
                    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                }

                let request_id = Uuid::new_v4().to_string();
                let (tx, rx) = tokio::sync::oneshot::channel();
                {
                    let mut captures = self.pending_captures.lock().unwrap();
                    captures.insert(request_id.clone(), tx);
                }

                self.event_bus
                    .emit_or_log(
                        BusEvent::Thread {
                            thread_id,
                            event: ThreadEvent::AppUiCaptureRequested {
                                app_id: app_id.clone(),
                                request_id: request_id.clone(),
                            },
                            meta: EventMeta::NONE,
                        },
                        "[AgenticLoop] AppUiCaptureRequested",
                    )
                    .await;

                Some(
                    match tokio::time::timeout(std::time::Duration::from_secs(10), rx).await {
                        Ok(Ok(capture)) => {
                            super::format_capture_result(&capture.screenshot, &capture.dom)
                        }
                        Ok(Err(_)) => {
                            "Error: Capture failed — the frontend channel was dropped.".to_string()
                        }
                        Err(_) => {
                            let mut captures = self.pending_captures.lock().unwrap();
                            captures.remove(&request_id);
                            "Error: Capture timed out (10s) — the frontend could not open or capture the app UI.".to_string()
                        }
                    },
                )
            }
        } else if tool_name == tn::RUN_CODING_AGENT || tool_name == tn::RUN_CLAUDE_LEGACY {
            let prompt = tool_args["prompt"].as_str().unwrap_or("");
            if prompt.is_empty() {
                Some("Error: prompt is required".to_string())
            } else {
                let relation = match parse_relation(tool_args) {
                    Ok(r) => r,
                    Err(e) => return Some(format!("Error: {}", e)),
                };
                let coding_agent = match parse_coding_agent(tool_args) {
                    Ok(agent) => agent,
                    Err(e) => return Some(format!("Error: {}", e)),
                };
                // Refuse the two arguments this tool cannot honour, rather than
                // accepting and dropping them. `run_session` owns both: it reads
                // the allowlist from the user's `cc-allowed-tools` file and
                // composes the system prompt itself. Both were declared in the
                // schema for a year and silently discarded; the schema no longer
                // declares them, and this catches a caller working from a stale
                // one. Naming the owner matters, because the caller's next move
                // differs: the allowlist is a Settings edit, not a spawn argument.
                for (arg, owner) in [
                    ("allowed_tools", "Settings › Coding agents › allowed tools"),
                    ("append_system_prompt", "the engine's own session prompt"),
                ] {
                    if tool_args.get(arg).is_some_and(|v| !v.is_null()) {
                        return Some(format!(
                            "Error: run_coding_agent does not take `{arg}` — it is owned by \
                             {owner} and a value here would be ignored. Remove it and re-spawn."
                        ));
                    }
                }
                // The model and effort pins, checked against the backend that
                // will actually run them. A bad id fails the spawn HERE, in the
                // same turn, so the caller learns its pin did not apply instead
                // of reporting a model choice the session never made.
                let spawn_model = match crate::runtime::validate_coding_agent_model(
                    coding_agent,
                    tool_args.get("model").and_then(|v| v.as_str()),
                ) {
                    Ok(m) => m,
                    Err(e) => return Some(format!("Error: {}", e)),
                };
                let spawn_effort = match crate::runtime::validate_coding_agent_effort(
                    coding_agent,
                    tool_args.get("reasoning_effort").and_then(|v| v.as_str()),
                ) {
                    Ok(e) => e,
                    Err(e) => return Some(format!("Error: {}", e)),
                };
                let workspace_arg = tool_args
                    .get("workspace")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty());
                // Defer workspace_name() until we know the same-workspace
                // path needs it — most calls omit `workspace` and stay local,
                // and workspace_name() allocates a fresh String each call.
                let current_ws = if workspace_arg.is_some() {
                    Some(self.workspace_name())
                } else {
                    None
                };
                if let Some(target_ws) = workspace_arg.filter(|w| Some(*w) != current_ws.as_deref())
                {
                    return Some(
                        self.cross_workspace_run_coding_agent(
                            prompt,
                            target_ws,
                            relation,
                            coding_agent,
                            tool_args,
                            SpawnSource {
                                thread_id,
                                tool_called_event_id,
                            },
                        )
                        .await,
                    );
                }
                let (parent_thread_id, spawning_event_id) =
                    relation.spawn_linkage(thread_id, tool_called_event_id);
                let origin = spawn_origin(thread_id, tool_called_event_id);

                // `folder` is the new canonical parameter; `repo` is the
                // deprecated alias kept for one release (temporary measure —
                // registered in docs/temporary-measures.md § "`repo` → `folder`
                // deprecated alias on `run_coding_agent`"). Passing both is an
                // error so callers don't silently get the resolution of one
                // when they meant the other.
                let folder_param = tool_args
                    .get("folder")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty());
                let repo_param = tool_args
                    .get("repo")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty());
                if folder_param.is_some() && repo_param.is_some() {
                    return Some(
                        "Error: pass `folder` (preferred) or `repo` (deprecated alias), not both."
                            .to_string(),
                    );
                }
                let folder_input = folder_param.or(repo_param);
                // Omitting `folder` MEANS "edit Lucidos itself" — which only
                // exists on an install launched from a source checkout. Refuse
                // here, synchronously, so the model learns it in the same turn
                // and can tell the user the truth instead of narrating platform
                // edits it cannot make. (`run_session` guards the same case for
                // every other caller; this site exists because a tool result is
                // the only channel the model actually reads.)
                if folder_input.is_none() && !crate::paths::has_lucidos_source() {
                    return Some(format!(
                        "Error: {}",
                        crate::engine::agent_session::err_no_lucidos_source()
                    ));
                }
                // Resolve the folder against the §3.2 classification table.
                // App folder (`data/apps/<id>` or absolute path under the
                // workspace's `data/apps/<id>`) → spawn an app coding-agent
                // thread. Lucidos source → existing flow. Registered repo
                // name → existing external-repo flow.
                let mut spawn_repo_id: Option<String> = None;
                let mut spawn_app_id: Option<String> = None;
                if let Some(folder_in) = folder_input {
                    use crate::engine::agent_session::{
                        classify_resolved_folder, resolve_folder_input, FolderClassification,
                    };
                    let ws_root = self.workspace_path().to_path_buf();
                    let pool = self.pool.clone();
                    let folder_in_owned = folder_in.to_string();
                    // Snapshot the repo registry up front. The classifier's
                    // `external_repo_match` closure is sync; without this
                    // snapshot it would have to await, and a no-op closure
                    // would mean External never matches and every registered
                    // external repo would fall through to UnrecognisedPath.
                    let registered_repos =
                        match crate::core::repositories::RepositoryStore::list(&pool).await {
                            Ok(list) => list,
                            Err(e) => {
                                return Some(format!("Error: Failed to list repositories: {e}"));
                            }
                        };
                    let resolved = resolve_folder_input(&folder_in_owned, &ws_root, |name| {
                        Ok(registered_repos
                            .iter()
                            .find(|r| r.name == name || r.id.to_string() == name)
                            .map(|r| std::path::PathBuf::from(&r.path)))
                    });
                    let canonical = match crate::core::repositories::RepositoryStore::resolve(
                        &pool,
                        &folder_in_owned,
                    )
                    .await
                    {
                        Ok(Some(repo)) => Some(std::path::PathBuf::from(repo.path)),
                        _ => resolved.ok(),
                    };
                    let Some(folder_abs) = canonical else {
                        return Some(format!(
                            "Error: `folder` value '{folder_in_owned}' could not be resolved — \
                             not a registered repo and not a path on disk in this workspace."
                        ));
                    };
                    // Resolve the Lucidos source repo root for the classifier.
                    // Absent on a packaged build (no source checkout) — `None`
                    // means the Lucidos-source branch can't match; App +
                    // External still classify.
                    let lucidos_root = registered_repos
                        .iter()
                        .find(|r| r.name == "Lucidos")
                        .map(|r| std::path::PathBuf::from(&r.path));
                    let class = classify_resolved_folder(
                        &folder_abs,
                        &ws_root,
                        lucidos_root.as_deref(),
                        |p| {
                            registered_repos
                                .iter()
                                .find(|r| std::path::Path::new(&r.path) == p)
                                .map(|r| std::path::PathBuf::from(&r.path))
                        },
                    );
                    match class {
                        Ok(FolderClassification::Lucidos { .. }) => {
                            // Existing flow — no repo_id, default folder.
                        }
                        Ok(FolderClassification::App { app_id, .. }) => {
                            spawn_app_id = Some(app_id);
                        }
                        Ok(FolderClassification::External { repo_root }) => {
                            // The classifier already confirmed this path is
                            // a registered external repo; pull the UUID from
                            // the snapshot rather than re-querying.
                            if let Some(repo) = registered_repos
                                .iter()
                                .find(|r| std::path::Path::new(&r.path) == repo_root)
                            {
                                spawn_repo_id = Some(repo.id.to_string());
                            } else {
                                return Some(format!(
                                    "Error: Repository at '{}' is not registered. Use manage_repositories action='add' first.",
                                    crate::core::home_path::abbreviate(repo_root.as_path())
                                ));
                            }
                        }
                        Err(e) => {
                            return Some(format!("Error: {e}"));
                        }
                    }
                }
                let repo_id = spawn_repo_id;

                // Absent `images` → forward the current message's images (default).
                // Present (even empty) → caller has explicitly chosen the selection.
                let resolved_images: Option<Vec<crate::api::ChatImage>> = match tool_args
                    .get("images")
                {
                    Some(serde_json::Value::Array(arr)) => {
                        let refs: Vec<String> = arr
                            .iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect();
                        if refs.len() != arr.len() {
                            return Some("Error: `images` entries must be strings".to_string());
                        }
                        let events = match self
                            .event_store
                            .get_thread_events(&thread_id.to_string())
                            .await
                        {
                            Ok(evts) => evts,
                            Err(e) => {
                                return Some(format!("Error: Failed to load thread events: {}", e))
                            }
                        };
                        match crate::engine::tools::image::resolve_thread_image_refs(
                            &self.workspace_path,
                            &events,
                            &refs,
                        ) {
                            Ok(imgs) => Some(imgs),
                            Err(e) => return Some(format!("Error: {}", e)),
                        }
                    }
                    _ => None,
                };

                if spawn_app_id.is_some() && repo_id.is_some() {
                    return Some(
                        "Error: Cannot pass both `app_id` and `repo_id` — an app \
                         coding-agent thread is not a repo"
                            .to_string(),
                    );
                }
                let caller_title = tool_args["title"].as_str();
                let images = resolved_images.as_deref().or(user_images);
                let cc_thread_id = uuid::Uuid::new_v4();
                let request = crate::engine::thread_queue::ThreadQueueRequest::CodingAgent {
                    // Stamped by `submit` from this task's chain depth.
                    depth: 0,
                    prompt: prompt.to_string(),
                    cc_thread_id,
                    image_hashes: self.queued_image_hashes(images),
                    device_id: device_id.map(str::to_string),
                    parent_thread_id,
                    spawning_event_id,
                    repo_id,
                    title: caller_title.map(str::to_string),
                    app_id: spawn_app_id,
                    coding_agent,
                    model: spawn_model,
                    reasoning_effort: spawn_effort,
                    origin,
                };
                let outcome = self.thread_queue.submit(request, None, None).await;
                Some(if outcome.admitted {
                    let ws = current_ws.unwrap_or_else(|| self.workspace_name());
                    relation.run_coding_agent_success_text(cc_thread_id, &ws, coding_agent)
                } else {
                    relation.queued_spawn_text(
                        &format!("{} session", coding_agent_label(coding_agent)),
                        cc_thread_id,
                        outcome.position,
                    )
                })
            }
        } else if tool_name == tn::RUN_THREAD {
            let prompt = tool_args["prompt"].as_str().unwrap_or("");
            if prompt.is_empty() {
                Some("Error: prompt is required".to_string())
            } else {
                let relation = match parse_relation(tool_args) {
                    Ok(r) => r,
                    Err(e) => return Some(format!("Error: {}", e)),
                };
                let (parent_thread_id, spawning_event_id) =
                    relation.spawn_linkage(thread_id, tool_called_event_id);
                let origin = spawn_origin(thread_id, tool_called_event_id);
                Some(
                    match LucidosEngine::check_thread_recursion_guard(&self.pool, thread_id).await {
                        Err(guard_err) => format!("Error: {}", guard_err),
                        Ok(_) => {
                            let child_thread_id = uuid::Uuid::new_v4();
                            // Without this the spawned loop defaults to LUCIDOS_MODEL
                            // instead of the user's chat-mode preference.
                            let (chat_model, chat_effort) =
                                crate::core::PreferenceStore::user_chat_settings(&self.pool).await;
                            let (model, reasoning_effort) =
                                match resolve_sub_thread_model_and_effort(
                                    tool_args,
                                    chat_model,
                                    chat_effort,
                                ) {
                                    Ok(pins) => pins,
                                    Err(e) => return Some(format!("Error: {}", e)),
                                };
                            let caller_title = tool_args["title"].as_str();
                            // The eager MessageReceived (a Child relation child's
                            // active_children_count must increment before the parent
                            // can complete this turn) lives in the Thread Queue
                            // executor's `prepare` hook — `submit` awaits it inline
                            // on the admitted path, preserving the ordering.
                            let request =
                                crate::engine::thread_queue::ThreadQueueRequest::SubThread {
                                    // Stamped by `submit`, as above.
                                    depth: 0,
                                    prompt: prompt.to_string(),
                                    child_thread_id,
                                    parent_thread_id,
                                    spawning_event_id,
                                    title: caller_title.map(str::to_string),
                                    model,
                                    reasoning_effort,
                                    pre_emitted_origin: None,
                                    origin,
                                };
                            let outcome = self.thread_queue.submit(request, None, None).await;
                            if outcome.admitted {
                                relation.run_thread_success_text(
                                    child_thread_id,
                                    &self.workspace_name(),
                                )
                            } else {
                                relation.queued_spawn_text(
                                    "Thread",
                                    child_thread_id,
                                    outcome.position,
                                )
                            }
                        }
                    },
                )
            }
        } else if let Some((server_id, mcp_tool_name)) =
            crate::mcp::McpManager::parse_mcp_tool_name(tool_name)
        {
            use crate::engine::mcp_permission::{mcp_gate, McpAsk, McpGate};

            let (auto_approve, server_name) = {
                let statuses = self.mcp_manager.list_servers().await.unwrap_or_default();
                let server = statuses.iter().find(|s| s.id == server_id);
                (
                    server.map(|s| s.auto_approve).unwrap_or(false),
                    server
                        .map(|s| s.name.clone())
                        .unwrap_or_else(|| server_id.clone()),
                )
            };

            // Pre-authorization gate: the server's `auto_approve` flag or a
            // non-interactive trigger thread proceeds with no card; otherwise
            // ask the user (in-thread `McpPermissionRequested` card), which also
            // honors a prior session / `mcp-allowed-tools` grant.
            if let McpGate::Ask = mcp_gate(auto_approve, meta.channel) {
                let args_summary = serde_json::to_string_pretty(tool_args)
                    .unwrap_or_else(|_| tool_args.to_string());
                let args_summary = if args_summary.len() > 500 {
                    format!(
                        "{}...",
                        &args_summary[..args_summary.floor_char_boundary(500)]
                    )
                } else {
                    args_summary
                };

                if let McpAsk::Refuse(refusal) = self
                    .ask_mcp_permission(
                        &server_id,
                        &server_name,
                        &mcp_tool_name,
                        tool_name,
                        tool_args,
                        &args_summary,
                        thread_id,
                        tool_use_id,
                        meta,
                        cancel_token,
                    )
                    .await
                {
                    return Some(refusal);
                }
            }

            Some(
                match self
                    .mcp_manager
                    .call_tool(&server_id, &mcp_tool_name, tool_args.clone())
                    .await
                {
                    Ok((result, _, _)) => result,
                    Err(e) => format!("Error: MCP tool call failed: {}", e),
                },
            )
        } else {
            None
        }
    }

    /// Cross-workspace `run_coding_agent`: POST a coding-agent spawn request
    /// to another workspace's engine. Returns the formatted tool-result string (success
    /// text on 2xx, error string otherwise) so the caller can pass it through
    /// to the LLM unchanged.
    ///
    /// Cross-workspace requires `relation=Top` — child-thread auto-resume
    /// callbacks across workspaces are unsupported because the receiving
    /// engine has no way to notify ours when the spawned thread terminates.
    /// Refusing Child explicitly (instead of silently downgrading to Top)
    /// keeps the LLM honest about the no-callback semantics.
    ///
    /// Image forwarding is not supported across workspaces yet — the images
    /// live in this workspace's blob store and the receiver wouldn't be able
    /// to resolve them. The `images` arg is ignored for cross-workspace.
    async fn cross_workspace_run_coding_agent(
        &self,
        prompt: &str,
        target_ws: &str,
        relation: Relation,
        coding_agent: crate::runtime::CodingAgent,
        tool_args: &serde_json::Value,
        source: SpawnSource,
    ) -> String {
        if matches!(relation, Relation::Child) {
            return format!(
                "Error: cross-workspace run_coding_agent (workspace=\"{}\") requires \
                 relation=\"top\" — child-thread auto-resume callbacks across \
                 workspaces are unsupported. Set relation=\"top\" if the spawn \
                 should run independently in {0}, or omit the workspace \
                 parameter for a same-workspace child thread.",
                target_ws
            );
        }
        let target =
            match crate::engine::http::workspace_resolver::resolve_workspace(target_ws, None) {
                Ok(t) => t,
                Err(e) => return format!("Error: cannot resolve workspace '{}': {}", target_ws, e),
            };
        let title = tool_args
            .get("title")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty());
        let repo = tool_args
            .get("repo")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty());
        let folder = tool_args
            .get("folder")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty());
        if folder.is_some() && repo.is_some() {
            return "Error: pass `folder` (preferred) or `repo` (deprecated alias), not both."
                .to_string();
        }
        // Validate the pins HERE too. This path returns before the local
        // spawn's validation, and the receiving engine cannot do it for us: it
        // sees an ordinary `ChatRequest` and would apply its own default for an
        // id it does not recognise, which is the silent-drop this whole change
        // removes. The vocabulary is compiled into both engines, so checking on
        // the sending side gives the caller the error in its own turn.
        let model = match crate::runtime::validate_coding_agent_model(
            coding_agent,
            tool_args.get("model").and_then(|v| v.as_str()),
        ) {
            Ok(m) => m,
            Err(e) => return format!("Error: {}", e),
        };
        let reasoning_effort = match crate::runtime::validate_coding_agent_effort(
            coding_agent,
            tool_args.get("reasoning_effort").and_then(|v| v.as_str()),
        ) {
            Ok(e) => e,
            Err(e) => return format!("Error: {}", e),
        };
        let ctx = crate::engine::http::workspace_client::WorkspaceCallCtx {
            self_workspace: self.workspace_name(),
            source_thread_id: Some(source.thread_id),
            source_event_id: source.tool_called_event_id,
            mode: ActorMode::Agent,
        };
        let spawn = crate::engine::http::workspace_client::CrossWorkspaceSpawn {
            prompt,
            title,
            repo,
            folder,
            coding_agent: Some(coding_agent),
            model: model.as_deref(),
            reasoning_effort: reasoning_effort.as_deref(),
        };
        match crate::engine::http::workspace_client::spawn_coding_agent_in_workspace(
            cross_workspace_http_client(),
            &target,
            &spawn,
            &ctx,
            &target.proto,
        )
        .await
        {
            Ok(cc_thread_id) => {
                relation.run_coding_agent_success_text(cc_thread_id, target_ws, coding_agent)
            }
            Err(e) => format!("Error: {}", e),
        }
    }

    /// Chat-side `ask_user_question`: walks the question batch via
    /// `walk_question_batch(channel=Chat)` and returns the joined
    /// `{question_text: answer_label}` map as a JSON string. The `Chat`
    /// channel is what makes `answer_pending_question` skip the CC-only
    /// resume side-effects (no `CodingAgentPromptSent`, no `ContinuationRequested`).
    async fn handle_chat_ask_user_question(
        &self,
        thread_id: Uuid,
        tool_use_id: &str,
        tool_args: &serde_json::Value,
    ) -> String {
        let questions = tool_args
            .get("questions")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        if !questions.is_array() {
            return "Error: `questions` must be an array of question objects".to_string();
        }

        let outcome = self
            .walk_question_batch(
                thread_id,
                tool_use_id,
                &questions,
                String::new(),
                EventChannel::Chat,
            )
            .await;
        let answer_kinds = match outcome {
            Ok(o) => o.answer_kinds,
            Err(e) => {
                crate::log!(
                    "[AskUserQuestion] chat walk failed for {thread_id}/{tool_use_id}: {e}"
                );
                return format!("Error: ask_user_question failed: {e}");
            }
        };

        if answer_kinds.is_empty() {
            // LLM sent zero questions in the batch. Surface a typed-error
            // shape rather than the empty `{}` so the model gets a clear
            // signal to retry with at least one question.
            return "Error: `questions` array was empty — pass at least one question".to_string();
        }

        let answers = crate::engine::agent_question::build_hook_answers(&answer_kinds, &questions);
        serde_json::to_string(&answers).unwrap_or_else(|_| answers.to_string())
    }

    /// Run an isolated agentic loop for intent execution.
    /// This is a simplified version of run_agentic_loop with its own system prompt,
    /// no conversation history, and all tools except execute_intent (no recursion).
    /// Returns only the final response text.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn run_intent_loop(
        &self,
        system_prompt: &str,
        task: &str,
        request_id: Uuid,
        extraction_ctx: &str,
        device_id: Option<&str>,
        cancel_token: &CancellationToken,
        thread_id: Uuid,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        // Context mode is forced OFF here, and it is the one gate this loop
        // overrides. The mode is an agreement between a chat turn's prompt and
        // its `notes` block, and this sub-loop assembles neither: it runs on
        // its own system prompt with no history and no notes block. Leaving the
        // gate open would offer a `notes` field pointing at a section of the
        // instructions that this prompt does not have.
        let tools = build_intent_tools(&crate::llm::ToolCapabilities {
            context_mode: false,
            ..self.read_turn_capabilities().await.gates
        });

        // Pin ONE provider Arc for this whole intent sub-loop — a runtime swap
        // (credential added/removed) must not change the in-flight provider.
        let provider = self.current_provider();

        let mut messages = vec![Message {
            role: "user".to_string(),
            content: MessageContent::Text(format!("Task: {}", task)),
        }];

        const MAX_INTENT_ITERATIONS: usize = 100;
        let mut iterations = 0;
        let mut last_tool_call: Option<(String, String)> = None;
        let mut consecutive_same_call = 0;

        loop {
            iterations += 1;

            if cancel_token.is_cancelled() {
                return Ok("Intent execution canceled.".to_string());
            }

            if iterations > MAX_INTENT_ITERATIONS {
                return Ok("Intent execution reached iteration limit.".to_string());
            }

            // Trim context if needed. Intent sub-loops have no user message to
            // pin, because the special-tool dispatcher synthesized the seed
            // messages rather than a user typing them. So removal runs freely
            // and there is no current-turn image to keep.
            // ~266k tokens at the 1.5 chars/token the budget assumes. (The old
            // comment here said "~100k tokens" — that was the retired chars/4
            // ratio; the value itself is unchanged.)
            let message_budget = 400_000;
            // The intent sub-loop runs no context mode. Nothing is protected,
            // nothing is held open, and a stub names its own way back exactly
            // as the control arm's does.
            trim_context_if_needed(
                &mut messages,
                message_budget,
                None,
                &[],
                crate::engine::context::TrimGuards {
                    protected: &Default::default(),
                    held_open: &Default::default(),
                    recovery: crate::engine::context::RecoveryClause::State,
                },
            );

            // Call LLM with no streaming (sub-loop doesn't stream text to frontend)
            let response = provider
                .chat(
                    messages.clone(),
                    tools.clone(),
                    None, // no model override — use default
                    Some(system_prompt),
                    None, // no token streaming callback
                    None, // no reasoning effort override
                )
                .await?;

            // No tool calls → final answer
            if response.tool_calls.is_empty() {
                return Ok(response.content.unwrap_or_else(|| "Done.".to_string()));
            }

            // Build assistant message with tool_use blocks
            let mut assistant_blocks: Vec<ContentBlock> = Vec::new();
            if let Some(ref text) = response.content {
                // Surface the sub-loop's narrative to the parent thread. We are
                // past the empty-tool-calls early return above, so this response
                // ALSO carries tool calls — meaning `text` is the connective prose
                // the intent Director wrote to set up the call, most importantly
                // the world-building that motivates an `ask_user_question` card.
                // The sub-loop runs with no token-stream callback, so without this
                // that prose is dropped and the user sees only disconnected
                // question cards referencing lore that was never written to the
                // chat (the "evig vinter never mentioned in the chat" bug). Emit
                // BEFORE the ToolCalled events below so the prose renders above the
                // card it introduces — mirroring how this loop already surfaces its
                // ToolCalled / ToolResult events to `thread_id` with EventMeta::NONE.
                if let Some(surface) = intent_narration_to_surface(Some(text)) {
                    self.event_bus
                        .emit_or_log(
                            BusEvent::Thread {
                                thread_id,
                                event: ThreadEvent::TextStreamed {
                                    text: surface.to_string(),
                                },
                                meta: EventMeta::NONE,
                            },
                            "[IntentLoop] TextStreamed",
                        )
                        .await;
                }
            }
            // `history_text`, not `content`. The surfacing above is the
            // user-facing half and stays on `content`, so Gemini's unprintable
            // narration reaches nobody. History still takes the model's own
            // words, or the next round forgets what this one decided.
            if let Some(text) = response.history_text() {
                assistant_blocks.push(ContentBlock::Text {
                    text: text.to_string(),
                });
            }
            for tc in &response.tool_calls {
                assistant_blocks.push(ContentBlock::ToolUse {
                    id: tc.id.clone(),
                    name: tc.name.clone(),
                    input: tc.arguments.clone(),
                    thought_signature: tc.thought_signature.clone(),
                });
            }
            messages.push(Message {
                role: "assistant".to_string(),
                content: MessageContent::Blocks(assistant_blocks),
            });

            // Circuit breaker for consecutive duplicate calls
            if response.tool_calls.len() == 1 {
                let tc = &response.tool_calls[0];
                let call_key = derive_call_key(&tc.name, &tc.arguments);
                let current_call = (tc.name.clone(), call_key);
                if Some(&current_call) == last_tool_call.as_ref() {
                    consecutive_same_call += 1;
                } else {
                    consecutive_same_call = 1;
                    last_tool_call = Some(current_call);
                }

                if consecutive_same_call >= 4 {
                    // Force break — add tool results and stop
                    let result_blocks: Vec<ContentBlock> = response
                        .tool_calls
                        .iter()
                        .map(|tc| ContentBlock::ToolResult {
                            tool_use_id: tc.id.clone(),
                            content: "STOP: Repeated tool call. Give your final answer now."
                                .to_string(),
                        })
                        .collect();
                    messages.push(Message {
                        role: "user".to_string(),
                        content: MessageContent::Blocks(result_blocks),
                    });
                    continue;
                }
            } else {
                consecutive_same_call = 0;
                last_tool_call = None;
            }

            // Execute each tool call
            let mut result_blocks: Vec<ContentBlock> = Vec::new();
            for tc in &response.tool_calls {
                // Emit ToolCalled via bus. Capture the event_id so spawn-style tools
                // can record it as the spawning_event_id of the new thread. Redact
                // first, then build BOTH the description and the args from the
                // redacted copy — the description renders in the steps UI, so a
                // postgres password in a bash command must not survive in it.
                let mut redacted_args = tc.arguments.clone();
                crate::core::redact_postgres_secrets_in_json(&mut redacted_args);
                let description = self.describe_tool(&tc.name, &redacted_args);
                let tool_called_event_id = self
                    .event_bus
                    .emit_for_id(BusEvent::Thread {
                        thread_id,
                        event: ThreadEvent::ToolCalled {
                            name: tc.name.clone(),
                            description,
                            args: redacted_args,
                        },
                        meta: EventMeta::NONE,
                    })
                    .await;

                let outcome: crate::engine::tools::ToolOutcome = if let Some(r) = self
                    .handle_special_tool(
                        &tc.name,
                        &tc.arguments,
                        thread_id,
                        None,
                        device_id,
                        tool_called_event_id,
                        &tc.id,
                        // Intent sub-loops expose only `build_intent_tools()`
                        // (no dynamically-discovered MCP tools), so the MCP
                        // permission branch is unreachable here — NONE is safe.
                        &EventMeta::NONE,
                        cancel_token,
                    )
                    .await
                {
                    crate::engine::tools::lift_legacy_string(r)
                } else {
                    super::run_tool_with_cancel(
                        self.execute_tool(
                            &tc.name,
                            &tc.arguments,
                            extraction_ctx,
                            request_id,
                            device_id,
                            cancel_token,
                            thread_id,
                        ),
                        cancel_token,
                    )
                    .await
                };

                let (result, is_error) = match outcome {
                    Ok(text) => (text, false),
                    Err(text) => (text, true),
                };
                self.event_bus
                    .emit_or_log(
                        BusEvent::Thread {
                            thread_id,
                            event: ThreadEvent::ToolResult {
                                name: tc.name.clone(),
                                result: crate::core::sanitize_for_jsonb(&result),
                                images: vec![],
                                success: !is_error,
                                // Same pairing contract as the main agentic
                                // loop: without the call's event id the
                                // frontend cannot route this result to its
                                // originating exchange, so an intent sub-loop
                                // that asks a question left the caller's
                                // "Executing ..." spinner pending forever.
                                tool_called_event_id,
                            },
                            meta: EventMeta::NONE,
                        },
                        "[IntentLoop] ToolResult",
                    )
                    .await;

                result_blocks.push(ContentBlock::ToolResult {
                    tool_use_id: tc.id.clone(),
                    content: result,
                });
            }

            messages.push(Message {
                role: "user".to_string(),
                content: MessageContent::Blocks(result_blocks),
            });
        }
    }
}

/// Decide whether an intent sub-loop LLM response carries narrative prose worth
/// surfacing to the parent thread as a `TextStreamed` event. Returns the text to
/// emit, or `None` for absent / whitespace-only content (a turn that is a pure
/// tool call with no prose must not emit an empty bubble). The text is returned
/// verbatim — the model's own paragraph breaks are preserved so it renders as
/// markdown; only the empty-content decision is made here.
fn intent_narration_to_surface(content: Option<&str>) -> Option<&str> {
    content.filter(|text| !text.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn intent_narration_surfaces_real_prose() {
        // The world-building the Director writes before an ask_user_question is
        // exactly what must reach the chat.
        let prose = "Et evig vinterlandskap ligger over fjordriket.\n\nHvem er fienden?";
        assert_eq!(intent_narration_to_surface(Some(prose)), Some(prose));
    }

    #[test]
    fn intent_narration_skips_absent_and_whitespace_only() {
        // A pure tool-call turn (no prose) must not emit an empty TextStreamed.
        assert_eq!(intent_narration_to_surface(None), None);
        assert_eq!(intent_narration_to_surface(Some("")), None);
        assert_eq!(intent_narration_to_surface(Some("  \n\t  ")), None);
    }

    #[test]
    fn parse_coding_agent_defaults_to_claude_code() {
        assert_eq!(
            parse_coding_agent(&json!({})).unwrap(),
            crate::runtime::CodingAgent::ClaudeCode
        );
    }

    #[test]
    fn parse_coding_agent_accepts_codex() {
        assert_eq!(
            parse_coding_agent(&json!({"coding_agent": "codex"})).unwrap(),
            crate::runtime::CodingAgent::Codex
        );
    }

    #[test]
    fn parse_coding_agent_rejects_unknown_backend() {
        let err = parse_coding_agent(&json!({"coding_agent": "cc"})).unwrap_err();
        assert!(
            err.contains("coding_agent"),
            "error should name the invalid field: {err}"
        );
    }

    #[test]
    fn codex_success_text_names_codex() {
        let tid = Uuid::nil();
        let text = Relation::Top.run_coding_agent_success_text(
            tid,
            "dev",
            crate::runtime::CodingAgent::Codex,
        );
        assert!(text.contains("Codex session"), "{text}");
        assert!(!text.contains("Claude Code"), "{text}");
    }

    // --- run_thread route pins ----------------------------------------------

    /// The account preferences, as `user_chat_settings` hands them over.
    fn chat_prefs() -> (Option<String>, Option<String>) {
        (
            Some("claude-opus-5@default".to_string()),
            Some("xhigh".to_string()),
        )
    }

    fn resolved(args: serde_json::Value) -> (Option<String>, Option<String>) {
        let (model, effort) = chat_prefs();
        resolve_sub_thread_model_and_effort(&args, model, effort).expect("valid pins")
    }

    /// The pre-existing behavior: a spawn that pins nothing runs on the
    /// account's chat model and effort.
    #[test]
    fn run_thread_without_pins_inherits_both_chat_preferences() {
        assert_eq!(
            resolved(json!({"prompt": "summarize this"})),
            (
                Some("claude-opus-5@default".to_string()),
                Some("xhigh".to_string())
            )
        );
    }

    /// The two pins resolve independently. Naming the model must not drag the
    /// effort down to a provider default, and vice versa.
    #[test]
    fn run_thread_model_pin_leaves_the_preference_effort_inherited() {
        assert_eq!(
            resolved(json!({"prompt": "p", "model": "claude-sonnet-5"})),
            (
                Some("claude-sonnet-5".to_string()),
                Some("xhigh".to_string())
            )
        );
    }

    #[test]
    fn run_thread_effort_pin_leaves_the_preference_model_inherited() {
        assert_eq!(
            resolved(json!({"prompt": "p", "reasoning_effort": "low"})),
            (
                Some("claude-opus-5@default".to_string()),
                Some("low".to_string())
            )
        );
    }

    #[test]
    fn run_thread_takes_both_pins_together() {
        assert_eq!(
            resolved(json!({
                "prompt": "p",
                "model": "claude-sonnet-5",
                "reasoning_effort": "none",
            })),
            (
                Some("claude-sonnet-5".to_string()),
                Some("none".to_string())
            )
        );
    }

    /// The tier vocabulary is ours, so a typo is a caller error. Rejecting it
    /// by name is what stops the spawn quietly running at `xhigh` while the
    /// caller believes it asked for something cheaper.
    #[test]
    fn run_thread_rejects_an_unknown_reasoning_effort_and_names_it() {
        let err = resolve_sub_thread_model_and_effort(
            &json!({"prompt": "p", "reasoning_effort": "cheap"}),
            None,
            None,
        )
        .unwrap_err();
        assert!(err.contains("cheap"), "error names the bad value: {err}");
        assert!(
            err.contains("none, low, medium, high, xhigh, max"),
            "error lists the tiers: {err}"
        );
    }

    /// The model id is NOT checked against the registry, matching the trigger
    /// route pins: a row can be disabled long after the caller read it, and a
    /// bad id surfaces as an ordinary thread failure instead.
    #[test]
    fn run_thread_passes_an_unknown_model_id_through_unchecked() {
        assert_eq!(
            resolved(json!({"prompt": "p", "model": "claude-imaginary-9"})).0,
            Some("claude-imaginary-9".to_string())
        );
    }

    /// Blank and null both read as "no pin", the same normalization the trigger
    /// form's Default option travels through. `Some("")` must never reach the
    /// provider as a model id.
    #[test]
    fn run_thread_reads_a_blank_or_null_pin_as_the_account_default() {
        assert_eq!(
            resolved(json!({"prompt": "p", "model": "   "})),
            chat_prefs()
        );
        assert_eq!(
            resolved(json!({"prompt": "p", "model": null, "reasoning_effort": null})),
            chat_prefs()
        );
        assert_eq!(
            resolved(json!({"prompt": "p", "reasoning_effort": "  "})),
            chat_prefs()
        );
    }

    /// A wrong-typed pin is refused rather than read as absent: silently
    /// substituting the account default would tell the caller its pin applied.
    #[test]
    fn run_thread_rejects_a_non_string_pin() {
        for args in [
            json!({"prompt": "p", "model": 5}),
            json!({"prompt": "p", "reasoning_effort": ["low"]}),
        ] {
            let err = resolve_sub_thread_model_and_effort(&args, None, None)
                .expect_err("a non-string pin must be refused");
            assert!(
                err.contains("must be a string"),
                "error states the contract: {err}"
            );
        }
    }

    // --- linkage vs attribution --------------------------------------------

    /// A `Top` spawn reports back to nobody: no parent linkage, so
    /// `notify_parent_if_child` early-returns and the projection's spawn branch
    /// never bumps the spawning thread's counts.
    #[test]
    fn top_spawn_carries_no_callback_linkage() {
        let spawning_thread = Uuid::new_v4();
        let tool_call = Uuid::new_v4();
        assert_eq!(
            Relation::Top.spawn_linkage(spawning_thread, Some(tool_call)),
            (None, None)
        );
    }

    #[test]
    fn child_spawn_carries_the_callback_linkage() {
        let spawning_thread = Uuid::new_v4();
        let tool_call = Uuid::new_v4();
        assert_eq!(
            Relation::Child.spawn_linkage(spawning_thread, Some(tool_call)),
            (Some(spawning_thread), Some(tool_call))
        );
    }

    /// The bug this pair of methods exists to prevent: a `Top` spawn dropped the
    /// linkage AND the attribution, so its route popover said "Unknown". The
    /// origin names the spawning thread regardless of relation.
    #[test]
    fn every_spawn_is_attributed_to_its_spawning_thread() {
        use crate::engine::thread_events::{MessageOrigin, ThreadDirection};
        let spawning_thread = Uuid::new_v4();
        let tool_call = Uuid::new_v4();
        let expected = MessageOrigin::ThreadLink {
            thread_id: spawning_thread,
            title: None,
            spawning_event_id: Some(tool_call),
            mode: ActorMode::Agent,
            direction: ThreadDirection::Parent,
        };
        assert_eq!(
            spawn_origin(spawning_thread, Some(tool_call)),
            Some(expected)
        );
    }

    /// `make_message_received` `.expect()`s on an origin whose `mode()` disagrees
    /// with the carried `ActorMode`, and both spawn paths emit with
    /// `ActorMode::Agent`. Pin the pairing so a drift is a failed test rather
    /// than a panic in the Thread Queue prepare hook.
    #[test]
    fn spawn_origin_mode_matches_the_agent_emit_sites() {
        let origin = spawn_origin(Uuid::new_v4(), None).expect("always Some");
        assert_eq!(origin.mode(), ActorMode::Agent);
    }

    /// A child spawn's payload must not change: the explicit origin has to equal
    /// what `synthesize_legacy_origin` built from its linkage before this split
    /// existed. Compared through a real `MessageReceived`, which is where the
    /// two meet.
    #[test]
    fn child_spawn_origin_equals_what_legacy_synthesis_produced() {
        use crate::engine::thread_events::ThreadEvent;
        let spawning_thread = Uuid::new_v4();
        let tool_call = Uuid::new_v4();
        let workspace = std::path::Path::new("/tmp/lucidos-test-ws");
        let (parent_thread_id, spawning_event_id) =
            Relation::Child.spawn_linkage(spawning_thread, Some(tool_call));

        // `explicit_origin: None` takes the legacy-synthesis branch.
        let synthesized = crate::engine::chat::make_message_received(
            workspace,
            "do the thing",
            None,
            None,
            None,
            parent_thread_id,
            spawning_event_id,
            ActorMode::Agent,
            None,
            None,
            None,
            None,
        );
        let explicit = crate::engine::chat::make_message_received(
            workspace,
            "do the thing",
            None,
            None,
            None,
            parent_thread_id,
            spawning_event_id,
            ActorMode::Agent,
            None,
            None,
            spawn_origin(spawning_thread, Some(tool_call)),
            None,
        );
        let (
            ThreadEvent::MessageReceived { origin: a, .. },
            ThreadEvent::MessageReceived { origin: b, .. },
        ) = (&synthesized, &explicit)
        else {
            panic!("make_message_received returns MessageReceived");
        };
        assert_eq!(a, b, "a child spawn's persisted origin must be unchanged");
    }
}
