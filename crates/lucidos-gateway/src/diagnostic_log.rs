use axum::extract::{RawQuery, State};
use axum::http::header;
use axum::response::IntoResponse;
use axum::{Extension, Json};
use lucidos_diagnostic_log::query::{self, Query, QueryResult, Source, SourceStatus};
use lucidos_diagnostic_log::{self as shared, Destination, Line};
use std::path::{Path, PathBuf};

pub fn dir_for(app_data: &Path) -> PathBuf {
    app_data.join("diagnostic-log")
}

pub(crate) fn owns_client_data(app_data: &Path, desktop_data: Option<&Path>) -> bool {
    desktop_data == Some(app_data)
}

pub(crate) async fn query(
    State(state): State<crate::server::GatewayState>,
    local: Option<Extension<crate::auth::AuthenticatedLocalProcess>>,
    RawQuery(raw): RawQuery,
) -> Result<impl IntoResponse, crate::error::ApiError> {
    use crate::error::ApiError;
    static SCANS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);

    if local.is_none() {
        return Err(ApiError::forbidden(
            "diagnostic log queries require the local token",
        ));
    }
    let query = query::parse(raw.as_deref(), chrono::Utc::now()).map_err(ApiError::bad_request)?;
    if query.engine_only() {
        return Err(ApiError::bad_request(
            "the gateway serves only gateway and client logs",
        ));
    }

    let allow_client = query.includes(Source::Client) && state.allows_client_diagnostics();
    let client_status = || QueryResult::unavailable(Source::Client, SourceStatus::TeamGateway);
    let result = if !allow_client && !query.includes(Source::Gateway) {
        query::merge(&query, [client_status()])
    } else if let Ok(permit) = SCANS.try_acquire() {
        let gateway_data = state.app_data().clone();
        let has_desktop_client = state.has_desktop_client();
        let filter = query.clone();
        let (gateway, client) = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let gateway = filter
                .includes(Source::Gateway)
                .then(|| query::run_source(&dir_for(&gateway_data), &filter, Source::Gateway));
            let client =
                allow_client.then(|| query_client(&filter, &gateway_data, has_desktop_client));
            (gateway, client)
        })
        .await
        .map_err(|_| ApiError::internal("diagnostic log scan failed"))?;
        let client = query.includes(Source::Client).then(|| {
            client
                .filter(|_| state.allows_client_diagnostics())
                .unwrap_or_else(client_status)
        });
        query::merge(&query, gateway.into_iter().chain(client))
    } else {
        query::merge(
            &query,
            query.machine_sources().map(|source| {
                if source == Source::Client && !allow_client {
                    client_status()
                } else {
                    QueryResult::unavailable(source, SourceStatus::Busy)
                }
            }),
        )
    };
    Ok(([(header::CACHE_CONTROL, "no-store")], Json(result)))
}

fn query_client(query: &Query, gateway_data: &Path, has_desktop_client: bool) -> QueryResult {
    if has_desktop_client {
        query::run_source(
            &gateway_data.join(shared::CLIENT_LIFECYCLE_DIR),
            query,
            Source::Client,
        )
    } else {
        QueryResult::unavailable(
            Source::Client,
            if cfg!(target_os = "macos") {
                SourceStatus::NotPresent
            } else {
                SourceStatus::Unsupported
            },
        )
    }
}

pub fn start(app_data: &Path) -> std::io::Result<()> {
    if shared::started() {
        return Ok(());
    }
    shared::sanitize::register_secret_values(std::env::vars_os().filter_map(|(name, value)| {
        let name = name.into_string().ok()?;
        let value = value.into_string().ok()?;
        secret_env_name(&name).then_some(value)
    }));
    shared::start(&dir_for(app_data))?;
    shared::install_panic_hook();
    shared::stderr::install()?;
    Ok(())
}

fn secret_env_name(name: &str) -> bool {
    let name = name.to_ascii_uppercase();
    if matches!(
        name.as_str(),
        "GOOGLE_APPLICATION_CREDENTIALS"
            | "LUCIDOS_TLS_KEY"
            | "LUCIDOS_PREVIEW_TLS_KEY"
            | "LUCIDOS_PREVIEW_USER_TLS_KEY"
    ) {
        return false;
    }
    name == "PGPASSWORD"
        || name == "DATABASE_URL"
        || name == "LUCIDOS_DATABASE_URL"
        || name.starts_with("CRED_")
        || name.starts_with("OAUTH_")
        || [
            "_TOKEN",
            "_SECRET",
            "_PASSWORD",
            "_PASSWD",
            "_KEY",
            "_PAT",
            "_CREDENTIALS",
        ]
        .iter()
        .any(|suffix| name.ends_with(suffix))
}

