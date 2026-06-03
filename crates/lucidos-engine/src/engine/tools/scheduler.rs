use super::super::LucidosEngine;
use crate::engine::event_bus::{BusEvent, SystemEvent};
use crate::llm::tool_names as tn;
use crate::triggers::{EventSubscription, TriggerRun};
use std::str::FromStr;

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
        let active_id = crate::scheduler::user_tasks::ACTIVE_TRIGGER_ID
            .try_with(|id| id.clone())
            .ok();
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

                // Parse cron: accepts a single string or an array of strings (optional if on is set)
                let cron_expressions = if args.get("cron").is_some() && !args["cron"].is_null() {
                    match parse_cron_arg(&args["cron"]) {
                        Ok(exprs) => exprs,
                        Err(e) => return Ok(e),
                    }
                } else {
                    Vec::new()
                };

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
                    Some(run_val) if !run_val.is_null() => {
                        serde_json::from_value(run_val.clone())
                            .map_err(|e| format!("Invalid 'run' field: {}. Expected {{ type: 'intent', intent: '...' }} or {{ type: 'script', path: '...' }}", e))?
                    }
                    _ => {
                        // Backward compat: accept prompt_text as a shorthand
                        let prompt_text = args.get("prompt_text").and_then(|v| v.as_str()).unwrap_or("");
                        if prompt_text.is_empty() {
                            return Ok("Error: 'run' is required. Use { type: 'intent', intent: '...' } or { type: 'script', path: '...' }".to_string());
                        }
                        TriggerRun::Intent { intent: prompt_text.to_string() }
                    }
                };

                // Check if timezone is set - required for triggers
                let tz_val = self.user_timezone.read().await.clone();
                if tz_val.is_empty() {
                    return Ok("Error: User timezone is not set. Use set_timezone tool first to set the user's timezone before creating triggers.".to_string());
                }

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

                self.event_bus
                    .emit(BusEvent::System(SystemEvent::TriggerCreated {
                        trigger_id: trigger_id_str.clone(),
                        payload: event_payload,
                        actor: None,
                    }))
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
                    "[ACTION COMPLETED] Created trigger '{}' (ID: {}) running {} with {} in timezone {}.",
                    name, trigger_id_str, run_desc, trigger_desc, tz_val
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

                let new_name = args.get("name").and_then(|v| v.as_str());
                let new_run: Option<TriggerRun> = match args.get("run").filter(|v| !v.is_null()) {
                    Some(v) => Some(serde_json::from_value(v.clone())
                        .map_err(|e| format!("Invalid 'run' field: {}. Expected {{ type: 'intent', text: '...' }} or {{ type: 'script', path: '...' }}", e))?),
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
                // Parse cron: Option<Vec<String>>
                // None = not provided (keep existing), Some(vec![]) = clear, Some(vec![...]) = set
                let new_cron: Option<Vec<String>> = if args.get("cron").is_some() {
                    if args["cron"].is_null() {
                        Some(vec![])
                    } else {
                        Some(match parse_cron_arg(&args["cron"]) {
                            Ok(exprs) => exprs,
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

                if new_name.is_none()
                    && new_run.is_none()
                    && new_cron.is_none()
                    && new_on.is_none()
                    && new_paused.is_none()
                    && new_app_id.is_none()
                    && new_go_to_review.is_none()
                    && new_group_id.is_none()
                {
                    return Ok(
                        "Error: At least one field besides trigger_id must be provided".to_string(),
                    );
                }

                // Read existing config from in-memory state
                let existing = {
                    let configs = self.trigger_configs.read().unwrap();
                    configs.get(&trigger_id).cloned()
                };
                let existing = match existing {
                    Some(c) => c,
                    None => return Ok(format!("Error: No trigger found with ID {}", trigger_id)),
                };

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
                    update_payload["schedule"] = serde_json::json!(crons);
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

                // Ensure trigger still has at least one firing mechanism
                let updated_schedule = new_cron.as_ref().unwrap_or(&existing.schedule);
                let updated_on = new_on.as_ref().unwrap_or(&existing.on);
                if updated_schedule.is_empty() && updated_on.is_empty() {
                    return Ok(
                        "Error: Trigger must have at least one cron schedule or event subscription"
                            .to_string(),
                    );
                }

                self.event_bus
                    .emit(BusEvent::System(SystemEvent::TriggerUpdated {
                        trigger_id: trigger_id.clone(),
                        payload: update_payload,
                        actor: None,
                    }))
                    .await?;

                let display_name = new_name.unwrap_or(&existing.name);
                let display_schedule = new_cron
                    .as_ref()
                    .map(|c| c.join(", "))
                    .unwrap_or_else(|| existing.schedule.join(", "));

                Ok(format!(
                    "[ACTION COMPLETED] Updated trigger '{}' (ID: {}). Changed: {}. Schedule: {} ({})",
                    display_name, trigger_id, updated_fields.join(", "), display_schedule, existing.timezone
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

                // Emit via EventBus
                self.event_bus
                    .emit(BusEvent::System(SystemEvent::TriggerDeleted {
                        trigger_id: trigger_id.clone(),
                        payload: serde_json::json!({
                            "trigger_id": trigger_id,
                            "name": existing.name,
                            "self_deleting": self_deleting,
                        }),
                        actor: None,
                    }))
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
                    let state = if paused { "already paused" } else { "already active" };
                    return Ok(format!("Trigger '{}' is {}.", existing.name, state));
                }
                let payload = serde_json::json!({ "trigger_id": &trigger_id, "paused": paused });
                self.event_bus
                    .emit(BusEvent::System(SystemEvent::TriggerUpdated {
                        trigger_id: trigger_id.clone(),
                        payload,
                        actor: None,
                    }))
                    .await?;
                let action = if paused { "Paused" } else { "Resumed" };
                Ok(format!("[ACTION COMPLETED] {} trigger '{}' (ID: {}).", action, existing.name, trigger_id))
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
                let explicit_order = args
                    .get("order")
                    .and_then(|v| v.as_i64())
                    .map(|n| n as i32);
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
                    Err(CreateTriggerGroupError::DuplicateName { existing_name }) => {
                        Ok(format!(
                            "Error: A group named '{}' already exists",
                            existing_name
                        ))
                    }
                    Err(CreateTriggerGroupError::EmitFailed(msg)) => {
                        Err(msg.into())
                    }
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
                    Err(RenameTriggerGroupError::DuplicateName { existing_name }) => {
                        Ok(format!(
                            "Error: A group named '{}' already exists",
                            existing_name
                        ))
                    }
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
                        let order = entry.get("order").and_then(|v| v.as_i64()).map(|n| n as i32);
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
                    let payload =
                        serde_json::json!({ "group_id": group_id, "order": order });
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
pub(crate) fn parse_on_arg(value: Option<&serde_json::Value>) -> Result<Vec<EventSubscription>, String> {
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
            "Error: 'on' object must carry an 'event_type' (and optional 'condition')"
                .to_string()
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

    if dow == "*" || dow.starts_with("*/") || dow.chars().any(|c| c.is_ascii_alphabetic()) {
        return expr.to_string();
    }

    let translated_dow = dow
        .split(',')
        .map(|segment| {
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

/// Parse the `cron` tool argument, which can be either a single string or a JSON array of strings.
/// Returns a Vec of validated cron expression strings, or an error message.
pub(crate) fn parse_cron_arg(value: &serde_json::Value) -> Result<Vec<String>, String> {
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

    // Validate each expression
    for expr in &expressions {
        let parts: Vec<&str> = expr.split_whitespace().collect();
        if parts.len() != 6 {
            return Err(format!(
                "Error: Invalid cron expression '{}'. Must have 6 fields: second minute hour day-of-month month day-of-week. Example: '0 0 8 * * *' for 8am daily.",
                expr
            ));
        }
        if parse_standard_cron(expr).is_err() {
            return Err(format!(
                "Error: Invalid cron expression '{}'. Check syntax.",
                expr
            ));
        }
    }

    Ok(expressions)
}

/// Find the nearest next occurrence across multiple cron schedules.
/// Returns the earliest upcoming time from any of the schedules.
pub(crate) fn next_occurrence_multi(
    schedules: &[cron::Schedule],
    tz: chrono_tz::Tz,
) -> Option<chrono::DateTime<chrono_tz::Tz>> {
    schedules.iter().filter_map(|s| s.upcoming(tz).next()).min()
}

#[cfg(test)]
#[path = "scheduler_tests.rs"]
mod tests;
