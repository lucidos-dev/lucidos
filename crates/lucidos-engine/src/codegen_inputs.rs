//! Engine-owned inputs consumed by the contract renderer.

pub const APP: &str = "crates/lucidos-app/src/generated";
pub const STYLES: &str = "crates/lucidos-app/src/styles/generated";
pub const SDK: &str = "packages/lucidos-sdk/src/generated";
pub const DOCS: &str = "docs/generated";
pub const INTERNAL: &str = ".codegen";

/// The folders the generated files land in, relative to the repo root.
pub const GENERATED_DIRS: [&str; 5] = [APP, STYLES, SDK, DOCS, INTERNAL];

pub const SOURCE_COMMIT_FILE: &str = crate::engine::frontend_refresh::SOURCE_COMMIT_FILE;

pub mod command_guard {
    use crate::engine::command_guard as guard;

    pub const BENIGN_PREFIX_WORDS: &[&str] = &guard::BENIGN_PREFIX_WORDS;
    pub const GUARD_SHELLS: &[&str] = &guard::GUARD_SHELLS;
    pub const MAX_SHELL_LAYERS: usize = guard::MAX_SHELL_LAYERS;

    pub fn grants_nothing_heads() -> &'static [&'static str] {
        &guard::GRANTS_NOTHING_HEADS
    }
}

pub fn summary_entry_kinds() -> [&'static str; 9] {
    crate::engine::summary_tree::log::EntryKind::ALL.map(|kind| kind.as_str())
}

pub fn title_match_fixture() -> String {
    crate::engine::title_match::codegen::render_fixture()
}
