use super::*;

/// Cached SDK bundle — loaded once on first request, reused thereafter.
/// In debug builds, reads from disk every time for hot-reload convenience.
static SDK_BUNDLE: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// GET /api/v1/sdk.js — serve the pre-built SDK bundle.
pub(super) async fn serve_sdk_js() -> Response {
    let sdk_js = if cfg!(debug_assertions) {
        // Dev mode: read from disk every time for hot-reload
        find_sdk_bundle()
    } else {
        SDK_BUNDLE.get_or_init(find_sdk_bundle).clone()
    };
    ([(header::CONTENT_TYPE, "application/javascript")], sdk_js).into_response()
}

/// Lucidos iframe theme stylesheet — design tokens (dark/light), defaults, scrollbars.
/// Apps include via `<link rel="stylesheet" href="/api/v1/sdk-iframe.css">`.
/// Theme switching is driven by `lucidos.ui.applyPreferences()` setting
/// `data-theme` on `<html>`.
pub(super) const SDK_IFRAME_CSS: &str = include_str!("sdk_iframe.css");

/// Audio unlock shim — monkey-patches `AudioContext` so app code reuses a
/// shared, gesture-unlocked instance and survives iOS PWA background cycles.
/// Must run before any app script creates an `AudioContext`, so apps include
/// it via `<script src="/api/v1/sdk-iframe-audio.js"></script>` early in `<head>`.
pub(super) const SDK_IFRAME_AUDIO_JS: &str = include_str!("sdk_iframe_audio.js");

/// GET /api/v1/sdk-iframe.css — serve the iframe theme stylesheet.
pub(super) async fn serve_sdk_iframe_css() -> Response {
    (
        [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
        SDK_IFRAME_CSS,
    )
        .into_response()
}

/// GET /api/v1/sdk-iframe-audio.js — serve the audio unlock shim.
pub(super) async fn serve_sdk_iframe_audio_js() -> Response {
    (
        [(
            header::CONTENT_TYPE,
            "application/javascript; charset=utf-8",
        )],
        SDK_IFRAME_AUDIO_JS,
    )
        .into_response()
}

fn find_sdk_bundle() -> String {
    let search_paths = [
        "packages/lucidos-sdk/dist/sdk.js",
        "../packages/lucidos-sdk/dist/sdk.js",
        "../../packages/lucidos-sdk/dist/sdk.js",
    ];

    for path in &search_paths {
        if let Ok(content) = std::fs::read_to_string(path) {
            return content;
        }
    }

    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            for path in &search_paths {
                let full_path = exe_dir.join(path);
                if let Ok(content) = std::fs::read_to_string(&full_path) {
                    return content;
                }
            }
        }
    }

    // Fallback: minimal SDK stub that logs a warning
    r#"(function(){
  console.warn('[Lucidos SDK] Built SDK bundle not found. Run: cd packages/lucidos-sdk && npm run build');
  window.lucidos = window.lucidos || {};
})();"#.to_string()
}

/// POST /api/v1/ui/navigate — emit a NavigationRequested event via EventBus.
#[derive(Deserialize)]
pub(super) struct NavigateRequest {
    pub target: String,
    #[serde(default)]
    pub params: serde_json::Value,
}

pub(super) async fn ui_navigate(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<NavigateRequest>,
) -> Response {
    if body.target.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "target is required" })),
        )
            .into_response();
    }

    let mut payload = serde_json::Map::new();
    payload.insert("target".to_string(), serde_json::Value::String(body.target));
    if let Some(params) = body.params.as_object() {
        for (k, v) in params {
            payload.insert(k.clone(), v.clone());
        }
    }
    let payload = serde_json::Value::Object(payload);

    let actor = super::actor::user_actor_resolved(&headers, &state.pool, None).await;
    if let Err(e) = state
        .engine
        .event_bus
        .emit(crate::engine::event_bus::BusEvent::Thread {
            thread_id: uuid::Uuid::nil(),
            event: crate::engine::thread_events::ThreadEvent::NavigationRequested {
                payload: payload.to_string(),
            },
            meta: crate::engine::thread_events::EventMeta::with_actor(actor),
        })
        .await
    {
        log!(@sdk, "Failed to emit NavigationRequested: {}", e);
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("Failed to emit navigation event: {}", e) })),
        )
            .into_response();
    }

    Json(serde_json::json!({ "success": true })).into_response()
}
