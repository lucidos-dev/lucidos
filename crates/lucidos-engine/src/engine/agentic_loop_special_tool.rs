//! Engine-side tool dispatch: `handle_special_tool` covers tools that need
//! more than `execute_tool()` can give them (capture/refresh app UI,
//! run_thread, resume_thread, run_coding_agent, …).

use uuid::Uuid;

use crate::engine::http::workspace_client::cross_workspace_http_client;
use crate::engine::thread_events::{ActorMode, EventChannel, EventMeta, WakeMode};
use crate::engine::LucidosEngine;
use crate::llm::tool_names as tn;
use crate::triggers::{normalize_route_setting, validate_trigger_reasoning_effort};

/// How a thread spawned by `run_thread` / `run_coding_agent` relates to the
/// thread that issued the spawn. Drives both the spawn linkage (parent /
/// spawning event ids) and the result-text branch.
///
/// `Child` (default) carries the child's resolved *wake mode*, and keeps the
/// callback: the spawning thread's `notify_parent_if_child` fires when the spawned thread reaches
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
    Child(WakeMode),
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

/// Parse the optional `relation` and `wake_mode` arguments from a spawn tool
/// call. The relation defaults to [`Relation::Child`] when absent, and accepts
/// `"child"` (canonical) and `"sub"` (back-compat alias from the pre-glossary
/// wire name). A child's wake mode is the declared one, else the default for a
/// parent that is or is not the home thread. Unknown values, non-string types
/// and a wake mode on a top spawn are errors. So the LLM sees the contract
/// violation instead of silently getting a default.
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

/// The route a `run_thread` child asks for, before the account fills its gaps.
///
/// The child inherits the calling turn's model, effort and provider, and an
/// explicit pin wins per field. A pinned model other than the caller's drops
/// the inherited effort and provider, which belong to the caller's model.
///
/// Validation follows the trigger route pins, the other place a chat model and
/// effort are pinned per spawn. The model id passes through unchecked
/// (`normalize_route_setting`). A registry row can be disabled long after the
/// caller read it, and a bad id surfaces as an ordinary thread failure. The
/// tier vocabulary is ours, so a typo there is rejected by name
/// (`validate_trigger_reasoning_effort`), never swapped for the default.
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

/// Longest argument dump an MCP permission card carries. Past it the summary is
/// clamped, so neither the card nor the persisted payload is unbounded.
const MCP_ARGS_SUMMARY_LIMIT: usize = 500;

/// The argument text an `McpPermissionRequested` card shows, and the same text
/// the event payload keeps and SSE broadcasts.
///
/// Redacted before it is written down. A connection string reaches an MCP tool
/// as an ordinary argument, and the event row is immutable. Every sibling
/// permission lane scrubs the same way: see `command_permission`'s
/// `redact_postgres_secrets` and `cc_permission`'s JSON walk.
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

