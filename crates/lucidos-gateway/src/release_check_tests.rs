//! Tests for the release check. Every outbound-request test runs against a
//! throwaway loopback origin, so the real lucidos.dev is never touched.

use super::*;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

// ── The fail-closed gate ────────────────────────────────────────────────────

/// A directory that looks like a Lucidos checkout to `repo_root_above`.
fn checkout(root: &Path) -> PathBuf {
    std::fs::create_dir_all(root.join("scripts")).unwrap();
    std::fs::write(root.join("scripts/web-dev.sh"), "#!/bin/bash\n").unwrap();
    root.join(".launch/debug/plain/lucidos-gateway")
}

#[test]
fn only_a_packaged_binary_outside_a_checkout_may_poll() {
    let dir = tempfile::tempdir().unwrap();
    // Separate trees: the marker must not sit above the installed path too.
    let in_checkout = checkout(&dir.path().join("repo"));
    let installed = dir
        .path()
        .join("install/runtime/lucidos-1.2.3-x/lucidos-gateway");

    assert!(deployment_is_installed(true, Some(&installed)));
    // Every other combination refuses.
    assert!(!deployment_is_installed(false, Some(&installed)));
    assert!(!deployment_is_installed(true, Some(&in_checkout)));
    assert!(!deployment_is_installed(false, Some(&in_checkout)));
    assert!(!deployment_is_installed(true, None));
    assert!(!deployment_is_installed(false, None));
}

#[test]
fn a_dev_gateway_never_polls_even_with_the_env_var_set() {
    // The env var is the half an operator can set by hand. The checkout test is
    // what keeps a maintainer's own work out of the numbers.
    let dir = tempfile::tempdir().unwrap();
    assert!(!deployment_is_installed(true, Some(&checkout(dir.path()))));
}

// ── Install shapes ──────────────────────────────────────────────────────────

#[test]
fn install_shape_reads_the_executable_path() {
    let bundle = Path::new("/Applications/Lucidos.app/Contents/Resources/lucidos-gateway");
    assert_eq!(install_shape(bundle), Some(InstallShape::DesktopApp));

    let stem = Path::new(
        "/home/u/.lucidos/runtime/lucidos-1.2.3-x86_64-unknown-linux-gnu/lucidos-gateway",
    );
    assert_eq!(install_shape(stem), Some(InstallShape::InstallerRerun));
    // The `current` symlink form resolves to the same shape.
    let current = Path::new("/home/u/.lucidos/runtime/current/lucidos-gateway");
    assert_eq!(install_shape(current), Some(InstallShape::InstallerRerun));

    assert_eq!(
        install_shape(Path::new("/usr/local/bin/lucidos-gateway")),
        None
    );
    assert_eq!(install_shape(Path::new("lucidos-gateway")), None);
}

#[test]
fn install_shape_wire_values_are_the_two_the_frontend_switches_on() {
    assert_eq!(InstallShape::DesktopApp.as_str(), "desktop-app");
    assert_eq!(InstallShape::InstallerRerun.as_str(), "installer-rerun");
}

#[test]
fn installer_command_carries_the_live_slug_and_a_non_default_prefix() {
    let default = Path::new("/home/u/.lucidos");
    assert_eq!(
        installer_command(Path::new("/home/u/.lucidos/work"), Some(default)).unwrap(),
        "curl -fsSL https://lucidos.dev/install.sh | sh -s -- --name work"
    );
    // A prefix that is not the default has to be spelled out, or the re-run
    // would install a second instance somewhere else.
    assert_eq!(
        installer_command(Path::new("/srv/lucidos/box"), Some(default)).unwrap(),
        "curl -fsSL https://lucidos.dev/install.sh | sh -s -- --name box --prefix /srv/lucidos"
    );
}

