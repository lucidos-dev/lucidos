//! Per-iframe synchronous prefs script — `GET /api/v1/sdk-prefs.js`.
//!
//! Apps opt into FOUC-free theme by adding the script tag *before* their
//! stylesheet:
//!
//! ```html
//! <script src="/api/v1/sdk-prefs.js"></script>
//! <link rel="stylesheet" href="/api/v1/sdk-iframe.css">
//! ```
//!
//! The script sets `data-theme-mode` (and, for app styles written before the
//! rename, the legacy `data-theme`), `--bg-primary`, `--font-ui` (and
//! `--user-ui-scale` when set) on `<html>` synchronously, so first paint
//! matches the user's preferences before any subsequent stylesheet evaluates.
//! The seed it carries also holds the device's `autocorrect` switch, which the
//! SDK's field stamp reads before its own preference read returns.
//!
//! **Where the values come from.** A same-origin iframe inherits the parent's
//! localStorage, so the shell's mirror writes are visible to it. An ISOLATED
//! frame is not same-origin and sees none of them, and this script is
//! parser-blocking, so nothing async can feed it either. So the engine resolves
//! the values and prepends them, and storage is the fallback behind them.
//!
//! `?device=` says whose. `api/app_ui.rs`'s `stamp_prefs_device` puts it on the
//! app's own reference to this route, which is the only edit the app document
//! takes. Without it the global preferences answer, and the body stays the
//! static bundle every caller shares.
//!
//! These keys are PER-WORKSPACE (`crates/lucidos-app/src/utils/workspaceStorage.ts`):
//! the parent writes them under `ws:<slug>:<key>`. This script runs in the iframe
//! realm, which the parent's `Storage.prototype` override does NOT reach, so it
//! derives the workspace slug itself from `location.pathname` (the app iframe
//! loads at `/<slug>/app/<id>/…`; mirrors `packages/lucidos-sdk/src/_storage.ts`)
//! and reads the namespaced keys — or the parent's write wouldn't match and the
//! iframe would FOUC. Direct access (`/app/<id>/`) → no slug → raw key.
//!
//! The SDK's `lucidos.ui.applyPreferences()` continues to handle live SSE
//! updates; it just overwrites the values this script set.

use super::*;

use crate::core::PreferenceStore;

/// The appearance FOUC script, built from `packages/lucidos-sdk/src/boot/` and
/// checked in. The app shell inlines the sibling `host` bundle into its own
/// `<head>`, so the two documents run ONE program: they used to run two
/// hand-copied ones, held together by a comment asking the next editor to keep
/// them in sync.
///
/// `include_str!` bakes it into the binary at compile time (cross-crate path,
/// same as `api/sdk.rs` does with the app's shared component CSS), so the
/// packaged build carries it with no runtime file dependency and `cargo build`
/// never needs npm to have run. A staleness test in the SDK package fails if
/// the committed bundle no longer matches its source.
const SDK_PREFS_JS: &str =
    include_str!("../../../../packages/lucidos-sdk/src/generated/appearance-boot.iframe.js");

/// The preference keys an app frame needs before anything async can answer, in
/// the order the SDK resolves them. The appearance keys paint the first frame.
/// `text-size` and `font-size` are the pre-grid aliases for `ui-scale`, carried
/// so this script and the live `ui.applyPreferences` pick the same scale.
/// `autocorrect` is read by the SDK's field stamp, which must answer before a
/// field's first focus (ADR 0262). `motion` resolves `data-motion` before the
/// first frame can animate. `theme-effects` sets `data-theme-effects` before a
/// theme's glow can paint. [`THEME_RESOLVED_SEED_KEY`] carries the active
/// theme's resolved token maps, which the engine alone can compute.
/// [`WORKSPACE_FONT_SEED_KEY`] carries the workspace font `font-family` names.
const SEED_KEYS: [&str; 11] = [
    "theme-mode",
    "font-family",
    "ui-scale",
    "text-size",
    "font-size",
    crate::core::themes::STYLE_OVERRIDES_KEY,
    "autocorrect",
    "motion",
    "theme-effects",
    THEME_RESOLVED_SEED_KEY,
    WORKSPACE_FONT_SEED_KEY,
];

/// Not a preference: the resolved maps of the device's `theme`, as JSON. The
/// boot script reads it under this name (`boot/appearanceBoot.ts`).
const THEME_RESOLVED_SEED_KEY: &str = "theme_resolved";

