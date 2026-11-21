//! Every file the engine generates for the frontend and the SDK.
//!
//! They are build outputs. `scripts/codegen.sh` is the one command that writes
//! them, through the `lucidos-codegen` bin or the
//! `write_generated_outputs` test below. Rust is the source of truth, so a
//! renderer refuses rather than guesses, and a refusal fails `make test`.

mod app_reach;

use std::path::Path;

mod agent_surface;
mod engine_constants;
mod gateway_constants;
mod knowhow_blocks;

#[cfg(test)]
use lucidos_engine::codegen_inputs::GENERATED_DIRS;
use lucidos_engine::codegen_inputs::{APP, DOCS, INTERNAL, SDK, STYLES};
pub(crate) use lucidos_engine::core::codegen::quoted;
pub use lucidos_engine::core::codegen::REGENERATE;

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
    use lucidos_engine::core::{
        fonts, preference_catalog, store::messages::spoken_merge, themes::parts,
    };
    use lucidos_engine::engine::thread_lifecycle;
    vec![
        file(
            INTERNAL,
            "knowhow-blocks.json",
            knowhow_blocks::render(repo_root),
        ),
        file(DOCS, "agent-surface.json", agent_surface::render()),
        GeneratedFile {
            path: lucidos_wire_codegen::WIRE_FILE.to_string(),
            contents: lucidos_wire_codegen::render(repo_root),
        },
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
            lucidos_engine::codegen_inputs::title_match_fixture(),
        ),
        file(APP, "gateway-constants.ts", gateway_constants::render()),
        file(
            APP,
            "version-order-fixture.json",
            serde_json::to_string_pretty(lucidos_update_channel::ORDERED_VERSIONS)
                .expect("the version fixture serialises"),
        ),
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
        file(SDK, "app-reach.ts", app_reach::render()),
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
            lucidos_engine::llm::tools::misc::navigate_targets_codegen::render(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// `scripts/codegen.sh` runs this when no freshly built `lucidos-codegen`
    /// is at hand, because it reuses the compile `make test` already paid for.
    #[test]
    #[ignore]
    fn write_generated_outputs() {
        let out = std::env::var_os("LUCIDOS_CODEGEN_OUT")
            .expect("LUCIDOS_CODEGEN_OUT names the folder to write into");
        let root = std::env::var_os("LUCIDOS_CODEGEN_REPO_ROOT")
            .map_or_else(guard_tests::repo_root, std::path::PathBuf::from);
        write_all(&render_all(&root), Path::new(&out)).expect("the outputs write");
    }

    mod guard_tests {
        use super::*;

        pub(super) fn repo_root() -> std::path::PathBuf {
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

        /// The hardcoded name list is a SECOND hand-maintained hop, and nothing guarded
        /// it. A variant added to the enum but missed there never reaches
        /// `EVENT_CLASSIFICATION`, so the frontend's drift guard never sees it either.
        #[test]
        fn all_persisted_event_types_matches_the_enum() {
            let parsed = lucidos_wire_codegen::persisted_event_type_names(&repo_root());
            let listed: BTreeSet<String> =
                lucidos_engine::engine::thread_lifecycle::all_persisted_event_types()
                    .into_iter()
                    .map(str::to_string)
                    .collect();
            let missing: Vec<&String> = parsed.difference(&listed).collect();
            let extra: Vec<&String> = listed.difference(&parsed).collect();
            assert!(
                missing.is_empty(),
                "these persisted `ThreadEvent` variants are missing from \
             `all_persisted_event_types()`, so they never reach EVENT_CLASSIFICATION \
             or the frontend's drift guard: {missing:?}"
            );
            assert!(
                extra.is_empty(),
                "`all_persisted_event_types()` names events the enum does not have \
             (or names a transient one): {extra:?}"
            );
        }

        /// Serde always sends the reset key, as `null` when unset. The wire
        /// type must say the same, or the frontend reads a required string.
        #[test]
        fn spend_cap_notice_reset_is_null_in_serde_and_in_the_wire_type() {
            let notice = lucidos_engine::llm::spend_cap::SpendCapNotice {
                provider: "openai".into(),
                resets_at: None,
                message: "spent".into(),
            };
            let wire = serde_json::to_value(notice).unwrap();
            assert_eq!(wire.get("resets_at"), Some(&serde_json::Value::Null));
            assert!(
                lucidos_wire_codegen::render(&repo_root()).contains("  resets_at: string | null;")
            );
        }

        /// The reader names its own output path, and it must sit in `APP`.
        #[test]
        fn the_wire_file_is_in_the_app_generated_folder() {
            assert!(lucidos_wire_codegen::WIRE_FILE
                .strip_prefix(APP)
                .is_some_and(|name| name.starts_with('/')));
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
    }
}
