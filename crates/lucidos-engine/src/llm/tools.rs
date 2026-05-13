use crate::engine::tools::bash::{
    BG_DEFAULT_TIMEOUT_SECS, BG_MAX_TIMEOUT_SECS, DEFAULT_TIMEOUT_SECS, MAX_TIMEOUT_SECS,
};
use crate::llm::provider::ToolDefinition;
use crate::llm::tool_names as tn;
use serde_json::json;

/// Shared JSON schema for the `cron` tool parameter.
/// When `nullable`, adds `null` as a valid type (for update_trigger to clear the schedule).
fn cron_schema(nullable: bool) -> serde_json::Value {
    let mut variants = vec![
        json!({ "type": "string" }),
        json!({ "type": "array", "items": { "type": "string" }, "minItems": 1 }),
    ];
    let desc = if nullable {
        variants.push(json!({ "type": "null" }));
        "Cron schedule(s) with 6 fields in USER'S LOCAL TIME: second minute hour day-of-month month day-of-week. Pass a single string for one schedule, an array of strings for multiple, or null to remove the cron schedule. Example: '0 0 8 * * *' for 8am daily."
    } else {
        "Cron schedule(s) with 6 fields in USER'S LOCAL TIME: second minute hour day-of-month month day-of-week. Pass a single string for one schedule, or an array of strings for multiple schedules (e.g., fire at both 8am and 8pm). Example: '0 0 8 * * *' for 8am daily, or ['0 0 8 * * *', '0 0 20 * * *'] for 8am and 8pm daily."
    };
    json!({ "description": desc, "oneOf": variants })
}

/// General-purpose notification tool, available in all contexts.
pub fn get_notification_tool() -> ToolDefinition {
    ToolDefinition {
        name: tn::SEND_NOTIFICATION.to_string(),
        description: "Send a push notification to the user's devices. Use this when you have something important to tell the user — task results, reminders, alerts, etc. Only set `app_id` when the notification is a direct call to action inside that specific app — i.e. tapping it opens that app to act on the thing the notification mentions (e.g. a habit-tracker app sending \"check in for today\" sets `app_id` to the habit-tracker id). Do NOT set `app_id` for general reminders, status messages, screen-time / bedtime nudges, or plain informational text. When in doubt, leave `app_id` unset.".to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "title": {
                    "type": "string",
                    "description": "Short notification title (in the user's language)."
                },
                "message": {
                    "type": "string",
                    "description": "Notification body text (in the user's language)."
                },
                "app_id": {
                    "type": "string",
                    "description": "Optional id of an app from the Available Apps list. Set this only when the notification is a direct call to action inside that app — tapping it opens the app to act. Omit for general reminders, status messages, screen-time / bedtime nudges, or plain informational text."
                }
            },
            "required": ["title", "message"]
        }),
    }
}

/// Tool for reading notifications (past notifications, unread count, etc.).
pub fn get_read_notifications_tool() -> ToolDefinition {
    ToolDefinition {
        name: tn::READ_NOTIFICATIONS.to_string(),
        description: "Read notifications from the notification inbox. Use this to check what notifications have been sent (including task error notifications), see unread counts, or review notification history.".to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "filter": {
                    "type": "string",
                    "enum": ["unread", "all"],
                    "description": "Filter: 'unread' for only unread notifications, 'all' for all. Default: 'unread'."
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of notifications to return (1-50, default 20)."
                }
            }
        }),
    }
}

/// Tool for navigating the Lucidos UI to a specific panel, app, or file.
pub fn get_navigate_ui_tool() -> ToolDefinition {
    ToolDefinition {
        name: tn::NAVIGATE_UI.to_string(),
        description: "Navigate the Lucidos UI to a specific panel, app, app UI, file, thread, or form. Use this when the user asks to open, show, or go to something in the interface — e.g. \"open settings\", \"show me the habit tracker\", \"go to triggers\", \"open my budget file\", \"go to that thread\".".to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "target": {
                    "type": "string",
                    "enum": ["files", "apps", "triggers", "settings", "changes", "notifications", "app", "app-ui", "file", "thread", "new-app", "new-trigger", "url"],
                    "description": "Navigation target. Use 'files', 'apps', 'triggers', 'settings', 'changes', 'notifications' for panels. Use 'app' to open an app by ID. Use 'app-ui' to open an app's UI. Use 'file' to preview a file. Use 'thread' to focus a thread by ID. Use 'new-app' or 'new-trigger' to open the creation form. Use 'url' to open a URL in the internal browser panel."
                },
                "settings_view": {
                    "type": "string",
                    "enum": ["devices", "accounts", "backup", "memory"],
                    "description": "Settings subview to open. Only used when target is 'settings'."
                },
                "app_id": {
                    "type": "string",
                    "description": "App ID to open. Required when target is 'app' or 'app-ui'."
                },
                "file_path": {
                    "type": "string",
                    "description": "File path to preview, including the directory prefix (e.g. 'artifacts/research/notes.md', 'knowhow/domain/guide.md'). Required when target is 'file'."
                },
                "id": {
                    "type": "string",
                    "description": "ID of the entity to navigate to. Required when target is 'thread' or 'trigger'."
                },
                "url": {
                    "type": "string",
                    "description": "URL to open in the internal browser panel. Required when target is 'url'."
                }
            },
            "required": ["target"]
        }),
    }
}

/// Tool for managing registered external git repositories for Claude Code sessions.
pub fn get_manage_repositories_tool() -> ToolDefinition {
    ToolDefinition {
        name: tn::MANAGE_REPOSITORIES.to_string(),
        description: "Manage registered external git repositories for Claude Code sessions. Users can register local repos so Claude Code can work on them.".to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["add", "list", "remove"],
                    "description": "Action to perform: 'add' registers a repo, 'list' shows all repos, 'remove' unregisters a repo."
                },
                "name": {
                    "type": "string",
                    "description": "Repository display name (required for 'add', used to find repo for 'remove')."
                },
                "path": {
                    "type": "string",
                    "description": "Absolute path to the git repository on disk (required for 'add'). Supports ~/."
                },
                "description": {
                    "type": "string",
                    "description": "Optional description of the repository (for 'add')."
                }
            },
            "required": ["action"]
        }),
    }
}