/// Not a preference: the workspace font the device's `font-family` names, as
/// JSON, so the boot script can register its faces (ADR 0308). The theme's own
/// workspace fonts ride inside [`THEME_RESOLVED_SEED_KEY`].
const WORKSPACE_FONT_SEED_KEY: &str = "workspace_font";

/// The global the seed lands on, read by `boot/appearanceBoot.ts` and the SDK's
/// `autocorrectStamp.ts`.
const SEED_GLOBAL: &str = "__lucidosPrefs";

/// Query for the prefs script: which device is asking.
#[derive(Debug, Deserialize)]
pub(super) struct SdkPrefsQuery {
    device: Option<String>,
}

/// GET /api/v1/sdk-prefs.js, the synchronous first-paint appearance script.
pub(super) async fn serve_sdk_prefs_js(
    State(state): State<AppState>,
    Query(query): Query<SdkPrefsQuery>,
) -> Response {
    let Some(device) = query.device.as_deref() else {
        // No device named, so the body is the bundle every caller shares and
        // stays cacheable exactly as it was.
        return (
            [
                (
                    header::CONTENT_TYPE,
                    "application/javascript; charset=utf-8",
                ),
                (header::CACHE_CONTROL, "public, max-age=300"),
            ],
            SDK_PREFS_JS,
        )
            .into_response();
    };

    // A failed read degrades rather than propagating, and that is deliberate.
    // Answering 500 would leave the app with no appearance script at all, where
    // returning nothing costs one frame at the default that
    // `ui.applyPreferences` then corrects. The engine log names it either way.
    let mut prefs = match PreferenceStore::get_all_for_device(&state.pool, device).await {
        Ok(prefs) => prefs,
        Err(e) => {
            log!("[SdkPrefs] first-paint seed unavailable, the app corrects itself: {e}");
            Default::default()
        }
    };
    let data_dir = state.workspace_path.join(crate::core::DATA_DIR);
    with_resolved_theme(&mut prefs, &data_dir);
    with_workspace_font(&mut prefs, &data_dir);

    (
        [
            (
                header::CONTENT_TYPE,
                "application/javascript; charset=utf-8",
            ),
            // The preferences are IN the body now, so caching it caches them:
            // change your theme and the next app open would paint the old one.
            // That flash is what this route exists to remove.
            (header::CACHE_CONTROL, "no-store"),
        ],
        format!("{}{}", seed_line(&prefs), SDK_PREFS_JS),
    )
        .into_response()
}

/// Replace whatever sits under [`THEME_RESOLVED_SEED_KEY`] with the resolved
/// maps of the `theme` preference, or nothing for the default theme. Replaced
/// rather than trusted, because any caller can write a preference by that name.
fn with_resolved_theme(
    prefs: &mut std::collections::HashMap<String, String>,
    data_dir: &std::path::Path,
) {
    prefs.remove(THEME_RESOLVED_SEED_KEY);
    let resolved = prefs
        .get("theme")
        .and_then(|id| crate::core::themes::resolved_json_for_preference(data_dir, id));
    if let Some(json) = resolved {
        prefs.insert(THEME_RESOLVED_SEED_KEY.to_string(), json);
    }
}

/// Replace whatever sits under [`WORKSPACE_FONT_SEED_KEY`] with the workspace
/// font the `font-family` preference names. A catalog font or a font that is
/// not installed seeds nothing.
fn with_workspace_font(
    prefs: &mut std::collections::HashMap<String, String>,
    data_dir: &std::path::Path,
) {
    use crate::core::workspace_fonts;
    prefs.remove(WORKSPACE_FONT_SEED_KEY);
    let Some(id) = prefs
        .get("font-family")
        .filter(|id| workspace_fonts::is_workspace_id(id))
    else {
        return;
    };
    let entry = workspace_fonts::list(data_dir)
        .find(id)
        .and_then(|font| serde_json::to_string(font).ok());
    if let Some(json) = entry {
        prefs.insert(WORKSPACE_FONT_SEED_KEY.to_string(), json);
    }
}

/// The `window.__lucidosPrefs = {…};` line prepended to the bundle, or an empty
/// string when nothing is stored.
///
/// A BTreeMap so the output is ordered, which lets a test compare the whole
/// line rather than parse it back.
fn seed_line(prefs: &std::collections::HashMap<String, String>) -> String {
    let seed: std::collections::BTreeMap<&str, &str> = SEED_KEYS
        .iter()
        .filter_map(|key| prefs.get(*key).map(|value| (*key, value.as_str())))
        .collect();
    if seed.is_empty() {
        return String::new();
    }
    match serde_json::to_string(&seed) {
        Ok(json) => format!("window.{}={};\n", SEED_GLOBAL, escape_for_script(&json)),
        Err(_) => String::new(),
    }
}

