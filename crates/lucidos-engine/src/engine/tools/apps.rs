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
    pub(crate) static INTENT_DEPTH: u32;
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
                let sizes = match args.get("sizes").filter(|v| !v.is_null()) {
                    None => None,
                    Some(value) => {
                        let names: Vec<String> = serde_json::from_value(value.clone())
                            .map_err(|_| "sizes must be a list of size names")?;
                        Some(crate::core::apps::parse_widget_sizes(&names)?)
                    }
                };
                let kind = match args["kind"].as_str() {
                    None | Some("app") if sizes.is_some() => {
                        return Err("only a widget takes sizes".into())
                    }
                    None | Some("app") => crate::core::NewAppKind::App,
                    Some("widget") => crate::core::NewAppKind::Widget {
                        origin_thread_id: thread_id,
                        sizes,
                    },
                    Some(other) => {
                        return Err(
                            format!("kind must be 'app' or 'widget', not '{}'", other).into()
                        )
                    }
                };

                let files = crate::core::AppFiles {
                    html: html_content,
                    icon_svg: args["icon_svg"].as_str(),
                };
                let (path, commit) = self
                    .app_manager
                    .create_app(&self.event_bus, id, name, description, files, kind.clone())
                    .await
                    .map_err(|e| format!("creating app: {}", e))?;
                let commit = &commit[..commit.floor_char_boundary(8)];
                let path = crate::core::home_path::abbreviate(&path);
                let lint =
                    super::app_lint::app_lint_note(&format!("apps/{id}/index.html"), html_content)
                        .unwrap_or_default();
                if kind == crate::core::NewAppKind::App {
                    return Ok(format!(
                        "Created app '{}' at {} (commit: {}){}",
                        name, path, commit, lint
                    ));
                }
                // A reply draws it, so the agent embeds it there (ADR 0466).
                Ok(format!(
                    "Created widget '{name}' at {path} (commit: {commit}). Embed it in your \
                     reply to show it: ![{name}](app:{id}){lint}"
                ))
            }
            "list_apps" => list_apps_text(&self.app_manager),
            "load_knowhow" => load_knowhow_impl(
                &self.knowhow_dirs(),
                self.system_knowhow_dir(),
                &self.thread_loaded_knowhow(thread_id).await?,
                args,
            ),
            "unload_knowhow" => {
                unload_knowhow_impl(&self.thread_loaded_knowhow(thread_id).await?, args)
            }
            _ => Err(format!("Unknown app tool: {}", name).into()),
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

/// The `list_apps` tool's answer. Widgets are not apps here (ADR 0402).
fn list_apps_text(
    app_manager: &crate::core::AppManager,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let apps = app_manager
        .list_apps()
        .map_err(|e| format!("listing apps: {}", e))?;
    if apps.is_empty() {
        return Ok("No apps found.".to_string());
    }
    Ok(apps
        .iter()
        .map(|a| format!("- {} (id: `{}`): {}", a.name, a.id, a.description))
        .collect::<Vec<_>>()
        .join("\n"))
}

/// The `load_knowhow` tool's answer, given the thread's loaded knowhow.
///
/// The full formatted body, or the not-found sentinel for a miss. A doc this
/// turn's block already holds, unchanged on disk, gets the short kept result
/// instead (ADR 0425). Either way the recorded pair is what the next turn's
/// fold reads, so nothing else is written.
pub(crate) fn load_knowhow_impl(
    kh_dirs: &crate::core::knowhow::KnowhowDirs,
    system_dir: Option<&std::path::Path>,
    loaded: &[crate::engine::loaded_knowhow::LoadedKnowhow],
    args: &serde_json::Value,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let id = required_knowhow_id(args)?;
    let body = crate::core::knowhow::load_one_knowhow_section(kh_dirs, system_dir, id)
        .unwrap_or_else(|| crate::core::knowhow::knowhow_not_found_body(id));
    let held = loaded
        .iter()
        .any(|doc| doc.id == id && doc.body_in_block() && doc.body == body);
    Ok(if held {
        crate::engine::loaded_knowhow::kept_result(id)
    } else {
        body
    })
}

/// The `unload_knowhow` tool's answer. The recorded pair is the unload.
pub(crate) fn unload_knowhow_impl(
    loaded: &[crate::engine::loaded_knowhow::LoadedKnowhow],
    args: &serde_json::Value,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let id = required_knowhow_id(args)?;
    Ok(if loaded.iter().any(|doc| doc.id == id) {
        format!(
            "Unloaded `{id}`. It leaves the [LOADED KNOWHOW] section from the next turn. \
             Call load_knowhow again if you need it."
        )
    } else {
        format!("`{id}` is not loaded, so there is nothing to unload.")
    })
}

