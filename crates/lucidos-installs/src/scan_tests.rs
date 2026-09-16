//! Fixture-tree tests for the install scan.
//!
//! Every case builds a throwaway `$HOME` and points [`ScanRoots::under`] at it,
//! so nothing here reads the machine it runs on. That matters twice over: the
//! developer running these has a real `.app` in `/Applications` and a real dev
//! gateway in `~/.lucidos`, and either would otherwise leak into a count.
//!
//! The load-bearing pair is `a_checkout_beside_the_app_is_not_a_conflict` and
//! `an_instance_and_the_app_on_one_port_conflict`. Together they are the whole
//! discriminator: coexistence stays silent, contention speaks.

use super::*;

/// A throwaway `$HOME`, removed when the test ends.
struct Fixture {
    home: PathBuf,
}

impl Fixture {
    fn new(tag: &str) -> Self {
        let home = std::env::temp_dir().join(format!(
            "lucidos-installs-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::remove_dir_all(&home).ok();
        std::fs::create_dir_all(&home).unwrap();
        // macOS puts the temp dir behind a symlink, so an un-canonicalised root
        // makes every `starts_with` on a resolved exe path miss.
        let home = std::fs::canonicalize(&home).unwrap();
        Fixture { home }
    }

    fn roots(&self) -> ScanRoots {
        ScanRoots::under(&self.home)
    }

    fn dir(&self, rel: &str) -> PathBuf {
        let path = self.home.join(rel);
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn file(&self, rel: &str, contents: &str) -> PathBuf {
        let path = self.home.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, contents).unwrap();
        path
    }

    /// A bundle in `~/Applications`, at `version` unless `version` is empty.
    fn bundle(&self, version: &str) -> PathBuf {
        let bundle = self.dir("Applications/Lucidos.app/Contents/Resources");
        if !version.is_empty() {
            self.file(
                "Applications/Lucidos.app/Contents/Info.plist",
                &info_plist(version),
            );
        }
        bundle.parent().unwrap().parent().unwrap().to_path_buf()
    }

    /// A registered installer instance on `port`.
    fn instance(&self, slug: &str, port: u16) {
        self.file(&format!(".lucidos/{slug}/port"), &format!("{port}\n"));
    }

    /// The shared runtime, with `current` pointing at it.
    fn runtime(&self, stem: &str) -> PathBuf {
        let dir = self.dir(&format!(".lucidos/runtime/{stem}"));
        #[cfg(unix)]
        std::os::unix::fs::symlink(&dir, self.home.join(".lucidos/runtime/current")).unwrap();
        dir
    }

    /// The dev gateway's machine-global data dir.
    fn dev_gateway(&self) {
        self.dir(".lucidos/gateway");
    }

    /// A checkout, identified by the marker the whole tree resolves it from.
    fn checkout(&self, name: &str, release: &str) -> PathBuf {
        let repo = self.dir(&format!("code/{name}"));
        self.file(&format!("code/{name}/scripts/web-dev.sh"), "#!/bin/bash\n");
        self.file(&format!("code/{name}/RELEASE"), &format!("{release}\n"));
        repo
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.home).ok();
    }
}

fn info_plist(version: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <plist version=\"1.0\">\n<dict>\n\
         \t<key>CFBundleName</key>\n\t<string>Lucidos</string>\n\
         \t<key>CFBundleShortVersionString</key>\n\t<string>{version}</string>\n\
         </dict>\n</plist>\n"
    )
}

fn kinds(inv: &Inventory) -> Vec<InstallKind> {
    inv.installs.iter().map(|i| i.kind).collect()
}

// ── enumeration ─────────────────────────────────────────────────────────────

#[test]
fn a_lone_desktop_app_is_listed_with_its_version_and_no_conflict() {
    let fx = Fixture::new("lone-app");
    fx.bundle("1.2.3");

    let inv = scan(&fx.roots(), &RunningProcess::default());

    assert_eq!(kinds(&inv), vec![InstallKind::DesktopApp]);
    assert_eq!(inv.installs[0].version.as_deref(), Some("1.2.3"));
    assert_eq!(inv.installs[0].port, Some(DEFAULT_GATEWAY_PORT));
    assert!(
        inv.conflicts.is_empty(),
        "one install can contend with nothing: {:?}",
        inv.conflicts
    );
}