pub fn secret(value: String) -> String {
    shared::sanitize::register_secret_values([value.clone()]);
    value
}

pub fn typed(line: Line) {
    output::host(&line.render(Destination::Owner));
    shared::record("Gateway", line);
}

pub fn legacy(message: String) {
    output::host(&message);
    shared::record_legacy("Gateway", message);
}

pub fn boot_failure(message: &str) {
    shared::record(
        "Gateway",
        shared::line!("boot failed: {}", shared::private(message)),
    );
    shared::flush(std::time::Duration::from_secs(1));
    shared::stderr::host_error(message);
}

pub fn probe(value: std::fmt::Arguments<'_>) {
    output::probe(value);
}

#[cfg(test)]
pub fn test_notice(value: std::fmt::Arguments<'_>) {
    shared::stderr::host_error(&value.to_string());
}

#[allow(clippy::print_stdout)]
mod output {
    pub fn host(message: &str) {
        println!(
            "{} [pid:{}] {message}",
            chrono::Local::now().format(lucidos_gateway_constants::LOG_STAMP_FORMAT),
            std::process::id()
        );
    }

    pub fn probe(value: std::fmt::Arguments<'_>) {
        println!("{value}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn environment_credentials_are_distinct_from_paths() {
        for name in [
            "PGPASSWORD",
            "DATABASE_URL",
            "GH_TOKEN",
            "GATEWAY_TEST_SECRET",
            "CRED_EXAMPLE",
            "OAUTH_EXAMPLE",
            "LUCIDOS_DATABASE_URL",
        ] {
            assert!(secret_env_name(name), "{name}");
        }
        for name in [
            "HOME",
            "PATH",
            "LUCIDOS_TLS_KEY",
            "LUCIDOS_PREVIEW_TLS_KEY",
            "LUCIDOS_PREVIEW_USER_TLS_KEY",
            "GOOGLE_APPLICATION_CREDENTIALS",
        ] {
            assert!(!secret_env_name(name), "{name}");
        }
    }

    #[test]
    fn startup_claims_the_gateway_before_opening_its_sink() {
        let source = include_str!("server.rs");
        let run = &source[source.find("pub async fn run(").unwrap()..];
        let claim = run.find("claim_data_dir(&app_data").unwrap();
        let start = run
            .find("crate::diagnostic_log::start(&app_data)?;")
            .unwrap();
        let registry = run.find("let registry_path").unwrap();
        assert!(claim < start && start < registry);
    }

    #[test]
    fn log_is_scoped_to_the_resolved_gateway_data_dir() {
        assert_eq!(
            dir_for(Path::new("/srv/gateway")),
            Path::new("/srv/gateway/diagnostic-log")
        );
    }
}

#[cfg(test)]
mod query_tests {
    use super::*;
    use crate::server::GatewayState;
    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use axum::response::Response;
    use tower::ServiceExt as _;

