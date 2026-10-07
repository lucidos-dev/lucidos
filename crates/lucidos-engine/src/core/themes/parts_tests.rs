use super::super::{parse_definition, resolve_mode, ThemeMode, TokenMap, CATALOG as TOKENS};
use super::*;

#[derive(Deserialize)]
struct Cases {
    valid: Vec<ValidCase>,
    invalid: Vec<InvalidCase>,
}

#[derive(Deserialize)]
struct ValidCase {
    token: String,
    value: String,
    canonical: String,
}

#[derive(Deserialize)]
struct InvalidCase {
    token: String,
    value: String,
    error: String,
}

/// The same fixture drives `themeParts.test.ts`, so the engine and every apply
/// site accept, refuse and canonicalise alike.
#[test]
fn the_grammar_matches_the_shared_fixture() {
    let cases: Cases = serde_json::from_str(include_str!("theme-part-cases.json")).unwrap();
    for case in &cases.valid {
        assert_eq!(
            check_part_token(&case.token, &case.value).as_deref(),
            Ok(case.canonical.as_str()),
            "{} = {}",
            case.token,
            case.value
        );
    }
    for case in &cases.invalid {
        assert_eq!(
            check_part_token(&case.token, &case.value),
            Err(case.error.clone()),
            "{} = {}",
            case.token,
            case.value
        );
    }
}

#[test]
fn a_canonical_value_is_its_own_canonical_form() {
    let cases: Cases = serde_json::from_str(include_str!("theme-part-cases.json")).unwrap();
    for case in &cases.valid {
        assert_eq!(
            check_part_token(&case.token, &case.canonical).as_deref(),
            Ok(case.canonical.as_str())
        );
    }
}

// --- The catalog ---

#[test]
fn every_part_property_is_declared_and_named_by_its_token() {
    let mut seen = std::collections::HashSet::new();
    for (part, usage) in CATALOG.tokens() {
        assert!(
            CATALOG.properties.contains_key(&usage.name),
            "{}.{} is not a declared property",
            part.id,
            usage.name
        );
        assert_eq!(usage.token, part_token(&part.id, &usage.name));
        assert!(seen.insert(usage.token.clone()), "{} twice", usage.token);
    }
}

#[test]
fn part_ids_are_kebab_case_and_never_a_protected_surface() {
    for part in &CATALOG.parts {
        assert!(super::super::validate_id(&part.id).is_ok(), "{}", part.id);
        assert!(!CATALOG.protected.iter().any(|p| p.id == part.id));
    }
}

#[test]
fn a_parent_is_a_shell_part_the_child_sits_inside() {
    for part in &CATALOG.parts {
        let Some(parent) = &part.parent else { continue };
        let parent = CATALOG.part(parent).expect("the parent is a part");
        assert!(parent.parent.is_none(), "{} nests two deep", part.id);
        assert_eq!(parent.frames, part.frames, "{}", part.id);
        for usage in &part.properties {
            let inherits_from_parent =
                matches!(usage.name.as_str(), "text-shadow" | "letter-spacing");
            if inherits_from_parent && parent.property(&usage.name).is_some() {
                assert!(
                    usage.default.contains(&part_token(&parent.id, &usage.name)),
                    "{}.{} must fall back to its parent",
                    part.id,
                    usage.name
                );
            }
        }
    }
}

/// `filter` makes its element a stacking context and the containing block of
/// fixed descendants, so it goes on icons only.
#[test]
fn filter_sits_on_leaf_icons_only() {
    for (part, usage) in CATALOG.tokens() {
        if usage.name == "filter" {
            assert!(
                part.selector.split(", ").all(|s| s.ends_with(" svg")),
                "{}",
                part.id
            );
        }
    }
}

/// The header and the composer can overlap scrolled content. The header takes
/// no box shadow, and the composer takes inset shadows only.
#[test]
fn a_part_that_can_overlap_content_takes_no_outer_shadow() {
    for (part, usage) in CATALOG.tokens() {
        if usage.name != "box-shadow" {
            continue;
        }
        assert!(!part.id.starts_with("header"), "{}", part.id);
        if part.id == "composer" {
            assert!(usage.inset_only);
        }
    }
}

#[test]
fn every_alias_is_a_catalog_token_and_every_target_takes_its_property() {
    for alias in &CATALOG.aliases {
        assert!(TOKENS.tokens.iter().any(|t| t.name == alias.token));
        for id in &alias.parts {
            let part = CATALOG.part(id).expect("an alias names parts");
            assert!(part.property(&alias.property).is_some(), "{id}");
        }
    }
}

