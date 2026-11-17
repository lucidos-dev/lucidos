use super::*;
use crate::test_support::{setup_test_db, teardown_test_db};

fn unavailable() -> Machine {
    Machine::Unavailable("no Landlock in this kernel".to_string())
}

/// The sandbox an agent of `workspace` gets, enforced for real. Panics when
/// this host cannot apply it, so a confinement test never passes unconfined.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn enforced_for(workspace: &Path) -> Sandbox {
    if let lucidos_agent_sandbox::Availability::Unavailable(why) = lucidos_agent_sandbox::probe() {
        panic!("the real sandbox is unavailable: {why}");
    }
    prepare_workspace(workspace);
    Sandbox::enforced_for_test(policy_for(workspace))
}

#[test]
fn refusal_names_the_positive_control_and_stored_opt_out() {
    let message = refusal("no sandbox available");
    assert!(message.contains("turns off Settings → Permissions → Sandbox agents"));
    assert!(message.contains("agents_unsandboxed=true"));
}

#[test]
fn the_owner_preference_wins_over_the_probe() {
    let cases = [
        (Machine::Available, false, SandboxStatus::Enforced),
        (Machine::Available, true, SandboxStatus::OwnerOptedOut),
        (Machine::Inherited, false, SandboxStatus::Inherited),
        (unavailable(), true, SandboxStatus::OwnerOptedOut),
        (
            unavailable(),
            false,
            SandboxStatus::Unavailable {
                reason: "no Landlock in this kernel".to_string(),
            },
        ),
    ];
    for (machine, opted_out, expected) in cases {
        assert_eq!(status_from(&machine, opted_out), expected);
    }
}

/// A seatbelt refuses to nest. Under the marker, a failed probe means an
/// outer Lucidos sandbox already holds this engine, not a broken machine.
#[test]
fn a_failed_probe_under_the_marker_is_an_inherited_sandbox() {
    let failed = || Availability::Unavailable("sandbox_apply: Operation not permitted".into());
    assert_eq!(machine_from(failed(), true), Machine::Inherited);
    assert!(matches!(
        machine_from(failed(), false),
        Machine::Unavailable(_)
    ));
    assert_eq!(
        machine_from(Availability::Available, true),
        Machine::Available
    );
}

/// A home shaped like an upgraded one: a private subtree, an older entry left
/// in place, and a gateway data dir holding one workspace.
#[test]
fn the_policy_hides_secrets_and_protects_every_workspace() {
    let scratch = tempfile::tempdir().unwrap();
    let home = scratch.path().join(".lucidos");
    let data = home.join("gateway");
    let listed = data.join("workspaces/ws-1");
    let own = scratch.path().join("elsewhere/dev");
    std::fs::create_dir_all(&listed).unwrap();
    let policy = policy_from(&PolicyInputs {
        lucidos_home: Some(home.clone()),
        data_dirs: vec![data.clone()],
        workspaces: vec![own.clone()],
    });
    for hidden in [
        home.join("private"),
        home.join("local-token"),
        home.join("network.toml"),
        home.join(".local-token.lock"),
        data.clone(),
    ] {
        assert!(policy.hidden.contains(&hidden), "{hidden:?}: {policy:?}");
    }
    assert_eq!(policy.open_within_hidden, vec![data.join("workspaces")]);
    // An agent that could edit the registry or the Trash could mark a user's
    // workspace machine-made, and the broker would skip the user's card.
    let registry = data.join("config/workspaces.json");
    let trash = data.join("deleted");
    std::fs::create_dir_all(registry.parent().unwrap()).unwrap();
    std::fs::write(&registry, "{}").unwrap();
    std::fs::create_dir_all(&trash).unwrap();
    for hidden in [&registry, &trash] {
        assert!(policy.hides(hidden), "{hidden:?}: {policy:?}");
    }
    assert!(!policy.hides(&listed));
    for ws in [&own, &listed] {
        assert!(policy.read_only.contains(&ws.join(".lucidos")));
        for writable in AGENT_WRITABLE_SUBPATHS {
            assert!(policy
                .writable_within_read_only
                .contains(&ws.join(writable)));
        }
    }
}