fn required_knowhow_id(
    args: &serde_json::Value,
) -> Result<&str, Box<dyn std::error::Error + Send + Sync>> {
    Ok(args["id"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or("id is required")?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::knowhow::{write_knowhow_file, KnowhowDirs};
    use crate::engine::loaded_knowhow::LoadedKnowhow;
    use serde_json::json;

    #[tokio::test]
    async fn the_list_apps_tool_shows_no_widget() {
        let bus = crate::test_support::offline_event_bus();
        let tmp = tempfile::tempdir().unwrap();
        let manager = crate::core::AppManager::new(tmp.path()).unwrap();
        crate::core::apps::tests::app_and_widget(&manager, &bus).await;
        let text = list_apps_text(&manager).unwrap();
        assert!(text.contains("habit-tracker"), "got: {text}");
        assert!(!text.contains("fare-grid"), "a widget leaked: {text}");
    }

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

    fn my_doc(local: &std::path::Path) -> String {
        write_knowhow_file(&local.join("my-doc.md"), "My Doc", "Body of my doc.");
        load_knowhow_impl(&dirs(local), None, &[], &json!({ "id": "my-doc" })).unwrap()
    }

    #[test]
    fn load_knowhow_returns_the_formatted_body_on_a_first_load() {
        let tmp = tempfile::tempdir().unwrap();
        let body = my_doc(&tmp.path().join("local"));
        assert!(body.contains("[KNOW-HOW: My Doc]"), "got: {}", body);
        assert!(body.contains("Body of my doc."), "got: {}", body);
    }

    #[test]
    fn load_knowhow_returns_the_not_found_sentinel_on_a_miss() {
        let tmp = tempfile::tempdir().unwrap();
        let local = tmp.path().join("local");
        std::fs::create_dir_all(&local).unwrap();
        let body =
            load_knowhow_impl(&dirs(&local), None, &[], &json!({ "id": "no-such-doc" })).unwrap();
        assert!(
            crate::core::knowhow::is_not_found_body(&body),
            "miss must return the not-found sentinel, got: {}",
            body
        );
    }

    #[test]
    fn knowhow_tools_reject_a_missing_or_empty_id() {
        let tmp = tempfile::tempdir().unwrap();
        let local = tmp.path().join("local");
        for args in [json!({}), json!({ "id": "" })] {
            let err = load_knowhow_impl(&dirs(&local), None, &[], &args).unwrap_err();
            assert!(err.to_string().contains("id is required"), "got: {}", err);
            let err = unload_knowhow_impl(&[], &args).unwrap_err();
            assert!(err.to_string().contains("id is required"), "got: {}", err);
        }
    }

    /// Keeping a doc the block already carries costs no body (ADR 0425).
    #[test]
    fn reloading_a_doc_the_block_holds_returns_the_kept_result() {
        let tmp = tempfile::tempdir().unwrap();
        let local = tmp.path().join("local");
        let body = my_doc(&local);
        let loaded = [LoadedKnowhow::loaded_turns_ago("my-doc", &body, 2)];
        let again = load_knowhow_impl(&dirs(&local), None, &loaded, &json!({ "id": "my-doc" }));
        assert_eq!(
            again.unwrap(),
            crate::engine::loaded_knowhow::kept_result("my-doc")
        );
    }

    /// The agent must never get no body while none is in front of it.
    #[test]
    fn reloading_returns_the_body_when_the_block_does_not_hold_it() {
        let tmp = tempfile::tempdir().unwrap();
        let local = tmp.path().join("local");
        let body = my_doc(&local);
        let cases = [
            (
                "loaded earlier in this turn",
                LoadedKnowhow::loaded_turns_ago("my-doc", &body, 0),
            ),
            (
                "changed on disk since",
                LoadedKnowhow::loaded_turns_ago("my-doc", "OLD", 1),
            ),
        ];
        for (case, doc) in cases {
            let again = load_knowhow_impl(&dirs(&local), None, &[doc], &json!({ "id": "my-doc" }));
            assert_eq!(again.unwrap(), body, "{case}");
        }
    }

    #[test]
    fn unload_knowhow_says_whether_the_doc_was_loaded() {
        let loaded = [LoadedKnowhow::loaded_turns_ago("my-doc", "BODY", 1)];
        let unloaded = unload_knowhow_impl(&loaded, &json!({ "id": "my-doc" })).unwrap();
        assert!(unloaded.starts_with("Unloaded `my-doc`"), "{unloaded}");
        assert!(unloaded.contains("from the next turn"), "{unloaded}");

        let absent = unload_knowhow_impl(&loaded, &json!({ "id": "other" })).unwrap();
        assert!(absent.contains("not loaded"), "{absent}");
    }
}
