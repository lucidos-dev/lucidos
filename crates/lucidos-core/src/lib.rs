//! Event persistence, stores, model providers, and shared engine types.

#![deny(clippy::print_stdout, clippy::print_stderr)]

/// The Providers page as copy names it: the one definition for engine strings.
/// A macro, not a `const`, so `concat!` can take it in `const` strings and
/// `match` arms. The frontend's twin is `PROVIDERS_SETTINGS_PATH`.
#[macro_export]
macro_rules! providers_settings {
    () => {
        "Settings → Providers"
    };
}

/// Typed logging preserves host output and stores classified diagnostic values.
#[macro_export]
macro_rules! log {
    (@$label:ident, $($arg:tt)*) => {
        $crate::typed_log_line(stringify!($label), ::lucidos_diagnostic_log::line!($($arg)*))
    };
    ($($arg:tt)*) => {
        $crate::typed_log_line($crate::module_label(module_path!()), ::lucidos_diagnostic_log::line!($($arg)*))
    };
}

pub fn typed_log_line(label: &'static str, line: lucidos_diagnostic_log::Line) {
    log_output::host(
        label,
        &line.render(lucidos_diagnostic_log::Destination::Owner),
    );
    lucidos_diagnostic_log::record(label, line);
}

// All host printing is confined to this logging module.
#[allow(clippy::print_stdout)]
mod log_output {
    pub fn host(label: &str, msg: &str) {
        println!(
            "{} [pid:{}] [{label}] {msg}",
            super::log_stamp(),
            std::process::id()
        );
    }
}

/// Legacy log with auto-derived module label, printed to stdout and
/// handed to the diagnostic log.
/// - `log_legacy!("message")` → auto module from `module_path!()`
/// - `log_legacy!(@Push, "message")` → explicit label override
#[macro_export]
macro_rules! log_legacy {
    (@$label:ident, $($arg:tt)*) => {
        $crate::log_line(stringify!($label), format!($($arg)*))
    };
    ($($arg:tt)*) => {
        $crate::log_line($crate::module_label(module_path!()), format!($($arg)*))
    };
}

/// The body of [`log_legacy!`]: one stdout line, then the same message to the
/// diagnostic log, which never blocks.
pub fn log_line(label: &'static str, msg: String) {
    log_output::host(label, &msg);
    diagnostic_log::record(label, msg);
}

/// The current local time, formatted to start a log line.
pub fn log_stamp() -> impl std::fmt::Display {
    chrono::Local::now().format(lucidos_gateway_constants::LOG_STAMP_FORMAT)
}

#[cfg(test)]
mod log_stamp_tests {
    #[test]
    fn a_log_stamp_is_rfc_3339_with_a_date_and_an_offset() {
        let stamp = super::log_stamp().to_string();
        let parsed = chrono::DateTime::parse_from_rfc3339(&stamp).expect(&stamp);
        assert_eq!(parsed.to_rfc3339(), stamp);
    }
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

pub mod boot_failure;
pub mod boot_report;
pub mod capability_manifest;
pub mod core;
pub mod diagnostic_bundles;
pub mod diagnostic_log;
pub mod engine;
pub mod engine_runtime;
pub mod gateway_auth;
pub mod gateway_users;
pub mod internal_faults;
pub mod llm;
pub mod net_config;
pub mod paths;
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub mod test_support;
