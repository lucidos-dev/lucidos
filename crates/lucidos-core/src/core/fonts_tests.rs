use std::collections::HashSet;
use std::path::PathBuf;

use super::*;

#[test]
fn ids_are_unique_kebab_case_and_never_the_follow_theme_value() {
    let mut seen = HashSet::new();
    for font in FONT_CATALOG {
        assert!(seen.insert(font.id), "duplicate id {}", font.id);
        assert_ne!(font.id, FOLLOW_THEME);
        assert!(
            crate::core::themes::is_well_formed_id(font.id),
            "'{}' is not lowercase kebab-case",
            font.id
        );
    }
}

#[test]
fn the_preference_accepts_follow_theme_then_every_id() {
    assert_eq!(FONT_PREFERENCE_VALUES[0], FOLLOW_THEME);
    let ids: Vec<&str> = FONT_CATALOG.iter().map(|f| f.id).collect();
    assert_eq!(&FONT_PREFERENCE_VALUES[1..], ids.as_slice());
}

#[test]
fn every_stack_passes_the_theme_token_gate() {
    for font in FONT_CATALOG {
        assert!(
            crate::core::themes::is_valid_token_value(font.stack),
            "{}: the stack is too long or uses a banned form",
            font.id
        );
    }
}

#[test]
fn the_fallback_is_vendored_and_fits_ui_text() {
    let fallback = find(FALLBACK_FONT).expect("the fallback is in the catalog");
    assert_eq!(fallback.source_name(), "vendored");
    assert!(fallback.kind.fits_ui());
}

#[test]
fn every_vendored_face_is_an_embedded_woff2() {
    let mut served = HashSet::new();
    for font in FONT_CATALOG {
        for face in font.faces() {
            assert!(
                face.bytes.len() > 5_000,
                "{}: {} bytes",
                face.asset,
                face.bytes.len()
            );
            assert_eq!(&face.bytes[..4], b"wOF2", "{}", face.asset);
            assert!(served.insert(face.served), "{} served twice", face.served);
            assert!(
                face.served.starts_with(font.id) && face.served.ends_with(".woff2"),
                "{}: a served name is <id>-<version>…woff2",
                face.served
            );
            assert!(
                face.served.chars().any(|c| c.is_ascii_digit()),
                "{}: the served name carries the version, which makes it immutable",
                face.served
            );
        }
    }
}

#[test]
fn every_font_is_bundled_or_on_the_device() {
    for font in FONT_CATALOG {
        match font.source {
            FontSource::Device => assert_eq!(font.license, "none", "{}", font.id),
            FontSource::Vendored { .. } => assert!(!font.faces().is_empty(), "{}", font.id),
        }
    }
}

#[test]
fn the_catalog_holds_exactly_these_fonts_in_group_order() {
    let ids: Vec<&str> = FONT_CATALOG.iter().map(|f| f.id).collect();
    assert_eq!(
        ids,
        [
            "system",
            "geist",
            "atkinson-hyperlegible-next",
            "inter",
            "roboto",
            "open-sans",
            "manrope",
            "source-serif-4",
            "lora",
            "literata",
            "fira-code",
            "monospace",
            "geist-mono",
            "atkinson-hyperlegible-mono",
            "jetbrains-mono",
            "ibm-plex-mono",
            "source-code-pro",
            "commit-mono",
            "cascadia-code",
            "vt323",
        ]
    );
    let groups: Vec<FontGroup> = FONT_CATALOG.iter().map(|f| f.group()).collect();
    let mut sorted = groups.clone();
    sorted.sort_by_key(|g| *g as u8);
    assert_eq!(groups, sorted, "Settings lists the catalog grouped");
}

#[test]
fn every_stack_ends_in_a_generic_family() {
    for font in FONT_CATALOG {
        let generic = font.stack.rsplit(',').next().unwrap().trim();
        assert!(
            ["sans-serif", "serif", "monospace"].contains(&generic),
            "{}: ends in {generic}",
            font.id
        );
    }
}

#[test]
fn the_mono_group_is_exactly_the_fonts_fit_for_code() {
    for font in FONT_CATALOG {
        assert_eq!(
            font.group() == FontGroup::Mono,
            font.kind.fits_code(),
            "{}",
            font.id
        );
    }
}

