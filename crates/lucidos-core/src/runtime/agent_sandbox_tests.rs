use super::*;
use crate::test_support::{setup_test_db, teardown_test_db};

fn unavailable() -> Machine {
    Machine::Unavailable("no Landlock in this kernel".to_string())
}

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
async fn an_enforced_spawn_cannot_create_a_legacy_grant_file() {
    let workspace = tempfile::tempdir().unwrap();
    let grants = workspace.path().join(".lucidos/cc-allowed-tools");
    std::fs::create_dir_all(grants.parent().unwrap()).unwrap();
    let mut cmd = enforced_for(workspace.path()).command("/bin/sh").unwrap();
    let status = cmd
        .arg("-c")
        .arg(format!("echo Bash >> '{}'", grants.display()))
        .status()
        .await
        .unwrap();
    assert!(!status.success());
    assert!(!grants.exists());
}

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

#[cfg(target_os = "macos")]
#[test]
fn the_ps_shim_is_an_executable_the_engine_rewrites_when_the_cli_moves() {
    use std::os::unix::fs::PermissionsExt;
    let workspace = tempfile::tempdir().unwrap();
    let cli = tempfile::tempdir().unwrap();
    std::fs::write(cli.path().join(super::LUCIDOS_BIN_NAME), "").unwrap();

    let dir = ps_shim_dir_if(true, workspace.path(), Some(cli.path())).expect("a shim dir");
    assert_eq!(dir, workspace.path().join(PS_SHIM_SUBPATH));
    let shim = dir.join("ps");
    let mode = std::fs::metadata(&shim).unwrap().permissions().mode();
    assert_eq!(mode & 0o111, 0o111);
    assert!(std::fs::read_to_string(&shim)
        .unwrap()
        .contains(&cli.path().display().to_string()));

    let moved = tempfile::tempdir().unwrap();
    std::fs::write(moved.path().join(super::LUCIDOS_BIN_NAME), "").unwrap();
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
    assert!(
        error.contains("error communicating with database"),
        "{error}"
    );
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

fn private_paths_fixture() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
    use lucidos_home::private;
    let scratch = tempfile::tempdir().unwrap();
    let workspace = scratch.path().join("workspace");
    let protected = scratch.path().join("private-folder");
    std::fs::create_dir_all(&protected).unwrap();
    let path = protected.join("confidential.txt");
    std::fs::write(&path, "placeholder").unwrap();
    let alias = scratch.path().join("alias");
    std::os::unix::fs::symlink(&protected, &alias).unwrap();
    assert!(private::replace_paths(
        &private::workspace_paths_file(&workspace),
        &[],
        std::slice::from_ref(&alias),
    )
    .unwrap());
    (scratch, workspace, protected, alias)
}

