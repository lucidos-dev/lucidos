use super::*;
use crate::core::event_subscription::frames::{
    LAGGED_FRAME, SSE_KEEPALIVE_EVENT, SSE_KEEPALIVE_INTERVAL,
};
use axum::body::Body;
use tokio::io::AsyncWriteExt;

const GZIP_PIPE_BUF_BYTES: usize = 64 * 1024;

#[derive(Deserialize)]
pub(super) struct GlobalEventsQuery {
    aggregates: Option<String>,
}

pub(super) async fn global_events(
    State(state): State<AppState>,
    Query(query): Query<GlobalEventsQuery>,
    headers: HeaderMap,
) -> Response {
    event_stream(
        &state.engine.event_bus,
        &state.engine.sse_connections,
        state.engine.pool(),
        query.aggregates.as_deref(),
        &headers,
    )
    .await
}

async fn event_stream(
    event_bus: &crate::engine::event_bus::EventBus,
    sse_connections: &crate::api::sse_connections::SseConnectionCounter,
    pool: &sqlx::PgPool,
    aggregates: Option<&str>,
    headers: &HeaderMap,
) -> Response {
    let wanted = aggregates.map(lucidos_gateway_stamp::row_stream::parse_aggregates);
    let conn_guard = wanted.is_none().then(|| sse_connections.connect());
    let rx = event_bus.subscribe();
    // INVARIANT: Subscribe first, then read the durable generation. Every event buffered
    // during this read follows the hello, including on the gzip transport.
    // An unreadable boundary must never look like a workspace without restores.
    let boundary = match crate::core::snapshot::boundary::read(pool).await {
        Ok(boundary) => boundary,
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    let hello = stream_hello(boundary.as_ref().map(|boundary| boundary.restore_id));
    let json_stream = tokio_stream::StreamExt::filter_map(BroadcastStream::new(rx), move |r| {
        let _hold = &conn_guard;
        match r {
            Ok(emitted) => match &wanted {
                None => Some(emitted.to_sse_json()),
                Some(wanted) => row_stream_frame(&emitted, wanted),
            },
            Err(tokio_stream::wrappers::errors::BroadcastStreamRecvError::Lagged(n)) => {
                log_legacy!("[SSE] Event stream lagged by {} events", n);
                Some(lagged_event_json(n))
            }
        }
    });
    let json_stream = tokio_stream::once(hello).chain(json_stream);

    if accepts_gzip(headers) {
        gzipped_sse_response(json_stream)
    } else {
        plain_sse_response(json_stream)
    }
}

fn row_stream_frame(
    emitted: &crate::engine::event_bus::EmittedEvent,
    wanted: &[String],
) -> Option<String> {
    let crate::engine::event_bus::BusEvent::System(event) = &emitted.typed else {
        return None;
    };
    let frame = lucidos_gateway_stamp::row_stream::Frame::for_event(
        event.event_type(),
        event.aggregate(),
        wanted,
    )?;
    serde_json::to_string(&frame).ok()
}

fn stream_hello(generation: Option<uuid::Uuid>) -> String {
    serde_json::json!({
        "type": "StreamHello",
        "data": { "restore_generation": generation }
    })
    .to_string()
}

enum Frame {
    Data(String),
    Keepalive,
}

impl Frame {
    fn chunks(&self) -> [&[u8]; 3] {
        match self {
            Frame::Data(json) => [b"data: ", json.as_bytes(), b"\n\n"],
            Frame::Keepalive => [b"event: ", SSE_KEEPALIVE_EVENT.as_bytes(), b"\ndata:\n\n"],
        }
    }
}

fn with_keepalives<S>(json_stream: S) -> impl Stream<Item = Frame> + Send + 'static
where
    S: Stream<Item = String> + Send + 'static,
{
    let mut ticks = tokio::time::interval(SSE_KEEPALIVE_INTERVAL);
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    futures::stream::unfold(
        (Box::pin(json_stream), ticks),
        |(mut json, mut ticks)| async {
            let frame = tokio::select! {
                item = json.next() => Frame::Data(item?),
                _ = ticks.tick() => Frame::Keepalive,
            };
            Some((frame, (json, ticks)))
        },
    )
}

fn plain_sse_response<S>(json_stream: S) -> Response
where
    S: Stream<Item = String> + Send + 'static,
{
    let body = with_keepalives(json_stream)
        .map(|frame| Ok::<_, Infallible>(axum::body::Bytes::from(frame.chunks().concat())));
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .body(Body::from_stream(body))
        .expect("static SSE response builder is infallible")
}

fn gzipped_sse_response<S>(json_stream: S) -> Response
where
    S: Stream<Item = String> + Send + 'static,
{
    use async_compression::tokio::write::GzipEncoder;
    use async_compression::Level;
    use tokio_util::io::ReaderStream;

    let (writer, reader) = tokio::io::duplex(GZIP_PIPE_BUF_BYTES);
    let body_stream = ReaderStream::new(reader);

    tokio::spawn(async move {
        let mut encoder = GzipEncoder::with_quality(writer, Level::Fastest);
        let frames = with_keepalives(json_stream);
        tokio::pin!(frames);
        while let Some(frame) = frames.next().await {
            let write_result: std::io::Result<()> = async {
                for chunk in frame.chunks() {
                    encoder.write_all(chunk).await?;
                }
                encoder.flush().await
            }
            .await;
            if write_result.is_err() {
                break;
            }
        }
        let _ = encoder.shutdown().await;
    });

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CONTENT_ENCODING, "gzip")
        .header(header::CACHE_CONTROL, "no-cache")
        .header("X-Accel-Buffering", "no")
        .body(Body::from_stream(body_stream))
        .expect("static SSE response builder is infallible")
}

fn accepts_gzip(headers: &HeaderMap) -> bool {
    let Some(val) = headers.get(header::ACCEPT_ENCODING) else {
        return false;
    };
    let Ok(s) = val.to_str() else {
        return false;
    };
    s.split(',').any(|piece| {
        let mut params = piece.split(';');
        let token = params.next().unwrap_or("").trim();
        if !token.eq_ignore_ascii_case("gzip") {
            return false;
        }
        params.all(|param| {
            let Some((name, value)) = param.split_once('=') else {
                return true;
            };
            if !name.trim().eq_ignore_ascii_case("q") {
                return true;
            }
            value
                .trim()
                .parse::<f32>()
                .map(|q| q > 0.0)
                .unwrap_or(false)
        })
    })
}

fn lagged_event_json(count: u64) -> String {
    serde_json::json!({
        "type": LAGGED_FRAME,
        "data": { "count": count },
    })
    .to_string()
}

fn health_status(shutting_down: bool) -> (StatusCode, &'static str) {
    if shutting_down {
        (StatusCode::SERVICE_UNAVAILABLE, "shutting_down")
    } else {
        (StatusCode::OK, "ok")
    }
}

pub(super) async fn health(State(state): State<AppState>) -> (StatusCode, Json<serde_json::Value>) {
    let (code, status) = health_status(state.engine.is_shutting_down());
    let workspace_name = state
        .workspace_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "unknown".to_string());
    let engine_version = crate::build_identity().engine_version;
    let latest_engine_version = read_engine_version();
    let latest_tauri_app_version = read_app_version();
    let database = state.engine.database_health();
    let agent_sandbox_unavailable = crate::runtime::agent_sandbox::unavailable_reason().await;
    let body = Json(serde_json::json!({
        "status": status,
        "workspace_client": lucidos_gateway_stamp::workspace_client::ClientContract::engine(),
        "workspace": workspace_name,
        "workspace_path": state.workspace_path.to_string_lossy(),
        "started_at": state.started_at.to_rfc3339(),
        "started_by_gateway": crate::api::gateway_stamp::started_by_gateway(),
        lucidos_gateway_stamp::row_stream::HEALTH_FIELD: true,
        // INVARIANT: Is the workspace database answering? An engine outlives its database,
        // and a client that cannot tell holds a black boot splash and reports
        // the outage once per failed load. Read from an atomic the background
        // probe writes, so this never puts database latency on the endpoint.
        "database_reachable": database.is_reachable(),
        "database_pool_exhausted": database.is_pool_exhausted(),
        "release": crate::build_identity().release,
        "release_dirty": crate::build_identity().release_dirty,
        // INVARIANT: The gateway's Updates page reads this as the build the
        // engine runs (ADR 0824). It must match `lucidos-engine --build-id`.
        "build_id": crate::build_identity().id,
        "engine_version": engine_version,
        "latest_engine_version": latest_engine_version,
        "latest_tauri_app_version": latest_tauri_app_version,
        "packaged": is_packaged(),
        "llm_configured": state.engine.llm_configured(),
        "configured_providers": state.engine.configured_providers(),
        "agent_sandbox_unavailable": agent_sandbox_unavailable,
        "agent_sandbox_truncation_unguarded":
            crate::runtime::agent_sandbox::truncation_unguarded().await,
        "agent_sandbox_metadata_unguarded":
            crate::runtime::agent_sandbox::metadata_unguarded().await,
    }));
    (code, body)
}

