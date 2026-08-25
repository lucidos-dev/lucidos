//! LLM-facing tool schemas.
//!
//! Each tool is one `ToolDefinition` literal — the description text and
//! parameter shape that the LLM sees. The handlers live in
//! `engine::tools::*`; this module owns only the wire contract.
//!
//! Schemas are grouped per-domain into child modules, each sitting next to
//! the family it describes. [`FAMILIES`] splices them in display order, so the
//! LLM still sees the whole tool surface in one vec. Every row of that table
//! names a [`Gate`]. That is ADR 0088's rule made structural: a family states
//! its gate, or states it has none. The per-domain families:
//!
//! - `browser` — autonomous browser-driving tools
//! - `file` — read/write/edit/list/glob/grep/copy/delete/import
//! - `exec` — run_python(_background), run_bash(_background), bash_output, bash_kill
//! - `email` — configure/send/read/save attachment
//! - `apps` — create/list/refresh/capture + load_knowhow
//! - `notifications` — send (reading/clearing the inbox is the grouped tool below)
//! - `threads`: run_thread, run_coding_agent, follow_up_child_thread
//!   (list/count is the grouped `threads` tool)
//! - `web` — web_search, fetch_news
//! - `proxy` — reload_proxy_modules, proxy_request, http_request
//! - `images` — save_thread_image, generate_image
//! - `misc` — navigate_ui, git_clone, get_backup_status, request_credential,
//!   connect_oauth_account, execute_intent, ask_user_question, todo_write
//!   (env-var management is the grouped `env_vars` tool, see below)
//!
//! Grouped, manifest-driven tools (one tool per domain with an `action` enum,
//! built from `crate::capability_manifest` and spliced by the chat/intent
//! callers, not here): `notifications` (list/mark_read/mark_all_read),
//! `preferences` (get/set), `triggers` (create/list/update/delete/pause/resume),
//! `trigger_groups` (list/create/rename/reorder/delete), `mcp`
//! (setup/list/start/stop/remove), `plugins` (install/register_marketplace/
//! check_updates/update/uninstall), `events` (emit/query/count), `changes`
//! (list/apply), `thread_queue` (list/update_policy; run-now/drop are CLI-only),
//! `memory` (correct/correct_by_id; stats/entries/source are CLI-only),
//! `env_vars` (list/set/delete; `set_environment_variable` is the back-compat
//! alias to `set`), `threads` (list/count; the run_thread / run_coding_agent /
//! follow_up_child_thread family stays standalone),
//! `manage_models` (list/add/enable/disable/remove; the `models` domain),
//! `manage_repositories` (add/list/remove; the `repositories` domain). Their
//! retired flat tool names (`read_notifications`, `create_trigger`,
//! `setup_mcp_server`, `install_plugin`, `emit_event`, `list_changes`,
//! `list_thread_queue`, `correct_memory`, `list_threads`,
//! `set_environment_variable`, …) stay wired as back-compat aliases in
//! `execute_tool`. `manage_models`/`manage_repositories`
//! keep their tool name (schema now manifest-built) and existing handlers.
//!
//! Splitting is purely cosmetic — the LLM still sees the full surface from
//! `get_default_tools()`, in the same order it had before the split.

mod apps;
mod browser;
mod email;
mod exec;
mod file;
mod images;
mod misc;
mod notifications;
mod proxy;
mod threads;
mod web;

use crate::llm::provider::ToolDefinition;

pub use images::{get_image_generation_tool, get_save_thread_image_tool, get_view_image_tool};
pub use misc::get_navigate_ui_tool;
/// The Settings sub-sections a deep link may name. Re-exported so a notification
/// producer's test can hold its own tap destination to this list, rather than
/// keeping a copy that drifts.
#[cfg(test)]
pub(crate) use misc::NAVIGABLE_SETTINGS_VIEWS;
pub use notifications::get_notification_tool;

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

/// What the WORKSPACE is configured to do, as the registry's gates read it.
///
/// A pure function of workspace configuration, and of nothing else: not the
/// thread, not the thread kind, not the caller. That is what keeps every
/// thread in a workspace on one byte-identical array, sharing a single
/// prompt-cache entry (ADR 0088 decision 2).
///
/// Resolved once per turn by `LucidosEngine::read_turn_capabilities`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ToolCapabilities {
    /// The workspace has at least one configured email account.
    pub email_account: bool,
    /// The workspace has at least one intent.
    pub intent: bool,
    /// An image-generation provider is configured.
    pub image_provider: bool,
    /// The *self-curated context mode* is on for this workspace.
    ///
    /// Unlike the three above it opens no family. It CLOSES one. The mode adds
    /// no tool and takes `todo_write` away, because the checklist moved into
    /// the working understanding. A schema billed on every request that
    /// nothing calls is the cost bug the mode exists to fix.
    pub context_mode: bool,
}