/// What a child's report does to this conversation under its wake mode, as
/// the spawn result states it. The parent reads this to know whether to wait.
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
            "Its wake_mode is \"no-wake\": a clean finish does NOT resume this \
             conversation, since the user reads the result in that thread. Its report \
             is filed for your next turn, and a failure still resumes this conversation."
        }
    }
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
            Self::Child(_) => (Some(spawning_thread_id), tool_called_event_id),
            Self::Top => (None, None),
        }
    }

    /// The child's wake mode, which rides beside the linkage. `None` for a
    /// top spawn, which reports to nobody.
    pub(crate) fn wake_mode(self) -> Option<WakeMode> {
        match self {
            Self::Child(mode) => Some(mode),
            Self::Top => None,
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
        // Per-turn event meta, cloned onto any emitted event. Its channel is
        // `Some(Trigger)` for a scheduled trigger and `None` for chat. That
        // picks the MCP gate's trigger rule over its card. Its actor goes on
        // the permission card.
        meta: &EventMeta,
        // Cancel token for the response — lets a blocking MCP permission card
        // resolve as canceled when the user clicks Stop, instead of dangling.
        cancel_token: &tokio_util::sync::CancellationToken,
        // The model, effort and provider this turn runs on. A `run_thread`
        // child inherits it.
        caller_route: &crate::core::ResolvedModelSelection,
    ) -> Option<String> {
        if tool_name == tn::ASK_USER_QUESTION {
            return Some(
                self.handle_chat_ask_user_question(thread_id, tool_use_id, tool_args, meta)
                    .await,
            );
        }
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
                // The model and effort pins, checked against the backend that
                // will actually run them. A bad id fails the spawn HERE, in the
                // same turn. The caller then learns its pin did not apply, and
                // cannot report a model choice the session never made. Below the
                // cross-workspace branch: this engine's list is not the target's.
                let spawn_model = match crate::runtime::validate_coding_agent_model(
                    coding_agent,
                    tool_args.get("model").and_then(|v| v.as_str()),
                ) {
                    Ok(m) => m,
                    Err(e) => return Some(format!("Error: {}", e)),
                };
                // Validated model, not the raw arg: a tier Codex restricts is
                // checked against the id the spawn will actually carry.
                let spawn_effort = match crate::runtime::validate_coding_agent_effort(
                    coding_agent,
                    spawn_model.as_deref(),
                    tool_args.get("reasoning_effort").and_then(|v| v.as_str()),
                ) {
                    Ok(e) => e,
                    Err(e) => return Some(format!("Error: {}", e)),
                };
                // The Lucidos Agent is an agent: its pick must be a rung. A
                // cross-workspace spawn is checked by the target's chat route.
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

                // `folder` is the canonical parameter; `repo` was a deprecated
                // alias, now removed (see `removed_repo_alias_error`).
                if let Some(err) = removed_repo_alias_error(tool_args) {
                    return Some(err.to_string());
                }
                let folder_input = tool_args
                    .get("folder")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty());
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
                    wake_mode: relation.wake_mode(),
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
                let parent_is_home =
                    crate::engine::home_thread::spawner_is_home(&self.pool, thread_id).await;
                let relation = match parse_relation(tool_args, parent_is_home) {
                    Ok(r) => r,
                    Err(e) => return Some(format!("Error: {}", e)),
                };
                // The Lucidos Agent is an agent: its chat pick must be a rung.
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
                            // Resolved and clamped now, so the child's eager
                            // MessageReceived stamps the route it runs on.
                            let provider = self.current_provider();
                            let configured = provider.configured_providers();
                            let route = crate::engine::chat::process::resolve_route_overrides(
                                &self.pool,
                                crate::core::ModelDefaults {
                                    registry: self.model_registry(),
                                    fallback_model: provider.default_model(),
                                },
                                |kind| configured.as_ref().is_none_or(|set| set.contains(&kind)),
                                None,
                                None,
                                None,
                                overrides,
                            )
                            .await;
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
                                    wake_mode: relation.wake_mode(),
                                    title: caller_title.map(str::to_string),
                                    model: route.model,
                                    reasoning_effort: route.reasoning_effort,
                                    provider: route.provider,
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

            // A thread may call only the servers it enabled. Asked before the
            // gate, so no card asks the user to allow a call that cannot run.
            if let Some(refusal) = self
                .mcp_manager
                .not_enabled_refusal(thread_id, &server_id, tool_name)
                .await
            {
                return Some(format!("Error: {refusal}"));
            }

            // Pre-authorization gate (ADR 0430). The server's `auto_approve`
            // flag proceeds anywhere. A trigger thread proceeds only on a
            // server its trigger lists, and is refused otherwise. A chat
            // thread asks the user (in-thread `McpPermissionRequested` card),
            // which also honors a prior session / `mcp-allowed-tools` grant.
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
        // `repo` was a deprecated alias for `folder`, now removed (see
        // `removed_repo_alias_error`).
        if let Some(err) = removed_repo_alias_error(tool_args) {
            return err.to_string();
        }
        let folder = tool_args
            .get("folder")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty());
        // Validate the pins HERE too: this path returns before the local
        // spawn's validation, and the receiving engine passes them straight to
        // the agent. Codex's list is compiled into both engines, so the sender
        // can check it. Claude Code's is discovered per workspace, so this
        // engine's list says nothing about the target's: the id goes through,
        // and the target's Claude Code accepts it or fails the child loudly.
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
        // For Claude Code only the shared tier vocabulary applies: a model's
        // own tiers come from the target's list, which this engine cannot see.
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
    /// The card's `message` renders in the turn `meta` belongs to.
    async fn handle_chat_ask_user_question(
        &self,
        thread_id: Uuid,
        tool_use_id: &str,
        tool_args: &serde_json::Value,
        meta: &EventMeta,
    ) -> String {
        let questions = tool_args
            .get("questions")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        if !questions.is_array() {
            return "Error: `questions` must be an array of question objects".to_string();
        }

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
                    owner_approval: None,
                },
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

        use crate::engine::agent_question::{build_hook_answers, with_answer_images, AnswerImages};
        let answers = build_hook_answers(&answer_kinds, &questions, AnswerImages::AttachedBlocks);
        let result = serde_json::to_string(&answers).unwrap_or_else(|_| answers.to_string());
        with_answer_images(result, &answer_kinds)
    }
}