#[test]
fn caches_agents_use_stay_open() {
    let home = PathBuf::from("/h/.lucidos");
    let policy = policy_from(&PolicyInputs {
        lucidos_home: Some(home.clone()),
        data_dirs: vec![],
        workspaces: vec![],
    });
    for open in ["venvs", "knowhow", "build-slots", "tmp"] {
        assert!(!policy.hidden.iter().any(|h| home.join(open).starts_with(h)));
    }
}

#[tokio::test]
async fn an_unsandboxable_machine_refuses_and_tells_the_owner_once() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let workspace = tempfile::tempdir().unwrap();
    for _ in 0..2 {
        let refused = resolve_under(&pool, Some(&bus), &unavailable(), workspace.path())
            .await
            .unwrap_err()
            .to_string();
        assert!(
            refused.contains(prefs::AGENTS_UNSANDBOXED.spec.key),
            "{refused}"
        );
        assert!(refused.contains("no Landlock in this kernel"), "{refused}");
        assert!(
            refused.contains("knowhow system-knowhow/agent-sandbox"),
            "{refused}"
        );
    }
    let notices: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM events WHERE event_type = 'NotificationCreated'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(notices, 1);
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn an_opted_out_owner_runs_the_program_bare() {
    let (pool, db) = setup_test_db().await;
    sqlx::query("INSERT INTO preferences (key, value) VALUES ($1, 'true')")
        .bind(prefs::AGENTS_UNSANDBOXED.spec.key)
        .execute(&pool)
        .await
        .unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let mut cmd = resolve_under(&pool, None, &unavailable(), workspace.path())
        .await
        .unwrap()
        .command("true")
        .unwrap();
    assert_eq!(cmd.as_std().get_program(), "true");
    assert!(
        !cmd.as_std().get_envs().any(|(key, _)| key == SANDBOXED_ENV),
        "an unsandboxed child must not claim a sandbox"
    );
    assert!(cmd.status().await.unwrap().success());
    teardown_test_db(&db).await;
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[tokio::test]
#[ignore = "confinement test: ./scripts/confinement-tests.sh runs it"]
async fn an_enforced_spawn_cannot_write_a_workspace_grant() {
    let workspace = tempfile::tempdir().unwrap();
    let grants = workspace.path().join(".lucidos/cc-allowed-tools");
    std::fs::create_dir_all(grants.parent().unwrap()).unwrap();
    std::fs::write(&grants, "Read\n").unwrap();
    let mut cmd = enforced_for(workspace.path()).command("/bin/sh").unwrap();
    let status = cmd
        .arg("-c")
        .arg(format!("echo Bash >> '{}'", grants.display()))
        .status()
        .await
        .unwrap();
    assert!(!status.success());
    assert_eq!(std::fs::read_to_string(&grants).unwrap(), "Read\n");
}

/// A nested engine's children run bare, inside the sandbox it inherited, and
/// pass the marker on so their own engines know too.
#[tokio::test]
async fn an_inherited_sandbox_spawns_bare_and_passes_the_marker_on() {
    let (pool, db) = setup_test_db().await;
    let workspace = tempfile::tempdir().unwrap();
    let sandbox = resolve_under(&pool, None, &Machine::Inherited, workspace.path())
        .await
        .unwrap();
    assert!(sandbox.is_active() && !sandbox.is_enforced());
    let cmd = sandbox.command("true").unwrap();
    assert_eq!(cmd.as_std().get_program(), "true");
    assert!(cmd
        .as_std()
        .get_envs()
        .any(|(key, value)| key == SANDBOXED_ENV && value.is_some()));
    teardown_test_db(&db).await;
}

fn permission(action: &str, ended: bool) -> PermissionRow {
    PermissionRow {
        permission_id: uuid::Uuid::new_v4(),
        action: action.to_string(),
        breadth: None,
        lifetime: "standing".to_string(),
        holder_kind: "workspace".to_string(),
        holder_id: None,
        granted_in_thread_id: Some(uuid::Uuid::new_v4()),
        granted_at: chrono::Utc::now(),
        ended_at: ended.then(chrono::Utc::now),
        end_reason: ended.then(|| "revoked".to_string()),
        pattern: None,
        source: crate::core::grants::GrantSource::Card.wire().to_string(),
        legacy_unmapped: false,
    }
}

/// Only a live `run-confinement-tests` permission builds the bare mode. Its
/// child claims no sandbox, so the confinement tests apply their own.
#[test]
fn only_a_live_confinement_permission_runs_bare() {
    let sandbox = Sandbox::outside_for_confinement(&permission("run-confinement-tests", false))
        .expect("a live confinement permission builds it");
    assert!(!sandbox.is_active() && !sandbox.is_enforced());
    let cmd = sandbox.command("true").unwrap();
    assert_eq!(cmd.as_std().get_program(), "true");
    assert!(!cmd.as_std().get_envs().any(|(key, _)| key == SANDBOXED_ENV));

    assert!(Sandbox::outside_for_confinement(&permission("run-confinement-tests", true)).is_none());
    assert!(Sandbox::outside_for_confinement(&permission("run-command", false)).is_none());
}

#[test]
fn confined_writes_reopen_only_the_given_roots() {
    let policy = policy_from(&PolicyInputs {
        lucidos_home: Some(PathBuf::from("/h/.lucidos")),
        data_dirs: vec![PathBuf::from("/h/.lucidos/gateway")],
        workspaces: vec![PathBuf::from("/w")],
    });
    let confined = confine_writes(&policy, &[PathBuf::from("/w/.lucidos/worktrees/t1")]);
    assert!(confined.read_only.contains(&PathBuf::from("/")));
    assert!(confined
        .read_only
        .contains(&PathBuf::from("/h/.lucidos/gateway/workspaces")));
    assert!(confined
        .writable_within_read_only
        .contains(&PathBuf::from("/w/.lucidos/worktrees/t1")));
    assert!(
        !confined
            .writable_within_read_only
            .contains(&PathBuf::from("/w/.lucidos/worktrees")),
        "a sibling worktree stays read-only"
    );
    assert_eq!(confined.hidden, policy.hidden);
}

/// Codex's invariant: writes land in its roots and nowhere else.
#[cfg(any(target_os = "macos", target_os = "linux"))]
#[tokio::test]
#[ignore = "confinement test: ./scripts/confinement-tests.sh runs it"]
async fn a_confined_spawn_writes_only_under_its_roots() {
    let workspace = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
    let root = workspace.path().join(".lucidos/worktrees/t1");
    std::fs::create_dir_all(&root).unwrap();
    let sandbox = enforced_for(workspace.path()).writes_only_under(std::slice::from_ref(&root));
    assert_eq!(sandbox.compile_cache(), AgentSandbox::WritesConfined);
    let write = |dir: &Path| {
        let mut cmd = sandbox.command("/bin/sh").unwrap();
        cmd.arg("-c")
            .arg(format!("echo x > '{}'", dir.join("f").display()));
        cmd
    };
    let allowed = write(&root).output().await.unwrap();
    assert!(allowed.status.success(), "allowed write: {allowed:?}");
    let refused = write(outside.path()).output().await.unwrap();
    assert!(!refused.status.success(), "outside write: {refused:?}");
    assert!(!outside.path().join("f").exists());
}

#[test]
fn only_an_active_sandbox_on_macos_gets_the_ps_shim() {
    for (active, macos, wanted) in [
        (true, true, true),
        (true, false, false),
        (false, true, false),
        (false, false, false),
    ] {
        assert_eq!(ps_shim_wanted(active, macos), wanted, "{active} {macos}");
    }
}

#[test]
fn the_ps_shim_execs_the_cli_and_quotes_its_path() {
    let script = ps_shim_script(Path::new("/opt/it's here/lucidos"));
    assert_eq!(
        script,
        "#!/bin/sh\nexec '/opt/it'\\''s here/lucidos' ps \"$@\"\n"
    );
}

// Only macOS writes a shim at all.
#[cfg(target_os = "macos")]
#[test]
fn the_ps_shim_is_an_executable_the_engine_rewrites_when_the_cli_moves() {
    use std::os::unix::fs::PermissionsExt;
    let workspace = tempfile::tempdir().unwrap();
    let cli = tempfile::tempdir().unwrap();
    std::fs::write(
        cli.path().join(super::super::lucidos_cli::LUCIDOS_BIN_NAME),
        "",
    )
    .unwrap();

    let dir = ps_shim_dir_if(true, workspace.path(), Some(cli.path())).expect("a shim dir");
    assert_eq!(dir, workspace.path().join(PS_SHIM_SUBPATH));
    let shim = dir.join("ps");
    let mode = std::fs::metadata(&shim).unwrap().permissions().mode();
    assert_eq!(mode & 0o111, 0o111);
    assert!(std::fs::read_to_string(&shim)
        .unwrap()
        .contains(&cli.path().display().to_string()));

    let moved = tempfile::tempdir().unwrap();
    std::fs::write(
        moved
            .path()
            .join(super::super::lucidos_cli::LUCIDOS_BIN_NAME),
        "",
    )
    .unwrap();
    ps_shim_dir_if(true, workspace.path(), Some(moved.path())).expect("a shim dir");
    assert!(std::fs::read_to_string(&shim)
        .unwrap()
        .contains(&moved.path().display().to_string()));
}

#[test]
fn no_cli_or_no_sandbox_leaves_no_ps_shim() {
    let workspace = tempfile::tempdir().unwrap();
    let cli = tempfile::tempdir().unwrap();
    assert_eq!(ps_shim_dir_if(true, workspace.path(), None), None);
    assert_eq!(
        ps_shim_dir_if(true, workspace.path(), Some(cli.path())),
        None,
        "a CLI dir without the binary"
    );
    assert_eq!(
        ps_shim_dir_if(false, workspace.path(), Some(cli.path())),
        None
    );
    assert!(!workspace.path().join(PS_SHIM_SUBPATH).exists());
}

/// The shim is what the agent's `ps` runs. If the agent could write the dir,
/// it could replace the shim.
#[test]
fn the_ps_shim_dir_is_read_only_to_agents() {
    let ws = PathBuf::from("/w/dev");
    let policy = policy_from(&PolicyInputs {
        lucidos_home: None,
        data_dirs: vec![],
        workspaces: vec![ws.clone()],
    });
    let shim_dir = ws.join(PS_SHIM_SUBPATH);
    assert!(shim_dir.starts_with(ws.join(".lucidos")));
    assert!(!AGENT_WRITABLE_SUBPATHS.contains(&PS_SHIM_SUBPATH));
    assert!(!policy
        .writable_within_read_only
        .iter()
        .any(|writable| shim_dir.starts_with(writable) || writable.starts_with(&shim_dir)));
}

mod guard_tests {
    /// The owner's condition: the confinement run is the one thing that runs
    /// outside the sandbox. One constructor builds that mode, and one production
    /// caller uses it.
    #[test]
    fn the_confinement_route_is_the_only_bare_spawn() {
        let sources = crate::test_support::source_scan::production_sources();
        let sites = |needle: &str| -> Vec<String> {
            sources
                .iter()
                .filter(|(_, text)| text.contains(needle))
                .map(|(path, text)| format!("{path} ×{}", text.matches(needle).count()))
                .collect()
        };
        assert_eq!(
            sites("(Mode::PermittedOutside)"),
            ["runtime/agent_sandbox.rs ×1"],
            "only outside_for_confinement may build the bare mode"
        );
        let mut callers = sites("outside_for_confinement(");
        callers.sort();
        assert_eq!(
            callers,
            [
                "engine/agent_session/confinement_run.rs ×1",
                "runtime/agent_sandbox.rs ×1",
            ],
            "only the confinement route may run bare, beside the definition"
        );
    }

    /// Every covered spawn site (decision 6 of
    /// `docs/plans/2026-11-10-os-sandbox-for-agent-subprocesses.md`) builds its
    /// child from a [`Sandbox`]. A raw command in one of them is the hole this
    /// pins shut. Each allowed raw command names why it runs no agent code.
    #[test]
    fn every_covered_spawn_site_builds_from_a_sandbox() {
        use crate::test_support::source_scan::{read_production_source, src_root};
        let covered: [(&str, &[&str]); 12] = [
            ("engine/tools/bash.rs", &[]),
            // `ps` reads a sidecar's pid before any agent code runs.
            (
                "engine/frontend_preview.rs",
                &["std::process::Command::new(\"ps\")"],
            ),
            ("engine/tools/bash_background.rs", &[]),
            // The venv is made by the system python, before any agent code.
            (
                "runtime/python.rs",
                &["tokio::process::Command::new(\"python3\")"],
            ),
            ("engine/engine_impl/scripts.rs", &[]),
            ("api/proxy_script_runner.rs", &[]),
            ("engine/agent_session/background_task.rs", &[]),
            ("mcp/client.rs", &[]),
            ("runtime/claude_code.rs", &[]),
            // Never spawned: it only collects the env the turns re-apply.
            (
                "runtime/codex.rs",
                &["tokio::process::Command::new(\"true\")"],
            ),
            ("runtime/codex_app_server.rs", &[]),
            ("core/shell.rs", &[]),
        ];
        for (file, allowed) in covered {
            let source = read_production_source(&src_root().join(file));
            let mut rest = source.clone();
            for raw in allowed {
                assert!(rest.contains(raw), "{file}: the allowed `{raw}` is gone");
                rest = rest.replacen(raw, "", 1);
            }
            let code: String = rest
                .lines()
                .filter(|line| !line.trim_start().starts_with("//"))
                .collect::<Vec<_>>()
                .join("\n");
            assert!(
                !code.contains("Command::new("),
                "{file} builds a raw command; build it from a Sandbox"
            );
            assert!(
                code.contains("Sandbox") || code.contains("sandbox"),
                "{file} no longer reaches the sandbox"
            );
        }
    }
}

#[test]
fn resolved_modes_have_stable_wire_values() {
    let cases = [
        (
            Mode::Enforced {
                policy: Arc::new(Policy::default()),
                writes_confined: false,
            },
            "enforced",
        ),
        (Mode::Inherited, "inherited"),
        (Mode::OwnerOptedOut, "owner-opted-out"),
        (Mode::PermittedOutside, "permitted-outside"),
    ];
    for (mode, expected) in cases {
        let sandbox = Sandbox(mode);
        let wire = serde_json::to_value(sandbox.mode()).unwrap();
        assert_eq!(wire, expected);
        assert_eq!(
            serde_json::from_value::<SandboxMode>(wire).unwrap(),
            sandbox.mode()
        );
    }
}

#[test]
fn legacy_session_started_has_no_sandbox_record() {
    use crate::engine::thread_events::ThreadEvent;
    let event: ThreadEvent = serde_json::from_value(serde_json::json!({
        "type": "SessionStarted", "session_id": "legacy"
    }))
    .unwrap();
    assert!(matches!(
        event,
        ThreadEvent::SessionStarted { sandbox: None, .. }
    ));
    assert!(serde_json::to_value(event)
        .unwrap()
        .get("sandbox")
        .is_none());
}

/// A head grant inside an open workspace cannot open a symlink into hidden storage.
#[cfg(unix)]
#[tokio::test]
#[ignore = "confinement test: ./scripts/confinement-tests.sh runs it"]
async fn workspace_carve_out_cannot_follow_links_before_or_after_a_grant() {
    use crate::engine::command_guard::{self, CommandSite, TrustedHeads};
    let scratch = tempfile::tempdir().unwrap();
    let gateway = scratch.path().join("gateway");
    let workspace = gateway.join("workspaces/demo");
    let private = scratch.path().join("private");
    for dir in [&workspace, &gateway.join("config"), &private] {
        std::fs::create_dir_all(dir).unwrap();
    }
    std::fs::write(gateway.join("config/key"), "hidden").unwrap();
    std::fs::write(private.join("key"), "hidden").unwrap();
    let policy = Policy {
        hidden: vec![gateway.clone(), private.clone()],
        open_within_hidden: vec![gateway.join("workspaces")],
        ..Default::default()
    };
    let sandbox = Sandbox::enforced_for_test(policy);
    let paths = command_guard::SecretPaths::for_sandbox(&sandbox, &workspace);
    for granted in [false, true] {
        for (name, target) in [
            ("gateway-link", gateway.join("config")),
            ("private-link", private.clone()),
        ] {
            let link = workspace.join(format!("{name}-{granted}"));
            std::os::unix::fs::symlink(target, &link).unwrap();
            let file = link.join("key");
            let command = format!("cat '{}'", file.display());
            assert_eq!(
                command_guard::grant_covers_command_with_paths(
                    "Bash",
                    &command,
                    CommandSite::TextOnly { root: None },
                    &TrustedHeads::NONE,
                    &paths,
                    |_| granted
                ),
                granted
            );
            let output = sandbox
                .command("/bin/cat")
                .unwrap()
                .arg(&file)
                .output()
                .await
                .unwrap();
            assert!(
                !output.status.success(),
                "a link escaped the executing sandbox"
            );
            assert!(output.stdout.is_empty());
        }
    }
}

#[test]
fn the_database_socket_follows_the_agent_url_and_ignores_tcp() {
    let url = "postgres:///workspace?host=%2Fprivate%2Frun%2Fpg&port=5544&user=agent";
    assert_eq!(
        database_socket(url).unwrap(),
        Some(PathBuf::from("/private/run/pg/.s.PGSQL.5544"))
    );
    assert_eq!(
        database_socket("postgres://agent@localhost/workspace").unwrap(),
        None
    );
    assert!(database_socket("invalid").is_err());
}

#[tokio::test]
async fn the_local_auth_check_matches_the_clusters_rules() {
    let (pool, db) = setup_test_db().await;
    let methods: Vec<Option<String>> = sqlx::query_scalar(
        "SELECT auth_method FROM pg_hba_file_rules WHERE type = 'local' OR error IS NOT NULL",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    let expected = methods.iter().all(|m| {
        m.as_deref()
            .is_some_and(|m| matches!(m, "scram-sha-256" | "md5" | "password" | "reject"))
    });
    assert_eq!(local_auth_requires_password(&pool).await.unwrap(), expected);
    teardown_test_db(&db).await;
}

#[cfg(unix)]
#[tokio::test]
#[ignore = "confinement test: ./scripts/confinement-tests.sh runs it"]
async fn psql_reaches_the_hidden_socket_as_the_read_only_agent() {
    use lucidos_pg_url::{Host, PgUrl};
    use tokio::net::{TcpStream, UnixListener};

    if let Availability::Unavailable(why) = lucidos_agent_sandbox::probe() {
        panic!("the real sandbox is unavailable: {why}");
    }
    let (pool, db) = setup_test_db().await;
    let database_url = crate::test_support::test_db_url(&db);
    let upstream = PgUrl::parse(&database_url).unwrap();
    let root = tempfile::tempdir().unwrap();
    let hidden = std::fs::canonicalize(root.path()).unwrap().join("pg");
    std::fs::create_dir(&hidden).unwrap();
    let socket = hidden.join(".s.PGSQL.5432");
    let listener = UnixListener::bind(&socket).unwrap();
    let forwarder = tokio::spawn(async move {
        loop {
            let (mut client, _) = listener.accept().await.unwrap();
            let mut server: Box<dyn ProxyStream> = match &upstream.host {
                Host::Tcp(host) => Box::new(
                    TcpStream::connect((host.as_str(), upstream.port))
                        .await
                        .unwrap(),
                ),
                Host::Socket(dir) => Box::new(
                    tokio::net::UnixStream::connect(
                        dir.join(format!(".s.PGSQL.{}", upstream.port)),
                    )
                    .await
                    .unwrap(),
                ),
            };
            tokio::spawn(async move {
                // psql may close either side after an error; the pump owns only test sockets.
                let _ = tokio::io::copy_bidirectional(&mut client, &mut server).await;
            });
        }
    });
    let agent = crate::core::agent_role::provision(&pool, &database_url, None)
        .await
        .unwrap();
    let mut agent = PgUrl::parse(&agent).unwrap();
    agent.host = Host::Socket(hidden.clone());
    agent.port = 5432;
    let url = agent.read_only_url();
    let mut policy = Policy {
        hidden: vec![hidden],
        ..Policy::default()
    };
    let mut admin = PgUrl::parse(&database_url).unwrap();
    admin.host = Host::Socket(policy.hidden[0].clone());
    admin.port = 5432;
    assert!(password_auth_is_effective(&admin.read_only_url())
        .await
        .unwrap());
    policy
        .connectable_sockets
        .push(database_socket(&url).unwrap().unwrap());
    let sandbox = Sandbox::enforced_for_test(policy);
    for (sql, succeeds) in [
        ("select 1; select count(*) from events", true),
        ("CREATE TABLE sandbox_write_probe (id int)", false),
    ] {
        let out = sandbox
            .command("/bin/sh")
            .unwrap()
            .env(crate::core::LUCIDOS_DATABASE_URL, &url)
            .env("SANDBOX_TEST_SQL", sql)
            .args([
                "-c",
                "psql \"$LUCIDOS_DATABASE_URL\" -X -v ON_ERROR_STOP=1 -c \"$SANDBOX_TEST_SQL\"",
            ])
            .output()
            .await
            .unwrap();
        assert_eq!(
            out.status.success(),
            succeeds,
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        if !succeeds {
            assert!(String::from_utf8_lossy(&out.stderr).contains("read-only transaction"));
        }
    }
    forwarder.abort();
    teardown_test_db(&db).await;
}

#[cfg(unix)]
trait ProxyStream: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send {}
#[cfg(unix)]
impl<T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send> ProxyStream for T {}

#[path = "agent_sandbox_native_tests.rs"]
mod native;

#[tokio::test]
async fn an_unreachable_auth_probe_fails_closed_without_connection_details() {
    let url = "postgres://fixture_admin:do-not-print-this@127.0.0.1:0/postgres";
    let error = password_auth_is_effective(url)
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("Could not verify password rejection"));
    assert!(!error.contains("do-not-print-this"));
    assert!(!error.contains(url));
}

#[tokio::test]
async fn the_database_authentication_check_times_out_before_any_spawn() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(async move {
        let (connection, _) = listener.accept().await.unwrap();
        std::future::pending::<()>().await;
        drop(connection);
    });
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect_lazy(&format!(
            "postgres://fixture_admin@127.0.0.1:{port}/postgres"
        ))
        .unwrap();
    let url = "postgres:///postgres?host=/tmp/lucidos-auth-stalled&user=fixture_admin";
    let result =
        verify_database_socket(&pool, database_socket(url).unwrap().unwrap(), url, true).await;
    assert!(result
        .err()
        .unwrap()
        .to_string()
        .contains("Timed out verifying"));
    server.abort();
    pool.close().await;
}