pub fn get_default_tools() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: tn::READ_FILE.to_string(),
            description: "Read the contents of a file in the workspace. Supports text files and images (.png, .jpg, .jpeg, .gif, .webp — displayed visually). SVGs are returned as text. Max image size: 5 MB. Text files >50KB are returned in chunks: the response ends with the exact `offset=` to pass on the next call to continue reading. Don't re-read content you've already seen.\n\nReads inside .zip and .lucidos-plugin archives transparently — point `path` past the archive segment, e.g. `artifacts/plugins/foo.lucidos-plugin/apps/x/index.html`. To inspect a small section of a long file use `start_line` + `line_count` instead of pulling the whole thing or shelling out to run_python.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Relative path under data/ (e.g., artifacts/notes.md, apps/my-app/ui/main/index.html). May traverse a .zip or .lucidos-plugin segment to read an entry inside the archive (e.g. artifacts/plugins/foo.lucidos-plugin/apps/x/index.html)."
                    },
                    "offset": {
                        "type": "integer",
                        "description": "Byte offset to start reading from (default 0). Use the `offset=` value from the previous truncated response to read the next chunk. Text files only. Ignored when `start_line` is set."
                    },
                    "start_line": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "1-based line to start reading from. Combine with `line_count` to read a specific range without pulling the whole file. Text files only."
                    },
                    "line_count": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Number of lines to read starting from `start_line` (default: read to end). Use small values (e.g. 5–50) when you only need to inspect a known location."
                    }
                },
                "required": ["path"]
            }),
        },
        ToolDefinition {
            name: tn::WRITE_FILE.to_string(),
            description: "Create or update a file in the workspace. For NEW files or FULL rewrites only. NEVER use when edit_file can do the job — rewriting introduces subtle regressions.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Relative path under data/ (e.g., artifacts/notes.md, apps/my-app/knowhow/api-reference.md, knowhow/domain.md). For knowhow/ paths, if the file already exists in shared (~/.lucidos/knowhow/) but not locally, the write updates the shared copy."
                    },
                    "content": {
                        "type": "string",
                        "description": "Content to write to the file"
                    },
                    "message": {
                        "type": "string",
                        "description": "Semantic commit message describing the intent (e.g., 'Add dark theme styles', 'Create job listing card layout')"
                    }
                },
                "required": ["path", "content"]
            }),
        },
        ToolDefinition {
            name: tn::EDIT_FILE.to_string(),
            description: "Make a targeted edit to an existing file. Two modes:\n\
                1. Text mode: old_string + new_string — search-and-replace for text files\n\
                2. JSON mode: json_path + new_value — surgical edit at a specific path for JSON files (.json, .slides, etc.)\n\
                JSON mode handles parsing, navigation, and re-serialization automatically. Use it for .slides and other JSON files to avoid escape and matching issues.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Relative path under data/ (e.g., artifacts/notes.md, artifacts/deck.slides, apps/my-app/knowhow/api-reference.md, knowhow/domain.md). For knowhow/ paths, if the file already exists in shared (~/.lucidos/knowhow/) but not locally, the edit updates the shared copy."
                    },
                    "old_string": {
                        "type": "string",
                        "description": "Text mode: the exact text to find in the file"
                    },
                    "new_string": {
                        "type": "string",
                        "description": "Text mode: the text to replace it with (must be different from old_string)"
                    },
                    "replace_all": {
                        "type": "boolean",
                        "description": "Text mode: replace all occurrences instead of just the first (default: false)"
                    },
                    "json_path": {
                        "type": "string",
                        "description": "JSON mode: path to the target value. Supports dot notation (`metadata.author.name`), array indices (`sections[1]`), quoted keys for non-identifier chars like dates or slugs (`dailyLog[\"2026-05-04\"]` or `dailyLog['2026-05-04']`), the JSONPath root marker (`$.streak`), and raw JSON Pointers (`/sections/1/title`). Mix freely — e.g. `habits[0].dailyLog[\"2026-05-04\"]`."
                    },
                    "new_value": {
                        "description": "JSON mode: the replacement value — can be any JSON type (string, number, object, array, boolean, null)"
                    },
                    "message": {
                        "type": "string",
                        "description": "Semantic commit message describing the intent (e.g., 'Fix filter button spacing', 'Update slide 3 title')"
                    }
                },
                "required": ["path"]
            }),
        },
        ToolDefinition {
            name: tn::RUN_PYTHON.to_string(),
            description: "Create and process data files. Write to data/artifacts/ with open(). \
                Changes are staged and committed atomically — apps see old files until commit. \
                Print freely for debugging (stdout is never captured as file content). \
                Packages are auto-installed via the packages parameter. \
                Environment variables injected automatically: CRED_{NAME} for api_key/bearer/basic credentials, \
                CRED_{NAME}_USERNAME and CRED_{NAME}_PASSWORD for password credentials, \
                OAUTH_{PROVIDER}_ACCESS_TOKEN and OAUTH_{PROVIDER}_EMAIL for connected OAuth accounts \
                (tokens are auto-refreshed). Provider/name is uppercased with hyphens/spaces/dots replaced by underscores.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "code": {
                        "type": "string",
                        "description": "Python code. Write files with open('data/artifacts/name.csv', 'w'). Print freely for debugging."
                    },
                    "packages": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Python packages to install before running (e.g., [\"pandas\", \"requests\"]). Installed into isolated venv."
                    },
                    "commit_message": {
                        "type": "string",
                        "description": "Git commit message for the files this script creates/updates (e.g., \"Import Oura sleep data\")."
                    }
                },
                "required": ["code"]
            }),
        },
        ToolDefinition {
            name: tn::RUN_BASH.to_string(),
            description: format!(
                "Run system commands (curl, wget, git, jq, system tools). \
                Do NOT use for creating or editing files in data/ — use run_python for that. \
                If you need to commit changes made by bash, use git add + git commit with a descriptive message. \
                Stdout and stderr returned (truncated to 100KB). Timeout: {DEFAULT_TIMEOUT_SECS}s default, {MAX_TIMEOUT_SECS}s max. \
                Bump `timeout_secs` to {MAX_TIMEOUT_SECS} for cargo/npm builds, full-repo greps, large `git log`/`git blame`, \
                or any command you expect to run >30s — the {DEFAULT_TIMEOUT_SECS}s default will kill them mid-stream and waste a turn retrying. \
                Environment variables injected automatically: CRED_{{NAME}} for api_key/bearer/basic credentials, \
                CRED_{{NAME}}_USERNAME and CRED_{{NAME}}_PASSWORD for password credentials, \
                OAUTH_{{PROVIDER}}_ACCESS_TOKEN and OAUTH_{{PROVIDER}}_EMAIL for connected OAuth accounts \
                (tokens are auto-refreshed). Provider/name is uppercased with hyphens/spaces/dots replaced by underscores."
            ),
            parameters: json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "Shell command to execute (passed to /bin/sh -c). NOT for writing to data/ — use run_python."
                    },
                    "timeout_secs": {
                        "type": "integer",
                        "description": format!("Timeout in seconds (default: {DEFAULT_TIMEOUT_SECS}, max: {MAX_TIMEOUT_SECS})")
                    }
                },
                "required": ["command"]
            }),
        },
        ToolDefinition {
            name: tn::RUN_BASH_BACKGROUND.to_string(),
            description: format!(
                "Spawn a long-running shell command in the background and return a task_id immediately. \
                Use whenever the command may exceed run_bash's {MAX_TIMEOUT_SECS}s sync ceiling — long HTTP polls, builds, scrapers, npm/cargo installs, repo-wide migrations. \
                Drain output incrementally with bash_output(task_id); cancel with bash_kill(task_id). \
                Default timeout {BG_DEFAULT_TIMEOUT_SECS}s, max {BG_MAX_TIMEOUT_SECS}s — the child is killed when the timeout fires. \
                NEVER hand-roll `for i in range(...): time.sleep(...)` polling loops in run_python — use this trio instead. \
                Same env-var injection as run_bash (CRED_*, OAUTH_*)."
            ),
            parameters: json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "Shell command to execute (passed to /bin/sh -c)."
                    },
                    "timeout_secs": {
                        "type": "integer",
                        "description": format!("Wall-clock seconds before the watchdog kills the child (default: {BG_DEFAULT_TIMEOUT_SECS}, max: {BG_MAX_TIMEOUT_SECS}).")
                    }
                },
                "required": ["command"]
            }),
        },
        ToolDefinition {
            name: tn::BASH_OUTPUT.to_string(),
            description: "Fetch incremental stdout/stderr from a background task created by run_bash_background. \
                Returns only output emitted since the previous bash_output call (drain semantics) — call repeatedly to follow a stream. \
                When the task finishes, returns the final tail with finished=true: STOP polling at this point — subsequent calls fall back to the event store and return the FULL final stdout/stderr again, which wastes context. \
                exit_code is null while the task is running, after a watchdog timeout (timed_out=true), and after bash_kill (killed=true)."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "task_id": {
                        "type": "string",
                        "description": "task_id returned by run_bash_background."
                    }
                },
                "required": ["task_id"]
            }),
        },
        ToolDefinition {
            name: tn::BASH_KILL.to_string(),
            description: "Cancel a running background bash task spawned via run_bash_background. \
                No-op if the task already finished. Use when the user asks to stop a long-running job or when the task has already produced enough output to act on."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "task_id": {
                        "type": "string",
                        "description": "task_id returned by run_bash_background."
                    }
                },
                "required": ["task_id"]
            }),
        },
        ToolDefinition {
            name: tn::LIST_FILES.to_string(),
            description: "List all files in the workspace (artifacts and App UIs). Call ONCE at start if needed, then use the results.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
        },
        ToolDefinition {
            name: tn::GLOB_FILES.to_string(),
            description: "Find files in the workspace matching a glob pattern. Patterns are relative to data/ — use the same paths you'd see in list_files (e.g. 'apps/**/index.html', 'artifacts/*.md', '**/*.csv'). Searches artifacts/, apps/, knowhow/, triggers/. Prefer this over run_bash with find/ls.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "Glob pattern relative to data/. Examples: 'apps/**/index.html', 'artifacts/*.md', '**/*.csv'."
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Max paths to return (default 200, max 1000). Returned paths are sorted; `truncated: true` in the result indicates the cap was hit."
                    }
                },
                "required": ["pattern"]
            }),
        },
        ToolDefinition {
            name: tn::GREP_FILES.to_string(),
            description: "Search file contents using a regex (Rust regex crate syntax). Searches artifacts/, apps/, knowhow/, triggers/. Skips binary files. Prefer this over run_bash with rg/grep — it's structured and respects workspace ignore rules. Lines longer than 300 chars are truncated with `…` (PDF text dumps and unwrapped JSON commonly trip this), and the total returned text is capped at ~50 KB — narrow with `path_glob` if you hit `truncated: true` and need more detail.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "Regex pattern (Rust regex crate syntax). Case-sensitive by default."
                    },
                    "path_glob": {
                        "type": "string",
                        "description": "Optional glob to restrict which files are searched (e.g. 'apps/**/*.html', 'artifacts/*.md'). Defaults to all files under data/."
                    },
                    "case_insensitive": {
                        "type": "boolean",
                        "description": "Match case-insensitively (default false)."
                    },
                    "max_matches": {
                        "type": "integer",
                        "description": "Total match cap across all files (default 100, max 500). `truncated: true` in the result indicates the cap was hit."
                    },
                    "context_lines": {
                        "type": "integer",
                        "description": "Lines of context to return before/after each match (default 0, max 5)."
                    }
                },
                "required": ["pattern"]
            }),
        },
        ToolDefinition {
            name: tn::COPY_FILE.to_string(),
            description: "Copy a file within the workspace. Use this instead of read_file + write_file when you need to duplicate or move content — it handles the copy server-side without passing content through the conversation.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "source": {
                        "type": "string",
                        "description": "Source path under data/ (e.g., artifacts/imported/report.txt)"
                    },
                    "destination": {
                        "type": "string",
                        "description": "Destination path under data/ (e.g., artifacts/projects/report.txt)"
                    },
                    "message": {
                        "type": "string",
                        "description": "Semantic commit message (e.g., 'Copy report to projects folder')"
                    }
                },
                "required": ["source", "destination"]
            }),
        },
        ToolDefinition {
            name: tn::DELETE_FILE.to_string(),
            description: "Delete a file from the workspace (recoverable from git history). Refuses plugin-owned paths; the error tells you the owning plugin id — call uninstall_plugin with that id instead so the user sees a confirm panel before deletion.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Relative path under data/ (e.g., artifacts/notes.md, apps/my-app/ui/main/old.js)"
                    },
                    "message": {
                        "type": "string",
                        "description": "Semantic commit message describing why (e.g., 'Remove unused utility module')"
                    }
                },
                "required": ["path"]
            }),
        },
        ToolDefinition {
            name: tn::RELOAD_PROXY_MODULES.to_string(),
            description: "Re-scan `data/auth-modules/` and reload every WASM signer module. Use after dropping a new or updated `<name>.wasm` (and optional `<name>.manifest.json` sidecar) into that directory — the new modules become available to the proxy pipeline immediately, without restarting the engine. Returns the list of modules now loaded so you can confirm what's available. Note: `install_plugin` auto-reloads when the plugin ships `auth-modules/` files; this tool is only needed for hand-placed modules.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
        },
        ToolDefinition {
            name: tn::PROXY_REQUEST.to_string(),
            description: "Call a backend configured in `data/config/apis.json` through the engine proxy. Prefer this over `http_request` whenever the API has a proxy entry — the credential is resolved by the engine and never appears in the tool args, the tool transcript, or any logs. Returns the raw response body for 2xx; for non-2xx returns `HTTP Error N: ...`. The proxy `name` indexes into `apis.json`; `path` is appended to the configured `base_url`. Auth pipeline supports static credentials (bearer/api_key/basic/query_param), HMAC signing, script-handshake login flows, and per-request WASM signer modules — the engine handles authentication transparently.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Proxy name as configured in `data/config/apis.json` (e.g., 'sonos', 'comfort')."
                    },
                    "path": {
                        "type": "string",
                        "description": "Path appended to the configured base_url (e.g., '/Spisestua/play'). Optional — defaults to root."
                    },
                    "method": {
                        "type": "string",
                        "enum": ["GET", "POST", "PUT", "DELETE", "PATCH"],
                        "description": "HTTP method. Defaults to GET."
                    },
                    "headers": {
                        "type": "object",
                        "description": "Optional caller-supplied headers (Content-Type, Accept, …). The engine adds the configured auth header automatically.",
                        "additionalProperties": { "type": "string" }
                    },
                    "body": {
                        "type": "string",
                        "description": "Optional request body for POST/PUT/PATCH."
                    }
                },
                "required": ["name"]
            }),
        },
        ToolDefinition {
            name: tn::HTTP_REQUEST.to_string(),
            description: "Make an HTTP request to fetch data from an API. Use temp_path for raw data (.lucidos/tmp/, not git-tracked). Use output_path only for final artifacts (auto-committed).".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "method": {
                        "type": "string",
                        "enum": ["GET", "POST", "PUT", "DELETE"],
                        "description": "HTTP method"
                    },
                    "url": {
                        "type": "string",
                        "description": "Full URL to request"
                    },
                    "headers": {
                        "type": "object",
                        "description": "Optional headers as key-value pairs",
                        "additionalProperties": { "type": "string" }
                    },
                    "body": {
                        "type": "string",
                        "description": "Optional request body (for POST/PUT)"
                    },
                    "temp_path": {
                        "type": "string",
                        "description": "Filename to save in .lucidos/tmp/ (not git-tracked). Just the filename, e.g. 'google_doc.json' — NOT '.lucidos/tmp/google_doc.json'."
                    },
                    "output_path": {
                        "type": "string",
                        "description": "Relative path under data/ to save response (e.g., artifacts/imported/oura/sleep.json). Git committed automatically. Refuses responses larger than 100 MB — for bulk reference data, fetch into temp_path or move the persisted file to ~/.lucidos/data/<name>/ (see system-knowhow/best-practices rule 8)."
                    }
                },
                "required": ["method", "url"]
            }),
        },
        ToolDefinition {
            name: tn::IMPORT_FILE.to_string(),
            description: "Import a file from the local filesystem into the artifacts directory. The file will be copied, committed to git, and indexed for search. Refuses files larger than 100 MB — for bulk reference data, move the file to ~/.lucidos/data/<name>/ and pin the absolute path in the relevant app's knowhow (see system-knowhow/best-practices rule 8).".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "source_path": {
                        "type": "string",
                        "description": "Absolute path to the source file on the local filesystem"
                    },
                    "destination": {
                        "type": "string",
                        "description": "Relative path in artifacts/imported/ (optional, defaults to original filename)"
                    }
                },
                "required": ["source_path"]
            }),
        },
        ToolDefinition {
            name: tn::GIT_CLONE.to_string(),
            description: "Clone a git repository. Per system-knowhow/best-practices rule 8, the agent must explicitly choose where the clone lands — there is no default. Two valid destination roots: '.lucidos/tmp/<name>/' for research / inspect / extract-then-discard work (ephemeral, gitignored, won't bloat artifact count); 'data/artifacts/imported/<name>/' for persistent, git-tracked dependencies the user wants to keep. Refuses to write to artifacts/imported/ if the resulting clone exceeds 500 files or 100 MB — extract only what the app needs, or move the dataset to ~/.lucidos/data/<name>/.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "Git repository URL (https://github.com/owner/repo or git@github.com:owner/repo)"
                    },
                    "branch": {
                        "type": "string",
                        "description": "Branch to clone (optional, defaults to default branch)"
                    },
                    "destination": {
                        "type": "string",
                        "description": "REQUIRED. Must start with one of two prefixes: '.lucidos/tmp/<name>/' (ephemeral, gitignored — the default per rule 8 for research / inspect / extract-then-discard work) or 'data/artifacts/imported/<name>/' (persistent, git-tracked — only when the user has confirmed they want the full repo in the workspace). Bare names without a prefix are rejected to prevent silently bloating data/artifacts/."
                    },
                    "include_patterns": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Glob patterns for files to include (optional, e.g., ['*.py', 'src/**/*.rs'])"
                    },
                    "exclude_patterns": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Glob patterns for files to exclude (optional, e.g., ['*.lock', 'node_modules/**'])"
                    }
                },
                "required": ["url", "destination"]
            }),
        },
        ToolDefinition {
            name: tn::SET_LANGUAGE.to_string(),
            description: "Set the user's preferred language for responses and summaries. Use this when the user tells you their language preference.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "language": {
                        "type": "string",
                        "description": "Language name (e.g., 'English', 'Spanish', 'French', 'German')"
                    }
                },
                "required": ["language"]
            }),
        },
        ToolDefinition {
            name: tn::SET_TIMEZONE.to_string(),
            description: "Set the user's timezone. Use this when the user tells you their timezone. Required before creating any triggers.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "timezone": {
                        "type": "string",
                        "description": "IANA timezone name (e.g., 'America/New_York', 'Europe/London', 'Asia/Tokyo')"
                    }
                },
                "required": ["timezone"]
            }),
        },
        ToolDefinition {
            name: tn::CREATE_TRIGGER.to_string(),
            description: "Create a NEW trigger. Before calling this, list_triggers and prefer update_trigger for any tweak to an existing workflow (schedule, prompt, rename, pause, extra cron entry — append to the cron array even for one-shot extras). Recreating orphans the old trigger's run history. Two live triggers with identical names are a UX trap — name distinctly. Schedule-based (cron), event-based (on_event), or both. Cron times in the USER'S LOCAL timezone. MUST set timezone first (set_timezone).".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "A short, descriptive name for the trigger"
                    },
                    "run": {
                        "type": "object",
                        "description": "What to execute. Either { type: 'intent', intent: '...' } for LLM intents (one sentence in the user's voice — keep procedure out of the intent), or { type: 'script', path: 'name/run.py' } for scripts. If the LLM judges a procedure relevant at fire time, it calls `load_knowhow` itself — same as in chat. There is no per-trigger knowhow allow-list to configure here."
                    },
                    "cron": cron_schema(false),
                    "on_event": {
                        "type": "string",
                        "description": "Event type to trigger on (e.g., 'OuraSleepImported'). When this event is emitted, the trigger fires."
                    },
                    "condition": {
                        "type": "object",
                        "description": "Optional payload filter for event triggers. Uses operators: $eq, $ne, $lt, $lte, $gt, $gte, $in. Example: {\"sleep_score\": {\"$lt\": 70}}"
                    },
                    "app_id": {
                        "type": "string",
                        "description": "Owning app directory name (e.g. 'trigger-workflow'). Set this when the trigger belongs to an app the user can open — notifications will deep-link to that app's UI. Omit for standalone triggers."
                    },
                    "go_to_review": {
                        "type": "boolean",
                        "description": "When true, threads spawned by this trigger surface in REVIEW on completion instead of going straight to HISTORY. Use for triggers whose output the user is meant to read — daily summaries, alerts, scheduled reports. Default false (silent execution, history-only) suits most cron triggers."
                    }
                },
                "required": ["name", "run"]
            }),
        },
        ToolDefinition {
            name: tn::LIST_TRIGGERS.to_string(),
            description: "List all triggers the user has created. Shows trigger names, schedules, and what each trigger runs.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
        },
        ToolDefinition {
            name: tn::UPDATE_TRIGGER.to_string(),
            description: "Update an existing trigger's name, schedule, event subscription, or run config. PREFER this over delete+create for any change to an existing workflow — the trigger_id stays stable so the run history stays linked. To add an extra firing time (including a temporary one-shot), append to the cron array; don't make a sibling trigger. Use list_triggers first to find the trigger ID. At least one field besides trigger_id must be provided.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "trigger_id": {
                        "type": "string",
                        "description": "UUID of the trigger to update"
                    },
                    "name": {
                        "type": "string",
                        "description": "New name for the trigger"
                    },
                    "run": {
                        "type": "object",
                        "description": "Change what to execute. { type: 'intent', intent: '...' } or { type: 'script', path: '...' }. If the LLM judges a procedure relevant at fire time, it calls `load_knowhow` itself — same as in chat. There is no per-trigger knowhow allow-list to configure here."
                    },
                    "cron": cron_schema(true),
                    "on_event": {
                        "type": ["string", "null"],
                        "description": "Event type to trigger on. Set to null to remove event subscription."
                    },
                    "condition": {
                        "type": ["object", "null"],
                        "description": "Payload filter for event triggers. Set to null to remove condition."
                    },
                    "paused": {
                        "type": "boolean",
                        "description": "Pause/resume the trigger as part of a multi-field update. For pause/resume alone, prefer the dedicated pause_trigger / resume_trigger tools."
                    },
                    "app_id": {
                        "type": ["string", "null"],
                        "description": "Owning app directory name (e.g. 'trigger-workflow'). Set to null to clear (e.g. trigger no longer belongs to any app)."
                    },
                    "go_to_review": {
                        "type": "boolean",
                        "description": "When true, future threads spawned by this trigger surface in REVIEW on completion instead of going straight to HISTORY. Setting this only affects new runs — already-completed threads are not retroactively re-routed."
                    }
                },
                "required": ["trigger_id"]
            }),
        },
        ToolDefinition {
            name: tn::DELETE_TRIGGER.to_string(),
            description: "Delete a trigger by its ID. Use list_triggers first to find the trigger ID.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "trigger_id": {
                        "type": "string",
                        "description": "UUID of the trigger to delete"
                    }
                },
                "required": ["trigger_id"]
            }),
        },
        ToolDefinition {
            name: tn::PAUSE_TRIGGER.to_string(),
            description: "Pause an existing trigger so it stops firing on its schedule and stops matching events. The trigger's config is preserved — use resume_trigger to re-enable it. Use list_triggers first to find the trigger ID.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "trigger_id": { "type": "string", "description": "UUID of the trigger to pause" }
                },
                "required": ["trigger_id"]
            }),
        },
        ToolDefinition {
            name: tn::RESUME_TRIGGER.to_string(),
            description: "Resume a previously paused trigger so it fires on its schedule and matches events again. Use list_triggers first to find the trigger ID.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "trigger_id": { "type": "string", "description": "UUID of the trigger to resume" }
                },
                "required": ["trigger_id"]
            }),
        },
        ToolDefinition {
            name: tn::FETCH_NEWS.to_string(),
            description: "Fetch recent news articles on a topic. Uses GDELT global news database which covers news in all languages worldwide. Automatically prioritizes sources from user's country based on their timezone.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "topic": {
                        "type": "string",
                        "description": "The news topic or search query (e.g., 'Trump', 'climate', 'sports', 'AI')"
                    },
                    "max_articles": {
                        "type": "integer",
                        "description": "Maximum number of articles to return (default: 5)"
                    }
                },
                "required": ["topic"]
            }),
        },
        // Browser tools for autonomous web browsing
        ToolDefinition {
            name: tn::BROWSER_OPEN.to_string(),
            description: "Open a web page in a browser session. Returns the page text content. The browser uses a persistent profile — logins, cookies, and localStorage carry over between sessions.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "Full URL to navigate to (e.g., 'https://news.ycombinator.com')"
                    },
                    "wait_for": {
                        "type": "string",
                        "description": "Optional CSS selector to wait for before returning content"
                    },
                    "visible": {
                        "type": "boolean",
                        "description": "Open a visible Chrome window the user can see and interact with. Use when the user says 'show me', 'let me log in', or when a site blocks headless browsers. Default: false (headless)."
                    }
                },
                "required": ["url"]
            }),
        },
        ToolDefinition {
            name: tn::BROWSER_EXTRACT.to_string(),
            description: "Extract content from elements on the current page. Use after browser_open.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "selector": {
                        "type": "string",
                        "description": "CSS selector for elements to extract (e.g., '.story-title', 'table.data', 'a.link')"
                    },
                    "format": {
                        "type": "string",
                        "enum": ["text", "html", "links", "table"],
                        "description": "Output format: 'text' (innerText), 'html' (outerHTML), 'links' (URLs with text), 'table' (table rows as pipe-separated)"
                    }
                },
                "required": ["selector", "format"]
            }),
        },
        ToolDefinition {
            name: tn::BROWSER_CLICK.to_string(),
            description: "Click an element on the current page. Use for buttons, links, or interactive elements.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "selector": {
                        "type": "string",
                        "description": "CSS selector for the element to click"
                    },
                    "wait_navigation": {
                        "type": "boolean",
                        "description": "Wait for page navigation after click (default: false)"
                    }
                },
                "required": ["selector"]
            }),
        },
        ToolDefinition {
            name: tn::BROWSER_TYPE.to_string(),
            description: "Type text into an input field on the current page. Use for search boxes, forms, etc.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "selector": {
                        "type": "string",
                        "description": "CSS selector for the input element"
                    },
                    "text": {
                        "type": "string",
                        "description": "Text to type into the input"
                    },
                    "clear": {
                        "type": "boolean",
                        "description": "Clear existing content before typing (default: false)"
                    },
                    "enter": {
                        "type": "boolean",
                        "description": "Press Enter after typing (default: false)"
                    }
                },
                "required": ["selector", "text"]
            }),
        },
        ToolDefinition {
            name: tn::BROWSER_EVAL.to_string(),
            description: "Execute JavaScript code on the current page and return the result. Use for complex interactions or data extraction.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "script": {
                        "type": "string",
                        "description": "JavaScript code to execute. Return value will be converted to string/JSON."
                    }
                },
                "required": ["script"]
            }),
        },
        ToolDefinition {
            name: tn::BROWSER_SCREENSHOT.to_string(),
            description: "Take a screenshot and save it to artifacts. Can optionally navigate to a URL first.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Relative path under data/ to save the screenshot (e.g., 'artifacts/screenshots/page.png')"
                    },
                    "url": {
                        "type": "string",
                        "description": "Optional URL to navigate to before taking the screenshot. Use this when taking screenshots of multiple different sites."
                    },
                    "selector": {
                        "type": "string",
                        "description": "Optional CSS selector to screenshot a specific element"
                    },
                    "full_page": {
                        "type": "boolean",
                        "description": "Capture the full scrollable page instead of just the viewport (default: false)"
                    }
                },
                "required": ["path"]
            }),
        },
        ToolDefinition {
            name: tn::BROWSER_CLOSE.to_string(),
            description: "Close the browser session. The browser will also auto-close after 30 minutes of inactivity.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
        },
        ToolDefinition {
            name: tn::BROWSER_FORGET_LOGIN.to_string(),
            description: "Remove a recorded browser login (e.g., expired session, user logged out).".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "domain": {
                        "type": "string",
                        "description": "Domain to forget (e.g., 'github.com')"
                    }
                },
                "required": ["domain"]
            }),
        },
        ToolDefinition {
            name: tn::BROWSER_CLEAR_DATA.to_string(),
            description: "Delete all Lucidos browser data: cookies, logins, localStorage, cache. Closes any running browser first. Use when the user wants to start fresh.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
        },
        ToolDefinition {
            name: tn::WEB_SEARCH.to_string(),
            description: "Search the web for information. Use this when you need to look up facts, verify information, or find current data you're unsure about. Search once or twice max — if you found the answer, STOP searching.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "The search query (e.g., 'python list comprehension', 'best pizza recipe')"
                    },
                    "max_results": {
                        "type": "integer",
                        "description": "Maximum number of results to return (default: 5)"
                    }
                },
                "required": ["query"]
            }),
        },
        ToolDefinition {
            name: tn::ENABLE_PUSH_NOTIFICATIONS.to_string(),
            description: "Enable or decline browser push notifications. Call with enabled=true if the user wants OS-level alerts for triggered tasks, or enabled=false if they decline.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "enabled": {
                        "type": "boolean",
                        "description": "true to enable push notifications, false to decline (won't ask again)"
                    }
                },
                "required": ["enabled"]
            }),
        },
        ToolDefinition {
            name: tn::REQUEST_CREDENTIAL.to_string(),
            description: "Request API credentials from the user via a secure modal dialog. NEVER accept credentials pasted in chat — always use this tool. The user enters the credential in a popup (not in chat), keeping it secure and out of the event log. Call this for ONE credential at a time and wait for it to resolve before requesting the next — issuing multiple credential requests in parallel stacks modals and forces the user to history-navigate between them.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "service_name": {
                        "type": "string",
                        "description": "Name of the service (e.g., 'oura', 'github', 'notion')"
                    },
                    "prompt": {
                        "type": "string",
                        "description": "Instructions shown in the modal. Include where to find the credential (e.g., 'Go to cloud.ouraring.com → Personal Access Tokens → Create. Paste the token below.')"
                    },
                    "base_url": {
                        "type": "string",
                        "description": "Base URL for the API (e.g., 'https://api.ouraring.com')"
                    },
                    "auth_type": {
                        "type": "string",
                        "enum": ["api_key", "bearer", "basic", "password", "oauth_client"],
                        "description": "Type of authentication. Use 'password' for username+password (injected as Basic auth), 'oauth_client' for OAuth client_id+client_secret. Default: api_key"
                    }
                },
                "required": ["service_name", "prompt", "base_url", "auth_type"]
            }),
        },
        // Email tools
        ToolDefinition {
            name: tn::CONFIGURE_EMAIL.to_string(),
            description: "Configure an email account for sending and reading email. Use web_search to look up the provider's IMAP/SMTP host and port before calling this. For authentication, prefer use_oauth if an OAuth account is connected (many providers require it now), otherwise fall back to app password.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Account label (e.g., 'Gmail', 'Work', 'Personal')"
                    },
                    "email_address": {
                        "type": "string",
                        "description": "Email address (e.g., 'user@gmail.com')"
                    },
                    "imap_host": {
                        "type": "string",
                        "description": "IMAP server hostname"
                    },
                    "imap_port": {
                        "type": "integer",
                        "description": "IMAP server port (default: 993 for TLS)"
                    },
                    "smtp_host": {
                        "type": "string",
                        "description": "SMTP server hostname"
                    },
                    "smtp_port": {
                        "type": "integer",
                        "description": "SMTP server port (default: 587 for STARTTLS)"
                    },
                    "username": {
                        "type": "string",
                        "description": "Login username (defaults to email_address if omitted)"
                    },
                    "use_tls": {
                        "type": "boolean",
                        "description": "Use TLS for connections (default: true)"
                    },
                    "require_send_confirmation": {
                        "type": "boolean",
                        "description": "Require user confirmation before sending (default: true)"
                    },
                    "use_oauth": {
                        "type": "string",
                        "description": "OAuth provider name to use for SMTP authentication (e.g., 'microsoft', 'google'). Uses XOAUTH2 instead of password auth. The provider must already be connected via connect_oauth_account."
                    }
                },
                "required": ["name", "email_address", "imap_host", "smtp_host"]
            }),
        },
        ToolDefinition {
            name: tn::SEND_EMAIL.to_string(),
            description: "Send an email. If the account requires confirmation, the user will see a preview and must approve before sending.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "to": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Recipient email addresses"
                    },
                    "subject": {
                        "type": "string",
                        "description": "Email subject line"
                    },
                    "body": {
                        "type": "string",
                        "description": "Email body (plain text)"
                    },
                    "cc": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "CC recipients"
                    },
                    "bcc": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "BCC recipients"
                    },
                    "reply_to_message_id": {
                        "type": "string",
                        "description": "Message-ID to reply to (for threading)"
                    },
                    "account": {
                        "type": "string",
                        "description": "Account name (uses default if omitted)"
                    },
                    "attachments": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "File paths relative to data/ to attach (e.g. 'artifacts/projects/report.pdf'). MIME type is auto-detected from extension."
                    }
                },
                "required": ["to", "subject", "body"]
            }),
        },
        ToolDefinition {
            name: tn::READ_EMAILS.to_string(),
            description: "Fetch recent emails from the inbox. Returns a list with sender, subject, date, and preview. Use read_email with a specific UID to get the full message body.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "folder": {
                        "type": "string",
                        "description": "IMAP folder (default: 'INBOX')"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Max emails to return (default: 10, max: 50)"
                    },
                    "search": {
                        "type": "string",
                        "description": "IMAP search query (e.g., 'FROM user@example.com', 'SUBJECT meeting', 'UNSEEN')"
                    },
                    "since": {
                        "type": "string",
                        "description": "Only emails since this date (format: '25-Feb-2026' — IMAP date format)"
                    },
                    "account": {
                        "type": "string",
                        "description": "Account name (uses default if omitted)"
                    }
                },
                "required": []
            }),
        },
        ToolDefinition {
            name: tn::READ_EMAIL.to_string(),
            description: "Read the full content of a single email by its UID (from read_emails results).".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "uid": {
                        "type": "integer",
                        "description": "Email UID from read_emails results"
                    },
                    "folder": {
                        "type": "string",
                        "description": "IMAP folder (default: 'INBOX')"
                    },
                    "account": {
                        "type": "string",
                        "description": "Account name (uses default if omitted)"
                    }
                },
                "required": ["uid"]
            }),
        },
        ToolDefinition {
            name: tn::SAVE_EMAIL_ATTACHMENT.to_string(),
            description: "Save an email attachment to the workspace. Use after read_email shows attachments. For PDF attachments, text is automatically extracted. Files are saved to data/artifacts/imported/email/ by default.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "uid": {
                        "type": "integer",
                        "description": "Email UID (from read_email results)"
                    },
                    "attachment_index": {
                        "type": "integer",
                        "description": "Attachment index (from read_email attachments list, 0-based)"
                    },
                    "folder": {
                        "type": "string",
                        "description": "IMAP folder (default: 'INBOX')"
                    },
                    "destination": {
                        "type": "string",
                        "description": "Destination path relative to data/artifacts/ (default: 'imported/email/<filename>')"
                    },
                    "account": {
                        "type": "string",
                        "description": "Account name (uses default if omitted)"
                    }
                },
                "required": ["uid", "attachment_index"]
            }),
        },
        // App management tools
        ToolDefinition {
            name: tn::CREATE_APP.to_string(),
            description: "Create a new app with a UI. The app will be saved to data/apps/{id}/ with manifest.json and index.html. IMPORTANT: App data should be stored in data/artifacts/ (e.g., data/artifacts/habits/data.json), NOT in data/apps/. The manifest.json MUST include both 'name' and 'description' fields.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "App ID (folder name, lowercase with hyphens, e.g., 'habit-tracker')"
                    },
                    "name": {
                        "type": "string",
                        "description": "Display name for the app"
                    },
                    "description": {
                        "type": "string",
                        "description": "One-line description"
                    },
                    "html_content": {
                        "type": "string",
                        "description": "Initial HTML content for the app's index.html"
                    }
                },
                "required": ["id", "name", "description", "html_content"]
            }),
        },
        ToolDefinition {
            name: tn::LIST_APPS.to_string(),
            description: "List all available apps in the workspace.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
        },
        ToolDefinition {
            name: tn::LOAD_KNOWHOW.to_string(),
            description: "Load the full content of a know-how document. The system prompt lists available know-how by name and description — use this tool to load the full content when a know-how file is relevant to the user's request.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "Know-how ID as shown in the know-how list (e.g., 'lucidos/cross-workspace')"
                    }
                },
                "required": ["id"]
            }),
        },
        ToolDefinition {
            name: tn::REFRESH_FILE.to_string(),
            description: "Refresh the user's file preview window to show updated content. Call this after writing/editing a file that the user has open.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "File path to refresh (e.g., artifacts/notes.md)"
                    }
                },
                "required": ["path"]
            }),
        },
        ToolDefinition {
            name: tn::REFRESH_APP.to_string(),
            description: "Refresh the iframe of the currently-open app so it reflects on-disk changes, then return a screenshot and DOM snapshot (unless skip_capture is true). If the app isn't currently open, the refresh is a no-op and the capture step will fail — use navigate_ui first when you need to look at an app the user hasn't opened.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "app_id": {
                        "type": "string",
                        "description": "The app ID to refresh"
                    },
                    "skip_capture": {
                        "type": "boolean",
                        "description": "If true, skip the automatic screenshot capture (useful during rapid iteration). Default: false"
                    }
                },
                "required": ["app_id"]
            }),
        },
        ToolDefinition {
            name: tn::CAPTURE_APP.to_string(),
            description: "Capture a screenshot and DOM snapshot of the currently open app UI to see what it looks like. Use this to check the visual result of your changes or when the user asks you to look at the UI.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "app_id": {
                        "type": "string",
                        "description": "The app ID whose UI to capture"
                    }
                },
                "required": ["app_id"]
            }),
        },
        ToolDefinition {
            name: tn::CONNECT_OAUTH_ACCOUNT.to_string(),
            description: "Connect an OAuth account (Google, Microsoft, GitHub, etc.) so Lucidos can make authenticated API requests on the user's behalf. Opens the user's browser for authorization. Supported providers: google, microsoft, github (or any custom OAuth 2.0 provider with client credentials configured).".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "provider": {
                        "type": "string",
                        "description": "Provider name (e.g., 'google', 'microsoft', 'github')"
                    },
                    "scopes": {
                        "type": "string",
                        "description": "Space-separated OAuth scopes to request (e.g., 'https://www.googleapis.com/auth/spreadsheets https://www.googleapis.com/auth/drive.readonly')"
                    }
                },
                "required": ["provider", "scopes"]
            }),
        },
        ToolDefinition {
            name: tn::RUN_THREAD.to_string(),
            description: "Start a new Lucidos thread to handle a subtask. Default behavior (relation=\"sub\") is a sub-thread: it runs its own agentic loop with full tool access, and when it completes a callback automatically resumes THIS thread with the sub-thread's result (including its final response text) — you do NOT need to poll. You can spawn multiple sub-threads in parallel; each reports back independently. When a sub-thread reports back, review its result — if it's incomplete, spawn another run_thread with a refined prompt. Pass relation=\"top\" instead when the spawn is for the user to read later (research/report) and you do NOT need the result yourself; the spawned thread runs as an independent top-level thread and never reports back. Use for non-code tasks (research, analysis, drafting). For code tasks, use run_claude instead.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "prompt": {
                        "type": "string",
                        "description": "The task for the new thread"
                    },
                    "title": {
                        "type": "string",
                        "description": "Optional short descriptive title (3-6 words) for the thread. When provided, the system will not auto-generate a title. Recommended so the thread list is meaningful at a glance."
                    },
                    "relation": {
                        "type": "string",
                        "enum": ["sub", "top"],
                        "description": "How the spawned thread relates to this one. 'sub' (default): when the spawned thread finishes, this thread automatically resumes with its result — use for delegated subtasks whose answer you need. 'top': fire-and-forget — the spawned thread runs independently as a top-level thread; this thread does not resume when it finishes. Use when the spawn is for the user to look at later, not for you."
                    }
                },
                "required": ["prompt"]
            }),
        },
        ToolDefinition {
            name: tn::RUN_CLAUDE.to_string(),
            description: "Run Claude Code to edit source code. ONLY use this when the user explicitly asks to modify code (Lucidos or an external repo). Never use this for workspace tasks like web scraping, file manipulation, data processing, or anything the native tools can handle — use browser_open, web_search, http_request, run_python, read_file, write_file, etc. instead.\n\nDefault behavior (relation=\"sub\") is a sub-thread: spawning returns immediately; when the CC session ends, a callback automatically resumes THIS thread with the sub-thread's final response text — you do NOT need to poll. For PARALLEL work, issue multiple run_claude calls in one response and they spawn concurrently, each reporting back independently. For SEQUENTIAL pipelines (where step N depends on step N-1's outcome — e.g., build → harden → e2e, stopping on first failure), spawn ONE run_claude and end the turn; the next step runs only after the callback resumes you. Never batch sequential spawns in one response — that defeats the dependency. Always inspect the sub-thread's final response text to determine pass/fail before acting on it or emitting milestones — the spawn ack is not a result.\n\nPass relation=\"top\" instead when the user asks for a piece of work to happen in its own thread that they will follow themselves (e.g. 'do this in a separate thread' / 'spawn a CC session for this and I'll check in later'). The spawned CC session runs as an independent top-level thread and will NOT report back to this conversation.\n\nSAME WORKSPACE ONLY. `run_claude` always spawns in the CURRENT workspace — there is no workspace parameter. When the user asks for the work to land in a different workspace ('do this in dev', 'fix it in personal', 'send to work', 'run in another workspace'), use `lucidos spawn-thread --to <ws> --cc ...` via `run_bash` instead — that is the only cross-workspace path. The `repo` parameter below selects a repo within THIS workspace's registry; it is NOT a workspace selector, and a `repo` name that happens to match another workspace's name will silently resolve here.\n\nBEFORE CALLING: identify which repo the work targets.\n- Editing Lucidos itself (engine/UI under the Lucidos source tree) → omit `repo`.\n- Prompt mentions any external path, repo name, or sibling-directory work → call `manage_repositories` with `action='list'`, find the matching registered repo, and pass `repo=<name>`. Without this the session runs in the Lucidos worktree and edits land in the wrong place even if the prompt uses absolute paths — absolute paths in the prompt are NOT a substitute for the `repo` parameter.\n- Target repo isn't registered → ask the user to register it (or register it yourself with `manage_repositories action='add'` if you know the path) before spawning.\n- Ambiguous → ask one question: 'which repo should this run in?' before calling.\n\nIf Rust backend files are changed, Lucidos shows the user a toast suggesting a restart — the user must manually trigger the rebuild and restart for backend changes to take effect (do NOT promise that changes will be live shortly). Frontend (TypeScript/CSS) changes are picked up automatically.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "prompt": {
                        "type": "string",
                        "description": "The coding task to perform. Be specific about what files to modify and what the desired outcome is."
                    },
                    "repo": {
                        "type": "string",
                        "description": "Repository ID or name from manage_repositories — resolved in the CURRENT workspace's repo registry. NOT a workspace selector: to target another workspace, use `lucidos spawn-thread --to <ws> --cc` via `run_bash` instead (this tool always spawns in the current workspace). REQUIRED whenever the work targets anything outside the Lucidos source tree — without it the session runs in this workspace's Lucidos worktree and edits go to the wrong directory even if the prompt uses absolute paths. Omit ONLY when editing Lucidos itself."
                    },
                    "allowed_tools": {
                        "type": "string",
                        "description": "Tools to auto-approve, comma-separated (default: 'Bash,Read,Edit,Write,Glob,Grep')"
                    },
                    "model": {
                        "type": "string",
                        "description": "Model override (e.g., 'sonnet', 'opus'). Omit to use Claude Code's default."
                    },
                    "append_system_prompt": {
                        "type": "string",
                        "description": "Additional instructions to append to Claude Code's system prompt"
                    },
                    "images": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Optional list of conversation images to forward to the CC session, e.g. [\"thread:1\", \"thread:3\"]. Indices match the 1-based order images appear in this thread. Omit to forward the current message's images (default). Pass an empty array to forward none."
                    },
                    "title": {
                        "type": "string",
                        "description": "Optional short descriptive title (3-6 words) for the spawned CC thread. When provided, the system will not auto-generate a title. Recommended so the thread list is meaningful at a glance."
                    },
                    "relation": {
                        "type": "string",
                        "enum": ["sub", "top"],
                        "description": "How the spawned CC session relates to this thread. 'sub' (default): when the CC session ends, this thread automatically resumes with its result — use for delegated coding subtasks whose outcome you need. 'top': fire-and-forget — the CC session runs independently as a top-level thread; this thread does not resume when it finishes. Use when the user asks for the work to happen in a separate thread they will follow themselves."
                    }
                },
                "required": ["prompt"]
            }),
        },
        ToolDefinition {
            name: tn::CORRECT_MEMORY.to_string(),
            description: "Search for and correct wrong memories. Use when the user says a stored memory is wrong, inaccurate, or should be removed. Searches by keyword, then uses semantic similarity to the wrong_fact to only delete entries that actually express the wrong claim — other entries mentioning the same keyword are kept.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "search_query": {
                        "type": "string",
                        "description": "Keyword to find candidate memories (e.g., 'Acme Corp'). Broad is OK — semantic filtering narrows it down."
                    },
                    "wrong_fact": {
                        "type": "string",
                        "description": "The specific wrong claim to delete (e.g., 'User works at Acme Corp'). Only memories semantically similar to this are deleted."
                    },
                    "correction": {
                        "type": "string",
                        "description": "Optional corrected fact to store after deleting wrong memories. Omit to just delete without replacement."
                    }
                },
                "required": ["search_query", "wrong_fact"]
            }),
        },
        ToolDefinition {
            name: tn::EXECUTE_INTENT.to_string(),
            description: "Execute a stored intent — a description of what the user wants, paired with know-how for how to achieve it. The intent's instructions and relevant know-how are loaded automatically. Internal tool calls happen inside the execution — only the final result is returned. Use this when the user's request matches an available intent.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "intent_id": {
                        "type": "string",
                        "description": "Intent ID (e.g., 'job-search/find-jobs', 'home-control/run-control-loop')"
                    },
                    "task": {
                        "type": "string",
                        "description": "What to do (e.g., 'Make the title black', 'Log today\\'s sleep data')"
                    }
                },
                "required": ["intent_id"]
            }),
        },
        ToolDefinition {
            name: tn::EMIT_EVENT.to_string(),
            description: "Emit a domain event to the event store. Use this to record task outcomes (e.g., GoogleDocEdited, OuraDataImported). Events are immutable facts — past tense, append-only.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "event_type": {
                        "type": "string",
                        "description": "Event type in PascalCase past tense (e.g., GoogleDocEdited, DataImported)"
                    },
                    "payload": {
                        "type": "object",
                        "description": "Event payload — REQUIRED. Include enough context to understand what happened. Example: {\"documentId\": \"abc\", \"title\": \"My Doc\", \"summary\": \"Changed title color to black\", \"operations\": 3}",
                        "properties": {
                            "summary": {
                                "type": "string",
                                "description": "Human-readable description of what happened"
                            }
                        },
                        "required": ["summary"]
                    }
                },
                "required": ["event_type", "payload"]
            }),
        },
        ToolDefinition {
            name: tn::QUERY_EVENTS.to_string(),
            description: "Query domain events from the event store. Use this to look up past events by type and/or time range. Returns events in reverse chronological order (newest first).".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "event_type": {
                        "type": "string",
                        "description": "Filter by event type (e.g., CheckboxToggled, DataImported). Omit to query all event types."
                    },
                    "since": {
                        "type": "string",
                        "description": "Only return events after this ISO 8601 / RFC 3339 timestamp (e.g., 2026-03-01T00:00:00Z)"
                    },
                    "until": {
                        "type": "string",
                        "description": "Only return events before this ISO 8601 / RFC 3339 timestamp"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum number of events to return (1-1000, default 100)"
                    }
                }
            }),
        },
        ToolDefinition {
            name: tn::INSTALL_PLUGIN.to_string(),
            description: "Stage a Lucidos plugin install for the user to confirm in a panel. A plugin is a coherent bundle of workspace content (apps, knowhow, triggers, scripts) that another author shipped. Source detection: a GitHub tree URL like 'https://github.com/owner/repo/tree/branch/subpath' is a monorepo install (clones the repo at that branch, uses subpath as the plugin root). A plain git URL or .git URL is cloned at the default branch. A path ending in '.lucidos-plugin' is unpacked locally. The user sees a confirm panel with the file list, source, and any overwrites — do NOT chat-ask the user about overwrites or repeat what the panel will show. After this call, do NOT respond about the install or claim it succeeded; the panel resolves it on Confirm/Cancel and emits the appropriate event. The next user message will tell you whether the install happened.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "source": {
                        "type": "string",
                        "description": "GitHub tree URL (e.g., 'https://github.com/lucidos-dev/plugins/tree/main/browser-learning'), a plain git URL, or an absolute path to a .lucidos-plugin file."
                    }
                },
                "required": ["source"]
            }),
        },
        ToolDefinition {
            name: tn::CHECK_PLUGIN_UPDATES.to_string(),
            description: "Check installed plugins for newer versions at their `source` URL. With no `id`, surveys all currently-installed plugins. Network failures per plugin are reported as `error` entries — they don't abort the whole check. Returns JSON describing each plugin's installed_version, latest_version, and whether it changed.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "Optional plugin id (e.g. 'browser-learning'). Omit to check every installed plugin."
                    }
                }
            }),
        },
        ToolDefinition {
            name: tn::UPDATE_PLUGIN.to_string(),
            description: "Apply the update for one installed plugin. Re-fetches the manifest from the recorded source, compares semver, and re-installs (with overwrite=true) if newer. Returns 'Already at latest (vX)' as a no-op when versions match.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "The plugin id to update (e.g. 'browser-learning')."
                    }
                },
                "required": ["id"]
            }),
        },
        ToolDefinition {
            name: tn::UNINSTALL_PLUGIN.to_string(),
            description: "Stage a Lucidos plugin uninstall for the user to confirm in a panel. Resolves `id` against the plugin id, the manifest name, OR an `apps/<dir>` folder name installed by the plugin (case-insensitive, dash/underscore/whitespace-insensitive — \"No role playing\", \"no-role-playing\", and \"anti-sycophancy-critique\" all resolve to the same plugin). On exact match, looks up the install record, partitions the recorded files into still-on-disk vs already-missing, and shows the user a confirm panel listing what will be deleted. On multiple matches, returns an error listing the candidates so you can re-call with the exact id. Do NOT chat-ask which files to delete — the panel surfaces them. Do NOT manually delete_file the listed paths; the panel's Confirm button removes them atomically + reloads WASM signers if needed (and delete_file refuses plugin-owned paths anyway). After this call, do NOT respond about the uninstall or claim files were removed; the panel resolves it on Confirm/Cancel and emits the appropriate event. The next user message will tell you the outcome.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "Plugin id, manifest name, or app folder installed by the plugin (e.g. 'browser-learning', 'Browser Learning', or 'browser-learning' app dir). Case- and dash/underscore/whitespace-insensitive."
                    }
                },
                "required": ["id"]
            }),
        },
        ToolDefinition {
            name: tn::DISMISS_FROM_CONTEXT.to_string(),
            description: "Drop a prior tool result or child-thread completion from your future resume context. Use when you're done with that information and want to keep your context lean across long pipelines. Pass the event_id from prior history: tool blocks show `evt-<uuid>` as the tool_use_id, and ChildThreadCompleted blocks include an `event_id: <uuid>` line. Either form is accepted.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "event_id": {
                        "type": "string",
                        "description": "Event id of the ToolCalled or ChildThreadCompleted event to dismiss. Accepts either the bare UUID (hyphenated or simple) or the `evt-<uuid>` form rendered as tool_use_id in tool blocks."
                    }
                },
                "required": ["event_id"]
            }),
        },
    ]
}

