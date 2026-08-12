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
//! The script reads the user's theme/font/scale from localStorage and sets
//! `data-theme`, `--bg-primary`, `--font-ui` (and `--user-ui-scale` when set)
//! on `<html>` synchronously, so first paint matches the user's preferences
//! before any subsequent stylesheet evaluates.
//!
//! Same-origin iframes (sandbox includes `allow-same-origin`) inherit the
//! parent's localStorage, so the parent shell's mirror writes — performed by
//! `applyTheme` / `applyFontFamily` / `applyUiScale` in
//! `crates/lucidos-app/src/store/actions/preferences.ts` — are immediately
//! visible to every iframe load. No cookie or DB roundtrip is needed.
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

/// GET /api/v1/sdk-prefs.js — synchronous prefs script driven by localStorage.
pub(super) async fn serve_sdk_prefs_js() -> Response {
    (
        [
            (
                header::CONTENT_TYPE,
                "application/javascript; charset=utf-8",
            ),
            (header::CACHE_CONTROL, "public, max-age=300"),
        ],
        SDK_PREFS_JS,
    )
        .into_response()
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
    fn script_reads_theme_font_and_scale_from_localstorage() {
        // Through `wsLocalGet`, the SDK's per-workspace storage helper, which is
        // the same one the SDK's live `applyPreferences()` uses. Reading raw
        // `localStorage` here is what the next test forbids.
        for key in ["lucidos-theme", "lucidos-font-family", "lucidos-ui-scale"] {
            assert!(
                SDK_PREFS_JS.contains(&format!("wsLocalGet(\"{key}\")")),
                "sdk-prefs.js must read {key}"
            );
        }
    }

    #[test]
    fn script_namespaces_keys_per_workspace() {
        // The iframe realm has no access to the parent's Storage.prototype
        // override, so it must derive the workspace slug and namespace the keys
        // itself, or a parent `ws:<slug>:lucidos-theme` write would never match
        // the iframe read and every app would FOUC. An app iframe has no <base>,
        // so its slug is the path before `/app/`.
        assert!(SDK_PREFS_JS.contains("indexOf(\"/app/\")"));
        assert!(SDK_PREFS_JS.contains("ws:"));
    }

    #[test]
    fn script_has_no_unscoped_appearance_reads() {
        // Guard against a regression that drops the wsKey() wrapper. No raw,
        // string-literal read of a per-workspace appearance key may remain.
        for key in [
            "lucidos-theme",
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
    fn script_resolves_system_theme_via_matchmedia() {
        // `system` must defer to matchMedia at execution time so light-OS
        // browsers don't FOUC dark-then-light.
        assert!(SDK_PREFS_JS.contains("matchMedia(\"(prefers-color-scheme: light)\")"));
    }

    #[test]
    fn script_sets_data_theme_and_bg_primary() {
        assert!(SDK_PREFS_JS.contains("setAttribute(\"data-theme\""));
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
