//! Lucidos Engine - Core cognitive operating system engine
//!
//! This crate provides the core functionality for Lucidos including:
//! - Event sourcing with PostgreSQL
//! - Artifact management with Git
//! - Memory indexing with vector search
//! - LLM integration with tool calling
//! - Python runtime execution

/// Timestamped log with auto-derived module label.
/// - `log!("message")` → auto module from `module_path!()`
/// - `log!(@Push, "message")` → explicit label override
#[macro_export]
macro_rules! log {
    (@$label:ident, $($arg:tt)*) => {
        println!("{} [pid:{}] [{}] {}",
            chrono::Local::now().format("%H:%M:%S"),
            std::process::id(),
            stringify!($label),
            format_args!($($arg)*))
    };
    ($($arg:tt)*) => {
        println!("{} [pid:{}] [{}] {}",
            chrono::Local::now().format("%H:%M:%S"),
            std::process::id(),
            $crate::module_label(module_path!()),
            format_args!($($arg)*))
    };
}

pub fn module_label(path: &str) -> &str {
    let segment = match path.rfind("::") {
        Some(i) => &path[i + 2..],
        None => path,
    };
    match segment {
        "api" => "API",
        "engine" => "Engine",
        "pinned_apps" => "Apps",
        "store" => "Store",
        "events" => "Events",
        "artifacts" => "Artifacts",
        "credentials" => "Credentials",
        "devices" => "Devices",
        "preferences" => "Preferences",
        "changes" => "Changes",
        "scheduler" | "persistence" | "tasks" | "user_tasks" | "config" | "triggers"
        | "condition" => "Scheduler",
        "push" | "notifications" => "Push",
        "memory" | "extractor" | "pgvector" | "fastembed" | "provider" => "Memory",
        "vertex" | "openai" => "LLM",
        "mcp" | "mcp_servers" | "client" => "MCP",
        "dev_proxy" => "DevProxy",
        "browser" | "browser_consent" => "Browser",
        "chat" | "agentic_loop" => "Engine",
        "context" | "document" | "types" => "Engine",
        "claude_code" => "Engine",
        "files" | "http" | "import" | "python" | "web" => "Engine",
        "email" => "Engine",
        "populate_memory" => "Populate",
        "lucidos_engine" => "Engine",
        _ => segment,
    }
}

pub mod api;
pub mod core;
pub mod dev_proxy;
pub mod engine;
pub mod llm;
pub mod mcp;
pub mod memory;
#[cfg(test)]
mod migration_tests;
pub mod paths;
pub mod runtime;
pub mod scheduler;
#[cfg(test)]
mod test_support;
pub mod triggers;

pub use engine::LucidosEngine;

/// Lucidos umbrella release version (e.g. "0.7"), distinct from per-crate
/// Cargo.toml semvers. Sourced from the repo-root `RELEASE` file at compile
/// time. `trim_ascii_end()` strips the trailing newline at const time.
pub const LUCIDOS_RELEASE: &str = {
    let raw = include_str!("../../../RELEASE").as_bytes();
    let trimmed = raw.trim_ascii_end();
    // SAFETY: input is ASCII (digits + '.' + whitespace); trim_ascii_end keeps
    // it valid UTF-8.
    match std::str::from_utf8(trimmed) {
        Ok(s) => s,
        Err(_) => panic!("RELEASE file must be valid UTF-8"),
    }
};

/// True when this build was made from a commit *after* the one that last
/// bumped RELEASE — i.e. the running code is past the published v<RELEASE>
/// snapshot. Set by build.rs from `git log -- RELEASE`. Defaults to false
/// when git is unavailable (shipped install, missing history).
pub const LUCIDOS_RELEASE_DIRTY: bool = match option_env!("LUCIDOS_RELEASE_DIRTY") {
    // `==` on `&str` isn't const-stable, so byte-compare the literal "true".
    Some(s) => {
        let b = s.as_bytes();
        b.len() == 4 && b[0] == b't' && b[1] == b'r' && b[2] == b'u' && b[3] == b'e'
    }
    None => false,
};

#[cfg(test)]
pub(crate) mod test_util {
    use crate::memory::FastEmbedProvider;
    use std::sync::OnceLock;

    /// Shared FastEmbedProvider across all test modules.
    /// Avoids lock file contention when multiple tests initialize the model.
    pub fn shared_embedder() -> &'static FastEmbedProvider {
        static PROVIDER: OnceLock<FastEmbedProvider> = OnceLock::new();
        PROVIDER.get_or_init(|| FastEmbedProvider::new().unwrap())
    }
}
