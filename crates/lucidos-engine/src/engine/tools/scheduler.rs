use super::super::LucidosEngine;
use crate::engine::event_bus::{BusEvent, SystemEvent};
use crate::engine::trigger_writes::TriggerWrite;
use crate::llm::tool_names as tn;
use crate::triggers::{
    normalize_route_setting, validate_trigger_reasoning_effort, EventSubscription, TriggerRun,
};
// Lets the never-fires diagnosis and the AND-footgun check read a parsed
// schedule's day-of-month / month / day-of-week ordinal sets, instead of
// re-parsing the cron field strings by hand.
use cron::TimeUnitSpec;
use std::str::FromStr;

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

impl LucidosEngine {
    pub(crate) async fn execute_scheduler_tool(
        &self,
        name: &str,
        args: &serde_json::Value,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
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
                    Some(run_val) if !run_val.is_null() => serde_json::from_value(run_val.clone())
                        .map_err(|e| format!("Invalid 'run' field: {}. {}", e, RUN_FIELD_SHAPES))?,
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

                self.emit_trigger_write(
                    TriggerWrite::Created,
                    &trigger_id_str,
                    event_payload,
                    None,
                )
                .await?;

                let trigger_desc = trigger_description(&cron_display, &subscriptions);
                let run_desc = match &run {
                    TriggerRun::Intent { intent } => {
                        let end = intent.floor_char_boundary(50);
                        format!("intent '{}'", &intent[..end])
                    }
                    TriggerRun::Script { path } => format!("script '{}'", path),
                };

                Ok(format!(
                    "[ACTION COMPLETED] Created trigger '{}' (ID: {}) running {} with {} in timezone {}.{}{}",
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
                        result.push_str(&format!(
                            "- **{}** (ID: {}) [{}]\n{}{}  Status: {}\n  Last run: {}\n  Run: {}\n\n",
                            config.name, config.id, trigger_type, schedule_display, event_display, status, last_run, run_info
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
                    Some(v) => Some(serde_json::from_value(v.clone()).map_err(|e| {
                        format!("Invalid 'run' field: {}. {}", e, RUN_FIELD_SHAPES)
                    })?),
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
                // model / reasoning_effort: Some(None) = clear back to the
                // account chat default, Some(Some(s)) = set, None = absent.
                let new_model: Option<Option<String>> = args
                    .get("model")
                    .map(|v| normalize_route_setting(v.as_str()));
                let new_reasoning_effort: Option<Option<String>> =
                    match args.get("reasoning_effort") {
                        Some(v) => match validate_trigger_reasoning_effort(v.as_str()) {
                            Ok(normalized) => Some(normalized),
                            Err(e) => return Ok(format!("Error: {}", e)),
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

                self.emit_trigger_write(TriggerWrite::Updated, &trigger_id, update_payload, None)
                    .await?;

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
                    "[ACTION COMPLETED] Updated trigger '{}' (ID: {}). Changed: {}. Schedule: {} ({}){}{}",
                    display_name, trigger_id, updated_fields.join(", "), display_schedule, existing.timezone, advice,
                    warning_suffix(&event_warnings)
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
                    None,
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
                self.emit_trigger_write(TriggerWrite::Updated, &trigger_id, payload, None)
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
                let explicit_order = args.get("order").and_then(|v| v.as_i64()).map(|n| n as i32);
                match self
                    .create_trigger_group_serialized(raw_name, explicit_order, None)
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
                    .rename_trigger_group_serialized(&group_id, raw_new_name, None)
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
                            .map(|n| n as i32);
                        if id.is_empty() || order.is_none() {
                            return Ok(
                                "Error: each ordering entry needs string id and integer order"
                                    .to_string(),
                            );
                        }
                        let order = order.unwrap();
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
                    self.event_bus
                        .emit(BusEvent::System(SystemEvent::TriggerGroupReordered {
                            group_id,
                            payload,
                            actor: None,
                        }))
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
                self.event_bus
                    .emit(BusEvent::System(SystemEvent::TriggerGroupDeleted {
                        group_id: group_id.clone(),
                        payload,
                        actor: None,
                    }))
                    .await?;
                Ok(format!(
                    "[ACTION COMPLETED] Deleted trigger group '{}' (ID: {}).",
                    group_name, group_id
                ))
            }
            _ => Ok(format!("Unknown scheduler tool: {}", name)),
        }
    }
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

/// Translate the day-of-week field from standard cron convention (0=Sun, 1=Mon, ..., 6=Sat, 7=Sun)
/// to the `cron` crate's 1-based convention (1=Sun, 2=Mon, ..., 7=Sat).
fn translate_dow_for_cron_crate(expr: &str) -> String {
    fn shift(s: &str) -> Option<String> {
        s.parse::<u8>()
            .ok()
            .filter(|&n| n <= 7)
            .map(|n| format!("{}", (n % 7) + 1))
    }

    let parts: Vec<&str> = expr.split_whitespace().collect();
    if parts.len() != 6 {
        return expr.to_string();
    }

    let dow = parts[5];

    if dow == "*" || dow.starts_with("*/") {
        return expr.to_string();
    }

    // Decide named vs numeric per comma segment, not for the whole field. A
    // mixed field (`Mon,1`) carries one segment of each kind: the named
    // segment has no digits to shift, and the numeric one still needs the
    // shift even though its sibling segment is a name. A new segment shape
    // needs the same check added to `numeric_dow_range_with_step_token`
    // below.
    //
    // A combined range-and-step token (`1-5/2`) is not shifted here, and
    // never needs to be: `validate_cron_expressions` rejects it first, via
    // `numeric_dow_range_with_step_token` below (see its doc comment for why).
    let translated_dow = dow
        .split(',')
        .map(|segment| {
            if segment.chars().any(|c| c.is_ascii_alphabetic()) {
                return segment.to_string();
            }
            if let Some((a, b)) = segment.split_once('-') {
                match (shift(a), shift(b)) {
                    (Some(a), Some(b)) => format!("{}-{}", a, b),
                    _ => segment.to_string(),
                }
            } else if let Some((base, step)) = segment.split_once('/') {
                match shift(base) {
                    Some(b) => format!("{}/{}", b, step),
                    None => segment.to_string(),
                }
            } else {
                shift(segment).unwrap_or_else(|| segment.to_string())
            }
        })
        .collect::<Vec<_>>()
        .join(",");

    format!(
        "{} {} {} {} {} {}",
        parts[0], parts[1], parts[2], parts[3], parts[4], translated_dow
    )
}

/// Parse a standard cron expression into a `cron::Schedule`, translating day-of-week
/// from standard convention (0=Sun, 6=Sat) to the `cron` crate's 1-based convention (1=Sun, 7=Sat).
pub(crate) fn parse_standard_cron(expr: &str) -> Result<cron::Schedule, String> {
    let translated = translate_dow_for_cron_crate(expr);
    cron::Schedule::from_str(&translated).map_err(|e| e.to_string())
}

/// Resolve an IANA timezone name for cron validation, falling back to UTC.
///
/// The fallback is safe for the never-fires guard, which is timezone-independent:
/// Feb 31 does not exist anywhere. It only shifts the wall-clock times reported
/// in the preview, so a bad name is logged rather than swallowed.
pub(crate) fn cron_tz_or_utc(name: &str, context: &str) -> chrono_tz::Tz {
    match name.parse() {
        Ok(tz) => tz,
        Err(_) => {
            log!(
                "[Scheduler] Invalid timezone '{}' for {}, validating cron in UTC",
                name,
                context
            );
            chrono_tz::UTC
        }
    }
}

/// How many upcoming fire times a create / update reports back. Small on
/// purpose: three is enough to make a wrong schedule obvious (a "monthly" job
/// that lists three dates a year apart) without turning the response into a
/// calendar.
pub(crate) const CRON_PREVIEW_COUNT: usize = 3;

/// A validated set of cron expressions, plus the advice a caller should surface
/// alongside it.
///
/// `warnings` are deliberately NOT errors: an expression that restricts both
/// day-of-month and day-of-week is legal, and is how the nth-weekday recipes are
/// written, so the warning rides along with the success instead of replacing it.
/// `next_runs` is merged across the whole set under OR semantics, because that is
/// how a trigger with several expressions actually fires.
#[derive(Debug, Clone, Default)]
pub(crate) struct ValidatedCron {
    pub expressions: Vec<String>,
    pub warnings: Vec<String>,
    pub next_runs: Vec<chrono::DateTime<chrono_tz::Tz>>,
}

impl ValidatedCron {
    /// The preview rendered for a text surface (the LLM tool result). Empty when
    /// the set itself is empty (an update that clears the schedule).
    pub fn preview_line(&self) -> Option<String> {
        if self.next_runs.is_empty() {
            return None;
        }
        let times: Vec<String> = self
            .next_runs
            .iter()
            .map(|t| t.format("%Y-%m-%d %H:%M %Z").to_string())
            .collect();
        Some(format!("Next {} runs: {}.", times.len(), times.join(", ")))
    }

    /// The preview rendered for a JSON surface (the HTTP API, and through it the
    /// CLI and the SDK).
    pub fn next_runs_rfc3339(&self) -> Vec<String> {
        self.next_runs.iter().map(|t| t.to_rfc3339()).collect()
    }

    /// Text appended to a create / update tool result: the next-run preview, then
    /// any AND-footgun warnings. Empty when there is nothing to add, so the
    /// caller can concatenate it unconditionally.
    pub fn advice_suffix(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if let Some(line) = self.preview_line() {
            parts.push(line);
        }
        parts.extend(self.warnings.iter().map(|w| format!("WARNING: {}", w)));
        if parts.is_empty() {
            String::new()
        } else {
            format!(" {}", parts.join(" "))
        }
    }
}

/// Find a numeric day-of-week range-with-step token (`1-5/2`) in a day-of-week
/// field, and return it verbatim.
///
/// `translate_dow_for_cron_crate` cannot shift this shape. It reads the
/// range end as `5/2`, fails to parse it, and leaves the token unshifted.
/// The token then fires on the `cron` crate's own numbering, not the user's.
/// A named token (`Mon-Fri/2`) has no such gap: names bypass translation and
/// the crate numbers them the same way the user wrote them.
///
/// Checked per comma-separated segment, not on the field as a whole. A
/// mixed field (`Mon,1-5/2`) must still catch the numeric segment: a letter
/// in one segment skips only that segment. A new segment shape must be
/// checked here and in `translate_dow_for_cron_crate`'s classifier: past
/// fixes here missed one of the two.
fn numeric_dow_range_with_step_token(dow: &str) -> Option<&str> {
    let is_numeric = |s: &str| s.parse::<u8>().is_ok();
    dow.split(',').find(|segment| {
        let Some((range, step)) = segment.split_once('/') else {
            return false;
        };
        let Some((start, end)) = range.split_once('-') else {
            return false;
        };
        is_numeric(start) && is_numeric(end) && is_numeric(step)
    })
}

/// Validate the cron expressions a trigger will run on: field count, syntax, and
/// whether each one can ever fire at all.
///
/// The never-fires check is the point of this function. `0 0 9 31 2 *` (Feb 31)
/// parses cleanly and is accepted by every syntax check, then silently does
/// nothing forever, which is the worst failure shape available: there is no error
/// to notice. `Schedule::upcoming(tz).next()` answers `None` for exactly those
/// expressions and costs tens of microseconds. The crate bounds its own search
/// horizon, so a schedule whose only matches lie centuries out also answers
/// `None`, which for a trigger is the right answer anyway.
///
/// `tz` only affects *when* the reported runs land: a never-fires expression is
/// never-fires in every timezone, so a UTC fallback does not weaken the check.
///
/// Error messages carry no prefix, so an HTTP caller can surface them verbatim.
/// [`parse_cron_arg`] adds the `Error:` prefix the LLM tool surface expects.
pub(crate) fn validate_cron_expressions(
    expressions: Vec<String>,
    tz: chrono_tz::Tz,
) -> Result<ValidatedCron, String> {
    let mut schedules: Vec<cron::Schedule> = Vec::with_capacity(expressions.len());
    let mut warnings: Vec<String> = Vec::new();

    for expr in &expressions {
        let parts: Vec<&str> = expr.split_whitespace().collect();
        if parts.len() != 6 {
            return Err(format!(
                "Invalid cron expression '{}'. Must have 6 fields: second minute hour day-of-month month day-of-week. Example: '0 0 8 * * *' for 8am daily.",
                expr
            ));
        }
        if let Some(token) = numeric_dow_range_with_step_token(parts[5]) {
            return Err(format!(
                "Cron expression '{}' uses day-of-week token '{}', which combines a \
                 numeric range and a step. This shape is not supported: list the days \
                 instead ('1,3,5'), or use the named weekday range instead ('Mon-Fri/2').",
                expr, token
            ));
        }
        let schedule = match parse_standard_cron(expr) {
            Ok(s) => s,
            Err(_) => return Err(format!("Invalid cron expression '{}'. Check syntax.", expr)),
        };
        if schedule.upcoming(tz).next().is_none() {
            return Err(format!(
                "Cron expression '{}' can never fire: {}.",
                expr,
                diagnose_never_fires(&schedule)
            ));
        }
        if let Some(warning) = and_footgun_warning(&schedule, expr) {
            warnings.push(warning);
        }
        schedules.push(schedule);
    }

    let next_runs = next_occurrences_multi(&schedules, tz, CRON_PREVIEW_COUNT);
    Ok(ValidatedCron {
        expressions,
        warnings,
        next_runs,
    })
}

/// The last year `cron` will search (its `Years` unit is 1970-2100). Naming it
/// keeps the fallback diagnosis honest: an expression can also fail the
/// never-fires check because its next match lies past this horizon, and a user
/// staring at "no date matches" deserves to know that is a possible reason.
///
/// Rejecting that case is still correct rather than over-strict. The runner
/// consults the same `upcoming()` oracle and exits with "no more occurrences"
/// when it answers `None`, so such a trigger genuinely would not fire: accepting
/// it would recreate exactly the silent non-firing this guard exists to stop.
const CRON_SEARCH_HORIZON_YEAR: u32 = 2100;

/// Longest a month can ever be. February is 29, not 28: `0 0 9 29 2 *` is rare
/// but perfectly real (2028, 2032, 2036), and must not be rejected.
fn longest_month_length(month: u32) -> u32 {
    match month {
        2 => 29,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

const MONTH_NAMES: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// Best-effort attribution for an expression the crate says can never match.
///
/// One cause dominates in practice: a day-of-month no selected month is long
/// enough to contain (Feb 30, Feb 31, the 31st of a 30-day month). That case gets
/// named precisely, because "invalid cron expression" tells the user nothing they
/// can act on. Anything else falls back to a plain statement, which is still far
/// more useful than silence.
pub(crate) fn diagnose_never_fires(schedule: &cron::Schedule) -> String {
    let days: Vec<u32> = schedule.days_of_month().iter().collect();
    let months: Vec<u32> = schedule.months().iter().collect();

    let shortest_day = days.iter().copied().min();
    let longest_month = months.iter().copied().map(longest_month_length).max();
    if let (Some(shortest_day), Some(longest_month)) = (shortest_day, longest_month) {
        // Every selected day exceeds every selected month's ceiling, so the
        // day-of-month / month pair alone is impossible.
        if shortest_day > longest_month {
            let names: Vec<&str> = months
                .iter()
                .filter_map(|m| MONTH_NAMES.get((*m as usize).saturating_sub(1)).copied())
                .collect();
            return format!(
                "day-of-month {} never occurs in month {} ({})",
                join_ordinals(&days),
                join_ordinals(&months),
                names.join(", ")
            );
        }
    }

    format!(
        "no date matches this combination of fields before {}, the last year the cron library searches",
        CRON_SEARCH_HORIZON_YEAR
    )
}

fn join_ordinals(ordinals: &[u32]) -> String {
    ordinals
        .iter()
        .map(|o| o.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

/// True when `sorted` contains `len` consecutive integers.
fn has_consecutive_run(sorted: &[u32], len: usize) -> bool {
    if len <= 1 {
        return !sorted.is_empty();
    }
    let mut run = 1usize;
    for pair in sorted.windows(2) {
        run = if pair[1] == pair[0] + 1 { run + 1 } else { 1 };
        if run >= len {
            return true;
        }
    }
    false
}

/// The day-of-month / day-of-week AND footgun.
///
/// Within one expression the two fields are ANDed (cron 0.15). Vixie cron ORs
/// them, which is why this surprises people: `0 0 9 1 * Mon` reads as "the 1st,
/// plus every Monday" and actually fires only when the 1st IS a Monday, about
/// 1.7 times a year and in lumpy gaps.
///
/// Not every restricted pair is a mistake, so this warns rather than rejects, and
/// stays quiet for the shape that is deliberate. A day-of-month window of 7
/// consecutive days contains every weekday, so the AND is guaranteed exactly one
/// match per selected month: that is precisely how "first Monday" (`1-7 * Mon`)
/// and "last Monday" (`25-31 1,3,5,7,8,10,12 Mon`) are expressed. A narrower
/// window is the footgun, because some months then match nothing.
fn and_footgun_warning(schedule: &cron::Schedule, expr: &str) -> Option<String> {
    if schedule.days_of_month().is_all() || schedule.days_of_week().is_all() {
        return None;
    }
    let days: Vec<u32> = schedule.days_of_month().iter().collect();
    if has_consecutive_run(&days, 7) {
        return None;
    }
    Some(format!(
        "Cron expression '{}' restricts both day-of-month and day-of-week. These are ANDed, \
         not ORed: it fires only when that day-of-month IS that weekday, which can be rare. \
         For \"either one\", pass two expressions instead. For \"the first <weekday> of the \
         month\", widen day-of-month to a 7-day window (e.g. '1-7').",
        expr
    ))
}

/// Parse the `cron` tool argument, which can be either a single string or a JSON
/// array of strings, then validate the result via [`validate_cron_expressions`].
pub(crate) fn parse_cron_arg(
    value: &serde_json::Value,
    tz: chrono_tz::Tz,
) -> Result<ValidatedCron, String> {
    let expressions: Vec<String> = match value {
        serde_json::Value::String(s) => vec![s.clone()],
        serde_json::Value::Array(arr) => {
            if arr.is_empty() {
                return Err("Error: cron array must not be empty".to_string());
            }
            arr.iter()
                .map(|v| {
                    v.as_str()
                        .map(|s| s.to_string())
                        .ok_or_else(|| "Error: each cron expression must be a string".to_string())
                })
                .collect::<Result<Vec<_>, _>>()?
        }
        _ => return Err("Error: cron must be a string or array of strings".to_string()),
    };

    validate_cron_expressions(expressions, tz).map_err(|e| format!("Error: {}", e))
}

/// The first `n` fire times across a whole schedule set, merged under OR
/// semantics (a trigger fires on the earliest match from any of its expressions).
///
/// Taking `n` from each stream before merging is sufficient: the first `n` of the
/// union are always contained in the union of each stream's own first `n`.
pub(crate) fn next_occurrences_multi(
    schedules: &[cron::Schedule],
    tz: chrono_tz::Tz,
    n: usize,
) -> Vec<chrono::DateTime<chrono_tz::Tz>> {
    let mut merged: Vec<chrono::DateTime<chrono_tz::Tz>> = schedules
        .iter()
        .flat_map(|s| s.upcoming(tz).take(n))
        .collect();
    merged.sort_unstable();
    merged.dedup();
    merged.truncate(n);
    merged
}

/// Find the nearest next occurrence across multiple cron schedules.
/// Returns the earliest upcoming time from any of the schedules.
pub(crate) fn next_occurrence_multi(
    schedules: &[cron::Schedule],
    tz: chrono_tz::Tz,
) -> Option<chrono::DateTime<chrono_tz::Tz>> {
    next_occurrences_multi(schedules, tz, 1).into_iter().next()
}

#[cfg(test)]
#[path = "scheduler_tests.rs"]
mod tests;