/// A theme names catalog tokens only, so its largest map is the catalog, the
/// protected palette and every part token. That must fit the apply-site cap.
#[test]
fn the_largest_resolved_map_fits_the_apply_cap() {
    let largest = TOKENS.tokens.len() + PROTECTED_TOKENS.len() + 1 + CATALOG.tokens().count();
    assert!(largest <= super::super::MAX_RESOLVED_TOKENS, "{largest}");
}

use super::super::protected::PROTECTED_TOKENS;

// --- A theme's parts ---

fn resolved(json: &str) -> (TokenMap, TokenMap) {
    let def = parse_definition(json.as_bytes()).expect("valid theme");
    (
        resolve_mode(&def, ThemeMode::Dark, None),
        resolve_mode(&def, ThemeMode::Light, None),
    )
}

fn parts_of(map: &TokenMap) -> Vec<(&str, &str)> {
    map.iter()
        .filter(|(k, _)| k.starts_with(PART_TOKEN_PREFIX))
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect()
}

fn refusal(json: &str) -> String {
    parse_definition(json.as_bytes())
        .expect_err("the theme should be refused")
        .to_string()
}

const GLOW: &str = "--part-chat-text-text-shadow";
const SPACING: &str = "--part-header-title-letter-spacing";
const ICON: &str = "--part-actor-icon-filter";

#[test]
fn the_default_theme_sets_no_part_token() {
    let (dark, light) = resolved(r#"{"name": "Plain"}"#);
    assert!(parts_of(&dark).is_empty() && parts_of(&light).is_empty());
}

#[test]
fn shared_parts_paint_in_both_modes_in_canonical_form() {
    let (dark, light) = resolved(
        r#"{"name": "G", "parts": {"chat-text": {"text-shadow": "0 0 .3EM var(--accent)"}}}"#,
    );
    for map in [&dark, &light] {
        assert_eq!(parts_of(map), vec![(GLOW, "0 0 0.3em var(--accent)")]);
    }
}

#[test]
fn mode_parts_paint_in_their_mode_only() {
    let (dark, light) = resolved(
        r#"{"name": "G", "dark": {"parts": {"actor-icon": {"filter": "drop-shadow(0 0 3px var(--accent))"}}}}"#,
    );
    assert_eq!(
        parts_of(&dark),
        vec![(ICON, "drop-shadow(0 0 3px var(--accent))")]
    );
    assert!(parts_of(&light).is_empty());
}

#[test]
fn a_mode_value_replaces_one_property_and_keeps_the_rest_shared() {
    let (dark, light) = resolved(
        r#"{"name": "G",
            "parts": {"chat-text": {"text-shadow": "0 0 0.3em red", "letter-spacing": "0.05em"}},
            "light": {"parts": {"chat-text": {"text-shadow": "0 0 0.1em blue"}}}}"#,
    );
    assert_eq!(dark.get(GLOW).unwrap(), "0 0 0.3em red");
    assert_eq!(light.get(GLOW).unwrap(), "0 0 0.1em blue");
    for map in [&dark, &light] {
        assert_eq!(
            map.get("--part-chat-text-letter-spacing").unwrap(),
            "0.05em"
        );
    }
}

#[test]
fn a_mode_adds_a_property_to_a_shared_part() {
    let (dark, light) = resolved(
        r##"{"name": "G",
            "parts": {"chat-text": {"text-shadow": "0 0 0.3em red"}},
            "dark": {"parts": {"chat-text": {"color": "#33ff33"}}}}"##,
    );
    assert_eq!(dark.get("--part-chat-text-color").unwrap(), "#33ff33");
    assert!(!light.contains_key("--part-chat-text-color"));
    assert_eq!(light.get(GLOW).unwrap(), "0 0 0.3em red");
}

#[test]
fn none_in_a_mode_switches_a_shared_shadow_off() {
    let (dark, light) = resolved(
        r#"{"name": "G",
            "parts": {"chat-text": {"text-shadow": "0 0 0.3em red"}},
            "light": {"parts": {"chat-text": {"text-shadow": "none"}}}}"#,
    );
    assert_eq!(dark.get(GLOW).unwrap(), "0 0 0.3em red");
    assert_eq!(light.get(GLOW).unwrap(), "none");
}

#[test]
fn a_theme_with_no_map_for_a_mode_paints_its_shared_parts_there() {
    let (_, light) = resolved(
        r##"{"name": "G", "dark": {"--bg-primary": "#000000"},
            "parts": {"header-title": {"letter-spacing": "0.08em"}}}"##,
    );
    assert_eq!(parts_of(&light), vec![(SPACING, "0.08em")]);
}

