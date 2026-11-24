use std::path::Path;

/// True if `repo_path` doesn't canonicalize to `dev_root`. Falls back to raw
/// equality when canonicalization fails (test-only paths, repos missing on disk).
pub fn is_external_repo_path(repo_path: &Path, dev_root: &Path) -> bool {
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    canon(repo_path) != canon(dev_root)
}

#[path = "../../../../engine_crate_paths.rs"]
mod engine_crate_paths;

const FRONT_DOOR_CRATES: &[&str] = &["lucidos-gateway", "lucidos-cli"];

/// Files outside the binary crates that a binary `include_str!`s or
/// `include_bytes!`s. A running process serves the copy it was BUILT with, so
/// only a rebuild picks up an edit. Some read as docs or frontend files by
/// path: the changelog, the CLI skill, the stylesheet served to app iframes.
/// `a_file_a_binary_embeds_requires_a_restart` fails when one is missing here.
const EMBEDDED_FILES: &[&str] = &[
    "RELEASE",
    "CHANGELOG.md",
    "release-notices.toml",
    ".claude/skills/lucidos-cli/SKILL.md",
    "crates/lucidos-app/VERSION",
    "crates/lucidos-app/index.html",
    "crates/lucidos-app/public/favicon.svg",
    "crates/lucidos-app/public/outbox-handoff.js",
    "crates/lucidos-app/public/remote-client-upgrade.js",
    "crates/lucidos-app/src/styles/global/shared-components.css",
    "crates/lucidos-app/src/styles/global/surface.css",
    "crates/lucidos-app/src/styles/global/text-input.css",
];

/// The desktop app's compile inputs besides its Rust code. The dev window moves
/// to a new build only at a switch (ADR 0461), so an edit to one needs a switch.
const APP_COMPILE_INPUTS: &[&str] = &[
    "crates/lucidos-app/tauri.conf.json",
    "crates/lucidos-app/Info.plist",
    "crates/lucidos-app/capabilities/",
    "crates/lucidos-app/permissions/",
    "crates/lucidos-app/icons/",
];

/// Check whether any of the given file paths would require an engine restart.
/// Rust source files (excluding tests/docs), SQL migrations, the SDK bundle
/// sources, the desktop app's compile inputs, and every file a binary embeds
/// all trigger a restart.
pub fn files_require_restart(files: &[String]) -> bool {
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
        let crate_name = f
            .strip_prefix("crates/")
            .and_then(|path| path.split_once('/'))
            .map(|(name, _)| name);
        let in_binary_crate = !is_test_or_doc
            && crate_name.is_some_and(|name| {
                FRONT_DOOR_CRATES.contains(&name)
                    || engine_crate_paths::engine_crates().any(|engine_crate| engine_crate == name)
            });
        // The host's @font-face resolves the vendored fonts through Vite, and
        // `core::fonts` embeds the same files to serve app iframes.
        let is_vendored_font =
            f.starts_with("crates/lucidos-app/src/assets/fonts/") && f.ends_with(".woff2");
        let is_app_compile_input = APP_COMPILE_INPUTS.iter().any(|i| f.starts_with(i));
        (is_rust_source && !is_test_or_doc)
            || is_migration
            || is_app_compile_input
            || is_sdk_bundle_source
            || in_binary_crate
            || is_vendored_font
            || EMBEDDED_FILES.contains(&f.as_str())
            || engine_crate_paths::ENGINE_INVENTORY_INPUTS.contains(&f.as_str())
    })
}

/// File extensions an app's iframe directly serves (HTML / CSS / JS / static
/// assets). Matches what `/api/v1/app/<id>/...` streams in `crates/lucidos-agents-api/src/api/apps.rs`.
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

pub use crate::core::git::files_have_client_update;
#[cfg(test)]
pub(super) use crate::core::git::{CLIENT_BUNDLE_DIRS, CLIENT_BUNDLE_FILES};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configured_engine_crates_include_embedded_assets() {
        for name in engine_crate_paths::engine_crates() {
            assert!(files_require_restart(&[format!(
                "crates/{name}/src/embedded.css"
            )]));
            assert!(!files_require_restart(&[format!(
                "crates/{name}/tests/fixture.css"
            )]));
        }
        assert!(files_require_restart(&[
            engine_crate_paths::TIER_MAP_PATH.into()
        ]));
    }
}