#[test]
fn installer_command_quotes_a_path_a_shell_would_split() {
    let cmd = installer_command(Path::new("/srv/my lucidos/box"), None).unwrap();
    assert!(cmd.ends_with("--prefix '/srv/my lucidos'"), "{cmd}");
    let quoted = installer_command(Path::new("/srv/it's/box"), None).unwrap();
    assert!(quoted.ends_with(r#"--prefix '/srv/it'\''s'"#), "{quoted}");
}

// ── The request ─────────────────────────────────────────────────────────────

#[test]
fn every_published_target_produces_a_well_formed_url() {
    for platform in ["macos", "linux"] {
        for arch in ["aarch64", "x86_64"] {
            let url = check_url(UPDATE_CHECK_ORIGIN, platform, arch, "1.2.3");
            assert_eq!(
                url,
                format!(
                    "https://lucidos.dev/api/update-check\
                     ?platform={platform}&arch={arch}&version=1.2.3"
                )
            );
        }
    }
}

#[test]
fn the_url_carries_exactly_three_parameters() {
    let url = check_url("https://x/y", "linux", "x86_64", "1.2.3");
    let query = url.split_once('?').unwrap().1;
    let keys: Vec<&str> = query
        .split('&')
        .map(|p| p.split_once('=').unwrap().0)
        .collect();
    assert_eq!(keys, vec!["platform", "arch", "version"]);
}

#[test]
fn platform_and_arch_keys_cover_what_we_publish_and_nothing_else() {
    assert_eq!(platform_key("macos"), Some("macos"));
    assert_eq!(platform_key("linux"), Some("linux"));
    assert_eq!(platform_key("windows"), None);
    assert_eq!(arch_key("aarch64"), Some("aarch64"));
    assert_eq!(arch_key("x86_64"), Some("x86_64"));
    assert_eq!(arch_key("riscv64"), None);
}

#[test]
fn this_host_is_a_target_we_publish_for() {
    // If this ever fails, the gateway is being built for a platform with no
    // release, and the check must stay silent there rather than guess.
    assert!(platform_key(std::env::consts::OS).is_some());
    assert!(arch_key(std::env::consts::ARCH).is_some());
}

// ── The answer ──────────────────────────────────────────────────────────────

#[test]
fn parse_response_reads_a_version_and_tolerates_unknown_fields() {
    assert_eq!(
        parse_response(r#"{"version":"1.3.0","unknown":true}"#).unwrap(),
        Some(PublishedRelease {
            version: "1.3.0".to_string(),
            notes: None
        })
    );
    assert_eq!(parse_response(r#"{"version":null}"#).unwrap(), None);
    assert_eq!(parse_response("{}").unwrap(), None);
    assert_eq!(parse_response(r#"{"version":"  "}"#).unwrap(), None);
}

/// Notes are optional in the contract. Present, they give the offer its
/// "What's new" link; absent or blank, the offer simply has none.
#[test]
fn parse_response_carries_notes_when_the_origin_has_them() {
    assert_eq!(
        parse_response(r#"{"version":"1.3.0","notes":"- a thing\n- another"}"#)
            .unwrap()
            .unwrap()
            .notes
            .as_deref(),
        Some("- a thing\n- another")
    );
    assert_eq!(
        parse_response(r#"{"version":"1.3.0","notes":"  "}"#)
            .unwrap()
            .unwrap()
            .notes,
        None
    );
}

#[test]
fn a_soft_404_is_an_error_never_up_to_date() {
    // Cloudflare Pages answers an unknown path with the landing page at 200.
    let html = "<!DOCTYPE html>\n<html><body>Lucidos</body></html>";
    assert!(parse_response(html).is_err());
    assert!(parse_response("  \n<html>").is_err());
    assert!(parse_response("").is_err());
    assert!(parse_response(r#"{"version":"0.29"#).is_err());
}

#[test]
fn version_is_newer_compares_numerically_and_refuses_junk() {
    assert!(version_is_newer("1.3.0", "1.2.0"));
    assert!(version_is_newer("1.2.1", "1.2.0"));
    assert!(version_is_newer("2.0", "1.99.99"));
    assert!(!version_is_newer("1.2.0", "1.2.0"));
    assert!(!version_is_newer("1.2.0", "1.3.0"));
    // Padding: a shorter version equals one with trailing zeros.
    assert!(!version_is_newer("1.2", "1.2.0"));
    assert!(version_is_newer("1.2.1", "1.2"));
    // An unreadable version offers nothing, rather than offering everything.
    assert!(!version_is_newer("unknown", "1.2.0"));
    assert!(!version_is_newer("1.3.0", "unknown"));
}

// ── The preference file ─────────────────────────────────────────────────────

#[test]
fn a_missing_or_malformed_preference_defaults_to_enabled() {
    assert_eq!(parse_updates_toml(""), UpdatesToml::default());
    assert_eq!(parse_updates_toml("garbage = = ="), UpdatesToml::default());
    assert_eq!(parse_updates_toml("[release_check"), UpdatesToml::default());
    assert_eq!(read_updates_toml(None), UpdatesToml::default());
    // The check is functional rather than telemetry, so it is on by default and
    // no click stands in front of it (ADR 0139).
    assert!(UpdatesToml::default().enabled);
}

#[test]
fn a_section_that_names_nothing_keeps_the_default() {
    assert!(parse_updates_toml("[release_check]\n").enabled);
}

/// A file written before ADR 0139 still carries `notice_acknowledged`. The raw
/// deserialize refuses no unknown field, so the file parses and its `enabled`
/// is honoured, rather than warning and falling back to the defaults.
#[test]
fn a_file_carrying_the_removed_field_still_parses() {
    // The stale value that used to mean "polls nothing". It now polls, because
    // the field is gone and `enabled` defaults true.
    let never_answered = parse_updates_toml("[release_check]\nnotice_acknowledged = false\n");
    assert_eq!(never_answered, UpdatesToml { enabled: true });

    // An answered notice beside an explicit off switch keeps the off switch.
    let turned_off =
        parse_updates_toml("[release_check]\nenabled = false\nnotice_acknowledged = true\n");
    assert_eq!(turned_off, UpdatesToml { enabled: false });
}

#[test]
fn render_then_parse_round_trips() {
    for cfg in [
        UpdatesToml { enabled: false },
        UpdatesToml { enabled: true },
    ] {
        assert_eq!(parse_updates_toml(&render_updates_toml(&cfg)), cfg);
    }
}

#[test]
fn writing_the_preference_is_atomic_and_readable_back() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested/updates.toml");
    let cfg = UpdatesToml { enabled: false };
    write_updates_toml(Some(&path), &cfg).unwrap();
    assert_eq!(read_updates_toml(Some(&path)), cfg);
    // No temp file is left behind by the rename.
    assert!(!path.with_extension("toml.tmp").exists());
}

#[test]
fn writing_without_a_home_is_an_error_not_a_panic() {
    assert!(write_updates_toml(None, &UpdatesToml::default()).is_err());
}

// ── Live requests against a throwaway origin ────────────────────────────────

struct Origin {
    url: String,
    seen: Arc<Mutex<Vec<String>>>,
}

impl Origin {
    fn requests(&self) -> usize {
        self.seen.lock().unwrap().len()
    }
}

fn http_ok(body: &str) -> String {
    format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\n\
         content-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    )
}

/// A loopback origin that records each request and answers with `response`.
async fn origin_serving(response: String, delay: Duration) -> Origin {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let recorded = seen.clone();
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            let body = response.clone();
            let recorded = recorded.clone();
            tokio::spawn(async move {
                let mut buf = [0u8; 8192];
                let n = sock.read(&mut buf).await.unwrap_or(0);
                recorded
                    .lock()
                    .unwrap()
                    .push(String::from_utf8_lossy(&buf[..n]).into_owned());
                if !delay.is_zero() {
                    tokio::time::sleep(delay).await;
                }
                let _ = sock.write_all(body.as_bytes()).await;
                let _ = sock.shutdown().await;
            });
        }
    });
    Origin {
        url: format!("http://{addr}/api/update-check"),
        seen,
    }
}

/// An installed deployment whose preference lives in `dir`.
fn deployment_in(dir: &Path, enabled: bool) -> Deployment {
    let config = dir.join("updates.toml");
    write_updates_toml(Some(&config), &UpdatesToml { enabled }).unwrap();
    Deployment {
        packaged: true,
        exe: Some(dir.join("runtime/lucidos-1.2.3-host/lucidos-gateway")),
        app_data: dir.join("default"),
        default_prefix: Some(dir.to_path_buf()),
        config_path: Some(config),
    }
}

#[tokio::test]
async fn the_request_carries_three_parameters_and_no_cookie() {
    let dir = tempfile::tempdir().unwrap();
    let origin = origin_serving(http_ok(r#"{"version":"99.0.0"}"#), Duration::ZERO).await;
    let check =
        ReleaseCheck::with_origin(&deployment_in(dir.path(), true), &origin.url, POLL_INTERVAL);

    let snapshot = check.refresh(false).await;
    assert_eq!(snapshot["latest"]["version"], "99.0.0");

    let seen = origin.seen.lock().unwrap();
    let request = seen.first().expect("one request went out");
    let lower = request.to_lowercase();
    assert!(!lower.contains("cookie:"), "{request}");
    assert!(!lower.contains("authorization:"), "{request}");
    assert!(lower.contains("user-agent: lucidos-gateway"), "{request}");
    let target = request.split_whitespace().nth(1).unwrap();
    let query = target.split_once('?').unwrap().1;
    let keys: Vec<&str> = query
        .split('&')
        .map(|p| p.split_once('=').unwrap().0)
        .collect();
    assert_eq!(keys, vec!["platform", "arch", "version"]);
}

#[tokio::test]
async fn three_concurrent_refreshes_make_one_request() {
    let dir = tempfile::tempdir().unwrap();
    // The delay keeps the first poll in flight while the other two arrive, so
    // they queue on the poll lock rather than racing past it.
    let origin = origin_serving(
        http_ok(r#"{"version":"99.0.0"}"#),
        Duration::from_millis(80),
    )
    .await;
    let check =
        ReleaseCheck::with_origin(&deployment_in(dir.path(), true), &origin.url, POLL_INTERVAL);

    let (a, b, c) = futures::join!(
        check.refresh(false),
        check.refresh(false),
        check.refresh(false)
    );
    assert_eq!(origin.requests(), 1);
    for snapshot in [a, b, c] {
        assert_eq!(snapshot["latest"]["version"], "99.0.0");
    }
}

#[tokio::test]
async fn a_fresh_answer_is_not_re_polled() {
    let dir = tempfile::tempdir().unwrap();
    let origin = origin_serving(http_ok(r#"{"version":"99.0.0"}"#), Duration::ZERO).await;
    let check =
        ReleaseCheck::with_origin(&deployment_in(dir.path(), true), &origin.url, POLL_INTERVAL);

    check.refresh(false).await;
    check.refresh(false).await;
    check.refresh(true).await; // A forced check honours its own floor too.
    assert_eq!(origin.requests(), 1);
}

/// A first launch polls on its own. There is no preference file yet, so the
/// default decides, and the default is on (ADR 0139).
#[tokio::test]
async fn a_fresh_install_polls_with_no_click() {
    let dir = tempfile::tempdir().unwrap();
    let origin = origin_serving(http_ok(r#"{"version":"99.0.0"}"#), Duration::ZERO).await;
    let mut dep = deployment_in(dir.path(), true);
    // Nothing has ever written the preference on this machine.
    dep.config_path = Some(dir.path().join("never-written/updates.toml"));
    let check = ReleaseCheck::with_origin(&dep, &origin.url, POLL_INTERVAL);

    let snapshot = check.refresh(false).await;
    assert_eq!(origin.requests(), 1);
    assert_eq!(snapshot["enabled"], true);
    assert_eq!(snapshot["latest"]["version"], "99.0.0");
}

/// An install that never answered the old notice now polls. That group was the
/// whole point of ADR 0139: an unanswered notice was a permanent silent
/// opt-out, withholding the fixes the check exists to deliver.
#[tokio::test]
async fn an_install_that_never_answered_the_notice_now_polls() {
    let dir = tempfile::tempdir().unwrap();
    let origin = origin_serving(http_ok(r#"{"version":"99.0.0"}"#), Duration::ZERO).await;
    let mut dep = deployment_in(dir.path(), true);
    // Exactly what a pre-ADR-0139 fresh install left on disk.
    let config = dir.path().join("unanswered-updates.toml");
    std::fs::write(
        &config,
        "[release_check]\nenabled = true\nnotice_acknowledged = false\n",
    )
    .unwrap();
    dep.config_path = Some(config);
    let check = ReleaseCheck::with_origin(&dep, &origin.url, POLL_INTERVAL);

    let snapshot = check.refresh(false).await;
    assert_eq!(
        origin.requests(),
        1,
        "a stale notice must not gate the poll"
    );
    assert_eq!(snapshot["enabled"], true);
    assert_eq!(snapshot["latest"]["version"], "99.0.0");
}

/// An install that answered the old notice with "Turn it off" wrote
/// `enabled = false`, and it stays off. The stale `notice_acknowledged` beside
/// it is ignored rather than fatal, so the off switch survives the migration.
#[tokio::test]
async fn an_install_that_turned_the_check_off_stays_off() {
    let dir = tempfile::tempdir().unwrap();
    let origin = origin_serving(http_ok(r#"{"version":"99.0.0"}"#), Duration::ZERO).await;
    let mut dep = deployment_in(dir.path(), true);
    // Written by hand, because the renderer no longer emits the dead field.
    let config = dir.path().join("legacy-updates.toml");
    std::fs::write(
        &config,
        "[release_check]\nenabled = false\nnotice_acknowledged = true\n",
    )
    .unwrap();
    dep.config_path = Some(config);
    let check = ReleaseCheck::with_origin(&dep, &origin.url, POLL_INTERVAL);

    let snapshot = check.refresh(false).await;
    assert_eq!(origin.requests(), 0, "an off switch on disk must hold");
    assert_eq!(snapshot["enabled"], false);
    assert_eq!(snapshot["supported"], true);
}

/// Turning the check off stops the AUTOMATIC poll, not the button. Settings
/// says manual checking still works, and a promise the code breaks is worse
/// than no switch at all.
#[tokio::test]
async fn a_forced_check_works_while_the_automatic_one_is_off() {
    let dir = tempfile::tempdir().unwrap();
    let origin = origin_serving(http_ok(r#"{"version":"99.0.0"}"#), Duration::ZERO).await;
    let check = ReleaseCheck::with_origin(
        &deployment_in(dir.path(), false),
        &origin.url,
        POLL_INTERVAL,
    );

    // The backstop tick honours the preference and stays silent.
    let snapshot = check.refresh(false).await;
    assert_eq!(origin.requests(), 0);
    assert_eq!(snapshot["enabled"], false);

    // The button asks anyway, because the click IS the request.
    let snapshot = check.refresh(true).await;
    assert_eq!(origin.requests(), 1);
    assert_eq!(snapshot["latest"]["version"], "99.0.0");
}

/// The deployment gate is the one `force` can never open.
#[tokio::test]
async fn a_forced_check_still_refuses_from_a_source_checkout() {
    let dir = tempfile::tempdir().unwrap();
    let origin = origin_serving(http_ok(r#"{"version":"99.0.0"}"#), Duration::ZERO).await;
    let mut dep = deployment_in(dir.path(), true);
    dep.exe = Some(checkout(&dir.path().join("repo")));
    let check = ReleaseCheck::with_origin(&dep, &origin.url, POLL_INTERVAL);

    check.refresh(true).await;
    assert_eq!(origin.requests(), 0);
}

/// A failed poll must be reportable, or the caller reads the unchanged
/// snapshot as "you are up to date".
#[tokio::test]
async fn a_failed_poll_is_recorded_and_a_later_success_clears_it() {
    let dir = tempfile::tempdir().unwrap();
    let html = "HTTP/1.1 200 OK\r\ncontent-type: text/html\r\ncontent-length: 6\r\n\
                connection: close\r\n\r\n<html>";
    let broken = origin_serving(html.to_string(), Duration::ZERO).await;
    let dep = deployment_in(dir.path(), true);
    let check = ReleaseCheck::with_origin(&dep, &broken.url, Duration::from_millis(1));

    let snapshot = check.refresh(false).await;
    assert!(
        snapshot["last_error"].is_string(),
        "a soft 404 must be reportable, got {snapshot}"
    );

    let good = origin_serving(http_ok(r#"{"version":"99.0.0"}"#), Duration::ZERO).await;
    let check = ReleaseCheck::with_origin(&dep, &good.url, Duration::from_millis(1));
    let snapshot = check.refresh(false).await;
    assert_eq!(snapshot["last_error"], serde_json::Value::Null);
}

#[tokio::test]
async fn a_dev_deployment_never_reaches_the_origin() {
    let dir = tempfile::tempdir().unwrap();
    let origin = origin_serving(http_ok(r#"{"version":"99.0.0"}"#), Duration::ZERO).await;
    let mut dep = deployment_in(dir.path(), true);
    dep.exe = Some(checkout(&dir.path().join("repo")));
    let check = ReleaseCheck::with_origin(&dep, &origin.url, POLL_INTERVAL);

    let snapshot = check.refresh(true).await;
    assert_eq!(origin.requests(), 0);
    assert_eq!(snapshot["supported"], false);
}

#[tokio::test]
async fn the_preference_is_re_read_on_every_tick() {
    let dir = tempfile::tempdir().unwrap();
    let origin = origin_serving(http_ok(r#"{"version":"99.0.0"}"#), Duration::ZERO).await;
    let dep = deployment_in(dir.path(), true);
    let config = dep.config_path.clone().unwrap();
    // A one-millisecond interval makes the second refresh due immediately, so
    // only the preference can stop it.
    let check = ReleaseCheck::with_origin(&dep, &origin.url, Duration::from_millis(1));

    check.refresh(false).await;
    assert_eq!(origin.requests(), 1);

    write_updates_toml(Some(&config), &UpdatesToml { enabled: false }).unwrap();
    tokio::time::sleep(Duration::from_millis(5)).await;
    let snapshot = check.refresh(false).await;

    assert_eq!(origin.requests(), 1, "turning it off must take effect now");
    assert_eq!(snapshot["enabled"], false);
}

#[tokio::test]
async fn a_redirect_is_not_followed() {
    let dir = tempfile::tempdir().unwrap();
    let redirect = "HTTP/1.1 302 Found\r\nlocation: http://example.invalid/x\r\n\
                    content-length: 0\r\nconnection: close\r\n\r\n";
    let origin = origin_serving(redirect.to_string(), Duration::ZERO).await;
    let check =
        ReleaseCheck::with_origin(&deployment_in(dir.path(), true), &origin.url, POLL_INTERVAL);

    let snapshot = check.refresh(false).await;
    assert_eq!(origin.requests(), 1, "exactly one request, no follow");
    assert_eq!(snapshot["latest"], serde_json::Value::Null);
    assert_eq!(snapshot["checked_at"], serde_json::Value::Null);
}

#[tokio::test]
async fn landing_page_html_is_not_read_as_up_to_date() {
    let dir = tempfile::tempdir().unwrap();
    let html = "<!DOCTYPE html><html><body>Lucidos</body></html>";
    let body = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/html\r\ncontent-length: {}\r\n\
         connection: close\r\n\r\n{html}",
        html.len()
    );
    let origin = origin_serving(body, Duration::ZERO).await;
    let check =
        ReleaseCheck::with_origin(&deployment_in(dir.path(), true), &origin.url, POLL_INTERVAL);

    let snapshot = check.refresh(false).await;
    // A soft 404 leaves the answer unknown, so `checked_at` never moves.
    assert_eq!(snapshot["checked_at"], serde_json::Value::Null);
    assert_eq!(snapshot["latest"], serde_json::Value::Null);
}

#[tokio::test]
async fn an_older_published_version_is_not_offered() {
    let dir = tempfile::tempdir().unwrap();
    let origin = origin_serving(http_ok(r#"{"version":"0.0.1"}"#), Duration::ZERO).await;
    let check =
        ReleaseCheck::with_origin(&deployment_in(dir.path(), true), &origin.url, POLL_INTERVAL);

    let snapshot = check.refresh(false).await;
    assert_eq!(snapshot["latest"], serde_json::Value::Null);
    // The answer arrived, so the check itself succeeded.
    assert_ne!(snapshot["checked_at"], serde_json::Value::Null);
}

#[tokio::test]
async fn an_installer_install_is_offered_a_command_and_a_bundle_is_not() {
    let dir = tempfile::tempdir().unwrap();
    let origin = origin_serving(http_ok(r#"{"version":"99.0.0"}"#), Duration::ZERO).await;
    let check =
        ReleaseCheck::with_origin(&deployment_in(dir.path(), true), &origin.url, POLL_INTERVAL);
    let snapshot = check.refresh(false).await;
    assert_eq!(snapshot["latest"]["install"], "installer-rerun");
    assert!(snapshot["latest"]["command"]
        .as_str()
        .unwrap()
        .contains("--name default"));

    let bundle = origin_serving(http_ok(r#"{"version":"99.0.0"}"#), Duration::ZERO).await;
    let mut dep = deployment_in(dir.path(), true);
    dep.exe = Some(
        dir.path()
            .join("Lucidos.app/Contents/Resources/lucidos-gateway"),
    );
    let app = ReleaseCheck::with_origin(&dep, &bundle.url, POLL_INTERVAL);
    let snapshot = app.refresh(false).await;
    assert_eq!(snapshot["latest"]["install"], "desktop-app");
    // The client installs a bundle, so there is no command to copy.
    assert_eq!(snapshot["latest"]["command"], serde_json::Value::Null);
}