#[test]
fn a_mode_map_of_parts_only_leaves_the_theme_in_both_modes() {
    let def = parse_definition(
        br#"{"name": "G", "dark": {"parts": {"chat-text": {"text-shadow": "0 0 0.3em red"}}}}"#,
    )
    .unwrap();
    assert_eq!(def.modes(), Vec::<ThemeMode>::new());
    let def = parse_definition(
        br##"{"name": "G", "dark": {"--bg-primary": "#000000", "parts": {"chat-text": {"text-shadow": "none"}}}}"##,
    )
    .unwrap();
    assert_eq!(def.modes(), vec![ThemeMode::Dark]);
}

#[test]
fn a_theme_file_round_trips_its_parts() {
    let json = r##"{"name":"G","dark":{"parts":{"chat-text":{"text-shadow":"none"}},"--bg-primary":"#000000"},"parts":{"composer":{"box-shadow":"inset 0 0 4px red"}}}"##;
    let def = parse_definition(json.as_bytes()).unwrap();
    let again = serde_json::to_string(&def).unwrap();
    assert_eq!(parse_definition(again.as_bytes()).unwrap(), def);
}

#[test]
fn every_refusal_names_the_field_the_rule_and_the_limit() {
    for (json, message) in [
        (
            r#"{"name":"x","parts":{"side-panel":{"color":"red"}}}"#,
            "parts.side-panel: not a part. GET /api/v1/themes/parts lists them.",
        ),
        (
            r#"{"name":"x","parts":{"permission-card":{"color":"red"}}}"#,
            "parts.permission-card: a protected surface, so a theme cannot style it.",
        ),
        (
            r#"{"name":"x","parts":{"header-title":{"opacity":"0"}}}"#,
            "parts.header-title.opacity: header-title takes text-shadow, letter-spacing.",
        ),
        (
            r#"{"name":"x","parts":{"chat-text":{"text-shadow":"0 0 2em red"}}}"#,
            "parts.chat-text.text-shadow: blur 2em is over the 0.6em cap.",
        ),
        (
            r#"{"name":"x","parts":{"composer":{"box-shadow":"inset 0 0 2em red"}}}"#,
            "parts.composer.box-shadow: use px for box-shadow lengths.",
        ),
        (
            r#"{"name":"x","parts":{"code-block":{"box-shadow":"0 0 1px red, 0 0 1px red, 0 0 1px red"}}}"#,
            "parts.code-block.box-shadow: 3 layers; the limit is 2.",
        ),
        (
            r#"{"name":"x","parts":{"composer":{"box-shadow":"0 0 4px red"}}}"#,
            "parts.composer.box-shadow: the composer takes inset shadows only.",
        ),
        (
            r#"{"name":"x","parts":{"inline-code":{"color":"var(--foo)"}}}"#,
            "parts.inline-code.color: var(--foo) is not a catalog colour token.",
        ),
        (
            r#"{"name":"x","parts":{"chat-text":{"color":"rgba(255,255,255,0.2)"}}}"#,
            "parts.chat-text.color: alpha 0.2 is under the 0.6 floor for text.",
        ),
        (
            r#"{"name":"x","parts":{"chat-text":{"text-shadow":"attr(x)"}}}"#,
            "parts.chat-text.text-shadow: attr() is not allowed here.",
        ),
        (
            r#"{"name":"x","dark":{"--part-chat-text-color":"red"}}"#,
            "`dark`: --part-chat-text-color is a part token. Set parts with `parts`.",
        ),
        (
            r#"{"name":"x","light":{"parts":{"chat-text":{"text-shadow":"0 0 2em red"}}}}"#,
            "light.parts.chat-text.text-shadow: blur 2em is over the 0.6em cap.",
        ),
        (
            r#"{"name":"x","dark":{"parts":"glow"}}"#,
            "dark.parts: must be a map of part ids to properties.",
        ),
        (
            r#"{"name":"x","dark":{"parts":{"chat-text":"glow"}}}"#,
            "dark.parts.chat-text: must be a map of properties to values.",
        ),
        (
            r#"{"name":"x","parts":{"chat-text":{"color":1}}}"#,
            "parts.chat-text.color: must be a string.",
        ),
        (
            r#"{"name":"x","dark":{"part":{}}}"#,
            "`dark`: 'part' is not a custom property name (--lowercase-kebab). Did you mean parts?",
        ),
    ] {
        assert_eq!(refusal(json), message, "{json}");
    }
}

