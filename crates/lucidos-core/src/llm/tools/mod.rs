mod apps;
mod browser;
mod email;
mod exec;
pub use exec::BASH_OUTPUT_MAX_WAIT_SECS;
mod file;
mod images;
mod judgment;
pub mod misc;
mod notifications;
mod proxy;
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub use proxy::proxy_tools;
mod threads;
mod web;

use crate::llm::provider::ToolDefinition;

pub use images::{get_image_generation_tool, get_save_thread_image_tool, get_view_image_tool};
#[cfg(any(test, feature = "test-support"))]
pub use misc::NAVIGABLE_SETTINGS_VIEWS;
pub use misc::READ_ARG;
pub use notifications::get_notification_tool;
pub use {misc::wake_parent_tool, misc::WAKE_ARG};
pub use {misc::EXPECT_WAKE_MINUTES_ARG, misc::EXPECT_WAKE_REASON_ARG};

pub const DEFAULT_TIMEOUT_SECS: u64 = 60;
pub const MAX_TIMEOUT_SECS: u64 = 300;

pub const BG_DEFAULT_TIMEOUT_SECS: u64 = 600;
pub const BG_MAX_TIMEOUT_SECS: u64 = 3600;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ToolCapabilities {
    pub email_account: bool,
    pub image_provider: bool,
    pub judgment_provider: bool,
    pub context_mode: bool,
    pub memory_tree: bool,
}

impl ToolCapabilities {
    pub fn all_open() -> Self {
        Self {
            email_account: true,
            image_provider: true,
            judgment_provider: true,
            context_mode: false,
            memory_tree: true,
        }
    }
}

pub use lucidos_capabilities::Gate;

pub(crate) fn gate_is_open(gate: Gate, caps: &ToolCapabilities) -> bool {
    match gate {
        Gate::Ungated => true,
        Gate::EmailAccount => caps.email_account,
        Gate::ImageProvider => caps.image_provider,
        Gate::JudgmentProvider => caps.judgment_provider,
        Gate::MemoryTree => caps.memory_tree,
        Gate::MemoryClassic => !caps.memory_tree,
    }
}

#[derive(Clone, Copy)]
pub enum Build {
    Fixed(fn() -> Vec<ToolDefinition>),
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

type FamilyRow = (Gate, Build);

/// INVARIANT: The chat-agent default tool set, in the exact order the LLM sees it.
///
/// The row order here IS the wire order. Reordering it rewrites the first
/// cache segment for every workspace, so do not.
const FAMILIES: &[FamilyRow] = &[
    (Gate::Ungated, Build::Fixed(file::read_write_edit_tools)),
    (Gate::Ungated, Build::Fixed(exec::exec_tools)),
    (Gate::Ungated, Build::Fixed(file::search_tools)),
    (Gate::Ungated, Build::Fixed(proxy::proxy_tools)),
    (Gate::Ungated, Build::Fixed(file::import_file_tools)),
    (Gate::Ungated, Build::Fixed(misc::git_clone_tools)),
    (Gate::Ungated, Build::Fixed(misc::backup_status_tools)),
    (Gate::Ungated, Build::Fixed(web::fetch_news_tools)),
    (Gate::Ungated, Build::Fixed(browser::browser_tools)),
    (Gate::Ungated, Build::Fixed(web::web_search_tools)),
    (Gate::Ungated, Build::Fixed(misc::request_credential_tools)),
    // INVARIANT: `configure_email` is the ONLY writer of the first `email_accounts` row,
    // so gating it on having one would make email setup unreachable. The four
    // schemas that operate on an account are the ones the engine would refuse.
    (Gate::Ungated, Build::Fixed(email::configure_email_tools)),
    (Gate::EmailAccount, Build::Fixed(email::mailbox_tools)),
    (Gate::Ungated, Build::Fixed(apps::app_tools)),
    (Gate::Ungated, Build::Fixed(misc::connect_oauth_tools)),
    (Gate::Ungated, Build::Fixed(threads::spawn_tools)),
    (Gate::Ungated, Build::Fixed(misc::ask_user_question_tools)),
    (Gate::Ungated, Build::Fixed(misc::await_event_tools)),
    (Gate::Ungated, Build::Fixed(misc::event_wait_agent_tools)),
    (Gate::Ungated, Build::Shaped(misc::todo_write_tools)), // INVARIANT: todo_write
    // INVARIANT: Last, so opening this gate moves no family already on the wire. It is
    // the only row whose gate a workspace opens by configuring a provider that
    // holds no conversation.
    (
        Gate::JudgmentProvider,
        Build::Fixed(judgment::judgment_tools),
    ),
];

const CHAT_TAIL: &[FamilyRow] = &[
    (Gate::Ungated, Build::Fixed(misc::navigate_ui_tools)),
    (Gate::Ungated, Build::Fixed(images::thread_image_tools)),
    (Gate::ImageProvider, Build::Fixed(images::generation_tools)),
    // INVARIANT: Last, so adding it moved no tool already on the wire. A chat turn and a
    // trigger run both splice this tail.
    (Gate::Ungated, Build::Fixed(misc::request_read_tools)),
    // INVARIANT: After request_read, so adding it moved no tool already on the wire.
    (Gate::Ungated, Build::Fixed(misc::expect_wake_tools)),
];

fn offered(rows: &[FamilyRow], caps: &ToolCapabilities) -> Vec<ToolDefinition> {
    rows.iter()
        .filter(|(gate, _)| gate_is_open(*gate, caps))
        .flat_map(|(_, build)| build.render(caps))
        .collect()
}

pub fn get_default_tools(caps: &ToolCapabilities) -> Vec<ToolDefinition> {
    offered(FAMILIES, caps)
}

pub fn chat_tail_tools(caps: &ToolCapabilities) -> Vec<ToolDefinition> {
    offered(CHAT_TAIL, caps)
}

#[cfg(test)]
mod tests;
