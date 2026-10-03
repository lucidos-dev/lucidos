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

/// Iframe-specific CSS: design tokens (dark/light), element defaults, the themed
/// `lucidos.ui.Select`, and scrollbars.
/// Apps include via `<link rel="stylesheet" href="/api/v1/sdk-iframe.css">`.
/// Theme switching is driven by `lucidos.ui.applyPreferences()` setting
/// `data-theme-mode` on `<html>`.
const SDK_IFRAME_BASE_CSS: &str = include_str!("sdk_iframe.css");

/// Lucidos's shared component layer — the SINGLE SOURCE OF TRUTH, shared with
/// the host bundle. The host imports this exact file via `global.css`
/// (`@import './global/shared-components.css'`); the engine appends it to the
/// iframe CSS so apps render `.action-btn` / `.list-row` / `.markdown-content` /
/// … identically to the host shell with no copy to keep in sync. `include_str!`
/// bakes it into the engine binary at compile time (cross-crate path), so the
/// packaged build carries it with no runtime file dependency.
const SHARED_COMPONENTS_CSS: &str =
    include_str!("../../../lucidos-app/src/styles/global/shared-components.css");

/// The surface anatomy. `.surface-box` is the menu/popover box every host
/// dropdown and control panel uses. `lucidos.ui.Select`'s own menu wears it
/// too (`global/surface.css`). The rest of the file is dialog head/body/foot.
/// An app never uses it, the same as any unused class in
/// `SHARED_COMPONENTS_CSS`. See docs/plans/2026-10-03-sdk-dropdown-shares-host-css.md.
const SURFACE_CSS: &str = include_str!("../../../lucidos-app/src/styles/global/surface.css");

/// The host's text-field look (`.text-input`, opt-in). Apps get the same box,
/// placeholder colour and focus ring as every host text field, from one
/// source. See docs/plans/2026-10-03-sdk-dropdown-shares-host-css.md.
const TEXT_INPUT_CSS: &str = include_str!("../../../lucidos-app/src/styles/global/text-input.css");

/// The theme part rules for app frames, generated from the part catalog: the
/// `@property` rules for the frame colour tokens, the protected reset, the
/// `theme-effects` reduce rule and the app opt-out (ADR 0307).
const THEME_PARTS_CSS: &str =
    include_str!("../../../lucidos-app/src/styles/generated/theme-parts-frame.css");

/// The theme-part rules that tint app CONTROLS (`.lucidos-select-trigger` /
/// `.text-input` / bare `input`/`select`/`button` border-color + box-shadow).
/// Concatenated LAST. It names `.lucidos-select-trigger` and `.text-input`
/// directly, tying their specificity with those classes' own `border`
/// shorthand in the shared files. The later rule wins a tie, so this file
/// must come after them.
const CONTROL_THEME_PARTS_CSS: &str = include_str!("sdk_iframe_control_theme_parts.css");

/// The served `/api/v1/sdk-iframe.css` body: iframe tokens and defaults, the
/// shared component layers, then the theme part rules. Concatenated once and
/// cached. `CONTROL_THEME_PARTS_CSS` must stay last — see its own comment.
fn sdk_iframe_css() -> &'static str {
    static CSS: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    CSS.get_or_init(|| {
        format!(
            "{SDK_IFRAME_BASE_CSS}\n{SHARED_COMPONENTS_CSS}\n{SURFACE_CSS}\n{TEXT_INPUT_CSS}\n\
             {THEME_PARTS_CSS}\n{CONTROL_THEME_PARTS_CSS}"
        )
    })
}

/// Audio unlock shim — monkey-patches `AudioContext` so app code reuses a
/// shared, gesture-unlocked instance and survives iOS PWA background cycles.
/// Must run before any app script creates an `AudioContext`, so apps include
/// it via `<script src="/api/v1/sdk-iframe-audio.js"></script>` early in `<head>`.
pub(super) const SDK_IFRAME_AUDIO_JS: &str = include_str!("sdk_iframe_audio.js");

/// The shared SSE worker: one `EventSource` per workspace, per browser profile.
///
/// A `SharedWorker` is keyed by its resolved script URL. This route sits under
/// the workspace's `/<slug>` prefix, so two workspaces get two workers and
/// neither can see the other's frames.
///
/// `include_str!` of a CHECKED-IN esbuild artifact, like the appearance boot
/// bundle beside it, so `cargo build` needs no prior npm run.
/// `sseWorker.staleness.test.ts` fails if the committed bundle drifts from its
/// TypeScript source.
const SSE_WORKER_JS: &str =
    include_str!("../../../../packages/lucidos-sdk/src/generated/sse-worker.js");

