use crate::engine::tools::ToolDispatchExt;
use uuid::Uuid;

use crate::engine::http::workspace_client::cross_workspace_http_client;
use crate::engine::thread_events::{ActorMode, EventChannel, EventMeta, WakeMode};
use crate::engine::LucidosEngine;
use crate::llm::tool_names as tn;
use crate::triggers::{normalize_route_setting, validate_trigger_reasoning_effort};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Relation {
    Child(WakeMode),
    Top,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct SpawnSource {
    thread_id: Uuid,
    tool_called_event_id: Option<Uuid>,
}

pub(crate) fn parse_relation(
    args: &serde_json::Value,
    parent_is_home: bool,
) -> Result<Relation, String> {
    let declared = WakeMode::from_arg(optional_string_arg(args, "wake_mode")?)?;
    let child = || Relation::Child(WakeMode::for_spawn(declared, parent_is_home));
    match args.get("relation") {
        None | Some(serde_json::Value::Null) => Ok(child()),
        Some(serde_json::Value::String(s)) => match s.as_str() {
            "child" | "sub" => Ok(child()),
            "top" if declared.is_some() => Err(
                "wake_mode applies to relation \"child\" only: a top thread never reports back"
                    .to_string(),
            ),
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

pub(crate) fn parse_coding_agent(
    args: &serde_json::Value,
) -> Result<Option<crate::runtime::CodingAgent>, String> {
    match args.get("coding_agent") {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::String(s)) => match s.as_str() {
            "claude-code" | "claude_code" => Ok(Some(crate::runtime::CodingAgent::ClaudeCode)),
            "codex" => Ok(Some(crate::runtime::CodingAgent::Codex)),
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

pub(crate) fn pick_installed_coding_agent(
    requested: Option<crate::runtime::CodingAgent>,
    is_installed: impl Fn(crate::runtime::CodingAgent) -> bool,
) -> Result<crate::runtime::CodingAgent, String> {
    use crate::runtime::CodingAgent;
    if let Some(agent) = requested.filter(|a| is_installed(*a)) {
        return Ok(agent);
    }
    let installed: Vec<CodingAgent> = CodingAgent::ALL
        .into_iter()
        .filter(|a| is_installed(*a))
        .collect();
    match (requested, installed.first()) {
        (None, Some(first)) => Ok(*first),
        (Some(asked), Some(alt)) => Err(format!(
            "{} is not installed on this machine. {} is installed: retry with \
             coding_agent \"{}\", or omit coding_agent.",
            asked.display_name(),
            alt.display_name(),
            alt.as_str()
        )),
        (_, None) => Err(
            "No coding agent is installed on this machine, so no session can start. \
             Tell the user to install Claude Code or Codex, or to set its binary path in \
             Settings › Coding agents."
                .to_string(),
        ),
    }
}

pub(crate) fn sub_thread_route_overrides(
    args: &serde_json::Value,
    caller: &crate::core::ResolvedModelSelection,
) -> Result<crate::core::ResolvedModelSelection, String> {
    let pinned_model = normalize_route_setting(optional_string_arg(args, "model")?);
    let pinned_effort =
        validate_trigger_reasoning_effort(optional_string_arg(args, "reasoning_effort")?)?;
    let inherited = match pinned_model {
        Some(model) if caller.model.as_ref() != Some(&model) => {
            crate::core::ResolvedModelSelection {
                model: Some(model),
                ..Default::default()
            }
        }
        _ => caller.clone(),
    };
    Ok(crate::core::ResolvedModelSelection {
        reasoning_effort: pinned_effort.or(inherited.reasoning_effort),
        ..inherited
    })
}

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

const MCP_ARGS_SUMMARY_LIMIT: usize = 500;

fn mcp_arguments_summary(tool_args: &serde_json::Value) -> String {
    let mut redacted = tool_args.clone();
    crate::core::redact_postgres_secrets_in_json(&mut redacted);
    let text = serde_json::to_string_pretty(&redacted).unwrap_or_else(|_| redacted.to_string());
    if text.len() > MCP_ARGS_SUMMARY_LIMIT {
        format!(
            "{}...",
            &text[..text.floor_char_boundary(MCP_ARGS_SUMMARY_LIMIT)]
        )
    } else {
        text
    }
}

fn child_report_contract(mode: WakeMode) -> &'static str {
    match mode {
        WakeMode::Wake => {
            "Its wake_mode is \"wake\": when it finishes, this conversation \
             automatically resumes with its results, so you don't need to check on it."
        }
        WakeMode::WakeWhenAllDone => {
            "Its wake_mode is \"wake-when-all-done\": its clean report is filed while \
             another child you spawned that way is still working, and the last one to \
             finish resumes this conversation. A failure resumes it at once."
        }
        WakeMode::NoWake => {
            "Its wake_mode is \"no-wake\": no finish resumes this conversation, not even \
             a failure, since the user reads the result in that thread. Its report is \
             filed for your next turn."
        }
    }
}

#[easy_ext::ext(AgenticLoopSpecialToolExt)]
pub(crate) impl LucidosEngine {
    async fn pick_installed_coding_agent(
        &self,
        requested: Option<crate::runtime::CodingAgent>,
    ) -> Result<crate::runtime::CodingAgent, String> {
        let installed = crate::runtime::installed_coding_agents(self.pool()).await;
        pick_installed_coding_agent(requested, |agent| installed.contains(&agent))
    }
}

fn coding_agent_label(agent: crate::runtime::CodingAgent) -> &'static str {
    match agent {
        crate::runtime::CodingAgent::ClaudeCode => "Claude Code",
        crate::runtime::CodingAgent::Codex => "Codex",
    }
}

impl Relation {
    pub(crate) fn spawn_linkage(
        self,
        spawning_thread_id: Uuid,
        tool_called_event_id: Option<Uuid>,
    ) -> (Option<Uuid>, Option<Uuid>) {
        match self {
            Self::Child(_) => (Some(spawning_thread_id), tool_called_event_id),
            Self::Top => (None, None),
        }
    }

    pub(crate) fn wake_mode(self) -> Option<WakeMode> {
        match self {
            Self::Child(mode) => Some(mode),
            Self::Top => None,
        }
    }

    pub(crate) fn run_thread_success_text(self, child_thread_id: Uuid, workspace: &str) -> String {
        match self {
            Self::Child(mode) => format!(
                "Thread started in workspace '{ws}' (thread_id: {tid}). {contract} \
                 Continue with other work or tell the user you've delegated this \
                 subtask in the {ws} workspace: [Open thread]({link})",
                contract = child_report_contract(mode),
                ws = workspace,
                tid = child_thread_id,
                link = crate::core::store::thread_link(workspace, child_thread_id),
            ),
            Self::Top => format!(
                "Top-level thread started in workspace '{ws}' (thread_id: {tid}). It \
                 runs independently and will NOT report back to this conversation. Tell \
                 the user you've started a separate thread in the {ws} workspace for \
                 them to follow: [Open thread]({link})",
                ws = workspace,
                tid = child_thread_id,
                link = crate::core::store::thread_link(workspace, child_thread_id),
            ),
        }
    }

    pub(crate) fn queued_spawn_text(self, label: &str, thread_id: Uuid, position: usize) -> String {
        let base = format!(
            "{label} queued at position {position} in the Thread Queue — Lucidos is at \
             background capacity. It starts automatically when a slot frees; its \
             thread_id will be {thread_id}."
        );
        match self {
            Self::Child(mode) => format!("{base} {}", child_report_contract(mode)),
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
            Self::Child(mode) => format!(
                "{label} session started in workspace '{ws}' (thread_id: {tid}). \
                 {contract} Tell the user you've started a new {label} thread in the \
                 {ws} workspace and include this link: [Open thread]({link})",
                contract = child_report_contract(mode),
                label = label,
                ws = workspace,
                tid = cc_thread_id,
                link = crate::core::store::thread_link(workspace, cc_thread_id),
            ),
            Self::Top => format!(
                "Top-level {label} session started in workspace '{ws}' (thread_id: \
                 {tid}). It runs independently and will NOT report back to this \
                 conversation. Include the link in your reply so the user can follow it: \
                 [Open thread]({link})",
                label = label,
                ws = workspace,
                tid = cc_thread_id,
                link = crate::core::store::thread_link(workspace, cc_thread_id),
            ),
        }
    }
}

pub(super) struct ChatAskOutcome {
    pub result: String,
    pub quiet: bool,
    pub quiet_reply: Option<String>,
}

impl ChatAskOutcome {
    fn failed(result: impl Into<String>) -> Self {
        Self {
            result: result.into(),
            quiet: false,
            quiet_reply: None,
        }
    }
}

#[easy_ext::ext(SpecialToolDispatchExt)]
pub(super) impl LucidosEngine {
    #[allow(clippy::too_many_arguments)]
    async fn handle_special_tool(
        &self,
        tool_name: &str,
        tool_args: &serde_json::Value,
        thread_id: Uuid,
        user_images: Option<&[crate::engine::types::ChatImage]>,
        device_id: Option<&str>,
        tool_called_event_id: Option<Uuid>,
        tool_use_id: &str,
        meta: &EventMeta,
        cancel_token: &tokio_util::sync::CancellationToken,
        caller_route: &crate::core::ResolvedModelSelection,
    ) -> Option<String> {
        if tool_name == tn::CAPTURE_APP || tool_name == tn::REFRESH_APP {
            let last_used = self.last_used_device(thread_id, device_id).await;
            Some(
                crate::engine::tools::app_capture::app_ui_tool_impl(
                    &self.event_bus,
                    &self.pool,
                    &self.pending_captures,
                    &self.artifact_manager,
                    tool_args,
                    thread_id,
                    tool_name == tn::REFRESH_APP,
                    last_used.as_deref(),
                    crate::engine::tools::app_capture::CAPTURE_TIMEOUT,
                )
                .await,
            )
        } else if tool_name == tn::RUN_CODING_AGENT || tool_name == tn::RUN_CLAUDE_LEGACY {
            let prompt = tool_args["prompt"].as_str().unwrap_or("");
            if prompt.is_empty() {
                Some("Error: prompt is required".to_string())
            } else {
                let parent_is_home =
                    crate::engine::home_thread::spawner_is_home(&self.pool, thread_id).await;
                let relation = match parse_relation(tool_args, parent_is_home) {
                    Ok(r) => r,
                    Err(e) => return Some(format!("Error: {}", e)),
                };
                let requested_agent = match parse_coding_agent(tool_args) {
                    Ok(agent) => agent,
                    Err(e) => return Some(format!("Error: {}", e)),
                };
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
                let workspace_arg = tool_args
                    .get("workspace")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty());
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
                            requested_agent.unwrap_or(crate::runtime::CodingAgent::ClaudeCode),
                            tool_args,
                            SpawnSource {
                                thread_id,
                                tool_called_event_id,
                            },
                        )
                        .await,
                    );
                }
                let coding_agent = match self.pick_installed_coding_agent(requested_agent).await {
                    Ok(agent) => agent,
                    Err(e) => return Some(format!("Error: {e}")),
                };
                let spawn_model = match crate::runtime::validate_coding_agent_model(
                    coding_agent,
                    tool_args.get("model").and_then(|v| v.as_str()),
                ) {
                    Ok(m) => m,
                    Err(e) => return Some(format!("Error: {}", e)),
                };
                let spawn_effort = match crate::runtime::validate_coding_agent_effort(
                    coding_agent,
                    spawn_model.as_deref(),
                    tool_args.get("reasoning_effort").and_then(|v| v.as_str()),
                ) {
                    Ok(e) => e,
                    Err(e) => return Some(format!("Error: {}", e)),
                };
                if let Err(e) = crate::runtime::intelligence_ladder::check_agent_pick(
                    self.pool(),
                    &self.model_routes(),
                    coding_agent.into(),
                    spawn_model.as_deref(),
                    spawn_effort.as_deref(),
                )
                .await
                {
                    return Some(format!("Error: {e}"));
                }
                let (parent_thread_id, spawning_event_id) =
                    relation.spawn_linkage(thread_id, tool_called_event_id);
                let origin = spawn_origin(thread_id, tool_called_event_id);

                if let Some(err) = removed_repo_alias_error(tool_args) {
                    return Some(err.to_string());
                }
                let folder_input = tool_args
                    .get("folder")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty());
                if folder_input.is_none() && !crate::paths::has_lucidos_source() {
                    return Some(format!(
                        "Error: {}",
                        crate::engine::agent_session::err_no_lucidos_source()
                    ));
                }
                let mut spawn_repo_id: Option<String> = None;
                let mut spawn_app_id: Option<String> = None;
                if let Some(folder_in) = folder_input {
                    use crate::engine::agent_session::{
                        classify_resolved_folder, resolve_folder_input, FolderClassification,
                    };
                    let ws_root = self.workspace_path().to_path_buf();
                    let pool = self.pool.clone();
                    let folder_in_owned = folder_in.to_string();
                    // INVARIANT: Snapshot the repo registry up front. The classifier's
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
                        Ok(FolderClassification::Lucidos { .. }) => {}
                        Ok(FolderClassification::App { app_id, .. }) => {
                            spawn_app_id = Some(app_id);
                        }
                        Ok(FolderClassification::External { repo_root }) => {
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

                let resolved_images: Option<Vec<crate::engine::types::ChatImage>> = match tool_args
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

                let caller_title = tool_args["title"].as_str();
                let images = resolved_images.as_deref().or(user_images);
                let cc_thread_id = uuid::Uuid::new_v4();
                let request = crate::engine::thread_queue::ThreadQueueRequest::CodingAgent {
                    depth: 0,
                    prompt: prompt.to_string(),
                    cc_thread_id,
                    image_hashes: self.queued_image_hashes(images),
                    device_id: device_id.map(str::to_string),
                    parent_thread_id,
                    spawning_event_id,
                    wake_mode: relation.wake_mode(),
                    repo_id,
                    title: caller_title.map(str::to_string),
                    app_id: spawn_app_id,
                    coding_agent,
                    model: spawn_model,
                    reasoning_effort: spawn_effort,
                    origin,
                };
                let outcome = self.thread_queue.submit(request, None).await;
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
                let parent_is_home =
                    crate::engine::home_thread::spawner_is_home(&self.pool, thread_id).await;
                let relation = match parse_relation(tool_args, parent_is_home) {
                    Ok(r) => r,
                    Err(e) => return Some(format!("Error: {}", e)),
                };
                if let Err(e) = crate::runtime::intelligence_ladder::check_agent_pick(
                    self.pool(),
                    &self.model_routes(),
                    crate::runtime::intelligence_ladder::LadderSurface::Chat,
                    normalize_route_setting(tool_args["model"].as_str()).as_deref(),
                    normalize_route_setting(tool_args["reasoning_effort"].as_str()).as_deref(),
                )
                .await
                {
                    return Some(format!("Error: {e}"));
                }
                let (parent_thread_id, spawning_event_id) =
                    relation.spawn_linkage(thread_id, tool_called_event_id);
                let origin = spawn_origin(thread_id, tool_called_event_id);
                let max_live_children = self
                    .thread_queue
                    .policy()
                    .await
                    .max_concurrent_children_per_thread;
                Some(
                    match LucidosEngine::check_thread_recursion_guard(
                        &self.pool,
                        thread_id,
                        max_live_children,
                    )
                    .await
                    {
                        Err(guard_err) => format!("Error: {}", guard_err),
                        Ok(_) => {
                            let child_thread_id = uuid::Uuid::new_v4();
                            let overrides =
                                match sub_thread_route_overrides(tool_args, caller_route) {
                                    Ok(overrides) => overrides,
                                    Err(e) => return Some(format!("Error: {}", e)),
                                };
                            // INVARIANT: Resolved and clamped now, so the child's eager
                            // MessageReceived stamps the route it runs on.
                            let route =
                                crate::engine::chat::process::resolve_available_route_overrides(
                                    &self.pool,
                                    &self.model_routes(),
                                    None,
                                    None,
                                    None,
                                    overrides,
                                )
                                .await;
                            let caller_title = tool_args["title"].as_str();
                            // INVARIANT: The eager MessageReceived (a Child relation child's
                            // active_children_count must increment before the parent
                            // can complete this turn) lives in the Thread Queue
                            // executor's `prepare` hook — `submit` awaits it inline
                            // on the admitted path, preserving the ordering.
                            let request =
                                crate::engine::thread_queue::ThreadQueueRequest::SubThread {
                                    depth: 0,
                                    prompt: prompt.to_string(),
                                    child_thread_id,
                                    parent_thread_id,
                                    spawning_event_id,
                                    wake_mode: relation.wake_mode(),
                                    title: caller_title.map(str::to_string),
                                    model: route.model,
                                    reasoning_effort: route.reasoning_effort,
                                    provider: route.provider,
                                    pre_emitted_origin: None,
                                    origin,
                                };
                            let outcome = self.thread_queue.submit(request, None).await;
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
            use crate::engine::mcp_permission::{
                mcp_gate, unlisted_trigger_refusal, McpAsk, McpGate,
            };

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

            if let Some(refusal) = self
                .mcp_manager
                .not_enabled_refusal(thread_id, &server_id, tool_name)
                .await
            {
                return Some(format!("Error: {refusal}"));
            }

            let firing_trigger = if meta.channel == Some(EventChannel::Trigger) {
                self.trigger_of_thread(thread_id).await
            } else {
                None
            };
            let listed_by_trigger = firing_trigger
                .as_ref()
                .is_some_and(|t| t.mcp_servers.contains(&server_id));
            match mcp_gate(auto_approve, meta.channel, listed_by_trigger) {
                McpGate::Proceed => {}
                McpGate::Refuse => {
                    self.notify_unlisted_trigger_mcp_call(
                        thread_id,
                        firing_trigger.as_ref(),
                        &server_name,
                    )
                    .await;
                    return Some(unlisted_trigger_refusal(tool_name, &server_id));
                }
                McpGate::Ask => {
                    let args_summary = mcp_arguments_summary(tool_args);
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
            }

            Some(
                match self
                    .mcp_manager
                    .call_tool(thread_id, &server_id, &mcp_tool_name, tool_args.clone())
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

    async fn cross_workspace_run_coding_agent(
        &self,
        prompt: &str,
        target_ws: &str,
        relation: Relation,
        coding_agent: crate::runtime::CodingAgent,
        tool_args: &serde_json::Value,
        source: SpawnSource,
    ) -> String {
        if matches!(relation, Relation::Child(_)) {
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
        if let Some(err) = removed_repo_alias_error(tool_args) {
            return err.to_string();
        }
        let folder = tool_args
            .get("folder")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty());
        let requested_model = tool_args.get("model").and_then(|v| v.as_str());
        let model = match coding_agent {
            crate::runtime::CodingAgent::ClaudeCode => requested_model
                .map(str::trim)
                .filter(|m| !m.is_empty())
                .map(str::to_string),
            crate::runtime::CodingAgent::Codex => {
                match crate::runtime::validate_coding_agent_model(coding_agent, requested_model) {
                    Ok(m) => m,
                    Err(e) => return format!("Error: {}", e),
                }
            }
        };
        let effort_model = match coding_agent {
            crate::runtime::CodingAgent::ClaudeCode => None,
            crate::runtime::CodingAgent::Codex => model.as_deref(),
        };
        let reasoning_effort = match crate::runtime::validate_coding_agent_effort(
            coding_agent,
            effort_model,
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
            folder,
            coding_agent: Some(coding_agent),
            model: model.as_deref(),
            reasoning_effort: reasoning_effort.as_deref(),
        };
        let verdict = crate::engine::agent_session::spawn_gate::thread_spawn_verdict(
            self.event_bus.pool(),
            &self.trigger_configs,
            source.thread_id,
        )
        .await;
        match crate::engine::http::workspace_client::spawn_coding_agent_in_workspace(
            cross_workspace_http_client(),
            &target,
            &spawn,
            &ctx,
            &target.proto,
            crate::core::base_path::gateway_port().as_deref(),
            &verdict,
        )
        .await
        {
            Ok(cc_thread_id) => {
                relation.run_coding_agent_success_text(cc_thread_id, target_ws, coding_agent)
            }
            Err(e) => format!("Error: {}", e),
        }
    }

    /// INVARIANT: Chat-side `ask_user_question`: walks the question batch via
    /// `walk_question_batch(channel=Chat)` and returns the joined
    /// `{question_text: answer_label}` map as a JSON string. The `Chat`
    /// channel is what makes `answer_pending_question` skip the CC-only
    /// resume side-effects (no `CodingAgentPromptSent`, no `ContinuationRequested`).
    /// The card's `message` renders in the turn `meta` belongs to.
    async fn handle_chat_ask_user_question(
        &self,
        thread_id: Uuid,
        tool_use_id: &str,
        tool_args: &serde_json::Value,
        meta: &EventMeta,
    ) -> ChatAskOutcome {
        let questions = tool_args
            .get("questions")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        if !questions.is_array() {
            return ChatAskOutcome::failed(
                "Error: `questions` must be an array of question objects",
            );
        }

        use crate::engine::permission::prepare::{prepared_ask, PreparedAsk};
        let (questions, permission_request) =
            match prepared_ask(self.pool(), thread_id, &questions).await {
                Ok(PreparedAsk::Ordinary) => (questions, None),
                Ok(PreparedAsk::Request { request, questions }) => (questions, Some(request)),
                Err(refusal) => return ChatAskOutcome::failed(format!("Error: {refusal}")),
            };

        use crate::engine::agent_question::{card_message, CardMessage};
        let message = card_message(tool_args).map(|text| CardMessage { text, meta });
        let outcome = self
            .walk_question_batch(
                thread_id,
                tool_use_id,
                &questions,
                message,
                crate::engine::agent_question::QuestionAsker {
                    cc_session_id: String::new(),
                    channel: EventChannel::Chat,
                    permission_request,
                },
            )
            .await;
        let outcome = match outcome {
            Ok(o) => o,
            Err(e) => {
                crate::log_legacy!(
                    "[AskUserQuestion] chat walk failed for {thread_id}/{tool_use_id}: {e}"
                );
                return ChatAskOutcome::failed(format!("Error: ask_user_question failed: {e}"));
            }
        };

        if outcome.answer_kinds.is_empty() {
            return ChatAskOutcome::failed(
                "Error: `questions` array was empty — pass at least one question",
            );
        }

        use crate::engine::agent_question::{build_hook_answers, with_answer_images, AnswerImages};
        let answers = build_hook_answers(
            &outcome.answer_kinds,
            &questions,
            AnswerImages::AttachedBlocks,
        );
        let result = serde_json::to_string(&answers).unwrap_or_else(|_| answers.to_string());
        ChatAskOutcome {
            result: with_answer_images(result, &outcome.answer_kinds),
            quiet: outcome.quiet,
            quiet_reply: outcome.quiet_reply,
        }
    }
}

fn removed_repo_alias_error(tool_args: &serde_json::Value) -> Option<&'static str> {
    tool_args
        .get("repo")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .map(|_| "Error: `repo` was renamed to `folder`. Pass `folder` instead.")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_removed_repo_alias_is_rejected_with_a_rename_error() {
        assert_eq!(
            removed_repo_alias_error(&json!({ "repo": "Lucidos" })),
            Some("Error: `repo` was renamed to `folder`. Pass `folder` instead.")
        );
        assert!(removed_repo_alias_error(&json!({ "folder": "data/apps/x" })).is_none());
        assert!(removed_repo_alias_error(&json!({ "repo": "" })).is_none());
        assert!(removed_repo_alias_error(&json!({ "repo": "  " })).is_none());
    }

    #[test]
    fn mcp_arguments_summary_redacts_a_postgres_password() {
        let args = json!({
            "conn": "postgresql://app:hunter2@db.example.com/prod",
            "nested": { "url": "postgres://u:s3cr3t@h/db" },
            "sql": "SELECT 1",
        });
        let summary = mcp_arguments_summary(&args);
        assert!(!summary.contains("hunter2"), "{summary}");
        assert!(!summary.contains("s3cr3t"), "{summary}");
        assert!(summary.contains("app:***@"), "{summary}");
        assert!(summary.contains("SELECT 1"), "{summary}");
    }

    #[test]
    fn mcp_arguments_summary_clamps_on_a_char_boundary() {
        let args = json!({ "note": "æ".repeat(2000) });
        let summary = mcp_arguments_summary(&args);
        assert!(summary.len() <= 503, "{}", summary.len());
        assert!(summary.ends_with("..."));
    }

    #[test]
    fn parse_coding_agent_leaves_an_omitted_agent_unnamed() {
        assert_eq!(parse_coding_agent(&json!({})).unwrap(), None);
        assert_eq!(
            parse_coding_agent(&json!({"coding_agent": null})).unwrap(),
            None
        );
    }

    #[test]
    fn parse_coding_agent_accepts_codex() {
        assert_eq!(
            parse_coding_agent(&json!({"coding_agent": "codex"})).unwrap(),
            Some(crate::runtime::CodingAgent::Codex)
        );
    }

    #[test]
    fn an_omitted_agent_prefers_claude_code_when_both_are_installed() {
        use crate::runtime::CodingAgent;
        assert_eq!(
            pick_installed_coding_agent(None, |_| true),
            Ok(CodingAgent::ClaudeCode)
        );
    }

    #[test]
    fn an_omitted_agent_falls_back_to_the_installed_one() {
        use crate::runtime::CodingAgent;
        assert_eq!(
            pick_installed_coding_agent(None, |a| a == CodingAgent::Codex),
            Ok(CodingAgent::Codex)
        );
    }

    #[test]
    fn an_explicit_installed_agent_is_kept() {
        use crate::runtime::CodingAgent;
        assert_eq!(
            pick_installed_coding_agent(Some(CodingAgent::Codex), |_| true),
            Ok(CodingAgent::Codex)
        );
    }

    #[test]
    fn an_explicit_missing_agent_is_refused_and_names_the_installed_one() {
        use crate::runtime::CodingAgent;
        let err =
            pick_installed_coding_agent(Some(CodingAgent::ClaudeCode), |a| a == CodingAgent::Codex)
                .unwrap_err();
        assert!(err.contains("Claude Code is not installed"), "{err}");
        assert!(err.contains("\"codex\""), "{err}");
    }

    #[test]
    fn no_installed_agent_refuses_an_omitted_or_explicit_request() {
        use crate::runtime::CodingAgent;
        for requested in [None, Some(CodingAgent::Codex)] {
            let err = pick_installed_coding_agent(requested, |_| false).unwrap_err();
            assert!(err.contains("No coding agent is installed"), "{err}");
        }
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

    fn caller() -> crate::core::ResolvedModelSelection {
        route(Some("claude-sonnet-5"), Some("low"), Some("anthropic"))
    }

    fn route(
        model: Option<&str>,
        effort: Option<&str>,
        provider: Option<&str>,
    ) -> crate::core::ResolvedModelSelection {
        crate::core::ResolvedModelSelection {
            model: model.map(str::to_string),
            reasoning_effort: effort.map(str::to_string),
            provider: provider.map(str::to_string),
        }
    }

    fn overrides(args: serde_json::Value) -> crate::core::ResolvedModelSelection {
        sub_thread_route_overrides(&args, &caller()).expect("valid pins")
    }

    #[test]
    fn run_thread_without_pins_inherits_the_whole_caller_route() {
        assert_eq!(overrides(json!({"prompt": "summarize this"})), caller());
    }

    #[test]
    fn run_thread_model_pin_drops_the_inherited_effort_and_provider() {
        assert_eq!(
            overrides(json!({"prompt": "p", "model": "claude-opus-5"})),
            route(Some("claude-opus-5"), None, None)
        );
    }

    #[test]
    fn run_thread_pinning_the_callers_own_model_keeps_its_route() {
        assert_eq!(
            overrides(json!({"prompt": "p", "model": "claude-sonnet-5"})),
            caller()
        );
    }

    #[test]
    fn run_thread_effort_pin_wins_over_the_inherited_effort() {
        assert_eq!(
            overrides(json!({"prompt": "p", "reasoning_effort": "high"})),
            route(Some("claude-sonnet-5"), Some("high"), Some("anthropic"))
        );
    }

    #[test]
    fn run_thread_takes_both_pins_together() {
        assert_eq!(
            overrides(json!({
                "prompt": "p",
                "model": "claude-opus-5",
                "reasoning_effort": "none",
            })),
            route(Some("claude-opus-5"), Some("none"), None)
        );
    }

    #[test]
    fn run_thread_rejects_an_unknown_reasoning_effort_and_names_it() {
        let err = sub_thread_route_overrides(
            &json!({"prompt": "p", "reasoning_effort": "cheap"}),
            &caller(),
        )
        .unwrap_err();
        assert!(err.contains("cheap"), "error names the bad value: {err}");
        assert!(
            err.contains("none, low, medium, high, xhigh, max"),
            "error lists the tiers: {err}"
        );
    }

    #[test]
    fn run_thread_passes_an_unknown_model_id_through_unchecked() {
        assert_eq!(
            overrides(json!({"prompt": "p", "model": "claude-imaginary-9"})).model,
            Some("claude-imaginary-9".to_string())
        );
    }

    #[test]
    fn run_thread_reads_a_blank_or_null_pin_as_inheriting() {
        for args in [
            json!({"prompt": "p", "model": "   "}),
            json!({"prompt": "p", "model": null, "reasoning_effort": null}),
            json!({"prompt": "p", "reasoning_effort": "  "}),
        ] {
            assert_eq!(overrides(args.clone()), caller(), "{args}");
        }
    }

    #[test]
    fn run_thread_rejects_a_non_string_pin() {
        for args in [
            json!({"prompt": "p", "model": 5}),
            json!({"prompt": "p", "reasoning_effort": ["low"]}),
        ] {
            let err = sub_thread_route_overrides(&args, &caller())
                .expect_err("a non-string pin must be refused");
            assert!(
                err.contains("must be a string"),
                "error states the contract: {err}"
            );
        }
    }

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
            Relation::Child(WakeMode::Wake).spawn_linkage(spawning_thread, Some(tool_call)),
            (Some(spawning_thread), Some(tool_call))
        );
    }

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

    #[test]
    fn spawn_origin_mode_matches_the_agent_emit_sites() {
        let origin = spawn_origin(Uuid::new_v4(), None).expect("always Some");
        assert_eq!(origin.mode(), ActorMode::Agent);
    }

    #[test]
    fn child_spawn_origin_equals_what_legacy_synthesis_produced() {
        use crate::engine::thread_events::ThreadEvent;
        let spawning_thread = Uuid::new_v4();
        let tool_call = Uuid::new_v4();
        let workspace = std::path::Path::new("/tmp/lucidos-test-ws");
        let (parent_thread_id, spawning_event_id) =
            Relation::Child(WakeMode::Wake).spawn_linkage(spawning_thread, Some(tool_call));

        let synthesized = crate::engine::chat::make_message_received(
            workspace,
            "do the thing",
            None,
            None,
            parent_thread_id,
            spawning_event_id,
            None,
            ActorMode::Agent,
            None,
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
            parent_thread_id,
            spawning_event_id,
            None,
            ActorMode::Agent,
            None,
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

    #[test]
    fn run_thread_text_states_the_callback_contract_and_the_workspace_link() {
        let thread = Uuid::new_v4();
        let link = crate::core::store::thread_link("dev", thread);

        let child = Relation::Child(WakeMode::Wake).run_thread_success_text(thread, "dev");
        assert!(
            child.contains(child_report_contract(WakeMode::Wake)),
            "a child spawn must state its wake contract: {child}"
        );
        assert!(child.contains("workspace 'dev'"), "{child}");
        assert!(child.contains(&link), "{child}");

        let top = Relation::Top.run_thread_success_text(thread, "dev");
        assert!(
            top.contains("will NOT report back"),
            "a top spawn must say it never calls back: {top}"
        );
        assert!(top.contains("workspace 'dev'"), "{top}");
        assert!(top.contains(&link), "{top}");
    }

    async fn run_coding_agent_result(
        engine: &std::sync::Arc<LucidosEngine>,
        args: serde_json::Value,
    ) -> String {
        engine
            .handle_special_tool(
                tn::RUN_CODING_AGENT,
                &args,
                Uuid::new_v4(),
                None,
                None,
                None,
                "tu-1",
                &EventMeta::NONE,
                &tokio_util::sync::CancellationToken::new(),
                &caller(),
            )
            .await
            .expect("run_coding_agent is handled by this dispatcher")
    }

    async fn point_agents_at(engine: &std::sync::Arc<LucidosEngine>, claude: &str, codex: &str) {
        use crate::core::{prefs, PreferenceStore};
        for (key, value) in [
            (prefs::CODING_AGENT_CLAUDE_PATH.key(), claude),
            (prefs::CODING_AGENT_CODEX_PATH.key(), codex),
        ] {
            PreferenceStore::set(engine.pool(), &engine.event_bus, key, value, None)
                .await
                .unwrap();
        }
    }

    fn fake_agent_binary(dir: &std::path::Path) -> String {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join("claude");
        std::fs::write(&path, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path.to_string_lossy().into_owned()
    }

    #[test]
    fn run_coding_agent_refuses_the_arguments_it_cannot_honour() {
        crate::test_support::run_on_engine_runtime(
            "engine::agentic_loop::special_tool::tests::run_coding_agent_refuses_the_arguments_it_cannot_honour",
            refuses_unhonoured_args_case,
        );
    }

    async fn refuses_unhonoured_args_case() {
        let (_pool, db) = crate::test_support::setup_test_db().await;
        let (engine, _workspace) = crate::test_support::boot_mock_engine(&db).await;

        let allowlist = run_coding_agent_result(
            &engine,
            json!({ "prompt": "do it", "allowed_tools": ["Bash"] }),
        )
        .await;
        assert!(
            allowlist.starts_with("Error:")
                && allowlist.contains("`allowed_tools`")
                && allowlist.contains("Settings"),
            "{allowlist}"
        );

        let system_prompt = run_coding_agent_result(
            &engine,
            json!({ "prompt": "do it", "append_system_prompt": "be brief" }),
        )
        .await;
        assert!(
            system_prompt.starts_with("Error:") && system_prompt.contains("`append_system_prompt`"),
            "{system_prompt}"
        );
    }

    #[test]
    fn run_coding_agent_refuses_a_straggler_repo_argument_before_spawning() {
        crate::test_support::run_on_engine_runtime(
            "engine::agentic_loop::special_tool::tests::run_coding_agent_refuses_a_straggler_repo_argument_before_spawning",
            straggler_repo_case,
        );
    }

    async fn straggler_repo_case() {
        let (_pool, db) = crate::test_support::setup_test_db().await;
        let (engine, _workspace) = crate::test_support::boot_mock_engine(&db).await;
        let bin_dir = tempfile::tempdir().unwrap();
        point_agents_at(
            &engine,
            &fake_agent_binary(bin_dir.path()),
            "/nonexistent/codex",
        )
        .await;

        let result =
            run_coding_agent_result(&engine, json!({ "prompt": "do it", "repo": "Lucidos" })).await;

        assert_eq!(
            result,
            "Error: `repo` was renamed to `folder`. Pass `folder` instead."
        );
    }

    #[test]
    fn a_cross_workspace_spawn_does_not_need_a_local_agent_install() {
        crate::test_support::run_on_engine_runtime(
            "engine::agentic_loop::special_tool::tests::a_cross_workspace_spawn_does_not_need_a_local_agent_install",
            cross_workspace_without_install_case,
        );
    }

    async fn cross_workspace_without_install_case() {
        let (_pool, db) = crate::test_support::setup_test_db().await;
        let (engine, _workspace) = crate::test_support::boot_mock_engine(&db).await;
        point_agents_at(&engine, "/nonexistent/claude", "/nonexistent/codex").await;

        let result = run_coding_agent_result(
            &engine,
            json!({ "prompt": "do it", "workspace": "no-such-workspace-xyz", "relation": "top" }),
        )
        .await;

        assert!(
            result.contains("cannot resolve workspace"),
            "the target workspace's own engine judges installation, so this must reach it: {result}"
        );
        let local = run_coding_agent_result(&engine, json!({ "prompt": "do it" })).await;
        assert!(
            local.contains("No coding agent is installed"),
            "a same-workspace spawn must be refused here: {local}"
        );
    }

    #[test]
    fn a_model_pin_is_checked_locally_but_not_before_the_cross_workspace_branch() {
        crate::test_support::run_on_engine_runtime(
            "engine::agentic_loop::special_tool::tests::a_model_pin_is_checked_locally_but_not_before_the_cross_workspace_branch",
            model_pin_order_case,
        );
    }

    async fn model_pin_order_case() {
        let (_pool, db) = crate::test_support::setup_test_db().await;
        let (engine, _workspace) = crate::test_support::boot_mock_engine(&db).await;
        let bin_dir = tempfile::tempdir().unwrap();
        point_agents_at(
            &engine,
            &fake_agent_binary(bin_dir.path()),
            "/nonexistent/codex",
        )
        .await;
        let bad_model = "no-such-model-id";
        let local_error = crate::runtime::validate_coding_agent_model(
            crate::runtime::CodingAgent::ClaudeCode,
            Some(bad_model),
        )
        .expect_err("this engine's Claude Code list does not offer the id");

        let local =
            run_coding_agent_result(&engine, json!({ "prompt": "do it", "model": bad_model }))
                .await;
        assert_eq!(local, format!("Error: {local_error}"));

        let cross = run_coding_agent_result(
            &engine,
            json!({
                "prompt": "do it",
                "model": bad_model,
                "workspace": "no-such-workspace-xyz",
                "relation": "top",
            }),
        )
        .await;
        assert!(
            cross.contains("cannot resolve workspace"),
            "this engine's list is not the target's, so the pin must go through: {cross}"
        );
    }
}
