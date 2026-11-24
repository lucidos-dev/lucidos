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

#![deny(clippy::print_stdout, clippy::print_stderr)]

/// Compile-checked values, with the same timestamped host output as legacy logs.
#[macro_export]
macro_rules! log {
    ($($arg:tt)*) => {
        $crate::diagnostic_log::typed(::lucidos_diagnostic_log::line!($($arg)*))
    };
}

/// Migration bridge: maintainer bundles exclude the whole unstructured message.
#[macro_export]
macro_rules! log_legacy {
    ($($arg:tt)*) => {
        $crate::diagnostic_log::legacy(format!($($arg)*))
    };
}

mod diagnostic_bundle;
mod diagnostic_log;
mod diagnostic_transport;

mod atomic_file;
mod auth;
mod auth_api;
mod boot_failure;
mod boot_phase;
mod broker;
mod build_id;
/// The one test that needs a live engine, run by `./scripts/e2e-api.sh`.
#[cfg(test)]
mod chain_tests;
mod cleanup;
mod cleanup_audit;
mod cleanup_process;
mod control;
mod disposable;
mod engine_hop;
mod engine_signal;
mod error;
mod external_address;
mod file_backup;
mod gateway_event_log;
mod gateway_identity;
mod gateway_protocol;
mod hook_socket;
mod host_logs;
mod installs;
mod local_updates;
mod manage_hop;
mod net_config;
mod next_boot;
#[cfg(test)]
mod notification_count_tests;
mod nudge;
mod oauth_callback;
mod pairing;
mod pairing_qr;
mod pairing_requests;
mod peer_connection;
mod peers;
mod postgres;
#[cfg(test)]
mod preference_test_keys;
mod proxy;
mod registry;
mod relay;
mod relay_probe_cache;
mod relay_timing;
mod release_check;
mod remote_browser;
mod remote_gateways;
mod remote_workspaces;
mod return_access;
mod row_subscription;
#[cfg(test)]
mod row_subscription_tests;
mod server;
mod slowness;
mod stack;
mod stamp_handover;
mod trash;
mod update_channels;
mod update_relay;
mod users_api;
#[cfg(test)]
mod value_pins_tests;
mod workspace_calls;
mod workspace_operations;
mod workspace_order;

/// Lucidos umbrella release version (e.g. "0.7"), sourced from the repo-root
/// `RELEASE` file at compile time. Mirrors the engine executable's `BuildIdentity::release`
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
    let outcome = match boot() {
        Ok(server::RunOutcome::Stopped) => std::process::ExitCode::SUCCESS,
        Ok(server::RunOutcome::AlreadyRunning) => {
            std::process::ExitCode::from(server::ALREADY_RUNNING_EXIT)
        }
        Err(e) => {
            diagnostic_log::boot_failure(&boot_failure_line(e));
            std::process::ExitCode::FAILURE
        }
    };
    lucidos_diagnostic_log::flush(std::time::Duration::from_secs(1));
    outcome
}

fn boot() -> Result<server::RunOutcome, BoxError> {
    // `--version` / `-V` before any heavy startup — machine-readable stdout, so
    // bare `println!` (no timestamp prefix) like the engine's `--version`.
    if std::env::args()
        .skip(1)
        .any(|a| a == "--version" || a == "-V")
    {
        diagnostic_log::probe(format_args!(
            "Lucidos {} (lucidos-gateway {})",
            LUCIDOS_RELEASE,
            env!("CARGO_PKG_VERSION"),
        ));
        return Ok(server::RunOutcome::Stopped);
    }

    // `--build-id` — print the baked build id and exit, BEFORE any runtime/port/PG
    // touch. A running gateway spawns `current_exe --build-id` to learn the on-disk
    // binary's id for the picker's "new gateway available" badge, so this must be a
    // single bare line on stdout (no log prefix), like `--version`. See
    // `server::gateway_build_id` and `docs/plans/2026-06-18-gateway-reload-control.md`.
    if std::env::args().skip(1).any(|a| a == "--build-id") {
        diagnostic_log::probe(format_args!("{}", server::GATEWAY_BUILD_ID));
        return Ok(server::RunOutcome::Stopped);
    }

    // Above the runtime build: both set process env, which is sound only while
    // this process is single-threaded. The take comes first, so the login shell
    // inherits no lock. A test below pins the order.
    let disposable = disposable::from_args(std::env::args_os().skip(1).collect())?;
    let inherited_lock_fd = server::take_inherited_lock_fd();
    let inherited_stamp_keys = stamp_handover::take_inherited();
    if disposable.is_none() {
        lucidos_shell_env::hydrate_login_shell_env(diagnostic_log::legacy);
    }

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_stack_size(WORKER_THREAD_STACK_SIZE)
        .build()?;
    rt.block_on(server::run(
        inherited_lock_fd,
        inherited_stamp_keys,
        disposable,
    ))
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

    /// Pinned on the source, because the property is an ordering inside `boot`.
    #[test]
    fn boot_sets_process_env_before_it_starts_any_threads() {
        let source = include_str!("main.rs");
        let boot_start = source.find("fn boot()").expect("boot() exists");
        let boot_end = boot_start + source[boot_start..].find("\n}\n").expect("boot() ends");
        let boot = &source[boot_start..boot_end];
        let runtime = boot
            .find("tokio::runtime::Builder")
            .expect("boot() builds the runtime");
        let [take, take_keys, hydrate] = [
            "server::take_inherited_lock_fd();",
            "stamp_handover::take_inherited();",
            "lucidos_shell_env::hydrate_login_shell_env(diagnostic_log::legacy);",
        ]
        .map(|env_writer| {
            let at = boot
                .find(env_writer)
                .unwrap_or_else(|| panic!("boot() calls {env_writer}"));
            assert!(
                at < runtime,
                "{env_writer} writes process env, so it must run before the runtime spawns threads"
            );
            at
        });
        assert!(
            take < hydrate && take_keys < hydrate,
            "hydration spawns the login shell, which must not inherit the reload's lock or keys"
        );
    }
}

#[cfg(test)]
mod diagnostic_logging_tests;
