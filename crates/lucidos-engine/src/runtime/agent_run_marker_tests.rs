use super::*;

/// Spelled out: the name is a wire contract `scripts/lib/workspace.sh` relies on.
const MARKER_ENTRY: &[u8] = b"LUCIDOS_AGENT_RUN=abc";

/// A `KERN_PROCARGS2` buffer: argc, exec path, padding, argv, environment,
/// the empty string that ends it, then the apple strings macOS appends.
fn procargs2(argv: &[&str], env: &[&str]) -> Vec<u8> {
    let mut buf = (argv.len() as i32).to_ne_bytes().to_vec();
    buf.extend_from_slice(b"/bin/sleep\0\0\0\0");
    for s in argv.iter().chain(env) {
        buf.extend_from_slice(s.as_bytes());
        buf.push(0);
    }
    buf.push(0);
    buf.extend_from_slice(b"executable_path=/bin/sleep\0");
    buf
}

#[test]
fn procargs2_yields_the_environment_and_never_argv() {
    let buf = procargs2(
        &["sleep", "LUCIDOS_AGENT_RUN=abc"],
        &["HOME=/Users/me", "LUCIDOS_AGENT_RUN=xyz"],
    );
    let env = procargs2_environment(&buf).expect("parses");
    assert_eq!(
        env,
        vec![&b"HOME=/Users/me"[..], &b"LUCIDOS_AGENT_RUN=xyz"[..]],
        "an argv string that looks like the marker is not an environment entry"
    );
}

#[test]
fn procargs2_counts_an_empty_argument() {
    let buf = procargs2(&["sh", "", "x"], &["LUCIDOS_AGENT_RUN=abc"]);
    let env = procargs2_environment(&buf).expect("parses");
    assert_eq!(env, vec![MARKER_ENTRY]);
}

#[test]
fn an_unparseable_procargs2_buffer_is_none() {
    assert_eq!(procargs2_environment(&[]), None, "empty");
    assert_eq!(procargs2_environment(&[1, 0]), None, "shorter than argc");
    let mut truncated = 5i32.to_ne_bytes().to_vec();
    truncated.extend_from_slice(b"/bin/sleep\0\0sleep\0");
    assert_eq!(
        procargs2_environment(&truncated),
        None,
        "fewer argv strings than argc"
    );
    let negative = (-1i32).to_ne_bytes();
    assert_eq!(procargs2_environment(&negative), None, "negative argc");
}

#[test]
fn environ_entries_split_on_nul() {
    assert_eq!(
        environ_entries(b"A=1\0LUCIDOS_AGENT_RUN=abc\0"),
        vec![&b"A=1"[..], MARKER_ENTRY]
    );
}

#[test]
fn only_the_exact_marker_entry_is_reapable() {
    assert!(reapable(&[b"A=1", MARKER_ENTRY], MARKER_ENTRY));
    assert!(!reapable(&[b"LUCIDOS_AGENT_RUN=abcd"], MARKER_ENTRY));
    assert!(!reapable(&[b"LUCIDOS_AGENT_RUN=other"], MARKER_ENTRY));
    assert!(!reapable(&[], MARKER_ENTRY));
}

#[test]
fn a_shared_sccache_daemon_is_never_reapable() {
    assert!(!reapable(
        &[MARKER_ENTRY, b"SCCACHE_START_SERVER=1"],
        MARKER_ENTRY
    ));
}

#[test]
fn no_refused_group_is_ever_signalled() {
    let found = [(10, 0), (11, 1), (12, 500), (13, 700), (14, 700)];
    assert_eq!(
        groups_to_signal(&found, 500),
        BTreeSet::from([700]),
        "group 0, group 1 and our own group are dropped; duplicates collapse"
    );
}

#[test]
fn the_marker_name_is_the_wire_name() {
    assert!(MARKER_ENTRY.starts_with(format!("{AGENT_RUN_ENV}=").as_bytes()));
}

/// Only an agent spawn stamps the marker. A background task, a trigger script
/// or an MCP server that did would die with the agent's turn (ADR 0257).
#[test]
fn only_the_agent_spawn_sets_the_marker() {
    use crate::test_support::source_scan::production_sources;
    let allowed = [
        "runtime/agent_run_marker.rs",
        "runtime/spawn_env.rs",
        "core/environment_variables.rs",
    ];
    for (rel, text) in production_sources() {
        if allowed.contains(&rel.as_str()) {
            continue;
        }
        assert!(
            !text.contains("AGENT_RUN_ENV") && !text.contains("LUCIDOS_AGENT_RUN"),
            "{rel} names the agent run marker; only the agent spawn may set it"
        );
    }
}

