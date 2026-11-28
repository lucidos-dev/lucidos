use super::*;

static SDK_BUNDLE: std::sync::OnceLock<String> = std::sync::OnceLock::new();

pub(super) async fn serve_sdk_js() -> Response {
    let sdk_js = if cfg!(debug_assertions) {
        find_sdk_bundle()
    } else {
        SDK_BUNDLE.get_or_init(find_sdk_bundle).clone()
    };
    ([(header::CONTENT_TYPE, "application/javascript")], sdk_js).into_response()
}

const SDK_IFRAME_BASE_CSS: &str = include_str!("sdk_iframe.css");

const SHARED_COMPONENTS_CSS: &str =
    include_str!("../../../lucidos-app/src/styles/global/shared-components.css");

const SURFACE_CSS: &str = include_str!("../../../lucidos-app/src/styles/global/surface.css");

const TEXT_INPUT_CSS: &str = include_str!("../../../lucidos-app/src/styles/global/text-input.css");

const SHAPES_CSS: &str = include_str!("sdk_iframe_shapes.css");

const CONTROL_THEME_PARTS_CSS: &str = include_str!("sdk_iframe_control_theme_parts.css");

fn sdk_iframe_css() -> &'static str {
    static CSS: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    CSS.get_or_init(|| {
        let theme_parts_css = crate::core::themes::parts::codegen::frame_css();
        format!(
            "{SDK_IFRAME_BASE_CSS}\n{SHARED_COMPONENTS_CSS}\n{SURFACE_CSS}\n{TEXT_INPUT_CSS}\n\
             {SHAPES_CSS}\n{theme_parts_css}\n{CONTROL_THEME_PARTS_CSS}"
        )
    })
}

pub(super) const SDK_IFRAME_AUDIO_JS: &str = include_str!("sdk_iframe_audio.js");

pub(super) async fn serve_sdk_iframe_css() -> Response {
    (
        [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
        sdk_iframe_css(),
    )
        .into_response()
}

/// INVARIANT: GET /api/v1/sse-worker.js: serve the shared SSE worker, one `EventSource`
/// per workspace, per browser profile.
///
/// A `SharedWorker` is keyed by its resolved script URL. This route sits under
/// the workspace's `/<slug>` prefix, so two workspaces get two workers and
/// neither can see the other's frames.
///
/// `no-cache` so a revalidation happens per load. A stale worker keeps relaying
/// frames correctly while quietly running the previous pong aggregation, which
/// is the failure hardest to notice.
pub(super) async fn serve_sse_worker_js() -> Response {
    static WORKER: BuiltSdkFile = BuiltSdkFile::new(SSE_WORKER_FILE);
    match WORKER.read() {
        Some(js) => (
            [
                (
                    header::CONTENT_TYPE,
                    "application/javascript; charset=utf-8",
                ),
                (header::CACHE_CONTROL, "no-cache"),
            ],
            js,
        )
            .into_response(),
        None => WORKER.missing(),
    }
}

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
    read_sdk_dist(SDK_JS_FILE).unwrap_or_else(|| {
        r#"(function(){
  console.warn('[Lucidos SDK] Built SDK bundle not found. Run: ./scripts/codegen.sh');
  window.lucidos = window.lucidos || {};
})();"#
            .to_string()
    })
}

const SDK_JS_FILE: &str = "sdk.js";
pub(super) const APPEARANCE_BOOT_FILE: &str = "appearance-boot.iframe.js";
const SSE_WORKER_FILE: &str = "sse-worker.js";

pub(super) struct BuiltSdkFile {
    name: &'static str,
    cached: std::sync::OnceLock<Option<String>>,
}

impl BuiltSdkFile {
    pub(super) const fn new(name: &'static str) -> Self {
        Self {
            name,
            cached: std::sync::OnceLock::new(),
        }
    }

    pub(super) fn read(&self) -> Option<String> {
        if cfg!(debug_assertions) {
            return read_sdk_dist(self.name);
        }
        self.cached.get_or_init(|| read_sdk_dist(self.name)).clone()
    }

    pub(super) fn missing(&self) -> Response {
        crate::log_legacy!(
            "[SDK] {} is not built, so it cannot be served. Run: ./scripts/codegen.sh",
            self.name
        );
        (
            StatusCode::SERVICE_UNAVAILABLE,
            format!("{} is not built. Run: ./scripts/codegen.sh\n", self.name),
        )
            .into_response()
    }
}

