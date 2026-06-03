use super::super::LucidosEngine;

impl LucidosEngine {
    pub(crate) async fn execute_app_tool(
        &self,
        name: &str,
        args: &serde_json::Value,
        thread_id: uuid::Uuid,
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
                    Err(e) => Err(format!("creating app: {}", e).into()),
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
                Err(e) => Err(format!("listing apps: {}", e).into()),
            },
            "load_knowhow" => {
                load_knowhow_impl(
                    &self.knowhow_dirs(),
                    self.system_knowhow_dir(),
                    &self.loaded_knowhow,
                    thread_id,
                    args,
                )
                .await
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
        let system_dir = self.system_knowhow_dir();
        let mut knowhow_context =
            crate::core::knowhow::load_knowhow_sections_merged(&kh_dirs, system_dir, &intent.knowhow);

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
                cancel_token,
                thread_id,
            )
            .await?;

        Ok(result)
    }
}

/// Validation + side-effect core for the `load_knowhow` tool, factored out
/// of the `LucidosEngine` impl so unit tests can exercise the loader and
/// store-insertion semantics against fixture knowhow dirs without booting
/// the full engine. The handler on `LucidosEngine` is now a thin wrapper.
///
/// On success: returns the formatted `[KNOW-HOW: …]` / `[SYSTEM-KNOWHOW: …]`
/// body and inserts the doc into the per-thread loaded set so subsequent
/// turns can dedupe via the `[LOADED KNOWHOW]` user-message section.
///
/// On miss: returns the canonical not-found sentinel (see
/// [`crate::core::knowhow::knowhow_not_found_body`]) and DOES NOT insert —
/// only real docs belong in the loaded set.
pub(crate) async fn load_knowhow_impl(
    kh_dirs: &crate::core::knowhow::KnowhowDirs,
    system_dir: Option<&std::path::Path>,
    loaded_knowhow: &crate::engine::loaded_knowhow::LoadedKnowhowStore,
    thread_id: uuid::Uuid,
    args: &serde_json::Value,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let id = args["id"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or("id is required")?;
    let body = crate::core::knowhow::load_one_knowhow_section(kh_dirs, system_dir, id)
        .unwrap_or_else(|| crate::core::knowhow::knowhow_not_found_body(id));
    if !crate::core::knowhow::is_not_found_body(&body) {
        loaded_knowhow
            .insert(
                thread_id,
                crate::engine::loaded_knowhow::LoadedKnowhow {
                    id: id.to_string(),
                    body: body.clone(),
                },
            )
            .await;
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::knowhow::{write_knowhow_file, KnowhowDirs};
    use crate::engine::loaded_knowhow::LoadedKnowhowStore;
    use serde_json::json;
    use uuid::Uuid;

    fn dirs(local: &std::path::Path) -> KnowhowDirs {
        KnowhowDirs {
            shared: None,
            local: local.to_path_buf(),
            apps: None,
            triggers: None,
        }
    }

    #[tokio::test]
    async fn load_knowhow_inserts_into_store_on_hit() {
        let tmp = tempfile::tempdir().unwrap();
        let local = tmp.path().join("local");
        write_knowhow_file(
            &local.join("my-doc.md"),
            "My Doc",
            "Body of my doc.",
        );

        let store = LoadedKnowhowStore::new();
        let thread_id = Uuid::new_v4();
        let body = load_knowhow_impl(
            &dirs(&local),
            None,
            &store,
            thread_id,
            &json!({ "id": "my-doc" }),
        )
        .await
        .expect("load should succeed");

        // Returned body is the formatted [KNOW-HOW: …] section.
        assert!(body.contains("[KNOW-HOW: My Doc]"), "got: {}", body);
        assert!(body.contains("Body of my doc."), "got: {}", body);

        // And the per-thread loaded set now contains exactly this doc.
        let loaded = store.for_thread(thread_id).await;
        assert_eq!(loaded.len(), 1, "expected one loaded doc, got: {:?}", loaded);
        assert_eq!(loaded[0].id, "my-doc");
        assert_eq!(loaded[0].body, body);
    }

    #[tokio::test]
    async fn load_knowhow_does_not_insert_on_miss() {
        let tmp = tempfile::tempdir().unwrap();
        let local = tmp.path().join("local");
        std::fs::create_dir_all(&local).unwrap();

        let store = LoadedKnowhowStore::new();
        let thread_id = Uuid::new_v4();
        let body = load_knowhow_impl(
            &dirs(&local),
            None,
            &store,
            thread_id,
            &json!({ "id": "no-such-doc" }),
        )
        .await
        .expect("miss returns Ok with sentinel body, not Err");

        // Body is the canonical not-found sentinel.
        assert!(
            crate::core::knowhow::is_not_found_body(&body),
            "miss must return the not-found sentinel, got: {}",
            body
        );

        // And the per-thread loaded set is untouched — only real docs belong.
        let loaded = store.for_thread(thread_id).await;
        assert!(
            loaded.is_empty(),
            "miss must not insert into the loaded set, got: {:?}",
            loaded
        );
    }

    #[tokio::test]
    async fn load_knowhow_rejects_missing_or_empty_id() {
        let tmp = tempfile::tempdir().unwrap();
        let local = tmp.path().join("local");
        std::fs::create_dir_all(&local).unwrap();

        let store = LoadedKnowhowStore::new();
        let thread_id = Uuid::new_v4();

        let err = load_knowhow_impl(&dirs(&local), None, &store, thread_id, &json!({}))
            .await
            .expect_err("missing id must error");
        assert!(
            err.to_string().contains("id is required"),
            "got: {}",
            err
        );

        let err = load_knowhow_impl(
            &dirs(&local),
            None,
            &store,
            thread_id,
            &json!({ "id": "" }),
        )
        .await
        .expect_err("empty id must error");
        assert!(
            err.to_string().contains("id is required"),
            "got: {}",
            err
        );
    }
}
