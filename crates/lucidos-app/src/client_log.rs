//! Sanitized, classified diagnostics for the packaged desktop shell.
//! Raw host output stays with the launching process; it is never a bundle source.
use std::path::{Path, PathBuf};

use lucidos_diagnostic_log as shared;

pub(crate) fn dir_for(app_data: &Path) -> PathBuf {
    app_data.join("client-diagnostic-log")
}

/// Source development keeps its terminal and never writes the packaged app's
/// data directory. The service owns a separate log and starts before this path.
pub(crate) fn install() {
    if tauri::is_dev() || crate::launch_role(std::env::args_os()) == crate::LaunchRole::Service {
        return;
    }
    let result = crate::desktop::app_data_dir_from_env().and_then(|root| {
        lucidos_home::ensure_storage_root(&root)?;
        start(&root)
    });
    if let Err(error) = result {
        shared::stderr::host_error(&format!("[client] no diagnostic log ({error})"));
    }
}

fn start(app_data: &Path) -> std::io::Result<()> {
    if shared::started() {
        return Ok(());
    }
    register_environment_secrets();
    shared::start(&dir_for(app_data))?;
    shared::install_panic_hook();
    shared::stderr::install()?;
    shared::stderr::install_stdout()?;
    shared::record(
        "Client",
        shared::line!("launch starting pid={}", std::process::id()),
    );
    Ok(())
}

/// A detached relaunch watcher must not depend on the dying client's pipe
/// reader. Its small headless helper owns a separate rotating sink, avoiding
/// concurrent writers to the client's segments.
pub fn capture_relaunch() {
    register_environment_secrets();
    let mut input = std::io::stdin().lock();
    let result = crate::desktop::app_data_dir_from_env()
        .and_then(|root| capture_relaunch_at(&root, &mut input));
    if let Err(error) = result {
        shared::stderr::host_error(&format!("[client] no relaunch diagnostic log ({error})"));
        // Still drain: a logging failure must never SIGPIPE the relaunch watcher.
        let _ = std::io::copy(&mut input, &mut std::io::sink());
    }
}

fn capture_relaunch_at(root: &Path, input: impl std::io::Read) -> std::io::Result<()> {
    lucidos_home::ensure_storage_root(root)?;
    // Rapid successive relaunches may overlap helpers. Serialize their writers,
    // retaining the fixed two-segment cap rather than creating per-run files.
    let _owner =
        lucidos_owner_lock::OwnerLock::acquire(&root.join("client-relaunch-diagnostic-log.lock"))?;
    shared::start(&root.join("client-relaunch-diagnostic-log"))?;
    shared::stderr::capture_reader("Relaunch", input);
    flush();
    Ok(())
}

fn register_environment_secrets() {
    shared::sanitize::register_secret_values(std::env::vars_os().filter_map(|(name, value)| {
        secret_env_name(name.to_str()?)
            .then(|| value.into_string().ok())
            .flatten()
    }));
    if let Some(token) = lucidos_local_token::read() {
        secret(token);
    }
}

/// Spawn capture before the watcher, so the watcher always has a reader after
/// this client exits. The helper never initializes Tauri or takes a client lock.
#[cfg(target_os = "macos")]
pub(crate) fn relaunch_output(exe: &Path) -> std::io::Result<std::process::Stdio> {
    use std::process::{Command, Stdio};
    let mut helper = Command::new(exe)
        .arg("--capture-client-relaunch-log")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    Ok(Stdio::from(
        helper.stdin.take().expect("piped helper stdin"),
    ))
}

fn secret_env_name(name: &str) -> bool {
    let name = name.to_ascii_uppercase();
    if matches!(
        name.as_str(),
        "GOOGLE_APPLICATION_CREDENTIALS"
            | "LUCIDOS_TLS_KEY"
            | "LUCIDOS_PREVIEW_TLS_KEY"
            | "LUCIDOS_PREVIEW_USER_TLS_KEY"
    ) {
        return false;
    }
    matches!(
        name.as_str(),
        "PGPASSWORD" | "DATABASE_URL" | "LUCIDOS_DATABASE_URL"
    ) || name.starts_with("CRED_")
        || name.starts_with("OAUTH_")
        || [
            "_TOKEN",
            "_SECRET",
            "_PASSWORD",
            "_PASSWD",
            "_KEY",
            "_PAT",
            "_CREDENTIALS",
        ]
        .iter()
        .any(|suffix| name.ends_with(suffix))
}

pub(crate) fn secret(value: String) -> String {
    shared::sanitize::register_secret_values([value.clone()]);
    value
}

