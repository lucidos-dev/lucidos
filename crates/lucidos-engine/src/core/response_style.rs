//! The **response style**: how much comes back in a chat or trigger answer.
//!
//! Lucidos ships three styles and the user may edit two of them, reset either,
//! and add as many of their own as they like. One is selected at a time, by the
//! `response_style` preference. The rest of the library is one JSON document in
//! `response_styles`.
//!
//! **Standard adds nothing, and nothing can make it add something.** It is the
//! off switch rather than a style. A workspace that never opens this setting
//! gets the prompt it always got, byte for byte. Every degraded state resolves
//! there too: a selected id nobody defines, a blank instruction, a document
//! that will not parse, a failed read.
//!
//! The heading and [`STYLE_FLOOR`] belong to the engine, and only the
//! instruction between them is editable. A user can ask for one-line answers.
//! They cannot ask for an answer that drops the warning they needed.
//!
//! See `docs/plans/2026-09-18-response-style-control.md`.

use serde::{Deserialize, Serialize};
use sqlx::PgPool;

use crate::core::{PreferenceStore, PREF_RESPONSE_STYLE, PREF_RESPONSE_STYLES};

/// The off switch's id. Selected when `response_style` is unset.
pub const STANDARD_ID: &str = "standard";

/// Longest style id. It matches [`MAX_LABEL_CHARS`] only because both are short
/// names, and one number is easier to state in the UI than two.
pub const MAX_ID_CHARS: usize = 40;
/// Longest label. It renders in a dropdown row beside its description.
pub const MAX_LABEL_CHARS: usize = 40;
/// Longest instruction, roughly 150 words. It rides in the cached system tier
/// of every turn, so this is a cost ceiling rather than a taste one.
pub const MAX_INSTRUCTION_CHARS: usize = 1_000;
/// Most entries one document may hold. A preference row is not a growth
/// surface, and 20 user styles is already more than anyone will pick between.
pub const MAX_STYLES: usize = 20;

/// The one rail an editable instruction cannot remove.
///
/// Appended to every rendered style, shipped or user-written, outside the text
/// the user controls. "Answer in one line" is a reasonable thing to ask for and
/// a dangerous thing to obey without this.
const STYLE_FLOOR: &str = "- Style never overrides substance. Keep every warning, \
     every caveat that changes the answer, and every step the user has to take. \
     When they ask why or how, the explanation IS the answer, so give it in full.";

/// A style Lucidos ships.
///
/// The two non-empty ones are seeds. An entry in the user's document with the
/// same id replaces the label and the instruction, and deleting that entry
/// brings this text back.
struct ShippedStyle {
    id: &'static str,
    label: &'static str,
    /// The one line the picker shows under the label. A user style has no such
    /// field and derives one from its instruction instead.
    description: &'static str,
    instruction: &'static str,
}

/// The shipped library, in picker order. Standard leads because it is the
/// default and the way back.
const SHIPPED: &[ShippedStyle] = &[
    ShippedStyle {
        id: STANDARD_ID,
        label: "Standard",
        description: "Nothing is added. Answers come back the way they do today.",
        instruction: "",
    },
    ShippedStyle {
        id: "concise",
        label: "Concise",
        description: "Answer first, no preamble or recap. Usually a few sentences.",
        instruction:
            "- Lead with the answer. No preamble, no restating the request, no closing recap.\n\
             - A few sentences is usually the whole reply. Stop once the question is answered.\n\
             - Ask the ONE follow-up in CONVERSATION STYLE only when you cannot answer without it.",
    },
    ShippedStyle {
        id: "minimal",
        label: "Minimal",
        description: "The short answer only, and no follow-up question.",
        instruction:
            "- Answer and stop. One or two sentences, or a short list when the answer is a list.\n\
             - No preamble, no recap, no narration of what you did, no offer of next steps.\n\
             - Do not ask the follow-up in CONVERSATION STYLE unless you cannot answer without it.",
    },
];

/// One entry in the `response_styles` document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StyleEntry {
    pub id: String,
    pub label: String,
    pub instruction: String,
}

/// Where a library row came from, which is what decides whether the editor
/// offers Reset or Delete. Mirrors `ModelInfo`'s `source`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum StyleSource {
    /// Shipped, and untouched.
    Builtin,
    /// Shipped, with the user's own text on top. Reset removes the override.
    Overridden,
    /// The user's own style. Delete removes it.
    User,
}