/// The launchers exec the gateway and the shared build-watch without the
/// marker. Otherwise an agent that starts them from Bash would reap them, and
/// every engine the gateway spawns, at its turn end.
#[test]
fn the_daemon_launchers_drop_the_marker() {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/lib/workspace.sh");
    let script = std::fs::read_to_string(&path).expect("read workspace.sh");
    assert!(
        script.contains(&format!("AGENT_RUN_ENV=\"{AGENT_RUN_ENV}\"")),
        "workspace.sh names the marker as the engine does"
    );
    for launch in [
        "run_gateway_supervised \"$(gateway_data_dir)\"",
        "exec perl \"$WORKSPACE_LIB_DIR/build_watch_exec.pl\"",
    ] {
        let line = script
            .lines()
            .find(|line| line.contains(launch))
            .unwrap_or_else(|| panic!("workspace.sh launches `{launch}`"));
        assert!(
            line.contains("unset \"$AGENT_RUN_ENV\""),
            "`{launch}` must be exec'd without the marker: {line}"
        );
    }
}

/// What a sleeper runs. macOS hides the environment of Apple platform binaries
/// such as `/bin/sleep`, so the sleeper is this test binary instead.
#[test]
#[ignore = "the sweep test spawns it as a sleeper"]
fn sleeper_body() {
    std::thread::sleep(Duration::from_secs(60));
}