pub(crate) fn flush() {
    shared::flush(std::time::Duration::from_secs(1));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relaunch_capture_survives_the_client_process() {
        use std::io::Write;
        use std::process::{Command, Stdio};
        const CHILD: &str = "CLIENT_RELAUNCH_TEST_ROOT";
        const HELPER: &str = "CLIENT_RELAUNCH_TEST_HELPER";
        if let Some(root) = std::env::var_os(CHILD) {
            if std::env::var_os(HELPER).is_some() {
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
                while !Path::new(&root).join("owner-exited").exists() {
                    assert!(std::time::Instant::now() < deadline, "owner did not exit");
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                register_environment_secrets();
                capture_relaunch_at(Path::new(&root), std::io::stdin().lock()).unwrap();
            } else {
                // This child must outlive the simulated GUI process. Waiting
                // here would deadlock: the test releases it after this exits.
                #[allow(clippy::zombie_processes)]
                let mut helper = Command::new(std::env::current_exe().unwrap())
                    .args([
                        "--exact",
                        "client_log::tests::relaunch_capture_survives_the_client_process",
                        "--nocapture",
                    ])
                    .env(HELPER, "1")
                    .stdin(Stdio::piped())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .unwrap();
                writeln!(
                    helper.stdin.take().unwrap(),
                    "PRIVATE_RELAUNCH_MARKER opaque-client-credential"
                )
                .unwrap();
                // The process exits without waiting for capture, as the GUI does.
            }
            return;
        }
        let root = tempfile::tempdir().unwrap();
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "client_log::tests::relaunch_capture_survives_the_client_process",
                "--nocapture",
            ])
            .env(CHILD, root.path())
            .env("CRED_DIAGNOSTIC_TEST", "opaque-client-credential")
            .output()
            .unwrap();
        assert!(output.status.success());
        std::fs::write(root.path().join("owner-exited"), b"").unwrap();
        let path = root
            .path()
            .join("client-relaunch-diagnostic-log")
            .join(shared::writer::FILE_NAME);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let text = std::fs::read_to_string(&path).unwrap_or_default();
            if text.contains("PRIVATE_RELAUNCH_MARKER") {
                assert!(!text.contains("opaque-client-credential"));
                for line in text.lines() {
                    let record: shared::writer::Record = serde_json::from_str(line).unwrap();
                    assert!(record.ts.ends_with('Z'));
                    assert!(!record
                        .render(shared::Destination::Maintainer)
                        .contains("PRIVATE_"));
                }
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "detached capture lost"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    #[test]
    fn shell_logging_wiring() {
        if let Some(root) = std::env::var_os("CLIENT_DIAGNOSTIC_TEST_ROOT") {
            start(Path::new(&root)).unwrap();
            if std::env::var_os("CLIENT_DIAGNOSTIC_PANIC").is_some() {
                panic!("PRIVATE_CLIENT_PANIC_MARKER");
            }
            use std::io::Write;
            writeln!(
                std::io::stderr(),
                "PRIVATE_STDERR_MARKER opaque-client-credential"
            )
            .unwrap();
            writeln!(std::io::stdout(), "PRIVATE_STDOUT_MARKER").unwrap();
            let path = dir_for(Path::new(&root)).join(shared::writer::FILE_NAME);
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            loop {
                let text = std::fs::read_to_string(&path).unwrap_or_default();
                if text.contains("PRIVATE_STDERR_MARKER") && text.contains("PRIVATE_STDOUT_MARKER")
                {
                    break;
                }
                assert!(std::time::Instant::now() < deadline, "missing captures");
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            flush();
            return;
        }
        let root = tempfile::tempdir().unwrap();
        let output = child(root.path(), false);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stderr).contains("opaque-client-credential"));
        assert!(String::from_utf8_lossy(&output.stdout).contains("PRIVATE_STDOUT_MARKER"));
        assert_records(root.path(), "PRIVATE_STDERR_MARKER");
        assert!(
            !root.path().join("logs/client.log").exists(),
            "no raw host-log copy"
        );
    }

    fn child(root: &Path, panic: bool) -> std::process::Output {
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "client_log::tests::shell_logging_wiring",
                "--nocapture",
            ])
            .env("CLIENT_DIAGNOSTIC_TEST_ROOT", root)
            .env("CRED_DIAGNOSTIC_TEST", "opaque-client-credential");
        if panic {
            command.env("CLIENT_DIAGNOSTIC_PANIC", "1");
        }
        command.output().unwrap()
    }

    fn assert_records(root: &Path, marker: &str) {
        let text = std::fs::read_to_string(dir_for(root).join(shared::writer::FILE_NAME)).unwrap();
        assert!(text.contains(marker));
        assert!(!text.contains("opaque-client-credential"));
        for line in text.lines() {
            let record: shared::writer::Record = serde_json::from_str(line).unwrap();
            assert!(record.ts.ends_with('Z'));
            assert!(!record
                .render(shared::Destination::Maintainer)
                .contains("PRIVATE_"));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir_for(root).join(shared::writer::FILE_NAME))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn fatal_shell_panic_survives_process_exit() {
        let root = tempfile::tempdir().unwrap();
        assert!(!child(root.path(), true).status.success());
        assert_records(root.path(), "PRIVATE_CLIENT_PANIC_MARKER");
    }

    #[test]
    fn credentials_are_distinct_from_configuration_paths() {
        for name in [
            "CRED_EXAMPLE",
            "OAUTH_EXAMPLE",
            "GH_TOKEN",
            "PGPASSWORD",
            "DATABASE_URL",
        ] {
            assert!(secret_env_name(name));
        }
        for name in [
            "HOME",
            "PATH",
            "GOOGLE_APPLICATION_CREDENTIALS",
            "LUCIDOS_TLS_KEY",
        ] {
            assert!(!secret_env_name(name));
        }
        assert_eq!(
            dir_for(Path::new("/srv/app")),
            Path::new("/srv/app/client-diagnostic-log")
        );
    }
}