/// `repo` was a deprecated alias for `folder` on `run_coding_agent`, now
/// removed. A straggler still passing `repo` must fail loud with a rename
/// error. A bare drop would read a `repo`-only call as "folder omitted", which
/// on a source checkout silently edits Lucidos itself. Permanent guard, not a
/// temporary measure. Returns `Some(error)` when a non-empty `repo` is present.
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

    /// `repo` was removed as an alias for `folder`. A straggler still passing
    /// it must get a loud rename error, never a silent misroute.
    #[test]
    fn a_removed_repo_alias_is_rejected_with_a_rename_error() {
        assert_eq!(
            removed_repo_alias_error(&json!({ "repo": "Lucidos" })),
            Some("Error: `repo` was renamed to `folder`. Pass `folder` instead.")
        );
        // Absent, empty, or whitespace-only `repo` is not the alias: no error.
        assert!(removed_repo_alias_error(&json!({ "folder": "data/apps/x" })).is_none());
        assert!(removed_repo_alias_error(&json!({ "repo": "" })).is_none());
        assert!(removed_repo_alias_error(&json!({ "repo": "  " })).is_none());
    }

    /// The summary is persisted into the event payload and broadcast over SSE.
    /// A connection string in a tool argument was written down in the clear.
    /// Every sibling permission lane scrubs first.
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

    /// The clamp keeps the card and the payload bounded, and must land on a
    /// character boundary rather than mid-codepoint.
    #[test]
    fn mcp_arguments_summary_clamps_on_a_char_boundary() {
        let args = json!({ "note": "æ".repeat(2000) });
        let summary = mcp_arguments_summary(&args);
        assert!(summary.len() <= 503, "{}", summary.len());
        assert!(summary.ends_with("..."));
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

    // --- run_thread route inheritance ---------------------------------------

    /// The calling turn's route: a trigger pinned to Sonnet at `low`, on
    /// Anthropic.
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

    /// The incident: an unpinned child ran on the account model, not on the
    /// route its pinned parent paid for.
    #[test]
    fn run_thread_without_pins_inherits_the_whole_caller_route() {
        assert_eq!(overrides(json!({"prompt": "summarize this"})), caller());
    }

    /// The caller's effort and provider belong to the caller's model, so a
    /// different model resolves its own.
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

    /// The tier vocabulary is ours, so a typo is a caller error. Rejecting it
    /// by name is what stops the spawn quietly running at `xhigh` while the
    /// caller believes it asked for something cheaper.
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

    /// The model id is NOT checked against the registry, matching the trigger
    /// route pins: a row can be disabled long after the caller read it, and a
    /// bad id surfaces as an ordinary thread failure instead.
    #[test]
    fn run_thread_passes_an_unknown_model_id_through_unchecked() {
        assert_eq!(
            overrides(json!({"prompt": "p", "model": "claude-imaginary-9"})).model,
            Some("claude-imaginary-9".to_string())
        );
    }

    /// Blank and null both read as "no pin", the same normalization the trigger
    /// form's Default option travels through. `Some("")` must never reach the
    /// provider as a model id.
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

    /// A wrong-typed pin is refused rather than read as absent: silently
    /// inheriting would tell the caller its pin applied.
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
            Relation::Child(WakeMode::Wake).spawn_linkage(spawning_thread, Some(tool_call)),
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
            Relation::Child(WakeMode::Wake).spawn_linkage(spawning_thread, Some(tool_call));

        // `explicit_origin: None` takes the legacy-synthesis branch.
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
}