#[test]
fn each_new_font_is_bundled_in_its_group() {
    for (id, group) in [
        ("inter", FontGroup::Sans),
        ("roboto", FontGroup::Sans),
        ("open-sans", FontGroup::Sans),
        ("manrope", FontGroup::Sans),
        ("lora", FontGroup::Serif),
        ("literata", FontGroup::Serif),
        ("jetbrains-mono", FontGroup::Mono),
        ("ibm-plex-mono", FontGroup::Mono),
        ("source-code-pro", FontGroup::Mono),
        ("commit-mono", FontGroup::Mono),
        ("cascadia-code", FontGroup::Mono),
        ("vt323", FontGroup::Mono),
    ] {
        let font = find(id).unwrap_or_else(|| panic!("{id} is in the catalog"));
        assert_eq!(font.source_name(), "vendored", "{id}");
        assert!(!font.faces().is_empty(), "{id}");
        assert_eq!(font.group(), group, "{id}");
        assert!(font.kind.fits_ui(), "{id}: fit for UI text");
    }
}

#[test]
fn the_fonts_with_programming_ligatures_are_flagged() {
    let flagged: Vec<&str> = FONT_CATALOG
        .iter()
        .filter(|f| f.ligatures)
        .map(|f| f.id)
        .collect();
    assert_eq!(flagged, ["fira-code", "jetbrains-mono", "cascadia-code"]);
}

#[test]
fn no_catalog_id_uses_the_workspace_font_prefix() {
    use crate::core::workspace_fonts::ID_PREFIX;
    for font in FONT_CATALOG {
        assert!(!font.id.starts_with(ID_PREFIX), "{}", font.id);
    }
    assert!(!FOLLOW_THEME.starts_with(ID_PREFIX));
}

#[test]
fn the_catalog_json_lists_workspace_fonts_after_the_catalog() {
    use crate::core::workspace_fonts::list_in;
    let dir = tempfile::tempdir().unwrap();
    let font_dir = dir.path().join("brand");
    std::fs::create_dir_all(&font_dir).unwrap();
    std::fs::write(
        font_dir.join("font.json"),
        r#"{"label":"Brand","group":"mono","faces":[{"file":"a.woff2"}]}"#,
    )
    .unwrap();
    std::fs::write(font_dir.join("a.woff2"), b"wOF2 bytes").unwrap();
    std::fs::create_dir_all(dir.path().join("broken")).unwrap();
    std::fs::write(dir.path().join("broken/font.json"), "{").unwrap();

    let json = catalog_json(&list_in(dir.path()));
    let fonts = json["fonts"].as_array().unwrap();
    assert_eq!(fonts.len(), FONT_CATALOG.len() + 1);
    let brand = fonts.last().unwrap();
    assert_eq!(brand["id"], "ws-brand");
    assert_eq!(brand["source"], "workspace");
    assert_eq!(brand["kind"], "both");
    assert_eq!(brand["group"], "mono");
    assert_eq!(brand["family"], "ws-brand");
    assert_eq!(brand["faces"][0]["path"], "fonts/brand/a.woff2");
    assert_eq!(brand["theme_nameable"], true);
    assert!(fonts[0].get("faces").is_none() && fonts[0].get("family").is_none());
    assert_eq!(json["invalid"][0]["id"], "ws-broken");
}

#[test]
fn the_catalog_json_lists_every_font() {
    let json = catalog_json(&Default::default());
    let fonts = json["fonts"].as_array().expect("a fonts array");
    assert_eq!(fonts.len(), FONT_CATALOG.len());
    assert_eq!(json["invalid"], serde_json::json!([]));
    assert_eq!(json["follow_theme"], FOLLOW_THEME);
    assert_eq!(json["fallback"], FALLBACK_FONT);
    let inter = fonts.iter().find(|f| f["id"] == "inter").unwrap();
    assert_eq!(inter["source"], "vendored");
    assert_eq!(inter["kind"], "ui");
    assert_eq!(inter["group"], "sans");
    let lora = fonts.iter().find(|f| f["id"] == "lora").unwrap();
    assert_eq!(lora["group"], "serif");
    assert!(fonts.iter().all(|f| f["theme_nameable"] == true));
}

