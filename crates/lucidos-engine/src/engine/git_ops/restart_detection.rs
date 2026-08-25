use std::path::Path;

/// True if `repo_path` doesn't canonicalize to `dev_root`. Falls back to raw
/// equality when canonicalization fails (test-only paths, repos missing on disk).
pub(crate) fn is_external_repo_path(repo_path: &Path, dev_root: &Path) -> bool {
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    canon(repo_path) != canon(dev_root)
}

/// Check whether any of the given file paths would require an engine restart.
/// Rust source files (excluding tests/docs), SQL migrations, the SDK bundle
/// sources, and engine-bundled static assets all trigger a restart.
pub(crate) fn files_require_restart(files: &[String]) -> bool {
    files.iter().any(|f| {
        let is_rust_source = f.ends_with(".rs") || f == "Cargo.toml" || f == "Cargo.lock";
        let is_test_or_doc = f.contains("/tests/") || f.starts_with("tests/") || f.ends_with(".md");
        let is_migration =
            f.ends_with(".sql") && (f.contains("/migrations/") || f.starts_with("migrations/"));
        // packages/lucidos-sdk → /api/v1/sdk.js. The bundle is rebuilt by
        // `web-dev.sh -b` on engine restart; without restart the previously-
        // built dist/sdk.js keeps being served.
        let is_sdk_bundle_source = f.starts_with("packages/lucidos-sdk/") && !is_test_or_doc;
        // include_str!'d into a Rust binary, so the running process serves the
        // copy it was BUILT with and a rebuild is the only way to pick up an
        // edit: the engine's /api/v1/sdk-iframe.* assets, and the app document,
        // whose boot-splash stylesheet + mark the gateway lifts out at compile
        // time (crates/lucidos-gateway/src/proxy.rs). Without the restart the
        // gateway keeps serving the previous splash while the app has the new
        // one, which is exactly the drift sharing the file removes.
        // The changelog is one of these too, and it is the one that looks like a
        // doc: `crate::engine::changelog` include_str!s it and serves it to the
        // What's New panel, so an edit that skipped the restart would leave the
        // panel showing the previous text with nothing saying why. The `.md`
        // exclusion above gates only the Rust-source and SDK-bundle arms, so
        // naming it here is enough.
        // The release notices are the changelog's sibling and cost more when
        // missed: `crate::engine::release_notices` include_str!s them, and an
        // authored notice that never reaches a modal is an instruction the user
        // is simply never given.
        // The font is the odd one: it lives in the APP crate (the host's
        // @font-face resolves it through Vite), which would otherwise read as a
        // frontend-only change, but the engine include_bytes!s that same file to
        // serve app iframes. One copy in the tree, two consumers, and only one
        // of them picks up an edit without a rebuild.
        let is_engine_bundled_asset = f == "crates/lucidos-engine/src/api/sdk_iframe.css"
            || f == "crates/lucidos-engine/src/api/sdk_iframe_audio.js"
            || f == "crates/lucidos-engine/src/api/sdk_fonts_fira_code.css"
            || f == "crates/lucidos-app/src/assets/fonts/FiraCode-VF.woff2"
            || f == "crates/lucidos-app/index.html"
            || f == "CHANGELOG.md"
            || f == "release-notices.toml";
        (is_rust_source && !is_test_or_doc)
            || is_migration
            || is_sdk_bundle_source
            || is_engine_bundled_asset
    })
}

/// File extensions an app's iframe directly serves (HTML / CSS / JS / static
/// assets). Matches what `/api/v1/app/<id>/...` streams in `crates/lucidos-engine/src/api/apps.rs`.
const APP_IFRAME_BUNDLED_EXTENSIONS: &[&str] = &[
    "html", "htm", "css", "js", "svg", "png", "jpg", "jpeg", "gif", "webp", "ico", "woff", "woff2",
    "ttf", "otf", "json",
];

/// True if any of `files` is a path inside `data/apps/<app_id>/` that the
/// app's iframe would actually load. Used by the app coding-agent thread
/// Apply path to decide whether to emit `AppUiRefreshRequested`.
///
/// `index.html` and `manifest.json` ALWAYS trigger. For other extensions,
/// matches the iframe-bundled allow-list above. Files outside the app folder
/// (e.g. a workspace-wide knowhow file the agent accidentally touched) do
/// not trigger the refresh — they couldn't affect this app's iframe anyway.
pub(crate) fn any_iframe_bundled_file_changed(files: &[String], app_id: &str) -> bool {
    let prefix = format!("data/apps/{}/", app_id);
    files.iter().any(|f| {
        if !f.starts_with(&prefix) {
            return false;
        }
        // index.html and manifest.json are always iframe-bundled even if a
        // future ext-list edit forgets them.
        if f == &format!("{prefix}index.html") || f == &format!("{prefix}manifest.json") {
            return true;
        }
        let ext = std::path::Path::new(f)
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase);
        match ext {
            Some(e) => APP_IFRAME_BUNDLED_EXTENSIONS.contains(&e.as_str()),
            None => false,
        }
    })
}

/// Check whether any of the given file paths are frontend files that would
/// require a client reload. TypeScript, CSS, HTML, and JavaScript files trigger this.
pub(crate) fn files_have_client_update(files: &[String]) -> bool {
    files.iter().any(|f| {
        f.ends_with(".ts")
            || f.ends_with(".tsx")
            || f.ends_with(".css")
            || f.ends_with(".html")
            || f.ends_with(".js")
            || f.ends_with(".jsx")
    })
}
