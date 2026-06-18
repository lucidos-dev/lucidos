//! LLM-facing tool schemas.
//!
//! Each tool is one `ToolDefinition` literal — the description text and
//! parameter shape that the LLM sees. The handlers live in
//! `engine::tools::*`; this module owns only the wire contract.
//!
//! Schemas are grouped per-domain into child modules, each sitting next to
//! the family it describes. `get_default_tools()` is the single entry point:
//! it splices each family in display order, so the LLM still sees the whole
//! tool surface in one vec. The per-domain families:
//!
//! - `browser` — autonomous browser-driving tools
//! - `file` — read/write/edit/list/glob/grep/copy/delete/import
//! - `exec` — run_python(_background), run_bash(_background), bash_output, bash_kill
//! - `triggers` — create/list/update/delete/pause/resume + trigger_group ops
//! - `email` — configure/send/read/save attachment
//! - `apps` — create/list/refresh/capture + load_knowhow
//! - `plugins` — install/marketplaces/check/update/uninstall
//! - `mcp` — setup/list/start/stop/remove server
//! - `notifications` — send/read + enable push
//! - `memory` — correct, dismiss-from-context
//! - `events` — emit/query/count
//! - `threads` — run_thread, run_coding_agent, list/count threads
//! - `changes` — list_changes, apply_change
//! - `thread_queue` — list_thread_queue, update_thread_queue_policy
//! - `web` — web_search, fetch_news
//! - `proxy` — reload_proxy_modules, proxy_request, http_request
//! - `images` — save_thread_image, generate_image
//! - `misc` — navigate_ui, manage_repositories, git_clone, set_language,
//!   set_timezone, request_credential, connect_oauth_account, execute_intent,
//!   ask_user_question, todo_write
//!
//! Splitting is purely cosmetic — the LLM still sees the full surface from
//! `get_default_tools()`, in the same order it had before the split.

mod apps;
mod browser;
mod changes;
mod email;
mod events;
mod exec;
mod file;
mod images;
mod mcp;
mod memory;
mod misc;
mod notifications;
mod plugins;
mod proxy;
mod thread_queue;
mod threads;
mod triggers;
mod web;

use crate::llm::provider::ToolDefinition;

pub use images::{get_image_generation_tool, get_save_thread_image_tool};
pub use mcp::get_mcp_tools;
pub use misc::{get_manage_repositories_tool, get_navigate_ui_tool};
pub use notifications::{get_notification_tool, get_read_notifications_tool};

/// Synchronous `run_bash` defaults. Both the JSON schema (described to the
/// LLM via the tool description) and the engine-side enforcement
/// (`engine::tools::bash`) read these so the documented contract stays in
/// sync with the runtime. Owned by `llm::tools` because the schema is the
/// public LLM-facing surface; engine just enforces.
pub(crate) const DEFAULT_TIMEOUT_SECS: u64 = 60;
pub(crate) const MAX_TIMEOUT_SECS: u64 = 300;

/// Background `run_bash_background` defaults. Higher than the synchronous
/// tool because the caller can poll across many turns; the LLM is still
/// bounded by `BG_MAX_TIMEOUT_SECS` to prevent runaway processes.
pub(crate) const BG_DEFAULT_TIMEOUT_SECS: u64 = 600;
pub(crate) const BG_MAX_TIMEOUT_SECS: u64 = 3600;

/// The chat-agent default tool set, in the exact order the LLM sees it. Each
/// family contributes its slice via a child-module builder; the splice order
/// here IS the wire order, so do not reorder the calls.
pub fn get_default_tools() -> Vec<ToolDefinition> {
    let mut tools: Vec<ToolDefinition> = Vec::new();
    tools.extend(file::read_write_edit_tools()); // read_file, write_file, edit_file
    tools.extend(exec::exec_tools()); // run_python(_background), run_bash(_background), bash_output, bash_kill
    tools.extend(file::search_tools()); // list_files, glob_files, grep_files, copy_file, delete_file
    tools.extend(proxy::proxy_tools()); // reload_proxy_modules, proxy_request, http_request
    tools.extend(file::import_file_tools()); // import_file
    tools.extend(misc::git_clone_tools()); // git_clone
    tools.extend(misc::locale_tools()); // set_language, set_timezone
    tools.extend(triggers::trigger_tools()); // create/list/update/delete/pause/resume + trigger_group ops
    tools.extend(web::fetch_news_tools()); // fetch_news
    tools.extend(browser::browser_tools()); // browser_* family
    tools.extend(web::web_search_tools()); // web_search
    tools.extend(notifications::enable_push_tools()); // enable_push_notifications
    tools.extend(misc::request_credential_tools()); // request_credential
    tools.extend(email::email_tools()); // configure/send/read emails + save attachment
    tools.extend(apps::app_tools()); // create_app, list_apps, load_knowhow, refresh_file, refresh_app, capture_app
    tools.extend(misc::connect_oauth_tools()); // connect_oauth_account
    tools.extend(threads::spawn_tools()); // run_thread, run_coding_agent
    tools.extend(memory::correct_memory_tools()); // correct_memory
    tools.extend(misc::execute_intent_tools()); // execute_intent
    tools.extend(events::event_tools()); // emit_event, query_events, count_events
    tools.extend(threads::list_tools()); // list_threads, count_threads
    tools.extend(changes::changes_tools()); // list_changes, apply_change
    tools.extend(thread_queue::thread_queue_tools()); // list_thread_queue, update_thread_queue_policy
    tools.extend(plugins::plugin_tools()); // install/marketplaces/check/update/uninstall plugin
    tools.extend(misc::ask_user_question_tools()); // ask_user_question
    tools.extend(memory::dismiss_from_context_tools()); // dismiss_from_context
    tools.extend(misc::todo_write_tools()); // todo_write
    tools
}

#[cfg(test)]
mod tests;
