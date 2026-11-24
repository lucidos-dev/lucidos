//! Real engines and real CLI children in a private HOME and database. The
//! production gateway spawn, stamp, socket and operation handlers do every hop.
use super::*;
use serde_json::{json, Value};
use snapshot_roundtrip_tests::fixture_client_builder;
use std::os::unix::fs::PermissionsExt;

const TEST: &str = "server::routing_topology_tests::isolated_directional_gateway_routing";
const DEVICE: &str = "routing-fixture-owner";
const CREDENTIAL: &str = "routing-fixture-device-credential";

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "Run through scripts/e2e-api.sh with the built engine and CLI"]
async fn isolated_directional_gateway_routing() -> Result<(), BoxError> {
    if std::env::var_os("E2E_ROUTING_ROUTER_CHILD").is_some() {
        return router_child().await;
    }
    if std::env::var_os("E2E_ROUTING_CHILD").is_some() {
        return child().await;
    }
    let home = tempfile::tempdir()?;
    let root = home.path().join(".lucidos");
    let app_data = lucidos_home::default_gateway_data_dir_in(&root);
    let private = lucidos_home::private::path_in(&root);
    lucidos_home::ensure_storage_root(&private)?;
    let stem = format!("routing-e2e-{}", lucidos_local_token::mint_hex(8)?);
    std::fs::write(root.join("routing-fixture"), &stem)?;
    let backend = postgres::PgBackend::from_env()?;
    let mut container = if matches!(backend, postgres::PgBackend::Docker) {
        let password = lucidos_local_token::mint_hex(24)?;
        let destination = private.join(lucidos_local_token::SHARED_PG_PASSWORD_FILE);
        std::fs::write(&destination, &password)?;
        std::fs::set_permissions(destination, std::fs::Permissions::from_mode(0o600))?;
        Some(snapshot_roundtrip_tests::FixturePostgres::start(&stem, &password).await?)
    } else {
        None
    };
    let log = std::fs::File::create(root.join("routing.log"))?;
    let mut command = tokio::process::Command::new(std::env::current_exe()?);
    command.env_clear();
    for name in [
        "PATH",
        "TMPDIR",
        "LUCIDOS_GATEWAY_PG_BACKEND",
        "LUCIDOS_PG_BIN_DIR",
        "LUCIDOS_PG_LIB_DIR",
        "LUCIDOS_AGENT_SANDBOXED",
        "FASTEMBED_CACHE_DIR",
        "LUCIDOS_SYSTEM_KNOWHOW_DIR",
        "LUCIDOS_SYSTEM_WIDGETS_DIR",
        "E2E_SNAPSHOT_ENGINE_BIN",
        "E2E_SNAPSHOT_CLI_BIN",
    ] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    command
        .args(["--ignored", "--exact", TEST, "--nocapture"])
        .env("HOME", home.path())
        .env("E2E_ROUTING_CHILD", &stem)
        .env("LUCIDOS_SECRET_KEY_STORE", "file")
        .env("LUCIDOS_ALLOW_WORKTREE_STACK", "1")
        .env("LUCIDOS_MODEL", "mock")
        .env("LUCIDOS_FORCE_QUERY_CLASSIFICATION", "all")
        .env("FAKE_CLAUDE_CODE_TURN_MS", "0")
        .stdout(log.try_clone()?)
        .stderr(log)
        .kill_on_drop(true);
    if container.is_some() {
        command.env("LUCIDOS_GATEWAY_PG_CONTAINER", &stem);
    }
    let result = tokio::time::timeout(Duration::from_secs(600), command.status()).await;
    for suffix in ["a", "b", "c"] {
        let id = format!("{stem}-{suffix}");
        let workspace = app_data.join("workspaces").join(&id);
        stack::reclaim_stale_engine(&workspace);
        let deadline = Instant::now() + Duration::from_secs(30);
        while stack::live_engine_pid(&workspace).is_some() && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        if stack::live_engine_pid(&workspace).is_some() {
            let kept = home.keep();
            return Err(format!(
                "routing fixture engine did not stop; retained {}",
                kept.display()
            )
            .into());
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
        let log_path = home.keep().join(".lucidos/routing.log");
        let text = std::fs::read_to_string(&log_path).unwrap_or_default();
        let tail: Vec<&str> = text.lines().rev().take(60).collect();
        return Err(format!(
            "routing topology failed; retained log at {}\n--- last log lines ---\n{}",
            log_path.display(),
            tail.into_iter().rev().collect::<Vec<_>>().join("\n")
        )
        .into());
    }
    Ok(())
}

struct Node {
    state: GatewayState,
    workspace: Workspace,
    base: String,
    owner: reqwest::Client,
    repo: String,
    serving: tokio::task::JoinHandle<Result<(), std::io::Error>>,
    handle: axum_server::Handle,
}

impl Node {
    fn api(&self) -> String {
        format!("{}/{}/api/v1", self.base, self.workspace.id)
    }

    async fn get(&self, path: &str) -> Result<Value, BoxError> {
        decode(
            self.owner
                .get(format!("{}{path}", self.api()))
                .send()
                .await?,
        )
        .await
    }

    async fn boot(&self) -> Result<(), BoxError> {
        self.state
            .start_workspace(&self.workspace.id)
            .map_err(|_| "fixture start refused")?;
        let deadline = Instant::now() + Duration::from_secs(120);
        loop {
            if self
                .owner
                .get(format!("{}/health", self.api()))
                .send()
                .await
                .is_ok_and(|r| r.status().is_success())
            {
                return Ok(());
            }
            if Instant::now() > deadline {
                return Err("routing fixture boot timed out".into());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    async fn stop(&self) -> Result<(), BoxError> {
        self.state
            .stop_workspace(&self.workspace.id, None)
            .await
            .map_err(|_| "fixture stop refused")?;
        let directory = self.workspace.resolve_dir(self.state.app_data());
        let deadline = Instant::now() + Duration::from_secs(20);
        while stack::live_engine_pid(&directory).is_some() {
            if Instant::now() > deadline {
                return Err("fixture engine did not finish shutting down".into());
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        Ok(())
    }

    async fn submit(&self, message: String, model: Option<&str>) -> Result<String, BoxError> {
        static NEXT_THREAD: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let thread = format!(
            "00000000-0000-4000-8000-{:012x}",
            NEXT_THREAD.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        );
        let mut body = json!({"message":message, "thread_id":thread,"new_thread":true,
            "mode":"human","use_coding_agent":true,"coding_agent":"claude-code","repo_id":self.repo});
        if let Some(model) = model {
            body["cc_model"] = model.into();
            body["reasoning_effort"] = "low".into();
        }
        decode(
            self.owner
                .post(format!("{}/chat/stream", self.api()))
                .header("content-type", "application/json")
                .body(body.to_string())
                .send()
                .await?,
        )
        .await?;
        Ok(thread)
    }

    async fn answer(&self, thread: &str) -> Result<String, BoxError> {
        let deadline = Instant::now() + Duration::from_secs(65);
        loop {
            let snapshot = self.get(&format!("/threads/{thread}/events")).await?;
            let events = snapshot["events"]
                .as_array()
                .ok_or("missing thread events")?;
            for event in events {
                if event["event_type"] == "ResponseGenerated" {
                    return event["payload"]["text"]
                        .as_str()
                        .map(str::to_owned)
                        .ok_or_else(|| "missing fixture answer".into());
                }
                if event["event_type"] == "ResponseFailed" {
                    return Err(
                        format!("routing fixture agent failed: {}", event["payload"]).into(),
                    );
                }
            }
            if Instant::now() > deadline {
                return Err(format!("routing probe timed out: {snapshot}").into());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    async fn probe(&self, args: &[String]) -> Result<(String, Value), BoxError> {
        let cli = std::env::var("E2E_SNAPSHOT_CLI_BIN")?;
        let message = format!("Probe routing CLI: {}", json!({"cli":cli,"args":args}));
        let thread = self.submit(message, None).await?;
        let proof = serde_json::from_str(&self.answer(&thread).await?)?;
        Ok((thread, proof))
    }

    async fn grant(&self, enabled: bool) -> Result<(), BoxError> {
        decode(
            self.owner
                .put(format!(
                    "{}/~/api/v1/remote-gateways/b/return-access",
                    self.base
                ))
                .header("origin", &self.base)
                .header("referer", format!("{}/~/", self.base))
                .header("sec-fetch-site", "same-origin")
                .header("content-type", "application/json")
                .body(json!({"enabled":enabled}).to_string())
                .send()
                .await?,
        )
        .await?;
        Ok(())
    }
}

async fn decode(response: reqwest::Response) -> Result<Value, BoxError> {
    let status = response.status();
    let text = response.text().await?;
    if !status.is_success() {
        return Err(format!("fixture HTTP {status}: {text}").into());
    }
    Ok(serde_json::from_str(&text)?)
}

/// The fake agent transports the CLI's JSON through JavaScript once. Preserve
/// stable key/value while allowing a floating estimate's final binary digit to
/// change in that extra JSON parse/stringify round trip. Liveness observation
/// timestamps and ages advance between the two reads; their types remain checked.
fn same_reply(actual: &Value, expected: &Value, path: &str) -> Result<(), BoxError> {
    match (actual, expected) {
        (Value::Object(a), Value::Object(b)) if a.len() == b.len() => {
            for (key, value) in b {
                if path.contains("/liveness")
                    && matches!(
                        key.as_str(),
                        "as_of" | "age_secs" | "last_activity_age_secs"
                    )
                {
                    let observed = a.get(key).ok_or("missing liveness observation")?;
                    if key == "as_of" {
                        chrono::DateTime::parse_from_rfc3339(
                            observed.as_str().ok_or("invalid liveness time")?,
                        )?;
                    } else if observed.as_i64().is_none_or(|age| age < 0) {
                        return Err("invalid liveness age".into());
                    }
                    continue;
                }
                same_reply(
                    a.get(key)
                        .ok_or_else(|| format!("missing reply key {path}/{key}"))?,
                    value,
                    &format!("{path}/{key}"),
                )?;
            }
        }
        (Value::Array(a), Value::Array(b)) if a.len() == b.len() => {
            for (index, (a, b)) in a.iter().zip(b).enumerate() {
                same_reply(a, b, &format!("{path}/{index}"))?;
            }
        }
        (Value::Number(a), Value::Number(b)) if a.is_f64() || b.is_f64() => {
            let (a, b) = (
                a.as_f64().ok_or("invalid number")?,
                b.as_f64().ok_or("invalid number")?,
            );
            if (a - b).abs() > f64::EPSILON * 4.0 * a.abs().max(b.abs()).max(1.0) {
                return Err(format!("reply number differs at {path}: {a} vs {b}").into());
            }
        }
        _ if actual == expected => {}
        _ => return Err(format!("routed reply differs at {path}").into()),
    }
    Ok(())
}

async fn node(
    home: &Path,
    app_data: &Path,
    stem: &str,
    suffix: &str,
    token: &str,
) -> Result<Node, BoxError> {
    // The CA must exist before the client below reads its public copy.
    let loopback = loopback_tls()?;
    let fixture = home.join(format!("gateway-{suffix}"));
    std::fs::create_dir_all(&fixture)?;
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let mut state =
        GatewayState::for_tests_with(Some(fixture), token, false).with_live_engine_sockets();
    let inner = Arc::get_mut(&mut state.inner).ok_or("state shared too soon")?;
    inner.app_data = app_data.into();
    inner.registry_path = app_data.join(format!("config/workspaces-{suffix}.json"));
    inner.snapshot_home = home.join(".lucidos");
    inner.gateway_port = port;
    inner.engine_bin = PathBuf::from(std::env::var("E2E_SNAPSHOT_ENGINE_BIN")?);
    inner.static_dir = None;
    inner.peer_connection = crate::peer_connection::PeerConnection::trusting_loopback_root(
        &lucidos_local_token::loopback_cert::read()?,
    )?;
    inner.pg_backend = postgres::PgBackend::from_env()?;
    let reserved = std::net::TcpListener::bind("127.0.0.1:0")?;
    let workspace = Workspace::gateway_provisioned(
        format!("{stem}-{suffix}"),
        format!("Routing {suffix}"),
        reserved.local_addr()?.port(),
    );
    state
        .inner
        .registry
        .lock()
        .unwrap()
        .add(workspace.clone())?;
    state
        .inner
        .registry
        .lock()
        .unwrap()
        .save(&state.inner.registry_path)?;
    drop(reserved);
    state.seed_device_for_test(auth::PairedDevice {
        id: DEVICE.into(),
        label: "Fixture owner".into(),
        credential_digest: auth::digest(CREDENTIAL),
        paired_at: "2020-01-01T00:00:00Z".into(),
        last_seen_at: None,
    })?;
    let owner = fixture_client_builder()?
        .timeout(Duration::from_secs(10))
        .default_headers({
            let mut h = axum::http::HeaderMap::new();
            h.insert(
                axum::http::header::COOKIE,
                format!(
                    "{}={CREDENTIAL}",
                    auth::CookieForm::Hardened.name(state.device_cookie_name())
                )
                .parse()?,
            );
            h
        })
        .build()?;
    let handle = axum_server::Handle::new();
    let serving = tokio::spawn(serve_listener(
        listener,
        gateway_router(state.clone()),
        handle.clone(),
        lucidos_gateway_tls::ListenerTls::Managed(state.certs().clone(), loopback),
    ));
    let mut node = Node {
        state,
        workspace,
        base: lucidos_local_token::client::origin(port),
        owner,
        repo: String::new(),
        serving,
        handle,
    };
    node.boot().await?;
    decode(
        node.owner
            .post(format!("{}/devices/register", node.api()))
            .header("content-type", "application/json")
            .body(json!({"device_id":DEVICE,"user_agent":"routing-e2e/1"}).to_string())
            .send()
            .await?,
    )
    .await?;
    let fake = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../lucidos-app/e2e/fake-claude-code/claude.mjs")
        .canonicalize()?;
    decode(
        node.owner
            .put(format!(
                "{}/preferences?key={}",
                node.api(),
                crate::preference_test_keys::key("CODING_AGENT_CLAUDE_PATH")
            ))
            .header("content-type", "application/json")
            .body(json!({"value":fake}).to_string())
            .send()
            .await?,
    )
    .await?;
    let repo = decode(node.owner.post(format!("{}/repositories", node.api()))
        .header("content-type","application/json").body(json!({"name":"Routing fixture","path":node.workspace.resolve_dir(app_data).canonicalize()?}).to_string()).send().await?).await?;
    node.repo = repo["id"]
        .as_str()
        .ok_or("missing fixture repository")?
        .into();
    Ok(node)
}

fn pair(a: &Node, b: &Node, name: &str) -> Result<crate::remote_gateways::RemoteGateway, BoxError> {
    use crate::gateway_protocol::GatewayRedemption;
    let secret = b.state.paired_gateways().mint_secret(
        Some("Routing gateway".into()),
        false,
        crate::pairing::Minter::PairedDevice,
    )?;
    let GatewayRedemption::Paired {
        pairing_id,
        credential,
    } = b.state.paired_gateways().redeem(
        &secret,
        None,
        crate::pairing::OwnerGate::NoDevicePaired,
    )?
    else {
        return Err("fixture pairing refused".into());
    };
    // A pairing starts at Deny. The topology exercises routed starts, so its
    // owner allows them, as the setting's card would.
    b.state
        .paired_gateways()
        .set_remote_starts(
            &pairing_id,
            lucidos_gateway_stamp::remote_start::RemoteStartSetting::Allow,
        )?
        .ok_or("fixture pairing vanished")?;
    let remote = crate::remote_gateways::RemoteGateway {
        name: name.into(),
        address: b.base.clone(),
        pairing_id,
        credential,
        gateway_id: Some(b.state.gateway_id().into()),
        headers: Default::default(),
        gateway_protocol: crate::gateway_protocol::GATEWAY_PROTOCOL,
        added_at: "2020-01-01T00:00:00Z".into(),
        rotation: Default::default(),
        return_access: None,
        trusted_pairing: false,
    };
    a.state
        .remote_gateways()
        .add(remote.clone())
        .map_err(|error| format!("fixture pairing: {error:?}"))?;
    Ok(remote)
}

fn args(words: &[&str], via: Option<&str>, to: Option<&str>) -> Vec<String> {
    let mut args: Vec<String> = words.iter().map(|word| (*word).into()).collect();
    if let Some(via) = via {
        args.extend(["--via".into(), via.into()]);
    }
    if let Some(to) = to {
        args.extend(["--to".into(), to.into()]);
    }
    args
}

async fn child() -> Result<(), BoxError> {
    let home = PathBuf::from(std::env::var_os("HOME").ok_or("missing fixture HOME")?);
    let stem = std::env::var("E2E_ROUTING_CHILD")?;
    if !stem.starts_with("routing-e2e-")
        || std::fs::read_to_string(home.join(".lucidos/routing-fixture"))? != stem
    {
        return Err("not an owned routing fixture".into());
    }
    let app_data = lucidos_home::gateway_data_dir().ok_or("missing fixture app data")?;
    std::fs::create_dir_all(app_data.join("config"))?;
    let token = lucidos_local_token::ensure()?;
    let a = node(&home, &app_data, &stem, "a", &token).await?;
    let b = node(&home, &app_data, &stem, "b", &token).await?;
    let c = node(&home, &app_data, &stem, "c", &token).await?;
    let result = exercise(&a, &b, &c).await;
    for node in [&a, &b, &c] {
        node.stop().await?;
        node.serving.abort();
    }
    result
}

async fn exercise(a: &Node, b: &Node, c: &Node) -> Result<(), BoxError> {
    let ab = pair(a, b, "b")?;
    let ac = pair(a, c, "c")?;
    let target = format!("{}@c", c.workspace.id);
    let thread = c
        .submit(
            "Say exactly: \"routing target conversation\" and nothing else.".into(),
            Some("claude-haiku-5-5"),
        )
        .await?;
    assert_eq!(c.answer(&thread).await?, "routing target conversation");
    let preferences = c.get("/preferences").await?;
    let route = a.state.gateway_id();
    let read = args(
        &["threads", "messages", "--thread", &thread],
        Some(route),
        Some(&target),
    );
    let spawn = args(
        &[
            "spawn-thread",
            "--message",
            "Say exactly: \"routed spawn answer\" and nothing else.",
            "--coding-agent",
            "claude-code",
            "--repo",
            &c.repo,
        ],
        Some(route),
        Some(&target),
    );
    let initial_count = c.get("/threads/count").await?;
    let (_, denied) = b.probe(&read).await?;
    assert_ne!(denied["code"], 0, "pairing must not imply return access");
    let (_, denied) = b.probe(&spawn).await?;
    assert_ne!(denied["code"], 0);
    assert_eq!(
        initial_count,
        c.get("/threads/count").await?,
        "denied return spawn has no target side effect"
    );
    let (a_thread, outgoing) = a
        .probe(&args(
            &["threads", "messages", "--thread", &thread],
            None,
            Some(&target),
        ))
        .await?;
    assert_eq!(outgoing["code"], 0, "A-local outgoing read: {outgoing}");
    assert_eq!(
        outgoing["data"],
        c.get(&format!("/threads/{thread}/messages")).await?
    );
    let (_, outgoing_spawn) = a.probe(&spawn).await?;
    assert_eq!(
        outgoing_spawn["code"], 0,
        "A-local outgoing spawn needs no return grant: {outgoing_spawn}"
    );
    let a_spawn = outgoing_spawn["data"]["thread_id"]
        .as_str()
        .ok_or("missing outgoing spawn identity")?;
    assert_eq!(c.answer(a_spawn).await?, "routed spawn answer");
    a.grant(true).await?;
    let (_, directory) = b
        .probe(&args(&["workspaces", "targets"], Some(route), None))
        .await?;
    assert_eq!(directory["code"], 0, "routed discovery: {directory}");
    assert_eq!(directory["data"]["workspaces"][0]["id"], a.workspace.id);
    assert!(!directory.to_string().contains(&ac.credential));
    let (_, local_a) = b
        .probe(&args(
            &["threads", "messages", "--thread", &a_thread],
            Some(route),
            Some(&a.workspace.id),
        ))
        .await?;
    assert_eq!(local_a["code"], 0);
    assert_eq!(
        local_a["data"],
        a.get(&format!("/threads/{a_thread}/messages")).await?
    );
    for (words, path) in [
        (
            vec!["threads", "get", "--thread", &thread],
            format!("/threads/{thread}"),
        ),
        (
            vec!["threads", "messages", "--thread", &thread],
            format!("/threads/{thread}/messages"),
        ),
        (
            vec!["events", "query", "--thread-id", &thread, "--limit", "20"],
            format!("/events/query?thread_id={thread}&limit=20"),
        ),
        (
            vec!["events", "count", "--thread-id", &thread],
            format!("/events/count?thread_id={thread}"),
        ),
        (
            vec!["threads", "list", "--limit", "20"],
            "/threads/list?limit=20".into(),
        ),
        (vec!["threads", "count"], "/threads/count".into()),
        (
            vec!["intelligence-ladder", "--thread", &thread, "--json"],
            format!("/intelligence-ladder?thread_id={thread}"),
        ),
    ] {
        let (_, proof) = b.probe(&args(&words, Some(route), Some(&target))).await?;
        assert_eq!(proof["code"], 0, "routed {words:?}: {proof}");
        same_reply(&proof["data"], &c.get(&path).await?, &path)?;
        if words[0] == "intelligence-ladder" {
            assert_eq!(proof["data"]["current"]["model"], "claude-haiku-5-5");
            assert_eq!(proof["data"]["current"]["effort"], "low");
        }
    }
    let (_, live) = b
        .probe(&args(
            &["threads", "liveness", "--thread", &thread],
            Some(route),
            Some(&target),
        ))
        .await?;
    assert_eq!(live["code"], 0);
    assert_eq!(
        live["data"]["state"],
        c.get(&format!("/threads/{thread}/liveness")).await?["state"]
    );
    assert_eq!(
        preferences,
        c.get("/preferences").await?,
        "reads must not mutate target preferences"
    );
    let (source, proof) = b.probe(&spawn).await?;
    assert_eq!(proof["code"], 0, "routed spawn: {proof}");
    let spawned = proof["data"]["thread_id"]
        .as_str()
        .ok_or("missing routed thread identity")?;
    assert_eq!(c.answer(spawned).await?, "routed spawn answer");
    let events = c.get(&format!("/threads/{spawned}/events")).await?;
    let origin = events["events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["event_type"] == "MessageReceived")
        .ok_or("missing routed message")?["payload"]["origin"]
        .clone();
    assert_eq!(origin["caller_gateway"], b.state.gateway_id());
    assert_eq!(origin["routing_gateway"], a.state.gateway_id());
    assert_eq!(origin["workspace"], b.workspace.id);
    assert_eq!(origin["thread_id"], source);
    let count = c.get("/threads/count").await?;
    let key = c
        .state
        .inner
        .stamp_keys
        .write()
        .unwrap()
        .remove(&c.workspace.id)
        .unwrap();
    let (_, denied) = b.probe(&spawn).await?;
    assert_ne!(
        denied["code"], 0,
        "an adopted keyless target must fail closed"
    );
    assert_eq!(count, c.get("/threads/count").await?);
    c.state
        .inner
        .stamp_keys
        .write()
        .unwrap()
        .insert(c.workspace.id.clone(), key);
    let old_route = b
        .state
        .paired_gateways()
        .list()
        .into_iter()
        .find(|p| p.id == ab.pairing_id)
        .unwrap()
        .return_route;
    a.grant(false).await?;
    b.state
        .paired_gateways()
        .set_return_route(&ab.pairing_id, old_route)?;
    let (_, denied) = b.probe(&spawn).await?;
    assert_ne!(denied["code"], 0);
    assert_eq!(
        count,
        c.get("/threads/count").await?,
        "revoked spawn has no target side effect"
    );
    a.grant(true).await?;
    for node in [a, b, c] {
        let previous = node
            .state
            .stamp_key(&node.workspace.id)
            .ok_or("missing running engine key")?;
        node.stop().await?;
        if node.workspace.id == c.workspace.id {
            let (_, denied) = b.probe(&read).await?;
            assert_ne!(
                denied["code"], 0,
                "a routed read must not start a stopped target implicitly"
            );
            assert!(c.state.engine_address(&c.workspace.id).is_none());
        }
        node.boot().await?;
        let current = node
            .state
            .stamp_key(&node.workspace.id)
            .ok_or("missing new engine key")?;
        assert!(
            !Arc::ptr_eq(&previous, &current),
            "a restarted engine must receive a fresh stamp key"
        );
        let (_, proof) = b.probe(&read).await?;
        assert_eq!(proof["code"], 0, "read after engine restart: {proof}");
    }
    a.state.remote_gateways().remove("c")?;
    let (_, denied) = b.probe(&spawn).await?;
    assert_ne!(denied["code"], 0);
    assert_eq!(count, c.get("/threads/count").await?);
    a.state
        .remote_gateways()
        .add(ac.clone())
        .map_err(|e| format!("restore fixture pairing: {e:?}"))?;
    c.state.paired_gateways().unpair(&ac.pairing_id)?;
    let (_, denied) = b.probe(&spawn).await?;
    assert_ne!(denied["code"], 0);
    assert_eq!(
        count,
        c.get("/threads/count").await?,
        "unpaired spawn has no target side effect"
    );
    a.state.remote_gateways().remove("b")?;
    let (_, denied) = b
        .probe(&args(
            &["threads", "messages", "--thread", &a_thread],
            Some(route),
            Some(&a.workspace.id),
        ))
        .await?;
    assert_ne!(denied["code"], 0);
    a.state
        .remote_gateways()
        .add(ab.clone())
        .map_err(|e| format!("restore fixture pairing: {e:?}"))?;
    b.state.paired_gateways().unpair(&ab.pairing_id)?;
    let (_, denied) = b.probe(&read).await?;
    assert_ne!(denied["code"], 0);
    restart_routing_process(a, b, c, &thread, &source).await?;
    crate::log!("Verified real A/B/C engines, target records and ladder, spawn provenance, revoke/unpair, engine restarts and routing-gateway process restart");
    Ok(())
}

async fn router_child() -> Result<(), BoxError> {
    use tokio::io::AsyncReadExt;
    let home = PathBuf::from(std::env::var_os("HOME").ok_or("missing routing HOME")?);
    let stem = std::env::var("E2E_ROUTING_CHILD")?;
    if !stem.starts_with("routing-e2e-")
        || std::fs::read_to_string(home.join(".lucidos/routing-fixture"))? != stem
    {
        return Err("not an owned routing fixture".into());
    }
    let token = lucidos_local_token::ensure()?;
    let mut state = GatewayState::for_tests_with(Some(home.join("gateway-a")), &token, false);
    let port: u16 = std::env::var("E2E_ROUTING_PORT")?.parse()?;
    let inner = Arc::get_mut(&mut state.inner).ok_or("router shared too soon")?;
    inner.gateway_port = port;
    inner.peer_connection = crate::peer_connection::PeerConnection::trusting_loopback_root(
        &lucidos_local_token::loopback_cert::read()?,
    )?;
    let listener = std::net::TcpListener::bind(("127.0.0.1", port))?;
    let handle = axum_server::Handle::new();
    let serving = tokio::spawn(serve_listener(
        listener,
        gateway_router(state.clone()),
        handle.clone(),
        lucidos_gateway_tls::ListenerTls::Managed(state.certs().clone(), loopback_tls()?),
    ));
    std::fs::write(
        home.join(".lucidos/router-ready.json"),
        json!({
            "instance_id":instance_id(),"gateway_id":state.gateway_id()
        })
        .to_string(),
    )?;
    let mut input = Vec::new();
    tokio::io::stdin().read_to_end(&mut input).await?;
    handle.shutdown();
    serving.await??;
    Ok(())
}

async fn restart_routing_process(
    a: &Node,
    b: &Node,
    c: &Node,
    thread: &str,
    source: &str,
) -> Result<(), BoxError> {
    // Re-establish only this fixture's pairings after the revocation matrix.
    a.state.remote_gateways().remove("c")?;
    pair(a, c, "c")?;
    a.state.remote_gateways().remove("b")?;
    let ab = pair(a, b, "b")?;
    a.grant(true).await?;
    let grant = a
        .state
        .remote_gateways()
        .list()
        .into_iter()
        .find(|g| g.name == "b")
        .unwrap()
        .return_access
        .unwrap();
    let target = format!("{}@c", c.workspace.id);
    let issue = || -> Result<Value, BoxError> {
        let operation=json!({
            "routing_instance":instance_id(),
            "envelope":{
                "request":{"via":a.state.gateway_id(),"to":target,
                    "operation":{"kind":"read","request":{"kind":"thread-messages","thread_id":thread}}},
                "caller_gateway":b.state.gateway_id(),"caller_workspace":b.workspace.id,
                "caller_thread_id":source,"verdict":"","nonce":auth::mint_credential()?
            }
        }).to_string();
        let receipt =
            b.state
                .paired_gateways()
                .with_return_receipts(&ab.pairing_id, &grant.id, |r| {
                    r.issue(
                        &ab.pairing_id,
                        &grant.id,
                        operation,
                        lucidos_gateway_stamp::now_unix(),
                    )
                })??;
        Ok(json!({"pairing_id":ab.pairing_id,"grant_id":grant.id,"receipt":receipt}))
    };
    let client = fixture_client_builder()?
        .timeout(Duration::from_secs(10))
        .build()?;
    let callback = format!(
        "{}{}/return-operation",
        a.base,
        crate::gateway_protocol::GATEWAY_API_PREFIX
    );
    let baseline = decode(
        client
            .post(&callback)
            .header("content-type", "application/json")
            .body(issue()?.to_string())
            .send()
            .await?,
    )
    .await?;
    same_reply(
        &baseline,
        &c.get(&format!("/threads/{thread}/messages")).await?,
        "before routing restart",
    )?;
    // Hold an otherwise-valid authoritative receipt across A's process change.
    a.stop().await?;
    let held = issue()?;
    a.handle.shutdown();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !a.serving.is_finished() {
        if Instant::now() > deadline {
            return Err("fixture gateway A did not stop".into());
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let home = PathBuf::from(std::env::var_os("HOME").ok_or("missing routing HOME")?);
    let ready = home.join(".lucidos/router-ready.json");
    let log = std::fs::File::create(home.join(".lucidos/router-restart.log"))?;
    let mut child = tokio::process::Command::new(std::env::current_exe()?)
        .args(["--ignored", "--exact", TEST, "--nocapture"])
        .env("E2E_ROUTING_ROUTER_CHILD", "1")
        .env("E2E_ROUTING_PORT", a.state.gateway_port().to_string())
        .stdin(std::process::Stdio::piped())
        .stdout(log.try_clone()?)
        .stderr(log)
        .kill_on_drop(true)
        .spawn()?;
    let result = async {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !ready.exists() {
            if Instant::now() > deadline {
                return Err::<(), BoxError>("fixture routing restart timed out".into());
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let restarted: Value = serde_json::from_slice(&std::fs::read(&ready)?)?;
        if restarted["instance_id"] == instance_id()
            || restarted["gateway_id"] != a.state.gateway_id()
        {
            return Err(
                "routing restart did not preserve identity and replace process state".into(),
            );
        }
        let response = client
            .post(&callback)
            .header("content-type", "application/json")
            .body(held.to_string())
            .send()
            .await?;
        let status = response.status();
        let body = response.text().await?;
        if status != reqwest::StatusCode::FORBIDDEN || !body.contains("restarted") {
            return Err(
                format!("a receipt crossed the routing process restart: {status} {body}").into(),
            );
        }
        let (_, proof) = b
            .probe(&args(
                &["threads", "messages", "--thread", thread],
                Some(a.state.gateway_id()),
                Some(&target),
            ))
            .await?;
        if proof["code"] != 0 {
            return Err(format!("fresh operation after routing restart failed: {proof}").into());
        }
        same_reply(
            &proof["data"],
            &c.get(&format!("/threads/{thread}/messages")).await?,
            "after routing restart",
        )?;
        Ok(())
    }
    .await;
    drop(child.stdin.take());
    let status = tokio::time::timeout(Duration::from_secs(10), child.wait()).await??;
    if !status.success() {
        return Err("fixture routing child failed; inspect router-restart.log".into());
    }
    result
}