fn is_packaged() -> bool {
    !crate::paths::has_lucidos_source()
}

/// INVARIANT: launchd label of the always-on engine service (matches
/// `crates/lucidos-app/src/desktop.rs` `SERVICE_AGENT_LABEL`). The client's own
/// login agent is a separate job and is deliberately NOT restarted from here:
/// restarting the service must not disturb the window the user is looking at.
const LAUNCH_AGENT_LABEL: &str = "com.lucidos.engine";

fn read_version_file(path: &std::path::Path) -> String {
    std::fs::read_to_string(path)
        .map(|v| v.trim().to_string())
        .unwrap_or_else(|_| "unknown".to_string())
}

fn read_engine_version() -> String {
    let path = crate::paths::repo_root()
        .ok()
        .map(|r| r.join("crates/lucidos-engine/VERSION"))
        .unwrap_or_default();
    read_version_file(&path)
}

fn read_app_version() -> String {
    let path = crate::paths::repo_root()
        .ok()
        .map(|r| r.join("crates/lucidos-app/VERSION"))
        .unwrap_or_default();
    read_version_file(&path)
}

/// INVARIANT: Switch the engine onto the new version, the disruptive step of the
/// new-version-available flow. The new binary is ALREADY on disk, rebuilt by
/// dev's *Apply* or installed by the packaged updater, so this only RESPAWNS.
///
/// It does NOT pre-emit boundary events. It stashes the device actor and
/// returns; the boundary aborts are emitted at real teardown by
/// `main.rs::shutdown_signal`. Nothing then shows "Switched" while the old
/// engine is still alive. A device actor also marks the shutdown as a
/// user-initiated switch, which recovery uses to auto-resume in-flight
/// coding-agent threads. A crash leaves no such actor, so those threads keep a
/// manual Continue.
pub(super) async fn restart_engine(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
) -> Result<StatusCode, (StatusCode, Json<serde_json::Value>)> {
    let actor = super::actor::user_actor(&headers, None);
    // INVARIANT: Stash the device actor for the teardown-time boundary emit (Phase 3) and the
    // recovery auto-resume signal (Phase 4). No pre-emit here. First writer wins,
    // and this IS the first writer: the gateway's restart-intent notify fires on
    // the respawn we are about to ask for, and must not overwrite the click's own
    // actor (see `stash_first_restart_actor`).
    let stashed = state.engine.stash_restart_actor(actor);

    let outcome = respawn_this_engine(&state).await;

    // INVARIANT: Nothing asked this engine to go down after all. Every arm in
    // `respawn_this_engine` fails BEFORE the process is signalled, so the stash
    // now describes a restart that never happened. Left in place it would attach
    // to whatever teardown came next, and under first-writer-wins it would
    // REFUSE that teardown's own actor. That is the case that bites: a stale
    // non-device actor here would block a later picker Restart from attributing
    // itself, costing its threads the pause and the auto-resume. Only clear what
    // THIS call stashed, so a concurrent notify's actor is never dropped for us.
    if outcome.is_err() && stashed {
        state.engine.take_restart_actor();
        log_legacy!("[Restart] Respawn request failed, cleared the stashed restart actor");
    }
    outcome
}

async fn respawn_this_engine(
    state: &AppState,
) -> Result<StatusCode, (StatusCode, Json<serde_json::Value>)> {
    if let (Ok(port), Ok(id)) = (
        std::env::var("LUCIDOS_GATEWAY_PORT"),
        std::env::var("LUCIDOS_WORKSPACE_ID"),
    ) {
        return restart_via_gateway(&port, &id).await;
    }

    if is_packaged() {
        return restart_via_launchd();
    }

    let script = crate::paths::script("web-dev.sh").map_err(|e| {
        log_legacy!("[Restart] {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
    })?;
    let ws = state.workspace_path.to_string_lossy().to_string();
    let log_path = state.workspace_path.join(".lucidos/engine.log");
    log_legacy!(
        "[Restart] Running {} --engine-only -w {}",
        script.display(),
        ws
    );
    let mut cmd = tokio::process::Command::new(&script);
    cmd.args(["-w", &ws, "--engine-only"]);
    let exe = crate::engine::engine_version::published_engine_exe().ok_or_else(|| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": "Cannot resolve the published engine path." })),
        )
    })?;
    let dir = crate::engine::engine_version::published_launch_dir(&exe).ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Launch the engine from the checkout's published .launch/<profile>/<variant>/ path before switching versions." })),
        )
    })?;
    cmd.arg("--publish-to").arg(dir);
    match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
    {
        Ok(log_file) => {
            let stderr_file = log_file.try_clone().or_else(|_| {
                std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&log_path)
            });
            cmd.stdout(log_file);
            match stderr_file {
                Ok(f) => {
                    cmd.stderr(f);
                }
                Err(_) => {
                    cmd.stderr(std::process::Stdio::null());
                }
            }
        }
        Err(e) => {
            log_legacy!(
                "[Restart] Failed to open log file for restart: {} — spawning with no output",
                e
            );
            cmd.stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null());
        }
    }
    match cmd.spawn() {
        Ok(_) => Ok(StatusCode::OK),
        Err(e) => {
            let msg = format!("Failed to spawn {}: {}", script.display(), e);
            log_legacy!("[Restart] {}", msg);
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": msg })),
            ))
        }
    }
}

pub(super) async fn engine_version_status(
    State(state): State<AppState>,
) -> Json<crate::engine::engine_version::VersionStatus> {
    Json(read_engine_version_status(&state).await)
}

pub(super) async fn read_engine_version_status(
    state: &AppState,
) -> crate::engine::engine_version::VersionStatus {
    state.engine.version_status().await
}

async fn engine_build_log(
    State(state): State<AppState>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let output = read_build_log(
        &state
            .workspace_path
            .join(crate::engine::engine_version::ENGINE_BUILD_LOG_PATH),
    )
    .await?;
    Ok(([(header::CACHE_CONTROL, "no-store")], output))
}

const BUILD_LOG_BYTES: u64 = 64 * 1024;

async fn read_build_log(path: &std::path::Path) -> Result<String, (StatusCode, String)> {
    use tokio::io::{AsyncReadExt, AsyncSeekExt};
    let unavailable = |_| {
        (
            StatusCode::NOT_FOUND,
            "Build output is not available yet.".to_string(),
        )
    };
    let mut file = tokio::fs::File::open(path).await.map_err(unavailable)?;
    let len = file.metadata().await.map_err(unavailable)?.len();
    file.seek(std::io::SeekFrom::Start(
        len.saturating_sub(BUILD_LOG_BYTES),
    ))
    .await
    .map_err(unavailable)?;
    let mut bytes = Vec::new();
    file.take(BUILD_LOG_BYTES)
        .read_to_end(&mut bytes)
        .await
        .map_err(unavailable)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

async fn engine_diagnostic_log(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::RawQuery(raw): axum::extract::RawQuery,
) -> Result<impl IntoResponse, ApiError> {
    static SCANS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);
    let source = DiagnosticLogSource {
        dir: crate::diagnostic_log::dir_for(&state.workspace_path),
        started: crate::diagnostic_log::started(),
        gateway_origin: std::env::var("LUCIDOS_GATEWAY_PORT")
            .ok()
            .and_then(|port| port.parse::<u16>().ok())
            .map(lucidos_local_token::client::origin),
        scans: &SCANS,
    };
    let result = query_diagnostic_log(&headers, raw.as_deref(), source).await?;
    Ok(([(header::CACHE_CONTROL, "no-store")], Json(result)))
}