/// Tool for saving a thread image to an artifact path.
pub fn get_save_thread_image_tool() -> ToolDefinition {
    ToolDefinition {
        name: tn::SAVE_THREAD_IMAGE.to_string(),
        description: "Save an image from the conversation history to an artifact file. Use this when the user wants to keep an image they pasted or that was generated earlier. The image is committed to git automatically.".to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "image": {
                    "type": "string",
                    "description": "Thread image reference: 'thread:N' where N is the 1-based sequential index of images in the conversation (same numbering as generate_image's input_images)."
                },
                "path": {
                    "type": "string",
                    "description": "Destination path relative to data/artifacts/ (e.g., 'projects/allergi/photo.jpg'). The image is committed to git."
                }
            },
            "required": ["image", "path"]
        }),
    }
}

/// Tool for generating or editing images.
pub fn get_image_generation_tool() -> ToolDefinition {
    ToolDefinition {
        name: tn::GENERATE_IMAGE.to_string(),
        description: "SYNTHESIZES a new image, or edits an existing image. Returns image bytes — never text. \
            This is NOT a vision/analysis tool: do NOT call it to 'describe', 'analyze', 'summarize', \
            'transcribe', or 'tell me what's in' an image. To describe an image already in the conversation, \
            just describe it directly in your reply — you can see it natively. \
            Provide `prompt` describing the desired output image. To edit an existing image, also pass \
            `input_images`. The current image provider may only support one input image — if you provide \
            multiple and it's not supported, the call fails with an error asking the user to pick one.".to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "prompt": {
                    "type": "string",
                    "description": "Describes the image to be synthesized (or how to edit input_images). Must describe a desired output picture, NOT instructions like 'describe this image' — that wastes a generation call and returns a meaningless image."
                },
                "input_images": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Optional image references to edit. Each entry is either 'thread:N' (Nth image in the conversation, 1-based) or an artifact path (e.g., 'artifacts/photo.png'). Omit for text-to-image generation."
                },
                "size": {
                    "type": "string",
                    "enum": ["square", "landscape", "portrait", "auto"],
                    "description": "Output image dimensions. Default: 'auto'."
                },
                "save_as_artifact": {
                    "type": "string",
                    "description": "Optional path relative to data/artifacts/ to save the generated image (e.g., 'generated/logo.png'). Image is git-committed."
                }
            },
            "required": ["prompt"]
        }),
    }
}

