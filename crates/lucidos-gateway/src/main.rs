#![deny(clippy::print_stdout, clippy::print_stderr)]

#[macro_export]
macro_rules! log {
    ($($arg:tt)*) => {
        $crate::diagnostic_log::typed(::lucidos_diagnostic_log::line!($($arg)*))
    };
}

#[macro_export]
macro_rules! log_legacy {
    ($($arg:tt)*) => {
        $crate::diagnostic_log::legacy(format!($($arg)*))
    };
}

mod device_names_api;
mod device_splices;
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
#[cfg(test)]
mod chain_tests;
mod cleanup;
mod cleanup_audit;
mod cleanup_process;
mod control;
mod control_jobs;
mod disposable;
mod engine_builds;
mod engine_hop;
mod engine_signal;
mod error;
mod external_address;
mod file_backup;
mod frontend_preview;
mod gateway_event_log;
mod gateway_hop;
mod gateway_identity;
mod gateway_protocol;
mod hook_socket;
mod host_logs;
mod installs;
mod local_updates;
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
mod restore_upload;
mod return_access;
mod row_subscription;
#[cfg(test)]
mod row_subscription_tests;
mod server;
mod slowness;
mod stack;
mod stamp_handover;
mod tailnet_status;
mod trash;
mod update_channels;
mod update_relay;
mod users_api;
#[cfg(test)]
mod value_pins_tests;
mod workspace_calls;
mod workspace_operations;
mod workspace_order;

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

const WORKER_THREAD_STACK_SIZE: usize = 16 * 1024 * 1024;

pub const BOOT_FAILED_MARKER: &str = "[gateway] boot failed:";

fn boot_failure_line(err: impl std::fmt::Display) -> String {
    format!("{BOOT_FAILED_MARKER} {err}")
}

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

    if std::env::args().skip(1).any(|a| a == "--build-id") {
        diagnostic_log::probe(format_args!("{}", server::GATEWAY_BUILD_ID));
        return Ok(server::RunOutcome::Stopped);
    }

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
        assert!(!line.contains('"'));
        assert_eq!(line.lines().count(), 1);
    }

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