#[test]
fn private_paths_are_shared_by_the_sandbox_and_file_tools() {
    let (scratch, workspace, protected, alias) = private_paths_fixture();
    let path = protected.join("confidential.txt");
    let mut policy = policy_for(&workspace);
    policy.open_within_hidden.push(protected.clone());
    policy.writable_within_read_only.push(protected);
    assert!(policy.hides(&path));
    assert!(policy.hides(&alias.join("future.txt")));
    let check = private_file_check(&workspace);
    assert!(check(&path).is_err());
    assert!(check(&alias.join("future.txt")).is_err());
    assert!(check(&scratch.path().join("sibling.txt")).is_ok());
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
#[ignore = "confinement test: ./scripts/confinement-tests.sh runs it"]
fn private_paths_deny_native_reads_and_removals() {
    if let Availability::Unavailable(why) = lucidos_agent_sandbox::probe() {
        panic!("the real sandbox is unavailable: {why}");
    }
    let (scratch, workspace, protected, _) = private_paths_fixture();
    let path = protected.join("confidential.txt");
    let mut policy = policy_for(&workspace);
    policy.open_within_hidden.push(protected.clone());
    policy.writable_within_read_only.push(protected);
    let run = |program: &str, path: &Path| {
        lucidos_agent_sandbox::command(program, &policy)
            .unwrap()
            .arg(path)
            .output()
            .unwrap()
            .status
            .success()
    };
    let public = scratch.path().join("public.txt");
    std::fs::write(&public, "public placeholder").unwrap();
    assert!(run("/bin/cat", &public));
    assert!(!run("/bin/cat", &path));
    assert!(!run("/bin/rm", &path));
}

#[test]
#[ignore = "confinement test: ./scripts/confinement-tests.sh runs it"]
fn trigger_write_scope_preserves_base_policy_and_fallback_modes() {
    let workspace = Path::new("/workspaces/example");
    let read_only = workspace.join(".lucidos");
    let policy = Policy {
        read_only: vec![read_only.clone()],
        ..Policy::default()
    };
    let scope = Sandbox::enforced_for_test(policy.clone())
        .with_trigger_write_scope(workspace, &[], true)
        .policy_for_execution(workspace);
    assert_eq!(scope.read_only, vec![read_only]);
    let roots = scope.write_scope.unwrap();
    assert!(roots.iter().any(|root| root == workspace));
    assert!(roots.contains(&PathBuf::from("/dev")));
    assert!(temp_dirs::roots().iter().all(|root| roots.contains(root)));
    for (sandbox, supported) in [
        (Sandbox::enforced_for_test(policy), false),
        (Sandbox::inherited(), true),
        (Sandbox::unsandboxed(), true),
    ] {
        assert_eq!(
            sandbox
                .with_trigger_write_scope(workspace, &[], supported)
                .mode(),
            sandbox.mode()
        );
        assert!(sandbox
            .with_trigger_write_scope(workspace, &[], supported)
            .policy_for_execution(workspace)
            .write_scope
            .is_none());
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[tokio::test]
#[ignore = "confinement test: ./scripts/confinement-tests.sh runs it"]
async fn trigger_write_scope_blocks_outside_writes_in_descendants() {
    if !lucidos_agent_sandbox::guards_truncation() {
        return;
    }
    let fixture = tempfile::Builder::new()
        .prefix("trigger-scope-")
        .tempdir_in(std::env::current_dir().unwrap())
        .unwrap();
    let workspace = fixture.path().join("workspace");
    let granted = fixture.path().join("granted");
    let outside = fixture.path().join("outside");
    for dir in [&workspace, &granted, &outside] {
        std::fs::create_dir(dir).unwrap();
    }
    let original = outside.join("existing");
    std::fs::write(&original, "keep").unwrap();
    let sandbox = Sandbox::enforced_for_test(Policy::default())
        .for_trigger(&workspace, std::slice::from_ref(&granted));
    let scratch = tempfile::tempdir_in(
        temp_dirs::roots()
            .iter()
            .find(|root| root.is_dir())
            .unwrap(),
    )
    .unwrap();
    let result = sandbox
        .command("/bin/sh")
        .unwrap()
        .args([
            "-c",
            r#"
            set -e
            printf ok > "$1"
            printf ok > "$2"
            printf ok > "$3"
            printf ok > /dev/null
            printf '' > /dev/stderr
            printf '' > /dev/fd/2
            if [ -d /dev/shm ]; then
                printf ok > /dev/shm/lucidos-trigger-scope-$$
                rm /dev/shm/lucidos-trigger-scope-$$
            fi
            if /bin/sh -c 'printf bad > "$1"' sh "$4"; then
                exit 1
            fi
            if /bin/sh -c ': > "$1"' sh "$5"; then
                exit 2
            fi
            if /bin/sh -c 'mv "$1" "$1.moved"' sh "$5"; then
                exit 3
            fi
            if [ "$6" = metadata ]; then
                if /bin/sh -c 'chmod 600 "$1"' sh "$5"; then
                    exit 4
                fi
                if /bin/sh -c 'touch -t 200001010000 "$1"' sh "$5"; then
                    exit 5
                fi
            fi
        "#,
            "sh",
        ])
        .args([
            workspace.join("write"),
            granted.join("write"),
            scratch.path().join("write"),
            outside.join("new"),
            original.clone(),
            PathBuf::from(if lucidos_agent_sandbox::guards_metadata() {
                "metadata"
            } else {
                "contents"
            }),
        ])
        .output()
        .await
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(!outside.join("new").exists());
    assert_eq!(std::fs::read_to_string(original).unwrap(), "keep");
}

#[tokio::test]
#[ignore = "confinement test: ./scripts/confinement-tests.sh runs it"]
async fn a_successful_command_never_reports_a_write_refusal() {
    let workspace = Path::new("/workspaces/example");
    let sandbox = Sandbox::enforced_for_test(Policy::default()).with_trigger_write_scope(
        workspace,
        &[],
        true,
    );
    let started = chrono::Utc::now() - chrono::Duration::minutes(1);
    let success = crate::core::shell::TaskOutcome::Exited(0);
    assert_eq!(
        sandbox
            .seatbelt_write_refusal(Some(std::process::id()), started, &success)
            .await,
        None
    );
}

#[cfg(target_os = "macos")]
#[tokio::test]
#[ignore = "confinement test: ./scripts/confinement-tests.sh runs it"]
async fn a_failed_trigger_command_finds_its_seatbelt_denial_in_the_log() {
    let fixture = tempfile::Builder::new()
        .prefix("trigger-log-")
        .tempdir_in(std::env::current_dir().unwrap())
        .unwrap();
    let workspace = fixture.path().join("workspace");
    let outside = fixture.path().join("outside");
    for dir in [&workspace, &outside] {
        std::fs::create_dir(dir).unwrap();
    }
    let sandbox = Sandbox::enforced_for_test(Policy::default()).for_trigger(&workspace, &[]);
    let started = chrono::Utc::now();
    let child = sandbox
        .command("/bin/sh")
        .unwrap()
        .args(["-c", r#"printf bad > "$1""#, "sh"])
        .arg(outside.join("new"))
        .spawn()
        .unwrap();
    let pid = child.id();
    let output = child.wait_with_output().await.unwrap();
    let outcome = crate::core::shell::TaskOutcome::from_status(output.status);
    assert!(!outcome.is_success());
    let mut denial = None;
    for _ in 0..10 {
        denial = sandbox.seatbelt_write_refusal(pid, started, &outcome).await;
        if denial.is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    let denial = denial.expect("the log holds the denial");
    assert!(denial.ends_with("/outside/new"), "{denial}");
}
