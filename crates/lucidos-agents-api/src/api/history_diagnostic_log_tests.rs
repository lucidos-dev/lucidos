use super::*;

fn diagnostic_headers(pairs: &[(&'static str, String)]) -> HeaderMap {
    let mut headers = HeaderMap::new();
    for (name, value) in pairs {
        headers.insert(*name, value.parse().unwrap());
    }
    headers
}

/// A token this engine minted, exactly as a spawn hands it out.
fn agent_token() -> String {
    crate::core::subprocess_origin::init_agent_origin_secret("subprocess-test-secret".to_string());
    crate::core::subprocess_origin::mint_agent_origin_token(None, 0, None)
        .expect("secret installed")
}

/// The refusal a query met, as status and reason slug.
async fn diagnostic_refusal(
    headers: &HeaderMap,
    raw: Option<&str>,
    dir: std::path::PathBuf,
    started: bool,
    scans: &'static tokio::sync::Semaphore,
) -> (StatusCode, Option<&'static str>) {
    let source = DiagnosticLogSource {
        dir,
        started,
        gateway_origin: None,
        scans,
    };
    let refusal = query_diagnostic_log(headers, raw, source)
        .await
        .expect_err("the query is refused");
    (refusal.status, refusal.reason)
}

#[tokio::test]
async fn diagnostic_log_answers_only_this_engines_agents() {
    static SCANS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);
    let dir = tempfile::tempdir().unwrap();
    let token = crate::core::subprocess_origin::HEADER_AGENT_ORIGIN_TOKEN;
    let app = crate::api::app_reach::APP_ID_HEADER;
    let refused = [
        diagnostic_headers(&[]),
        diagnostic_headers(&[(token, "-.0.-.deadbeef".to_string())]),
        diagnostic_headers(&[(app, "habit-tracker".to_string())]),
        diagnostic_headers(&[(token, agent_token()), (app, "habit-tracker".to_string())]),
    ];
    for headers in &refused {
        for raw in [None, Some("source=gateway"), Some("source=all")] {
            assert_eq!(
                diagnostic_refusal(headers, raw, dir.path().into(), true, &SCANS).await,
                (StatusCode::FORBIDDEN, Some("diagnostic_log_agent_only")),
                "{headers:?}"
            );
        }
    }
    let agent = diagnostic_headers(&[(token, agent_token())]);
    let source = DiagnosticLogSource {
        dir: dir.path().into(),
        started: true,
        gateway_origin: None,
        scans: &SCANS,
    };
    let answer = query_diagnostic_log(&agent, Some("since=2h"), source)
        .await
        .expect("an agent of this engine gets an answer");
    assert_eq!(answer.returned, 0);
}

#[tokio::test]
async fn diagnostic_log_refusals_carry_their_reason() {
    static SCANS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);
    static FULL: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(0);
    let dir = tempfile::tempdir().unwrap();
    let agent = diagnostic_headers(&[(
        crate::core::subprocess_origin::HEADER_AGENT_ORIGIN_TOKEN,
        agent_token(),
    )]);
    assert_eq!(
        diagnostic_refusal(&agent, Some("since=8d"), dir.path().into(), true, &SCANS).await,
        (StatusCode::BAD_REQUEST, Some("diagnostic_log_bad_query"))
    );
    assert_eq!(
        diagnostic_refusal(
            &agent,
            Some("source=engine"),
            dir.path().into(),
            false,
            &SCANS
        )
        .await,
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Some("diagnostic_log_unavailable")
        )
    );
    assert_eq!(
        diagnostic_refusal(
            &agent,
            Some("source=engine"),
            dir.path().into(),
            true,
            &FULL
        )
        .await,
        (StatusCode::TOO_MANY_REQUESTS, Some("diagnostic_log_busy"))
    );
    let outside = tempfile::tempdir().unwrap();
    let linked = dir.path().join("diagnostic-log");
    std::os::unix::fs::symlink(outside.path(), &linked).unwrap();
    assert_eq!(
        diagnostic_refusal(&agent, Some("source=engine"), linked, true, &SCANS).await,
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Some("diagnostic_log_unavailable")
        )
    );
}

