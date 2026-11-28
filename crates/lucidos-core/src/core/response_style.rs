use serde::{Deserialize, Serialize};
use sqlx::PgPool;

use crate::core::prefs;
use crate::core::technical_literacy::{self, TechnicalLiteracy};

pub const STANDARD_ID: &str = "standard";

pub const MAX_ID_CHARS: usize = 40;
pub const MAX_LABEL_CHARS: usize = 40;
pub const MAX_INSTRUCTION_CHARS: usize = 1_000;
pub const MAX_STYLES: usize = 20;

/// INVARIANT: The one rail an editable instruction cannot remove.
///
/// Appended to every rendered style, shipped or user-written, outside the text
/// the user controls. "Answer in one line" is a reasonable thing to ask for and
/// a dangerous thing to obey without this.
const STYLE_FLOOR: &str = "- Style never overrides substance. Keep every warning, \
     every caveat that changes the answer, and every step the user has to take. \
     When they ask why or how, the explanation IS the answer, so give it in full.";

struct ShippedStyle {
    id: &'static str,
    label: &'static str,
    description: &'static str,
    instruction: &'static str,
}

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
        description: "The outcome, not the process, and no follow-up question.",
        instruction:
            "- Give the outcome, not the process: what you found, what changed, or what is done. Leave out how you got there.\n\
             - One or two sentences, or a short list when the answer is a list. No preamble, no recap, no offer of next steps.\n\
             - Do not ask the follow-up in CONVERSATION STYLE unless you cannot answer without it.",
    },
    ShippedStyle {
        id: "learning",
        label: "Learning",
        description: "Explains the why as it goes, so you learn how things work.",
        instruction:
            "- Explain the why as you go: what you are doing, why this way, and the idea behind it.\n\
             - Introduce one new idea at a time, and connect it to something the user already knows.\n\
             - End with the one thing worth remembering from this answer.",
    },
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StyleEntry {
    pub id: String,
    pub label: String,
    pub instruction: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum StyleSource {
    Builtin,
    Overridden,
    User,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Style {
    pub id: String,
    pub label: String,
    pub description: String,
    pub instruction: String,
    pub source: StyleSource,
    pub editable: bool,
}

pub fn is_valid_style_id(id: &str) -> bool {
    !id.is_empty()
        && id.chars().count() <= MAX_ID_CHARS
        && crate::core::slug::slugify_kebab(id) == id
}

pub fn validate_document(raw: &str) -> Result<Vec<StyleEntry>, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }

    let entries: Vec<StyleEntry> = serde_json::from_str(trimmed).map_err(|e| {
        format!(
            "'{}' must be a JSON array of {{id, label, instruction}} objects: {}",
            prefs::RESPONSE_STYLES.key(),
            e
        )
    })?;

    if entries.len() > MAX_STYLES {
        return Err(format!(
            "'{}' holds at most {} styles (got {})",
            prefs::RESPONSE_STYLES.key(),
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

fn read_document(raw: Option<&str>) -> Vec<StyleEntry> {
    let Some(raw) = raw else {
        return Vec::new();
    };
    match validate_document(raw) {
        Ok(entries) => entries,
        Err(e) => {
            log_legacy!(
                "[ResponseStyle] '{}' is unusable ({}). Falling back to the shipped styles",
                prefs::RESPONSE_STYLES.key(),
                e
            );
            Vec::new()
        }
    }
}

pub fn merge(entries: &[StyleEntry]) -> Vec<Style> {
    let mut library: Vec<Style> = SHIPPED
        .iter()
        .map(|s| {
            let override_entry = entries.iter().find(|e| e.id == s.id && s.id != STANDARD_ID);
            match override_entry {
                Some(e) => Style {
                    id: s.id.to_string(),
                    label: e.label.trim().to_string(),
                    description: s.description.to_string(),
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

const MAX_DERIVED_DESCRIPTION_CHARS: usize = 80;

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
    // INVARIANT: Counted in CHARACTERS, so cut in characters. `floor_char_boundary` takes
    // a BYTE index. Pairing it with the char count above clipped a Japanese
    // line to a third of its budget, and a Cyrillic one to half.
    let kept: String = first.chars().take(MAX_DERIVED_DESCRIPTION_CHARS).collect();
    format!("{}…", kept.trim_end())
}

/// INVARIANT: Wrap both parts into the prompt section, floor included once, at the end.
///
/// It LEADS with a blank line rather than being followed by one. That is what
/// lets an empty section leave the surrounding prompt byte-identical to a build
/// carrying no style at all.
fn render(instruction: &str, literacy: Option<TechnicalLiteracy>) -> String {
    let mut parts: Vec<String> = Vec::new();
    let instruction = instruction.trim();
    if !instruction.is_empty() {
        parts.push(instruction.to_string());
    }
    if let Some(level) = literacy {
        parts.push(level.chat_rules());
    }
    if parts.is_empty() {
        return String::new();
    }
    format!("\n\nRESPONSE STYLE:\n{}\n{}", parts.join("\n"), STYLE_FLOOR)
}

pub fn section_for(
    library: &[Style],
    selected: &str,
    literacy: Option<TechnicalLiteracy>,
) -> String {
    let instruction = match library.iter().find(|s| s.id == selected) {
        Some(style) => style.instruction.as_str(),
        None => {
            log_legacy!(
                "[ResponseStyle] selected style '{}' is not in the library. Using Standard",
                selected
            );
            ""
        }
    };
    render(instruction, literacy)
}

pub async fn library(pool: &PgPool) -> Vec<Style> {
    let document = prefs::RESPONSE_STYLES.stored(pool).await;
    merge(&read_document(document.as_deref()))
}

pub async fn resolve(pool: &PgPool) -> String {
    let literacy = technical_literacy::read(pool).await;
    let selected = prefs::RESPONSE_STYLE.read(pool).await;
    if selected == STANDARD_ID {
        return render("", literacy);
    }
    section_for(&library(pool).await, &selected, literacy)
}

pub async fn user_written_selection(pool: &PgPool) -> Option<Style> {
    let selected = prefs::RESPONSE_STYLE.read(pool).await;
    library(pool)
        .await
        .into_iter()
        .find(|s| s.id == selected && s.source != StyleSource::Builtin)
}

pub fn widest_shipped_section() -> String {
    let widest_style = SHIPPED
        .iter()
        .map(|s| s.instruction)
        .max_by_key(|i| i.len())
        .unwrap_or_default();
    let widest_level = TechnicalLiteracy::ALL
        .into_iter()
        .max_by_key(|l| l.chat_rules().len())
        .expect("there are literacy levels");
    render(widest_style, Some(widest_level))
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

    #[test]
    fn standard_renders_nothing() {
        let library = merge(&[]);
        assert_eq!(section_for(&library, STANDARD_ID, None), "");
        assert_eq!(find(&library, STANDARD_ID).instruction, "");
    }

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

    #[test]
    fn a_stored_override_cannot_make_standard_inject_text() {
        let library = merge(&[entry(STANDARD_ID, "Loud", "- Say everything twice.")]);
        let standard = find(&library, STANDARD_ID);

        assert_eq!(standard.instruction, "");
        assert_eq!(standard.label, "Standard");
        assert_eq!(standard.source, StyleSource::Builtin);
        assert_eq!(section_for(&library, STANDARD_ID, None), "");
    }

    #[test]
    fn the_shipped_styles_are_standard_concise_minimal_and_learning() {
        let ids: Vec<String> = merge(&[]).into_iter().map(|s| s.id).collect();
        assert_eq!(ids, vec![STANDARD_ID, "concise", "minimal", "learning"]);
    }

    #[test]
    fn every_rendered_style_ends_with_the_floor() {
        let library = merge(&[
            entry("concise", "Concise", "- My own take on concise."),
            entry("board-report", "Board report", "- Three bullets, no more."),
        ]);

        for style in &library {
            let section = section_for(&library, &style.id, None);
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

    #[test]
    fn editing_a_shipped_style_overrides_it_and_leaves_the_rest_alone() {
        let library = merge(&[entry("minimal", "Terse", "- One sentence. Nothing else.")]);

        let minimal = find(&library, "minimal");
        assert_eq!(minimal.label, "Terse");
        assert_eq!(minimal.source, StyleSource::Overridden);
        assert!(minimal.editable);
        let shipped = SHIPPED.iter().find(|s| s.id == "minimal").unwrap();
        assert_eq!(minimal.description, shipped.description);

        let concise = find(&library, "concise");
        assert_eq!(concise.source, StyleSource::Builtin);
        assert!(concise.instruction.contains("Lead with the answer"));
    }

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

        assert_eq!(library.len(), SHIPPED.len() + 1);
        let mine = find(&library, "board-report");
        assert_eq!(mine.source, StyleSource::User);
        assert!(mine.editable);
        assert!(section_for(&library, "board-report", None).contains("Three bullets."));
    }

    #[test]
    fn a_selected_id_nobody_defines_resolves_to_standard() {
        assert_eq!(section_for(&merge(&[]), "deleted-last-week", None), "");
    }

    #[test]
    fn style_and_literacy_render_independently_under_one_heading() {
        let library = merge(&[]);
        let levels = std::iter::once(None).chain(TechnicalLiteracy::ALL.into_iter().map(Some));
        for literacy in levels {
            for style in &library {
                let section = section_for(&library, &style.id, literacy);
                if style.instruction.is_empty() && literacy.is_none() {
                    assert_eq!(section, "", "Standard with no level must stay silent");
                    continue;
                }
                assert_eq!(section.matches("RESPONSE STYLE:").count(), 1);
                assert_eq!(section.matches(STYLE_FLOOR).count(), 1);
                assert!(section.ends_with(STYLE_FLOOR));
                assert!(section.contains(&style.instruction));
                if let Some(level) = literacy {
                    assert!(
                        section.contains(&level.chat_rules()),
                        "{level:?} is missing"
                    );
                }
            }
        }
    }

    #[test]
    fn a_missing_style_keeps_the_literacy_part() {
        let section = section_for(
            &merge(&[]),
            "deleted-last-week",
            Some(TechnicalLiteracy::Developer),
        );
        assert!(section.contains(&TechnicalLiteracy::Developer.chat_rules()));
    }

    #[test]
    fn an_absent_or_blank_document_yields_the_shipped_library() {
        assert_eq!(read_document(None), Vec::new());
        assert_eq!(read_document(Some("   ")), Vec::new());
        assert_eq!(merge(&read_document(None)).len(), SHIPPED.len());
    }

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

    #[test]
    fn the_widest_shipped_section_is_a_real_rendered_section() {
        let widest = widest_shipped_section();
        assert!(widest.starts_with("\n\nRESPONSE STYLE:\n"));
        assert!(widest.ends_with(STYLE_FLOOR));
        for shipped in SHIPPED {
            assert!(render(shipped.instruction, None).len() <= widest.len());
        }
    }

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

        assert_eq!(resolve(&pool).await, "");

        seed(prefs::RESPONSE_STYLE.key(), "minimal".to_string()).await;
        let minimal = resolve(&pool).await;
        assert!(minimal.contains("Give the outcome, not the process"));
        assert!(minimal.ends_with(STYLE_FLOOR));

        let document = serde_json::to_string(&vec![entry(
            "minimal",
            "Terse",
            "- One sentence, and only one.",
        )])
        .unwrap();
        seed(prefs::RESPONSE_STYLES.key(), document).await;
        let overridden = resolve(&pool).await;
        assert!(overridden.contains("One sentence, and only one."));
        assert!(!overridden.contains("Give the outcome, not the process"));
        assert!(overridden.ends_with(STYLE_FLOOR));

        let document = serde_json::to_string(&vec![entry(
            "board-report",
            "Board report",
            "- Three bullets, no more.",
        )])
        .unwrap();
        seed(prefs::RESPONSE_STYLES.key(), document).await;
        seed(prefs::RESPONSE_STYLE.key(), "board-report".to_string()).await;
        assert!(resolve(&pool).await.contains("Three bullets, no more."));

        seed(prefs::RESPONSE_STYLES.key(), "[]".to_string()).await;
        assert_eq!(resolve(&pool).await, "");

        seed(prefs::RESPONSE_STYLES.key(), "{not json".to_string()).await;
        seed(prefs::RESPONSE_STYLE.key(), "concise".to_string()).await;
        assert!(resolve(&pool).await.contains("Lead with the answer."));

        seed(prefs::RESPONSE_STYLE.key(), STANDARD_ID.to_string()).await;
        assert_eq!(resolve(&pool).await, "");

        seed(prefs::RESPONSE_STYLE.key(), "   ".to_string()).await;
        assert_eq!(resolve(&pool).await, "");

        seed(prefs::TECHNICAL_LITERACY.key(), "non-technical".to_string()).await;
        let literacy_only = resolve(&pool).await;
        assert!(literacy_only.contains(&TechnicalLiteracy::NonTechnical.chat_rules()));
        assert!(literacy_only.ends_with(STYLE_FLOOR));

        seed(prefs::RESPONSE_STYLE.key(), "minimal".to_string()).await;
        let both = resolve(&pool).await;
        assert!(both.contains("Give the outcome, not the process"));
        assert!(both.contains(&TechnicalLiteracy::NonTechnical.chat_rules()));
        assert_eq!(both.matches("RESPONSE STYLE:").count(), 1);

        seed(prefs::TECHNICAL_LITERACY.key(), "wizard".to_string()).await;
        let unknown = resolve(&pool).await;
        assert!(unknown.contains("Give the outcome, not the process"));
        assert!(!unknown.contains("The user is"));

        pool.close().await;
        crate::test_support::teardown_test_db(&db_name).await;
    }

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
        crate::test_support::seed_preference(&pool, prefs::RESPONSE_STYLES.key(), &document)
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
