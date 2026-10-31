//! The Lucidos Agent's file tools against every data prefix, driven through a
//! real agent turn by the mock's `MOCK_TOOL_CALL:` sentinel.
//!
//! The regression: `write_file` to `themes/<id>.json` landed at
//! `data/artifacts/themes/<id>.json`, unvalidated, because the tools kept their
//! own short copy of the data prefix list.

use crate::support::{
    base_url, db_url, e2e_font_slug, poll_thread_summary_by_marker, unique_marker, user_client,
    workspace_path, workspace_tree_lock,
};
use serde_json::json;

/// Run one agent turn that makes exactly one tool call, and return the text
/// of its result.
async fn call_tool(pool: &sqlx::PgPool, tool: &str, args: serde_json::Value) -> String {
    let client = user_client().await;
    let marker = unique_marker(&format!("api-file-tool-{tool}"));
    let resp = client
        .post(format!("{}/api/v1/chat/stream", base_url()))
        .json(&json!({
            "message": format!("{marker} MOCK_TOOL_CALL: {tool} {args}"),
            "mode": "human",
        }))
        .send()
        .await
        .expect("chat request failed");
    assert_eq!(resp.status(), 200, "chat/stream should accept the message");

    let thread_id = poll_thread_summary_by_marker(pool, &marker, 25)
        .await
        .thread_id
        .to_string();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        let result: Option<serde_json::Value> = sqlx::query_scalar(
            "SELECT payload FROM events WHERE aggregate = 'thread' AND aggregate_id = $1 \
               AND event_type = 'ToolResult' ORDER BY sequence LIMIT 1",
        )
        .bind(&thread_id)
        .fetch_optional(pool)
        .await
        .expect("DB query failed");
        if let Some(payload) = result {
            return payload["result"].as_str().unwrap_or_default().to_string();
        }
        assert!(
            std::time::Instant::now() < deadline,
            "{tool} never produced a result"
        );
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
}

async fn data_event_count(pool: &sqlx::PgPool, event_type: &str, path: &str) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM events WHERE event_type = $1 AND payload->'data'->>'path' = $2",
    )
    .bind(event_type)
    .bind(path)
    .fetch_one(pool)
    .await
    .expect("DB query failed")
}

