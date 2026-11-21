use super::super::LucidosEngine;
use crate::engine::event_bus::SystemEvent;
use crate::engine::trigger_writes::{RequestedTriggerGrants, TriggerWrite};
use crate::llm::tool_names as tn;
use crate::scheduler::cron_rules::cron_tz_or_utc;
use crate::scheduler::cron_rules::{parse_cron_arg, ValidatedCron};
use crate::triggers::{
    normalize_mcp_servers, normalize_route_setting, resolve_trigger_provider_update,
    validate_trigger_provider, validate_trigger_reasoning_effort, ActorField, ContinueKeySpec,
    EventSubscription, TriggerRun, ACTOR_FIELD_KEY,
};
// Lets the never-fires diagnosis and the AND-footgun check read a parsed
// schedule's day-of-month / month / day-of-week ordinal sets, instead of
// re-parsing the cron field strings by hand.

/// Both write tools here arm a trigger's `on:` list, never a thread's wait.
const TRIGGER_SURFACE: crate::core::event_subscription::SubscriptionSurface =
    crate::core::event_subscription::SubscriptionSurface::Trigger;

/// The two accepted shapes of a trigger's `run` field, for a parse failure to
/// quote back. One const, because the two copies drifted: update's named the
/// retired `text` alias where create's named `intent`.
const RUN_FIELD_SHAPES: &str =
    "Expected { type: 'intent', intent: '...' } or { type: 'script', path: '...' }";

/// Hard guard: scheduling tools (`create_trigger`, `update_trigger`,
/// `delete_trigger`, `pause_trigger`, `resume_trigger`) called from inside a
/// scheduled trigger's LLM execution are usually a bug — the LLM mistook the
/// fire for a user request and is trying to (re-)schedule the same intent.
/// Reject those calls so a single confused turn cannot spawn an infinite
/// trigger-creation loop.
///
/// Returns `Some(error)` if the call must be rejected, `None` if it may
/// proceed. Self-action (delete/pause/resume on the firing trigger's own id)
/// is the one legitimate pattern: the trigger's intent text may say "stop
/// after this", and the LLM's only correct response is to act on its own id.
///
/// Pure: callers pass in `active_trigger_id` (read from `ACTIVE_TRIGGER_ID`
/// at the dispatch site). `None` means "not in a trigger fire" → always allow.
///
/// **`run_trigger` is deliberately NOT gated here.** Its guard is stricter (it
/// refuses even self-id, because self-run recurses where self-pause terminates)
/// AND it has to hold on `POST /api/v1/triggers/run` too, which never reaches
/// this function. Both would drift if stated twice, so the single owner is
/// `engine_impl::trigger_runs::check_off_schedule_run`, which every surface
/// funnels through. Note the runaway is bounded regardless: a second cron fire
/// while one is active COALESCES away (`policy.rs`), so a script trigger that
/// calls `lucidos triggers run` on itself over HTTP is reported as already
/// running rather than stacking.
pub(crate) fn check_scheduling_tool_in_trigger(
    tool_name: &str,
    target_trigger_id: Option<&str>,
    active_trigger_id: Option<&str>,
    active_trigger_name: Option<&str>,
) -> Option<String> {
    let active_id = active_trigger_id?;

    // Early-return for tool names we don't gate, so we don't allocate format
    // strings on every read-only call (e.g. `list_triggers`).
    match tool_name {
        tn::CREATE_TRIGGER
        | tn::UPDATE_TRIGGER
        | tn::DELETE_TRIGGER
        | tn::PAUSE_TRIGGER
        | tn::RESUME_TRIGGER => {}
        _ => return None,
    }

    let display_name = active_trigger_name.unwrap_or("(unknown)");
    let preamble = format!(
        "Error: Scheduling tools are disabled during scheduled trigger fires. \
         You are currently executing trigger '{}' (id: {}). \
         Execute the trigger's steps directly instead.",
        display_name, active_id
    );

    match tool_name {
        tn::CREATE_TRIGGER | tn::UPDATE_TRIGGER => Some(format!(
            "{} If you need to stop this trigger from re-firing, call delete_trigger or pause_trigger on id={}.",
            preamble, active_id
        )),
        tn::DELETE_TRIGGER | tn::PAUSE_TRIGGER | tn::RESUME_TRIGGER => match target_trigger_id {
            Some(t) if t == active_id => None,
            _ => Some(format!(
                "{} Only {} on id={} (this trigger itself) is permitted from inside a fire.",
                preamble, tool_name, active_id
            )),
        },
        _ => None, // unreachable after the early-return above
    }
}

/// The model, reasoning effort and provider pins an `update_trigger` call sets.
/// Each is `None` when the call leaves it alone and `Some(None)` when it goes
/// back to the account default.
#[derive(Debug, PartialEq)]
struct RoutePinUpdate {
    model: Option<Option<String>>,
    reasoning_effort: Option<Option<String>>,
    provider: Option<Option<String>>,
}

/// Read the three pins from `update_trigger` args. A provider pin is also
/// cleared when the model moves without one: a pin belongs to its model.
fn route_pin_update(
    registry: &crate::llm::ModelRegistry,
    existing: &crate::triggers::TriggerConfig,
    args: &serde_json::Value,
) -> Result<RoutePinUpdate, String> {
    let model = args
        .get("model")
        .map(|v| normalize_route_setting(v.as_str()));
    let reasoning_effort = args
        .get("reasoning_effort")
        .map(|v| validate_trigger_reasoning_effort(v.as_str()))
        .transpose()?;
    let provider = resolve_trigger_provider_update(
        registry,
        existing,
        model.as_ref().map(Option::as_deref),
        args.get("provider").map(|v| v.as_str()),
    )?;
    Ok(RoutePinUpdate {
        model,
        reasoning_effort,
        provider,
    })
}