struct DiagnosticLogSource {
    dir: std::path::PathBuf,
    started: bool,
    gateway_origin: Option<String>,
    scans: &'static tokio::sync::Semaphore,
}

pub(super) fn diagnostic_log_reader_allowed(headers: &HeaderMap) -> bool {
    !headers.contains_key(super::app_reach::APP_ID_HEADER)
        && matches!(
            crate::core::subprocess_origin::subprocess_origin(headers),
            crate::core::subprocess_origin::SubprocessOrigin::Subprocess { .. }
        )
}

async fn query_diagnostic_log(
    headers: &HeaderMap,
    raw: Option<&str>,
    source: DiagnosticLogSource,
) -> Result<crate::diagnostic_log::query::QueryResult, ApiError> {
    use crate::diagnostic_log::query::{self, QueryResult, Source, SourceStatus};
    if !diagnostic_log_reader_allowed(headers) {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "The diagnostic log answers only this workspace's agents.",
        )
        .with_reason("diagnostic_log_agent_only"));
    }
    let query = query::parse(raw, chrono::Utc::now())
        .map_err(|e| ApiError::bad_request(e).with_reason("diagnostic_log_bad_query"))?;
    let unavailable = |detail: String| {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            format!("The diagnostic log is unavailable: {detail}"),
        )
        .with_reason("diagnostic_log_unavailable")
    };
    if query.engine_only() && !source.started {
        return Err(unavailable("this engine never started it".into()));
    }
    let engine = if !query.includes(Source::Engine) {
        None
    } else if !source.started {
        Some(QueryResult::unavailable(
            Source::Engine,
            SourceStatus::NotPresent,
        ))
    } else if let Ok(permit) = source.scans.try_acquire() {
        let filter = query.clone();
        let result = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            query::run(&source.dir, &filter, Source::Engine)
        })
        .await
        .unwrap_or_else(|error| Err(std::io::Error::other(error)));
        Some(match result {
            Ok(answer) => answer,
            Err(error) if query.engine_only() => return Err(unavailable(error.to_string())),
            Err(error) => QueryResult::unavailable(
                Source::Engine,
                if error.kind() == std::io::ErrorKind::NotFound {
                    SourceStatus::NotPresent
                } else {
                    SourceStatus::Unreadable
                },
            ),
        })
    } else if query.engine_only() {
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "Two diagnostic log queries are already running. Try again shortly.",
        )
        .with_reason("diagnostic_log_busy"));
    } else {
        Some(QueryResult::unavailable(Source::Engine, SourceStatus::Busy))
    };
    let gateway = if query.engine_only() {
        None
    } else {
        let result = match source.gateway_origin {
            Some(origin) => fetch_gateway_diagnostic_log(&origin, &query).await,
            None => Err(SourceStatus::GatewayUnreachable),
        };
        Some(result.unwrap_or_else(|status| query::machine_unavailable(&query, status)))
    };
    let result = query::merge(&query, engine.into_iter().chain(gateway)).into_public();
    log_legacy!(
        "[DiagnosticLog] Served {} lines ({})",
        result.returned,
        query.summary()
    );
    Ok(result)
}

async fn fetch_gateway_diagnostic_log(
    origin: &str,
    query: &crate::diagnostic_log::query::Query,
) -> Result<crate::diagnostic_log::query::QueryResult, crate::diagnostic_log::query::SourceStatus> {
    use crate::diagnostic_log::query::{QueryResult, SourceStatus};

    let unreachable = SourceStatus::GatewayUnreachable;
    let client = crate::gateway_auth::client_builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .map_err(|_| unreachable)?;
    let response = client
        .get(format!("{origin}/~/api/v1/control/diagnostic-log"))
        .query(&query.forward_params())
        .send()
        .await
        .map_err(|_| unreachable)?;
    let picker = response.status().is_success()
        && response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(';').next())
            .is_some_and(|media_type| media_type.trim().eq_ignore_ascii_case("text/html"));
    if response.status() == StatusCode::NOT_FOUND || picker {
        return Err(SourceStatus::GatewayTooOld);
    }
    let result: QueryResult = response
        .error_for_status()
        .map_err(|_| unreachable)?
        .json()
        .await
        .map_err(|_| unreachable)?;
    if !result.sources.keys().copied().eq(query.machine_sources())
        || result.lines.iter().any(|line| {
            !line
                .source
                .is_some_and(|source| result.sources.contains_key(&source))
                || line.ts.parse::<chrono::DateTime<chrono::Utc>>().is_err()
        })
    {
        return Err(unreachable);
    }
    Ok(result)
}

pub(super) async fn engine_changelog() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "releases": crate::engine::changelog::changelog_releases().await,
    }))
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum RebuildMode {
    Join,
    #[default]
    Restart,
}

#[derive(Default, Deserialize)]
pub(super) struct RebuildQuery {
    #[serde(default)]
    mode: RebuildMode,
}

pub(super) async fn engine_rebuild(
    State(state): State<AppState>,
    Query(query): Query<RebuildQuery>,
) -> StatusCode {
    match query.mode {
        RebuildMode::Join => state.engine.join_or_start_background_rebuild(),
        RebuildMode::Restart => state.engine.restart_background_rebuild(),
    }
    StatusCode::ACCEPTED
}

async fn restart_via_gateway(
    gateway_port: &str,
    workspace_id: &str,
) -> Result<StatusCode, (StatusCode, Json<serde_json::Value>)> {
    let fail = |msg: String| {
        log_legacy!("[Restart] {}", msg);
        Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": msg })),
        ))
    };
    let client = match crate::gateway_auth::client_builder().build() {
        Ok(c) => c,
        Err(e) => return fail(format!("gateway restart client build failed: {e}")),
    };
    let url = format!(
        "{}/~/api/v1/control/workspaces/{workspace_id}/restart",
        lucidos_local_token::client::origin(gateway_port)
    );
    log_legacy!("[Restart] Gateway mode: POST {}", url);
    match client.post(&url).send().await {
        Ok(resp) if resp.status().is_success() => Ok(StatusCode::OK),
        Ok(resp) => fail(format!("gateway restart returned {}", resp.status())),
        Err(e) => fail(format!("gateway restart request failed: {e}")),
    }
}

/// INVARIANT: Packaged restart: the engine runs as the `com.lucidos.engine` launchd
/// service, so it restarts itself with `launchctl kickstart -k`. launchd
/// SIGTERMs the service supervisor, which sends the engine its graceful-stop
/// SIGUSR1 and respawns a clean Postgres and engine. We spawn launchctl and
/// return 200 at once: the response flushes during the drain window, and the
/// frontend tolerates a network rejection after a 2xx.
fn restart_via_launchd() -> Result<StatusCode, (StatusCode, Json<serde_json::Value>)> {
    let uid = unsafe { libc::getuid() };
    let target = format!("gui/{uid}/{LAUNCH_AGENT_LABEL}");
    log_legacy!("[Restart] Packaged mode: launchctl kickstart -k {}", target);
    match std::process::Command::new("launchctl")
        .args(["kickstart", "-k", &target])
        .spawn()
    {
        Ok(_) => Ok(StatusCode::OK),
        Err(e) => {
            let msg = format!("Failed to restart service via launchctl: {e}");
            log_legacy!("[Restart] {}", msg);
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": msg })),
            ))
        }
    }
}

