pub mod apps;
pub mod artifacts;
pub mod backup;
pub mod changes;
pub mod changes_projection;
pub mod credentials;
pub mod devices;
pub mod email;
pub mod events;
pub mod intents;
pub mod knowhow;
pub mod mcp_servers;
pub mod oauth;
pub mod pinned_apps;
pub mod plugins;
pub mod preferences;
pub mod repositories;
pub mod store;
pub mod system_knowhow;
pub mod thread_presence;
pub mod user_dir;

/// Get the database URL from the environment, with a default for local dev.
pub fn database_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://lucidos:lucidos@localhost:5432/lucidos".to_string())
}

/// Directory structure within workspace/data/
pub const DATA_DIR: &str = "data";
pub const ARTIFACTS_DIR: &str = "data/artifacts";
pub const APPS_DIR: &str = "data/apps";
pub const KNOWHOW_DIR: &str = "data/knowhow";

pub use apps::{App, AppManager, AppManifest};
pub use artifacts::{list_searchable_data_files, ArtifactChange, ArtifactManager};
pub use credentials::{AuthType, Credential, CredentialInfo, CredentialStore};
pub use devices::DeviceStore;
pub use email::{EmailAccount, EmailAccountInfo, EmailStore};
pub use intents::{Intent, IntentStore};
pub use knowhow::{Knowhow, KnowhowDirs, KnowhowStore, KnowhowSummary};
pub use oauth::{OAuthAccount, OAuthAccountInfo, OAuthStore};
pub use pinned_apps::{PinnedAppStore, PinnedAppUi};
pub use system_knowhow::{is_system_knowhow_path, SystemKnowhowStore};

/// Migrate legacy `prompts/` directories to `intents/` across the workspace.
///
/// Handles three levels:
/// - `data/prompts/` → individual files become standalone triggers in `data/triggers/`
/// - `data/apps/*/prompts/` → renamed to `data/apps/*/intents/`
/// - `data/apps/*/triggers/` left as-is (already correct)
pub fn migrate_prompts_to_intents(workspace: &std::path::Path) {
    log!("[Migration] Checking for legacy prompts/ directories...");
    let data_dir = workspace.join(DATA_DIR);

    // Top-level prompts become standalone triggers (each .md gets its own dir)
    let top_prompts = data_dir.join("prompts");
    if top_prompts.is_dir() {
        let triggers_dir = data_dir.join("triggers");
        if let Ok(entries) = std::fs::read_dir(&top_prompts) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("md") {
                    continue;
                }
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    let dest_dir = triggers_dir.join(stem);
                    if dest_dir.exists() {
                        continue;
                    }
                    if let Err(e) = std::fs::create_dir_all(&dest_dir) {
                        log!("[Migration] Failed to create {}: {}", dest_dir.display(), e);
                        continue;
                    }
                    let dest = dest_dir.join(entry.file_name());
                    if let Err(e) = std::fs::rename(&path, &dest) {
                        log!(
                            "[Migration] Failed to move {} → {}: {}",
                            path.display(),
                            dest.display(),
                            e
                        );
                    } else {
                        log!(
                            "[Migration] Moved prompt {} → {}",
                            path.display(),
                            dest.display()
                        );
                    }
                }
            }
        }
        if std::fs::read_dir(&top_prompts)
            .map(|mut d| d.next().is_none())
            .unwrap_or(false)
        {
            if let Err(e) = std::fs::remove_dir(&top_prompts) {
                log!("[Migration] Failed to remove empty data/prompts/: {}", e);
            } else {
                log!("[Migration] Removed empty data/prompts/");
            }
        }
    }

    // App-level prompts/ → intents/ (simple rename)
    let apps_dir = data_dir.join("apps");
    if apps_dir.is_dir() {
        if let Ok(entries) = std::fs::read_dir(&apps_dir) {
            for entry in entries.flatten() {
                let app_prompts = entry.path().join("prompts");
                let app_intents = entry.path().join("intents");
                if app_prompts.is_dir() && !app_intents.exists() {
                    if let Err(e) = std::fs::rename(&app_prompts, &app_intents) {
                        log!(
                            "[Migration] Failed to rename {}/prompts → intents: {}",
                            entry.file_name().to_string_lossy(),
                            e
                        );
                    } else {
                        log!(
                            "[Migration] Renamed {}/prompts → intents",
                            entry.file_name().to_string_lossy()
                        );
                    }
                }
            }
        }
    }
}
pub use events::EventRow;
pub use mcp_servers::{McpServer, McpServerStore};
pub use preferences::{
    PreferenceStore, DEFAULT_CHAT_MODEL, PREF_CHAT_MODEL, PREF_CHAT_REASONING_EFFORT,
    PREF_IMAGE_MODEL, PREF_MODEL_IMAGE_DESCRIPTION, PREF_MODEL_MEMORY, PREF_MODEL_TITLE,
    PREF_VERTEX_REGION,
};
pub use store::{
    ConversationMessage, ConversationSnapshot, EventStore, ResponseEvent, SessionMessage, Step,
    ThreadEventRow, ThreadInfo,
};
pub use thread_presence::ThreadPresenceStore;