fn trigger_grant_status(asked: bool) -> &'static str {
    if asked {
        "\n\nTrigger grants await the owner's answer in this thread."
    } else {
        ""
    }
}

#[easy_ext::ext(SchedulerExt)]
pub(crate) impl LucidosEngine {
    async fn execute_scheduler_tool(
        &self,
        name: &str,
        args: &serde_json::Value,
        thread_id: uuid::Uuid,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let actor = Some(super::agent_tool_actor(thread_id));
        // Defense in depth: the prompt envelope asks the LLM not to call these
        // during a trigger fire; this enforces it even when the LLM ignores it.
        let active_id = crate::scheduler::user_tasks::current_trigger_id();
        let active_name = active_id.as_deref().and_then(|id| {
            self.trigger_configs
                .read()
                .unwrap()
                .get(id)
                .map(|c| c.name.clone())
        });
        let target_id = args.get("trigger_id").and_then(|v| v.as_str());
        if let Some(err) = check_scheduling_tool_in_trigger(
            name,
            target_id,
            active_id.as_deref(),
            active_name.as_deref(),
        ) {
            return Ok(err);
        }

        match name {
            "create_trigger" => {
                let name = args["name"].as_str().unwrap_or("");
                let subscriptions = match parse_on_arg(args.get("on")) {
                    Ok(subs) => subs,
                    Err(e) => return Ok(e),
                };
                // Every entry in the `on` array. A trigger armed on a name the
                // engine never emits looks armed and never fires.
                let event_warnings = match crate::core::event_subscription::check_subscriptions(
                    &self.pool,
                    &subscriptions,
                    TRIGGER_SURFACE,
                )
                .await
                {
                    Ok(warnings) => warnings,
                    Err(msg) => return Ok(format!("Error: {msg}")),
                };

                // Timezone first: the never-fires guard reports its next-run
                // preview in it, and a trigger without one is refused anyway, so
                // there is nothing to validate cron against until it is known.
                let tz_val = self.user_timezone.read().await.clone();
                if tz_val.is_empty() {
                    return Ok("Error: User timezone is not set. Call set_preference(key=\"timezone\", value=\"…\") first to set the user's timezone before creating triggers.".to_string());
                }
                let tz = cron_tz_or_utc(&tz_val, "create_trigger");

                // Parse cron: accepts a single string or an array of strings (optional if on is set)
                let cron = if args.get("cron").is_some() && !args["cron"].is_null() {
                    match parse_cron_arg(&args["cron"], tz) {
                        Ok(v) => v,
                        Err(e) => return Ok(e),
                    }
                } else {
                    ValidatedCron::default()
                };
                let cron_expressions = &cron.expressions;

                if name.is_empty() {
                    return Ok("Error: name is required".to_string());
                }
                if cron_expressions.is_empty() && subscriptions.is_empty() {
                    return Ok(
                        "Error: At least one of 'cron' or 'on' (event subscriptions) is required"
                            .to_string(),
                    );
                }

                // Parse run field: { type: "intent", intent: "..." } or { type: "script", path: "..." }
                let run: TriggerRun = match args.get("run") {
                    Some(run_val) if !run_val.is_null() => parse_run_arg(run_val)?,
                    _ => {
                        // Backward compat: accept prompt_text as a shorthand
                        let prompt_text = args
                            .get("prompt_text")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        if prompt_text.is_empty() {
                            return Ok("Error: 'run' is required. Use { type: 'intent', intent: '...' } or { type: 'script', path: '...' }".to_string());
                        }
                        TriggerRun::Intent {
                            intent: prompt_text.to_string(),
                        }
                    }
                };

                // Emit via EventBus — persists to events table AND notifies scheduler instantly
                let trigger_id_str = uuid::Uuid::new_v4().to_string();
                let cron_display = cron_expressions.join(", ");
                // No slug here: the writer mints one under the lock that
                // serializes the emit. `from_created_payload`'s derive is a pure
                // function of the name, so legacy events replay to the same
                // slug. Two triggers named alike would share one directory.
                let mut event_payload = serde_json::json!({
                    "trigger_id": trigger_id_str,
                    "name": name,
                    "schedule": cron_expressions,
                    "timezone": tz_val,
                    "run": serde_json::to_value(&run).unwrap(),
                });
                if !subscriptions.is_empty() {
                    event_payload["on"] = serde_json::to_value(&subscriptions)
                        .expect("EventSubscription serialization is infallible");
                }
                // Owning app dir (e.g. "trigger-workflow"); stamped onto notifications
                // emitted from this trigger so the popover can deep-link to the app.
                if let Some(aid) = args
                    .get("app_id")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                {
                    event_payload["app_id"] = serde_json::json!(aid);
                }
                if args
                    .get("go_to_review")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
                {
                    event_payload["go_to_review"] = serde_json::json!(true);
                }
                // Optional trigger group membership. Validate against the
                // in-memory registry so dangling references can't slip in via
                // the LLM tool surface — the HTTP API rejects the same case.
                if let Some(gid) = args
                    .get("group_id")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                {
                    let known = self.trigger_groups.read().unwrap().contains_key(gid);
                    if !known {
                        return Ok(format!(
                            "Error: Unknown group_id '{}'. Use list_trigger_groups to find an existing id or create_trigger_group first.",
                            gid
                        ));
                    }
                    event_payload["group_id"] = serde_json::json!(gid);
                }
                // Per-trigger model / reasoning effort. Absent (the common case)
                // leaves the trigger on the account chat defaults, so only an
                // explicit pick is stamped.
                if let Some(model) =
                    normalize_route_setting(args.get("model").and_then(|v| v.as_str()))
                {
                    event_payload["model"] = serde_json::json!(model);
                }
                match validate_trigger_reasoning_effort(
                    args.get("reasoning_effort").and_then(|v| v.as_str()),
                ) {
                    Ok(Some(effort)) => {
                        event_payload["reasoning_effort"] = serde_json::json!(effort)
                    }
                    Ok(None) => {}
                    Err(e) => return Ok(format!("Error: {}", e)),
                }
                match validate_trigger_provider(
                    &self.model_registry,
                    args.get("model").and_then(|v| v.as_str()),
                    args.get("provider").and_then(|v| v.as_str()),
                ) {
                    Ok(Some(provider)) => event_payload["provider"] = serde_json::json!(provider),
                    Ok(None) => {}
                    Err(e) => return Ok(format!("Error: {}", e)),
                }
                // An agent's chat pick must be a rung (ADR 0525).
                if let Err(e) = crate::runtime::intelligence_ladder::check_agent_pick(
                    &self.pool,
                    &self.model_routes(),
                    crate::runtime::intelligence_ladder::LadderSurface::Chat,
                    event_payload["model"].as_str(),
                    event_payload["reasoning_effort"].as_str(),
                )
                .await
                {
                    return Ok(format!("Error: {e}"));
                }
                let mcp_servers = match mcp_servers_arg(args.get("mcp_servers")) {
                    Ok(ids) => ids.unwrap_or_default(),
                    Err(e) => return Ok(e),
                };
                if !mcp_servers.is_empty() {
                    event_payload["mcp_servers"] = serde_json::json!(mcp_servers);
                }
                match ContinueKeySpec::parse(args.get("continue_key"))
                    .and_then(|spec| ContinueKeySpec::check_run(spec.as_ref(), &run).map(|()| spec))
                {
                    Ok(Some(spec)) => event_payload["continue_key"] = serde_json::json!(spec),
                    Ok(None) => {}
                    Err(e) => return Ok(format!("Error: {e}")),
                }
                match ActorField::parse(args.get("actor"))
                    .and_then(|field| ActorField::check_run(field.as_ref(), &run).map(|()| field))
                {
                    Ok(Some(field)) => event_payload[ACTOR_FIELD_KEY] = serde_json::json!(field),
                    Ok(None) => {}
                    Err(e) => return Ok(format!("Error: {e}")),
                }

                let requested: RequestedTriggerGrants = serde_json::from_value(args.clone())?;
                let asked = self
                    .trigger_registry_writer()
                    .write_with_requested_grants(
                        TriggerWrite::Created,
                        &trigger_id_str,
                        event_payload,
                        actor,
                        requested,
                        Some(thread_id),
                    )
                    .await?;
                let grant_status = trigger_grant_status(asked);

                let trigger_desc = trigger_description(&cron_display, &subscriptions);
                let run_desc = match &run {
                    TriggerRun::Intent { intent } => {
                        let end = intent.floor_char_boundary(50);
                        format!("intent '{}'", &intent[..end])
                    }
                    TriggerRun::Script { path } => format!("script '{}'", path),
                };

                Ok(format!(
                    "[ACTION COMPLETED] Created trigger '{}' (ID: {}) running {} with {} in timezone {}.{}{}{grant_status}",
                    name, trigger_id_str, run_desc, trigger_desc, tz_val, cron.advice_suffix(),
                    warning_suffix(&event_warnings)
                ))
            }
            "list_triggers" => {
                let configs: Vec<crate::triggers::TriggerConfig> = {
                    let configs = self.trigger_configs.read().unwrap();
                    configs.values().cloned().collect()
                };

                if configs.is_empty() {
                    Ok("No triggers found.".to_string())
                } else {
                    let mut result = format!("Found {} trigger(s):\n\n", configs.len());
                    for config in &configs {
                        let status = if config.paused { "paused" } else { "active" };
                        let last_run = config
                            .last_run
                            .map(|t| t.format("%Y-%m-%d %H:%M").to_string())
                            .unwrap_or_else(|| "never".to_string());
                        let run_info = match &config.run {
                            TriggerRun::Intent { intent } => {
                                let end = intent.floor_char_boundary(60);
                                format!("intent: {}", &intent[..end])
                            }
                            TriggerRun::Script { path } => format!("script: {}", path),
                        };
                        let trigger_type = config.trigger_type_label();
                        let schedule_display = if !config.schedule.is_empty() {
                            format!(
                                "  Schedule: {} ({})\n",
                                config.schedule.join(", "),
                                config.timezone
                            )
                        } else {
                            String::new()
                        };
                        let event_display = if config.on.is_empty() {
                            String::new()
                        } else {
                            let rendered = config
                                .on
                                .iter()
                                .map(|sub| match &sub.condition {
                                    Some(c) => format!("{} when {}", sub.event_type, c),
                                    None => sub.event_type.clone(),
                                })
                                .collect::<Vec<_>>()
                                .join(", ");
                            format!("  Event: on {}\n", rendered)
                        };
                        let mcp_display = if config.mcp_servers.is_empty() {
                            String::new()
                        } else {
                            format!("  MCP servers: {}\n", config.mcp_servers.join(", "))
                        };
                        let key_display = config
                            .continue_key
                            .as_ref()
                            .map(|spec| format!("  Continue key: {}\n", serde_json::json!(spec)))
                            .unwrap_or_default();
                        let actor_display = config
                            .actor
                            .as_ref()
                            .map(|field| format!("  Actor: {}\n", field.0))
                            .unwrap_or_default();
                        let coding_agents = if config.coding_agent_grant {
                            "allowed"
                        } else {
                            "not allowed (only the owner can grant)"
                        };
                        result.push_str(&format!(
                            "- **{}** (ID: {}) [{}]\n{}{}{}{}{}  Status: {}\n  Coding agents: {}\n  Last run: {}\n  Run: {}\n\n",
                            config.name, config.id, trigger_type, schedule_display, event_display, mcp_display, key_display, actor_display, status, coding_agents, last_run, run_info
                        ));
                    }
                    Ok(result)
                }
            }
            "update_trigger" => {
                let trigger_id = match args["trigger_id"].as_str() {
                    Some(s) if !s.is_empty() => s.to_string(),
                    _ => return Ok("Error: trigger_id is required".to_string()),
                };

                // Read the existing config up front. An unknown id is refused
                // either way, so failing fast costs nothing, and the trigger's own
                // timezone is what the cron guard below validates against (update
                // cannot change it).
                let existing = {
                    let configs = self.trigger_configs.read().unwrap();
                    configs.get(&trigger_id).cloned()
                };
                let existing = match existing {
                    Some(c) => c,
                    None => return Ok(format!("Error: No trigger found with ID {}", trigger_id)),
                };

                let new_name = args.get("name").and_then(|v| v.as_str());
                let new_run: Option<TriggerRun> = match args.get("run").filter(|v| !v.is_null()) {
                    Some(v) => Some(parse_run_arg(v)?),
                    None => None,
                };
                // Subscriptions are sent as a full replacement: None = absent
                // (keep existing), Some(empty) = clear, Some(non-empty) = set.
                // `on: null` collapses to "no subscriptions" so the LLM can
                // clear by passing null without an explicit empty array.
                let new_on: Option<Vec<EventSubscription>> = if args.get("on").is_some() {
                    Some(match parse_on_arg(args.get("on")) {
                        Ok(subs) => subs,
                        Err(e) => return Ok(e),
                    })
                } else {
                    None
                };
                // Only what this call supplies. An `on:` list left absent keeps
                // whatever the trigger already had.
                let event_warnings = match crate::core::event_subscription::check_subscriptions(
                    &self.pool,
                    new_on.as_deref().unwrap_or_default(),
                    TRIGGER_SURFACE,
                )
                .await
                {
                    Ok(warnings) => warnings,
                    Err(msg) => return Ok(format!("Error: {msg}")),
                };
                // Parse cron: Option<ValidatedCron>
                // None = not provided (keep existing), Some(empty) = clear, Some(non-empty) = set
                let new_cron: Option<ValidatedCron> = if args.get("cron").is_some() {
                    if args["cron"].is_null() {
                        Some(ValidatedCron::default())
                    } else {
                        let tz = cron_tz_or_utc(&existing.timezone, "update_trigger");
                        Some(match parse_cron_arg(&args["cron"], tz) {
                            Ok(v) => v,
                            Err(e) => return Ok(e),
                        })
                    }
                } else {
                    None
                };
                let new_paused: Option<bool> = args.get("paused").and_then(|v| v.as_bool());
                // app_id: Some(None) = explicit null = clear; Some(Some(s)) = set; None = absent
                let new_app_id: Option<Option<String>> = if args.get("app_id").is_some() {
                    Some(
                        args["app_id"]
                            .as_str()
                            .map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty()),
                    )
                } else {
                    None
                };
                let new_go_to_review: Option<bool> =
                    args.get("go_to_review").and_then(|v| v.as_bool());
                // group_id: Some(None) = clear, Some(Some(s)) = set, None = absent
                let new_group_id: Option<Option<String>> = if args.get("group_id").is_some() {
                    Some(
                        args["group_id"]
                            .as_str()
                            .map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty()),
                    )
                } else {
                    None
                };
                if let Some(Some(ref gid)) = new_group_id {
                    let known = self.trigger_groups.read().unwrap().contains_key(gid);
                    if !known {
                        return Ok(format!(
                            "Error: Unknown group_id '{}'. Use list_trigger_groups to find an existing id or create_trigger_group first.",
                            gid
                        ));
                    }
                }
                let RoutePinUpdate {
                    model: new_model,
                    reasoning_effort: new_reasoning_effort,
                    provider: new_provider,
                } = match route_pin_update(&self.model_registry, &existing, args) {
                    Ok(update) => update,
                    Err(e) => return Ok(format!("Error: {}", e)),
                };

                // A changed pick must be a rung, judged as the trigger will
                // fire on it (ADR 0525).
                if new_model.is_some() || new_reasoning_effort.is_some() {
                    let model = new_model.clone().unwrap_or_else(|| existing.model.clone());
                    let effort = new_reasoning_effort
                        .clone()
                        .unwrap_or_else(|| existing.reasoning_effort.clone());
                    if let Err(e) = crate::runtime::intelligence_ladder::check_agent_pick(
                        &self.pool,
                        &self.model_routes(),
                        crate::runtime::intelligence_ladder::LadderSurface::Chat,
                        model.as_deref(),
                        effort.as_deref(),
                    )
                    .await
                    {
                        return Ok(format!("Error: {e}"));
                    }
                }
                let new_mcp_servers = match mcp_servers_arg(args.get("mcp_servers")) {
                    Ok(ids) => ids,
                    Err(e) => return Ok(e),
                };
                // Some(None) = clear, Some(Some(spec)) = set, None = absent.
                let new_continue_key: Option<Option<ContinueKeySpec>> =
                    match args.get("continue_key") {
                        Some(raw) => match ContinueKeySpec::parse(Some(raw)) {
                            Ok(spec) => Some(spec),
                            Err(e) => return Ok(format!("Error: {e}")),
                        },
                        None => None,
                    };
                let new_actor: Option<Option<ActorField>> = match args.get("actor") {
                    Some(raw) => match ActorField::parse(Some(raw)) {
                        Ok(field) => Some(field),
                        Err(e) => return Ok(format!("Error: {e}")),
                    },
                    None => None,
                };

                if new_name.is_none()
                    && new_run.is_none()
                    && new_cron.is_none()
                    && new_on.is_none()
                    && new_paused.is_none()
                    && new_app_id.is_none()
                    && new_go_to_review.is_none()
                    && new_group_id.is_none()
                    && new_model.is_none()
                    && new_reasoning_effort.is_none()
                    && new_provider.is_none()
                    && new_mcp_servers.is_none()
                    && new_continue_key.is_none()
                    && new_actor.is_none()
                {
                    return Ok(
                        "Error: At least one field besides trigger_id must be provided".to_string(),
                    );
                }

                // Build update payload with only changed fields
                let mut update_payload = serde_json::json!({
                    "trigger_id": trigger_id,
                });
                let mut updated_fields = Vec::new();

                if let Some(n) = new_name {
                    update_payload["name"] = serde_json::json!(n);
                    updated_fields.push("name");
                }
                if let Some(crons) = &new_cron {
                    update_payload["schedule"] = serde_json::json!(crons.expressions);
                    updated_fields.push("schedule");
                }
                if let Some(ref run) = new_run {
                    update_payload["run"] = serde_json::to_value(run).unwrap();
                    updated_fields.push("run");
                }
                if let Some(ref subs) = new_on {
                    update_payload["on"] = serde_json::to_value(subs)
                        .expect("EventSubscription serialization is infallible");
                    updated_fields.push("on");
                }
                if let Some(paused) = new_paused {
                    update_payload["paused"] = serde_json::json!(paused);
                    updated_fields.push("paused");
                }
                if let Some(ref aid) = new_app_id {
                    update_payload["app_id"] = serde_json::json!(aid);
                    updated_fields.push("app_id");
                }
                if let Some(v) = new_go_to_review {
                    update_payload["go_to_review"] = serde_json::json!(v);
                    updated_fields.push("go_to_review");
                }
                if let Some(ref gid) = new_group_id {
                    update_payload["group_id"] = serde_json::json!(gid);
                    updated_fields.push("group_id");
                }
                if let Some(ref model) = new_model {
                    update_payload["model"] = serde_json::json!(model);
                    updated_fields.push("model");
                }
                if let Some(ref effort) = new_reasoning_effort {
                    update_payload["reasoning_effort"] = serde_json::json!(effort);
                    updated_fields.push("reasoning_effort");
                }
                if let Some(ref provider) = new_provider {
                    update_payload["provider"] = serde_json::json!(provider);
                    updated_fields.push("provider");
                }
                if let Some(ref ids) = new_mcp_servers {
                    update_payload["mcp_servers"] = serde_json::json!(ids);
                    updated_fields.push("mcp_servers");
                }
                if let Some(ref spec) = new_continue_key {
                    update_payload["continue_key"] = serde_json::json!(spec);
                    updated_fields.push("continue_key");
                }
                if let Some(ref field) = new_actor {
                    update_payload[ACTOR_FIELD_KEY] = serde_json::json!(field);
                    updated_fields.push("actor");
                }

                // Ensure trigger still has at least one firing mechanism
                let updated_schedule = new_cron
                    .as_ref()
                    .map(|c| &c.expressions)
                    .unwrap_or(&existing.schedule);
                let updated_on = new_on.as_ref().unwrap_or(&existing.on);
                if updated_schedule.is_empty() && updated_on.is_empty() {
                    return Ok(
                        "Error: Trigger must have at least one cron schedule or event subscription"
                            .to_string(),
                    );
                }
                let mut next = existing.clone();
                next.apply_update(&update_payload);
                if let Err(e) = ContinueKeySpec::check_run(next.continue_key.as_ref(), &next.run)
                    .and_then(|()| ActorField::check_run(next.actor.as_ref(), &next.run))
                {
                    return Ok(format!("Error: {e}"));
                }
                // This tool is always an agent's, never the owner's.
                let grant_notice = existing.unowned_edit_clears_grants(&next).then(|| {
                    crate::triggers::TriggerConfig::clear_grants_in(&mut update_payload);
                    format!("\n\n{}", existing.grants_cleared_notice())
                });

                let requested: RequestedTriggerGrants = serde_json::from_value(args.clone())?;
                let asked = self
                    .trigger_registry_writer()
                    .write_with_requested_grants(
                        TriggerWrite::Updated,
                        &trigger_id,
                        update_payload,
                        actor,
                        requested,
                        Some(thread_id),
                    )
                    .await?;
                let grant_status = trigger_grant_status(asked);
                if let (Some(_), Some(engine)) = (&grant_notice, self.try_clone_arc()) {
                    crate::scheduler::user_tasks::notify_trigger_grants_cleared(&engine, &existing)
                        .await;
                }

                let display_name = new_name.unwrap_or(&existing.name);
                let display_schedule = new_cron
                    .as_ref()
                    .map(|c| c.expressions.join(", "))
                    .unwrap_or_else(|| existing.schedule.join(", "));
                // Only the caller's own new expressions carry a preview: an
                // update that left the schedule alone has nothing new to report.
                let advice = new_cron
                    .as_ref()
                    .map(|c| c.advice_suffix())
                    .unwrap_or_default();

                Ok(format!(
                    "[ACTION COMPLETED] Updated trigger '{}' (ID: {}). Changed: {}. Schedule: {} ({}){}{}{grant_status}{}",
                    display_name, trigger_id, updated_fields.join(", "), display_schedule, existing.timezone, advice,
                    warning_suffix(&event_warnings),
                    grant_notice.unwrap_or_default()
                ))
            }
            "delete_trigger" => {
                let trigger_id = match args["trigger_id"].as_str() {
                    Some(s) if !s.is_empty() => s.to_string(),
                    _ => return Ok("Error: trigger_id is required".to_string()),
                };

                // Check if trigger exists in in-memory state
                let existing = {
                    let configs = self.trigger_configs.read().unwrap();
                    configs.get(&trigger_id).cloned()
                };
                let existing = match existing {
                    Some(c) => c,
                    None => return Ok(format!("Error: No trigger found with ID {}", trigger_id)),
                };

                let self_deleting =
                    crate::scheduler::user_tasks::is_self_deleting_trigger(&trigger_id);

                self.emit_trigger_write(
                    TriggerWrite::Deleted,
                    &trigger_id,
                    serde_json::json!({
                        "trigger_id": trigger_id,
                        "name": existing.name,
                        "self_deleting": self_deleting,
                    }),
                    actor,
                )
                .await?;

                Ok(format!(
                    "[ACTION COMPLETED] Deleted trigger '{}' (ID: {}).",
                    existing.name, trigger_id
                ))
            }
            "pause_trigger" | "resume_trigger" => {
                let paused = name == "pause_trigger";
                let trigger_id = match args["trigger_id"].as_str() {
                    Some(s) if !s.is_empty() => s.to_string(),
                    _ => return Ok("Error: trigger_id is required".to_string()),
                };
                let existing = {
                    let configs = self.trigger_configs.read().unwrap();
                    configs.get(&trigger_id).cloned()
                };
                let existing = match existing {
                    Some(c) => c,
                    None => return Ok(format!("Error: No trigger found with ID {}", trigger_id)),
                };
                if existing.paused == paused {
                    let state = if paused {
                        "already paused"
                    } else {
                        "already active"
                    };
                    return Ok(format!("Trigger '{}' is {}.", existing.name, state));
                }
                let payload = serde_json::json!({ "trigger_id": &trigger_id, "paused": paused });
                // Through the chokepoint so the registry is already paused when
                // the next tool call in this same turn reads it. `pause_trigger`
                // followed by `run_trigger` is the agent-side twin of the HTTP
                // race, and it would otherwise fire the trigger it just paused.
                self.emit_trigger_write(TriggerWrite::Updated, &trigger_id, payload, actor)
                    .await?;
                let action = if paused { "Paused" } else { "Resumed" };
                Ok(format!(
                    "[ACTION COMPLETED] {} trigger '{}' (ID: {}).",
                    action, existing.name, trigger_id
                ))
            }
            tn::RUN_TRIGGER => {
                let trigger_id = match args["trigger_id"].as_str() {
                    Some(s) if !s.is_empty() => s.to_string(),
                    _ => return Ok("Error: trigger_id is required".to_string()),
                };
                // Every refusal and the submit itself live in one place so the
                // LLM, CLI and HTTP surfaces cannot drift apart.
                match self.run_trigger_off_schedule(&trigger_id).await {
                    Ok(outcome) => Ok(format!("[ACTION COMPLETED] {}", outcome.message())),
                    Err(refusal) => Ok(format!("Error: {}", refusal.message())),
                }
            }
            "list_trigger_groups" => {
                let mut groups: Vec<crate::triggers::TriggerGroup> = {
                    let g = self.trigger_groups.read().unwrap();
                    g.values().cloned().collect()
                };
                groups.sort_by(|a, b| a.order.cmp(&b.order).then(a.created.cmp(&b.created)));

                if groups.is_empty() {
                    return Ok("No trigger groups found.".to_string());
                }

                // Denormalize member counts from the trigger registry — no
                // projection table; the in-memory state is already authoritative.
                let mut counts: std::collections::HashMap<String, usize> =
                    std::collections::HashMap::new();
                {
                    let configs = self.trigger_configs.read().unwrap();
                    for c in configs.values() {
                        if let Some(ref gid) = c.group_id {
                            *counts.entry(gid.clone()).or_insert(0) += 1;
                        }
                    }
                }
                let mut result = format!("Found {} trigger group(s):\n\n", groups.len());
                for g in &groups {
                    let n = counts.get(&g.id).copied().unwrap_or(0);
                    result.push_str(&format!(
                        "- **{}** (ID: {}) order={} members={}\n",
                        g.name, g.id, g.order, n
                    ));
                }
                Ok(result)
            }
            "create_trigger_group" => {
                use crate::engine::trigger_group_writes::CreateTriggerGroupError;
                let raw_name = args["name"].as_str().unwrap_or("");
                // `try_from`, never `as`: a wrapping cast would store an
                // unrelated in-range order for an out-of-range one.
                let explicit_order = match args
                    .get("order")
                    .and_then(|v| v.as_i64())
                    .map(i32::try_from)
                {
                    Some(Ok(n)) => Some(n),
                    Some(Err(_)) => return Ok("Error: order is out of range.".to_string()),
                    None => None,
                };
                match self
                    .create_trigger_group_serialized(raw_name, explicit_order, actor)
                    .await
                {
                    Ok(c) => Ok(format!(
                        "[ACTION COMPLETED] Created trigger group '{}' (ID: {}, order: {}).",
                        c.name, c.group_id, c.order
                    )),
                    Err(CreateTriggerGroupError::EmptyName) => {
                        Ok("Error: name is required".to_string())
                    }
                    Err(CreateTriggerGroupError::DuplicateName { existing_name }) => Ok(format!(
                        "Error: A group named '{}' already exists",
                        existing_name
                    )),
                    Err(CreateTriggerGroupError::EmitFailed(msg)) => Err(msg.into()),
                }
            }
            "rename_trigger_group" => {
                use crate::engine::trigger_group_writes::RenameTriggerGroupError;
                let group_id = match args["group_id"].as_str() {
                    Some(s) if !s.is_empty() => s.to_string(),
                    _ => return Ok("Error: group_id is required".to_string()),
                };
                let raw_new_name = args["name"].as_str().unwrap_or("");
                match self
                    .rename_trigger_group_serialized(&group_id, raw_new_name, actor)
                    .await
                {
                    Ok(_) => Ok(format!(
                        "[ACTION COMPLETED] Renamed trigger group (ID: {}) to '{}'.",
                        group_id,
                        raw_new_name.trim()
                    )),
                    Err(RenameTriggerGroupError::EmptyName) => {
                        Ok("Error: name is required".to_string())
                    }
                    Err(RenameTriggerGroupError::NotFound) => {
                        Ok(format!("Error: No group found with ID {}", group_id))
                    }
                    Err(RenameTriggerGroupError::DuplicateName { existing_name }) => Ok(format!(
                        "Error: A group named '{}' already exists",
                        existing_name
                    )),
                    Err(RenameTriggerGroupError::EmitFailed(msg)) => Err(msg.into()),
                }
            }
            "reorder_trigger_groups" => {
                let ordering = match args.get("ordering").and_then(|v| v.as_array()) {
                    Some(arr) => arr.clone(),
                    None => return Ok("Error: ordering array is required".to_string()),
                };
                let to_change: Vec<(String, i32)> = {
                    let g = self.trigger_groups.read().unwrap();
                    let mut acc = Vec::with_capacity(ordering.len());
                    for entry in &ordering {
                        let id = entry.get("id").and_then(|v| v.as_str()).unwrap_or("");
                        let order = entry
                            .get("order")
                            .and_then(|v| v.as_i64())
                            .and_then(|n| i32::try_from(n).ok());
                        let Some(order) = order.filter(|_| !id.is_empty()) else {
                            return Ok(
                                "Error: each ordering entry needs string id and integer order"
                                    .to_string(),
                            );
                        };
                        let current = match g.get(id) {
                            Some(g) => g,
                            None => {
                                return Ok(format!("Error: Unknown group_id '{}'", id));
                            }
                        };
                        if current.order != order {
                            acc.push((id.to_string(), order));
                        }
                    }
                    acc
                };
                let n = to_change.len();
                for (group_id, order) in to_change {
                    let payload = serde_json::json!({ "group_id": group_id, "order": order });
                    crate::engine::trigger_group_writes::emit_and_apply_group_event(
                        &self.trigger_groups,
                        &self.event_bus,
                        SystemEvent::TriggerGroupReordered {
                            group_id,
                            payload,
                            actor: actor.clone(),
                        },
                    )
                    .await?;
                }
                Ok(format!(
                    "[ACTION COMPLETED] Reordered {} trigger group(s).",
                    n
                ))
            }
            "delete_trigger_group" => {
                let group_id = match args["group_id"].as_str() {
                    Some(s) if !s.is_empty() => s.to_string(),
                    _ => return Ok("Error: group_id is required".to_string()),
                };
                let group_name = {
                    let g = self.trigger_groups.read().unwrap();
                    match g.get(&group_id) {
                        Some(g) => g.name.clone(),
                        None => return Ok(format!("Error: No group found with ID {}", group_id)),
                    }
                };
                // Block-delete-if-non-empty: surface the members so the LLM can
                // either move them (update_trigger group_id) or delete them.
                let members: Vec<String> = {
                    let configs = self.trigger_configs.read().unwrap();
                    configs
                        .values()
                        .filter(|c| c.group_id.as_deref() == Some(&group_id))
                        .map(|c| c.id.clone())
                        .collect()
                };
                if !members.is_empty() {
                    return Ok(format!(
                        "Error: Group '{}' still has {} member trigger(s): {}. \
                         Move them with update_trigger (group_id: null) or delete them, then retry.",
                        group_name,
                        members.len(),
                        members.join(", ")
                    ));
                }
                let payload = serde_json::json!({ "group_id": group_id });
                crate::engine::trigger_group_writes::emit_and_apply_group_event(
                    &self.trigger_groups,
                    &self.event_bus,
                    SystemEvent::TriggerGroupDeleted {
                        group_id: group_id.clone(),
                        payload,
                        actor,
                    },
                )
                .await?;
                Ok(format!(
                    "[ACTION COMPLETED] Deleted trigger group '{}' (ID: {}).",
                    group_name, group_id
                ))
            }
            _ => Err(format!("Unknown scheduler tool: {}", name).into()),
        }
    }
}