pub(super) async fn get_history(
    State(state): State<AppState>,
    Query(query): Query<EventQuery>,
) -> Result<Json<ConversationSnapshot>, (StatusCode, String)> {
    let event_id = query.event;
    match state
        .event_store
        .get_conversation_at_event(event_id, &state.workspace_path)
        .await
    {
        Ok(snapshot) => Ok(Json(snapshot)),
        Err(e) => Err((StatusCode::NOT_FOUND, format!("Error: {}", e))),
    }
}

#[derive(Deserialize)]
pub(super) struct MessagesQuery {
    #[serde(default = "default_messages_limit")]
    limit: i64,
    #[serde(default)]
    before: Option<String>,
}

fn default_messages_limit() -> i64 {
    20
}

fn parse_optional_rfc3339(
    param: &str,
    raw: Option<&str>,
) -> Result<Option<chrono::DateTime<chrono::Utc>>, String> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    match chrono::DateTime::parse_from_rfc3339(raw) {
        Ok(dt) => Ok(Some(dt.with_timezone(&chrono::Utc))),
        Err(e) => Err(format!(
            "{param} '{raw}' is not an RFC 3339 timestamp \
             (e.g. 2026-09-01T00:00:00Z): {e}"
        )),
    }
}

pub(super) async fn get_recent_messages(
    State(state): State<AppState>,
    Query(query): Query<MessagesQuery>,
) -> Result<Json<Vec<SessionMessage>>, (StatusCode, String)> {
    let before = parse_optional_rfc3339("before", query.before.as_deref())
        .map_err(|msg| (StatusCode::BAD_REQUEST, msg))?;
    let limit = query.limit.clamp(1, 500);
    let messages = state
        .event_store
        .get_recent_messages(limit, before)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to load messages: {}", e),
            )
        })?;
    Ok(Json(messages))
}

pub(super) async fn get_session_messages(
    State(state): State<AppState>,
    Query(query): Query<SessionMessagesQuery>,
) -> Result<Json<Vec<SessionMessage>>, (StatusCode, String)> {
    let request_id = query.id;
    let messages = state
        .event_store
        .get_request_messages_by_id(&request_id)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to load messages: {}", e),
            )
        })?;
    Ok(Json(messages))
}

#[derive(Deserialize)]
pub(super) struct EventsQueryParams {
    #[serde(default, alias = "type")]
    event_type: Option<String>,
    #[serde(default)]
    since: Option<String>,
    #[serde(default)]
    until: Option<String>,
    #[serde(default)]
    before_event_id: Option<uuid::Uuid>,
    #[serde(default)]
    after_event_id: Option<uuid::Uuid>,
    #[serde(default)]
    thread_id: Option<uuid::Uuid>,
    #[serde(default)]
    event_id: Option<String>,
    #[serde(default = "default_events_limit")]
    limit: i64,
}

fn default_events_limit() -> i64 {
    100
}

fn validate_cursor_pair(
    before_event_id: Option<uuid::Uuid>,
    after_event_id: Option<uuid::Uuid>,
) -> Result<(), String> {
    if before_event_id.is_some() && after_event_id.is_some() {
        return Err("before_event_id and after_event_id are mutually exclusive".into());
    }
    Ok(())
}

#[derive(Deserialize)]
pub(super) struct EventsCountParams {
    #[serde(default, alias = "type")]
    event_type: Option<String>,
    #[serde(default)]
    since: Option<String>,
    #[serde(default)]
    until: Option<String>,
    #[serde(default)]
    thread_id: Option<uuid::Uuid>,
}

pub(super) async fn count_events(
    State(state): State<AppState>,
    Query(q): Query<EventsCountParams>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let since = parse_optional_rfc3339("since", q.since.as_deref())
        .map_err(|msg| (StatusCode::BAD_REQUEST, msg))?;
    let until = parse_optional_rfc3339("until", q.until.as_deref())
        .map_err(|msg| (StatusCode::BAD_REQUEST, msg))?;
    if let Some(et) = q.event_type.as_deref() {
        let (count, byte_total) = state
            .event_store
            .count_events(Some(et), since, until, q.thread_id)
            .await
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Failed to count events: {}", e),
                )
            })?;
        Ok(Json(serde_json::json!({
            "count": count,
            "byte_total": byte_total,
        })))
    } else {
        let rows = state
            .event_store
            .count_events_by_type(since, until, q.thread_id)
            .await
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Failed to count events: {}", e),
                )
            })?;
        let total_count: i64 = rows.iter().map(|(_, c, _)| *c).sum();
        let total_byte_total: i64 = rows.iter().map(|(_, _, b)| *b).sum();
        let by_type: Vec<serde_json::Value> = rows
            .into_iter()
            .map(|(et, count, byte_total)| {
                serde_json::json!({
                    "event_type": et,
                    "count": count,
                    "byte_total": byte_total,
                })
            })
            .collect();
        Ok(Json(serde_json::json!({
            "by_type": by_type,
            "total_count": total_count,
            "total_byte_total": total_byte_total,
        })))
    }
}

pub(super) async fn query_events(
    State(state): State<AppState>,
    Query(q): Query<EventsQueryParams>,
) -> Result<Json<Vec<crate::core::EventRow>>, (StatusCode, String)> {
    let since = parse_optional_rfc3339("since", q.since.as_deref())
        .map_err(|msg| (StatusCode::BAD_REQUEST, msg))?;
    let until = parse_optional_rfc3339("until", q.until.as_deref())
        .map_err(|msg| (StatusCode::BAD_REQUEST, msg))?;
    let limit = q.limit.clamp(1, 1000);
    if let Err(msg) = validate_cursor_pair(q.before_event_id, q.after_event_id) {
        return Err((StatusCode::BAD_REQUEST, msg));
    }
    // INVARIANT: Refused rather than ignored: a malformed address silently dropped would
    // widen a one-row lookup into the newest 100 events of every type.
    let event_id = match q.event_id.as_deref() {
        None => None,
        Some(raw) => match crate::core::store::parse_event_address(raw) {
            Some(id) => Some(id),
            None => {
                return Err((
                    StatusCode::BAD_REQUEST,
                    format!("event_id '{raw}' is not a uuid or an 'evt-<32 hex>' address"),
                ))
            }
        },
    };
    let result = state
        .event_store
        .query_events_paged(
            crate::core::store::EventQueryFilters {
                event_type: q.event_type.as_deref(),
                since,
                until,
                before_event_id: q.before_event_id,
                after_event_id: q.after_event_id,
                thread_id: q.thread_id,
                event_id,
            },
            limit,
        )
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to query events: {}", e),
            )
        })?;
    match result {
        crate::core::store::QueryEventsResult::Events(mut events) => {
            crate::engine::mask_stored_events(&state.pool, &mut events)
                .await
                .map_err(|e| {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("events withheld, since they could not be masked: {e}"),
                    )
                })?;
            Ok(Json(events))
        }
        crate::core::store::QueryEventsResult::CursorNotFound => Err((
            StatusCode::NOT_FOUND,
            "cursor event_id not found".to_string(),
        )),
    }
}

pub(super) async fn event_types(
    State(state): State<AppState>,
) -> Result<Json<Vec<String>>, (StatusCode, String)> {
    let catalog = crate::core::event_subscription::event_type_catalog(
        &state.event_store,
        crate::core::event_subscription::SubscriptionSurface::Trigger,
    )
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    let mut all: Vec<String> = catalog.engine.into_iter().map(str::to_string).collect();
    all.extend(catalog.workspace);
    all.sort();
    Ok(Json(all))
}