#[test]
fn all_three_vehicles_are_enumerated_together() {
    let fx = Fixture::new("all-three");
    fx.bundle("1.2.3");
    fx.runtime("lucidos-0.26.2-aarch64-apple-darwin");
    fx.instance("default", 5253);
    fx.dev_gateway();

    let inv = scan(&fx.roots(), &RunningProcess::default());

    assert_eq!(
        kinds(&inv),
        vec![
            InstallKind::DesktopApp,
            InstallKind::HeadlessInstaller,
            InstallKind::SourceCheckout,
        ]
    );
    // The instance's version is the SHARED runtime's, which is the binary it
    // actually runs. This is the number the incident's user never saw.
    let instance = &inv.installs[1];
    assert_eq!(instance.version.as_deref(), Some("0.26.2"));
    assert_eq!(instance.port, Some(5253));
}

#[test]
fn a_reserved_directory_is_never_read_as_an_instance() {
    let fx = Fixture::new("reserved");
    // The dev gateway's own dir, and a stray marker inside the shared runtime.
    fx.file(".lucidos/gateway/port", "5251\n");
    fx.file(".lucidos/runtime/port", "5252\n");
    fx.file(".lucidos/logs/port", "5252\n");

    let inv = scan(&fx.roots(), &RunningProcess::default());

    assert_eq!(
        kinds(&inv),
        vec![InstallKind::SourceCheckout],
        "reserved slugs are the dev gateway and the shared runtime, never instances"
    );
}

#[test]
fn an_instance_is_found_under_a_non_default_prefix_and_names_it() {
    let fx = Fixture::new("prefix");
    let extra = fx.dir("elsewhere");
    std::fs::create_dir_all(extra.join("work")).unwrap();
    std::fs::write(extra.join("work/port"), "5300\n").unwrap();
    let mut roots = fx.roots();
    roots.installer_prefixes.push(extra.clone());

    let inv = scan(&roots, &RunningProcess::default());

    let found = &inv.installs[0];
    assert_eq!(found.kind, InstallKind::HeadlessInstaller);
    assert!(
        found.name.contains(&extra.display().to_string()),
        "a non-default prefix belongs in the name: {}",
        found.name
    );
    assert!(
        found.removal.contains("--prefix"),
        "and in the command that removes it: {}",
        found.removal
    );
}

// ── the discriminator ───────────────────────────────────────────────────────

#[test]
fn a_checkout_beside_the_app_is_not_a_conflict() {
    let fx = Fixture::new("dev-plus-dmg");
    fx.bundle("1.2.3");
    fx.dev_gateway();

    let inv = scan(&fx.roots(), &RunningProcess::default());

    assert_eq!(inv.installs.len(), 2);
    assert_eq!(inv.installs[0].port, Some(DEFAULT_GATEWAY_PORT));
    assert_eq!(inv.installs[1].port, Some(DEFAULT_DEV_GATEWAY_PORT));
    assert!(
        inv.conflicts.is_empty(),
        "this is the maintainer's own setup and it must never warn: {:?}",
        inv.conflicts
    );
}

#[test]
fn an_instance_and_the_app_on_one_port_conflict() {
    let fx = Fixture::new("the-trap");
    fx.bundle("0.36.0");
    fx.runtime("lucidos-0.26.2-aarch64-apple-darwin");
    fx.instance("default", DEFAULT_GATEWAY_PORT);

    let inv = scan(&fx.roots(), &RunningProcess::default());

    assert_eq!(inv.conflicts.len(), 1);
    let conflict = &inv.conflicts[0];
    assert_eq!(conflict.port, DEFAULT_GATEWAY_PORT);
    assert_eq!(conflict.installs.len(), 2);
    assert!(conflict.installs[0].contains("Lucidos.app"));
    assert!(conflict.installs[1].contains("default"));
}

#[test]
fn a_conflict_fingerprint_ignores_everything_but_its_own_port() {
    let fx = Fixture::new("fingerprint");
    fx.bundle("0.36.0");
    fx.instance("default", DEFAULT_GATEWAY_PORT);
    let before = scan(&fx.roots(), &RunningProcess::default()).conflicts[0].fingerprint();

    // A quiet third install arrives on its own port, and then a SECOND
    // conflict on a port this one knows nothing about.
    fx.dev_gateway();
    fx.instance("alpha", 5300);
    fx.instance("beta", 5300);
    let inv = scan(&fx.roots(), &RunningProcess::default());
    let after = inv
        .conflicts
        .iter()
        .find(|c| c.port == DEFAULT_GATEWAY_PORT)
        .expect("the original conflict is still there")
        .fingerprint();

    assert!(!before.is_empty(), "the trap has a fingerprint");
    assert_eq!(
        before, after,
        "an acknowledged conflict stays acknowledged when another port starts fighting"
    );
    let other = inv.conflicts.iter().find(|c| c.port == 5300).unwrap();
    assert_ne!(
        other.fingerprint(),
        after,
        "a different port is a different thing to say"
    );
}