/// Both checks a `run.type = "script"` path has to pass at the boundary.
///
/// The extension check once ran alone, so `"../../evil.sh"` answered 200 with
/// a cron preview and showed as armed. `scheduler::user_tasks` refuses it at
/// fire time through the canonical guard, so every fire died instead. This is
/// the same refusal, made before the trigger exists.
pub(crate) fn validate_script_path(path: &str) -> Result<(), String> {
    crate::triggers::validate_script_extension(path)?;
    if crate::core::is_path_traversal(path) {
        return Err(format!("Invalid script path: {}", path));
    }
    Ok(())
}

/// [`validate_script_path`] for a parsed `run`, the check every create and
/// update path shares: the `/api/v1/triggers` routes and the `create_trigger` /
/// `update_trigger` LLM tools. An intent run has no path to check.
pub(crate) fn validate_trigger_run(run: &TriggerRun) -> Result<(), String> {
    match run {
        TriggerRun::Script { path } => validate_script_path(path),
        TriggerRun::Intent { .. } => Ok(()),
    }
}

/// Parse the tool's `run` argument and refuse a script path the trigger could
/// never run, before a `TriggerCreated` or `TriggerUpdated` exists.
pub(crate) fn parse_run_arg(run_val: &serde_json::Value) -> Result<TriggerRun, String> {
    let run: TriggerRun = serde_json::from_value(run_val.clone())
        .map_err(|e| format!("Invalid 'run' field: {}. {}", e, RUN_FIELD_SHAPES))?;
    validate_trigger_run(&run)?;
    Ok(run)
}

