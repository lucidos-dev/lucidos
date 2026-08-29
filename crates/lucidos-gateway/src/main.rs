//! `lucidos-gateway` — the standalone workspace gateway (ADR 0014).
//!
//! A headless reverse-proxy + control plane that fronts N always-on
//! per-workspace `lucidos-engine` stacks, addressed by path prefix `/<slug>/`.
//! It owns the workspace registry (`<app-data>/config/workspaces.json`),
//! provisions + supervises one Postgres + engine stack per workspace (engine
//! bound loopback-only in packaged builds; all interfaces in dev — ADR 0014 "Dev
//! runtime topology"), provisions one database per workspace in a shared
//! Postgres cluster, and reverse-proxies `/<slug>/*` to the matching engine as
//! a **pure streaming forwarder** (strip `/<slug>`, add
//! `X-Forwarded-Prefix: /<slug>/`, forward the response untouched — compression
//! and SSE intact). All gateway-owned surface (workspace picker, control API,
//! health) lives behind the reserved sigil namespace `/~/`.
//!
//! Standalone by design (ADR 0014 §1): NO dependency on `lucidos-engine`, so the
//! only network-facing process links proxy + supervise + registry code, not the
//! engine's heavy core. It spawns the engine binary by path via
//! `LUCIDOS_ENGINE_BIN` (its own `current_exe` is the gateway, so the engine
//! path must be explicit).

/// Timestamped log line, `[pid:N]`-tagged like the engine's. Messages carry
/// their own `[Gateway]` prefix (kept consistent across the crate), so this is a
/// thin wrapper — no module-label derivation. Duplicated from the engine on
/// purpose (ADR 0014 §1: the gateway must not link the engine for a 6-line
/// macro).
#[macro_export]
macro_rules! log {
    ($($arg:tt)*) => {
        println!("{} [pid:{}] {}",
            chrono::Local::now().format("%H:%M:%S"),
            std::process::id(),
            format_args!($($arg)*))
    };
}

mod auth;
mod auth_api;
mod boot_failure;
mod boot_phase;
mod build_id;
mod control;
mod error;
mod file_backup;
mod hook_socket;
mod net_config;
mod next_boot;
mod pairing_qr;
mod postgres;
mod proxy;
mod registry;
mod release_check;
mod server;
mod stack;

/// Lucidos umbrella release version (e.g. "0.7"), sourced from the repo-root
/// `RELEASE` file at compile time. Mirrors `lucidos_engine::LUCIDOS_RELEASE`
/// (duplicated, not shared — ADR 0014 §1).
pub const LUCIDOS_RELEASE: &str = {
    let raw = include_str!("../../../RELEASE").as_bytes();
    let trimmed = raw.trim_ascii_end();
    // SAFETY: input is ASCII (digits + '.' + whitespace); trim keeps valid UTF-8.
    match std::str::from_utf8(trimmed) {
        Ok(s) => s,
        Err(_) => panic!("RELEASE file must be valid UTF-8"),
    }
};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Match the engine's larger worker-thread stack (deep proxy/await chains stay
/// well under it, but keeping parity is free — virtual address space committed
/// lazily).
const WORKER_THREAD_STACK_SIZE: usize = 16 * 1024 * 1024;

/// The token a fatal boot stamps on stderr, and a contract rather than a log
/// nicety. Under the packaged `.app` this stderr is
/// `<app-data>/logs/engine-service.err.log`. The desktop client reads that file
/// back to tell its startup splash why the background service keeps dying.
/// `lucidos-app` cannot link this crate (ADR 0014 §1), so its own test greps
/// this file for the token.
pub const BOOT_FAILED_MARKER: &str = "[gateway] boot failed:";

/// The stderr line a fatal boot writes. Pure, so the marker and the reason stay
/// on one line that the client's parser can split.
fn boot_failure_line(err: impl std::fmt::Display) -> String {
    format!("{BOOT_FAILED_MARKER} {err}")
}

/// Report a fatal boot the way the packaged client can read, rather than as
/// Rust's `Error: "…"` Debug form. That form quotes and escapes the message, so
/// a path-bearing reason reached the log unreadable and reached the user not at
/// all.
fn main() -> std::process::ExitCode {
    match boot() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{}", boot_failure_line(e));
            std::process::ExitCode::FAILURE
        }
    }
}

fn boot() -> Result<(), BoxError> {
    // `--version` / `-V` before any heavy startup — machine-readable stdout, so
    // bare `println!` (no timestamp prefix) like the engine's `--version`.
    if std::env::args()
        .skip(1)
        .any(|a| a == "--version" || a == "-V")
    {
        println!(
            "Lucidos {} (lucidos-gateway {})",
            LUCIDOS_RELEASE,
            env!("CARGO_PKG_VERSION"),
        );
        return Ok(());
    }

    // `--build-id` — print the baked build id and exit, BEFORE any runtime/port/PG
    // touch. A running gateway spawns `current_exe --build-id` to learn the on-disk
    // binary's id for the picker's "new gateway available" badge, so this must be a
    // single bare line on stdout (no log prefix), like `--version`. See
    // `server::gateway_build_id` and `docs/plans/2026-06-18-gateway-reload-control.md`.
    if std::env::args().skip(1).any(|a| a == "--build-id") {
        println!("{}", server::GATEWAY_BUILD_ID);
        return Ok(());
    }

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_stack_size(WORKER_THREAD_STACK_SIZE)
        .build()?;
    rt.block_on(server::run())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fatal_boot_is_one_line_the_client_can_split() {
        let line = boot_failure_line(
            "LUCIDOS_STATIC_DIR is set to /Resources/frontend but its index.html is missing",
        );
        assert_eq!(
            line,
            "[gateway] boot failed: LUCIDOS_STATIC_DIR is set to /Resources/frontend but its \
             index.html is missing"
        );
        // No quoting, no escaping: the reason reads as itself. Rust's own
        // `Error: "…"` form is what this replaced.
        assert!(!line.contains('"'));
        assert_eq!(line.lines().count(), 1);
    }
}