#[test]
fn a_contended_port_has_no_single_server() {
    let fx = Fixture::new("serving");
    fx.bundle("0.36.0");
    fx.instance("default", DEFAULT_GATEWAY_PORT);
    fx.dev_gateway();

    let inv = scan(&fx.roots(), &RunningProcess::default());

    assert!(
        inv.serving_port(DEFAULT_GATEWAY_PORT).is_none(),
        "two claimants means the answer is the conflict, never a guess"
    );
    assert_eq!(
        inv.serving_port(DEFAULT_DEV_GATEWAY_PORT).map(|i| i.kind),
        Some(InstallKind::SourceCheckout)
    );
}

// ── which one is running ────────────────────────────────────────────────────

#[test]
fn the_data_dir_separates_two_instances_off_one_runtime() {
    let fx = Fixture::new("shared-runtime");
    let runtime = fx.runtime("lucidos-1.2.3-aarch64-apple-darwin");
    fx.instance("alpha", 5252);
    fx.instance("beta", 5253);
    let running = RunningProcess {
        exe: Some(runtime.join("lucidos-gateway")),
        data_dir: Some(fx.home.join(".lucidos/beta")),
    };

    let inv = scan(&fx.roots(), &running);

    let flags: Vec<bool> = inv.installs.iter().map(|i| i.running_here).collect();
    assert_eq!(
        flags,
        vec![false, true],
        "the executable is shared, so only the data dir can tell them apart"
    );
}

#[test]
fn a_checkout_is_located_from_the_running_binary() {
    let fx = Fixture::new("checkout");
    fx.dev_gateway();
    let repo = fx.checkout("lucidos", "1.2.3");
    let running = RunningProcess {
        exe: Some(repo.join(".launch/debug/plain/lucidos-gateway")),
        data_dir: Some(fx.home.join(".lucidos/gateway")),
    };

    let inv = scan(&fx.roots(), &running);

    let checkout = &inv.installs[0];
    assert_eq!(checkout.root.as_deref(), Some(repo.as_path()));
    assert_eq!(checkout.version.as_deref(), Some("1.2.3"));
    assert!(checkout.name.contains("lucidos"));
    assert!(checkout.running_here);
}

#[test]
fn a_checkout_nobody_runs_is_still_reported_without_its_path() {
    let fx = Fixture::new("orphan-checkout");
    fx.dev_gateway();

    let inv = scan(&fx.roots(), &RunningProcess::default());

    let checkout = &inv.installs[0];
    assert_eq!(checkout.kind, InstallKind::SourceCheckout);
    assert_eq!(checkout.root, None);
    assert_eq!(checkout.version, None);
    assert_eq!(checkout.port, Some(DEFAULT_DEV_GATEWAY_PORT));
    assert!(!checkout.running_here);
}

// ── readers ─────────────────────────────────────────────────────────────────

#[test]
fn a_bundle_with_no_readable_plist_reports_no_version() {
    let fx = Fixture::new("no-plist");
    fx.bundle("");

    let inv = scan(&fx.roots(), &RunningProcess::default());

    assert_eq!(kinds(&inv), vec![InstallKind::DesktopApp]);
    assert_eq!(
        inv.installs[0].version, None,
        "an unknown version is a gap, never a guess"
    );
}

#[test]
fn bundle_short_version_reads_the_xml_form_and_refuses_anything_else() {
    let fx = Fixture::new("plist-reader");
    let good = fx.file("good.plist", &info_plist("1.2.3"));
    assert_eq!(bundle_short_version(&good).as_deref(), Some("1.2.3"));

    let empty = fx.file("empty.plist", &info_plist(""));
    assert_eq!(bundle_short_version(&empty), None);

    let other_key = fx.file(
        "other.plist",
        "<plist><dict><key>CFBundleVersion</key><string>9</string></dict></plist>",
    );
    assert_eq!(bundle_short_version(&other_key), None);

    let truncated = fx.file(
        "truncated.plist",
        "<key>CFBundleShortVersionString</key>\n<string>1.0",
    );
    assert_eq!(bundle_short_version(&truncated), None);

    assert_eq!(bundle_short_version(&fx.home.join("absent.plist")), None);
}

