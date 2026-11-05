//! Session-scoped gateway router and engines under a private HOME. Never boots
//! the machine-global gateway or registers a real user's workspace.
use super::*;
use serde_json::{json, Value};
use std::os::unix::fs::PermissionsExt;

const TEST: &str = "server::snapshot_roundtrip_tests::isolated_snapshot_cli_roundtrip";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "Run through scripts/e2e-api.sh with the built engine and CLI"]
async fn isolated_snapshot_cli_roundtrip() -> Result<(), BoxError> {
    if std::env::var_os("E2E_SNAPSHOT_CHILD").is_some() {
        return child().await;
    }
    let variants: &[bool] = if std::env::var_os("E2E_SNAPSHOT_BROWSER").is_some() {
        &[false]
    } else {
        &[false, true]
    };
    for &relocated in variants {
        run_fixture(relocated).await?;
    }
    Ok(())
}

async fn run_fixture(relocated: bool) -> Result<(), BoxError> {
    let home = tempfile::tempdir()?;
    let root = home.path().join(".lucidos");
    let app_data = root.join(if relocated {
        "relocated-gateway"
    } else {
        lucidos_home::GATEWAY_DIR_NAME
    });
    lucidos_home::ensure_storage_root(&root)?;
    let stem = format!("snapshot-e2e-{}", lucidos_local_token::mint_hex(12)?);
    std::fs::write(root.join("snapshot-fixture"), &stem)?;
    let backend = postgres::PgBackend::from_env()?;
    let mut container = if matches!(backend, postgres::PgBackend::Docker) {
        let password = lucidos_local_token::mint_hex(24)?;
        let destination = root.join(lucidos_local_token::SHARED_PG_PASSWORD_FILE);
        std::fs::write(&destination, &password)?;
        std::fs::set_permissions(destination, std::fs::Permissions::from_mode(0o600))?;
        Some(FixturePostgres::start(&stem, &password).await?)
    } else {
        None
    };
    let log_path = root.join("fixture.log");
    let log = std::fs::File::create(&log_path)?;
    let mut command = tokio::process::Command::new(std::env::current_exe()?);
    command
        .args(["--ignored", "--exact", TEST, "--nocapture"])
        .env("HOME", home.path())
        .env("E2E_SNAPSHOT_CHILD", &stem)
        .env("LUCIDOS_SECRET_KEY_STORE", "file")
        .env("LUCIDOS_MODEL", "mock")
        .stdout(log.try_clone()?)
        .stderr(log)
        .kill_on_drop(true);
    if container.is_some() {
        command.env("LUCIDOS_GATEWAY_PG_CONTAINER", &stem);
    }
    for name in [
        "LUCIDOS_GATEWAY_DATA",
        "LUCIDOS_WORKSPACE",
        "LUCIDOS_THREAD_ID",
        "LUCIDOS_AGENT_ORIGIN_TOKEN",
        "LUCIDOS_DATABASE_URL",
        "LUCIDOS_STATIC_DIR",
        "LUCIDOS_TLS_CERT",
        "LUCIDOS_TLS_KEY",
        "LUCIDOS_PACKAGED",
    ] {
        command.env_remove(name);
    }
    if relocated {
        command.env("LUCIDOS_GATEWAY_DATA", &app_data);
    }
    let result = tokio::time::timeout(Duration::from_secs(600), command.status()).await;
    // Even a failed child cannot leave engines attached to a deleted fixture.
    for suffix in ["source", "target"] {
        let id = format!("{stem}-{suffix}");
        let workspace = app_data.join("workspaces").join(&id);
        stack::reclaim_stale_engine(&workspace);
        let deadline = Instant::now() + Duration::from_secs(30);
        while stack::live_engine_pid(&workspace).is_some() && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        if stack::live_engine_pid(&workspace).is_some() {
            let kept = home.keep();
            return Err(format!("fixture engine did not stop; retained {}", kept.display()).into());
        }
        if matches!(backend, postgres::PgBackend::Embedded { .. }) {
            postgres::teardown_workspace(&backend, &id, &app_data).await?;
        }
    }
    if let postgres::PgBackend::Embedded { bin, lib } = backend {
        postgres::stop_cluster_robustly(&bin, &lib, &app_data.join("pgdata"));
    }
    if let Some(container) = &mut container {
        container.finish()?;
    }
    if !result.is_ok_and(|status| status.is_ok_and(|status| status.success())) {
        let kept = home.keep();
        return Err(format!(
            "isolated snapshot fixture failed; log retained at {}",
            kept.join(".lucidos/fixture.log").display()
        )
        .into());
    }
    Ok(())
}