    async fn request(state: &GatewayState, params: &str, token: Option<&str>) -> Response {
        let mut request = Request::get(format!("/~/api/v1/control/diagnostic-log?{params}"));
        if let Some(token) = token {
            request = request.header(crate::auth::HEADER_LOCAL_TOKEN, token);
        }
        crate::server::gateway_router(state.clone())
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    fn write(dir: &Path, msg: &str) {
        std::fs::create_dir_all(dir).unwrap();
        let record = serde_json::json!({
            "ts": (chrono::Utc::now() - chrono::Duration::seconds(1)).to_rfc3339(),
            "pid": 123, "label": "Test", "msg": msg,
        });
        std::fs::write(dir.join(shared::writer::FILE_NAME), format!("{record}\n")).unwrap();
    }

    fn write_client(dir: &Path) {
        shared::writer::Segments::open(&dir.join(shared::CLIENT_LIFECYCLE_DIR))
            .unwrap()
            .append_line("Relaunch", shared::line!("relaunch marker"))
            .unwrap();
    }

    #[tokio::test]
    async fn diagnostic_log_requires_the_local_token_and_refuses_a_path() {
        let root = tempfile::tempdir().unwrap();
        let state = GatewayState::for_tests_with_static_dir(Some(root.path().into()));
        for token in [None, Some("wrong-token")] {
            assert_eq!(
                request(&state, "source=all", token).await.status(),
                StatusCode::UNAUTHORIZED
            );
        }
        let token = Some(state.local_token());
        assert_eq!(
            request(&state, "path=/tmp/log", token).await.status(),
            StatusCode::BAD_REQUEST
        );
        write(&dir_for(state.app_data()), "gateway evidence");
        let response = request(&state, "source=gateway", token).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
        let answer: QueryResult = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(answer.sources[&Source::Gateway], SourceStatus::Ok);
        assert_eq!(answer.lines[0].msg, "gateway evidence");
    }

    #[tokio::test]
    async fn diagnostic_log_keeps_gateway_evidence_through_team_and_pairing_changes() {
        let root = tempfile::tempdir().unwrap();
        let state = GatewayState::for_tests_with_static_dir(Some(root.path().into()))
            .with_desktop_client_for_tests();
        for name in ["Owner", "Member"] {
            state
                .record(|directory| directory.add_user(name))
                .unwrap()
                .unwrap();
        }
        write(&dir_for(state.app_data()), "retained gateway evidence");
        write_client(state.app_data());

        async fn answer(state: &GatewayState) -> QueryResult {
            let response = request(state, "source=all", Some(state.local_token())).await;
            assert_eq!(response.status(), StatusCode::OK);
            let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
            let answer: QueryResult = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(answer.sources[&Source::Gateway], SourceStatus::Ok);
            assert!(answer
                .lines
                .iter()
                .any(|line| line.msg == "retained gateway evidence"));
            answer
        }

        let team = answer(&state).await;
        assert_eq!(team.sources[&Source::Client], SourceStatus::TeamGateway);
        assert!(team
            .lines
            .iter()
            .all(|line| line.source == Some(Source::Gateway)));

        let member = state.directory().users.pop().unwrap();
        state
            .record(|directory| directory.remove_user(&member.id))
            .unwrap()
            .unwrap();
        let personal = answer(&state).await;
        assert_eq!(personal.sources[&Source::Client], SourceStatus::Ok);
        assert!(personal
            .lines
            .iter()
            .any(|line| { line.source == Some(Source::Client) && line.msg == "relaunch marker" }));

        let pairings = state.paired_gateways();
        let secret = pairings
            .mint_secret(
                None,
                Default::default(),
                crate::pairing::Minter::LocalProcess,
            )
            .unwrap();
        let crate::gateway_protocol::GatewayRedemption::Paired { pairing_id, .. } = pairings
            .redeem(&secret, None, crate::pairing::OwnerGate::NoDevicePaired)
            .unwrap()
        else {
            panic!("the incoming gateway should be paired");
        };
        assert!(state.directory().relayed.is_empty());
        let paired = answer(&state).await;
        assert_eq!(paired.sources[&Source::Client], SourceStatus::TeamGateway);
        assert!(paired
            .lines
            .iter()
            .all(|line| line.source == Some(Source::Gateway)));
        assert!(pairings.unpair(&pairing_id).unwrap());
        assert_eq!(
            answer(&state).await.sources[&Source::Client],
            SourceStatus::Ok
        );
    }

    #[test]
    fn diagnostic_log_reads_lifecycle_only_in_its_own_desktop_data_dir() {
        let desktop = tempfile::tempdir().unwrap();
        let headless = tempfile::tempdir().unwrap();
        for name in [shared::CLIENT_LOG_DIR, shared::CLIENT_RELAUNCH_LOG_DIR] {
            write(&desktop.path().join(name), "raw process evidence");
        }
        let query = query::parse(Some("source=client"), chrono::Utc::now()).unwrap();
        let owns_desktop = |dir| owns_client_data(dir, Some(desktop.path()));
        assert!(owns_desktop(desktop.path()));
        assert!(!owns_client_data(desktop.path(), None));
        assert_eq!(
            query_client(&query, desktop.path(), owns_desktop(desktop.path())).sources
                [&Source::Client],
            SourceStatus::NotPresent
        );
        write_client(desktop.path());
        write_client(headless.path());
        let query = query::parse(Some("source=client"), chrono::Utc::now()).unwrap();
        let answer = query_client(&query, desktop.path(), owns_desktop(desktop.path()));
        assert_eq!(answer.returned, 1);
        assert_eq!(answer.sources[&Source::Client], SourceStatus::Ok);
        assert_eq!(answer.lines[0].source, Some(Source::Client));
        assert_eq!(answer.lines[0].msg, "relaunch marker");
        assert!(
            query_client(&query, headless.path(), owns_desktop(headless.path()))
                .lines
                .is_empty()
        );
    }

    #[test]
    fn diagnostic_log_refuses_a_symlinked_client_directory() {
        let root = tempfile::tempdir().unwrap();
        let outside = root.path().join("outside");
        write(&outside, "outside evidence");
        let client = root.path().join(shared::CLIENT_LIFECYCLE_DIR);
        std::os::unix::fs::symlink(&outside, client).unwrap();
        let query = query::parse(Some("source=client"), chrono::Utc::now()).unwrap();
        let answer = query_client(&query, root.path(), true);
        assert!(answer.lines.is_empty());
        assert_eq!(answer.sources[&Source::Client], SourceStatus::Unreadable);
    }
}