/// Create a commit from the current index state.
/// Shared by ArtifactManager and AppManager.
pub fn commit_index(repo: &git2::Repository, message: &str) -> Result<String, git2::Error> {
    let mut index = repo.index()?;
    let tree_id = index.write_tree()?;
    let tree = repo.find_tree(tree_id)?;

    let sig = git2::Signature::now("Lucidos", "lucidos@local")?;

    let parent = repo.head().ok().and_then(|h| h.peel_to_commit().ok());
    let parents: Vec<&git2::Commit> = parent.as_ref().map(|p| vec![p]).unwrap_or_default();

    let commit_id = repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &parents)?;

    Ok(commit_id.to_string())
}

/// Check if a file extension indicates a binary (non-text) file.
/// Used to decide whether to read as bytes vs text, skip indexing, etc.
pub fn is_binary_extension(ext: &str) -> bool {
    matches!(
        ext,
        "pdf"
            | "png"
            | "jpg"
            | "jpeg"
            | "gif"
            | "webp"
            | "ico"
            | "zip"
            | "tar"
            | "gz"
            | "rar"
            | "7z"
            | "doc"
            | "docx"
            | "xls"
            | "xlsx"
            | "ppt"
            | "pptx"
            | "mp3"
            | "mp4"
            | "wav"
            | "avi"
            | "mov"
            | "exe"
            | "dll"
            | "so"
            | "dylib"
            | "woff"
            | "woff2"
            | "ttf"
            | "eot"
    )
}

/// Parse YAML frontmatter from a markdown file.
/// Extracts `name:` and a configurable list field (e.g., `knowhow:` or `domains:`).
/// Returns (name, list_values, body) or None if no valid frontmatter.
pub fn parse_md_frontmatter(text: &str, list_field: &str) -> Option<(String, Vec<String>, String)> {
    if !text.starts_with("---") {
        return None;
    }

    let parts: Vec<&str> = text.splitn(3, "---").collect();
    if parts.len() < 3 {
        return None;
    }

    let frontmatter = parts[1].trim();
    let body = parts[2].trim_start_matches('\n').to_string();

    let mut name = None;
    let mut list_values = Vec::new();
    let mut in_list = false;
    let list_prefix = format!("{}:", list_field);

    for line in frontmatter.lines() {
        if let Some(value) = line.strip_prefix("name:") {
            in_list = false;
            let v = value.trim().trim_matches('"');
            if !v.is_empty() {
                name = Some(v.to_string());
            }
        } else if let Some(value) = line.strip_prefix(&list_prefix) {
            let v = value.trim().trim_matches('"');
            if !v.is_empty() {
                list_values.push(v.to_string());
                in_list = false;
            } else {
                in_list = true;
            }
        } else if in_list {
            let trimmed = line.trim();
            if let Some(item) = trimmed.strip_prefix("- ") {
                list_values.push(item.trim().trim_matches('"').to_string());
            } else if !trimmed.is_empty() && !trimmed.starts_with('#') {
                in_list = false;
            }
        }
    }

    let name = name?;
    Some((name, list_values, body))
}

/// Strip null bytes from a string. PostgreSQL JSONB rejects \u0000.
pub fn sanitize_for_jsonb(s: &str) -> String {
    s.replace('\0', "")
}

/// Format a byte count as a human-readable size string (e.g. `1.5 KB`, `2.5 MB`).
pub fn format_byte_size(bytes: usize) -> String {
    if bytes >= 1_048_576 {
        format!("{:.1} MB", bytes as f64 / 1_048_576.0)
    } else if bytes >= 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{} bytes", bytes)
    }
}

#[cfg(test)]
mod format_byte_size_tests {
    use super::format_byte_size;