/// A private container with an anonymous volume, never the gateway's shared
/// data volume. The password is passed only through the process environment.
struct FixturePostgres {
    name: Option<String>,
}

impl FixturePostgres {
    async fn start(name: &str, password: &str) -> Result<Self, BoxError> {
        let reservation = std::net::TcpListener::bind("127.0.0.1:0")?;
        let port = reservation.local_addr()?.port();
        drop(reservation);
        let output = std::process::Command::new("docker")
            .args([
                "run",
                "--rm",
                "-d",
                "--name",
                name,
                "--memory=512m",
                "--memory-swap=512m",
                "--shm-size=128m",
                "-p",
                &format!("127.0.0.1:{port}:5432"),
                "-e",
                "POSTGRES_PASSWORD",
                "-e",
                "POSTGRES_USER",
                "-e",
                "POSTGRES_DB",
                postgres::PG_IMAGE,
                "postgres",
                "-c",
                "max_connections=100",
            ])
            .env("POSTGRES_PASSWORD", password)
            .env("POSTGRES_USER", postgres::PG_USER)
            .env("POSTGRES_DB", "postgres")
            .output()?;
        if !output.status.success() {
            return Err("private fixture Postgres did not start".into());
        }
        let fixture = Self {
            name: Some(name.to_owned()),
        };
        // Docker publishes its TCP proxy before initdb has finished. Wait for
        // the final server's TCP listener, not the temporary init socket.
        let deadline = Instant::now() + Duration::from_secs(90);
        loop {
            let ready = tokio::process::Command::new("docker")
                .args([
                    "exec",
                    name,
                    "pg_isready",
                    "-h",
                    "127.0.0.1",
                    "-U",
                    postgres::PG_USER,
                    "-d",
                    "postgres",
                ])
                .output()
                .await?;
            if ready.status.success() {
                return Ok(fixture);
            }
            if Instant::now() > deadline {
                return Err("private fixture Postgres readiness timed out".into());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    fn finish(&mut self) -> Result<(), BoxError> {
        if let Some(name) = &self.name {
            let output = std::process::Command::new("docker")
                .args(["rm", "-f", name])
                .output()?;
            if !output.status.success() {
                return Err("private fixture Postgres cleanup failed".into());
            }
            self.name = None;
        }
        Ok(())
    }
}

impl Drop for FixturePostgres {
    fn drop(&mut self) {
        if self.finish().is_err() {
            crate::log!("[SnapshotE2E] private Postgres cleanup failed");
        }
    }
}

async fn child() -> Result<(), BoxError> {
    let home = PathBuf::from(std::env::var_os("HOME").ok_or("home unavailable")?);
    let root = home.join(".lucidos");
    let app_data = lucidos_home::gateway_data_dir().ok_or("fixture gateway data unavailable")?;
    let stem = std::env::var("E2E_SNAPSHOT_CHILD")?;
    if !stem.starts_with("snapshot-e2e-")
        || std::fs::read_to_string(root.join("snapshot-fixture"))? != stem
    {
        return Err("not a disposable snapshot fixture".into());
    }
    let token = lucidos_local_token::ensure()?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let mut state =
        GatewayState::for_tests_with(Some(home), &token, false).with_live_engine_sockets();
    let inner = Arc::get_mut(&mut state.inner).ok_or("fixture state shared too soon")?;
    inner.app_data = app_data.clone();
    inner.registry_path = app_data.join("config/workspaces.json");
    inner.snapshot_home = root.clone();
    inner.gateway_port = port;
    inner.engine_bin =
        PathBuf::from(std::env::var_os("E2E_SNAPSHOT_ENGINE_BIN").ok_or("built engine missing")?);
    inner.static_dir = None;
    inner.pg_backend = postgres::PgBackend::from_env()?;
    std::fs::create_dir_all(app_data.join("config"))?;
    let mut workspaces = Vec::new();
    let mut reservations = Vec::new();
    for suffix in ["source", "target"] {
        let reservation = std::net::TcpListener::bind("127.0.0.1:0")?;
        let ws = Workspace::gateway_provisioned(
            format!("{stem}-{suffix}"),
            format!("Snapshot {suffix}"),
            reservation.local_addr()?.port(),
        );
        state.inner.registry.lock().unwrap().add(ws.clone())?;
        workspaces.push(ws);
        reservations.push(reservation);
    }
    state
        .inner
        .registry
        .lock()
        .unwrap()
        .save(&state.inner.registry_path)?;
    drop(reservations);
    let serving = state.clone();
    let server = tokio::spawn(async move { axum::serve(listener, gateway_router(serving)).await });
    let result = exercise_roundtrip(&state, port, &token, &workspaces).await;
    for ws in &workspaces {
        state
            .stop_workspace(&ws.id, None)
            .await
            .map_err(|_| "fixture stop refused")?;
    }
    server.abort();
    result
}

async fn exercise_roundtrip(
    state: &GatewayState,
    port: u16,
    token: &str,
    workspaces: &[Workspace],
) -> Result<(), BoxError> {
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .build()?;
    for ws in workspaces {
        state
            .start_workspace(&ws.id)
            .map_err(|_| "fixture start refused")?;
        let deadline = Instant::now() + Duration::from_secs(120);
        loop {
            let response = client
                .get(format!("http://127.0.0.1:{port}/{}/api/v1/health", ws.id))
                .header(crate::auth::HEADER_LOCAL_TOKEN, token)
                .send()
                .await;
            if response.is_ok_and(|response| response.status().is_success()) {
                break;
            }
            if Instant::now() > deadline {
                return Err("fixture engine boot timed out".into());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
    let target = &workspaces[1];
    let source = &workspaces[0];
    let source_api = format!("http://127.0.0.1:{}/api/v1", source.port);
    let target_api = format!("http://127.0.0.1:{}/api/v1", target.port);
    let device = "snapshot-roundtrip-owner";
    for api in [&source_api, &target_api] {
        require_success(
            client
                .post(format!("{api}/devices/register"))
                .header("content-type", "application/json")
                .body(json!({"device_id":device,"user_agent":"snapshot-e2e/1"}).to_string())
                .send()
                .await?,
            "register device",
        )?;
    }
    let owner = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .default_headers({
            let mut headers = axum::http::HeaderMap::new();
            headers.insert("x-lucidos-device-id", device.parse()?);
            headers
        })
        .build()?;
    let fake = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../lucidos-app/e2e/fake-claude-code/claude.mjs")
        .canonicalize()?;
    require_success(
        owner
            .put(format!(
                "{source_api}/preferences?key={}",
                crate::preference_test_keys::key("CODING_AGENT_CLAUDE_PATH")
            ))
            .header("content-type", "application/json")
            .body(json!({"value":fake}).to_string())
            .send()
            .await?,
        "set fixture agent",
    )?;
    let source_dir = source.resolve_dir(state.app_data()).canonicalize()?;
    let repository: Value = serde_json::from_str(&require_success(owner.post(format!("{source_api}/repositories"))
        .header("content-type", "application/json")
        .body(json!({"name":"Snapshot source","path":source_dir,"description":"Disposable snapshot source"}).to_string())
        .send().await?, "register source repository")?.text().await?)?;
    let repo_id = repository["id"]
        .as_str()
        .ok_or("source repository missing")?;
    if Path::new(repository["path"].as_str().ok_or("source path missing")?).canonicalize()?
        != source_dir
    {
        return Err("source repository escaped fixture".into());
    }
    let directory = target.resolve_dir(state.app_data());
    let directory = directory.to_str().ok_or("fixture path unavailable")?;
    let capture = cli(&["create", "--workspace", directory, "--wait", "--json"]).await?;
    assert_eq!(capture["state"]["phase"], "completed");
    assert_eq!(capture["workspace_id"], target.id);
    let handle = capture["handle"].as_str().ok_or("capture handle missing")?;
    let checked = cli(&["check", handle, "--workspace", directory, "--json"]).await?;
    assert_eq!(checked["state"]["phase"], "completed");
    let seed = "00000001-0000-4000-8000-000000000000";
    require_success(
        owner
            .post(format!("{target_api}/threads"))
            .header("content-type", "application/json")
            .body(json!({"id":seed,"mode":"lucidos"}).to_string())
            .send()
            .await?,
        "create later thread",
    )?;
    require_success(
        owner
            .put(format!("{target_api}/data/artifacts/after-snapshot.txt"))
            .header("content-type", "text/plain")
            .body("after snapshot\n")
            .send()
            .await?,
        "write later file",
    )?;
    let file = target
        .resolve_dir(state.app_data())
        .join("data/artifacts/after-snapshot.txt");
    let refused = agent_restore(&owner, &source_api, repo_id, &target.id, handle, port, 2).await?;
    assert_eq!(refused["code"], 2, "an unapproved agent cannot restore");
    assert_eq!(
        refused["status"]["error"],
        "snapshot_restore_approval_required"
    );
    assert!(file.exists());
    assert_eq!(
        owner
            .get(format!("{target_api}/threads/{seed}"))
            .send()
            .await?
            .status(),
        reqwest::StatusCode::OK
    );
    require_success(
        owner
            .put(format!(
                "{target_api}/preferences?key={}",
                crate::preference_test_keys::key("SNAPSHOT_RESTORE_BY_AGENTS")
            ))
            .header("content-type", "application/json")
            .body(json!({"value":"allow"}).to_string())
            .send()
            .await?,
        "allow fixture agent restore",
    )?;
    let restored = agent_restore(&owner, &source_api, repo_id, &target.id, handle, port, 3).await?;
    assert_eq!(restored["code"], 0, "restore CLI completion");
    assert_eq!(restored["status"]["phase"], "completed");
    assert_eq!(restored["status"]["workspace_id"], target.id);
    assert!(!file.exists());
    assert_eq!(
        owner
            .get(format!("{target_api}/threads/{seed}"))
            .send()
            .await?
            .status(),
        reqwest::StatusCode::NOT_FOUND
    );
    let snapshots = cli(&["list", "--workspace", directory, "--json"]).await?;
    let recovery = snapshots
        .as_array()
        .ok_or("snapshot list missing")?
        .iter()
        .find(|snapshot| snapshot["kind"] == "recovery")
        .ok_or("recovery snapshot missing")?;
    let recovery_handle = recovery["handle"]
        .as_str()
        .ok_or("recovery handle missing")?;
    let preferences: Value = serde_json::from_str(
        &require_success(
            owner
                .get(format!("{target_api}/preferences"))
                .send()
                .await?,
            "read restored preferences",
        )?
        .text()
        .await?,
    )?;
    let policy = preferences["preferences"]
        .as_object()
        .ok_or("preferences missing")?
        .get(crate::preference_test_keys::key(
            "SNAPSHOT_RESTORE_BY_AGENTS",
        ))
        .map(Value::as_str);
    assert_eq!(
        policy,
        Some(Some("allow")),
        "restore carries the current agent policy forward (ADR 0612)"
    );
    let undone = agent_restore(
        &owner,
        &source_api,
        repo_id,
        &target.id,
        recovery_handle,
        port,
        4,
    )
    .await?;
    assert_eq!(undone["code"], 0, "Undo CLI completion");
    assert_eq!(undone["status"]["phase"], "completed");
    assert_eq!(undone["status"]["workspace_id"], target.id);
    assert_eq!(std::fs::read_to_string(file)?, "after snapshot\n");
    assert_eq!(
        owner
            .get(format!("{target_api}/threads/{seed}"))
            .send()
            .await?
            .status(),
        reqwest::StatusCode::OK
    );
    assert_eq!(
        state
            .inner
            .registry
            .lock()
            .unwrap()
            .get(&target.id)
            .ok_or("target registration lost")?
            .dir,
        target.dir
    );
    if let Some(browser) = std::env::var_os("E2E_SNAPSHOT_BROWSER") {
        if browser != "chromium" {
            return Err("snapshot browser fixture supports Chromium only".into());
        }
        require_success(
            owner
                .put(format!(
                    "{target_api}/preferences?key={}",
                    crate::preference_test_keys::key("MEMORY_MODULE")
                ))
                .header(
                    "x-lucidos-restore-generation",
                    undone["status"]["restore_id"]
                        .as_str()
                        .ok_or("Undo generation missing")?,
                )
                .header("content-type", "application/json")
                .body(json!({"value":"classic"}).to_string())
                .send()
                .await?,
            "choose fixture memory module",
        )?;
        let label = "Isolated Settings restore";
        let captured = cli(&[
            "create",
            "--workspace",
            directory,
            "--label",
            label,
            "--wait",
            "--json",
        ])
        .await?;
        assert_eq!(captured["state"]["phase"], "completed");
        let config = json!({
            "port":port, "engine_port":target.port, "workspace":target.id, "name":target.name,
            "device":device, "token":token, "label":label,
            "handle":captured["handle"], "generation":undone["status"]["restore_id"],
        });
        let mut browser = tokio::process::Command::new("node");
        browser
            .arg(
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../lucidos-app/e2e/snapshot-settings-roundtrip.mjs"),
            )
            .env("E2E_SNAPSHOT_BROWSER_CONFIG", config.to_string())
            .kill_on_drop(true);
        let status = tokio::time::timeout(Duration::from_secs(240), browser.status()).await??;
        if !status.success() {
            return Err("isolated Settings browser restore failed".into());
        }
    }
    Ok(())
}

fn require_success(
    response: reqwest::Response,
    operation: &str,
) -> Result<reqwest::Response, BoxError> {
    if !response.status().is_success() {
        return Err(format!("fixture {operation} failed: {}", response.status()).into());
    }
    Ok(response)
}

async fn agent_restore(
    owner: &reqwest::Client,
    source_api: &str,
    repo_id: &str,
    target: &str,
    handle: &str,
    port: u16,
    turn: u8,
) -> Result<Value, BoxError> {
    let thread = format!("{turn:08x}-0000-4000-8000-000000000000");
    let cli = std::env::var("E2E_SNAPSHOT_CLI_BIN")?;
    let payload = json!({"cli":cli,"workspace":target,"handle":handle,"port":port});
    require_success(owner.post(format!("{source_api}/chat/stream"))
        .header("content-type", "application/json")
        .body(json!({"message":format!("Probe snapshot CLI restore: {payload}"),"thread_id":thread,"new_thread":true,"mode":"human","use_coding_agent":true,"coding_agent":"claude-code","repo_id":repo_id}).to_string())
        .send().await?, "start source agent")?;
    let deadline = Instant::now() + Duration::from_secs(150);
    loop {
        let events: Value = serde_json::from_str(
            &require_success(
                owner
                    .get(format!("{source_api}/threads/{thread}/events"))
                    .send()
                    .await?,
                "read source agent",
            )?
            .text()
            .await?,
        )?;
        for event in events["events"].as_array().ok_or("source events missing")? {
            match event["event_type"].as_str() {
                Some("ResponseGenerated") => {
                    return Ok(serde_json::from_str(
                        event["payload"]["text"]
                            .as_str()
                            .ok_or("source proof missing")?,
                    )?)
                }
                Some("ResponseFailed" | "ResponseAborted" | "ResponseCanceled") => {
                    return Err("source agent ended without CLI proof".into())
                }
                _ => {}
            }
        }
        if Instant::now() > deadline {
            return Err("source CLI proof timed out".into());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn cli(args: &[&str]) -> Result<Value, BoxError> {
    let output = tokio::process::Command::new(
        std::env::var_os("E2E_SNAPSHOT_CLI_BIN").ok_or("built CLI missing")?,
    )
    .arg("snapshot")
    .args(args)
    .output()
    .await?;
    if !output.status.success() {
        return Err("snapshot fixture CLI failed".into());
    }
    Ok(serde_json::from_slice(&output.stdout)?)
}
