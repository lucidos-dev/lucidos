use crate::{pairing, server::GatewayState};
use axum::{
    extract::{Path, Request, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde_json::Value;
use std::{
    io::Read,
    path::{Path as FsPath, PathBuf},
};
use tower::ServiceExt;

pub fn router() -> Router<GatewayState> {
    Router::new()
        .route("/latest.json", get(feed))
        .route("/:bundle", get(bundle))
}

fn read_staged(dir: &FsPath) -> Result<Value, String> {
    let file =
        std::fs::File::open(dir.join("latest.json")).map_err(|e| format!("Local feed: {e}"))?;
    let mut body = String::new();
    file.take((crate::release_check::MAX_BODY_BYTES + 1) as u64)
        .read_to_string(&mut body)
        .map_err(|e| format!("Local feed: {e}"))?;
    if body.len() > crate::release_check::MAX_BODY_BYTES {
        return Err("Local feed exceeds 64 KiB".into());
    }
    serde_json::from_str(&body).map_err(|e| format!("Local feed: {e}"))
}

fn bundle_name(feed: &Value) -> Result<&str, String> {
    let platforms = feed["platforms"]
        .as_object()
        .ok_or("Local feed has no platforms")?;
    if platforms.len() != 1 {
        return Err("Local feed must name exactly one macOS bundle".into());
    }
    let entry = platforms
        .get("darwin-aarch64")
        .ok_or("Local feed needs darwin-aarch64")?;
    let url = entry["url"]
        .as_str()
        .ok_or("Local feed has no bundle URL")?;
    let name = url
        .rsplit('/')
        .next()
        .ok_or("Local feed has no bundle name")?;
    if name.is_empty()
        || name.starts_with('.')
        || name.contains("..")
        || !name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
    {
        return Err("Local feed has an invalid bundle name".into());
    }
    Ok(name)
}

fn stored_bundle(dir: &FsPath, name: &str) -> Result<PathBuf, String> {
    let root = dir
        .canonicalize()
        .map_err(|e| format!("Local directory: {e}"))?;
    let path = dir
        .join(name)
        .canonicalize()
        .map_err(|e| format!("Local bundle: {e}"))?;
    if path.parent() != Some(root.as_path()) || !path.is_file() {
        return Err("Local bundle must be a file inside the Local directory".into());
    }
    Ok(path)
}

pub(crate) fn read_feed(dir: &FsPath, origin: &str) -> Result<Value, String> {
    let mut feed = read_staged(dir)?;
    let name = bundle_name(&feed)?.to_owned();
    stored_bundle(dir, &name)?;
    feed["platforms"]["darwin-aarch64"]["url"] = format!("{origin}/~/updates/local/{name}").into();
    crate::release_check::parse_feed(&feed.to_string(), "macos", "aarch64")?;
    Ok(feed)
}

fn authorize(req: &Request) -> Result<(), StatusCode> {
    let local = req
        .extensions()
        .get::<pairing::Peer>()
        .is_some_and(|p| !p.public_listener && p.remote.ip().to_canonical().is_loopback());
    if !local
        || pairing::is_forwarded(req.headers())
        || req
            .extensions()
            .get::<crate::auth::AuthenticatedLocalProcess>()
            .is_none()
    {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(())
}

async fn feed(State(state): State<GatewayState>, req: Request) -> Response {
    if let Err(status) = authorize(&req) {
        return status.into_response();
    }
    let peer = req
        .extensions()
        .get::<pairing::Peer>()
        .expect("authorized peer");
    let scheme = match lucidos_gateway_tls::Connection::of(req.extensions()) {
        lucidos_gateway_tls::Connection::Tls => crate::net_config::SCHEME_HTTPS,
        lucidos_gateway_tls::Connection::Plain
        | lucidos_gateway_tls::Connection::PlainWithHttps { .. }
        | lucidos_gateway_tls::Connection::PlainLoopback => crate::net_config::SCHEME_HTTP,
    };
    let local = std::net::SocketAddr::new(peer.local.ip().to_canonical(), peer.local.port());
    let origin = format!("{scheme}://{local}");
    match state
        .release_check()
        .local_channel_dir()
        .and_then(|dir| read_feed(&dir, &origin))
    {
        Ok(feed) => Json(feed).into_response(),
        Err(e) => (StatusCode::SERVICE_UNAVAILABLE, e).into_response(),
    }
}

async fn bundle(
    State(state): State<GatewayState>,
    Path(requested): Path<String>,
    req: Request,
) -> Response {
    if let Err(status) = authorize(&req) {
        return status.into_response();
    }
    let path = state.release_check().local_channel_dir().and_then(|dir| {
        let feed = read_staged(&dir)?;
        let name = bundle_name(&feed)?;
        if requested != name {
            return Err("bundle is not named by the Local feed".into());
        }
        stored_bundle(&dir, name)
    });
    match path {
        Ok(path) => match tower_http::services::ServeFile::new(path)
            .oneshot(req)
            .await
        {
            Ok(mut response) => {
                response.headers_mut().insert(
                    header::CACHE_CONTROL,
                    "no-store".parse().expect("static header"),
                );
                response.into_response()
            }
            Err(e) => match e {},
        },
        Err(e) => (StatusCode::NOT_FOUND, e).into_response(),
    }
}

#[cfg(test)]
#[path = "local_updates_tests.rs"]
mod tests;