/// A row of the merged library, as the picker and the editor see it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Style {
    pub id: String,
    pub label: String,
    pub description: String,
    pub instruction: String,
    pub source: StyleSource,
    /// False for Standard alone. Sent rather than derived, so the client does
    /// not carry a second copy of the rule.
    pub editable: bool,
}

/// Whether an id is a well-formed style id: bounded, and already the kebab-case
/// `core::slug` would produce.
///
/// The contract is a round trip through `slugify_kebab` rather than a charset
/// re-listed here. `core::slug` is the shared home of that rule. A second
/// hand-rolled copy is how two surfaces start disagreeing about a legal id.
pub fn is_valid_style_id(id: &str) -> bool {
    !id.is_empty()
        && id.chars().count() <= MAX_ID_CHARS
        && crate::core::slug::slugify_kebab(id) == id
}

/// Parse and bounds-check a `response_styles` document.
///
/// The write gate calls this, so every message it returns is read by a person
/// or handed to the agent. Nothing is truncated or dropped: a document is
/// wholly acceptable or wholly refused.
pub fn validate_document(raw: &str) -> Result<Vec<StyleEntry>, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }

    let entries: Vec<StyleEntry> = serde_json::from_str(trimmed).map_err(|e| {
        format!(
            "'{}' must be a JSON array of {{id, label, instruction}} objects: {}",
            PREF_RESPONSE_STYLES, e
        )
    })?;

    if entries.len() > MAX_STYLES {
        return Err(format!(
            "'{}' holds at most {} styles (got {})",
            PREF_RESPONSE_STYLES,
            MAX_STYLES,
            entries.len()
        ));
    }

    let mut seen = std::collections::HashSet::new();
    for entry in &entries {
        if !is_valid_style_id(&entry.id) {
            return Err(format!(
                "style id '{}' must be lowercase kebab-case, at most {} characters",
                entry.id, MAX_ID_CHARS
            ));
        }
        // Refused rather than ignored, so the mistake is named. `merge` stays
        // defensive anyway, for a row written before this gate existed.
        if entry.id == STANDARD_ID {
            return Err(format!(
                "'{}' is the off switch and cannot be edited or replaced",
                STANDARD_ID
            ));
        }
        if !seen.insert(entry.id.as_str()) {
            return Err(format!("style id '{}' appears twice", entry.id));
        }
        let label = entry.label.trim();
        if label.is_empty() || label.chars().count() > MAX_LABEL_CHARS {
            return Err(format!(
                "style '{}' needs a label of 1 to {} characters",
                entry.id, MAX_LABEL_CHARS
            ));
        }
        let instruction = entry.instruction.trim();
        if instruction.is_empty() || instruction.chars().count() > MAX_INSTRUCTION_CHARS {
            return Err(format!(
                "style '{}' needs an instruction of 1 to {} characters",
                entry.id, MAX_INSTRUCTION_CHARS
            ));
        }
    }
    Ok(entries)
}

/// Read a stored document, tolerating anything.
///
/// A value the gate would refuse today may already be in the table. A malformed
/// row must cost the user their styles rather than their turn.
fn read_document(raw: Option<&str>) -> Vec<StyleEntry> {
    let Some(raw) = raw else {
        return Vec::new();
    };
    match validate_document(raw) {
        Ok(entries) => entries,
        Err(e) => {
            log!(
                "[ResponseStyle] '{}' is unusable ({}). Falling back to the shipped styles",
                PREF_RESPONSE_STYLES,
                e
            );
            Vec::new()
        }
    }
}

/// Merge the shipped styles with the user's document.
///
/// A stored entry whose id is shipped replaces that row's label and
/// instruction; one with a fresh id is appended. Standard is skipped whatever
/// the document says.
pub fn merge(entries: &[StyleEntry]) -> Vec<Style> {
    let mut library: Vec<Style> = SHIPPED
        .iter()
        .map(|s| {
            let override_entry = entries.iter().find(|e| e.id == s.id && s.id != STANDARD_ID);
            match override_entry {
                Some(e) => Style {
                    id: s.id.to_string(),
                    label: e.label.trim().to_string(),
                    description: derive_description(&e.instruction),
                    instruction: e.instruction.trim().to_string(),
                    source: StyleSource::Overridden,
                    editable: true,
                },
                None => Style {
                    id: s.id.to_string(),
                    label: s.label.to_string(),
                    description: s.description.to_string(),
                    instruction: s.instruction.to_string(),
                    source: StyleSource::Builtin,
                    editable: s.id != STANDARD_ID,
                },
            }
        })
        .collect();

    for entry in entries {
        if SHIPPED.iter().any(|s| s.id == entry.id) {
            continue;
        }
        library.push(Style {
            id: entry.id.clone(),
            label: entry.label.trim().to_string(),
            description: derive_description(&entry.instruction),
            instruction: entry.instruction.trim().to_string(),
            source: StyleSource::User,
            editable: true,
        });
    }
    library
}