#[test]
fn runtime_stem_version_splits_at_the_first_dash_after_the_prefix() {
    assert_eq!(
        runtime_stem_version("lucidos-1.2.3-aarch64-apple-darwin").as_deref(),
        Some("1.2.3")
    );
    assert_eq!(
        runtime_stem_version("lucidos-0.26.2-x86_64-unknown-linux-gnu").as_deref(),
        Some("0.26.2")
    );
    // Not a stem at all, or a stem with no triple, or a version that is not one.
    assert_eq!(runtime_stem_version("current"), None);
    assert_eq!(runtime_stem_version("lucidos-1.2.3"), None);
    assert_eq!(
        runtime_stem_version("lucidos-main-aarch64-apple-darwin"),
        None
    );
    assert_eq!(runtime_stem_version("lucidos--aarch64-apple-darwin"), None);
}

#[test]
fn an_ambiguous_runtime_yields_no_version_rather_than_directory_order() {
    let fx = Fixture::new("ambiguous-runtime");
    fx.dir(".lucidos/runtime/lucidos-0.26.2-aarch64-apple-darwin");
    fx.dir(".lucidos/runtime/lucidos-1.2.3-aarch64-apple-darwin");
    fx.instance("default", 5252);

    assert_eq!(current_runtime_dir(&fx.home.join(".lucidos")), None);
    let inv = scan(&fx.roots(), &RunningProcess::default());
    assert_eq!(inv.installs[0].version, None);
}

#[test]
fn one_runtime_with_no_current_link_is_unambiguous() {
    let fx = Fixture::new("sole-runtime");
    fx.dir(".lucidos/runtime/lucidos-0.31.0-aarch64-apple-darwin");
    fx.instance("default", 5252);

    let inv = scan(&fx.roots(), &RunningProcess::default());

    assert_eq!(inv.installs[0].version.as_deref(), Some("0.31.0"));
}

#[test]
fn a_persisted_engine_port_overrides_the_stable_default() {
    let fx = Fixture::new("engine-port");
    fx.bundle("1.2.3");
    fx.file(
        "Library/Application Support/com.lucidos.app/config/engine-port",
        "5299\n",
    );

    let inv = scan(&fx.roots(), &RunningProcess::default());

    assert_eq!(inv.installs[0].port, Some(5299));
    assert!(inv.installs[0].data_dir.is_some());
}

#[test]
fn an_unreadable_port_marker_leaves_the_port_unknown() {
    let fx = Fixture::new("bad-port");
    fx.file(".lucidos/default/port", "not-a-port\n");

    let inv = scan(&fx.roots(), &RunningProcess::default());

    assert_eq!(inv.installs[0].port, None);
    assert!(
        inv.conflicts.is_empty(),
        "an unknown port contends with nothing"
    );
}

// ── launch agents ───────────────────────────────────────────────────────────

#[test]
fn every_job_that_can_start_an_install_is_named() {
    let fx = Fixture::new("agents");
    fx.bundle("1.2.3");
    fx.file("Library/LaunchAgents/com.lucidos.engine.plist", "<plist/>");
    fx.file("Library/LaunchAgents/com.lucidos.client.plist", "<plist/>");
    fx.instance("default", 5253);
    fx.file(
        "Library/LaunchAgents/com.lucidos.gateway.default.plist",
        "<plist/>",
    );
    fx.file(
        ".config/systemd/user/lucidos-gateway-default.service",
        "[Unit]\n",
    );

    let inv = scan(&fx.roots(), &RunningProcess::default());

    let app_labels: Vec<&str> = inv.installs[0]
        .agents
        .iter()
        .map(|a| a.label.as_str())
        .collect();
    assert_eq!(app_labels, vec![SERVICE_AGENT_LABEL, LOGIN_AGENT_LABEL]);
    let instance = &inv.installs[1];
    assert_eq!(instance.agents.len(), 2);
    assert_eq!(instance.agents[0].manager, AgentManager::Launchd);
    assert_eq!(instance.agents[1].manager, AgentManager::Systemd);
}

// ── the report an uninstaller prints ────────────────────────────────────────

#[test]
fn a_second_bundle_the_uninstaller_does_not_trash_is_a_leftover() {
    // The app's uninstall trashes the RUNNING bundle and nothing else, which is
    // why the report takes a predicate rather than a kind.
    let fx = Fixture::new("two-bundles");
    let running = fx.bundle("1.2.3");
    let other = fx.dir("Applications2/Lucidos.app/Contents");
    let mut roots = fx.roots();
    roots
        .application_dirs
        .push(other.parent().unwrap().parent().unwrap().to_path_buf());
    let me = RunningProcess {
        exe: Some(running.join("Contents/MacOS/Lucidos")),
        data_dir: None,
    };

    let inv = scan(&roots, &me);
    let report = leftovers_report(&inv, |i| i.running_here).join("\n");

    assert_eq!(inv.installs.len(), 2);
    assert!(
        report.contains("Applications2"),
        "the bundle nobody trashed is still on the machine: {report}"
    );
    assert_eq!(
        report.matches("Lucidos.app").count(),
        1,
        "and the one being removed is not reported as left behind"
    );
}