#[test]
fn a_colour_token_must_hold_a_colour() {
    let message = refusal(r#"{"name":"x","tokens":{"--accent":"red, 9em 9em 5em red"}}"#);
    assert!(message.contains("--accent is a colour token"), "{message}");
    for ok in [
        "transparent",
        "var(--text-primary)",
        "color-mix(in srgb, #58a6ff 40%, transparent)",
    ] {
        let json = format!(r#"{{"name":"x","tokens":{{"--accent-light":"{ok}"}}}}"#);
        assert!(parse_definition(json.as_bytes()).is_ok(), "{ok}");
    }
}

// --- The --text-glow alias ---

const TEXT_GLOW_TARGETS: [&str; 4] = [
    "--part-chat-text-text-shadow",
    "--part-actor-label-text-shadow",
    "--part-header-title-text-shadow",
    "--part-composer-text-text-shadow",
];

#[test]
fn a_theme_that_sets_only_text_glow_still_glows() {
    let (dark, light) =
        resolved(r#"{"name":"G","dark":{"--text-glow":"0 0 0.25rem rgba(51, 255, 51, 0.55)"}}"#);
    for token in TEXT_GLOW_TARGETS {
        assert_eq!(
            dark.get(token).map(String::as_str),
            Some("0 0 0.25em rgba(51, 255, 51, 0.55)"),
            "{token}"
        );
    }
    assert!(
        !dark.contains_key("--text-glow"),
        "the alias leaves the map"
    );
    assert!(parts_of(&light).is_empty());
}

#[test]
fn an_explicit_part_value_wins_over_the_alias() {
    let (dark, _) = resolved(
        r#"{"name":"G","tokens":{"--text-glow":"0 0 0.3em red"},
            "parts":{"header-title":{"text-shadow":"none"}}}"#,
    );
    assert_eq!(dark.get("--part-header-title-text-shadow").unwrap(), "none");
    assert_eq!(dark.get(GLOW).unwrap(), "0 0 0.3em red");
}

#[test]
fn a_text_glow_in_px_compiles_as_em() {
    let (dark, _) = resolved(r##"{"name":"G","tokens":{"--text-glow":"0 0 8px #33ff33"}}"##);
    assert_eq!(dark.get(GLOW).unwrap(), "0 0 0.5em #33ff33");
}

#[test]
fn a_part_colour_its_background_hides_is_refused() {
    let message = refusal(r#"{"name":"x","parts":{"chat-text":{"color":"var(--bg-primary)"}}}"#);
    assert_eq!(
        message,
        "in dark mode, the chat-text color is 1.0:1 on --bg-primary. Text needs at least 3:1 against its background."
    );
    let message = refusal(
        r##"{"name":"x","parts":{"inline-code":{"color":"#101010","background-color":"#111111"}}}"##,
    );
    assert!(message.contains("inline-code color"), "{message}");
    assert!(
        message.contains("--part-inline-code-background-color"),
        "{message}"
    );
}

// --- Retro parts (ADR 0313) ---

/// The screen part paints on the fills that hold the whole shell, protected
/// surfaces included, so nothing it sets may inherit into them.
#[test]
fn the_screen_part_takes_no_inherited_property() {
    let screen = CATALOG.part("screen").expect("the screen part");
    for usage in &screen.properties {
        assert!(!CATALOG.property(&usage.name).inherits, "{}", usage.name);
    }
}

#[test]
fn scanlines_that_pull_page_text_under_the_floor_are_refused() {
    let message = refusal(
        r##"{"name":"x","dark":{"--bg-primary":"#000000","--text-primary":"#777777"},
            "parts":{"screen":{"background-image":"repeating-linear-gradient(transparent 0, rgba(255, 255, 255, 0.25) 2px)"}}}"##,
    );
    assert_eq!(
        message,
        "in dark mode, --text-primary is 2.3:1 on --bg-primary under the screen scanlines. Text needs at least 3:1 against its background."
    );
}

#[test]
fn scanlines_that_pull_a_part_colour_under_the_floor_are_refused() {
    // #666666 clears 3:1 on the black page, but not on a pale band.
    let message = refusal(
        r##"{"name":"x","dark":{"--bg-primary":"#000000"},
            "parts":{"chat-text":{"color":"#666666"},
                     "screen":{"background-image":"repeating-linear-gradient(transparent 0, rgba(255, 255, 255, 0.25) 2px)"}}}"##,
    );
    assert_eq!(
        message,
        "in dark mode, the chat-text color is 1.8:1 on --bg-primary under the screen scanlines. Text needs at least 3:1 against its background."
    );
}

#[test]
fn a_phosphor_theme_with_every_retro_part_resolves() {
    let (dark, _) = resolved(
        r##"{"name":"Phosphor","dark":{"--bg-primary":"#050805","--text-primary":"#33ff33"},
            "parts":{
              "screen":{"background-image":"repeating-linear-gradient(transparent 0, transparent 2px, rgba(51, 255, 51, 0.08) 2px, rgba(51, 255, 51, 0.08) 3px)"},
              "composer-text":{"caret-shape":"block","caret-color":"#33ff33"},
              "surface":{"border-style":"double","border-width":"3px","border-color":"#0f4d0f"}}}"##,
    );
    assert_eq!(dark["--part-composer-text-caret-shape"], "block");
    assert_eq!(dark["--part-surface-border-style"], "double");
    assert_eq!(dark["--part-surface-border-width"], "3px");
    assert!(dark["--part-screen-background-image"].starts_with("repeating-linear-gradient("));
}

/// A protected surface draws no fill of its own, so its text may sit on a
/// scanline band. The clamp keeps every protected text colour at 4.5:1 there.
#[test]
fn protected_text_clears_every_scanline_band() {
    // Muted text at the page floor, and pale bands that pull the page
    // toward it: without the bands in the clamp, protected text drops to
    // about 3.6:1 on them.
    let (dark, _) = resolved(
        r##"{"name":"x","dark":{"--bg-primary":"#000000","--text-primary":"#767676","--text-secondary":"#767676"},
            "parts":{"screen":{"background-image":"repeating-linear-gradient(rgba(118, 118, 118, 0.25) 0, rgba(118, 118, 118, 0.25) 2px)"}}}"##,
    );
    let page = super::super::color::evaluate("#000000", &|_| None).unwrap();
    let band = super::super::color::evaluate("rgba(118, 118, 118, 0.25)", &|_| None)
        .unwrap()
        .over(page);
    for token in [
        "--protected-text",
        "--protected-text-muted",
        "--protected-accent",
        "--protected-caution",
        "--protected-confirm-text",
        "--protected-danger-text",
    ] {
        let text = super::super::color::evaluate(&dark[token], &|_| None).unwrap();
        let ratio = super::super::color::contrast(text, band);
        assert!(ratio >= 4.5, "{token} is {ratio:.2}:1 on a band");
    }
}

