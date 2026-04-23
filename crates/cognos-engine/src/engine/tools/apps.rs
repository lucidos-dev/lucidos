use super::super::CognosEngine;

impl CognosEngine {
    pub(crate) async fn execute_app_tool(
        &self,
        name: &str,
        args: &serde_json::Value,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        match name {
            "create_app" => {
                let id = args["id"].as_str().unwrap_or("unnamed");
                let name = args["name"].as_str().unwrap_or(id);
                let description = args["description"].as_str().unwrap_or("");
                let html_content = args["html_content"]
                    .as_str()
                    .or_else(|| args["instructions"].as_str())
                    .unwrap_or("");

                match self
                    .app_manager
                    .create_app(id, name, description, html_content)
                {
                    Ok((path, commit)) => {
                        // Stamp know-how references into the new app's manifest
                        let kh_dirs = self.knowhow_dirs();
                        let knowhow_summaries =
                            crate::core::KnowhowStore::load_merged_summaries(&kh_dirs);
                        if let Err(e) = self
                            .app_manager
                            .stamp_knowhow(id, &knowhow_summaries, &[], self.embedder.as_ref())
                            .await
                        {
                            log!("[Apps] Failed to stamp know-how for {}: {}", id, e);
                        }
                        if let Err(e) = self
                            .event_bus
                            .emit(crate::engine::event_bus::BusEvent::System(
                                crate::engine::event_bus::SystemEvent::AppCreated {
                                    app_id: id.to_string(),
                                    name: Some(name.to_string()),
                                    actor: None,
                                },
                            ))
                            .await
                        {
                            log!("[Apps] Failed to emit AppCreated event: {}", e);
                        }
                        Ok(format!(
                            "Created app '{}' at {} (commit: {})",
                            name,
                            path.display(),
                            &commit[..commit.floor_char_boundary(8)]
                        ))
                    }
                    Err(e) => Ok(format!("Error creating app: {}", e)),
                }
            }
            "list_apps" => match self.app_manager.list_apps() {
                Ok(apps) => {
                    if apps.is_empty() {
                        Ok("No apps found.".to_string())
                    } else {
                        Ok(apps
                            .iter()
                            .map(|a| format!("- {} (id: `{}`): {}", a.name, a.id, a.description))
                            .collect::<Vec<_>>()
                            .join("\n"))
                    }
                }
                Err(e) => Ok(format!("Error listing apps: {}", e)),
            },
            "load_knowhow" => {
                let id = args["id"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .ok_or("id is required")?;

                // System docs: explicit `system-docs/<id>` prefix routes to the
                // engine-shipped, read-only docs at <repo>/system-docs/.
                if let Some(sys_id) = id.strip_prefix("system-docs/") {
                    let dir = self.system_docs_dir().ok_or("System docs not available")?;
                    return Ok(match crate::core::SystemDocsStore::load(dir, sys_id) {
                        Some(sd) => crate::core::SystemDocsStore::format_section(&sd),
                        None => format!(
                            "System doc '{}' not found. Check the System Docs list in the system prompt for available IDs.",
                            id
                        ),
                    });
                }

                // Try top-level knowhow first, then app-scoped (app_id/knowhow_id)
                let kh_dirs = self.knowhow_dirs();
                let kh =
                    crate::core::KnowhowStore::load_with_fallback(&kh_dirs, id).or_else(|| {
                        // App-scoped: "app_id/knowhow_id" → apps/app_id/knowhow/knowhow_id.md
                        let (app_id, kh_id) = id.split_once('/')?;
                        if app_id.contains("..")
                            || app_id.starts_with('/')
                            || app_id.starts_with('\\')
                        {
                            return None;
                        }
                        let app_kh_dir = self
                            .workspace_path
                            .join(crate::core::APPS_DIR)
                            .join(app_id)
                            .join("knowhow");
                        crate::core::KnowhowStore::load(&app_kh_dir, kh_id)
                    });
                match kh {
                    Some(kh) => Ok(kh.format_section()),
                    None => Ok(format!("Know-how '{}' not found. Use the know-how list in the system prompt to see available IDs.", id)),
                }
            }
            _ => Ok(format!("Unknown app tool: {}", name)),
        }
    }

    /// Handle the execute_intent tool call.
    ///
    /// Loads an intent by ID, builds an isolated system prompt
    /// with relevant know-how, and runs a sub agentic loop.
    pub(crate) async fn handle_execute_intent(
        &self,
        args: &serde_json::Value,
        extraction_ctx: &str,
        request_id: uuid::Uuid,
        device_id: Option<&str>,
        cancel_token: &tokio_util::sync::CancellationToken,
        thread_id: uuid::Uuid,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let intent_id = args["intent_id"]
            .as_str()
            .or_else(|| args["prompt_id"].as_str())
            .ok_or("intent_id is required")?;
        let task = args["task"].as_str().unwrap_or("");

        // Load from IntentStore (app-scoped + standalone triggers)
        let data_dir = self.workspace_path.join(crate::core::DATA_DIR);
        let intent = crate::core::IntentStore::load(&data_dir, intent_id)
            .ok_or_else(|| format!("Intent '{}' not found", intent_id))?;

        // Load referenced know-how (general + app-specific if this is an app intent)
        let kh_dirs = self.knowhow_dirs();
        let mut knowhow_context =
            crate::core::knowhow::load_knowhow_sections_merged(&kh_dirs, &intent.knowhow);

        // If this is an app intent (id contains '/'), also load that app's knowhow
        if let Some((app_id, _)) = intent_id.split_once('/') {
            let app_knowhow_dir = self
                .workspace_path
                .join(crate::core::APPS_DIR)
                .join(app_id)
                .join("knowhow");
            let app_knowhow = crate::core::knowhow::load_app_knowhow(&app_knowhow_dir);
            if !app_knowhow.is_empty() {
                knowhow_context.push_str(&app_knowhow);
            }
        }

        // Build execution rules
        let rules = "\
[EXECUTION RULES]
- ALWAYS EXECUTE, NEVER DESCRIBE. Call tools, don't describe what you would do. \
If you catch yourself writing \"I'll do X\" without a tool call, STOP.
- Use cached data in artifacts before making fresh API calls.
- Emit events for outcomes via emit_event tool. Query past events via query_events tool.
- Show errors to the user. Never swallow errors silently.
[END EXECUTION RULES]";

        let system_prompt = format!(
            "{}\n\n[INTENT — {}]\n{}\n[END INTENT]{}",
            rules, intent_id, intent.content, knowhow_context
        );

        // Run isolated sub-loop
        let result = self
            .run_intent_loop(
                &system_prompt,
                task,
                request_id,
                extraction_ctx,
                device_id,
                intent_id,
                cancel_token,
                thread_id,
            )
            .await?;

        Ok(result)
    }
}