/// Make a JSON document safe to serve as a JavaScript body.
///
/// U+2028 and U+2029 are legal in JSON and were JavaScript line terminators
/// before ES2019. `style_overrides` is a map any app may write through
/// `lucidos.preferences.set`, so its values are not ours to trust. Escaping
/// both costs nothing and removes the question.
fn escape_for_script(json: &str) -> String {
    json.replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029")
}

/// Route for the `/sdk-prefs.js` asset.
pub(super) fn router() -> Router<AppState> {
    Router::new().route("/sdk-prefs.js", get(serve_sdk_prefs_js))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The VALUES this script applies (the theme default, the font stacks, the
    /// ligature pairs, the scale grid, the style-override rules) are asserted
    /// once, at their source, in `packages/lucidos-sdk/src/appearance.test.ts`.
    /// Re-asserting them here would be re-checking a copy that no longer exists,
    /// and it would pin esbuild's output formatting instead of behaviour.
    ///
    /// What is left is the ENGINE's side of the contract: that the artifact it
    /// serves is the boot script, is complete, is the IFRAME build, and still
    /// reads the workspace-scoped keys the shell writes.

    #[test]
    fn serves_the_generated_boot_bundle() {
        assert!(
            SDK_PREFS_JS.contains("GENERATED from packages/lucidos-sdk/src/boot/"),
            "the served script must be the built bundle, not something hand-edited here"
        );
        // Wrapped so its locals never reach the iframe's global scope. esbuild's
        // IIFE wrapper is an arrow form, hence the loose check.
        assert!(SDK_PREFS_JS.contains("(() => {"));
        assert!(SDK_PREFS_JS.trim_end().ends_with("})();"));
    }

    #[test]
    fn script_is_the_iframe_build_not_the_shell_one() {
        // The two entry points differ in exactly the pieces that are the SHELL's:
        // the boot-splash gradient and the theme-telemetry POST. Serving the host
        // bundle to app iframes would paint the brand gradient behind every app
        // and have each of them POST breadcrumbs on load.
        assert!(
            !SDK_PREFS_JS.contains("radial-gradient"),
            "the boot splash belongs to the shell"
        );
        assert!(
            !SDK_PREFS_JS.contains("client-log"),
            "theme telemetry belongs to the shell"
        );
        // And the shell's escape hatch is not the iframe's: the shell clears the
        // key before an iframe loads, so `?style-reset` must be off here.
        assert!(SDK_PREFS_JS.contains("styleReset: false"));
    }

    #[test]
    fn script_reads_theme_mode_font_and_scale_from_the_seed_then_storage() {
        // Two sources, in that order. The seed is what an ISOLATED app frame
        // has, since it can read none of the shell's storage. Storage is the
        // shell's own path, and the fallback behind the seed everywhere else.
        assert!(SDK_PREFS_JS.contains(r#"THEME_MODE_KEY = "theme-mode""#));
        assert!(SDK_PREFS_JS.contains(r#"THEME_MODE_STORAGE_KEY = "lucidos-theme-mode""#));
        assert!(SDK_PREFS_JS.contains("seeded(served, THEME_MODE_KEY, THEME_MODE_STORAGE_KEY)"));
        assert!(
            SDK_PREFS_JS.contains(r#"seeded(served, "font-family", "lucidos-font-family")"#),
            "sdk-prefs.js must read font-family from the seed, then storage"
        );
        // Scale reads the seed inline rather than through `seeded`, because it
        // carries the two pre-grid aliases as well. Same order, same fallback.
        assert!(SDK_PREFS_JS.contains(r#"served["ui-scale"]"#));
        assert!(SDK_PREFS_JS.contains(r#"served["text-size"]"#));
        assert!(SDK_PREFS_JS.contains(r#"wsLocalGet("lucidos-ui-scale")"#));
    }

    #[test]
    fn the_seed_the_script_reads_is_the_one_this_route_writes() {
        // Two halves of one contract in two languages, so a rename on either
        // side has to be a rename on both.
        assert!(
            SDK_PREFS_JS.contains(&format!("globalThis.{SEED_GLOBAL}")),
            "the bundle must read the global this route prepends"
        );
    }

    #[test]
    fn script_namespaces_keys_per_workspace() {
        // The iframe realm has no access to the parent's Storage.prototype
        // override, so it must derive the workspace slug and namespace the keys
        // itself, or a parent `ws:<slug>:lucidos-theme-mode` write would never match
        // the iframe read and every app would FOUC. Direct to an engine the
        // slug is the path before `/app/`. Behind a gateway the engine stamps a
        // frame capability base, and the slug is everything before that segment
        // (ADR 0238). The script has to carry both derivations.
        assert!(SDK_PREFS_JS.contains("indexOf(\"/app/\")"));
        assert!(SDK_PREFS_JS.contains("~cap"));
        assert!(SDK_PREFS_JS.contains("ws:"));
    }

    #[test]
    fn script_has_no_unscoped_appearance_reads() {
        // Guard against a regression that drops the wsKey() wrapper. No raw,
        // string-literal read of a per-workspace appearance key may remain.
        for key in [
            "lucidos-theme-mode",
            "lucidos-font-family",
            "lucidos-ui-scale",
            "lucidos-style-overrides",
        ] {
            let raw = format!("localStorage.getItem(\"{key}\")");
            assert!(
                !SDK_PREFS_JS.contains(&raw),
                "sdk-prefs.js must not read {key} unscoped, wrap it in wsKey()"
            );
        }
    }

    #[test]
    fn script_resolves_system_theme_mode_via_matchmedia() {
        // `system` must defer to matchMedia at execution time so light-OS
        // browsers don't FOUC dark-then-light.
        assert!(SDK_PREFS_JS.contains("matchMedia(\"(prefers-color-scheme: light)\")"));
    }

    #[test]
    fn script_sets_data_theme_mode_and_bg_primary() {
        assert!(SDK_PREFS_JS.contains(r#"THEME_MODE_ATTRIBUTE = "data-theme-mode""#));
        assert!(SDK_PREFS_JS.contains("setAttribute(THEME_MODE_ATTRIBUTE, resolved)"));
        // App styles written before the rename still key on `data-theme`
        // (docs/temporary-measures.md § Legacy `data-theme` in app frames).
        assert!(SDK_PREFS_JS.contains(r#"LEGACY_THEME_MODE_ATTRIBUTE = "data-theme""#));
        assert!(SDK_PREFS_JS.contains("legacyThemeModeAttribute: true"));
        assert!(SDK_PREFS_JS.contains("setProperty(\"--bg-primary\""));
        assert!(SDK_PREFS_JS.contains("setProperty(\"--font-ui\""));
        assert!(SDK_PREFS_JS.contains("setProperty(\"--font-features-text\""));
        assert!(SDK_PREFS_JS.contains("setProperty(\"--font-features-code\""));
    }

    #[test]
    fn script_sets_inline_html_background() {
        // iOS PWA regression: until the iframe's stylesheet applies
        // `html { background: var(--bg-primary); }`, <html> has no background
        // and WKWebView's underlying white shows through any area body doesn't
        // cover. Setting style.background directly on <html> from FOUC closes
        // that gap on first paint.
        assert!(
            SDK_PREFS_JS.contains("d.style.background ="),
            "FOUC must set d.style.background inline, not just the --bg-primary CSS variable"
        );
    }

    #[test]
    fn script_never_sets_font_feature_settings_directly() {
        // Scope is decided by the two rules in api/sdk_iframe.css, which consume
        // the published custom properties. The bare property is inherited, so
        // writing it here would ligature an app's prose as well as its code.
        assert!(!SDK_PREFS_JS.contains("setProperty(\"font-feature-settings\""));
    }

    fn prefs(pairs: &[(&str, &str)]) -> std::collections::HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn the_seed_line_carries_the_appearance_keys() {
        assert_eq!(
            seed_line(&prefs(&[("theme-mode", "dark"), ("ui-scale", "150")])),
            "window.__lucidosPrefs={\"theme-mode\":\"dark\",\"ui-scale\":\"150\"};\n"
        );
    }

    #[test]
    fn the_seed_line_carries_the_autocorrect_switch() {
        // An isolated frame reads none of the shell's storage, so the seed is
        // its only synchronous source for the stamp's first value.
        assert_eq!(
            seed_line(&prefs(&[("autocorrect", "true")])),
            "window.__lucidosPrefs={\"autocorrect\":\"true\"};\n"
        );
    }

    #[test]
    fn the_seed_line_carries_theme_effects() {
        assert_eq!(
            seed_line(&prefs(&[("theme-effects", "reduce")])),
            "window.__lucidosPrefs={\"theme-effects\":\"reduce\"};\n"
        );
    }

    #[test]
    fn the_seed_line_carries_motion() {
        // Without it an isolated frame paints its first frame from the OS
        // switch alone, and a device that chose Reduce sees motion.
        assert_eq!(
            seed_line(&prefs(&[("motion", "reduce")])),
            "window.__lucidosPrefs={\"motion\":\"reduce\"};\n"
        );
    }

    #[test]
    fn the_seed_line_carries_nothing_else() {
        let line = seed_line(&prefs(&[
            ("theme-mode", "light"),
            ("chat_model", "claude-opus-5"),
        ]));
        assert!(line.contains(r#""theme-mode":"light""#));
        assert!(
            !line.contains("chat_model"),
            "an unrelated preference must not reach an app: {line}"
        );
    }

    #[test]
    fn the_seed_carries_the_active_theme_resolved() {
        let data = std::path::Path::new("/nonexistent");
        let mut seeded = prefs(&[("theme", "nord")]);
        with_resolved_theme(&mut seeded, data);
        let line = seed_line(&seeded);
        assert!(line.contains("theme_resolved"), "{line}");
        assert!(line.contains("--bg-primary"), "{line}");

        // A forged value under the seed key never survives, and the default
        // theme seeds nothing to apply.
        let mut forged = prefs(&[("theme", "lucidos"), ("theme_resolved", "{\"dark\":{}}")]);
        with_resolved_theme(&mut forged, data);
        assert!(!forged.contains_key("theme_resolved"));
    }

    #[test]
    fn the_seed_carries_the_picked_workspace_font() {
        let dir = tempfile::tempdir().unwrap();
        let font = dir.path().join("fonts/brand");
        std::fs::create_dir_all(&font).unwrap();
        std::fs::write(
            font.join("font.json"),
            r#"{"label":"Brand","group":"sans","faces":[{"file":"a.woff2"}]}"#,
        )
        .unwrap();
        std::fs::write(font.join("a.woff2"), b"wOF2 bytes").unwrap();

        let mut seeded = prefs(&[("font-family", "ws-brand")]);
        with_workspace_font(&mut seeded, dir.path());
        let line = seed_line(&seeded);
        assert!(line.contains("workspace_font"), "{line}");
        assert!(line.contains("fonts/brand/a.woff2"), "{line}");

        // A forged entry never survives, and a font that is gone seeds nothing.
        for pick in ["ws-gone", "fira-code"] {
            let mut forged = prefs(&[("font-family", pick), ("workspace_font", "{}")]);
            with_workspace_font(&mut forged, dir.path());
            assert!(!forged.contains_key("workspace_font"), "{pick}");
        }
    }

    #[test]
    fn nothing_stored_prepends_nothing() {
        assert_eq!(seed_line(&prefs(&[])), "");
    }

    #[test]
    fn the_seed_cannot_end_the_script_early() {
        // `style_overrides` is a map any app may write through
        // `lucidos.preferences.set`, and it is served as JavaScript. `serde_json`
        // escapes the quote, so the value stays one string literal.
        let line = seed_line(&prefs(&[(
            "style_overrides",
            "{\"--x\":\"\";window.stolen=1;//\"}",
        )]));
        assert_eq!(
            line,
            "window.__lucidosPrefs=\
             {\"style_overrides\":\"{\\\"--x\\\":\\\"\\\";window.stolen=1;//\\\"}\"};\n",
            "every quote in the value stays escaped, so it is one string literal"
        );
    }

    #[test]
    fn the_seed_precedes_the_bundle_it_feeds() {
        let body = format!(
            "{}{}",
            seed_line(&prefs(&[("theme-mode", "dark")])),
            SDK_PREFS_JS
        );
        let seed_at = body.find("__lucidosPrefs").expect("seed present");
        let bundle_at = body.find("(() => {").expect("bundle present");
        assert!(
            seed_at < bundle_at,
            "the bundle reads the seed, so it must run after it"
        );
    }

    #[test]
    fn script_still_applies_the_style_remote() {
        // Presence only. Two behaviours that used to be asserted here by reading
        // the script top to bottom cannot be: in a BUNDLE the source order of a
        // definition says nothing about execution order. That "overrides are
        // applied last" rule, and "no stored scale leaves --user-ui-scale
        // unset", are now driven for real against a fake document in
        // `packages/lucidos-sdk/src/boot/appearanceBoot.test.ts`, which is a
        // stronger check than the scan ever was.
        assert!(SDK_PREFS_JS.contains("lucidos-style-overrides"));
    }
}
