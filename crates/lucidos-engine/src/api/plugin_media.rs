//! The *plugin media* route (ADR 0414). It serves only what a media copy's
//! index lists (invariant I13), and never as a document: every response carries
//! `nosniff` and a sandbox CSP, so opening an SVG directly cannot run script on
//! the engine origin (invariant I6). The Lucidos shell alone reaches it
//! (`app_reach`, invariant I17).

use std::path::Path;

use axum::extract::{Path as UrlPath, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;

use super::AppState;
use crate::core::plugin_media::{self, is_folder_name};

/// No script, no subresource, no same-origin access, whatever the file is.
pub(super) const MEDIA_CSP: &str = "sandbox; default-src 'none'";

/// The paths are literals because the `app_reach` scan reads them from this
/// source. A test pins them to the mounts the catalog URLs are built from.
pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/plugins/media/installed/:plugin_id/*path",
            get(installed_media),
        )
        .route(
            "/plugins/media/catalog/:marketplace_id/:plugin_id/:version/*path",
            get(catalog_media),
        )
}

/// `GET /api/v1/plugins/media/installed/:plugin_id/*path`: an installed
/// plugin's media, which outlives its marketplace.
async fn installed_media(
    State(state): State<AppState>,
    UrlPath((plugin_id, path)): UrlPath<(String, String)>,
    headers: HeaderMap,
) -> Response {
    if !is_folder_name(&plugin_id) {
        return media_headers(not_found());
    }
    let copy = plugin_media::installed_dir(&state.workspace_path, &plugin_id);
    media_response(&copy, &path, &headers).await
}

/// `GET /api/v1/plugins/media/catalog/:marketplace_id/:plugin_id/:version/*path`:
/// the media a scan cached for one catalog row.
async fn catalog_media(
    State(state): State<AppState>,
    UrlPath((marketplace_id, plugin_id, version, path)): UrlPath<(String, String, String, String)>,
    headers: HeaderMap,
) -> Response {
    if ![&marketplace_id, &plugin_id, &version]
        .iter()
        .all(|s| is_folder_name(s))
    {
        return media_headers(not_found());
    }
    let copy =
        plugin_media::cache_dir(&state.workspace_path, &marketplace_id, &plugin_id, &version);
    media_response(&copy, &path, &headers).await
}

/// Serve `path` from the media copy at `copy`, when its index lists it.
pub(super) async fn media_response(copy: &Path, path: &str, headers: &HeaderMap) -> Response {
    let Some(file) = plugin_media::listed_file(copy, path) else {
        return media_headers(not_found());
    };
    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    let response =
        super::file_response::serve_file(&file, super::content_type_for_ext(&ext), headers).await;
    media_headers(response)
}

fn not_found() -> Response {
    (StatusCode::NOT_FOUND, "Not a listed plugin media file").into_response()
}

fn media_headers(mut response: Response) -> Response {
    let headers = response.headers_mut();
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(MEDIA_CSP),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::plugin_media::{installed_dir, resolve, write_copy, MediaDeclaration};

    fn copy_with(
        files: &[(&str, &[u8])],
        declared: &str,
    ) -> (tempfile::TempDir, std::path::PathBuf) {
        let tree = tempfile::tempdir().unwrap();
        for (rel, bytes) in files {
            let path = tree.path().join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, bytes).unwrap();
        }
        let table: toml::Table = toml::from_str(declared).unwrap();
        let media = resolve(tree.path(), &MediaDeclaration::from_manifest(&table));
        let workspace = tempfile::tempdir().unwrap();
        let copy = installed_dir(workspace.path(), "p");
        write_copy(&media, &copy).unwrap();
        (workspace, copy)
    }

    fn assert_media_headers(response: &Response) {
        let h = response.headers();
        assert_eq!(h.get(header::X_CONTENT_TYPE_OPTIONS).unwrap(), "nosniff");
        assert_eq!(h.get(header::CONTENT_SECURITY_POLICY).unwrap(), MEDIA_CSP);
    }

    /// Invariant I6: every media response, found or not, carries both headers.
    #[tokio::test]
    async fn every_media_response_carries_the_sandbox_headers() {
        let (_ws, copy) = copy_with(
            &[("media/icon.svg", b"<svg onload=\"alert(1)\"/>")],
            "icon = \"media/icon.svg\"",
        );
        let found = media_response(&copy, "media/icon.svg", &HeaderMap::new()).await;
        assert_eq!(found.status(), StatusCode::OK);
        assert_eq!(
            found.headers().get(header::CONTENT_TYPE).unwrap(),
            "image/svg+xml"
        );
        assert_media_headers(&found);

        let missing = media_response(&copy, "media/nope.svg", &HeaderMap::new()).await;
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
        assert_media_headers(&missing);
    }

    /// Invariant I13: a file in the copy that the index does not list is a 404.
    #[tokio::test]
    async fn an_unlisted_file_is_not_served() {
        let (_ws, copy) = copy_with(
            &[("media/a.png", b"png")],
            "screenshots = [\"media/a.png\"]",
        );
        std::fs::write(copy.join("files/media/page.html"), "<script>").unwrap();
        let response = media_response(&copy, "media/page.html", &HeaderMap::new()).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let response = media_response(&copy, "../p/index.json", &HeaderMap::new()).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    /// A video seeks, which Safari needs before it plays at all.
    #[tokio::test]
    async fn a_video_answers_a_range() {
        let (_ws, copy) = copy_with(
            &[("media/v.mp4", b"0123456789")],
            "videos = [\"media/v.mp4\"]",
        );
        let mut headers = HeaderMap::new();
        headers.insert(header::RANGE, HeaderValue::from_static("bytes=2-5"));
        let response = media_response(&copy, "media/v.mp4", &headers).await;
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        assert_media_headers(&response);
    }

    /// The reach table lists every route the router serves, so a media row
    /// there under each mount means the catalog URLs reach a handler.
    #[test]
    fn the_routes_mount_where_the_catalog_urls_point() {
        for mount in [plugin_media::INSTALLED_ROUTE, plugin_media::CATALOG_ROUTE] {
            let prefix = format!("{mount}/");
            assert!(
                crate::api::app_reach::ROUTE_REACH
                    .iter()
                    .any(|(path, _, _)| path.starts_with(&prefix)),
                "no route under {mount}"
            );
        }
    }

    #[test]
    fn a_folder_name_is_one_plain_segment() {
        for bad in ["", ".", "..", "a/b", "a\\b"] {
            assert!(!is_folder_name(bad), "{bad:?}");
        }
        assert!(is_folder_name("1.0.0+build"));
    }
}