impl ToolCapabilities {
    /// Every [`Gate`] open, which is the whole engine-authored surface rather
    /// than one workspace's array. For tests asking whether a schema is
    /// registered at all, which no gate should be able to answer for them.
    ///
    /// `context_mode` is not a gate and is off here. It shapes rather than
    /// gates, and what it does is CLOSE a family, so the widest array is the
    /// one with the mode off.
    pub fn all_open() -> Self {
        Self {
            email_account: true,
            intent: true,
            image_provider: true,
            context_mode: false,
        }
    }
}

/// Why a tool family is, or is not, offered to a workspace.
///
/// Every row of [`FAMILIES`] and [`CHAT_TAIL`] carries one, so a new family
/// cannot arrive resident by omission: the row does not compile without it.
/// [`Gate::Ungated`] is how a family states that it has none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gate {
    /// No gate. Every workspace can act on this family.
    Ungated,
    /// At least one configured email account.
    EmailAccount,
    /// At least one intent.
    Intent,
    /// A configured image-generation provider.
    ImageProvider,
}

impl Gate {
    fn is_open(self, caps: &ToolCapabilities) -> bool {
        match self {
            Gate::Ungated => true,
            Gate::EmailAccount => caps.email_account,
            Gate::Intent => caps.intent,
            Gate::ImageProvider => caps.image_provider,
        }
    }
}

/// How a family's schemas are rendered, once its gate says they are offered.
///
/// Separate from [`Gate`], which answers WHETHER. This answers WHAT SHAPE, and
/// almost every family answers `Fixed`: its bytes are the same in every
/// workspace on this build. `Shaped` is for a family whose schema is itself a
/// function of workspace configuration, which today is `todo_write`: the
/// self-curated context mode takes it away. Reading the capabilities is the
/// only way to vary a schema: an `if` at the splice site is what ADR 0088
/// replaced.
#[derive(Clone, Copy)]
pub enum Build {
    /// Same bytes in every workspace.
    Fixed(fn() -> Vec<ToolDefinition>),
    /// Bytes shaped by workspace configuration.
    Shaped(fn(&ToolCapabilities) -> Vec<ToolDefinition>),
}

impl Build {
    fn render(self, caps: &ToolCapabilities) -> Vec<ToolDefinition> {
        match self {
            Build::Fixed(build) => build(),
            Build::Shaped(build) => build(caps),
        }
    }
}

/// One registry row: the gate deciding whether a workspace is offered this
/// family, and the builder that renders its schemas.
type FamilyRow = (Gate, Build);

