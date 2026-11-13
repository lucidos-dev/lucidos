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
fn the_user_agent_carries_the_version_and_nothing_else() {
    assert_eq!(user_agent("0.33.0"), "lucidos-gateway/0.33.0");
    // The whole value, so a field smuggled in beside the version fails here.
    // ADR 0108 allows the version because the query string already carries it,
    // and allows nothing further.
    let ua = user_agent(crate::LUCIDOS_RELEASE);
    let (product, version) = ua.split_once('/').expect("product/version");
    assert_eq!(product, "lucidos-gateway");
    assert_eq!(version, crate::LUCIDOS_RELEASE);
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

fn only_enabled(enabled: bool) -> UpdatesToml {
    UpdatesToml {
        enabled,
        ..UpdatesToml::default()
    }
}

fn with_address(address: &str) -> UpdatesToml {
    UpdatesToml {
        update_address: address.to_string(),
        ..UpdatesToml::default()
    }
}

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
    assert_eq!(never_answered, only_enabled(true));

    // An answered notice beside an explicit off switch keeps the off switch.
    let turned_off =
        parse_updates_toml("[release_check]\nenabled = false\nnotice_acknowledged = true\n");
    assert_eq!(turned_off, only_enabled(false));
}

#[test]
fn render_then_parse_round_trips() {
    for cfg in [
        only_enabled(false),
        only_enabled(true),
        with_address("http://127.0.0.1:5173/api/v1/data/artifacts/releases/rc/latest.json"),
        UpdatesToml {
            enabled: false,
            update_address: r#"https://example.com/a "quoted" \ path"#.to_string(),
        },
        // A refused value survives a rewrite, so it stays visible until fixed.
        with_address("http://example.com/latest.json"),
    ] {
        assert_eq!(parse_updates_toml(&render_updates_toml(&cfg)), cfg);
    }
}

#[test]
fn writing_the_preference_is_atomic_and_readable_back() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested/updates.toml");
    let cfg = only_enabled(false);
    write_updates_toml(Some(&path), &cfg).unwrap();
    assert_eq!(read_updates_toml(Some(&path)), cfg);
    // No temp file is left behind by the rename.
    assert!(!path.with_extension("toml.tmp").exists());
}

#[test]
fn the_update_address_is_read_raw_and_judged_by_the_shared_rule() {
    let cfg = parse_updates_toml(
        "[release_check]\nupdate_address = \"http://localhost:5173/latest.json\"\n",
    );
    assert!(
        cfg.enabled,
        "naming only the address keeps the default switch"
    );
    assert_eq!(
        cfg.address().unwrap().unwrap().as_str(),
        "http://localhost:5173/latest.json"
    );

    // A hand-edited bad value is kept and refused, never dropped to default.
    let bad = parse_updates_toml("[release_check]\nupdate_address = \"http://example.com/x\"\n");
    assert_eq!(bad.update_address, "http://example.com/x");
    assert!(bad.address().is_err());

    // A value of the wrong type keeps the rest of the file, and is refused.
    let typed = parse_updates_toml("[release_check]\nenabled = false\nupdate_address = 5\n");
    assert!(!typed.enabled);
    assert!(typed.address().is_err());
}

#[test]
fn a_write_keeps_the_field_it_does_not_name() {
    let dir = tempfile::tempdir().unwrap();
    let dep = deployment_in(dir.path(), true);
    let config = dep.config_path.clone().unwrap();
    let check = ReleaseCheck::with_origin(&dep, "http://127.0.0.1:9/x", POLL_INTERVAL);

    check
        .update_config(None, Some(" https://example.com/latest.json "))
        .unwrap();
    check.update_config(Some(false), None).unwrap();
    let cfg = read_updates_toml(Some(&config));
    assert!(!cfg.enabled);
    assert_eq!(cfg.update_address, "https://example.com/latest.json");

    check.update_config(Some(true), None).unwrap();
    check.update_config(None, Some("")).unwrap();
    assert_eq!(read_updates_toml(Some(&config)), only_enabled(true));
}

#[test]
fn a_refused_address_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let dep = deployment_in(dir.path(), true);
    let config = dep.config_path.clone().unwrap();
    let check = ReleaseCheck::with_origin(&dep, "http://127.0.0.1:9/x", POLL_INTERVAL);
    let before = std::fs::read_to_string(&config).unwrap();

    for bad in [
        "http://example.com/latest.json",
        "file:///tmp/latest.json",
        "/tmp/x",
    ] {
        let err = check.update_config(Some(false), Some(bad)).unwrap_err();
        assert!(matches!(err, ConfigWriteError::InvalidAddress(_)), "{bad}");
    }
    assert_eq!(std::fs::read_to_string(&config).unwrap(), before);
}

#[test]
fn a_set_address_is_logged_at_startup() {
    assert_eq!(update_address_log_line(&UpdatesToml::default()), None);
    let set = update_address_log_line(&with_address("https://example.com/latest.json")).unwrap();
    assert!(set.contains("https://example.com/latest.json"), "{set}");
    assert!(set.contains("not the published release"), "{set}");
    let refused = update_address_log_line(&with_address("http://example.com/x")).unwrap();
    assert!(refused.contains("refused"), "{refused}");
}