#[test]
fn a_theme_that_sets_only_scanlines_gets_a_clamped_palette() {
    let (dark, light) = resolved(
        r#"{"name":"x","parts":{"screen":{"background-image":"repeating-linear-gradient(transparent 0, #0000001a 2px)"}}}"#,
    );
    for map in [&dark, &light] {
        assert!(map.contains_key("--protected-text"), "{map:?}");
    }
    let (plain, _) = resolved(r#"{"name":"x","parts":{"chat-text":{"text-shadow":"none"}}}"#);
    assert!(!plain.contains_key("--protected-text"));
}

#[test]
fn a_style_override_never_sets_the_scanlines() {
    assert_eq!(
        super::super::validate_style_overrides(
            r#"{"--part-screen-background-image":"repeating-linear-gradient(transparent 0, #0000001a 2px)"}"#
        ),
        Err("style_overrides: --part-screen-background-image: a theme sets it, never an override: the protected palette is clamped against it.".into())
    );
}

#[test]
fn scanline_colours_lists_each_stop_colour() {
    assert_eq!(
        grammar::scanline_colours(
            "repeating-linear-gradient(transparent 0, rgba(51, 255, 51, 0.08) 2px, #0000001a 3px)"
        ),
        vec!["transparent", "rgba(51, 255, 51, 0.08)", "#0000001a"]
    );
    assert!(grammar::scanline_colours("none").is_empty());
}

#[test]
fn a_text_glow_past_the_part_caps_is_refused() {
    let message = refusal(r#"{"name":"G","tokens":{"--text-glow":"0 0 2rem red"}}"#);
    assert!(
        message.contains("--text-glow sets text-shadow on chat-text")
            && message.contains("0.6em cap"),
        "{message}"
    );
}

// --- Generated files ---

const REGENERATE: &str =
    "cargo test -p lucidos-engine --lib generate_theme_parts_files -- --ignored";

fn repo_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("repo root")
        .to_path_buf()
}

fn ts_path() -> std::path::PathBuf {
    repo_root().join("packages/lucidos-sdk/src/generated/theme-parts.ts")
}

fn css_path(realm: Realm) -> std::path::PathBuf {
    let name = match realm {
        Realm::Shell => "theme-parts.css",
        Realm::Frame => "theme-parts-frame.css",
    };
    repo_root()
        .join("crates/lucidos-app/src/styles/generated")
        .join(name)
}

/// Where a generated stylesheet loads. App frames define and register only
/// the frame colour tokens, so an app's `var(--token, fallback)` still falls
/// back for the rest.
#[derive(Clone, Copy)]
enum Realm {
    Shell,
    Frame,
}

fn header(comment_open: &str, line: &str) -> String {
    format!(
        "{comment_open} AUTO-GENERATED. Do not edit by hand.\n{line} Regenerate: {REGENERATE}\n{blank}\n{line} Source of truth: crates/lucidos-engine/src/core/themes/theme-parts.json\n{line} and the colour tokens of theme-tokens.json.\n",
        blank = line.trim_end()
    )
}

fn json(value: impl serde::Serialize) -> String {
    serde_json::to_string(&value).expect("serialises")
}

fn grammar_ts(grammar: &Grammar) -> String {
    let unit = |u: &grammar::Unit| format!("'{}'", json(u).trim_matches('"'));
    match grammar {
        Grammar::Colour { min_alpha } => match min_alpha {
            Some(a) => format!("{{ grammar: 'colour', minAlpha: {a} }}"),
            None => "{ grammar: 'colour' }".into(),
        },
        Grammar::Shadow {
            unit: u,
            max_layers,
            max_offset,
            max_blur,
            max_spread,
        } => {
            let spread = max_spread.map_or(String::new(), |s| format!(", maxSpread: {s}"));
            format!(
                "{{ grammar: 'shadow', unit: {}, maxLayers: {max_layers}, maxOffset: {max_offset}, maxBlur: {max_blur}{spread} }}",
                unit(u)
            )
        }
        Grammar::DropShadow {
            unit: u,
            max_offset,
            max_blur,
        } => format!(
            "{{ grammar: 'drop-shadow', unit: {}, maxOffset: {max_offset}, maxBlur: {max_blur} }}",
            unit(u)
        ),
        Grammar::Spacing { unit: u, min, max } => format!(
            "{{ grammar: 'spacing', unit: {}, min: {min}, max: {max} }}",
            unit(u)
        ),
        Grammar::Length { unit: u, min, max } => format!(
            "{{ grammar: 'length', unit: {}, min: {min}, max: {max} }}",
            unit(u)
        ),
        Grammar::Keyword { values } => {
            let quoted: Vec<String> = values.iter().map(|v| format!("'{v}'")).collect();
            format!("{{ grammar: 'keyword', values: [{}] }}", quoted.join(", "))
        }
        Grammar::Scanlines {
            max_alpha,
            max_period,
            max_stops,
        } => format!(
            "{{ grammar: 'scanlines', maxAlpha: {max_alpha}, maxPeriod: {max_period}, maxStops: {max_stops} }}"
        ),
    }
}

/// What the apply-site twin needs: each property's grammar, each part
/// token's part and property, the colour tokens and the named colours.
fn generate_ts() -> String {
    let mut out = header("//", "//");
    out.push_str("\nexport type PartUnit = 'em' | 'px';\n\n");
    out.push_str("export type PartGrammar =\n");
    out.push_str("  | { grammar: 'colour'; minAlpha?: number }\n");
    out.push_str("  | { grammar: 'shadow'; unit: PartUnit; maxLayers: number; maxOffset: number; maxBlur: number; maxSpread?: number }\n");
    out.push_str(
        "  | { grammar: 'drop-shadow'; unit: PartUnit; maxOffset: number; maxBlur: number }\n",
    );
    out.push_str("  | { grammar: 'spacing'; unit: PartUnit; min: number; max: number }\n");
    out.push_str("  | { grammar: 'length'; unit: PartUnit; min: number; max: number }\n");
    out.push_str("  | { grammar: 'keyword'; values: readonly string[] }\n");
    out.push_str(
        "  | { grammar: 'scanlines'; maxAlpha: number; maxPeriod: number; maxStops: number };\n\n",
    );
    out.push_str("export interface PartTokenSpec {\n  part: string;\n  property: string;\n  insetOnly: boolean;\n  frames: boolean;\n}\n\n");
    out.push_str("export const PART_PROPERTIES: Record<string, PartGrammar> = {\n");
    for (name, property) in &CATALOG.properties {
        out.push_str(&format!("  '{name}': {},\n", grammar_ts(&property.grammar)));
    }
    out.push_str("};\n\nexport const PART_TOKENS: Record<string, PartTokenSpec> = {\n");
    for (part, usage) in CATALOG.tokens() {
        out.push_str(&format!(
            "  '{}': {{ part: '{}', property: '{}', insetOnly: {}, frames: {} }},\n",
            usage.token, part.id, usage.name, usage.inset_only, part.frames
        ));
    }
    out.push_str("};\n\nexport const COLOUR_TOKENS: readonly string[] = [\n");
    for (token, _) in super::super::colour_tokens() {
        out.push_str(&format!("  '{token}',\n"));
    }
    out.push_str("];\n\n/** The colour tokens app frames define and register. */\n");
    out.push_str("export const FRAME_COLOUR_TOKENS: readonly string[] = [\n");
    for (token, _) in super::super::colour_tokens().filter(|(_, frames)| *frames) {
        out.push_str(&format!("  '{token}',\n"));
    }
    out.push_str("];\n\nexport const NAMED_COLOURS: readonly string[] = [\n");
    let names: Vec<&str> = super::super::color::named_colours().collect();
    for chunk in names.chunks(6) {
        let quoted: Vec<String> = chunk.iter().map(|n| format!("'{n}'")).collect();
        out.push_str(&format!("  {},\n", quoted.join(", ")));
    }
    out.push_str("];\n\n// The engine's caps on a theme value, from core/themes.\n");
    let caps = [
        ("MAX_MIX_DEPTH", grammar::MAX_MIX_DEPTH.to_string()),
        (
            "MAX_VALUE_LENGTH",
            super::super::MAX_VALUE_LENGTH.to_string(),
        ),
        (
            "MAX_RESOLVED_TOKENS",
            super::super::MAX_RESOLVED_TOKENS.to_string(),
        ),
        ("MAX_SHADOW_PX", super::super::MAX_SHADOW_PX.to_string()),
        ("PX_PER_REM", super::super::PX_PER_REM.to_string()),
    ];
    for (name, value) in caps {
        out.push_str(&format!("export const {name} = {value};\n"));
    }
    let functions: Vec<String> = super::super::SHADOW_COLOUR_FUNCTIONS
        .iter()
        .map(|f| format!("'{f}'"))
        .collect();
    out.push_str(&format!(
        "export const SHADOW_COLOUR_FUNCTIONS: readonly string[] = [{}];\n",
        functions.join(", ")
    ));
    out
}

/// A colour token's `@property` initial value: its dark default as a
/// literal, since an initial value may not name another property. Base CSS
/// sets every token on the root, so this paints only under a broken value.
fn initial_value(token: &str) -> String {
    match super::super::colour_in(&TokenMap::new(), ThemeMode::Dark, token) {
        Some(colour) if colour.a < 1.0 => colour.to_rgba_css(),
        Some(colour) => colour.to_hex(),
        None => "transparent".into(),
    }
}

fn declarations(out: &mut String, names: impl Iterator<Item = String>, value: &str) {
    for name in names {
        out.push_str(&format!("    {name}: {value};\n"));
    }
}

/// The part CSS for one realm: the shell imports one, and the engine appends
/// the other to `/api/v1/sdk-iframe.css`. Consumers stay hand-written.
fn generate_css(realm: Realm) -> String {
    let mut out = header("/*", "  ");
    out.push_str("*/\n\n");
    out.push_str("/* Every colour token this realm defines is a <color>, so a var() in a part\n   value substitutes a computed colour, never text that could add a shadow\n   layer. */\n");
    let registered = super::super::colour_tokens()
        .filter(|(_, frames)| *frames || matches!(realm, Realm::Shell))
        .map(|(token, _)| token);
    for token in registered {
        out.push_str(&format!(
            "@property {token} {{\n    syntax: '<color>';\n    inherits: true;\n    initial-value: {};\n}}\n\n",
            initial_value(token)
        ));
    }
    let all = || CATALOG.tokens().map(|(_, u)| u.token.clone());
    out.push_str("/* A protected surface: no part token reaches inside, and no inherited\n   part paint flows in from a container. */\n.protected-surface {\n");
    declarations(&mut out, all(), "initial");
    out.push_str("}\n\n:where(.protected-surface) {\n");
    for (name, property) in &CATALOG.properties {
        if let Some(value) = &property.protected_value {
            out.push_str(&format!("    {name}: {value};\n"));
        }
    }
    out.push_str("}\n\n");
    out.push_str("/* theme-effects: reduce drops every part effect: shadows, filters and\n   scanlines. On body, because the tokens sit inline on <html>, where no\n   rule can beat them. */\nhtml[data-theme-effects=\"reduce\"] body {\n");
    declarations(
        &mut out,
        CATALOG
            .tokens()
            .filter(|(_, u)| CATALOG.property(&u.name).effect)
            .map(|(_, u)| u.token.clone()),
        "initial",
    );
    out.push_str("}\n\n");
    out.push_str(
        "/* An app frame that opts out paints no part. */\nhtml[data-theme-parts=\"off\"] body {\n",
    );
    declarations(&mut out, all(), "initial");
    out.push_str("}\n");
    out
}

fn assert_up_to_date(path: std::path::PathBuf, generated: String) {
    let existing = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("{} is missing. Run: {REGENERATE}", path.display()));
    assert_eq!(
        existing,
        generated,
        "{} is stale. Run: {REGENERATE}",
        path.display()
    );
}