fn read_sdk_dist(name: &str) -> Option<String> {
    if let Some(dir) = std::env::var_os("LUCIDOS_SDK_DIR") {
        let path = std::path::Path::new(&dir).join(name);
        match std::fs::read_to_string(&path) {
            Ok(content) => return Some(content),
            // INVARIANT: LUCIDOS_SDK_DIR is set (packaged) but the file is missing or
            // unreadable: a real staging defect. Log it SERVER-side so it is
            // never invisible, then try the checkout layouts below.
            Err(e) => crate::log_legacy!(
                "[SDK] LUCIDOS_SDK_DIR is set but {} is unreadable: {e}",
                path.display()
            ),
        }
    }

    let rel = format!("packages/lucidos-sdk/dist/{name}");

    if let Ok(root) = crate::paths::repo_root() {
        if let Ok(content) = std::fs::read_to_string(root.join(&rel)) {
            return Some(content);
        }
    }

    let search_paths = [rel.clone(), format!("../{rel}"), format!("../../{rel}")];
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(std::path::Path::to_path_buf));
    search_paths
        .iter()
        .map(std::path::PathBuf::from)
        .chain(
            exe_dir
                .iter()
                .flat_map(|dir| search_paths.iter().map(move |p| dir.join(p))),
        )
        .find_map(|path| std::fs::read_to_string(path).ok())
}

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
    let mut payload = serde_json::Value::Object(payload);
    if let Err(e) = super::resolve_thread_id_in_nav_payload(&mut payload, None) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": e })),
        )
            .into_response();
    }
    if let Err(e) = super::resolve_repo_in_nav_payload(&state.pool, &mut payload).await {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": e })),
        )
            .into_response();
    }
    let payload = payload;
    log_legacy!(
        @sdk,
        "ui.navigate target={:?} app_id={:?} id={:?} (app-iframe, nil thread)",
        payload.get("target").and_then(|v| v.as_str()),
        payload.get("app_id").and_then(|v| v.as_str()),
        payload.get("id").and_then(|v| v.as_str())
    );

    let actor = super::actor::user_actor(&headers, None);
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
        log_legacy!(@sdk, "Failed to emit NavigationRequested: {}", e);
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("Failed to emit navigation event: {}", e) })),
        )
            .into_response();
    }

    Json(serde_json::json!({ "success": true })).into_response()
}

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/sdk.js", get(serve_sdk_js))
        .route("/sdk-iframe.css", get(serve_sdk_iframe_css))
        .route("/sdk-iframe-audio.js", get(serve_sdk_iframe_audio_js))
        .route("/sse-worker.js", get(serve_sse_worker_js))
        .route("/ui/navigate", post(ui_navigate))
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_script_that_is_not_built_is_a_503() {
        let file = super::BuiltSdkFile::new("not-built.js");
        assert!(file.read().is_none());
        assert_eq!(
            file.missing().status(),
            axum::http::StatusCode::SERVICE_UNAVAILABLE
        );
    }

    use super::*;

    #[test]
    fn the_served_stylesheet_carries_the_theme_part_rules() {
        let css = sdk_iframe_css();
        for needle in [
            "@property --accent {",
            "html[data-theme-effects=\"reduce\"] body {",
            "html[data-theme-parts=\"off\"] body {",
            "var(--part-app-text-text-shadow, none)",
        ] {
            assert!(css.contains(needle), "sdk-iframe.css lacks {needle}");
        }
    }

    #[test]
    fn iframe_body_is_sized_from_the_type_scale() {
        let rule = SDK_IFRAME_BASE_CSS
            .split("\nbody {")
            .nth(1)
            .and_then(|rest| rest.split('}').next())
            .expect("sdk_iframe.css must carry a top-level `body {` rule");
        assert!(
            rule.contains("font-size: var(--font-size-sm);"),
            "app <body> must default to the chat prose step, not the raw \
             root font-size and not an off-scale rem. Found:\n{rule}"
        );
    }

    #[test]
    fn iframe_page_never_scrolls_sideways() {
        let html = rule_body(SDK_IFRAME_BASE_CSS, "html")
            .expect("sdk_iframe.css must carry a top-level `html {` rule");
        assert!(
            html.contains("overflow-x: clip;"),
            "<html> must clip sideways. Found:\n{html}"
        );
        let body = rule_body(SDK_IFRAME_BASE_CSS, "body")
            .expect("sdk_iframe.css must carry a top-level `body {` rule");
        assert!(
            body.contains("overflow-x: clip;") && body.contains("max-width: 100%;"),
            "<body> must clip sideways and fit the page. Found:\n{body}"
        );
        assert!(
            !body.contains("overflow-x: hidden"),
            "<body> must not take `hidden`: it would become the page's scroller"
        );
    }

    #[test]
    fn iframe_controls_name_the_ui_font() {
        for selector in ["button", "input, textarea, select"] {
            let rule = rule_body(SDK_IFRAME_BASE_CSS, selector)
                .unwrap_or_else(|| panic!("sdk_iframe.css must carry a `{selector} {{` rule"));
            assert!(
                rule.contains("font-family: var(--font-ui);"),
                "`{selector}` must name --font-ui: a control inherits no family \
                 from body, so without it the app paints in the UA face. \
                 Found:\n{rule}"
            );
        }
    }

    #[test]
    fn the_control_theme_part_rule_outruns_the_shared_border_shorthands() {
        let css = sdk_iframe_css();
        let theme_part_pos = css
            .find(", .lucidos-select-trigger, .text-input {")
            .expect("control theme-part rule must be in the served stylesheet");
        for needle in ["\n.dropdown-trigger {", "\n.text-input,"] {
            let pos = css
                .find(needle)
                .unwrap_or_else(|| panic!("served stylesheet lacks {needle}"));
            assert!(
                pos < theme_part_pos,
                "{needle} must come BEFORE the control theme-part rule, or its \
                 border shorthand undoes the theme's tint"
            );
        }
    }

    #[test]
    fn the_widget_guidance_names_only_served_shapes() {
        use crate::engine::widget_guidance::{widgets_rules, WidgetAudience};
        let guidance = widgets_rules(WidgetAudience::Chat { automatic: true });
        let css = sdk_iframe_css();
        for class in ["ring", "bar-run", "choice-row"] {
            assert!(
                guidance.contains(&format!(".{class}")),
                "guidance lacks .{class}"
            );
            assert!(
                css.contains(&format!("\n.{class} {{")),
                "the guidance names .{class}, which the served stylesheet lacks"
            );
        }
    }

    fn rule_body<'a>(css: &'a str, selector: &str) -> Option<&'a str> {
        let needle = format!("\n{selector} {{");
        let mut from = 0;
        while let Some(rel) = css[from..].find(&needle) {
            let at = from + rel;
            if !css[..at].trim_end().ends_with(',') {
                return css[at + needle.len()..].split('}').next();
            }
            from = at + needle.len();
        }
        None
    }

    #[tokio::test]
    async fn an_app_navigate_refuses_the_current_thread_alias() {
        if !crate::test_support::in_engine_child(
            "api::sdk::tests::an_app_navigate_refuses_the_current_thread_alias",
        )
        .await
        {
            return;
        }
        let (_pool, db) = crate::test_support::setup_test_db().await;
        let (engine, workspace) = crate::test_support::boot_mock_engine(&db).await;
        let state = crate::api::permission_requests::tests::permission_test_state(
            engine,
            workspace.path().to_path_buf(),
        )
        .await;
        let body = super::NavigateRequest {
            target: "thread".to_string(),
            params: serde_json::json!({ "id": "current" }),
        };

        let response = super::ui_navigate(
            axum::extract::State(state),
            axum::http::HeaderMap::new(),
            axum::Json(body),
        )
        .await;

        assert_eq!(response.status(), axum::http::StatusCode::BAD_REQUEST);
        crate::test_support::teardown_test_db(&db).await;
    }

    mod guard_tests {
        #[test]
        fn served_sdk_files_are_staged() {
            let script = std::fs::read_to_string(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../scripts/lib/stage_runtime.sh"),
            )
            .expect("stage_runtime.sh exists");
            let list = script
                .lines()
                .find(|l| l.starts_with("STAGE_RUNTIME_SERVED_SDK_FILES=("))
                .expect("stage_runtime.sh lists the served SDK files");
            for name in [
                super::SDK_JS_FILE,
                super::APPEARANCE_BOOT_FILE,
                super::SSE_WORKER_FILE,
            ] {
                assert!(
                    list.split(['(', ' ', ')']).any(|w| w == name),
                    "release staging does not check {name}: {list}"
                );
            }
        }
    }
}