#[test]
fn only_vt323_overrides_its_metrics() {
    let overridden: Vec<&str> = FONT_CATALOG
        .iter()
        .filter(|f| f.metrics.is_some())
        .map(|f| f.id)
        .collect();
    assert_eq!(overridden, ["vt323"]);
}

#[test]
fn every_vt323_face_carries_the_metric_overrides() {
    let rules = find("vt323")
        .unwrap()
        .font_face_rules(|face| face.served.to_string());
    for descriptor in [
        "size-adjust: 134.6%;",
        "ascent-override: 65.7%;",
        "descent-override: 25.7%;",
        "line-gap-override: 0%;",
    ] {
        assert_eq!(
            rules.matches(descriptor).count(),
            2,
            "both VT323 faces declare {descriptor}\n{rules}"
        );
    }
    let fira = find("fira-code")
        .unwrap()
        .font_face_rules(|face| face.served.to_string());
    assert!(
        !fira.contains("override") && !fira.contains("size-adjust"),
        "{fira}"
    );
}

#[test]
fn a_font_with_no_bold_face_declares_its_face_over_every_weight() {
    let without_bold: Vec<&str> = FONT_CATALOG
        .iter()
        .filter(|f| !f.has_bold())
        .map(|f| f.id)
        .collect();
    assert_eq!(without_bold, ["vt323"]);

    let vt323 = find("vt323")
        .unwrap()
        .font_face_rules(|face| face.served.to_string());
    assert_eq!(vt323.matches("font-weight: 100 900;").count(), 2, "{vt323}");
    let fira = find("fira-code")
        .unwrap()
        .font_face_rules(|face| face.served.to_string());
    assert!(fira.contains("font-weight: 300 700;"), "{fira}");
    let plex = find("ibm-plex-mono")
        .unwrap()
        .font_face_rules(|face| face.served.to_string());
    assert!(
        plex.contains("font-weight: 400;") && plex.contains("font-weight: 700;"),
        "{plex}"
    );
}

#[test]
fn the_catalog_json_says_which_fonts_have_a_bold_face() {
    let json = catalog_json(&Default::default());
    let fonts = json["fonts"].as_array().expect("a fonts array");
    let bold = |id: &str| fonts.iter().find(|f| f["id"] == id).unwrap()["bold"].clone();
    assert_eq!(bold("vt323"), false);
    assert_eq!(bold("fira-code"), true);
    assert_eq!(bold("system"), true);
}

#[test]
fn a_face_reaches_bold_from_weight_600() {
    assert!(weight_reaches_bold("600"));
    assert!(weight_reaches_bold("300 700"));
    assert!(!weight_reaches_bold("300 500"));
    assert!(!weight_reaches_bold("400"));
    assert!(!weight_reaches_bold("not a weight"));
}

mod guard_tests {
    use super::*;

    fn repo_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .expect("repo root")
            .to_path_buf()
    }

    #[test]
    fn every_vendored_font_ships_its_license_text() {
        let dir = repo_root().join("crates/lucidos-app/src/assets/fonts");
        for font in FONT_CATALOG {
            let FontSource::Vendored { family, .. } = font.source else {
                continue;
            };
            let file = dir.join(format!("LICENSE-{}.txt", family.replace(' ', "")));
            let text = std::fs::read_to_string(&file)
                .unwrap_or_else(|_| panic!("{}: missing {}", font.id, file.display()));
            assert!(
                text.contains("SIL OPEN FONT LICENSE Version 1.1"),
                "{}",
                file.display()
            );
            assert_eq!(font.license, "OFL-1.1", "{}", font.id);
        }
    }

    #[test]
    fn the_knowhow_names_every_font_id() {
        for doc in ["system-knowhow/preferences.md", "system-knowhow/js-sdk.md"] {
            let text = crate::core::knowhow::doc_family_text(&repo_root(), doc);
            for value in FONT_PREFERENCE_VALUES {
                assert!(
                    text.contains(&format!("`{value}`")),
                    "{doc} never names `{value}`"
                );
            }
        }
    }

    #[test]
    fn the_themes_knowhow_names_every_font() {
        let text = crate::core::knowhow::doc_family_text(&repo_root(), "system-knowhow/themes.md");
        for font in FONT_CATALOG {
            assert!(
                text.contains(&format!("`{}`", font.id)),
                "themes.md never names `{}`",
                font.id
            );
        }
    }
}