/// Longest derived description, so a picker row stays one line.
const MAX_DERIVED_DESCRIPTION_CHARS: usize = 80;

/// The one line a user style shows under its label: its first line of
/// instruction, without the bullet marker, clipped.
fn derive_description(instruction: &str) -> String {
    let first = instruction
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("");
    let first = first.trim_start_matches(['-', '*']).trim();
    if first.chars().count() <= MAX_DERIVED_DESCRIPTION_CHARS {
        return first.to_string();
    }
    // Counted in CHARACTERS, so cut in characters. `floor_char_boundary` takes
    // a BYTE index. Pairing it with the char count above clipped a Japanese
    // line to a third of its budget, and a Cyrillic one to half.
    let kept: String = first.chars().take(MAX_DERIVED_DESCRIPTION_CHARS).collect();
    format!("{}…", kept.trim_end())
}

/// Wrap an instruction into the prompt section, floor included.
///
/// It LEADS with a blank line rather than being followed by one. That is what
/// lets an empty section leave the surrounding prompt byte-identical to a build
/// carrying no style at all.
fn render(instruction: &str) -> String {
    let instruction = instruction.trim();
    if instruction.is_empty() {
        return String::new();
    }
    format!("\n\nRESPONSE STYLE:\n{}\n{}", instruction, STYLE_FLOOR)
}

/// The prompt section for `selected`, given a merged library.
///
/// An id the library does not hold resolves to nothing, which is Standard. That
/// is the state left behind by deleting the style you had selected.
pub fn section_for(library: &[Style], selected: &str) -> String {
    match library.iter().find(|s| s.id == selected) {
        Some(style) => render(&style.instruction),
        None => {
            log!(
                "[ResponseStyle] selected style '{}' is not in the library. Using Standard",
                selected
            );
            String::new()
        }
    }
}

/// Read one preference, treating a DB error as unset.
///
/// A turn that refuses to run because a style row is unreadable is strictly
/// worse than one that runs at the default. `build_chat_system_prompt` makes
/// the same judgment for its own mandatory keys.
async fn read_preference(pool: &PgPool, key: &str) -> Option<String> {
    match PreferenceStore::get(pool, key).await {
        Ok(value) => value,
        Err(e) => {
            log!(
                "[ResponseStyle] failed to read '{}': {}. Using Standard",
                key,
                e
            );
            None
        }
    }
}

/// This workspace's merged style library.
pub async fn library(pool: &PgPool) -> Vec<Style> {
    let document = read_preference(pool, PREF_RESPONSE_STYLES).await;
    merge(&read_document(document.as_deref()))
}

/// The response-style section for this turn's system prompt, or an empty string
/// when the workspace is on Standard.
///
/// Read once per turn, at prompt assembly. Both keys are workspace-global, so
/// the result is the same for every thread and the cached system tier stays
/// shared (ADR 0084).
pub async fn resolve(pool: &PgPool) -> String {
    let selected = read_preference(pool, PREF_RESPONSE_STYLE)
        .await
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| STANDARD_ID.to_string());
    if selected == STANDARD_ID {
        return String::new();
    }
    section_for(&library(pool).await, &selected)
}