#[tokio::test]
async fn diagnostic_log_names_missing_gateway_sources_and_keeps_engine_lines() {
    use crate::diagnostic_log::query::{Source, SourceStatus};

    static SCANS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);
    let dir = tempfile::tempdir().unwrap();
    let record = serde_json::json!({
        "ts": chrono::Utc::now().to_rfc3339(), "pid": 7, "label": "Test", "msg": "engine evidence",
    });
    std::fs::write(
        dir.path().join(crate::diagnostic_log::writer::FILE_NAME),
        format!("{record}\n"),
    )
    .unwrap();
    let agent = diagnostic_headers(&[(
        crate::core::subprocess_origin::HEADER_AGENT_ORIGIN_TOKEN,
        agent_token(),
    )]);
    let (missing, missing_server) = serve_test_gateway(axum::Router::new()).await;
    let (picker, picker_server) = serve_test_gateway(axum::Router::new().fallback(|| async {
        axum::response::Html("<!doctype html><title>Workspace picker</title>")
    }))
    .await;
    for (gateway_origin, status) in [
        (None, SourceStatus::GatewayUnreachable),
        (Some(missing), SourceStatus::GatewayTooOld),
        (Some(picker), SourceStatus::GatewayTooOld),
    ] {
        let source = DiagnosticLogSource {
            dir: dir.path().into(),
            started: true,
            gateway_origin,
            scans: &SCANS,
        };
        let answer = query_diagnostic_log(&agent, Some("source=all"), source)
            .await
            .unwrap();
        assert_eq!(answer.lines[0].msg, "engine evidence");
        assert_eq!(answer.sources[&Source::Engine], SourceStatus::Ok);
        assert_eq!(answer.sources[&Source::Gateway], status);
        assert_eq!(answer.sources[&Source::Client], status);
    }
    missing_server.abort();
    picker_server.abort();
}

async fn serve_test_gateway(router: axum::Router) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (origin, server)
}

#[tokio::test]
async fn diagnostic_log_full_engine_scans_do_not_block_machine_sources() {
    use crate::diagnostic_log::query::{Source, SourceStatus};

    static SCANS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);
    let _held = SCANS.try_acquire_many(2).unwrap();
    let agent = diagnostic_headers(&[(
        crate::core::subprocess_origin::HEADER_AGENT_ORIGIN_TOKEN,
        agent_token(),
    )]);
    let (origin, server) = serve_test_gateway(axum::Router::new().route(
        "/~/api/v1/control/diagnostic-log",
        get(
            |axum::extract::RawQuery(raw): axum::extract::RawQuery| async move {
                use crate::diagnostic_log::query;
                let query = query::parse(raw.as_deref(), chrono::Utc::now()).unwrap();
                Json(query::machine_unavailable(&query, SourceStatus::NotPresent))
            },
        ),
    ))
    .await;
    let dir = tempfile::tempdir().unwrap();
    for (raw, expected) in [
        (
            "source=all",
            vec![
                (Source::Engine, SourceStatus::Busy),
                (Source::Gateway, SourceStatus::NotPresent),
                (Source::Client, SourceStatus::NotPresent),
            ],
        ),
        (
            "source=gateway",
            vec![(Source::Gateway, SourceStatus::NotPresent)],
        ),
        (
            "source=client",
            vec![(Source::Client, SourceStatus::NotPresent)],
        ),
    ] {
        let answer = query_diagnostic_log(
            &agent,
            Some(raw),
            DiagnosticLogSource {
                dir: dir.path().into(),
                started: true,
                gateway_origin: Some(origin.clone()),
                scans: &SCANS,
            },
        )
        .await
        .unwrap();
        assert_eq!(answer.sources, expected.into_iter().collect(), "{raw}");
    }
    server.abort();
}

#[tokio::test]
async fn diagnostic_log_waiting_for_gateway_releases_engine_scan_slot() {
    use std::sync::Arc;
    use tokio::sync::Notify;

    static SCANS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);
    let _held = SCANS.try_acquire().unwrap();
    let agent = diagnostic_headers(&[(
        crate::core::subprocess_origin::HEADER_AGENT_ORIGIN_TOKEN,
        agent_token(),
    )]);
    let arrived = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let (origin, server) = serve_test_gateway(axum::Router::new().route(
        "/~/api/v1/control/diagnostic-log",
        get({
            let arrived = Arc::clone(&arrived);
            let release = Arc::clone(&release);
            move || async move {
                arrived.notify_one();
                release.notified().await;
                StatusCode::NOT_FOUND
            }
        }),
    ))
    .await;
    let dir = tempfile::tempdir().unwrap();
    let source = DiagnosticLogSource {
        dir: dir.path().into(),
        started: true,
        gateway_origin: Some(origin),
        scans: &SCANS,
    };
    let agent_copy = agent.clone();
    let all =
        tokio::spawn(
            async move { query_diagnostic_log(&agent_copy, Some("source=all"), source).await },
        );
    tokio::time::timeout(std::time::Duration::from_secs(5), arrived.notified())
        .await
        .expect("the combined query reaches the gateway");
    let engine = query_diagnostic_log(
        &agent,
        Some("source=engine"),
        DiagnosticLogSource {
            dir: dir.path().into(),
            started: true,
            gateway_origin: None,
            scans: &SCANS,
        },
    )
    .await;
    release.notify_one();
    all.await.unwrap().unwrap();
    server.abort();
    assert!(engine.is_ok(), "the engine slot is free: {engine:?}");
}