#[test]
fn writing_without_a_home_is_an_error_not_a_panic() {
    assert!(write_updates_toml(None, &UpdatesToml::default()).is_err());
}

// ── Live requests against a throwaway origin ────────────────────────────────

pub(crate) struct Origin {
    pub(crate) url: String,
    seen: Arc<Mutex<Vec<String>>>,
}

impl Origin {
    pub(crate) fn requests(&self) -> usize {
        self.seen.lock().unwrap().len()
    }
}

pub(crate) fn http_ok(body: &str) -> String {
    format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\n\
         content-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    )
}

/// A loopback origin that records each request and answers with `response`.
pub(crate) async fn origin_serving(response: String, delay: Duration) -> Origin {
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
pub(crate) fn deployment_in(dir: &Path, enabled: bool) -> Deployment {
    let config = dir.join("updates.toml");
    write_updates_toml(Some(&config), &only_enabled(enabled)).unwrap();
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
    // Match the header NAME case-insensitively and keep the value's own case,
    // so a version carrying a letter compares as sent rather than folded.
    let sent_ua = request
        .lines()
        .find(|l| l.to_ascii_lowercase().starts_with("user-agent:"))
        .and_then(|l| l.split_once(':'))
        .map(|(_, v)| v.trim())
        .expect("the request names itself");
    assert_eq!(sent_ua, user_agent(crate::LUCIDOS_RELEASE), "{request}");
    let target = request.split_whitespace().nth(1).unwrap();
    let query = target.split_once('?').unwrap().1;
    let keys: Vec<&str> = query
        .split('&')
        .map(|p| p.split_once('=').unwrap().0)
        .collect();
    assert_eq!(keys, vec!["platform", "arch", "version"]);
    // ONE SOURCE, read off the wire. The user agent repeats the query
    // parameter. A second version source would show up here as a
    // disagreement, rather than as two places to keep in step by hand.
    let sent_version = query
        .split('&')
        .find_map(|p| p.strip_prefix("version="))
        .expect("the query carries a version");
    assert_eq!(sent_ua, format!("lucidos-gateway/{sent_version}"));
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

/// Ten callers that each stop listening while the origin is still answering:
/// a window hidden or reloaded mid-fetch. The gateway drops each handler then.
/// Every abandoned refresh used to send its own request and record none of
/// them, which is the ten-in-one-minute burst one install sent.
async fn abandoned_refreshes_make_one_request(force: bool) {
    let dir = tempfile::tempdir().unwrap();
    let origin = origin_serving(
        http_ok(r#"{"version":"99.0.0"}"#),
        Duration::from_millis(300),
    )
    .await;
    let check =
        ReleaseCheck::with_origin(&deployment_in(dir.path(), true), &origin.url, POLL_INTERVAL);

    // At least the first caller gives up, since the origin takes ten times
    // longer than it waits.
    for _ in 0..10 {
        let _ = tokio::time::timeout(Duration::from_millis(30), check.refresh(force)).await;
    }
    // The answer of the one request that went out still reaches the next caller.
    let snapshot = check.refresh(false).await;
    assert_eq!(origin.requests(), 1);
    assert_eq!(snapshot["latest"]["version"], "99.0.0");
}

#[tokio::test]
async fn abandoned_refreshes_make_one_request_between_them() {
    abandoned_refreshes_make_one_request(false).await;
}

#[tokio::test]
async fn abandoned_forced_checks_still_honour_the_floor() {
    abandoned_refreshes_make_one_request(true).await;
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

    write_updates_toml(Some(&config), &only_enabled(false)).unwrap();
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

// ── The update address ──────────────────────────────────────────────────────

fn set_address(dep: &Deployment, address: &str) {
    write_updates_toml(dep.config_path.as_deref(), &with_address(address)).unwrap();
}

#[tokio::test]
async fn a_set_address_is_fetched_as_a_plain_latest_json() {
    let dir = tempfile::tempdir().unwrap();
    let default = origin_serving(http_ok(r#"{"version":"98.0.0"}"#), Duration::ZERO).await;
    let rc = origin_serving(
        http_ok(r#"{"version":"99.0.0","notes":"rc","platforms":{}}"#),
        Duration::ZERO,
    )
    .await;
    let dep = deployment_in(dir.path(), true);
    let address = rc
        .url
        .replace("/api/update-check", "/releases/rc/latest.json");
    set_address(&dep, &address);
    let check = ReleaseCheck::with_origin(&dep, &default.url, POLL_INTERVAL);

    let snapshot = check.refresh(false).await;
    assert_eq!(default.requests(), 0, "the default origin is not asked");
    assert_eq!(snapshot["latest"]["version"], "99.0.0");
    assert_eq!(snapshot["update_address"], address);
    assert_eq!(snapshot["update_address_error"], Value::Null);
    let seen = rc.seen.lock().unwrap();
    let target = seen[0].split_whitespace().nth(1).unwrap().to_string();
    assert_eq!(target, "/releases/rc/latest.json", "no query parameters");
}

#[tokio::test]
async fn an_unset_address_asks_the_default_origin_as_before() {
    let dir = tempfile::tempdir().unwrap();
    let origin = origin_serving(http_ok(r#"{"version":"99.0.0"}"#), Duration::ZERO).await;
    let check =
        ReleaseCheck::with_origin(&deployment_in(dir.path(), true), &origin.url, POLL_INTERVAL);

    let snapshot = check.refresh(false).await;
    assert_eq!(snapshot["update_address"], Value::Null);
    let seen = origin.seen.lock().unwrap();
    let target = seen[0].split_whitespace().nth(1).unwrap();
    let (platform, arch) = (
        platform_key(std::env::consts::OS).unwrap(),
        arch_key(std::env::consts::ARCH).unwrap(),
    );
    assert_eq!(
        target,
        check_url("/api/update-check", platform, arch, crate::LUCIDOS_RELEASE)
    );
}

#[tokio::test]
async fn a_refused_address_fails_closed_and_sends_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let origin = origin_serving(http_ok(r#"{"version":"99.0.0"}"#), Duration::ZERO).await;
    let dep = deployment_in(dir.path(), true);
    let check = ReleaseCheck::with_origin(&dep, &origin.url, Duration::from_millis(1));
    check.refresh(false).await;
    assert_eq!(origin.requests(), 1);

    set_address(&dep, "http://example.com/latest.json");
    let snapshot = check.refresh(true).await;
    assert_eq!(origin.requests(), 1, "no request under a refused address");
    assert!(snapshot["update_address_error"].is_string(), "{snapshot}");
    assert_eq!(snapshot["last_error"], snapshot["update_address_error"]);
    assert_eq!(snapshot["update_address"], "http://example.com/latest.json");
    // The default origin's earlier answer is not offered either.
    assert_eq!(snapshot["latest"], Value::Null);
    assert_eq!(snapshot["checked_at"], Value::Null);
}

#[tokio::test]
async fn a_changed_address_is_polled_at_once_and_hides_the_old_answer() {
    let dir = tempfile::tempdir().unwrap();
    let default = origin_serving(http_ok(r#"{"version":"98.0.0"}"#), Duration::ZERO).await;
    let rc = origin_serving(http_ok(r#"{"version":"99.0.0"}"#), Duration::ZERO).await;
    let dep = deployment_in(dir.path(), true);
    let check = ReleaseCheck::with_origin(&dep, &default.url, POLL_INTERVAL);
    assert_eq!(check.refresh(false).await["latest"]["version"], "98.0.0");

    set_address(&dep, &rc.url);
    // The hour is not up, and the answer still comes from the new address.
    assert_eq!(check.refresh(false).await["latest"]["version"], "99.0.0");
    assert_eq!(rc.requests(), 1);

    set_address(&dep, "");
    assert_eq!(
        check.snapshot()["latest"],
        Value::Null,
        "stale until re-polled"
    );
    assert_eq!(check.refresh(false).await["latest"]["version"], "98.0.0");
    assert_eq!(default.requests(), 2);
}

#[tokio::test]
async fn a_failed_poll_of_a_new_address_drops_the_old_answer() {
    let dir = tempfile::tempdir().unwrap();
    let rc = origin_serving(http_ok(r#"{"version":"99.0.0"}"#), Duration::ZERO).await;
    let down = origin_serving(
        "HTTP/1.1 503 Service Unavailable\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
            .to_string(),
        Duration::ZERO,
    )
    .await;
    let dep = deployment_in(dir.path(), true);
    let check = ReleaseCheck::with_origin(&dep, &down.url, POLL_INTERVAL);
    set_address(&dep, &rc.url);
    assert_eq!(check.refresh(false).await["latest"]["version"], "99.0.0");

    // Back to the default, which is down: the rc offer must not pass as published.
    set_address(&dep, "");
    let snapshot = check.refresh(false).await;
    assert!(snapshot["last_error"].is_string(), "got {snapshot}");
    assert_eq!(snapshot["latest"], Value::Null, "got {snapshot}");
    assert_eq!(snapshot["checked_at"], Value::Null, "got {snapshot}");
}

#[tokio::test]
async fn the_relayed_version_never_comes_from_another_address() {
    let dir = tempfile::tempdir().unwrap();
    let default = origin_serving(http_ok(r#"{"version":"98.0.0"}"#), Duration::ZERO).await;
    let dep = deployment_in(dir.path(), true);
    let check = ReleaseCheck::with_origin(&dep, &default.url, POLL_INTERVAL);
    check.refresh(false).await;
    assert_eq!(check.latest_version().as_deref(), Some("98.0.0"));

    set_address(&dep, "http://127.0.0.1:9/latest.json");
    assert_eq!(check.latest_version(), None, "stale until re-polled");
    set_address(&dep, "http://example.com/latest.json");
    assert_eq!(
        check.latest_version(),
        None,
        "a refused address offers nothing"
    );
}