/// The chat-agent default tool set, in the exact order the LLM sees it.
///
/// The row order here IS the wire order. Reordering it rewrites the first
/// cache segment for every workspace, so do not.
const FAMILIES: &[FamilyRow] = &[
    (Gate::Ungated, Build::Fixed(file::read_write_edit_tools)), // read_file, write_file, edit_file
    (Gate::Ungated, Build::Fixed(exec::exec_tools)), // run_python(_background), run_bash(_background), bash_output, bash_kill
    (Gate::Ungated, Build::Fixed(file::search_tools)), // list_files, glob_files, grep_files, copy_file, delete_file
    (Gate::Ungated, Build::Fixed(proxy::proxy_tools)), // reload_proxy_modules, proxy_request, http_request
    (Gate::Ungated, Build::Fixed(file::import_file_tools)), // import_file
    (Gate::Ungated, Build::Fixed(misc::git_clone_tools)), // git_clone
    (Gate::Ungated, Build::Fixed(misc::backup_status_tools)), // get_backup_status
    // Spliced from `capability_manifest::llm_tools()` by the chat and intent
    // callers rather than here:
    // - get_preferences/set_preference → grouped `preferences` tool
    // - the trigger and trigger-group family → `triggers` / `trigger_groups`
    // - env-var management → `env_vars`, with set_environment_variable an alias
    (Gate::Ungated, Build::Fixed(web::fetch_news_tools)), // fetch_news
    (Gate::Ungated, Build::Fixed(browser::browser_tools)), // browser_* family
    (Gate::Ungated, Build::Fixed(web::web_search_tools)), // web_search
    (Gate::Ungated, Build::Fixed(misc::request_credential_tools)), // request_credential
    // `configure_email` is the ONLY writer of the first `email_accounts` row,
    // so gating it on having one would make email setup unreachable. The four
    // schemas that operate on an account are the ones the engine would refuse.
    (Gate::Ungated, Build::Fixed(email::configure_email_tools)), // configure_email
    (Gate::EmailAccount, Build::Fixed(email::mailbox_tools)), // send_email, read_emails, read_email, save_email_attachment
    (Gate::Ungated, Build::Fixed(apps::app_tools)), // create_app, list_apps, load_knowhow, refresh_app, capture_app
    (Gate::Ungated, Build::Fixed(misc::connect_oauth_tools)), // connect_oauth_account
    (Gate::Ungated, Build::Fixed(threads::spawn_tools)), // run_thread, run_coding_agent, follow_up_child_thread
    // correct_memory/correct_memory_by_id are the grouped `memory` manifest tool
    // (spliced via capability_manifest::llm_tools()).
    (Gate::Intent, Build::Fixed(misc::execute_intent_tools)), // execute_intent
    // emit/query/count events and list/apply changes are the grouped `events` /
    // `changes` manifest tools (spliced via capability_manifest::llm_tools()).
    // list_threads/count_threads are the grouped `threads` manifest tool (spliced
    // via capability_manifest::llm_tools()); run_thread / run_coding_agent /
    // follow_up_child_thread stay standalone above (threads::spawn_tools()).
    // list_thread_queue/update_thread_queue_policy are the grouped `thread_queue`
    // manifest tool (spliced via capability_manifest::llm_tools()).
    // plugin + MCP management are the grouped `plugins` / `mcp` manifest tools
    // (spliced from capability_manifest::llm_tools() by the chat/intent callers).
    (Gate::Ungated, Build::Fixed(misc::ask_user_question_tools)), // ask_user_question
    (Gate::Ungated, Build::Fixed(misc::await_event_tools)),       // await_event
    (Gate::Ungated, Build::Fixed(misc::event_wait_agent_tools)), // list_event_waits, cancel_event_wait
    // Ungated, so every workspace is offered it. Shaped, because the mode
    // adds no tool and takes this one away: the checklist moved into the
    // working understanding, and two write surfaces for one list is the cost
    // bug twice over.
    (Gate::Ungated, Build::Shaped(misc::todo_write_tools)), // todo_write
];

/// The schemas the chat caller splices AFTER the grouped manifest set, so
/// they cannot live in [`FAMILIES`] without moving on the wire.
///
/// Same table shape for the same reason. `generate_image` is the gate ADR
/// 0088 extends, so it belongs to the one mechanism rather than to an `if`
/// at the splice site.
const CHAT_TAIL: &[FamilyRow] = &[
    (Gate::Ungated, Build::Fixed(misc::navigate_ui_tools)), // navigate_ui
    // view_image re-loads an earlier thread image into vision. It needs no
    // image *generation* provider, so it is ungated unlike `generate_image`.
    (Gate::Ungated, Build::Fixed(images::thread_image_tools)), // save_thread_image, view_image
    (Gate::ImageProvider, Build::Fixed(images::generation_tools)), // generate_image
];

/// Splice one registry table down to the families this workspace can use.
fn offered(rows: &[FamilyRow], caps: &ToolCapabilities) -> Vec<ToolDefinition> {
    rows.iter()
        .filter(|(gate, _)| gate.is_open(caps))
        .flat_map(|(_, build)| build.render(caps))
        .collect()
}

/// The chat-agent default tool set for a workspace: [`FAMILIES`] with every
/// closed gate dropped.
pub fn get_default_tools(caps: &ToolCapabilities) -> Vec<ToolDefinition> {
    offered(FAMILIES, caps)
}

/// The chat-only tail, spliced after `send_notification` and the grouped
/// manifest set. See [`CHAT_TAIL`].
pub fn chat_tail_tools(caps: &ToolCapabilities) -> Vec<ToolDefinition> {
    offered(CHAT_TAIL, caps)
}

#[cfg(test)]
mod tests;
