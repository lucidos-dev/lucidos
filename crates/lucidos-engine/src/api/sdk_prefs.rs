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
//! The SDK's `lucidos.ui.applyPreferences()` continues to handle live SSE
//! updates; it just overwrites the values this script set.

use super::*;

// Mirrors the inline FOUC IIFE in `crates/lucidos-app/index.html`. Keep the
// two in sync — when adding a font/scale option, update both. The two scripts
// can't share a source: this one is parser-blocking JS served to iframes, the
// other is parser-blocking JS embedded in the parent shell HTML, and both
// must run before any TS module bootstraps.
const SDK_PREFS_JS: &str = r##"(function() {
  var d = document.documentElement;
  var raw = localStorage.getItem("lucidos-theme");
  var theme = (raw === "light" || raw === "dark" || raw === "system") ? raw : "dark";
  var resolved = theme === "system"
    ? (matchMedia("(prefers-color-scheme: light)").matches ? "light" : "dark")
    : theme;
  d.setAttribute("data-theme", resolved);
  var bg = resolved === "light" ? "#ffffff" : "#0d1117";
  d.style.setProperty("--bg-primary", bg);
  // iOS PWA: covers the WKWebView-white gap before sdk-iframe.css loads
  // (mirrors the parent's inline FOUC IIFE in index.html).
  d.style.background = bg;
  var FONTS = {
    monospace: "'SF Mono', 'Fira Code', 'JetBrains Mono', Monaco, Consolas, monospace",
    system: "system-ui, -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif",
    inter: "'Inter', system-ui, sans-serif",
    "jetbrains-mono": "'JetBrains Mono', monospace",
    "ibm-plex-mono": "'IBM Plex Mono', monospace"
  };
  var fontKey = localStorage.getItem("lucidos-font-family");
  d.style.setProperty("--font-ui", FONTS[fontKey] || FONTS.monospace);
  var scale = localStorage.getItem("lucidos-ui-scale");
  if (scale) {
    // Mirrors clampUiScale in preferences.ts — keep (75, 200, 12.5) in sync.
    var legacy = { small: 100, medium: 112.5, large: 125 };
    var n = legacy[scale];
    if (n === undefined) n = parseFloat(scale);
    if (!isNaN(n)) {
      var snapped = Math.min(200, Math.max(75, Math.round(n / 12.5) * 12.5));
      d.style.setProperty("--user-ui-scale", snapped + "%");
    }
  }
})();"##;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_reads_theme_from_localstorage() {
        assert!(SDK_PREFS_JS.contains("localStorage.getItem(\"lucidos-theme\")"));
    }

    #[test]
    fn script_reads_font_family_from_localstorage() {
        assert!(SDK_PREFS_JS.contains("localStorage.getItem(\"lucidos-font-family\")"));
    }

    #[test]
    fn script_reads_ui_scale_from_localstorage() {
        assert!(SDK_PREFS_JS.contains("localStorage.getItem(\"lucidos-ui-scale\")"));
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
    }

    #[test]
    fn script_sets_inline_html_background() {
        // iOS PWA regression — until the iframe's stylesheet applies
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
    fn script_defaults_to_dark_for_unknown_theme() {
        // Anything not light/dark/system falls back to dark — matches the
        // SDK's `prefs['theme'] || 'dark'` behavior.
        assert!(SDK_PREFS_JS.contains("? raw : \"dark\""));
    }

    #[test]
    fn script_is_wrapped_in_iife() {
        // IIFE keeps locals out of the iframe's global scope.
        assert!(SDK_PREFS_JS.starts_with("(function()"));
        assert!(SDK_PREFS_JS.ends_with("})();"));
    }

    #[test]
    fn script_omits_scale_var_when_unset() {
        // The `if (scale)` branch means missing localStorage entry → no
        // `--user-ui-scale` set, letting the stylesheet default apply.
        assert!(SDK_PREFS_JS.contains("if (scale)"));
    }

}