/// The widest section the ENGINE authors, for the always-loaded budget meter.
///
/// A user's own instruction is workspace content, like `user_profile.md`, so it
/// is not on that meter. What is billed is the shipped text plus the wrapper
/// and the floor.
///
/// Test-only, because the meter is: production reads a real selection.
#[cfg(test)]
pub fn widest_shipped_section() -> String {
    SHIPPED
        .iter()
        .map(|s| render(s.instruction))
        .max_by_key(String::len)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, label: &str, instruction: &str) -> StyleEntry {
        StyleEntry {
            id: id.to_string(),
            label: label.to_string(),
            instruction: instruction.to_string(),
        }
    }

    fn find<'a>(library: &'a [Style], id: &str) -> &'a Style {
        library
            .iter()
            .find(|s| s.id == id)
            .unwrap_or_else(|| panic!("{id} must be in the library"))
    }

    /// The promise the whole feature rests on: a workspace that never opens
    /// this setting pays nothing and reads nothing new.
    #[test]
    fn standard_renders_nothing() {
        let library = merge(&[]);
        assert_eq!(section_for(&library, STANDARD_ID), "");
        assert_eq!(find(&library, STANDARD_ID).instruction, "");
    }

    /// Standard is the off switch, so it is the one row with no editor.
    #[test]
    fn standard_is_the_only_row_that_cannot_be_edited() {
        for style in merge(&[]) {
            assert_eq!(
                style.editable,
                style.id != STANDARD_ID,
                "{} has the wrong editability",
                style.id
            );
        }
    }

    /// Belt and braces against the write gate. A row written before
    /// `validate_document` existed must not make the off switch speak.
    #[test]
    fn a_stored_override_cannot_make_standard_inject_text() {
        let library = merge(&[entry(STANDARD_ID, "Loud", "- Say everything twice.")]);
        let standard = find(&library, STANDARD_ID);

        assert_eq!(standard.instruction, "");
        assert_eq!(standard.label, "Standard");
        assert_eq!(standard.source, StyleSource::Builtin);
        assert_eq!(section_for(&library, STANDARD_ID), "");
    }

    #[test]
    fn the_shipped_styles_are_standard_concise_and_minimal() {
        let ids: Vec<String> = merge(&[]).into_iter().map(|s| s.id).collect();
        assert_eq!(ids, vec![STANDARD_ID, "concise", "minimal"]);
    }

    /// Every style the prompt can carry ends with the rail, however it was
    /// authored. That is what stops "answer in one line" dropping a warning.
    #[test]
    fn every_rendered_style_ends_with_the_floor() {
        let library = merge(&[
            entry("concise", "Concise", "- My own take on concise."),
            entry("board-report", "Board report", "- Three bullets, no more."),
        ]);

        for style in &library {
            let section = section_for(&library, &style.id);
            if style.id == STANDARD_ID {
                assert_eq!(section, "", "Standard must stay silent");
                continue;
            }
            assert!(
                section.ends_with(STYLE_FLOOR),
                "{} rendered without the floor: {section}",
                style.id
            );
            assert!(section.starts_with("\n\nRESPONSE STYLE:\n"));
        }
    }

    /// A shipped style keeps its own description. An edited or user-written one
    /// derives a line from its instruction, having no description field.
    #[test]
    fn editing_a_shipped_style_overrides_it_and_leaves_the_rest_alone() {
        let library = merge(&[entry("minimal", "Terse", "- One sentence. Nothing else.")]);

        let minimal = find(&library, "minimal");
        assert_eq!(minimal.label, "Terse");
        assert_eq!(minimal.source, StyleSource::Overridden);
        assert!(minimal.editable);
        assert_eq!(minimal.description, "One sentence. Nothing else.");

        let concise = find(&library, "concise");
        assert_eq!(concise.source, StyleSource::Builtin);
        assert!(concise.instruction.contains("Lead with the answer"));
    }

    /// Removing the override is the Reset button. It brings the shipped text
    /// back rather than a blank.
    #[test]
    fn dropping_an_override_restores_the_shipped_text() {
        let edited = merge(&[entry("concise", "Mine", "- Mine.")]);
        let reset = merge(&[]);

        assert_ne!(
            find(&edited, "concise").instruction,
            find(&reset, "concise").instruction
        );
        assert_eq!(find(&reset, "concise").label, "Concise");
        assert_eq!(find(&reset, "concise").source, StyleSource::Builtin);
    }

    #[test]
    fn a_user_style_is_appended_after_the_shipped_ones() {
        let library = merge(&[entry("board-report", "Board report", "- Three bullets.")]);

        assert_eq!(library.len(), 4);
        let mine = find(&library, "board-report");
        assert_eq!(mine.source, StyleSource::User);
        assert!(mine.editable);
        assert!(section_for(&library, "board-report").contains("Three bullets."));
    }

    /// The state left behind by deleting the style you had selected.
    #[test]
    fn a_selected_id_nobody_defines_resolves_to_standard() {
        assert_eq!(section_for(&merge(&[]), "deleted-last-week"), "");
    }

    #[test]
    fn an_absent_or_blank_document_yields_the_shipped_library() {
        assert_eq!(read_document(None), Vec::new());
        assert_eq!(read_document(Some("   ")), Vec::new());
        assert_eq!(merge(&read_document(None)).len(), SHIPPED.len());
    }

    /// Unparseable JSON costs the user their styles, never their turn.
    #[test]
    fn an_unreadable_document_falls_back_rather_than_failing() {
        assert_eq!(read_document(Some("{not json")), Vec::new());
        assert_eq!(read_document(Some("{\"id\":\"x\"}")), Vec::new());
    }

    #[test]
    fn the_gate_accepts_a_well_formed_document() {
        let raw = serde_json::to_string(&vec![entry(
            "board-report",
            "Board report",
            "- Three bullets, no more.",
        )])
        .unwrap();
        assert_eq!(validate_document(&raw).unwrap().len(), 1);
        assert_eq!(validate_document("").unwrap(), Vec::new());
        assert_eq!(validate_document("[]").unwrap(), Vec::new());
    }

    #[test]
    fn the_gate_refuses_a_malformed_id() {
        for bad in [
            "",
            "Board Report",
            "board_report",
            "-lead",
            "trail-",
            "a--b",
        ] {
            assert!(!is_valid_style_id(bad), "{bad} must be refused");
        }
        for good in ["a", "board-report", "style-2"] {
            assert!(is_valid_style_id(good), "{good} must be accepted");
        }

        let raw = serde_json::to_string(&vec![entry("Board Report", "X", "- y")]).unwrap();
        assert!(validate_document(&raw).unwrap_err().contains("kebab-case"));
    }

    #[test]
    fn the_gate_refuses_an_entry_claiming_the_off_switch() {
        let raw = serde_json::to_string(&vec![entry(STANDARD_ID, "Loud", "- Twice.")]).unwrap();
        assert!(validate_document(&raw).unwrap_err().contains("off switch"));
    }

    #[test]
    fn the_gate_refuses_a_duplicate_id() {
        let raw = serde_json::to_string(&vec![
            entry("mine", "One", "- a"),
            entry("mine", "Two", "- b"),
        ])
        .unwrap();
        assert!(validate_document(&raw).unwrap_err().contains("twice"));
    }

    #[test]
    fn the_gate_refuses_values_past_their_bounds() {
        let long_label = "x".repeat(MAX_LABEL_CHARS + 1);
        let raw = serde_json::to_string(&vec![entry("mine", &long_label, "- a")]).unwrap();
        assert!(validate_document(&raw).unwrap_err().contains("label"));

        let long_instruction = "x".repeat(MAX_INSTRUCTION_CHARS + 1);
        let raw = serde_json::to_string(&vec![entry("mine", "Mine", &long_instruction)]).unwrap();
        assert!(validate_document(&raw).unwrap_err().contains("instruction"));

        let raw = serde_json::to_string(&vec![entry("mine", "Mine", "   ")]).unwrap();
        assert!(validate_document(&raw).unwrap_err().contains("instruction"));

        let many: Vec<StyleEntry> = (0..=MAX_STYLES)
            .map(|i| entry(&format!("style-{i}"), "S", "- a"))
            .collect();
        let raw = serde_json::to_string(&many).unwrap();
        assert!(validate_document(&raw).unwrap_err().contains("at most"));
    }

    /// The bounds count CHARACTERS, so a multi-byte instruction is not refused
    /// for being long in bytes.
    #[test]
    fn the_bounds_count_characters_rather_than_bytes() {
        let wide = "é".repeat(MAX_INSTRUCTION_CHARS);
        assert!(wide.len() > MAX_INSTRUCTION_CHARS);
        let raw = serde_json::to_string(&vec![entry("mine", "Mine", &wide)]).unwrap();
        assert!(validate_document(&raw).is_ok());
    }

    #[test]
    fn a_derived_description_is_one_clipped_line_without_its_bullet() {
        assert_eq!(
            derive_description("- Three bullets.\n- More."),
            "Three bullets."
        );
        assert_eq!(derive_description(""), "");

        let long = format!("- {}", "word ".repeat(40));
        let derived = derive_description(&long);
        assert!(derived.ends_with('…'));
        assert!(derived.chars().count() <= MAX_DERIVED_DESCRIPTION_CHARS + 1);
    }

    /// The meter bills engine-authored text, so it must see the widest SHIPPED
    /// section and not an empty one.
    #[test]
    fn the_widest_shipped_section_is_a_real_rendered_section() {
        let widest = widest_shipped_section();
        assert!(widest.starts_with("\n\nRESPONSE STYLE:\n"));
        assert!(widest.ends_with(STYLE_FLOOR));
        for shipped in SHIPPED {
            assert!(render(shipped.instruction).len() <= widest.len());
        }
    }

    /// The link the pure tests above cannot reach: two preference rows in, one
    /// prompt section out. It is the only part of the chain the compiler does
    /// not check, and a key spelled two ways would pass everything else.
    #[tokio::test]
    async fn the_resolver_reads_both_keys_and_degrades_to_standard() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let seed = |key: &'static str, value: String| {
            let pool = pool.clone();
            async move {
                crate::test_support::seed_preference(&pool, key, &value)
                    .await
                    .unwrap()
            }
        };

        // A fresh workspace has neither row, so it is on Standard.
        assert_eq!(resolve(&pool).await, "");

        // A shipped style, selected. The section is the shipped text.
        seed(PREF_RESPONSE_STYLE, "minimal".to_string()).await;
        let minimal = resolve(&pool).await;
        assert!(minimal.contains("Answer and stop."));
        assert!(minimal.ends_with(STYLE_FLOOR));

        // Overridden, so the user's words replace the shipped ones.
        let document = serde_json::to_string(&vec![entry(
            "minimal",
            "Terse",
            "- One sentence, and only one.",
        )])
        .unwrap();
        seed(PREF_RESPONSE_STYLES, document).await;
        let overridden = resolve(&pool).await;
        assert!(overridden.contains("One sentence, and only one."));
        assert!(!overridden.contains("Answer and stop."));
        assert!(overridden.ends_with(STYLE_FLOOR));

        // A style of their own, selected by its id.
        let document = serde_json::to_string(&vec![entry(
            "board-report",
            "Board report",
            "- Three bullets, no more.",
        )])
        .unwrap();
        seed(PREF_RESPONSE_STYLES, document).await;
        seed(PREF_RESPONSE_STYLE, "board-report".to_string()).await;
        assert!(resolve(&pool).await.contains("Three bullets, no more."));

        // Deleting the style you had selected. The id survives, the row does
        // not, and the turn still runs.
        seed(PREF_RESPONSE_STYLES, "[]".to_string()).await;
        assert_eq!(resolve(&pool).await, "");

        // A document written before the gate existed costs the styles, not the
        // turn: the shipped library answers and the selection still resolves.
        seed(PREF_RESPONSE_STYLES, "{not json".to_string()).await;
        seed(PREF_RESPONSE_STYLE, "concise".to_string()).await;
        assert!(resolve(&pool).await.contains("Lead with the answer."));

        // Explicitly back to Standard, which is silent again.
        seed(PREF_RESPONSE_STYLE, STANDARD_ID.to_string()).await;
        assert_eq!(resolve(&pool).await, "");

        // A blank selection reads as unset rather than as a missing style.
        seed(PREF_RESPONSE_STYLE, "   ".to_string()).await;
        assert_eq!(resolve(&pool).await, "");

        pool.close().await;
        crate::test_support::teardown_test_db(&db_name).await;
    }

    /// What the Settings editor renders, straight from the engine, so the
    /// shipped text has one home rather than a mirror in TypeScript.
    #[tokio::test]
    async fn the_library_reports_each_row_source_for_the_editor() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;

        let shipped = library(&pool).await;
        assert_eq!(shipped.len(), SHIPPED.len());
        assert!(shipped.iter().all(|s| s.source == StyleSource::Builtin));

        let document = serde_json::to_string(&vec![
            entry("concise", "Mine", "- Mine."),
            entry("board-report", "Board report", "- Three bullets."),
        ])
        .unwrap();
        crate::test_support::seed_preference(&pool, PREF_RESPONSE_STYLES, &document)
            .await
            .unwrap();

        let merged = library(&pool).await;
        assert_eq!(find(&merged, STANDARD_ID).source, StyleSource::Builtin);
        assert_eq!(find(&merged, "concise").source, StyleSource::Overridden);
        assert_eq!(find(&merged, "minimal").source, StyleSource::Builtin);
        assert_eq!(find(&merged, "board-report").source, StyleSource::User);

        pool.close().await;
        crate::test_support::teardown_test_db(&db_name).await;
    }
}
