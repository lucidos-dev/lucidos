use super::super::LucidosEngine;

/// Maximum nesting depth for `execute_intent`. A healthy run never nests at
/// all: the intent sub-loop's tool set (`build_intent_tools`) deliberately
/// excludes `execute_intent`. This is a hard backstop — if that exclusion ever
/// regresses, or a future nested-loop tool re-enters here, we return a clear
/// error instead of recursing until the worker thread's stack overflows and the
/// engine aborts (SIGABRT). The actual headroom for the *legitimate* (bounded)
/// agentic-loop → intent-sub-loop chain comes from the enlarged worker stack
/// (`WORKER_THREAD_STACK_SIZE` in `main.rs`); this guard exists so a *runaway*
/// can never silently turn into a stack overflow again.
const MAX_INTENT_DEPTH: u32 = 4;

const INTENT_EXECUTION_RULES: &str = "\
[EXECUTION RULES]
- Fulfill the intent in this run. If a tool is needed, call the tool instead of explaining a future plan.
- Use cached data in artifacts before making fresh API calls.
- Emit events for outcomes via emit_event tool. Query past events via query_events tool.
- Show errors to the user. Never swallow errors silently.
[END EXECUTION RULES]";

tokio::task_local! {
    /// Current `execute_intent` nesting depth on this task. Read at the top of
    /// `handle_execute_intent`; the inner sub-loop runs scoped to `depth + 1`.
    static INTENT_DEPTH: u32;
}

/// True when an `execute_intent` call at `current_depth` must be refused to
/// avoid unbounded intent nesting (and the stack overflow it would cause).
fn intent_nesting_exceeded(current_depth: u32) -> bool {
    current_depth >= MAX_INTENT_DEPTH
}

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
                            crate::core::home_path::abbreviate(&path),
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

        // Hard depth guard: refuse before recursing far enough to overflow the
        // worker stack. `execute_intent` is excluded from the intent sub-loop's
        // tools, so this should never trip in practice — it's the backstop that
        // turns a regression into a clear, logged error instead of a SIGABRT.
        let depth = INTENT_DEPTH.try_with(|d| *d).unwrap_or(0);
        if intent_nesting_exceeded(depth) {
            log!(
                "[Intents] Refusing to execute '{}' — intent nesting depth {} reached limit {}",
                intent_id,
                depth,
                MAX_INTENT_DEPTH
            );
            return Err(format!(
                "Error: refusing to execute intent '{}' — intent execution is already nested {} \
                 levels deep (limit {}). An intent must not recursively execute intents; this \
                 limit prevents a stack overflow.",
                intent_id, depth, MAX_INTENT_DEPTH
            )
            .into());
        }

        // Load from IntentStore (app-scoped + standalone triggers)
        let data_dir = self.workspace_path.join(crate::core::DATA_DIR);
        let intent = crate::core::IntentStore::load(&data_dir, intent_id)
            .ok_or_else(|| format!("Intent '{}' not found", intent_id))?;

        // Load referenced know-how (general + app-specific if this is an app intent)
        let kh_dirs = self.knowhow_dirs();
        let system_dir = self.system_knowhow_dir();
        let mut knowhow_context = crate::core::knowhow::load_knowhow_sections_merged(
            &kh_dirs,
            system_dir,
            &intent.knowhow,
        );

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

        let system_prompt = format!(
            "{}\n\n[INTENT — {}]\n{}\n[END INTENT]{}",
            INTENT_EXECUTION_RULES, intent_id, intent.content, knowhow_context
        );

        // Run isolated sub-loop, scoped one level deeper so a (mis)nested
        // execute_intent inside it is caught by the depth guard above.
        let result = INTENT_DEPTH
            .scope(
                depth + 1,
                self.run_intent_loop(
                    &system_prompt,
                    task,
                    request_id,
                    extraction_ctx,
                    device_id,
                    cancel_token,
                    thread_id,
                ),
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

    #[test]
    fn intent_nesting_guard_boundary() {
        // The guard must allow normal (un-nested / shallowly nested) execution
        // and refuse once the nesting reaches the cap — that refusal is what
        // turns a runaway into a clean error instead of a stack-overflow abort.
        assert!(
            !intent_nesting_exceeded(0),
            "top-level execute_intent allowed"
        );
        assert!(
            !intent_nesting_exceeded(MAX_INTENT_DEPTH - 1),
            "one below the cap still allowed"
        );
        assert!(
            intent_nesting_exceeded(MAX_INTENT_DEPTH),
            "at the cap must be refused"
        );
        assert!(
            intent_nesting_exceeded(MAX_INTENT_DEPTH + 1),
            "above the cap must be refused"
        );
    }

    #[test]
    fn intent_execution_rules_avoid_echo_prone_slogans() {
        assert!(
            !INTENT_EXECUTION_RULES.contains("ALWAYS EXECUTE, NEVER DESCRIBE"),
            "intent execution rules should avoid slogans Gemini may echo"
        );
        assert!(
            !INTENT_EXECUTION_RULES.contains("If you catch yourself"),
            "intent execution rules should avoid self-referential meta-instructions"
        );
        assert!(INTENT_EXECUTION_RULES.contains("emit_event"));
        assert!(INTENT_EXECUTION_RULES.contains("query_events"));
    }

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
        write_knowhow_file(&local.join("my-doc.md"), "My Doc", "Body of my doc.");

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
        assert_eq!(
            loaded.len(),
            1,
            "expected one loaded doc, got: {:?}",
            loaded
        );
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
        assert!(err.to_string().contains("id is required"), "got: {}", err);

        let err = load_knowhow_impl(&dirs(&local), None, &store, thread_id, &json!({ "id": "" }))
            .await
            .expect_err("empty id must error");
        assert!(err.to_string().contains("id is required"), "got: {}", err);
    }
}