/// A sleeper leading its own process group, the shape a Claude Code Bash call
/// takes, with `env` added to its environment.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn sleeper(env: &[(&str, &str)]) -> std::process::Child {
    use std::os::unix::process::CommandExt;
    let mut cmd = std::process::Command::new(std::env::current_exe().expect("test binary"));
    cmd.args([
        "--ignored",
        "--exact",
        "runtime::agent_run_marker::tests::sleeper_body",
    ])
    .stdout(std::process::Stdio::null())
    .stderr(std::process::Stdio::null())
    .process_group(0);
    for (key, value) in env {
        cmd.env(key, value);
    }
    cmd.spawn().expect("spawn sleeper")
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn wait_for_exit(child: &mut std::process::Child) -> bool {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        if child.try_wait().expect("try_wait").is_some() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[tokio::test]
async fn the_sweep_reaps_its_own_marker_and_nothing_else() {
    let marker = AgentRunMarker::mint();
    let foreign = AgentRunMarker::mint();
    let mut escaped = sleeper(&[(AGENT_RUN_ENV, marker.value())]);
    let mut other_session = sleeper(&[(AGENT_RUN_ENV, foreign.value())]);
    let mut unmarked = sleeper(&[]);
    // sccache's own entry, spelled as sccache writes it.
    let mut daemon = sleeper(&[
        (AGENT_RUN_ENV, marker.value()),
        ("SCCACHE_START_SERVER", "1"),
    ]);

    reap_marked_groups(marker, "a test", Duration::from_millis(200)).await;

    let escaped_died = wait_for_exit(&mut escaped);
    let survivors = [
        ("another session's process", &mut other_session),
        ("an unmarked process", &mut unmarked),
        ("the shared sccache daemon", &mut daemon),
    ]
    .map(|(name, child)| (name, child.try_wait().expect("try_wait").is_none()));
    for child in [&mut escaped, &mut other_session, &mut unmarked, &mut daemon] {
        let _ = child.kill();
        let _ = child.wait();
    }

    assert!(escaped_died, "a marked process in its own group is reaped");
    for (name, alive) in survivors {
        assert!(alive, "{name} survives the sweep");
    }
}

/// Write the record an earlier engine process would have left for `thread`.
fn record_from_another_engine(workspace: &Path, thread: Uuid, marker: &AgentRunMarker) {
    let path = record_path(workspace, thread);
    std::fs::create_dir_all(path.parent().expect("record dir")).expect("create record dir");
    std::fs::write(&path, format!("an-earlier-engine {}\n", marker.value())).expect("write");
}

#[test]
fn a_thread_session_adopts_the_marker_an_earlier_engine_recorded() {
    let workspace = tempfile::tempdir().expect("tempdir");
    let thread = Uuid::new_v4();
    let earlier = AgentRunMarker::mint();
    record_from_another_engine(workspace.path(), thread, &earlier);

    let run = AgentRun::for_thread(workspace.path(), thread, TeardownHandover::default());

    assert_eq!(
        run.marker(),
        &earlier,
        "the resumed session carries the old marker"
    );
}

/// Inside one engine the previous session's sweep may still be running, so
/// adopting its marker would let that sweep kill the new session.
#[test]
fn a_thread_session_never_adopts_a_marker_this_engine_recorded() {
    let workspace = tempfile::tempdir().expect("tempdir");
    let thread = Uuid::new_v4();

    let first = AgentRun::for_thread(workspace.path(), thread, TeardownHandover::default());
    let second = AgentRun::for_thread(workspace.path(), thread, TeardownHandover::default());
    let recorded = std::fs::read_to_string(record_path(workspace.path(), thread)).expect("record");

    assert_ne!(
        first.marker(),
        second.marker(),
        "each session mints its own"
    );
    assert!(
        recorded.trim_end().ends_with(second.marker().value()),
        "the record names the latest session: {recorded}"
    );
}

#[test]
fn an_unreadable_record_mints_a_fresh_marker() {
    let workspace = tempfile::tempdir().expect("tempdir");
    let thread = Uuid::new_v4();
    let path = record_path(workspace.path(), thread);
    std::fs::create_dir_all(path.parent().expect("record dir")).expect("create record dir");
    std::fs::write(&path, "garbage").expect("write");

    let run = AgentRun::for_thread(workspace.path(), thread, TeardownHandover::default());

    assert_eq!(
        AgentRunMarker::parse(run.marker().value()).as_ref(),
        Some(run.marker()),
        "a fresh marker"
    );
}

/// A failed write leaves the old record in place. Adopting from it would hand
/// the next session in this engine the same marker, within reach of this
/// session's sweep.
#[cfg(unix)]
#[test]
fn a_session_that_cannot_record_its_marker_never_adopts_one() {
    use std::os::unix::fs::PermissionsExt;
    // SAFETY: geteuid takes no arguments and cannot fail.
    if unsafe { libc::geteuid() } == 0 {
        // Root writes through a read-only directory, so no write fails here.
        return;
    }
    let workspace = tempfile::tempdir().expect("tempdir");
    let thread = Uuid::new_v4();
    let earlier = AgentRunMarker::mint();
    record_from_another_engine(workspace.path(), thread, &earlier);
    let dir = record_path(workspace.path(), thread)
        .parent()
        .expect("record dir")
        .to_path_buf();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o500)).expect("chmod");

    let run = AgentRun::for_thread(workspace.path(), thread, TeardownHandover::default());

    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).expect("chmod");
    assert_ne!(
        run.marker(),
        &earlier,
        "a session that cannot record mints its own"
    );
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn wait_a_grace(child: &mut std::process::Child) -> bool {
    std::thread::sleep(Duration::from_millis(500));
    child.try_wait().expect("try_wait").is_none()
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[tokio::test]
async fn a_handed_over_run_leaves_its_processes_running() {
    let workspace = tempfile::tempdir().expect("tempdir");
    let handover = TeardownHandover::default();
    let run = AgentRun::for_thread(workspace.path(), Uuid::new_v4(), handover.clone());
    let mut suites = sleeper(&[(AGENT_RUN_ENV, run.marker().value())]);

    handover.hand_over();
    run.finish(Duration::from_millis(200)).await;

    let alive = wait_a_grace(&mut suites);
    let _ = suites.kill();
    let _ = suites.wait();
    assert!(alive, "a teardown that promises a resume reaps nothing");
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[tokio::test]
async fn a_run_not_handed_over_reaps_on_finish() {
    let workspace = tempfile::tempdir().expect("tempdir");
    let run = AgentRun::for_thread(
        workspace.path(),
        Uuid::new_v4(),
        TeardownHandover::default(),
    );
    let mut escaped = sleeper(&[(AGENT_RUN_ENV, run.marker().value())]);

    run.finish(Duration::from_millis(200)).await;

    let died = wait_for_exit(&mut escaped);
    let _ = escaped.kill();
    let _ = escaped.wait();
    assert!(died, "a turn end still reaps (ADR 0391)");
}

/// The handed-over processes stay bounded: the session that resumes the thread
/// carries the same marker, so its own end reaps them.
#[cfg(any(target_os = "macos", target_os = "linux"))]
#[tokio::test]
async fn the_resumed_session_reaps_what_the_handed_over_run_left() {
    let workspace = tempfile::tempdir().expect("tempdir");
    let thread = Uuid::new_v4();
    let handover = TeardownHandover::default();
    let before = AgentRun::for_thread(workspace.path(), thread, handover.clone());
    let mut suites = sleeper(&[(AGENT_RUN_ENV, before.marker().value())]);
    handover.hand_over();
    let marker = before.marker().clone();
    before.finish(Duration::from_millis(200)).await;
    // What the next engine finds on disk.
    record_from_another_engine(workspace.path(), thread, &marker);

    let resumed = AgentRun::for_thread(workspace.path(), thread, TeardownHandover::default());
    resumed.finish(Duration::from_millis(200)).await;

    let died = wait_for_exit(&mut suites);
    let _ = suites.kill();
    let _ = suites.wait();
    assert!(
        died,
        "the resumed session's end reaps the handed-over processes"
    );
}
