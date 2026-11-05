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
                &self.workspace_path.join(crate::core::DATA_DIR),
                &self.thread_loaded_knowhow(thread_id).await?,
                args,
            ),
            "unload_knowhow" => {
                unload_knowhow_impl(&self.thread_loaded_knowhow(thread_id).await?, args)
            }
            _ => Err(format!("Unknown app tool: {}", name).into()),
        }
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
/// The full formatted body, or the not-found sentinel for a miss. An id that
/// names no doc falls back to an intent, so running an intent is loading it.
/// A doc wins an id clash. A doc this turn's block already holds, unchanged on
/// disk, gets the short kept result instead (ADR 0425). Either way the
/// recorded pair is what the next turn's fold reads, so nothing else is
/// written.
pub(crate) fn load_knowhow_impl(
    kh_dirs: &crate::core::knowhow::KnowhowDirs,
    system_dir: Option<&std::path::Path>,
    data_dir: &std::path::Path,
    loaded: &[crate::engine::loaded_knowhow::LoadedKnowhow],
    args: &serde_json::Value,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let id = required_knowhow_id(args)?;
    let body = crate::core::knowhow::load_one_knowhow_section(kh_dirs, system_dir, id)
        .or_else(|| load_intent_body(kh_dirs, system_dir, data_dir, id))
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

/// An intent as `load_knowhow` returns it: the intent body, then the docs its
/// `knowhow:` names, then, for an app intent, every doc in its app's knowhow.
fn load_intent_body(
    kh_dirs: &crate::core::knowhow::KnowhowDirs,
    system_dir: Option<&std::path::Path>,
    data_dir: &std::path::Path,
    id: &str,
) -> Option<String> {
    if id.starts_with(crate::core::knowhow::SYSTEM_KNOWHOW_PREFIX) {
        return None;
    }
    let intent = crate::core::IntentStore::load(data_dir, id)?;
    let mut body = format!(
        "[INTENT: {}]\n{}\n[END INTENT]",
        intent.name, intent.content
    );
    body.push_str(&crate::core::knowhow::load_knowhow_sections_merged(
        kh_dirs,
        system_dir,
        &intent.knowhow,
    ));
    if let Some((app_id, _)) = id.split_once('/') {
        let app_knowhow_dir = data_dir.join("apps").join(app_id).join("knowhow");
        body.push_str(&crate::core::knowhow::load_app_knowhow(&app_knowhow_dir));
    }
    Some(body)
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
        load_knowhow_impl(&dirs(local), None, local, &[], &json!({ "id": "my-doc" })).unwrap()
    }

    /// A workspace `data/` dir whose knowhow dirs resolve app-scoped ids.
    struct DataDir {
        _tmp: tempfile::TempDir,
        data: std::path::PathBuf,
    }

    impl DataDir {
        fn new() -> Self {
            let tmp = tempfile::tempdir().unwrap();
            let data = tmp.path().join("data");
            std::fs::create_dir_all(data.join("knowhow")).unwrap();
            Self { _tmp: tmp, data }
        }

        fn write(&self, rel: &str, text: &str) {
            let path = self.data.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }

        fn load(&self, id: &str) -> String {
            let dirs = KnowhowDirs {
                shared: None,
                local: self.data.join("knowhow"),
                apps: Some(self.data.join("apps")),
                triggers: Some(self.data.join("triggers")),
            };
            load_knowhow_impl(&dirs, None, &self.data, &[], &json!({ "id": id })).unwrap()
        }
    }

    #[test]
    fn load_knowhow_runs_an_app_intent_with_its_docs_and_the_apps_knowhow() {
        let ws = DataDir::new();
        ws.write(
            "apps/habit-tracker/intents/weekly-review.md",
            "---\nname: Weekly Review\nknowhow: review-style\n---\nReview the week.",
        );
        ws.write(
            "knowhow/review-style.md",
            "---\nname: Review Style\n---\nKeep it short.",
        );
        ws.write(
            "apps/habit-tracker/knowhow/streaks.md",
            "---\nname: Streaks\n---\nA streak breaks after one missed day.",
        );
        let body = ws.load("habit-tracker/weekly-review");
        assert!(body.starts_with("[INTENT: Weekly Review]"), "{body}");
        for part in [
            "Review the week.",
            "[KNOW-HOW: Review Style]",
            "Keep it short.",
            "[APP KNOW-HOW: Streaks]",
            "A streak breaks after one missed day.",
        ] {
            assert!(body.contains(part), "missing {part:?} in {body}");
        }
    }

    #[test]
    fn load_knowhow_runs_an_app_trigger_procedure() {
        let ws = DataDir::new();
        ws.write(
            "apps/habit-tracker/triggers/daily-check.md",
            "---\nname: Daily Check\n---\nCheck today's habits.",
        );
        let body = ws.load("habit-tracker/daily-check");
        assert!(body.starts_with("[INTENT: Daily Check]"), "{body}");
        assert!(body.contains("Check today's habits."), "{body}");
    }

    #[test]
    fn load_knowhow_runs_a_standalone_trigger_intent() {
        let ws = DataDir::new();
        ws.write(
            "triggers/morning-brief/morning-brief.md",
            "---\nname: Morning Brief\n---\nSummarise the calendar.",
        );
        let body = ws.load("morning-brief");
        assert!(body.starts_with("[INTENT: Morning Brief]"), "{body}");
        assert!(body.contains("Summarise the calendar."), "{body}");
        assert!(!body.contains("APP KNOW-HOW"), "{body}");
    }

    #[test]
    fn an_id_neither_knowhow_nor_intent_returns_the_not_found_sentinel() {
        let ws = DataDir::new();
        let body = ws.load("habit-tracker/no-such-intent");
        assert!(crate::core::knowhow::is_not_found_body(&body), "{body}");
    }

    /// An id that loads a doc today keeps loading that doc.
    #[test]
    fn a_knowhow_doc_wins_over_an_intent_with_the_same_id() {
        let ws = DataDir::new();
        ws.write("apps/x/knowhow/foo.md", "---\nname: Foo Doc\n---\nThe doc.");
        ws.write(
            "apps/x/intents/foo.md",
            "---\nname: Foo Intent\n---\nThe intent.",
        );
        let body = ws.load("x/foo");
        assert!(body.contains("The doc."), "{body}");
        assert!(!body.contains("The intent."), "{body}");
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
        let body = load_knowhow_impl(
            &dirs(&local),
            None,
            &local,
            &[],
            &json!({ "id": "no-such-doc" }),
        )
        .unwrap();
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
            let err = load_knowhow_impl(&dirs(&local), None, &local, &[], &args).unwrap_err();
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
        let again = load_knowhow_impl(
            &dirs(&local),
            None,
            &local,
            &loaded,
            &json!({ "id": "my-doc" }),
        );
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
            let again = load_knowhow_impl(
                &dirs(&local),
                None,
                &local,
                &[doc],
                &json!({ "id": "my-doc" }),
            );
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
