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
        "run_gateway_supervised \"$gw_pidfile\"",
        "exec node dev-build-watch.mjs",
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

    reap_marked_groups(marker, Duration::from_millis(200)).await;

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