/// GET /api/v1/sdk-iframe.css — serve the iframe stylesheet (iframe tokens/
/// defaults + the shared component layer).
pub(super) async fn serve_sdk_iframe_css() -> Response {
    (
        [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
        sdk_iframe_css(),
    )
        .into_response()
}

/// GET /api/v1/sse-worker.js: serve the shared SSE worker.
///
/// `no-cache` so a revalidation happens per load. A stale worker keeps relaying
/// frames correctly while quietly running the previous pong aggregation, which
/// is the failure hardest to notice.
pub(super) async fn serve_sse_worker_js() -> Response {
    (
        [
            (
                header::CONTENT_TYPE,
                "application/javascript; charset=utf-8",
            ),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        SSE_WORKER_JS,
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
    // Packaged desktop bundle: the launcher sets LUCIDOS_SDK_DIR to the staged
    // SDK resource dir (which contains sdk.js). Checked first so the bundle
    // doesn't depend on cwd / exe-relative layout. No-op when unset (dev/docker).
    if let Some(dir) = std::env::var_os("LUCIDOS_SDK_DIR") {
        let path = std::path::Path::new(&dir).join("sdk.js");
        match std::fs::read_to_string(&path) {
            Ok(content) => return content,
            // LUCIDOS_SDK_DIR is set (packaged) but the bundle is missing /
            // unreadable — a real staging defect. Log a SERVER-side error so it
            // isn't invisible (apps would otherwise silently lose
            // `window.lucidos.*` with only a browser-console warning from the
            // stub below). We still fall through to the stub so app pages load.
            Err(e) => crate::log!(
                "[SDK] LUCIDOS_SDK_DIR is set but {} is unreadable: {} — serving the SDK stub; \
                 apps will lose window.lucidos.*",
                path.display(),
                e
            ),
        }
    }

    const SDK_REL: &str = "packages/lucidos-sdk/dist/sdk.js";

    // Dev, resolved from the CHECKOUT rather than from a fixed number of `..`
    // hops above the binary. `paths::repo_root` walks `current_exe()`'s ancestors
    // for `scripts/web-dev.sh`, so it is independent of how deep the engine binary
    // sits and of which top-level directory holds it. That matters because the dev
    // launcher publishes it to `.launch/<profile>/<variant>/` (ADR 0022 + 0063), so
    // the exe-relative `../../` fallback below bottoms out at `<repo>/.launch` and
    // still cannot see the checkout root. The gateway spawns engines with cwd = the
    // WORKSPACE dir, so the cwd-relative reads never hit in the normal dev topology
    // and this is the branch that actually serves the bundle.
    if let Ok(root) = crate::paths::repo_root() {
        if let Ok(content) = std::fs::read_to_string(root.join(SDK_REL)) {
            return content;
        }
    }

    // cwd-relative (a directly-launched engine runs with cwd = the checkout) and
    // exe-relative, kept as fallbacks for layouts `repo_root` can't resolve.
    let search_paths = [
        SDK_REL,
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
    let mut payload = serde_json::Value::Object(payload);
    // An app writes this id, and the page dereferences it with no way to ask
    // what was meant. `None` for the caller: an app iframe has no thread of its
    // own, as the nil-thread emit below says. So the `current` alias is refused
    // here rather than resolved to whichever thread we happen to serve.
    if let Err(e) = super::resolve_thread_id_in_nav_payload(&mut payload, None) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": e })),
        )
            .into_response();
    }
    let payload = payload;
    log!(
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
        log!(@sdk, "Failed to emit NavigationRequested: {}", e);
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("Failed to emit navigation event: {}", e) })),
        )
            .into_response();
    }

    Json(serde_json::json!({ "success": true })).into_response()
}

/// Routes for the SDK static assets and the `/ui/navigate` SDK bridge.
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
    use super::*;

    /// An app that loads this stylesheet gets typed colour tokens and every
    /// part reset. A theme's parts then paint there as in the shell, and
    /// `data-theme-parts="off"` switches them off (ADR 0307).
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

    /// An app's `<body>` must carry the chat prose step, `--font-size-sm`, so
    /// unsized app text reads at the size of the chat beside it (ADR 0319).
    /// Two ways to get this wrong, and the assert below covers both. Leave the
    /// declaration off and unstyled app text falls to the raw root font-size
    /// (`1rem`), which is `--font-size-xl`, a SECTION HEADING. Write a raw `rem`
    /// and it ships an off-scale size to every app, against the closed-set rule
    /// the host shell follows (`.claude/rules/frontend-css.md`).
    ///
    /// The host's `body` in `styles/global/base.css` stays on `--font-size-md`,
    /// the host's step for single-line UI. See `docs/code-review-priors.md` for
    /// the history, and `styles/__tests__/text-defaults-guard.test.ts` for the
    /// frontend twin of this assert.
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

    /// A control inherits NOTHING from `body`: the UA stylesheet applies the
    /// `font` shorthand to it, which resets the family too. `input, textarea,
    /// select` always named `--font-ui` here; `button` did not, so an app's
    /// buttons painted in the system UI face while its own inputs painted in
    /// the workspace font, inside the same app. Same defect the host shell had
    /// at `.welcome-dismiss`, found by `e2e/type-scale.spec.ts` on 2026-08-13.
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

    /// The control theme-part rule names `.lucidos-select-trigger` and
    /// `.text-input` directly, tying its specificity with those classes' own
    /// `border` shorthand (`.dropdown-trigger` and `.text-input`, one class
    /// each). A tie goes to whichever rule is later. So the theme-part rule
    /// must come after both, or the tie-break picks the shorthand instead of
    /// the tint (ADR 0307). This is the regression `CONTROL_THEME_PARTS_CSS`
    /// exists to prevent — see its own doc comment.
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

    /// Body of the top-level rule whose selector list is exactly `selector`.
    ///
    /// The naive `split("\nbutton {")` finds the wrong rule: `button` is also
    /// the LAST member of the grouped `html, input, textarea, select, button`
    /// font-feature-settings rule, whose final selector line reads exactly that
    /// way. So a candidate is rejected only when what precedes it CONTINUES a
    /// selector list, which is to say ends in a comma.
    ///
    /// Testing for a comma rather than for a preceding `}` is the load-bearing
    /// choice: a rule in this sheet is very often preceded by a comment, so a
    /// `}`-test would reject the real rule the moment anybody documented it,
    /// and fail with "must carry a rule" about a rule sitting right there.
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
}