/// The commits that ever touched `repo_path` in the workspace repo.
fn commits_touching(repo_path: &str) -> String {
    let out = std::process::Command::new("git")
        .args(["log", "--all", "--oneline", "--", repo_path])
        .current_dir(workspace_path())
        .output()
        .expect("git log should run");
    assert!(out.status.success(), "git log failed");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[tokio::test]
async fn a_theme_written_by_the_agent_lands_in_themes_and_cannot_be_edited_in_place() {
    let _tree = workspace_tree_lock().read().await;
    let pool = sqlx::PgPool::connect(&db_url()).await.unwrap();
    let id = format!("e2e-agent-theme-{}", uuid::Uuid::new_v4().simple());
    let rel = format!("themes/{id}.json");
    let theme = r##"{"name":"Harbour","dark":{"--accent":"#d4a650"}}"##;

    let written = call_tool(
        &pool,
        "write_file",
        json!({ "path": rel, "content": theme }),
    )
    .await;
    assert!(
        written.contains(&format!("CREATED: {rel}")),
        "got: {written}"
    );
    let on_disk = workspace_path().join("data").join(&rel);
    assert!(on_disk.exists(), "the theme must land at data/{rel}");
    assert!(
        !workspace_path().join("data/artifacts").join(&rel).exists(),
        "the theme must not be misrouted under artifacts/"
    );
    assert_eq!(
        data_event_count(&pool, "DataFileWritten", &rel).await,
        1,
        "the write announces itself, so an open picker refreshes"
    );

    let served: serde_json::Value = user_client()
        .await
        .get(format!("{}/api/v1/theme?id={id}", base_url()))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(served["source"], "workspace", "got: {served}");

    let edited = call_tool(
        &pool,
        "edit_file",
        json!({ "path": rel, "json_path": "dark.--accent", "new_value": "#000000" }),
    )
    .await;
    assert!(edited.starts_with("Error:"), "got: {edited}");
    assert!(edited.contains("write_file"), "names the route: {edited}");
    assert_eq!(std::fs::read_to_string(&on_disk).unwrap(), theme);

    let deleted = call_tool(&pool, "delete_file", json!({ "path": rel })).await;
    assert!(
        deleted.contains(&format!("DELETED: {rel}")),
        "got: {deleted}"
    );
    assert!(!on_disk.exists());
    assert_eq!(data_event_count(&pool, "DataFileDeleted", &rel).await, 1);
}

#[tokio::test]
async fn a_broken_theme_written_by_the_agent_is_refused_before_disk_or_git() {
    let _tree = workspace_tree_lock().read().await;
    let pool = sqlx::PgPool::connect(&db_url()).await.unwrap();
    let id = format!("e2e-agent-theme-{}", uuid::Uuid::new_v4().simple());
    let rel = format!("themes/{id}.json");

    let refused = call_tool(
        &pool,
        "write_file",
        json!({
            "path": rel,
            "content": r#"{"name":"Leaky","dark":{"--bg-primary":"url(https://example.com)"}}"#,
        }),
    )
    .await;
    assert!(refused.starts_with("Error:"), "got: {refused}");
    assert!(
        refused.contains("url("),
        "carries the engine's reason: {refused}"
    );
    assert!(!workspace_path().join("data").join(&rel).exists());
    assert!(!workspace_path().join("data/artifacts").join(&rel).exists());
    assert_eq!(commits_touching(&format!("data/{rel}")), "");
    assert_eq!(data_event_count(&pool, "DataFileWritten", &rel).await, 0);
}

/// Every file tool announces a change outside `artifacts/`, once. An open
/// preview of that file refreshes on the announcement alone, since the live
/// tool call no longer carries the path it wrote.
#[tokio::test]
async fn every_file_tool_announces_a_knowhow_change_once() {
    let _tree = workspace_tree_lock().read().await;
    let pool = sqlx::PgPool::connect(&db_url()).await.unwrap();
    let id = uuid::Uuid::new_v4().simple();
    let rel = format!("knowhow/e2e/announce-{id}.md");
    let copy = format!("knowhow/e2e/announce-{id}-copy.md");

    let written = call_tool(
        &pool,
        "write_file",
        json!({ "path": rel, "content": "# Notes\nfirst draft\n" }),
    )
    .await;
    assert!(
        written.contains(&format!("CREATED: {rel}")),
        "got: {written}"
    );
    assert_eq!(data_event_count(&pool, "DataFileWritten", &rel).await, 1);

    let edited = call_tool(
        &pool,
        "edit_file",
        json!({ "path": rel, "old_string": "first draft", "new_string": "second draft" }),
    )
    .await;
    assert!(!edited.starts_with("Error:"), "got: {edited}");
    assert_eq!(data_event_count(&pool, "DataFileEdited", &rel).await, 1);

    let copied = call_tool(
        &pool,
        "copy_file",
        json!({ "source": rel, "destination": copy }),
    )
    .await;
    assert!(!copied.starts_with("Error:"), "got: {copied}");
    assert_eq!(data_event_count(&pool, "DataFileWritten", &copy).await, 1);

    for path in [&rel, &copy] {
        let deleted = call_tool(&pool, "delete_file", json!({ "path": path })).await;
        assert!(
            deleted.contains(&format!("DELETED: {path}")),
            "got: {deleted}"
        );
        assert_eq!(data_event_count(&pool, "DataFileDeleted", path).await, 1);
    }
}

/// An `artifacts/` write keeps its own `Artifact*` event and gains no
/// `DataFile*`, so the Files list and the preview see one announcement.
#[tokio::test]
async fn an_artifact_written_by_the_agent_announces_no_data_file_event() {
    let _tree = workspace_tree_lock().read().await;
    let pool = sqlx::PgPool::connect(&db_url()).await.unwrap();
    let rel = format!(
        "artifacts/e2e/announce-{}.md",
        uuid::Uuid::new_v4().simple()
    );

    let written = call_tool(
        &pool,
        "write_file",
        json!({ "path": rel, "content": "x\n" }),
    )
    .await;
    assert!(
        written.contains(&format!("CREATED: {rel}")),
        "got: {written}"
    );
    assert_eq!(data_event_count(&pool, "DataFileWritten", &rel).await, 0);

    let deleted = call_tool(&pool, "delete_file", json!({ "path": rel })).await;
    assert!(
        deleted.contains(&format!("DELETED: {rel}")),
        "got: {deleted}"
    );
    assert_eq!(data_event_count(&pool, "DataFileDeleted", &rel).await, 0);
}

/// A workspace font's manifest is checked like a theme: refused before disk or
/// git when invalid, announced when written, and never edited in place.
#[tokio::test]
async fn a_font_manifest_written_by_the_agent_is_checked_and_cannot_be_edited_in_place() {
    let _tree = workspace_tree_lock().read().await;
    let pool = sqlx::PgPool::connect(&db_url()).await.unwrap();
    let slug = e2e_font_slug("e2e-agent-font");
    let rel = format!("fonts/{slug}/font.json");

    let refused = call_tool(
        &pool,
        "write_file",
        json!({
            "path": rel,
            "content": r#"{"label":"Leaky","group":"sans","faces":[{"file":"https://cdn.example.com/a.woff2"}]}"#,
        }),
    )
    .await;
    assert!(refused.starts_with("Error:"), "got: {refused}");
    assert!(!workspace_path().join("data").join(&rel).exists());
    assert!(!workspace_path().join("data/artifacts").join(&rel).exists());
    assert_eq!(commits_touching(&format!("data/{rel}")), "");

    let manifest = r#"{"label":"Brand","group":"sans","faces":[{"file":"a.woff2"}]}"#;
    let written = call_tool(
        &pool,
        "write_file",
        json!({ "path": rel, "content": manifest }),
    )
    .await;
    assert!(
        written.contains(&format!("CREATED: {rel}")),
        "got: {written}"
    );
    assert_eq!(
        data_event_count(&pool, "DataFileWritten", &rel).await,
        1,
        "the write announces itself, so an open font list refreshes"
    );

    let edited = call_tool(
        &pool,
        "edit_file",
        json!({ "path": rel, "json_path": "label", "new_value": "Other" }),
    )
    .await;
    assert!(edited.starts_with("Error:"), "got: {edited}");
    assert!(edited.contains("write_file"), "names the route: {edited}");

    let deleted = call_tool(&pool, "delete_file", json!({ "path": rel })).await;
    assert!(
        deleted.contains(&format!("DELETED: {rel}")),
        "got: {deleted}"
    );
    assert_eq!(data_event_count(&pool, "DataFileDeleted", &rel).await, 1);
}

/// Removes the symlinks a test planted, even when an assertion fails, so the
/// run leaves the workspace tree clean.
struct PlantedLinks(Vec<std::path::PathBuf>);

impl Drop for PlantedLinks {
    fn drop(&mut self) {
        for link in &self.0 {
            let _ = std::fs::remove_file(link);
        }
    }
}

/// No tool writes through a symlink to a file outside its tree: not a linked
/// leaf, and not a new file under a linked directory (ADR 0051).
#[cfg(unix)]
#[tokio::test]
async fn no_file_tool_writes_through_a_symlink_out_of_its_tree() {
    use std::os::unix::fs::symlink;

    let _tree = workspace_tree_lock().read().await;
    let pool = sqlx::PgPool::connect(&db_url()).await.unwrap();
    let id = uuid::Uuid::new_v4().simple();
    let outside = tempfile::tempdir().unwrap();
    let target = outside.path().join("f.md");
    std::fs::write(&target, "outside\n").unwrap();

    let artifacts = workspace_path().join("data/artifacts");
    let tmp = workspace_path().join(".lucidos/tmp");
    std::fs::create_dir_all(&tmp).unwrap();
    let dir = format!("link-{id}-dir");
    let leaf = format!("link-{id}-leaf.md");
    let source = format!("link-{id}-source.md");
    std::fs::write(tmp.join(&source), "source\n").unwrap();
    let _planted = PlantedLinks(vec![
        artifacts.join(&dir),
        artifacts.join(&leaf),
        tmp.join(&dir),
        tmp.join(&source),
    ]);
    symlink(outside.path(), artifacts.join(&dir)).unwrap();
    symlink(&target, artifacts.join(&leaf)).unwrap();
    symlink(outside.path(), tmp.join(&dir)).unwrap();

    let leaf = format!("artifacts/{leaf}");
    let under_dir = format!("artifacts/{dir}/new.md");
    let source = format!(".lucidos/tmp/{source}");
    let calls = [
        ("write_file", json!({ "path": leaf, "content": "x" })),
        ("write_file", json!({ "path": under_dir, "content": "x" })),
        (
            "edit_file",
            json!({ "path": leaf, "old_string": "outside", "new_string": "x" }),
        ),
        (
            "copy_file",
            json!({ "source": source, "destination": leaf }),
        ),
        (
            "copy_file",
            json!({ "source": source, "destination": under_dir }),
        ),
        (
            "http_request",
            json!({ "url": "https://example.com/", "output_path": leaf }),
        ),
        (
            "http_request",
            json!({ "url": "https://example.com/", "temp_path": format!("{dir}/new.md") }),
        ),
    ];
    for (tool, args) in calls {
        let result = call_tool(&pool, tool, args.clone()).await;
        assert!(
            result.starts_with("Error:") && result.contains("symlink"),
            "{tool} {args}: {result}"
        );
    }
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "outside\n");
    assert!(!outside.path().join("new.md").exists());
}

