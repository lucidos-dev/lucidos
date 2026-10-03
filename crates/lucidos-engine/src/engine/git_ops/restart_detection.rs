use std::path::Path;

/// True if `repo_path` doesn't canonicalize to `dev_root`. Falls back to raw
/// equality when canonicalization fails (test-only paths, repos missing on disk).
pub(crate) fn is_external_repo_path(repo_path: &Path, dev_root: &Path) -> bool {
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    canon(repo_path) != canon(dev_root)
}

/// The crates the engine, gateway and `lucidos` CLI binaries are built from.
/// Everything in them but tests and docs is compiled in or feeds the build.
const BINARY_CRATES: &[&str] = &[
    "crates/lucidos-engine/",
    "crates/lucidos-gateway/",
    "crates/lucidos-cli/",
];

/// Files outside [`BINARY_CRATES`] that a binary `include_str!`s or
/// `include_bytes!`s. A running process serves the copy it was BUILT with, so
/// only a rebuild picks up an edit. Some read as docs or frontend files by
/// path: the changelog, the CLI skill, the stylesheet served to app iframes.
/// `a_file_a_binary_embeds_requires_a_restart` fails when one is missing here.
const EMBEDDED_FILES: &[&str] = &[
    "RELEASE",
    "CHANGELOG.md",
    "release-notices.toml",
    ".claude/skills/lucidos-cli/SKILL.md",
    "crates/lucidos-app/index.html",
    "crates/lucidos-app/public/favicon.svg",
    "crates/lucidos-app/src/styles/global/shared-components.css",
    "crates/lucidos-app/src/styles/global/surface.css",
    "crates/lucidos-app/src/styles/global/text-input.css",
    "crates/lucidos-app/src/styles/generated/theme-parts-frame.css",
];

/// Check whether any of the given file paths would require an engine restart.
/// Rust source files (excluding tests/docs), SQL migrations, the SDK bundle
/// sources, and every file a binary embeds all trigger a restart.
pub(crate) fn files_require_restart(files: &[String]) -> bool {
    files.iter().any(|f| {
        let is_rust_source = f.ends_with(".rs")
            || f == "Cargo.toml"
            || f.ends_with("/Cargo.toml")
            || f == "Cargo.lock";
        let is_test_or_doc = f.contains("/tests/") || f.starts_with("tests/") || f.ends_with(".md");
        let is_migration =
            f.ends_with(".sql") && (f.contains("/migrations/") || f.starts_with("migrations/"));
        // packages/lucidos-sdk → /api/v1/sdk.js. The bundle is rebuilt by
        // `web-dev.sh -b` on engine restart; without restart the previously-
        // built dist/sdk.js keeps being served.
        let is_sdk_bundle_source = f.starts_with("packages/lucidos-sdk/") && !is_test_or_doc;
        let in_binary_crate = BINARY_CRATES.iter().any(|c| f.starts_with(c)) && !is_test_or_doc;
        // The host's @font-face resolves the vendored fonts through Vite, and
        // `core::fonts` embeds the same files to serve app iframes.
        let is_vendored_font =
            f.starts_with("crates/lucidos-app/src/assets/fonts/") && f.ends_with(".woff2");
        (is_rust_source && !is_test_or_doc)
            || is_migration
            || is_sdk_bundle_source
            || in_binary_crate
            || is_vendored_font
            || EMBEDDED_FILES.contains(&f.as_str())
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

/// Directories whose files feed the served client bundle. Mirrors `watchDirs`
/// in `crates/lucidos-app/dev-build-watch.mjs`.
pub(super) const CLIENT_BUNDLE_DIRS: &[&str] = &[
    "crates/lucidos-app/src/",
    "crates/lucidos-app/public/",
    "crates/lucidos-app/vite/",
    "packages/lucidos-sdk/src/",
];

/// Single files that feed the served client bundle. Mirrors `watchFiles` in
/// `crates/lucidos-app/dev-build-watch.mjs`.
pub(super) const CLIENT_BUNDLE_FILES: &[&str] = &[
    "crates/lucidos-app/index.html",
    "crates/lucidos-app/vite.config.ts",
    "crates/lucidos-app/package.json",
    "package.json",
    "package-lock.json",
];

/// True if any of `files` is an input the build-watch rebuilds the served
/// client from. After a frontend-only Apply the engine waits for that rebuild,
/// so a file the watch ignores (an e2e spec, a script) must not count: no
/// build would run, and the wait would end in a false "not served yet" warning.
pub(crate) fn files_have_client_update(files: &[String]) -> bool {
    files.iter().any(|f| {
        CLIENT_BUNDLE_FILES.contains(&f.as_str())
            || CLIENT_BUNDLE_DIRS.iter().any(|dir| f.starts_with(dir))
    })
}
