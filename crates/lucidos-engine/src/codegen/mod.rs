//! Every file the engine generates for the frontend and the SDK.
//!
//! They are build outputs. `scripts/codegen.sh` is the one command that writes
//! them, through the `lucidos-codegen` bin or the
//! `write_generated_outputs` test below. Rust is the source of truth, so a
//! renderer refuses rather than guesses, and a refusal fails `make test`.

use std::path::Path;

mod agent_surface;
mod engine_constants;
mod gateway_constants;
mod knowhow_blocks;

/// The command each generated file's header names.
pub const REGENERATE: &str = "./scripts/codegen.sh";

/// The folders the generated files land in, relative to the repo root.
pub const GENERATED_DIRS: [&str; 5] = [APP, STYLES, SDK, "docs/generated", ".codegen"];

const APP: &str = "crates/lucidos-app/src/generated";
const STYLES: &str = "crates/lucidos-app/src/styles/generated";
const SDK: &str = "packages/lucidos-sdk/src/generated";

/// One generated file: where it goes, relative to the repo root, and its bytes.
pub struct GeneratedFile {
    pub path: String,
    pub contents: String,
}

fn file(dir: &str, name: &str, contents: String) -> GeneratedFile {
    GeneratedFile {
        path: format!("{dir}/{name}"),
        contents,
    }
}

/// Renders every generated file. `repo_root` is where the wire-type reader
/// finds the engine source.
pub fn render_all(repo_root: &Path) -> Vec<GeneratedFile> {
    use crate::core::{fonts, preference_catalog, store::messages::spoken_merge, themes::parts};
    use crate::engine::{thread_events::ts_codegen, thread_lifecycle, title_match};
    vec![
        file(
            ".codegen",
            "knowhow-blocks.json",
            knowhow_blocks::render(repo_root),
        ),
        file(
            "docs/generated",
            "agent-surface.json",
            agent_surface::render(),
        ),
        file(APP, "thread-event-wire.ts", ts_codegen::render(repo_root)),
        file(
            APP,
            "thread-lifecycle.ts",
            thread_lifecycle::codegen::render_ts(),
        ),
        file(
            APP,
            "cross-validation-fixture.json",
            thread_lifecycle::codegen::render_fixture(),
        ),
        file(
            APP,
            "spoken-merge-fixture.json",
            spoken_merge::render_fixture(),
        ),
        file(
            APP,
            "title-match-fixture.json",
            title_match::codegen::render_fixture(),
        ),
        file(APP, "gateway-constants.ts", gateway_constants::render()),
        file(STYLES, "font-faces.css", fonts::codegen::render_css()),
        file(
            STYLES,
            "theme-parts.css",
            parts::codegen::render_shell_css(),
        ),
        file(
            STYLES,
            "theme-parts-frame.css",
            parts::codegen::render_frame_css(),
        ),
        file(
            SDK,
            "app-reach.ts",
            crate::api::app_reach::codegen::render(),
        ),
        file(
            SDK,
            "capabilities.ts",
            lucidos_capabilities::codegen::render_sdk_capabilities(REGENERATE),
        ),
        file(SDK, "engine-constants.ts", engine_constants::render()),
        file(SDK, "font-catalog.ts", fonts::codegen::render_ts()),
        file(
            SDK,
            "navigate-targets.ts",
            crate::llm::tools::misc::navigate_targets_codegen::render(),
        ),
        file(
            SDK,
            "preference-catalog.ts",
            preference_catalog::codegen::render(),
        ),
        file(SDK, "theme-parts.ts", parts::codegen::render_ts()),
    ]
}

/// Writes `files` under `out_dir`, creating folders as needed.
pub fn write_all(files: &[GeneratedFile], out_dir: &Path) -> std::io::Result<()> {
    for f in files {
        let path = out_dir.join(&f.path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, &f.contents)?;
    }
    Ok(())
}

/// A string as a JSON (and so TypeScript) literal.
pub(crate) fn quoted(text: &str) -> String {
    serde_json::to_string(text).expect("a string serialises")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn repo_root() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    /// Every renderer runs on every `make test`, so a refusal (an unregistered
    /// wire type, say) fails the suite rather than the next codegen run.
    #[test]
    fn every_generated_file_renders_once_into_a_generated_folder() {
        let files = render_all(&repo_root());
        let mut seen = BTreeSet::new();
        for f in &files {
            assert!(seen.insert(&f.path), "{} is rendered twice", f.path);
            assert!(
                GENERATED_DIRS
                    .iter()
                    .any(|dir| f.path.strip_prefix(dir).is_some_and(|n| n.starts_with('/'))),
                "{} is outside the generated folders",
                f.path
            );
            assert!(
                f.contents.contains(REGENERATE) || f.path.ends_with(".json"),
                "{} does not name {REGENERATE} in its header",
                f.path
            );
        }
    }

    /// The shell cannot read `render_all`, so a pin holds its one copy.
    #[test]
    fn codegen_sentinel_is_rendered() {
        let script = std::fs::read_to_string(repo_root().join("scripts/lib/codegen.sh"))
            .expect("scripts/lib/codegen.sh exists");
        let sentinel = script
            .lines()
            .find_map(|l| l.strip_prefix("CODEGEN_SENTINEL="))
            .expect("codegen.sh names a sentinel");
        assert!(
            render_all(&repo_root()).iter().any(|f| f.path == sentinel),
            "CODEGEN_SENTINEL={sentinel} is not a file render_all writes"
        );
    }

    /// `scripts/codegen.sh` runs this when no freshly built `lucidos-codegen`
    /// is at hand, because it reuses the compile `make test` already paid for.
    #[test]
    #[ignore]
    fn write_generated_outputs() {
        let out = std::env::var_os("LUCIDOS_CODEGEN_OUT")
            .expect("LUCIDOS_CODEGEN_OUT names the folder to write into");
        let root = std::env::var_os("LUCIDOS_CODEGEN_REPO_ROOT")
            .map_or_else(repo_root, std::path::PathBuf::from);
        write_all(&render_all(&root), Path::new(&out)).expect("the outputs write");
    }
}
