/// Canonical tool name constants — single source of truth for all tool name strings.
/// Used by tool definitions (tools.rs), tool dispatch (execute_tool, handle_special_tool),
/// and the agentic loop for caching/circuit-breaker checks.
// File operations
pub const READ_FILE: &str = "read_file";
pub const WRITE_FILE: &str = "write_file";
pub const EDIT_FILE: &str = "edit_file";
pub const LIST_FILES: &str = "list_files";
pub const GLOB_FILES: &str = "glob_files";
pub const GREP_FILES: &str = "grep_files";
pub const COPY_FILE: &str = "copy_file";
pub const DELETE_FILE: &str = "delete_file";

// Execution
pub const RUN_PYTHON: &str = "run_python";
pub const RUN_PYTHON_BACKGROUND: &str = "run_python_background";
pub const RUN_BASH: &str = "run_bash";
pub const RUN_BASH_BACKGROUND: &str = "run_bash_background";
pub const BASH_OUTPUT: &str = "bash_output";
pub const BASH_KILL: &str = "bash_kill";

// Browser
pub const BROWSER_OPEN: &str = "browser_open";
pub const BROWSER_EXTRACT: &str = "browser_extract";
pub const BROWSER_CLICK: &str = "browser_click";
pub const BROWSER_TYPE: &str = "browser_type";
pub const BROWSER_EVAL: &str = "browser_eval";
pub const BROWSER_SCREENSHOT: &str = "browser_screenshot";
pub const BROWSER_CLOSE: &str = "browser_close";
pub const BROWSER_FORGET_LOGIN: &str = "browser_forget_login";
pub const BROWSER_CLEAR_DATA: &str = "browser_clear_data";

// Web
pub const WEB_SEARCH: &str = "web_search";
pub const FETCH_NEWS: &str = "fetch_news";
pub const HTTP_REQUEST: &str = "http_request";
pub const PROXY_REQUEST: &str = "proxy_request";
pub const RELOAD_PROXY_MODULES: &str = "reload_proxy_modules";

// Email
pub const SEND_EMAIL: &str = "send_email";
pub const READ_EMAILS: &str = "read_emails";
pub const READ_EMAIL: &str = "read_email";
pub const CONFIGURE_EMAIL: &str = "configure_email";
pub const SAVE_EMAIL_ATTACHMENT: &str = "save_email_attachment";

// Import
pub const IMPORT_FILE: &str = "import_file";
pub const GIT_CLONE: &str = "git_clone";

// Triggers
pub const CREATE_TRIGGER: &str = "create_trigger";
pub const LIST_TRIGGERS: &str = "list_triggers";
pub const UPDATE_TRIGGER: &str = "update_trigger";
pub const DELETE_TRIGGER: &str = "delete_trigger";
pub const PAUSE_TRIGGER: &str = "pause_trigger";
pub const RESUME_TRIGGER: &str = "resume_trigger";

// Trigger groups (user-visible folders that organize triggers in the panel)
pub const LIST_TRIGGER_GROUPS: &str = "list_trigger_groups";
pub const CREATE_TRIGGER_GROUP: &str = "create_trigger_group";
pub const RENAME_TRIGGER_GROUP: &str = "rename_trigger_group";
pub const REORDER_TRIGGER_GROUPS: &str = "reorder_trigger_groups";
pub const DELETE_TRIGGER_GROUP: &str = "delete_trigger_group";

// Preferences
pub const SET_LANGUAGE: &str = "set_language";
pub const SET_TIMEZONE: &str = "set_timezone";
pub const ENABLE_PUSH_NOTIFICATIONS: &str = "enable_push_notifications";
pub const SET_ENVIRONMENT_VARIABLE: &str = "set_environment_variable";

// Credentials & OAuth
pub const REQUEST_CREDENTIAL: &str = "request_credential";
pub const CONNECT_OAUTH_ACCOUNT: &str = "connect_oauth_account";

// Apps (successor to Skills)
pub const CREATE_APP: &str = "create_app";
pub const LIST_APPS: &str = "list_apps";
pub const EXECUTE_INTENT: &str = "execute_intent";
pub const LOAD_KNOWHOW: &str = "load_knowhow";

// App UI & file refresh (handled by handle_special_tool)
pub const REFRESH_FILE: &str = "refresh_file";
pub const REFRESH_APP: &str = "refresh_app";
pub const CAPTURE_APP: &str = "capture_app";

// Fan-out / child threads (handled by handle_special_tool)
pub const RUN_CODING_AGENT: &str = "run_coding_agent";
pub const RUN_CLAUDE_LEGACY: &str = "run_claude";
pub const RUN_THREAD: &str = "run_thread";

// Memory
pub const CORRECT_MEMORY: &str = "correct_memory";
pub const CORRECT_MEMORY_BY_ID: &str = "correct_memory_by_id";

// Events
pub const EMIT_EVENT: &str = "emit_event";
pub const QUERY_EVENTS: &str = "query_events";
pub const COUNT_EVENTS: &str = "count_events";

// Thread queries (script/trigger-facing introspection)
pub const LIST_THREADS: &str = "list_threads";
pub const COUNT_THREADS: &str = "count_threads";

// Changes (pending coding-agent-proposed changes — list + apply)
pub const LIST_CHANGES: &str = "list_changes";
pub const APPLY_CHANGE: &str = "apply_change";

// Thread Queue (background admission-control policy + live queue)
pub const LIST_THREAD_QUEUE: &str = "list_thread_queue";
pub const UPDATE_THREAD_QUEUE_POLICY: &str = "update_thread_queue_policy";

// Context management
pub const DISMISS_FROM_CONTEXT: &str = "dismiss_from_context";

// Todo list (Lucidos Agent's runtime progress visibility — chat + trigger surfaces only)
pub const TODO_WRITE: &str = "todo_write";

// Notifications & UI
pub const SEND_NOTIFICATION: &str = "send_notification";
pub const READ_NOTIFICATIONS: &str = "read_notifications";
pub const NAVIGATE_UI: &str = "navigate_ui";
pub const ASK_USER_QUESTION: &str = "ask_user_question";

// Image generation
pub const GENERATE_IMAGE: &str = "generate_image";
pub const SAVE_THREAD_IMAGE: &str = "save_thread_image";
// Pull an image posted earlier in the thread back into the model's vision.
pub const VIEW_IMAGE: &str = "view_image";

// MCP server management
pub const SETUP_MCP_SERVER: &str = "setup_mcp_server";
pub const LIST_MCP_SERVERS: &str = "list_mcp_servers";
pub const START_MCP_SERVER: &str = "start_mcp_server";
pub const STOP_MCP_SERVER: &str = "stop_mcp_server";
pub const REMOVE_MCP_SERVER: &str = "remove_mcp_server";

// Repository management
pub const MANAGE_REPOSITORIES: &str = "manage_repositories";

// Plugins
pub const INSTALL_PLUGIN: &str = "install_plugin";
pub const REGISTER_PLUGIN_MARKETPLACE: &str = "register_plugin_marketplace";
pub const CHECK_PLUGIN_UPDATES: &str = "check_plugin_updates";
pub const UPDATE_PLUGIN: &str = "update_plugin";
pub const UNINSTALL_PLUGIN: &str = "uninstall_plugin";