/// Build human-readable trigger description for create responses.
fn trigger_description(cron_display: &str, subscriptions: &[EventSubscription]) -> String {
    let event_display = if subscriptions.is_empty() {
        None
    } else {
        Some(
            subscriptions
                .iter()
                .map(|s| s.event_type.as_str())
                .collect::<Vec<_>>()
                .join(", "),
        )
    };
    match (cron_display.is_empty(), event_display) {
        (false, Some(ev)) => format!("schedule '{}' AND event '{}'", cron_display, ev),
        (true, Some(ev)) => format!("event '{}'", ev),
        _ => format!("schedule '{}'", cron_display),
    }
}

/// The event-type warnings, appended to a trigger tool's success text.
///
/// The write went through, so this is a note rather than an error. It names a
/// type nobody here has emitted, which is the caller's chance to catch a typo
/// in a domain event name of their own.
fn warning_suffix(warnings: &[String]) -> String {
    if warnings.is_empty() {
        return String::new();
    }
    format!("\n\nWARNING: {}", warnings.join("\nWARNING: "))
}

/// Parse the LLM tool's `mcp_servers` argument. `None` when absent. `null`
/// clears, so it reads as an empty list, and so does `[]`. A single string is
/// one id. Anything else is an `Error: ...` string the LLM can read directly.
pub(crate) fn mcp_servers_arg(
    value: Option<&serde_json::Value>,
) -> Result<Option<Vec<String>>, String> {
    match value {
        None => Ok(None),
        Some(serde_json::Value::Null) => Ok(Some(Vec::new())),
        Some(serde_json::Value::String(id)) => Ok(Some(normalize_mcp_servers([id.as_str()]))),
        Some(serde_json::Value::Array(ids)) if ids.iter().all(|v| v.is_string()) => Ok(Some(
            normalize_mcp_servers(ids.iter().filter_map(|v| v.as_str())),
        )),
        Some(_) => Err(
            "Error: 'mcp_servers' must be an array of MCP server ids, e.g. [\"slack\"]".to_string(),
        ),
    }
}