#[test]
fn the_leftovers_report_names_what_this_uninstaller_cannot_remove() {
    let fx = Fixture::new("leftovers");
    fx.bundle("1.2.3");
    fx.runtime("lucidos-0.26.2-aarch64-apple-darwin");
    fx.instance("default", DEFAULT_GATEWAY_PORT);
    fx.file(
        "Library/LaunchAgents/com.lucidos.gateway.default.plist",
        "<plist/>",
    );

    let inv = scan(&fx.roots(), &RunningProcess::default());
    let report = leftovers_report(&inv, |i| i.kind == InstallKind::DesktopApp).join("\n");

    assert!(!report.contains("Lucidos.app"), "the app removes itself");
    assert!(report.contains("install.sh instance \"default\""));
    assert!(report.contains("version 0.26.2"));
    assert!(report.contains(".lucidos/default"));
    assert!(report.contains("com.lucidos.gateway.default.plist"));
    assert!(report.contains("--uninstall --name default"));
}

#[test]
fn the_leftovers_report_is_empty_when_nothing_is_left() {
    let fx = Fixture::new("no-leftovers");
    fx.bundle("1.2.3");

    let inv = scan(&fx.roots(), &RunningProcess::default());

    assert!(leftovers_report(&inv, |i| i.kind == InstallKind::DesktopApp).is_empty());
}

#[test]
fn a_removal_command_quotes_a_slug_that_needs_it() {
    let default = Path::new("/home/u/.lucidos");
    assert_eq!(
        installer_removal(default, "work", default),
        "curl -fsSL https://lucidos.dev/install.sh | sh -s -- --uninstall --name work"
    );
    let odd = Path::new("/opt/lucidos prefix");
    assert!(installer_removal(odd, "a b", default).contains("--name 'a b'"));
    assert!(installer_removal(odd, "a b", default).contains("--prefix '/opt/lucidos prefix'"));
}

// ── the safety property ─────────────────────────────────────────────────────

/// The scan must never run a discovered binary and never write.
///
/// This code runs at client startup against well-known paths, so executing what
/// it finds would be a privilege hazard. A source scan is the right shape here:
/// no runtime test can prove the absence of a call on an untaken branch.
#[test]
fn no_execute_and_no_write() {
    let source = include_str!("lib.rs");
    for forbidden in [
        "Command",
        "std::process",
        "fs::write",
        "fs::remove",
        "fs::create_dir",
        "fs::rename",
        "fs::copy",
        "File::create",
        "OpenOptions",
        "unsafe",
    ] {
        assert!(
            !source.contains(forbidden),
            "the scanner must not reach for `{forbidden}`: it reads, and nothing else"
        );
    }
}

// ── the diagnostic ──────────────────────────────────────────────────────────

/// Print what THIS machine carries. Ignored by default, since the answer is a
/// property of the machine rather than of the code.
///
/// Run it with `cargo test -p lucidos-installs -- --ignored --nocapture`. It
/// is the first question a support conversation asks. Unlike the control-plane
/// route it needs no running gateway, and the machines that hit this are
/// exactly the ones whose gateway is ten releases old.
#[test]
#[ignore = "describes the machine it runs on, not the code"]
fn describe_this_machine() {
    let inventory = scan_this_machine(None);
    for install in &inventory.installs {
        println!(
            "{} [{}] version {} port {}{}",
            install.name,
            install.kind.label(),
            install.version.as_deref().unwrap_or("unknown"),
            install
                .port
                .map(|p| p.to_string())
                .unwrap_or_else(|| "unknown".into()),
            if install.running_here {
                " (this process)"
            } else {
                ""
            },
        );
        for agent in &install.agents {
            println!("    starts itself: {}", agent.path.display());
        }
        println!("    remove with:   {}", install.removal);
    }
    for conflict in &inventory.conflicts {
        println!(
            "CONFLICT on port {}: {}",
            conflict.port,
            conflict.installs.join(" and ")
        );
    }
    println!("conflicts: {}", inventory.conflicts.len());
}
