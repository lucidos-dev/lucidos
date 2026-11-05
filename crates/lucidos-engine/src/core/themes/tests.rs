use super::*;

fn def(json: &str) -> ThemeDefinition {
    parse_definition(json.as_bytes()).expect("valid theme")
}

fn map(pairs: &[(&str, &str)]) -> TokenMap {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ValidationCases {
    shadow_tokens: Vec<String>,
    shadow_values_within_reach: Vec<String>,
    shadow_values_past_reach: Vec<String>,
    ui_font_tokens: Vec<String>,
    valid_names: Vec<String>,
    invalid_names: Vec<String>,
    valid_values: Vec<String>,
    invalid_values: Vec<String>,
}

/// The same fixture drives `appearance-themes.test.ts`, so the engine and every apply
/// site agree on what a theme may contain.
#[test]
fn token_rules_match_the_shared_fixture() {
    let cases: ValidationCases =
        serde_json::from_str(include_str!("theme-validation-cases.json")).unwrap();
    // The SDK refuses the same UI font tokens in a style override, and holds
    // an override's shadow to the same reach.
    assert_eq!(cases.ui_font_tokens, UI_FONT_TOKENS);
    let shadows: Vec<&str> = CATALOG
        .tokens
        .iter()
        .filter(|t| t.kind == SHADOW_KIND)
        .map(|t| t.name.as_str())
        .collect();
    assert_eq!(cases.shadow_tokens, shadows);
    for value in &cases.shadow_values_within_reach {
        validate_shadow_reach(value).unwrap_or_else(|e| panic!("{value}: {e}"));
    }
    for value in &cases.shadow_values_past_reach {
        assert!(
            validate_shadow_reach(value).is_err(),
            "{value} should be refused"
        );
    }
    for name in &cases.valid_names {
        assert!(is_valid_token_name(name), "{name} should be a valid name");
    }
    for name in &cases.invalid_names {
        assert!(!is_valid_token_name(name), "{name} should be refused");
    }
    for value in &cases.valid_values {
        assert!(
            is_valid_token_value(value),
            "{value} should be a valid value"
        );
    }
    for value in &cases.invalid_values {
        assert!(!is_valid_token_value(value), "{value} should be refused");
    }
}

#[test]
fn every_built_in_theme_is_valid_and_listed_in_order() {
    let themes = list(Path::new("/nonexistent"));
    let ids: Vec<&str> = themes.iter().map(|l| l.id.as_str()).collect();
    let expected: Vec<&str> = BUILT_IN_THEMES.iter().map(|(id, _)| *id).collect();
    assert_eq!(ids, expected);
    assert!(themes.iter().all(|l| l.source == ThemeSource::BuiltIn));
    assert!(
        themes.iter().all(|l| !l.definition.credit.is_empty()),
        "every built-in credits its palette"
    );
}

#[test]
fn every_built_in_theme_has_its_family() {
    use ThemeFamily::*;
    let family = |id: &str| built_in(id).unwrap().definition.family;
    for (id, expected) in [
        ("lucidos", Blue),
        ("nord", Blue),
        ("tokyo-night", Blue),
        ("solarized", Blue),
        ("amethyst", Violet),
        ("catppuccin", Violet),
        ("rose-pine", Violet),
        ("gruvbox", Warm),
        ("everforest", Warm),
        ("paper", Warm),
        ("minimal", Neutral),
        ("mono", Neutral),
        ("ember", Warm),
        ("harbour", Blue),
        ("real-computer", Neutral),
    ] {
        assert_eq!(family(id), Some(expected), "{id}");
    }
    assert!(
        BUILT_INS.iter().all(|l| l.definition.family.is_some()),
        "a new built-in names its family"
    );
}

#[test]
fn a_theme_without_a_family_stays_valid() {
    let ok = br#"{"name":"Mine"}"#;
    assert!(validate_workspace_write("mine.json", ok, WorkspaceFonts::default).is_ok());
    let json = serde_json::to_value(def(r#"{"name":"Mine"}"#)).unwrap();
    assert!(json.get("family").is_none(), "{json}");
}

#[test]
fn an_unknown_family_is_refused_with_the_choices() {
    let err = refusal(r#"{"name":"x","family":"teal"}"#);
    assert!(err.contains("teal") && err.contains("violet"), "{err}");
    let json = serde_json::to_value(def(r#"{"name":"x","family":"warm"}"#)).unwrap();
    assert_eq!(json["family"], "warm");
}

#[test]
fn the_default_theme_paints_nothing() {
    let theme = get(Path::new("/nonexistent"), DEFAULT_THEME_ID)
        .unwrap()
        .unwrap();
    assert!(theme.resolved.dark.is_empty());
    assert!(theme.resolved.light.is_empty());
    assert_eq!(
        resolved_json_for_preference(Path::new("/nonexistent"), DEFAULT_THEME_ID),
        None
    );
}

#[test]
fn a_mode_map_wins_over_shared_tokens() {
    let d = def(
        r##"{"name":"x","tokens":{"--radius-control":"1rem","--accent":"#111111"},"dark":{"--accent":"#222222"}}"##,
    );
    let dark = resolve_mode(&d, ThemeMode::Dark, None);
    let light = resolve_mode(&d, ThemeMode::Light, None);
    assert_eq!(dark["--accent"], "#222222");
    assert_eq!(light["--accent"], "#111111");
    assert_eq!(dark["--radius-control"], "1rem");
    assert_eq!(d.modes(), vec![ThemeMode::Dark]);
}

#[test]
fn a_seed_fills_in_its_derived_tokens() {
    let d = def(
        r##"{"name":"x","dark":{"--bg-primary":"#101010","--text-primary":"#eeeeee","--accent":"#88c0d0"}}"##,
    );
    let dark = resolve_mode(&d, ThemeMode::Dark, None);
    assert!(dark["--bg-secondary"].contains("var(--bg-primary)"));
    assert!(dark["--text-muted"].contains("var(--text-primary)"));
    assert_eq!(dark["--accent-action"], "var(--accent)");
    assert!(dark["--header-bar-top"].contains("var(--accent)"));
    assert_eq!(dark["--header-fg"], "var(--text-primary)");
    // The light mode has no map, so nothing is set and nothing derives.
    assert!(resolve_mode(&d, ThemeMode::Light, None).is_empty());
}

/// Bold text steps away from the page, whichever mode paints it. A dark theme
/// shown under the light theme still brightens its bold, and a light theme under
/// the dark theme darkens it.
#[test]
fn strong_text_steps_away_from_the_themes_own_page() {
    let dark_page =
        def(r##"{"name":"x","tokens":{"--bg-primary":"#050805","--text-primary":"#33ff33"}}"##);
    for mode in [ThemeMode::Dark, ThemeMode::Light] {
        assert_eq!(
            resolve_mode(&dark_page, mode, None)["--text-strong"],
            "color-mix(in oklab, var(--text-primary) 60%, white)",
            "{mode:?}"
        );
    }
    let light_page =
        def(r##"{"name":"x","tokens":{"--bg-primary":"#fdf6e3","--text-primary":"#333333"}}"##);
    for mode in [ThemeMode::Dark, ThemeMode::Light] {
        assert_eq!(
            resolve_mode(&light_page, mode, None)["--text-strong"],
            "color-mix(in oklab, var(--text-primary) 60%, black)",
            "{mode:?}"
        );
    }
}

#[test]
fn a_theme_keeps_its_own_strong_text_and_one_without_seeds_sets_none() {
    let own = def(
        r##"{"name":"x","tokens":{"--bg-primary":"#000000","--text-primary":"#33ff33","--text-strong":"#ffb000"}}"##,
    );
    assert_eq!(
        resolve_mode(&own, ThemeMode::Light, None)["--text-strong"],
        "#ffb000"
    );
    let square = def(r#"{"name":"x","tokens":{"--radius-control":"0"}}"#);
    assert!(!resolve_mode(&square, ThemeMode::Light, None).contains_key("--text-strong"));
}

/// A theme that writes its own header fill has it carried up through the macOS
/// title-bar band. One that only retunes the stops keeps the band solid.
#[test]
fn an_own_header_fill_continues_through_the_titlebar_band() {
    let striped = def(
        r##"{"name":"x","dark":{"--header-gradient":"repeating-linear-gradient(to bottom, #000000 0 2px, #052205 2px 3px)"}}"##,
    );
    assert_eq!(
        resolve_mode(&striped, ThemeMode::Dark, None)["--titlebar-strip-continues"],
        "1"
    );
    let stops = def(r##"{"name":"x","dark":{"--header-bar-top":"#000000"}}"##);
    assert!(!resolve_mode(&stops, ThemeMode::Dark, None).contains_key("--titlebar-strip-continues"));
}

#[test]
fn derivation_never_overwrites_an_explicit_token() {
    let d = def(r##"{"name":"x","dark":{"--accent":"#88c0d0","--accent-action":"#5e81ac"}}"##);
    assert_eq!(
        resolve_mode(&d, ThemeMode::Dark, None)["--accent-action"],
        "#5e81ac"
    );
}

#[test]
fn a_theme_without_seeds_derives_nothing() {
    let d = def(r#"{"name":"x","tokens":{"--radius-control":"0"}}"#);
    assert_eq!(
        resolve_theme_tokens(&d, ThemeMode::Dark, None),
        map(&[("--radius-control", "0")])
    );
}

#[test]
fn every_derivation_names_catalog_seeds_and_passes_the_value_rules() {
    let names: Vec<&str> = CATALOG.tokens.iter().map(|t| t.name.as_str()).collect();
    for token in &CATALOG.tokens {
        assert!(is_valid_token_name(&token.name), "{}", token.name);
        let Some(d) = &token.derive else { continue };
        assert!(
            is_valid_token_value(&d.value),
            "{} derives an invalid value",
            token.name
        );
        assert!(!d.seeds.is_empty(), "{} derives from no seed", token.name);
        for seed in &d.seeds {
            assert!(
                names.contains(&seed.as_str()),
                "{} seeds on unknown {seed}",
                token.name
            );
        }
    }
}

#[test]
fn a_definition_is_refused_for_each_broken_shape() {
    for (json, why) in [
        (r#"{"name":""}"#, "empty name"),
        (r#"{"description":"no name"}"#, "missing name"),
        (r#"{"name":"x","Dark":{}}"#, "unknown field"),
        (
            r#"{"name":"x","tokens":{"accent":"red"}}"#,
            "bad token name",
        ),
        (
            r#"{"name":"x","dark":{"--accent":"url(https://example.com)"}}"#,
            "banned value",
        ),
        (
            r#"{"name":"x","light":{"--accent":"red; color: blue"}}"#,
            "declaration break",
        ),
        ("not json", "not JSON"),
    ] {
        assert!(
            parse_definition(json.as_bytes()).is_err(),
            "{why} should be refused"
        );
    }
}

#[test]
fn a_workspace_write_names_a_fresh_valid_id() {
    let ok = br##"{"name":"Mine","dark":{"--accent":"#ff0000"}}"##;
    assert!(validate_workspace_write("mine.json", ok, WorkspaceFonts::default).is_ok());
    assert!(
        validate_workspace_write("nord.json", ok, WorkspaceFonts::default).is_err(),
        "built-in id"
    );
    assert!(
        validate_workspace_write("Mine.json", ok, WorkspaceFonts::default).is_err(),
        "uppercase id"
    );
    assert!(
        validate_workspace_write("mine.css", ok, WorkspaceFonts::default).is_err(),
        "not JSON"
    );
    assert!(
        validate_workspace_write("nested/mine.json", ok, WorkspaceFonts::default).is_err(),
        "nested"
    );
    assert!(
        validate_workspace_write("mine.json", b"{}", WorkspaceFonts::default).is_err(),
        "invalid body"
    );
}

#[test]
fn ids_are_lowercase_kebab() {
    for ok in ["nord", "rose-pine", "a1", "my-theme-2"] {
        assert!(validate_id(ok).is_ok(), "{ok}");
    }
    for bad in [
        "",
        "Nord",
        "rose_pine",
        "-x",
        "x-",
        "a--b",
        "../x",
        "x.json",
    ] {
        assert!(validate_id(bad).is_err(), "{bad}");
    }
}

#[test]
fn workspace_themes_follow_the_built_ins_sorted_and_broken_ones_are_skipped() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join(THEMES_DIR);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("zeta.json"),
        r##"{"name":"Zeta","dark":{"--accent":"#123456"}}"##,
    )
    .unwrap();
    std::fs::write(dir.join("alpha.json"), r#"{"name":"Alpha"}"#).unwrap();
    std::fs::write(
        dir.join("broken.json"),
        r#"{"name":"x","tokens":{"--accent":"url(x)"}}"#,
    )
    .unwrap();
    std::fs::write(dir.join("nord.json"), r#"{"name":"Shadow"}"#).unwrap();
    std::fs::write(dir.join("notes.txt"), "not a theme").unwrap();

    let themes = list(tmp.path());
    let workspace: Vec<&str> = themes
        .iter()
        .filter(|l| l.source == ThemeSource::Workspace)
        .map(|l| l.id.as_str())
        .collect();
    assert_eq!(workspace, vec!["alpha", "zeta"]);
    let nord = themes.iter().find(|l| l.id == "nord").unwrap();
    assert_eq!(
        nord.source,
        ThemeSource::BuiltIn,
        "a file cannot shadow a built-in"
    );

    let zeta = get(tmp.path(), "zeta").unwrap().unwrap();
    assert_eq!(zeta.resolved.dark["--accent"], "#123456");
    assert!(get(tmp.path(), "missing").unwrap().is_none());
    assert!(get(tmp.path(), "broken").is_err());
    assert!(get(tmp.path(), "../etc").is_err());

    let seeded = resolved_json_for_preference(tmp.path(), "zeta").unwrap();
    assert!(seeded.contains("\"--accent\":\"#123456\""));
    assert_eq!(resolved_json_for_preference(tmp.path(), "missing"), None);
}

#[test]
fn a_served_theme_flattens_its_definition() {
    let theme = get(Path::new("/nonexistent"), "paper").unwrap().unwrap();
    let json = serde_json::to_value(&theme).unwrap();
    assert_eq!(json["id"], "paper");
    assert_eq!(json["source"], "built-in");
    assert_eq!(json["name"], "Paper");
    assert_eq!(json["modes"], serde_json::json!(["light"]));
    assert!(json["resolved"]["light"]["--bg-primary"].is_string());
    assert_eq!(json["resolved"]["dark"], serde_json::json!({}));
}

/// The agent-facing knowhow names every token a theme can tune, so the agent
/// never has to guess one. A new catalog token needs its row in the same change.
#[test]
fn the_themes_knowhow_lists_every_catalog_token() {
    let doc = include_str!("../../../../../system-knowhow/themes.md");
    for token in &CATALOG.tokens {
        assert!(
            doc.contains(&format!("| `{}` |", token.name)),
            "system-knowhow/themes.md has no row for {}",
            token.name
        );
    }
    for (id, _) in BUILT_IN_THEMES {
        assert!(
            doc.contains(&format!("`{id}`")),
            "system-knowhow/themes.md never names {id}"
        );
    }
}

#[test]
fn value_length_counts_what_javascript_counts() {
    // An astral character is one char but two UTF-16 units.
    let value = "\u{1F600}".repeat(61);
    assert_eq!(value.chars().count(), 61);
    assert!(!is_valid_token_value(&value));
}

// --- Fonts a theme suggests ---

fn refusal(json: &str) -> String {
    validate_workspace_write("mine.json", json.as_bytes(), WorkspaceFonts::default)
        .expect_err("the theme should be refused")
        .to_string()
}

#[test]
fn a_theme_suggests_fonts_by_catalog_id() {
    let d = def(r#"{"name":"x","fonts":{"ui":"geist","mono":"geist-mono"}}"#);
    let theme = build("x", ThemeSource::Workspace, d, &WorkspaceFonts::default());
    assert_eq!(theme.resolved.fonts.ui.as_deref(), Some("geist"));
    let stack = fonts::find("geist-mono").unwrap().stack;
    // The code font is an ordinary token, so every surface paints it unchanged.
    assert_eq!(theme.resolved.dark[FONT_MONO_TOKEN], stack);
    assert_eq!(theme.resolved.light[FONT_MONO_TOKEN], stack);
    // The UI font is NOT a token: laid inline, it would beat the user's pick.
    assert!(!theme.resolved.dark.contains_key("--font-ui"));
    let json = serde_json::to_value(&theme.resolved).unwrap();
    assert_eq!(json["fonts"]["ui"], "geist");
}

#[test]
fn a_theme_without_fonts_serialises_as_before() {
    let d = def(r##"{"name":"x","dark":{"--accent":"#123456"}}"##);
    let json = serde_json::to_value(
        build("x", ThemeSource::Workspace, d, &WorkspaceFonts::default()).resolved,
    )
    .unwrap();
    assert!(json.get("fonts").is_none(), "{json}");
}

#[test]
fn an_unknown_font_id_is_refused() {
    let err = refusal(r#"{"name":"x","fonts":{"ui":"comic-sans"}}"#);
    assert!(err.contains("'comic-sans' is not a font id"), "{err}");
    let err = refusal(r#"{"name":"x","fonts":{"mono":"nope"}}"#);
    assert!(err.contains("`fonts.mono`"), "{err}");
}

/// ADR 0303: every catalog font is bundled, so a theme may name any of them,
/// the three that once loaded from Google Fonts included.
#[test]
fn a_theme_may_name_every_catalog_font_fit_for_its_slot() {
    for font in fonts::FONT_CATALOG {
        if font.kind.fits_ui() {
            let json = format!(r#"{{"name":"x","fonts":{{"ui":"{}"}}}}"#, font.id);
            validate_workspace_write("mine.json", json.as_bytes(), WorkspaceFonts::default)
                .unwrap_or_else(|e| panic!("fonts.ui = {}: {e}", font.id));
        }
        if font.kind.fits_code() {
            let json = format!(r#"{{"name":"x","fonts":{{"mono":"{}"}}}}"#, font.id);
            validate_workspace_write("mine.json", json.as_bytes(), WorkspaceFonts::default)
                .unwrap_or_else(|e| panic!("fonts.mono = {}: {e}", font.id));
        }
    }
}

#[test]
fn every_new_code_font_is_valid_as_fonts_mono() {
    for id in [
        "jetbrains-mono",
        "ibm-plex-mono",
        "source-code-pro",
        "commit-mono",
        "cascadia-code",
        "vt323",
    ] {
        let d = def(&format!(r#"{{"name":"x","fonts":{{"mono":"{id}"}}}}"#));
        let theme = build("x", ThemeSource::Workspace, d, &WorkspaceFonts::default());
        assert_eq!(
            theme.resolved.dark[FONT_MONO_TOKEN],
            fonts::find(id).unwrap().stack,
            "{id}"
        );
    }
}

#[test]
fn a_proportional_font_cannot_be_the_code_font() {
    let err = refusal(r#"{"name":"x","fonts":{"mono":"source-serif-4"}}"#);
    assert!(err.contains("proportional"), "{err}");
}

#[test]
fn fonts_mono_and_a_font_mono_token_are_refused_together() {
    let err =
        refusal(r#"{"name":"x","fonts":{"mono":"geist-mono"},"dark":{"--font-mono":"monospace"}}"#);
    assert!(err.contains("not both"), "{err}");
}

/// The UI font reaches a theme only through `fonts.ui`. A token would be laid
/// over the user's explicit pick.
#[test]
fn a_ui_font_token_is_refused() {
    for name in UI_FONT_TOKENS {
        let err = refusal(&format!(r#"{{"name":"x","tokens":{{"{name}":"serif"}}}}"#));
        assert!(err.contains("`fonts.ui`"), "{name}: {err}");
    }
}

#[test]
fn a_free_form_code_font_stack_still_works() {
    let d = def(r#"{"name":"x","tokens":{"--font-mono":"'Iosevka', ui-monospace, monospace"}}"#);
    assert_eq!(
        resolve_mode(&d, ThemeMode::Dark, None)[FONT_MONO_TOKEN],
        "'Iosevka', ui-monospace, monospace"
    );
}

#[test]
fn an_unknown_font_slot_is_refused() {
    let err = refusal(r#"{"name":"x","fonts":{"heading":"geist"}}"#);
    assert!(err.contains("heading"), "{err}");
}

/// A device with no font set follows its theme. A font on the default theme
/// would change what every fresh install paints (ADR 0077).
#[test]
fn the_default_theme_suggests_no_font() {
    assert!(built_in(DEFAULT_THEME_ID)
        .unwrap()
        .resolved
        .fonts
        .is_empty());
}

#[test]
fn some_built_in_themes_suggest_a_ui_font() {
    let ui_font = |id: &str| built_in(id).unwrap().resolved.fonts.ui;
    assert_eq!(ui_font("paper").as_deref(), Some("source-serif-4"));
    assert_eq!(ui_font("minimal").as_deref(), Some("geist"));
    assert_eq!(ui_font("mono").as_deref(), Some("geist-mono"));
}

// --- Workspace fonts a theme suggests ---

fn installed(fonts: &[(&str, &str)]) -> WorkspaceFonts {
    let dir = tempfile::tempdir().unwrap();
    for (slug, group) in fonts {
        let font = dir.path().join(slug);
        std::fs::create_dir_all(&font).unwrap();
        std::fs::write(
            font.join("font.json"),
            format!(r#"{{"label":"{slug}","group":"{group}","faces":[{{"file":"a.woff2"}}]}}"#),
        )
        .unwrap();
        std::fs::write(font.join("a.woff2"), b"wOF2 bytes").unwrap();
    }
    workspace_fonts::list_in(dir.path())
}

#[test]
fn a_theme_may_name_an_installed_workspace_font_in_a_slot_it_fits() {
    let fonts = installed(&[("brand", "sans"), ("code", "mono")]);
    let write =
        |json: &str| validate_workspace_write("mine.json", json.as_bytes(), || fonts.clone());
    assert!(write(r#"{"name":"x","fonts":{"ui":"ws-brand","mono":"ws-code"}}"#).is_ok());
    assert!(write(r#"{"name":"x","fonts":{"ui":"ws-code"}}"#).is_ok());
    let err = write(r#"{"name":"x","fonts":{"mono":"ws-brand"}}"#)
        .unwrap_err()
        .to_string();
    assert!(err.contains("proportional"), "{err}");
    let err = write(r#"{"name":"x","fonts":{"ui":"ws-gone"}}"#)
        .unwrap_err()
        .to_string();
    assert!(err.contains("not an installed workspace font"), "{err}");
}

/// A theme is parsed apart from any workspace, so a well-formed workspace font
/// id passes parsing. Whether it exists is asked where the fonts are at hand.
#[test]
fn parsing_accepts_a_well_formed_workspace_font_id_and_nothing_looser() {
    assert!(parse_definition(br#"{"name":"x","fonts":{"ui":"ws-brand"}}"#).is_ok());
    for id in ["ws-", "ws-Brand", "ws-a/b", "wsbrand"] {
        let json = format!(r#"{{"name":"x","fonts":{{"ui":"{id}"}}}}"#);
        assert!(parse_definition(json.as_bytes()).is_err(), "{id}");
    }
}

#[test]
fn a_resolved_theme_lays_a_workspace_code_font_and_carries_its_entry() {
    let fonts = installed(&[("code", "mono")]);
    let d = def(r#"{"name":"x","fonts":{"ui":"ws-code","mono":"ws-code"}}"#);
    let theme = build("x", ThemeSource::Workspace, d, &fonts);
    let entry = fonts.find("ws-code").unwrap();
    assert_eq!(theme.resolved.dark[FONT_MONO_TOKEN], entry.stack);
    // Approval cards keep a catalog code font: the workspace one is unchecked.
    assert_eq!(
        theme.resolved.dark[PROTECTED_FONT_MONO_TOKEN],
        protected_font_mono(&theme.definition)
    );
    assert_ne!(theme.resolved.dark[PROTECTED_FONT_MONO_TOKEN], entry.stack);
    assert_eq!(theme.resolved.fonts.ui.as_deref(), Some("ws-code"));
    // One entry, though the font fills both slots.
    assert_eq!(theme.resolved.workspace_fonts, vec![entry.clone()]);
    let json = serde_json::to_value(&theme.resolved).unwrap();
    assert_eq!(
        json["workspace_fonts"][0]["faces"][0]["path"],
        "fonts/code/a.woff2"
    );
}

/// A font removed after the theme was written drops out, so its slot falls
/// back as if the theme had never named it.
#[test]
fn a_vanished_or_unfit_workspace_font_drops_out_of_the_resolved_theme() {
    let d = def(r#"{"name":"x","fonts":{"ui":"ws-gone","mono":"ws-brand"}}"#);
    let theme = build(
        "x",
        ThemeSource::Workspace,
        d,
        &installed(&[("brand", "serif")]),
    );
    assert!(
        theme.resolved.fonts.is_empty(),
        "{:?}",
        theme.resolved.fonts
    );
    assert!(theme.resolved.workspace_fonts.is_empty());
    assert!(!theme.resolved.dark.contains_key(FONT_MONO_TOKEN));
    let json = serde_json::to_value(&theme.resolved).unwrap();
    assert!(json.get("workspace_fonts").is_none(), "{json}");
}

#[test]
fn get_resolves_a_workspace_theme_against_the_workspace_fonts() {
    let dir = tempfile::tempdir().unwrap();
    let font = dir.path().join("fonts/brand");
    std::fs::create_dir_all(&font).unwrap();
    std::fs::write(
        font.join("font.json"),
        r#"{"label":"Brand","group":"sans","faces":[{"file":"a.woff2"}]}"#,
    )
    .unwrap();
    std::fs::write(font.join("a.woff2"), b"wOF2 bytes").unwrap();
    std::fs::create_dir_all(dir.path().join(THEMES_DIR)).unwrap();
    std::fs::write(
        dir.path().join("themes/branded.json"),
        r#"{"name":"Branded","fonts":{"ui":"ws-brand"}}"#,
    )
    .unwrap();

    let theme = get(dir.path(), "branded").unwrap().unwrap();
    assert_eq!(theme.resolved.fonts.ui.as_deref(), Some("ws-brand"));
    assert_eq!(theme.resolved.workspace_fonts[0].id, "ws-brand");

    std::fs::remove_dir_all(&font).unwrap();
    let theme = get(dir.path(), "branded").unwrap().unwrap();
    assert!(theme.resolved.fonts.ui.is_none());
}

// --- Protected surfaces ---

fn colour(value: &str) -> color::Rgba {
    color::evaluate(value, &|_| None).unwrap_or_else(|| panic!("{value} is not a colour"))
}

/// Assert every rule the protected palette promises, for one mode's palette.
fn assert_palette_holds(palette: &[(&str, String)], context: &str) {
    let get = |name: &str| {
        let value = &palette
            .iter()
            .find(|(n, _)| *n == name)
            .unwrap_or_else(|| panic!("{context}: no {name}"))
            .1;
        assert!(!value.contains("var("), "{context}: {name} is {value}");
        colour(value)
    };
    let fills = [
        "--protected-bg",
        "--protected-surface",
        "--protected-raised",
    ]
    .map(get);
    for text in [
        "--protected-text",
        "--protected-text-muted",
        "--protected-accent",
        "--protected-caution",
        "--protected-confirm-text",
        "--protected-danger-text",
    ] {
        for fill in fills {
            let ratio = color::contrast(get(text), fill);
            assert!(ratio >= 4.5, "{context}: {text} is {ratio:.2}:1 on a fill");
        }
    }
    for (fill, label) in [
        ("--protected-confirm", "--protected-on-confirm"),
        ("--protected-danger", "--protected-on-danger"),
        ("--protected-action", "--protected-on-action"),
    ] {
        let ratio = color::contrast(get(fill), get(label));
        assert!(ratio >= 4.5, "{context}: {label} is {ratio:.2}:1 on {fill}");
    }
    for green in ["--protected-confirm", "--protected-confirm-text"] {
        assert!(
            protected::reads_as(GREEN_HUES, get(green)),
            "{context}: {green} is not green"
        );
    }
    for red in ["--protected-danger", "--protected-danger-text"] {
        assert!(
            protected::reads_as(RED_HUES, get(red)),
            "{context}: {red} is not red"
        );
    }
    assert!(
        !protected::reads_as(RED_HUES, get("--protected-action")),
        "{context}: the neutral action reads as destructive"
    );
    assert!(
        get("--protected-scrim").a >= 0.4,
        "{context}: the scrim does not dim"
    );
}

fn palette_of(pairs: &[(&str, &str)], mode: ThemeMode) -> Vec<(&'static str, String)> {
    protected_palette(&map(pairs), mode)
}

#[test]
fn every_built_in_theme_gets_a_palette_that_holds() {
    for theme in BUILT_INS.iter() {
        for mode in [ThemeMode::Dark, ThemeMode::Light] {
            let tokens = resolve_theme_tokens(&theme.definition, mode, None);
            assert_palette_holds(
                &protected_palette(&tokens, mode),
                &format!("{} {}", theme.id, mode.label()),
            );
        }
    }
}

/// A hovered row and a menu's keyboard cursor paint `--bg-hover` on the page
/// and on every popover, so the fill has to show on both.
#[test]
fn every_built_in_hover_fill_shows_on_the_page_and_on_a_surface() {
    const MIN_RATIO: f64 = 1.04;
    for theme in BUILT_INS.iter() {
        for mode in [ThemeMode::Dark, ThemeMode::Light] {
            let tokens = resolve_theme_tokens(&theme.definition, mode, None);
            let fill = |token: &str| colour_in(&tokens, mode, token).unwrap();
            let hover = fill("--bg-hover");
            for under in ["--bg-primary", "--surface-bg"] {
                let under_fill = fill(under);
                let ratio = color::contrast(hover.over(under_fill), under_fill);
                assert!(
                    ratio >= MIN_RATIO,
                    "{} {}: --bg-hover is {ratio:.3}:1 on {under}",
                    theme.id,
                    mode.label()
                );
            }
        }
    }
}

/// The clamp holds for any tokens at all, including ones validation refuses:
/// a style override never passes through the theme validator.
#[test]
fn a_hostile_palette_is_clamped_readable() {
    let hostile: &[(&str, &[(&str, &str)])] = &[
        (
            "text equals the fill",
            &[
                ("--bg-primary", "#202020"),
                ("--surface-bg", "#202020"),
                ("--bg-tertiary", "#202020"),
                ("--text-primary", "#202020"),
                ("--text-secondary", "#202020"),
            ],
        ),
        (
            "green and red swapped",
            &[("--accent-green", "#f85149"), ("--accent-red", "#3fb950")],
        ),
        (
            "grey confirm and deny",
            &[("--accent-green", "#808080"), ("--accent-red", "#808080")],
        ),
        ("transparent scrim", &[("--scrim", "transparent")]),
        ("white scrim", &[("--scrim", "rgba(255, 255, 255, 0.9)")]),
        (
            "surface equals the page",
            &[("--bg-primary", "#ffffff"), ("--surface-bg", "#ffffff")],
        ),
        (
            "fills straddle the middle",
            &[
                ("--bg-primary", "#000000"),
                ("--surface-bg", "#ffffff"),
                ("--bg-tertiary", "#777777"),
            ],
        ),
        ("mid-grey page", &[("--bg-primary", "#777777")]),
        (
            "red neutral action",
            &[
                ("--accent-action", "#ff0000"),
                ("--text-on-accent", "#ff0000"),
            ],
        ),
        (
            "invisible text",
            &[
                ("--text-primary", "transparent"),
                ("--accent", "rgba(0, 0, 0, 0)"),
            ],
        ),
    ];
    for (why, pairs) in hostile {
        for mode in [ThemeMode::Dark, ThemeMode::Light] {
            assert_palette_holds(
                &palette_of(pairs, mode),
                &format!("{why}, {}", mode.label()),
            );
        }
    }
}

/// A sweep across page, text and accent colours: whatever a theme picks, the
/// palette holds.
#[test]
fn the_palette_holds_across_a_sweep_of_themes() {
    let pages = [
        "#000000", "#0b1020", "#3a3a3a", "#6b6b6b", "#9a9a9a", "#e0d0b0", "#ffffff",
    ];
    let accents = [
        "#ff0000", "#ffcc00", "#00ff00", "#00ffff", "#0000ff", "#ff00ff", "#777777",
    ];
    for page in pages {
        for text in pages {
            for accent in accents {
                let pairs = [
                    ("--bg-primary", page),
                    ("--text-primary", text),
                    ("--accent", accent),
                    ("--accent-green", accent),
                    ("--accent-red", accent),
                    ("--accent-yellow", accent),
                ];
                for mode in [ThemeMode::Dark, ThemeMode::Light] {
                    assert_palette_holds(
                        &palette_of(&pairs, mode),
                        &format!("page {page} text {text} accent {accent}"),
                    );
                }
            }
        }
    }
}

#[test]
fn an_unevaluable_source_falls_back_to_the_default_theme() {
    let default = protected_palette(&TokenMap::new(), ThemeMode::Dark);
    let odd = palette_of(
        &[("--text-primary", "light-dark(#000, #fff)")],
        ThemeMode::Dark,
    );
    assert_eq!(odd, default);
}

#[test]
fn a_theme_paints_the_palette_and_the_default_theme_leaves_it_to_the_stylesheet() {
    let d = def(r##"{"name":"x","dark":{"--accent":"#88c0d0"}}"##);
    let dark = resolve_mode(&d, ThemeMode::Dark, None);
    for name in protected::PROTECTED_TOKENS
        .into_iter()
        .chain([PROTECTED_FONT_MONO_TOKEN])
    {
        assert!(dark.contains_key(name), "{name}");
    }
    assert!(
        resolve_mode(&d, ThemeMode::Light, None).is_empty(),
        "an untouched mode paints nothing"
    );
}

/// The default theme paints nothing inline, so `base.css` must carry exactly
/// the palette the engine derives for it.
#[test]
fn base_css_carries_the_default_protected_palette() {
    let css = include_str!("../../../../lucidos-app/src/styles/global/base.css");
    for (mode, selector) in [
        (ThemeMode::Dark, "html, html[data-theme-mode=\"dark\"] {"),
        (ThemeMode::Light, "html[data-theme-mode=\"light\"] {"),
    ] {
        let start = css.find(selector).expect("theme block");
        let block = &css[start..start + css[start..].find("\n}").expect("block end")];
        let mut expected = protected_palette(&TokenMap::new(), mode);
        let default_theme = ThemeDefinition {
            name: "Default".to_string(),
            description: String::new(),
            author: String::new(),
            credit: String::new(),
            family: None,
            tokens: TokenMap::new(),
            dark: None,
            light: None,
            fonts: ThemeFonts::default(),
            parts: PartsMap::new(),
        };
        expected.push((
            PROTECTED_FONT_MONO_TOKEN,
            protected_font_mono(&default_theme).to_string(),
        ));
        let wanted: String = expected
            .iter()
            .map(|(name, value)| format!("    {name}: {value};\n"))
            .collect();
        for (name, value) in &expected {
            assert!(
                block.contains(&format!("    {name}: {value};")),
                "base.css `{selector}` needs {name}: {value}. The whole block:\n{wanted}"
            );
        }
    }
}

// --- Refusal: a theme far enough off to be hostile ---

#[test]
fn a_theme_may_name_catalog_tokens_only() {
    let err = refusal(r#"{"name":"x","tokens":{"--z-modal":"0"}}"#);
    assert!(err.contains("--z-modal is not a theme token"), "{err}");
    let err = refusal(r#"{"name":"x","tokens":{"--font-size-md":"1px"}}"#);
    assert!(err.contains("not a theme token"), "{err}");
    let err = refusal(r##"{"name":"x","dark":{"--protected-text":"#000000"}}"##);
    assert!(err.contains("Lucidos computes --protected-text"), "{err}");
}

#[test]
fn text_the_page_hides_is_refused() {
    let err =
        refusal(r##"{"name":"x","dark":{"--bg-primary":"#101010","--text-primary":"#101010"}}"##);
    assert!(
        err.contains("in dark mode, --text-primary on --bg-primary is 1.0:1"),
        "{err}"
    );
    // A shared page colour applies in both modes, and the light default text
    // is dark, so the light mode is refused too.
    let err = refusal(r##"{"name":"x","tokens":{"--bg-primary":"#101010"}}"##);
    assert!(err.contains("in light mode"), "{err}");
}

#[test]
fn faint_but_legible_text_is_accepted_and_clamped_on_approval_surfaces() {
    // About 3.5:1: past the refusal floor, short of AA.
    let d = def(r##"{"name":"x","dark":{"--bg-primary":"#202020","--text-primary":"#808080"}}"##);
    let dark = resolve_mode(&d, ThemeMode::Dark, None);
    let text = colour(&dark["--protected-text"]);
    let bg = colour(&dark["--protected-bg"]);
    assert!(color::contrast(text, bg) >= 4.5);
    assert_ne!(dark["--protected-text"], "#808080", "the clamp moved it");
}

#[test]
fn swapped_green_and_red_are_refused() {
    let err =
        refusal(r##"{"name":"x","dark":{"--accent-green":"#f85149","--accent-red":"#3fb950"}}"##);
    assert!(err.contains("--accent-green reads as red"), "{err}");
    let err = refusal(r##"{"name":"x","light":{"--accent-red":"#1a7f37"}}"##);
    assert!(
        err.contains("in light mode, --accent-red reads as green"),
        "{err}"
    );
}

#[test]
fn a_transparent_scrim_is_accepted_and_clamped_on_approval_surfaces() {
    let d = def(
        r#"{"name":"x","tokens":{"--scrim":"transparent","--surface-bg":"var(--bg-primary)"}}"#,
    );
    for mode in [ThemeMode::Dark, ThemeMode::Light] {
        let scrim = colour(&resolve_mode(&d, mode, None)["--protected-scrim"]);
        assert!(scrim.a >= 0.4, "{}", mode.label());
    }
}

#[test]
fn a_shadow_that_reaches_far_outside_its_box_is_refused() {
    for (value, why) in [
        ("0 0 0 100vmax #000", "100vmax"),
        ("0 0 0 40px red", "40px"),
        ("inset 0 0 0 3rem #fff", "48px"),
        ("32px 0 32px 32px #000", "80px"),
        ("0 0 abs(-9999px) #000", "abs()"),
        ("0 0 0 1 #000", "in a unit"),
        ("rgb(0 0 0)999px 0 0", "cannot be read"),
        ("0 0 rgb(0 0 0)var(--x)", "cannot be read"),
        ("0 0 0 var(--radius-surface) #000", "var()"),
        ("0 0 calc(1px * 999) #000", "calc()"),
        ("0 4px 8px red, 0 0 0 999px blue", "999px"),
    ] {
        let json = format!(r#"{{"name":"x","tokens":{{"--shadow-lg":"{value}"}}}}"#);
        let err = refusal(&json);
        assert!(err.contains("the shadow --shadow-lg"), "{value}: {err}");
        assert!(err.contains(why), "{value}: {err}");
    }
    for value in [
        "0 8px 32px rgba(0, 0, 0, 0.4)",
        "0 -0.5rem 1.5rem -0.25rem rgba(0, 0, 0, 0.1)",
        "0 0 0 0.1875rem color-mix(in srgb, var(--accent) 55%, transparent)",
        "none",
        "rgba(51, 255, 51, 0.6)",
    ] {
        let json = format!(r#"{{"name":"x","tokens":{{"--shadow-lg":"{value}"}}}}"#);
        validate_workspace_write("mine.json", json.as_bytes(), WorkspaceFonts::default)
            .unwrap_or_else(|e| panic!("{value}: {e}"));
    }
}

/// A command on an approval card shows in a catalog code font or the default
/// stack, never a free-form `--font-mono` a theme named.
#[test]
fn the_protected_code_font_is_a_catalog_stack() {
    let free = def(r#"{"name":"x","tokens":{"--font-mono":"'Wingdings', monospace"}}"#);
    let dark = resolve_mode(&free, ThemeMode::Dark, None);
    assert_eq!(dark[FONT_MONO_TOKEN], "'Wingdings', monospace");
    assert!(!dark[PROTECTED_FONT_MONO_TOKEN].contains("Wingdings"));
    let catalog = def(r#"{"name":"x","fonts":{"mono":"geist-mono"}}"#);
    let theme = build(
        "x",
        ThemeSource::Workspace,
        catalog,
        &WorkspaceFonts::default(),
    );
    assert_eq!(
        theme.resolved.dark[PROTECTED_FONT_MONO_TOKEN],
        fonts::find("geist-mono").unwrap().stack
    );
}

#[test]
fn a_theme_write_is_validated_and_other_paths_are_not() {
    let theme = br##"{"name":"Mine","dark":{"--accent":"#ff0000"}}"##;
    assert!(validate_data_write(
        std::path::Path::new("/nonexistent"),
        "themes/mine.json",
        theme
    )
    .is_ok());
    assert!(
        validate_data_write(
            std::path::Path::new("/nonexistent"),
            "themes/nord.json",
            theme
        )
        .is_err(),
        "built-in id"
    );
    assert!(validate_data_write(
        std::path::Path::new("/nonexistent"),
        "themes/mine.json",
        b"not json"
    )
    .is_err());
    assert!(
        validate_data_write(
            std::path::Path::new("/nonexistent"),
            "themes/mine.json",
            br#"{"name":"x","tokens":{"--accent":"url(x)"}}"#
        )
        .is_err(),
        "banned value"
    );
    let unknown_font = validate_data_write(
        std::path::Path::new("/nonexistent"),
        "themes/mine.json",
        br#"{"name":"x","fonts":{"ui":"no-such-font"}}"#,
    )
    .expect_err("an unknown font id");
    assert!(unknown_font.contains("not a font id"), "{unknown_font}");
    assert!(validate_data_write(
        std::path::Path::new("/nonexistent"),
        "themes/mine.json",
        br#"{"name":"x","fonts":{"ui":"geist"}}"#
    )
    .is_ok());
    assert!(validate_data_write(
        std::path::Path::new("/nonexistent"),
        "artifacts/mine.json",
        b"not json"
    )
    .is_ok());
    // A sibling whose name only starts with `themes` is not under `themes/`.
    assert!(validate_data_write(
        std::path::Path::new("/nonexistent"),
        "themesy/mine.json",
        b"not json"
    )
    .is_ok());
}

#[test]
fn is_theme_path_matches_the_folder_and_nothing_beside_it() {
    assert!(is_theme_path("themes/harbour.json"));
    assert!(!is_theme_path("themes"));
    assert!(!is_theme_path("themesy/harbour.json"));
    assert!(!is_theme_path("artifacts/themes/harbour.json"));
}

#[test]
fn a_theme_mode_value_is_never_a_theme_id() {
    for mode in THEME_MODE_VALUES {
        let err = validate_id(mode).expect_err("a mode value must be refused");
        assert!(
            err.to_string()
                .contains(crate::core::prefs::THEME_MODE.key()),
            "{err}"
        );
    }
    assert!(validate_id("nord").is_ok());
}