pub(super) async fn emit_event(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<EmitEventRequest>,
) -> Response {
    if let Err(msg) =
        crate::core::event_subscription::validate_emittable_event_type(&body.event_type)
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": msg })),
        )
            .into_response();
    }

    let actor = match super::actor::require_user_actor_response(&headers, &state.pool).await {
        Ok(actor) => actor,
        Err(refusal) => return refusal,
    };

    let (depth, emitting_trigger_id) = emit_chain_and_trigger(&headers);
    let emit = async {
        if body.transient {
            state
                .engine
                .broadcast_transient_domain_event(
                    &body.event_type,
                    body.payload,
                    actor,
                    emitting_trigger_id,
                )
                .await
                .map(|()| None)
        } else {
            state
                .engine
                .emit_domain_event(&body.event_type, body.payload, actor, emitting_trigger_id)
                .await
                .map(Some)
        }
    };
    match crate::scheduler::user_tasks::EVENT_TRIGGER_DEPTH
        .scope(depth, emit)
        .await
    {
        Ok(Some(id)) => Json(serde_json::json!({
            "success": true,
            "event_id": id.to_string(),
        }))
        .into_response(),
        Ok(None) => Json(serde_json::json!({ "success": true })).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("Failed to emit event: {}", e) })),
        )
            .into_response(),
    }
}

fn emit_chain_and_trigger(headers: &HeaderMap) -> (u32, Option<String>) {
    match crate::core::subprocess_origin::subprocess_origin(headers) {
        crate::core::subprocess_origin::SubprocessOrigin::Subprocess {
            chain_depth,
            emitting_trigger_id,
            ..
        } => (chain_depth, emitting_trigger_id),
        crate::core::subprocess_origin::SubprocessOrigin::NotSubprocess => (0, None),
    }
}

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/health", get(health))
        .route("/restart", post(restart_engine))
        .route("/engine/version-status", get(engine_version_status))
        .route("/engine/changelog", get(engine_changelog))
        .route("/engine/rebuild", post(engine_rebuild))
        .route("/engine/build-log", get(engine_build_log))
        .route("/engine/diagnostic-log", get(engine_diagnostic_log))
        .route(
            "/engine/diagnostic-snapshot",
            get(super::diagnostic_snapshot::snapshot),
        )
        .route("/events", get(global_events))
        .route("/history", get(get_history))
        .route("/messages", get(get_recent_messages))
        .route("/session/messages", get(get_session_messages))
        .route("/events/query", get(query_events))
        .route("/events/count", get(count_events))
        .route("/events/types", get(event_types))
        .route("/events/emit", post(emit_event))
        .route(
            "/events/:event_id/context",
            get(super::threads::get_context_capture),
        )
        .route(
            "/events/:event_id/tool-result",
            get(super::threads::get_tool_result),
        )
        .route(
            "/events/:event_id/tool-args",
            get(super::threads::get_tool_args),
        )
        .route(
            "/events/:event_id/location",
            get(super::threads::get_event_location),
        )
}