#[test]
fn generated_theme_parts_ts_is_up_to_date() {
    assert_up_to_date(ts_path(), generate_ts());
}

#[test]
fn generated_theme_parts_css_is_up_to_date() {
    for realm in [Realm::Shell, Realm::Frame] {
        assert_up_to_date(css_path(realm), generate_css(realm));
    }
}

#[test]
#[ignore]
fn generate_theme_parts_files() {
    for (path, content) in [
        (ts_path(), generate_ts()),
        (css_path(Realm::Shell), generate_css(Realm::Shell)),
        (css_path(Realm::Frame), generate_css(Realm::Frame)),
    ] {
        std::fs::write(&path, content).unwrap();
        crate::log!("[Codegen] wrote {}", path.display());
    }
}

// --- Style overrides ---

#[test]
fn a_style_override_part_token_must_pass_the_grammar() {
    use super::super::validate_style_overrides;
    assert!(
        validate_style_overrides(r#"{"--part-chat-text-text-shadow":"0 0 0.3em red"}"#).is_ok()
    );
    assert!(validate_style_overrides(r#"{"--accent":"anything the apply site judges"}"#).is_ok());
    assert!(validate_style_overrides("not json").is_ok());
    assert_eq!(
        validate_style_overrides(r#"{"--part-chat-text-text-shadow":"0 0 9em red"}"#),
        Err(
            "style_overrides: --part-chat-text-text-shadow: blur 9em is over the 0.6em cap.".into()
        )
    );
    assert_eq!(
        validate_style_overrides(r#"{"--part-nope-color":"red"}"#),
        Err("style_overrides: --part-nope-color: not a part token. GET /api/v1/themes/parts lists them.".into())
    );
    assert!(validate_style_overrides(r#"{"--part-chat-text-color":7}"#).is_err());
    assert_eq!(
        validate_style_overrides(r#"{"--shadow-md":"0 0 0 100vmax #000"}"#),
        Err("style_overrides: --shadow-md: the shadow reaches 100vmax, in a unit whose size cannot be checked".into())
    );
}

// --- Docs and fixtures ---

#[test]
fn the_themes_knowhow_lists_every_part_with_its_properties() {
    let doc = include_str!("../../../../../system-knowhow/themes.md");
    for part in &CATALOG.parts {
        let row = doc
            .lines()
            .find(|line| line.starts_with(&format!("| `{}` |", part.id)))
            .unwrap_or_else(|| panic!("system-knowhow/themes.md has no row for {}", part.id));
        let names: Vec<String> = part
            .properties
            .iter()
            .map(|p| format!("`{}`", p.name))
            .collect();
        assert!(row.ends_with(&format!("| {} |", names.join(", "))), "{row}");
    }
}

#[test]
fn the_retro_effects_example_is_a_valid_theme() {
    let doc = include_str!("../../../../../system-knowhow/themes.md");
    let section = &doc[doc.find("### Retro effects").expect("the section")..];
    let start = section.find("```json\n").expect("an example") + "```json\n".len();
    let example = &section[start..start + section[start..].find("```").expect("its end")];
    parse_definition(example.as_bytes()).expect("the example is a valid theme");
}

/// The browser specs use these, never a shipped theme (ADR 0307).
#[test]
fn the_browser_fixture_themes_are_valid_and_every_cap_sets_every_part() {
    let every_cap = parse_definition(include_bytes!(
        "../../../../lucidos-app/e2e/themes/parts-every-cap.json"
    ))
    .expect("parts-every-cap.json is a valid theme");
    for part in &CATALOG.parts {
        let set = every_cap
            .parts
            .get(&part.id)
            .unwrap_or_else(|| panic!("{} unset", part.id));
        for usage in &part.properties {
            assert!(set.contains_key(&usage.name), "{}.{}", part.id, usage.name);
        }
    }
    parse_definition(include_bytes!(
        "../../../../lucidos-app/e2e/themes/text-glow-only.json"
    ))
    .expect("text-glow-only.json is a valid theme");
    parse_definition(include_bytes!(
        "../../../../lucidos-app/e2e/themes/retro-phosphor.json"
    ))
    .expect("retro-phosphor.json is a valid theme");
}