/// Tool definitions for MCP server management.
pub fn get_mcp_tools() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: tn::SETUP_MCP_SERVER.to_string(),
            description: "Register and connect a new MCP (Model Context Protocol) server. The server process is spawned and tools are discovered automatically. Use web_search first to find the right package and install command for the MCP server the user wants.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "Unique identifier for this server (e.g., 'blender-mcp', 'roblox-studio'). Use lowercase with hyphens."
                    },
                    "name": {
                        "type": "string",
                        "description": "Human-readable name (e.g., 'Blender MCP', 'Roblox Studio MCP')"
                    },
                    "command": {
                        "type": "string",
                        "description": "Command to run the MCP server (e.g., 'npx', 'uvx', 'node')"
                    },
                    "args": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Arguments for the command (e.g., ['blender-mcp'] for 'uvx blender-mcp')"
                    },
                    "env": {
                        "type": "object",
                        "description": "Optional environment variables for the server process",
                        "additionalProperties": { "type": "string" }
                    }
                },
                "required": ["id", "name", "command", "args"]
            }),
        },
        ToolDefinition {
            name: tn::LIST_MCP_SERVERS.to_string(),
            description: "List all configured MCP servers with their status (running/stopped) and available tools.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {}
            }),
        },
        ToolDefinition {
            name: tn::START_MCP_SERVER.to_string(),
            description: "Start a stopped MCP server by its ID.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "Server ID to start"
                    }
                },
                "required": ["id"]
            }),
        },
        ToolDefinition {
            name: tn::STOP_MCP_SERVER.to_string(),
            description: "Stop a running MCP server by its ID.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "Server ID to stop"
                    }
                },
                "required": ["id"]
            }),
        },
        ToolDefinition {
            name: tn::REMOVE_MCP_SERVER.to_string(),
            description: "Remove an MCP server configuration (stops it first if running).".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "Server ID to remove"
                    }
                },
                "required": ["id"]
            }),
        },
    ]
}

#[cfg(test)]
#[path = "tools_tests.rs"]
mod tests;