    #[test]
    fn formats_byte_sizes_across_thresholds() {
        assert_eq!(format_byte_size(0), "0 bytes");
        assert_eq!(format_byte_size(500), "500 bytes");
        assert_eq!(format_byte_size(1023), "1023 bytes");
        assert_eq!(format_byte_size(1024), "1.0 KB");
        assert_eq!(format_byte_size(1536), "1.5 KB");
        assert_eq!(format_byte_size(1_048_576), "1.0 MB");
        assert_eq!(format_byte_size(2_621_440), "2.5 MB");
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        // Find the last char boundary at or before `max` to avoid
        // panicking on multi-byte UTF-8 characters (e.g. æ, ø, å).
        let end = s.floor_char_boundary(max);
        format!("{}...", &s[..end])
    }
}

/// Summarize a `glob_files` / `grep_files` JSON result as "N items[, truncated]".
/// Falls back to a char count if the JSON can't be parsed (e.g. the handler
/// returned an "Error: ..." string).
fn describe_search_result(result: &str, items_key: &str) -> Option<String> {
    let parsed: serde_json::Value = serde_json::from_str(result).ok()?;
    let count = parsed.get(items_key)?.as_array()?.len();
    let truncated = parsed
        .get("truncated")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    Some(if truncated {
        format!("{} {}, truncated", count, items_key)
    } else {
        format!("{} {}", count, items_key)
    })
}

pub fn describe_tool_result(tool_name: &str, result: &str, success: bool) -> Option<String> {
    if !success {
        let msg = result.lines().next().unwrap_or(result);
        return Some(truncate(msg, 120));
    }
    match tool_name {
        "read_file" => Some(format!("{} chars", result.len())),
        "list_files" => Some(format!("{} items", result.lines().count())),
        "glob_files" => describe_search_result(result, "paths"),
        "grep_files" => describe_search_result(result, "matches"),
        "search_artifacts" => Some(format!("{} results", result.lines().count())),
        "run_python" | "run_bash" => result.lines().next().map(|l| truncate(l, 100)),
        "write_file" | "edit_file" | "create_app" | "execute_intent" => Some("Done".to_string()),
        "git_commit" => result.lines().next().map(|l| truncate(l, 80)),
        "git_diff" | "git_log" => Some(format!("{} lines", result.lines().count())),
        "http_request" | "proxy_request" => result.lines().next().map(|l| truncate(l, 80)),
        _ => {
            if result.len() <= 80 {
                Some(result.to_string())
            } else {
                Some(format!("{} chars", result.len()))
            }
        }
    }
}