/// A `temp_path` naming a directory is refused before the request goes out,
/// with a reason about the argument rather than the file tools.
#[tokio::test]
async fn a_temp_path_naming_a_directory_is_refused_for_what_it_is() {
    let pool = sqlx::PgPool::connect(&db_url()).await.unwrap();
    for temp_path in ["", "downloads/"] {
        let result = call_tool(
            &pool,
            "http_request",
            json!({ "url": "https://example.com/", "temp_path": temp_path }),
        )
        .await;
        assert!(
            result.starts_with("Error: temp_path must name a file"),
            "{temp_path:?}: {result}"
        );
    }
}

/// An app file the agent writes is linted. The findings ride back in the tool
/// result, and the write lands exactly as asked.
#[tokio::test]
async fn an_app_file_with_lint_findings_is_written_and_its_result_names_them() {
    let _tree = workspace_tree_lock().read().await;
    let pool = sqlx::PgPool::connect(&db_url()).await.unwrap();
    let dir = format!("apps/e2e-app-lint-{}", uuid::Uuid::new_v4().simple());
    let rel = format!("{dir}/index.html");
    let html = "<style>\nbutton:hover { background: grey; }\n</style>\n\
                <button class=\"action-btn-secondary\">Sort</button>\n";

    let written = call_tool(&pool, "write_file", json!({ "path": rel, "content": html })).await;
    assert!(
        written.starts_with(&format!("[ACTION COMPLETED] CREATED: {rel}")),
        "the write is not refused: {written}"
    );
    assert!(
        written.contains(&format!("[APP LINT] {rel}: 2 problem(s)")),
        "got: {written}"
    );
    assert!(written.contains("line 2: a `:hover` rule outside `@media (hover: hover)`"));
    assert!(written.contains("line 4: `action-btn-secondary` without the base `action-btn`"));
    let on_disk = workspace_path().join("data").join(&rel);
    assert_eq!(std::fs::read_to_string(&on_disk).unwrap(), html);

    let edited = call_tool(
        &pool,
        "edit_file",
        json!({
            "path": rel,
            "old_string": "class=\"action-btn-secondary\"",
            "new_string": "class=\"action-btn action-btn-secondary\"",
        }),
    )
    .await;
    assert!(
        edited.starts_with(&format!("[ACTION COMPLETED] UPDATED: {rel}")),
        "got: {edited}"
    );
    assert!(
        edited.contains(&format!("[APP LINT] {rel}: 1 problem(s)")),
        "the edit is linted from the file it left: {edited}"
    );
    assert!(!edited.contains("without the base"), "got: {edited}");

    let deleted = call_tool(&pool, "delete_file", json!({ "path": rel })).await;
    assert!(
        deleted.contains(&format!("DELETED: {rel}")),
        "got: {deleted}"
    );
    // Git tracks no directories, so the emptied app folder is no tree change.
    // The delete may prune it already; one left behind must be empty.
    let app_dir = workspace_path().join("data").join(&dir);
    if app_dir.exists() {
        std::fs::remove_dir(&app_dir).unwrap();
    }
}