#[cfg(test)]
#[path = "history_diagnostic_log_tests.rs"]
mod diagnostic_log_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::chat::process::ChatProcessExt;

    #[test]
    fn rebuild_query_defaults_to_restart_and_accepts_join() {
        let query = |uri: &str| Query::<RebuildQuery>::try_from_uri(&uri.parse().unwrap());
        assert!(matches!(
            query("/engine/rebuild").unwrap().mode,
            RebuildMode::Restart
        ));
        assert!(matches!(
            query("/engine/rebuild?mode=join").unwrap().mode,
            RebuildMode::Join
        ));
        assert!(matches!(
            query("/engine/rebuild?mode=restart").unwrap().mode,
            RebuildMode::Restart
        ));
        assert!(query("/engine/rebuild?mode=invalid").is_err());
    }

    #[tokio::test]
    async fn build_log_is_bounded_and_missing_output_is_explicit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("build.log");
        assert_eq!(
            read_build_log(&path).await.unwrap_err().0,
            StatusCode::NOT_FOUND
        );
        let mut bytes = vec![b'x'; usize::try_from(BUILD_LOG_BYTES).unwrap() + 16 * 1024];
        bytes.extend_from_slice(b"publish failed");
        tokio::fs::write(&path, bytes).await.unwrap();
        let text = read_build_log(&path).await.unwrap();
        assert_eq!(text.len(), usize::try_from(BUILD_LOG_BYTES).unwrap());
        assert!(text.ends_with("publish failed"));
    }

    #[tokio::test]
    async fn providerless_boot_health_chat_and_live_configuration() {
        use crate::{core::prefs, llm::ProviderBuildContext, test_support::*};
        const CHILD: &str = "LUCIDOS_TEST_PROVIDERLESS_CHILD";
        if std::env::var_os(CHILD).is_none() {
            let home = tempfile::tempdir().unwrap();
            let output = tokio::time::timeout(
                std::time::Duration::from_secs(120),
                tokio::process::Command::new(std::env::current_exe().unwrap())
                    .args([
                        "--exact",
                        "api::history::tests::providerless_boot_health_chat_and_live_configuration",
                        "--nocapture",
                    ])
                    .env("RUST_MIN_STACK", "16777216")
                    .env(CHILD, "1")
                    .env("GIT_AUTHOR_NAME", "Test User")
                    .env("GIT_AUTHOR_EMAIL", "test@example.com")
                    .env("GIT_COMMITTER_NAME", "Test User")
                    .env("GIT_COMMITTER_EMAIL", "test@example.com")
                    .env("HOME", home.path())
                    .env_remove("LUCIDOS_GATEWAY_DATA")
                    .env_remove("LUCIDOS_BOOT_WITHOUT_PROVIDER")
                    .kill_on_drop(true)
                    .output(),
            )
            .await
            .unwrap()
            .unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }
        let (pool, db) = setup_test_db().await;
        for pref in crate::llm::provider_preferences::PROVIDER_ENABLED_SWITCHES {
            seed_preference(&pool, pref.key(), "false").await.unwrap();
        }
        seed_preference(&pool, prefs::OPENCODE_FREE_ENABLED.key(), "false")
            .await
            .unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let engine = Arc::new(
            crate::engine::LucidosEngine::new(
                workspace.path().to_path_buf(),
                &test_db_url(&db),
                ProviderBuildContext {
                    default_model: prefs::CHAT_MODEL.default_text().into(),
                    model_is_mock: false,
                    vertex_project_id: String::new(),
                    vertex_location: crate::llm::vertex::location_handle(String::new()),
                    vertex_token_cache: None,
                    model_registry: Default::default(),
                    not_served: Default::default(),
                    claude_gateway_access: None,
                    chatgpt_plan_access: None,
                },
                std::sync::Arc::new(crate::engine::agent_recovery::EngineAgentHost),
                Box::new(crate::engine::provider_adapters::ProviderAdapters::default()),
            )
            .await
            .expect("engine boots without provider or opt-in"),
        );
        engine.set_self_arc(&engine);
        let scheduler = crate::scheduler::SchedulerManager::new(engine.clone(), pool.clone())
            .await
            .unwrap();
        let state = AppState {
            engine: engine.clone(),
            pool: pool.clone(),
            event_store: engine.event_store().clone(),
            embedder: crate::memory::EmbedderSlot::empty(),
            memory_index: None,
            workspace_path: workspace.path().to_path_buf(),
            app_manager: engine.shared_app_manager(),
            scheduler: Arc::new(tokio::sync::Mutex::new(scheduler)),
            started_at: chrono::Utc::now(),
            pending_oauth_flows: Default::default(),
            reveal_tokens: Default::default(),
            socket_tickets: Default::default(),
            event_hooks: std::sync::Arc::new(crate::api::hooks::NoEventHooks),
        };
        let (status, Json(body)) = health(State(state.clone())).await;
        let contract: lucidos_gateway_stamp::workspace_client::ClientContract =
            serde_json::from_value(body["workspace_client"].clone()).unwrap();
        assert!(contract.supports_engine());
        assert!(!contract.supports_gateway());
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["llm_configured"], false);
        let thread = uuid::Uuid::new_v4();
        let _result = engine
            .process_message_with_steps(
                "Hello",
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                Some(false),
                None,
                Some(thread),
                None,
                None,
                None,
                None,
                None,
                None,
                crate::engine::thread_events::ActorMode::Human,
                None,
                None,
                None,
                None,
                None,
                None,
                crate::engine::FollowUpUrgency::Normal,
                None,
                None,
            )
            .await;
        let failure: serde_json::Value = sqlx::query_scalar("SELECT payload FROM events WHERE thread_id = $1 AND event_type = 'ResponseFailed' ORDER BY sequence DESC LIMIT 1")
            .bind(thread).fetch_one(&pool).await.expect("chat emits visible failure");
        assert!(
            failure.to_string().contains("No LLM provider configured"),
            "{failure}"
        );
        assert!(
            failure.to_string().contains(providers_settings!()),
            "{failure}"
        );
        for (key, value) in [
            (prefs::LOCAL_BASE_URL.key(), "http://127.0.0.1:1/v1"),
            (prefs::PROVIDER_ENABLED_LOCAL.key(), "true"),
        ] {
            crate::core::PreferenceStore::set(&pool, &engine.event_bus, key, value, None)
                .await
                .unwrap();
        }
        async fn wait_configured(engine: &crate::engine::LucidosEngine, expected: bool) {
            tokio::time::timeout(std::time::Duration::from_secs(10), async {
                while engine.llm_configured() != expected {
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("provider subscriber updates live handle");
        }
        wait_configured(&engine, true).await;
        crate::core::PreferenceStore::set(
            &pool,
            &engine.event_bus,
            prefs::PROVIDER_ENABLED_LOCAL.key(),
            "false",
            None,
        )
        .await
        .unwrap();
        wait_configured(&engine, false).await;
        assert_eq!(health(State(state)).await.1 .0["llm_configured"], false);
        engine.mark_shutting_down();
        drop(engine);
        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[test]
    fn stream_hello_exposes_only_the_durable_generation() {
        let generation = uuid::Uuid::new_v4();
        let hello: serde_json::Value =
            serde_json::from_str(&stream_hello(Some(generation))).unwrap();
        assert_eq!(
            hello,
            serde_json::json!({
                "type": "StreamHello", "data": { "restore_generation": generation }
            })
        );
        let hello: serde_json::Value = serde_json::from_str(&stream_hello(None)).unwrap();
        assert!(hello["data"]["restore_generation"].is_null());
    }

    #[tokio::test]
    async fn restore_generation_precedes_domain_frames_on_both_transports() {
        use async_compression::tokio::bufread::GzipDecoder;
        use tokio::io::{AsyncReadExt, BufReader};
        use tokio_util::io::StreamReader;

        for gzip in [false, true] {
            let generation = uuid::Uuid::new_v4();
            let hello = stream_hello(Some(generation));
            let domain = "{\"type\":\"ThreadEvent\",\"data\":{}}".to_owned();
            let frames = tokio_stream::once(hello.clone())
                .chain(tokio_stream::once(domain.clone()))
                .chain(futures::stream::pending());
            let response = if gzip {
                gzipped_sse_response(frames)
            } else {
                plain_sse_response(frames)
            };
            let body = response
                .into_body()
                .into_data_stream()
                .map(|chunk| chunk.map_err(std::io::Error::other));
            let reader = StreamReader::new(body);
            let mut reader: std::pin::Pin<Box<dyn tokio::io::AsyncRead>> = if gzip {
                Box::pin(GzipDecoder::new(BufReader::new(reader)))
            } else {
                Box::pin(reader)
            };
            let expected = format!("data: {hello}\n\ndata: {domain}\n\n");
            let mut wire = Vec::new();
            tokio::time::timeout(std::time::Duration::from_secs(5), async {
                while !String::from_utf8_lossy(&wire).contains(&format!("data: {domain}\n\n")) {
                    let mut chunk = [0; 1024];
                    let count = reader.read(&mut chunk).await.unwrap();
                    assert!(count > 0);
                    wire.extend_from_slice(&chunk[..count]);
                }
            })
            .await
            .unwrap();
            let wire = String::from_utf8(wire)
                .unwrap()
                .replace("event: keepalive\ndata:\n\n", "");
            assert_eq!(wire, expected);
        }
    }

    #[test]
    fn an_engine_in_teardown_reports_unavailable() {
        assert_eq!(
            health_status(true),
            (StatusCode::SERVICE_UNAVAILABLE, "shutting_down")
        );
        assert_eq!(health_status(false), (StatusCode::OK, "ok"));
    }

    #[test]
    fn an_emit_takes_its_depth_and_its_fire_from_the_signed_token() {
        use crate::core::subprocess_origin::{
            init_agent_origin_secret, mint_agent_origin_token, HEADER_AGENT_ORIGIN_TOKEN,
        };
        init_agent_origin_secret("history-emit-secret".to_string());

        let with = |token: Option<&str>| {
            let mut headers = HeaderMap::new();
            if let Some(token) = token {
                headers.insert(
                    HEADER_AGENT_ORIGIN_TOKEN,
                    HeaderValue::from_str(token).expect("a minted token is header safe"),
                );
            }
            emit_chain_and_trigger(&headers)
        };

        let fire = mint_agent_origin_token(None, 2, Some("nightly-scan")).expect("secret is set");
        assert_eq!(with(Some(&fire)), (2, Some("nightly-scan".to_string())));

        let plain = mint_agent_origin_token(None, 1, None).expect("secret is set");
        assert_eq!(
            with(Some(&plain)),
            (1, None),
            "a spawn outside any fire claims no trigger"
        );

        assert_eq!(
            with(None),
            (0, None),
            "an untrusted caller roots its own chain and suppresses nobody"
        );
    }

    const KEEPALIVE_WIRE: &[u8] = b"event: keepalive\ndata:\n\n";

    #[tokio::test]
    async fn the_gzipped_stream_writes_its_first_bytes_before_any_event() {
        use async_compression::tokio::bufread::GzipDecoder;
        use tokio::io::{AsyncReadExt, BufReader};
        use tokio_util::io::StreamReader;

        let silent = futures::stream::pending::<String>();
        let response = gzipped_sse_response(silent);
        let body = response
            .into_body()
            .into_data_stream()
            .map(|chunk| chunk.map_err(std::io::Error::other));
        let mut wire = GzipDecoder::new(BufReader::new(StreamReader::new(body)));

        let mut first = vec![0u8; KEEPALIVE_WIRE.len()];
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            wire.read_exact(&mut first),
        )
        .await
        .expect("the stream must announce itself without waiting for an event")
        .expect("the first frame must decode");
        assert_eq!(first, KEEPALIVE_WIRE);
    }

    #[tokio::test]
    async fn the_plain_stream_opens_with_the_keepalive_event_and_relays_data() {
        let events = futures::stream::iter(["{\"a\":1}".to_string()])
            .chain(futures::stream::pending::<String>());
        let response = plain_sse_response(events);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/event-stream"
        );
        let mut body = response.into_body().into_data_stream();

        let mut wire = Vec::new();
        let want = KEEPALIVE_WIRE.len() + b"data: {\"a\":1}\n\n".len();
        while wire.len() < want {
            let chunk = tokio::time::timeout(std::time::Duration::from_secs(5), body.next())
                .await
                .expect("both frames are ready at once")
                .expect("the stream must not end")
                .expect("a frame must not be an error");
            wire.extend_from_slice(&chunk);
        }
        let wire = String::from_utf8(wire).unwrap();
        assert!(wire.contains("event: keepalive\ndata:\n\n"), "{wire:?}");
        assert!(wire.contains("data: {\"a\":1}\n\n"), "{wire:?}");
        assert!(
            wire.lines().all(|line| !line.starts_with(':')),
            "a comment line never reaches JS: {wire:?}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_quiet_stream_sends_a_keepalive_every_interval() {
        let frames = with_keepalives(futures::stream::pending::<String>());
        tokio::pin!(frames);
        for _ in 0..3 {
            assert!(matches!(frames.next().await, Some(Frame::Keepalive)));
        }
    }

    #[test]
    fn the_keepalive_frame_names_the_exported_event() {
        let wire = Frame::Keepalive.chunks().concat();
        assert_eq!(
            wire,
            format!("event: {SSE_KEEPALIVE_EVENT}\ndata:\n\n").into_bytes()
        );
    }

    #[test]
    fn is_packaged_false_for_source_build() {
        assert!(
            !is_packaged(),
            "a source-built engine must not be classified as packaged"
        );
    }

    #[test]
    fn accepts_gzip_recognises_standard_offers() {
        fn h(value: &str) -> axum::http::HeaderMap {
            let mut m = axum::http::HeaderMap::new();
            m.insert(
                axum::http::header::ACCEPT_ENCODING,
                axum::http::HeaderValue::from_str(value).unwrap(),
            );
            m
        }
        assert!(accepts_gzip(&h("gzip")));
        assert!(accepts_gzip(&h("GZIP")));
        assert!(accepts_gzip(&h("gzip;q=1.0")));
        assert!(accepts_gzip(&h("deflate, gzip")));
        assert!(accepts_gzip(&h("br, gzip;q=0.9, deflate;q=0.5")));
        assert!(!accepts_gzip(&h("deflate")));
        assert!(!accepts_gzip(&h("br")));
        assert!(!accepts_gzip(&h("x-gzip")));
        assert!(!accepts_gzip(&h("")));
        assert!(!accepts_gzip(&axum::http::HeaderMap::new()));

        assert!(!accepts_gzip(&h("gzip;q=0")));
        assert!(!accepts_gzip(&h("gzip; q=0")));
        assert!(!accepts_gzip(&h("gzip;q=0.0")));
        assert!(!accepts_gzip(&h("gzip;q=0.000")));
        assert!(!accepts_gzip(&h("deflate, gzip;q=0")));
        assert!(accepts_gzip(&h("gzip, deflate;q=0")));
    }

    #[test]
    fn the_dropdown_seed_offers_only_names_a_trigger_can_fire_on() {
        use crate::core::event_subscription::{known_names, SubscriptionSurface};
        let seed = known_names::subscribable_event_type_names(SubscriptionSurface::Trigger);

        for name in &seed {
            assert!(
                crate::engine::thread_lifecycle::classify_event(name).is_some()
                    || crate::engine::event_bus::SystemEvent::is_persisted_type_name(name),
                "'{name}' is neither a ThreadEvent nor a persisted SystemEvent, so \
                 the matcher never sees it and the dropdown would offer a \
                 subscription that can never fire.",
            );
        }

        for streaming in crate::core::event_subscription::PER_TOKEN_STREAMING_EVENT_TYPES {
            assert!(!seed.contains(streaming), "'{streaming}' never reaches it");
        }

        assert!(seed.contains(&"ChildThreadCompleted"));
    }

    #[test]
    fn a_malformed_timestamp_cursor_is_refused_rather_than_dropped() {
        let err = parse_optional_rfc3339("since", Some("2026-09-01"))
            .expect_err("a bare date is not RFC 3339");
        assert!(
            err.contains("since") && err.contains("2026-09-01"),
            "the refusal names the parameter and the value, got: {err}"
        );

        for bad in ["", "yesterday", "2026-09-01 00:00:00", "1757030400"] {
            assert!(
                parse_optional_rfc3339("before", Some(bad)).is_err(),
                "{bad:?} must not pass as a cursor"
            );
        }
    }

    #[test]
    fn an_absent_cursor_is_no_bound_and_a_valid_one_lands_in_utc() {
        assert_eq!(parse_optional_rfc3339("since", None), Ok(None));

        let parsed = parse_optional_rfc3339("since", Some("2026-09-01T02:00:00+02:00"))
            .expect("a well-formed offset timestamp")
            .expect("present");
        assert_eq!(parsed.to_rfc3339(), "2026-09-01T00:00:00+00:00");
    }

    #[test]
    fn validate_cursor_pair_allows_zero_or_one_cursor() {
        assert!(validate_cursor_pair(None, None).is_ok());
        assert!(validate_cursor_pair(Some(uuid::Uuid::new_v4()), None).is_ok());
        assert!(validate_cursor_pair(None, Some(uuid::Uuid::new_v4())).is_ok());
    }

    #[test]
    fn validate_cursor_pair_rejects_both_cursors_set() {
        let err = validate_cursor_pair(Some(uuid::Uuid::new_v4()), Some(uuid::Uuid::new_v4()))
            .unwrap_err();
        assert!(
            err.contains("mutually exclusive"),
            "error should mention mutual exclusion: {err}"
        );
    }

    mod row_stream_tests {
        use super::*;
        use crate::engine::event_bus::{BusEvent, EmittedEvent, EventBus, SystemEvent};
        use lucidos_gateway_stamp::row_stream;

        const SECRET: &str = "row-stream-payload-marker";

        fn picker_row_events() -> Vec<SystemEvent> {
            let now = chrono::Utc::now();
            let actor = Some(crate::engine::thread_events::MessageOrigin::Device {
                device_id: SECRET.into(),
            });
            vec![
                SystemEvent::NotificationCreated {
                    id: uuid::Uuid::new_v4().to_string(),
                    title: SECRET.into(),
                    message: SECRET.into(),
                    task_id: None,
                    app_id: Some(SECRET.into()),
                    thread_id: None,
                    event_id: None,
                    tap: Default::default(),
                    actor: actor.clone(),
                    audience: Some(vec![SECRET.into()]),
                    push_group: None,
                },
                SystemEvent::NotificationRead {
                    id: SECRET.into(),
                    actor: actor.clone(),
                },
                SystemEvent::NotificationsAllRead {
                    actor: actor.clone(),
                },
                threads_deleted(actor.clone()),
                SystemEvent::UsersChanged,
                SystemEvent::BackupCompleted {
                    filename: SECRET.into(),
                    size_bytes: 1,
                    started_at: now,
                    finished_at: now,
                },
                SystemEvent::BackupFailed {
                    error: SECRET.into(),
                    started_at: now,
                    finished_at: now,
                },
                SystemEvent::PreferencesChanged {
                    key: SECRET.into(),
                    value: Some(SECRET.into()),
                    actor,
                },
            ]
        }

        fn threads_deleted(
            actor: Option<crate::engine::thread_events::MessageOrigin>,
        ) -> SystemEvent {
            SystemEvent::ThreadsDeleted {
                thread_ids: vec![uuid::Uuid::new_v4()],
                event_count: 1,
                memory_count: 0,
                worktrees_removed: 0,
                widgets_removed: 0,
                actor,
            }
        }

        fn emitted(typed: BusEvent) -> EmittedEvent {
            EmittedEvent {
                event_id: uuid::Uuid::new_v4(),
                seq: None,
                created: chrono::Utc::now(),
                typed,
                aggregate: None,
                depth: 0,
                emitting_trigger_id: None,
            }
        }

        fn wanted() -> Vec<String> {
            row_stream::AGGREGATES.map(str::to_string).to_vec()
        }

        #[test]
        fn the_shared_contract_names_the_engines_events_and_frames() {
            let events = picker_row_events();
            let types: Vec<&str> = events.iter().map(SystemEvent::event_type).collect();
            let sent = [
                &row_stream::UNREAD_EVENT_TYPES[..],
                &row_stream::BACKUP_EVENT_TYPES[..],
            ]
            .concat();
            assert_eq!(types, sent);
            for event in &events {
                assert!(
                    row_stream::AGGREGATES.contains(&event.aggregate()),
                    "{}",
                    event.event_type()
                );
            }
            assert_eq!(row_stream::SILENCE_LIMIT, SSE_KEEPALIVE_INTERVAL * 3);
            let hello: serde_json::Value = serde_json::from_str(&stream_hello(None)).unwrap();
            assert_eq!(
                row_stream::RESYNC_FRAMES,
                [hello["type"].as_str().unwrap(), LAGGED_FRAME]
            );
        }

        #[test]
        fn a_frame_never_carries_a_payload() {
            for event in picker_row_events() {
                let expected = serde_json::json!({
                    "type": event.event_type(),
                    "aggregate": event.aggregate(),
                });
                let marked = !matches!(event, SystemEvent::UsersChanged);
                let whole = emitted(BusEvent::System(event));
                assert_eq!(
                    whole.to_sse_json().contains(SECRET),
                    marked,
                    "the fixture is marked wherever it has a payload"
                );
                let frame = row_stream_frame(&whole, &wanted()).expect("a frame");
                assert!(!frame.contains(SECRET), "{frame}");
                let parsed: serde_json::Value = serde_json::from_str(&frame).unwrap();
                assert_eq!(parsed, expected);
            }
        }

        #[test]
        fn events_outside_the_shared_lists_are_dropped() {
            let dropped = [
                SystemEvent::BackupKeyRevealed { actor: None },
                SystemEvent::LanguageSet {
                    language: SECRET.into(),
                },
            ];
            for event in dropped {
                let name = event.event_type();
                let whole = emitted(BusEvent::System(event));
                assert_eq!(row_stream_frame(&whole, &wanted()), None, "{name}");
            }
        }

        async fn next_frame(body: &mut axum::body::BodyDataStream) -> serde_json::Value {
            loop {
                let chunk = tokio::time::timeout(std::time::Duration::from_secs(10), body.next())
                    .await
                    .expect("a frame within the budget")
                    .expect("the stream stays open")
                    .unwrap();
                let text = String::from_utf8(chunk.to_vec()).unwrap();
                if let Some(json) = text.strip_prefix("data: ") {
                    return serde_json::from_str(json.trim()).unwrap();
                }
            }
        }

        #[tokio::test]
        async fn every_picker_row_write_reaches_the_stream_and_no_page_is_counted() {
            use crate::core::preferences::PreferenceStore;
            use crate::core::prefs;
            use crate::gateway_users::Reader;
            use crate::scheduler::notifications::NotificationStore;

            let (pool, db) = crate::test_support::setup_test_db().await;
            let (bus, _parent_rx) = EventBus::new(pool.clone());
            let pages = crate::api::sse_connections::SseConnectionCounter::new();
            let aggregates = row_stream::AGGREGATES.join(",");
            let response =
                event_stream(&bus, &pages, &pool, Some(&aggregates), &HeaderMap::new()).await;
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(pages.count(), 0, "a row stream is not a page");
            let mut body = response.into_body().into_data_stream();
            assert_eq!(next_frame(&mut body).await["type"], "StreamHello");

            let mut created = Vec::new();
            for _ in 0..2 {
                let id = uuid::Uuid::new_v4();
                created.push(id);
                bus.emit(BusEvent::System(SystemEvent::NotificationCreated {
                    id: id.to_string(),
                    title: SECRET.into(),
                    message: SECRET.into(),
                    task_id: None,
                    app_id: None,
                    thread_id: None,
                    event_id: None,
                    tap: Default::default(),
                    actor: None,
                    audience: None,
                    push_group: None,
                }))
                .await
                .unwrap();
            }
            assert!(
                NotificationStore::mark_read(&pool, &bus, &Reader::Team, created[0], None)
                    .await
                    .unwrap()
            );
            assert_eq!(
                NotificationStore::mark_all_read(&pool, &bus, &Reader::Team, None)
                    .await
                    .unwrap(),
                1
            );
            bus.emit(BusEvent::System(threads_deleted(None)))
                .await
                .unwrap();
            bus.emit(BusEvent::System(SystemEvent::UsersChanged))
                .await
                .unwrap();
            PreferenceStore::set(&pool, &bus, prefs::BACKUP_PROVIDER.key(), SECRET, None)
                .await
                .unwrap();
            bus.emit(BusEvent::System(SystemEvent::BackupKeyRevealed {
                actor: None,
            }))
            .await
            .unwrap();
            let now = chrono::Utc::now();
            bus.emit(BusEvent::System(SystemEvent::BackupCompleted {
                filename: SECRET.into(),
                size_bytes: 1,
                started_at: now,
                finished_at: now,
            }))
            .await
            .unwrap();

            let mut seen = Vec::new();
            for _ in 0..8 {
                let frame = next_frame(&mut body).await;
                assert!(!frame.to_string().contains(SECRET), "{frame}");
                assert_eq!(frame.as_object().unwrap().len(), 2, "{frame}");
                seen.push((
                    frame["type"].as_str().unwrap().to_string(),
                    frame["aggregate"].as_str().unwrap().to_string(),
                ));
            }
            let pair = |t: &str, a: &str| (t.to_string(), a.to_string());
            assert_eq!(
                seen,
                [
                    pair("NotificationCreated", "notification"),
                    pair("NotificationCreated", "notification"),
                    pair("NotificationRead", "notification"),
                    pair("NotificationsAllRead", "notification"),
                    pair("ThreadsDeleted", "ops"),
                    pair("UsersChanged", "user"),
                    pair("PreferencesChanged", "preference"),
                    pair("BackupCompleted", "ops"),
                ]
            );

            let page = event_stream(&bus, &pages, &pool, None, &HeaderMap::new()).await;
            assert_eq!(pages.count(), 1);
            let mut page_body = page.into_body().into_data_stream();
            assert_eq!(next_frame(&mut page_body).await["type"], "StreamHello");
            let event = SystemEvent::BackupFailed {
                error: SECRET.into(),
                started_at: now,
                finished_at: now,
            };
            bus.emit(BusEvent::System(event)).await.unwrap();
            let whole = next_frame(&mut page_body).await;
            assert_eq!(whole["data"]["error"], SECRET);
            drop(page_body);
            assert_eq!(pages.count(), 0);

            drop(body);
            pool.close().await;
            crate::test_support::teardown_test_db(&db).await;
        }
    }

    #[test]
    fn lagged_event_json_has_stable_wire_shape() {
        let parsed: serde_json::Value = serde_json::from_str(&lagged_event_json(42)).unwrap();
        assert_eq!(parsed["type"], "Lagged");
        assert_eq!(parsed["data"]["count"], 42);
    }

    mod guard_tests {
        use super::*;

        #[test]
        fn read_app_version_reads_fresh_from_disk() {
            let engine_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
            let version_file = engine_dir
                .parent()
                .unwrap()
                .join("lucidos-app")
                .join("VERSION");
            let original = std::fs::read_to_string(&version_file).ok();

            struct Restore(std::path::PathBuf, Option<String>);
            impl Drop for Restore {
                fn drop(&mut self) {
                    match &self.1 {
                        Some(content) => {
                            let _ = std::fs::write(&self.0, content);
                        }
                        None => {
                            let _ = std::fs::remove_file(&self.0);
                        }
                    }
                }
            }
            let _guard = Restore(version_file.clone(), original);

            std::fs::write(&version_file, "1.0.0-test\n").unwrap();
            assert_eq!(read_app_version(), "1.0.0-test");

            std::fs::write(&version_file, "2.0.0-test\n").unwrap();
            assert_eq!(read_app_version(), "2.0.0-test");

            std::fs::remove_file(&version_file).unwrap();
            assert_eq!(read_app_version(), "unknown");
        }

        #[test]
        fn read_engine_version_reads_fresh_from_disk() {
            let engine_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
            let version_file = engine_dir.join("../lucidos-engine/VERSION");
            let original = std::fs::read_to_string(&version_file).ok();

            struct Restore(std::path::PathBuf, Option<String>);
            impl Drop for Restore {
                fn drop(&mut self) {
                    match &self.1 {
                        Some(content) => {
                            let _ = std::fs::write(&self.0, content);
                        }
                        None => {
                            let _ = std::fs::remove_file(&self.0);
                        }
                    }
                }
            }
            let _guard = Restore(version_file.clone(), original);

            std::fs::write(&version_file, "2026.04.13.1\n").unwrap();
            assert_eq!(read_engine_version(), "2026.04.13.1");

            std::fs::write(&version_file, "2026.04.13.2\n").unwrap();
            assert_eq!(read_engine_version(), "2026.04.13.2");

            std::fs::remove_file(&version_file).unwrap();
            assert_eq!(read_engine_version(), "unknown");
        }
    }
}