/// Human-friendly description of a tool call, used for progress steps in both
/// live streaming (engine.rs) and session replay (store.rs).
pub fn describe_tool(name: &str, args: &serde_json::Value) -> String {
    match name {
        "list_files" => "Listing files in workspace...".to_string(),
        "glob_files" => format!(
            "Globbing {}...",
            args["pattern"].as_str().unwrap_or("pattern")
        ),
        "grep_files" => {
            let pat = args["pattern"].as_str().unwrap_or("pattern");
            if let Some(path_glob) = args.get("path_glob").and_then(|v| v.as_str()) {
                format!("Grepping {} in {}...", pat, path_glob)
            } else {
                format!("Grepping {}...", pat)
            }
        }
        "read_file" => format!("Reading {}...", args["path"].as_str().unwrap_or("file")),
        "write_file" => format!("Writing {}...", args["path"].as_str().unwrap_or("file")),
        "edit_file" => format!("Editing {}...", args["path"].as_str().unwrap_or("file")),
        "copy_file" => format!(
            "Copying {} → {}...",
            args["source"].as_str().unwrap_or("file"),
            args["destination"].as_str().unwrap_or("file")
        ),
        "delete_file" => format!("Deleting {}...", args["path"].as_str().unwrap_or("file")),
        "run_python" => {
            if let Some(path) = args["output_path"].as_str() {
                format!("Running Python → {}...", path)
            } else {
                "Running Python code...".to_string()
            }
        }
        "run_bash" => {
            let cmd = args["command"].as_str().unwrap_or("command");
            format!("Running: {}...", truncate(cmd, 60))
        }
        "http_request" => {
            let url = args["url"].as_str().unwrap_or("URL");
            if let Some(path) = args["temp_path"].as_str() {
                format!("Fetching {} → .lucidos/tmp/{}...", url, path)
            } else if let Some(path) = args["output_path"].as_str() {
                format!("Fetching {} → artifacts/{}...", url, path)
            } else {
                format!("Fetching {}...", url)
            }
        }
        "proxy_request" => {
            let name = args["name"].as_str().unwrap_or("proxy");
            let method = args["method"].as_str().unwrap_or("GET");
            let path = args["path"].as_str().unwrap_or("");
            format!("{} via {} proxy: {}...", method, name, path)
        }
        "import_file" => format!(
            "Importing {}...",
            args["source_path"].as_str().unwrap_or("file")
        ),
        "create_trigger" => format!(
            "Creating trigger '{}'...",
            args["name"].as_str().unwrap_or("trigger")
        ),
        "list_triggers" => "Listing triggers...".to_string(),
        "delete_trigger" => format!(
            "Deleting trigger {}...",
            args["trigger_id"].as_str().unwrap_or("trigger")
        ),
        "set_language" => format!(
            "Setting language to {}...",
            args["language"].as_str().unwrap_or("language")
        ),
        "set_timezone" => format!(
            "Setting timezone to {}...",
            args["timezone"].as_str().unwrap_or("timezone")
        ),
        "fetch_news" => format!(
            "Fetching news about '{}'...",
            args["topic"].as_str().unwrap_or("topic")
        ),
        "browser_open" => format!("Opening {}...", args["url"].as_str().unwrap_or("URL")),
        "browser_extract" => format!(
            "Extracting {} from {}...",
            args["format"].as_str().unwrap_or("content"),
            args["selector"].as_str().unwrap_or("elements")
        ),
        "browser_click" => format!(
            "Clicking {}...",
            args["selector"].as_str().unwrap_or("element")
        ),
        "browser_type" => format!(
            "Typing into {}...",
            args["selector"].as_str().unwrap_or("input")
        ),
        "browser_eval" => "Executing JavaScript...".to_string(),
        "browser_screenshot" => {
            let path = args["path"].as_str().unwrap_or("screenshot.png");
            if let Some(url) = args.get("url").and_then(|v| v.as_str()) {
                format!("Taking screenshot of {}...", url)
            } else {
                format!("Taking screenshot {}...", path)
            }
        }
        "browser_close" => "Closing browser...".to_string(),
        "web_search" => format!(
            "Searching for {}...",
            args["query"].as_str().unwrap_or("web")
        ),
        "request_credential" => format!(
            "Requesting {} credentials...",
            args["service_name"].as_str().unwrap_or("API")
        ),
        "connect_oauth_account" => format!(
            "Connecting {} account...",
            args["provider"].as_str().unwrap_or("OAuth")
        ),
        "create_app" => format!(
            "Creating app '{}'...",
            args["name"].as_str().unwrap_or("app")
        ),
        "list_apps" => "Listing apps...".to_string(),
        "list_intents" => "Listing intents...".to_string(),
        "list_knowhow" => "Listing know-how...".to_string(),
        "load_knowhow" => format!(
            "Loading know-how '{}'...",
            args["id"].as_str().unwrap_or("knowhow")
        ),
        "execute_intent" => format!(
            "Executing intent {}...",
            args["intent_id"].as_str().unwrap_or("intent")
        ),
        "refresh_file" => format!("Refreshing {}...", args["path"].as_str().unwrap_or("file")),
        "refresh_app" => format!(
            "Refreshing {}...",
            args.get("app_name")
                .or(args.get("app_id"))
                .and_then(|v| v.as_str())
                .unwrap_or("app")
        ),
        "capture_app" => format!(
            "Capturing {}...",
            args.get("app_name")
                .or(args.get("app_id"))
                .and_then(|v| v.as_str())
                .unwrap_or("app")
        ),
        "run_claude" => "Executing Claude Code...".to_string(),
        "configure_email" => format!(
            "Configuring email account '{}'...",
            args["name"].as_str().unwrap_or("email")
        ),
        "send_email" => format!(
            "Sending email to {}...",
            args["to"]
                .as_array()
                .map(|a| a
                    .iter()
                    .filter_map(|v| v.as_str())
                    .collect::<Vec<_>>()
                    .join(", "))
                .unwrap_or_default()
        ),
        "read_emails" => format!(
            "Reading emails from {}...",
            args.get("folder")
                .and_then(|v| v.as_str())
                .unwrap_or("INBOX")
        ),
        "read_email" => format!("Reading email #{}...", args["uid"].as_u64().unwrap_or(0)),
        "emit_event" => format!(
            "Emitting {} event...",
            args["event_type"].as_str().unwrap_or("event")
        ),
        "setup_mcp_server" => format!(
            "Setting up MCP server '{}'...",
            args["name"].as_str().unwrap_or("server")
        ),
        "list_mcp_servers" => "Listing MCP servers...".to_string(),
        "start_mcp_server" => format!(
            "Starting MCP server '{}'...",
            args["id"].as_str().unwrap_or("server")
        ),
        "stop_mcp_server" => format!(
            "Stopping MCP server '{}'...",
            args["id"].as_str().unwrap_or("server")
        ),
        "remove_mcp_server" => format!(
            "Removing MCP server '{}'...",
            args["id"].as_str().unwrap_or("server")
        ),
        "navigate_ui" => {
            let target = args["target"].as_str().unwrap_or("panel");
            match target {
                "app" | "app-ui" => format!(
                    "Opening {}...",
                    args.get("app_name")
                        .or(args.get("app_id"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("app")
                ),
                "file" => format!("Opening {}...", args["path"].as_str().unwrap_or("file")),
                "url" => format!("Opening {}...", args["url"].as_str().unwrap_or("URL")),
                _ => format!("Opening {}...", target),
            }
        }
        "update_trigger" => format!(
            "Updating trigger {}...",
            args["name"]
                .as_str()
                .or(args["trigger_id"].as_str())
                .unwrap_or("trigger")
        ),
        "send_notification" => format!(
            "Sending notification '{}'...",
            args["title"].as_str().unwrap_or("notification")
        ),
        "generate_image" => format!(
            "Generating image: {}...",
            truncate(args["prompt"].as_str().unwrap_or("image"), 50)
        ),
        "git_clone" => format!(
            "Cloning {}...",
            args["url"].as_str().unwrap_or("repository")
        ),
        "save_email_attachment" => format!(
            "Saving email attachment #{}...",
            args["attachment_index"].as_u64().unwrap_or(0)
        ),
        "run_thread" => format!(
            "Running thread: {}...",
            truncate(args["prompt"].as_str().unwrap_or("task"), 50)
        ),
        "correct_memory" => "Updating memory...".to_string(),
        "query_events" => format!(
            "Querying {} events...",
            args["event_type"].as_str().unwrap_or("all")
        ),
        "read_notifications" => "Reading notifications...".to_string(),
        "manage_repositories" => match args["action"].as_str() {
            Some("add") => format!(
                "Adding repository '{}'...",
                args["name"].as_str().unwrap_or("repo")
            ),
            Some("remove") => format!(
                "Removing repository '{}'...",
                args["name"].as_str().unwrap_or("repo")
            ),
            Some("list") => "Listing repositories...".to_string(),
            _ => "Managing repositories...".to_string(),
        },
        "browser_forget_login" => format!(
            "Forgetting login for {}...",
            args["domain"].as_str().unwrap_or("site")
        ),
        "browser_clear_data" => "Clearing browser data...".to_string(),
        "install_plugin" => format!(
            "Installing plugin from {}...",
            args["source"].as_str().unwrap_or("source")
        ),
        "check_plugin_updates" => match args.get("id").and_then(|v| v.as_str()) {
            Some(id) => format!("Checking plugin '{}' for updates...", id),
            None => "Checking installed plugins for updates...".to_string(),
        },
        "update_plugin" => format!(
            "Updating plugin '{}'...",
            args["id"].as_str().unwrap_or("plugin")
        ),
        "uninstall_plugin" => format!(
            "Uninstalling plugin '{}'...",
            args["id"].as_str().unwrap_or("plugin")
        ),
        "enable_push_notifications" => "Enabling push notifications...".to_string(),
        _ if name.starts_with("mcp__") => {
            let rest = &name[5..];
            if let Some(sep) = rest.find("__") {
                let tool_name = &rest[sep + 2..];
                format!("MCP: {}...", tool_name)
            } else {
                format!("MCP: {}...", name)
            }
        }
        _ => format!("Executing {}...", name),
    }
}

/// Human-friendly description of a Claude Code tool call.
pub fn describe_cc_tool(name: &str, args: &serde_json::Value) -> String {
    fn basename(p: &str) -> &str {
        p.rsplit('/').next().unwrap_or(p)
    }
    let str_arg = |key: &str| args.get(key).and_then(|v| v.as_str()).unwrap_or("");

    match name {
        "Read" => {
            let p = str_arg("file_path");
            if p.is_empty() {
                "Read file".into()
            } else {
                format!("Read {}", basename(p))
            }
        }
        "Edit" | "MultiEdit" => {
            let p = str_arg("file_path");
            if p.is_empty() {
                "Edit file".into()
            } else {
                format!("Edit {}", basename(p))
            }
        }
        "Write" => {
            let p = str_arg("file_path");
            if p.is_empty() {
                "Write file".into()
            } else {
                format!("Write {}", basename(p))
            }
        }
        "Glob" => {
            let pat = str_arg("pattern");
            if pat.is_empty() {
                "Find files".into()
            } else {
                format!("Find {}", pat)
            }
        }
        "Grep" => {
            let pat = str_arg("pattern");
            if pat.is_empty() {
                "Search code".into()
            } else {
                format!("Search '{}'", pat)
            }
        }
        "Bash" => {
            let cmd = str_arg("command");
            if cmd.is_empty() {
                "Run command".into()
            } else {
                let first_line = cmd
                    .lines()
                    .find(|l| !l.trim().is_empty())
                    .map(|l| l.trim())
                    .unwrap_or(cmd);
                format!("Run {}", truncate(first_line, 57))
            }
        }
        "WebFetch" => {
            let url = str_arg("url");
            if url.is_empty() {
                "Fetch URL".into()
            } else {
                let origin: String = url.splitn(4, '/').take(3).collect::<Vec<_>>().join("/");
                format!("Fetch {}", origin)
            }
        }
        "WebSearch" => {
            let q = str_arg("query");
            if q.is_empty() {
                "Web search".into()
            } else {
                format!("Search '{}'", q)
            }
        }
        "Agent" => {
            let desc = str_arg("description");
            if desc.is_empty() {
                "Run agent".into()
            } else {
                desc.to_string()
            }
        }
        "Skill" => {
            let s = str_arg("skill");
            if s.is_empty() {
                "Run skill".into()
            } else {
                format!("Run skill: {}", s)
            }
        }
        "NotebookEdit" => {
            let p = str_arg("file_path");
            if p.is_empty() {
                "Edit notebook".into()
            } else {
                format!("Edit {}", basename(p))
            }
        }
        _ => name.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_describe_tool_result_read_file() {
        let content = "hello world"; // 11 chars
        let result = describe_tool_result("read_file", content, true);
        assert_eq!(result, Some("11 chars".to_string()));
    }

    #[test]
    fn test_describe_tool_result_list_files() {
        let result = describe_tool_result("list_files", "file1\nfile2\nfile3", true);
        assert_eq!(result, Some("3 items".to_string()));
    }

    #[test]
    fn test_describe_tool_result_search_artifacts() {
        let result = describe_tool_result("search_artifacts", "match1\nmatch2", true);
        assert_eq!(result, Some("2 results".to_string()));
    }

    #[test]
    fn test_describe_tool_result_failure() {
        let result = describe_tool_result(
            "read_file",
            "File not found: /foo/bar.txt\nsome stack trace",
            false,
        );
        assert_eq!(result, Some("File not found: /foo/bar.txt".to_string()));
    }

    #[test]
    fn test_describe_tool_result_failure_truncates() {
        let long_err = "x".repeat(200);
        let result = describe_tool_result("any_tool", &long_err, false);
        let expected = format!("{}...", &"x".repeat(120));
        assert_eq!(result, Some(expected));
    }

    #[test]
    fn test_describe_tool_result_run_python() {
        let result = describe_tool_result("run_python", "42\nsome debug output", true);
        assert_eq!(result, Some("42".to_string()));
    }

    #[test]
    fn test_describe_tool_result_write_file() {
        assert_eq!(
            describe_tool_result("write_file", "OK", true),
            Some("Done".to_string())
        );
        assert_eq!(
            describe_tool_result("edit_file", "OK", true),
            Some("Done".to_string())
        );
        assert_eq!(
            describe_tool_result("create_app", "OK", true),
            Some("Done".to_string())
        );
    }

    #[test]
    fn test_describe_tool_result_git_commit() {
        let result = describe_tool_result("git_commit", "[main abc123] feat: add feature", true);
        assert_eq!(result, Some("[main abc123] feat: add feature".to_string()));
    }

    #[test]
    fn test_describe_tool_result_git_diff() {
        let diff = "--- a/file.rs\n+++ b/file.rs\n@@ -1,3 +1,4 @@\n+new line";
        let result = describe_tool_result("git_diff", diff, true);
        assert_eq!(result, Some("4 lines".to_string()));
    }

    #[test]
    fn test_describe_tool_result_unknown_short() {
        let result = describe_tool_result("custom_tool", "short result", true);
        assert_eq!(result, Some("short result".to_string()));
    }

    #[test]
    fn test_describe_tool_result_unknown_long() {
        let long_result = "x".repeat(100);
        let result = describe_tool_result("custom_tool", &long_result, true);
        assert_eq!(result, Some("100 chars".to_string()));
    }

    #[test]
    fn test_truncate_within_limit() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn test_truncate_exceeds_limit() {
        assert_eq!(truncate("hello world", 5), "hello...");
    }

    #[test]
    fn test_truncate_multibyte_char_boundary() {
        // Exact reproduction of the production panic:
        // byte index 120 is not a char boundary; it is inside 'æ' (bytes 119..121)
        let norwegian = "For å bestille et Buypass ID med høyt sikkerhetsnivå (nivå 4), må du vanligvis oppfylle visse krav, inkludert å være 13 år eller eldre";
        // This must not panic — truncate at 120 bytes falls inside 'æ' in 'være'
        let result = truncate(norwegian, 120);
        assert!(result.ends_with("..."));
        // Must not split a multi-byte char
        assert!(result.is_char_boundary(result.len()));
    }

    #[test]
    fn test_truncate_multibyte_various() {
        // 'æ' is 2 bytes, 'ø' is 2 bytes, 'å' is 2 bytes
        assert_eq!(truncate("æøå", 2), "æ..."); // cut at byte 2 = after 'æ'
        assert_eq!(truncate("æøå", 3), "æ..."); // byte 3 is inside 'ø', back up to after 'æ'
        assert_eq!(truncate("æøå", 1), "..."); // byte 1 is inside 'æ', back up to start
    }

    #[test]
    fn test_describe_tool_result_failure_with_norwegian() {
        // The actual crash: web_search result with Norwegian text treated as failure
        let norwegian_error = "For å bestille et Buypass ID med høyt sikkerhetsnivå (nivå 4), må du vanligvis oppfylle visse krav, inkludert å være 13 år eller eldre og registrert i det norske folkeregisteret";
        // Must not panic
        let result = describe_tool_result("web_search", norwegian_error, false);
        assert!(result.is_some());
    }

    // --- describe_cc_tool tests ---

    #[test]
    fn test_describe_cc_tool_read() {
        let args = serde_json::json!({"file_path": "/home/user/src/main.rs"});
        assert_eq!(describe_cc_tool("Read", &args), "Read main.rs");
        assert_eq!(
            describe_cc_tool("Read", &serde_json::json!({})),
            "Read file"
        );
    }

    #[test]
    fn test_describe_cc_tool_edit() {
        let args = serde_json::json!({"file_path": "/src/lib.rs"});
        assert_eq!(describe_cc_tool("Edit", &args), "Edit lib.rs");
        assert_eq!(describe_cc_tool("MultiEdit", &args), "Edit lib.rs");
    }

    #[test]
    fn test_describe_cc_tool_write() {
        let args = serde_json::json!({"file_path": "/tmp/output.txt"});
        assert_eq!(describe_cc_tool("Write", &args), "Write output.txt");
    }

    #[test]
    fn test_describe_cc_tool_glob() {
        let args = serde_json::json!({"pattern": "**/*.rs"});
        assert_eq!(describe_cc_tool("Glob", &args), "Find **/*.rs");
        assert_eq!(
            describe_cc_tool("Glob", &serde_json::json!({})),
            "Find files"
        );
    }

    #[test]
    fn test_describe_cc_tool_grep() {
        let args = serde_json::json!({"pattern": "TODO"});
        assert_eq!(describe_cc_tool("Grep", &args), "Search 'TODO'");
    }

    #[test]
    fn test_describe_cc_tool_bash() {
        let args = serde_json::json!({"command": "cargo test"});
        assert_eq!(describe_cc_tool("Bash", &args), "Run cargo test");
        // Multiline: picks first non-empty line
        let args2 = serde_json::json!({"command": "\n  git status\necho done"});
        assert_eq!(describe_cc_tool("Bash", &args2), "Run git status");
        assert_eq!(
            describe_cc_tool("Bash", &serde_json::json!({})),
            "Run command"
        );
    }

    #[test]
    fn test_describe_cc_tool_bash_truncates() {
        let long_cmd = "x".repeat(100);
        let args = serde_json::json!({"command": long_cmd});
        let result = describe_cc_tool("Bash", &args);
        assert!(result.starts_with("Run "));
        assert!(result.ends_with("..."));
    }

    #[test]
    fn test_describe_cc_tool_web_fetch() {
        let args = serde_json::json!({"url": "https://example.com/api/data?q=1"});
        assert_eq!(
            describe_cc_tool("WebFetch", &args),
            "Fetch https://example.com"
        );
    }

    #[test]
    fn test_describe_cc_tool_web_search() {
        let args = serde_json::json!({"query": "rust lifetimes"});
        assert_eq!(
            describe_cc_tool("WebSearch", &args),
            "Search 'rust lifetimes'"
        );
    }

    #[test]
    fn test_describe_cc_tool_agent() {
        let args = serde_json::json!({"description": "Find all TODO comments"});
        assert_eq!(describe_cc_tool("Agent", &args), "Find all TODO comments");
        assert_eq!(
            describe_cc_tool("Agent", &serde_json::json!({})),
            "Run agent"
        );
    }

    #[test]
    fn test_describe_cc_tool_skill() {
        let args = serde_json::json!({"skill": "commit"});
        assert_eq!(describe_cc_tool("Skill", &args), "Run skill: commit");
    }

    #[test]
    fn test_describe_cc_tool_unknown() {
        assert_eq!(
            describe_cc_tool("CustomTool", &serde_json::json!({})),
            "CustomTool"
        );
    }

    #[test]
    fn migrate_top_level_prompts_to_triggers() {
        let dir = std::env::temp_dir().join("lucidos_test_migrate_prompts");
        let _ = std::fs::remove_dir_all(&dir);
        let prompts_dir = dir.join("data/prompts");
        std::fs::create_dir_all(&prompts_dir).unwrap();
        std::fs::write(
            prompts_dir.join("sleep-reminder.md"),
            "---\nname: Sleep\n---\nGo to bed.",
        )
        .unwrap();
        std::fs::write(prompts_dir.join("notes.txt"), "not a markdown file").unwrap();

        migrate_prompts_to_intents(&dir);

        // .md moved into triggers/{stem}/
        assert!(dir
            .join("data/triggers/sleep-reminder/sleep-reminder.md")
            .exists());
        // non-.md left behind
        assert!(prompts_dir.join("notes.txt").exists());
        // prompts dir still exists (not empty due to notes.txt)
        assert!(prompts_dir.exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn migrate_app_prompts_to_intents() {
        let dir = std::env::temp_dir().join("lucidos_test_migrate_app_prompts");
        let _ = std::fs::remove_dir_all(&dir);
        let app_prompts = dir.join("data/apps/my-app/prompts");
        std::fs::create_dir_all(&app_prompts).unwrap();
        std::fs::write(
            app_prompts.join("workflow.md"),
            "---\nname: Workflow\n---\nDo things.",
        )
        .unwrap();

        migrate_prompts_to_intents(&dir);

        // prompts/ renamed to intents/
        assert!(dir.join("data/apps/my-app/intents/workflow.md").exists());
        assert!(!app_prompts.exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn migrate_skips_if_already_done() {
        let dir = std::env::temp_dir().join("lucidos_test_migrate_idempotent");
        let _ = std::fs::remove_dir_all(&dir);

        // Already-migrated state: intents/ exists, prompts/ also exists with a file
        let app_intents = dir.join("data/apps/my-app/intents");
        let app_prompts = dir.join("data/apps/my-app/prompts");
        std::fs::create_dir_all(&app_intents).unwrap();
        std::fs::create_dir_all(&app_prompts).unwrap();
        std::fs::write(app_prompts.join("old.md"), "old").unwrap();
        std::fs::write(app_intents.join("new.md"), "new").unwrap();

        migrate_prompts_to_intents(&dir);

        // prompts/ NOT overwritten — intents/ already existed
        assert!(app_prompts.exists());
        assert!(app_intents.join("new.md").exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn migrate_empty_prompts_dir_removed() {
        let dir = std::env::temp_dir().join("lucidos_test_migrate_empty_prompts");
        let _ = std::fs::remove_dir_all(&dir);
        let prompts_dir = dir.join("data/prompts");
        std::fs::create_dir_all(&prompts_dir).unwrap();
        std::fs::write(
            prompts_dir.join("only.md"),
            "---\nname: Only\n---\nContent.",
        )
        .unwrap();

        migrate_prompts_to_intents(&dir);

        // File moved, dir should be empty and removed
        assert!(!prompts_dir.exists());
        assert!(dir.join("data/triggers/only/only.md").exists());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