/// Parse the LLM tool's `on` argument into a Vec of subscriptions, accepting:
///
/// 1. Absent / `null` → empty Vec.
/// 2. Single string `"EventName"` → one entry, no condition.
/// 3. Array of strings `["A", "B"]` → entries with no conditions.
/// 4. Array of subscription objects
///    `[{"event_type": "X", "condition": {...}}, ...]` → as-is.
/// 5. Single subscription object → one-entry Vec.
///
/// Returns the parsed Vec on success; otherwise an `Error: ...` string the
/// LLM can read directly. Blank `event_type` strings are dropped.
pub(crate) fn parse_on_arg(
    value: Option<&serde_json::Value>,
) -> Result<Vec<EventSubscription>, String> {
    let Some(value) = value.filter(|v| !v.is_null()) else {
        return Ok(Vec::new());
    };
    let mut subscriptions = Vec::new();
    let push_string = |out: &mut Vec<EventSubscription>, s: &str| {
        let t = s.trim();
        if !t.is_empty() {
            out.push(EventSubscription {
                event_type: t.to_string(),
                condition: None,
            });
        }
    };
    if let Some(s) = value.as_str() {
        push_string(&mut subscriptions, s);
        return Ok(subscriptions);
    }
    if let Some(obj) = value.as_object() {
        let sub = EventSubscription::from_object_entry(obj).ok_or_else(|| {
            "Error: 'on' object must carry an 'event_type' (and optional 'condition')".to_string()
        })?;
        subscriptions.push(sub);
        return Ok(subscriptions);
    }
    if let Some(arr) = value.as_array() {
        for entry in arr {
            if let Some(s) = entry.as_str() {
                push_string(&mut subscriptions, s);
                continue;
            }
            let Some(obj) = entry.as_object() else {
                return Err(
                    "Error: each entry in 'on' must be an event-type string or a \
                     subscription object {event_type, condition?}"
                        .to_string(),
                );
            };
            let sub = EventSubscription::from_object_entry(obj).ok_or_else(|| {
                "Error: subscription object in 'on' missing 'event_type'".to_string()
            })?;
            subscriptions.push(sub);
        }
        return Ok(subscriptions);
    }
    Err(
        "Error: 'on' must be an event-type string, a subscription object, or an array of either"
            .to_string(),
    )
}

#[cfg(test)]
#[path = "scheduler_tests.rs"]
mod tests;
