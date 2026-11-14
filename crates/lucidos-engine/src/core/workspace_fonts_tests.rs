use super::*;

const WOFF2: &[u8] = b"wOF2 the rest of a font";
const TTF: &[u8] = b"\x00\x01\x00\x00 the rest of a font";
const OTF: &[u8] = b"OTTO the rest of a font";

fn manifest(group: &str, files: &[&str]) -> String {
    let faces: Vec<String> = files
        .iter()
        .map(|f| format!(r#"{{"file":"{f}","weight":"400"}}"#))
        .collect();
    format!(
        r#"{{"label":"Brand Sans","group":"{group}","faces":[{}]}}"#,
        faces.join(",")
    )
}

/// Write a font directory under `root` with a manifest and the given files.
fn install(root: &Path, slug: &str, manifest: &str, files: &[(&str, &[u8])]) {
    let dir = root.join(slug);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(MANIFEST_FILE), manifest).unwrap();
    for (name, bytes) in files {
        std::fs::write(dir.join(name), bytes).unwrap();
    }
}

fn reason_for(fonts: &WorkspaceFonts, id: &str) -> String {
    assert!(fonts.find(id).is_none(), "{id} should be invalid");
    fonts
        .invalid
        .iter()
        .find(|f| f.id == id)
        .unwrap_or_else(|| panic!("{id} is neither listed nor reported: {fonts:?}"))
        .reason
        .clone()
}

// --- No third-party request ---

/// A face is a leaf file name, so nothing in a manifest can point anywhere
/// but the font's own directory on the local engine.
#[test]
fn a_face_file_can_name_nothing_but_a_leaf_font_file() {
    for file in [
        "../evil.woff2",
        "sub/font.woff2",
        "sub\\font.woff2",
        "/etc/font.woff2",
        "https://cdn.example.com/font.woff2",
        "//cdn.example.com/font.woff2",
        "url(x).woff2",
        "font.woff2?x=1",
        "font.woff2#x",
        "font woff2.woff2",
        ".hidden.woff2",
        "",
        "font.html",
        "font.svg",
        "font.ttc",
        "font",
    ] {
        let json = manifest("sans", &[file]);
        assert!(
            parse_manifest(json.as_bytes()).is_err(),
            "accepted {file:?}"
        );
    }
    for file in ["Brand-Regular.woff2", "brand_bold.WOFF", "b.ttf", "b.1.otf"] {
        let json = manifest("sans", &[file]);
        assert!(parse_manifest(json.as_bytes()).is_ok(), "refused {file:?}");
    }
}

/// A URL has nowhere to go in a manifest.
#[test]
fn a_manifest_refuses_any_field_it_does_not_define() {
    let json = r#"{"label":"x","group":"sans","faces":[{"file":"a.woff2"}],"url":"https://cdn.example.com/a.woff2"}"#;
    assert!(parse_manifest(json.as_bytes()).is_err());
    let json = r#"{"label":"x","group":"sans","faces":[{"file":"a.woff2","src":"https://cdn.example.com/a.woff2"}]}"#;
    assert!(parse_manifest(json.as_bytes()).is_err());
}

// --- No user CSS ---

/// The stack is laid as a theme token, so it must pass the token value gate at
/// the longest slug, for every group.
#[test]
fn every_stack_passes_the_theme_token_gate_at_the_longest_slug() {
    let dir = tempfile::tempdir().unwrap();
    let slug = "a".repeat(MAX_SLUG_LEN);
    for group in ["sans", "serif", "mono"] {
        install(
            dir.path(),
            &slug,
            &manifest(group, &["a.woff2"]),
            &[("a.woff2", WOFF2)],
        );
        let fonts = list_in(dir.path());
        let font = &fonts.fonts[0];
        assert!(
            super::super::themes::is_valid_token_value(&font.stack),
            "{group}: {}",
            font.stack
        );
    }
}

#[test]
fn a_hostile_label_stays_out_of_the_stack() {
    let dir = tempfile::tempdir().unwrap();
    let label = r#"x'; } body { background: url(https://evil.example) } /*"#;
    let json = format!(
        r#"{{"label":{},"group":"sans","faces":[{{"file":"a.woff2"}}]}}"#,
        serde_json::to_string(label).unwrap()
    );
    install(dir.path(), "brand", &json, &[("a.woff2", WOFF2)]);
    let fonts = list_in(dir.path());
    let font = fonts.find("ws-brand").expect("the label is only text");
    assert_eq!(font.label, label);
    assert_eq!(font.family, "ws-brand");
    assert_eq!(
        font.stack,
        "'ws-brand', system-ui, -apple-system, 'Segoe UI', sans-serif"
    );
}

// --- The group decides the slots ---

#[test]
fn sans_and_serif_are_ui_fonts_and_mono_fits_both() {
    let dir = tempfile::tempdir().unwrap();
    for group in ["sans", "serif", "mono"] {
        install(
            dir.path(),
            group,
            &manifest(group, &["a.woff2"]),
            &[("a.woff2", WOFF2)],
        );
    }
    let fonts = list_in(dir.path());
    let kind = |id: &str| fonts.find(id).unwrap().kind;
    assert_eq!(kind("ws-sans"), FontKind::Ui);
    assert_eq!(kind("ws-serif"), FontKind::Ui);
    assert_eq!(kind("ws-mono"), FontKind::Both);
    assert_eq!(fonts.find("ws-serif").unwrap().group, FontGroup::Serif);
    assert!(fonts.find("ws-mono").unwrap().stack.ends_with("monospace"));
}

// --- Listing is the authority ---

#[test]
fn a_valid_font_lists_with_its_faces_as_data_paths() {
    let dir = tempfile::tempdir().unwrap();
    let json = r#"{"label":"Brand Sans","group":"sans","ligatures":true,"license":"OFL-1.1","faces":[
        {"file":"Brand.woff2","weight":"100 900"},
        {"file":"Brand-Italic.ttf","weight":"400","style":"italic"}]}"#;
    install(
        dir.path(),
        "brand-sans",
        json,
        &[("Brand.woff2", WOFF2), ("Brand-Italic.ttf", TTF)],
    );
    let fonts = list_in(dir.path());
    assert!(fonts.invalid.is_empty(), "{:?}", fonts.invalid);
    let font = fonts.find("ws-brand-sans").unwrap();
    assert_eq!(font.label, "Brand Sans");
    assert_eq!(font.license, "OFL-1.1");
    assert!(font.ligatures);
    assert_eq!(
        font.faces,
        vec![
            WorkspaceFontFace {
                path: "fonts/brand-sans/Brand.woff2".into(),
                weight: "100 900".into(),
                style: FaceStyle::Normal,
            },
            WorkspaceFontFace {
                path: "fonts/brand-sans/Brand-Italic.ttf".into(),
                weight: "400".into(),
                style: FaceStyle::Italic,
            },
        ]
    );
}

#[test]
fn each_broken_font_is_left_out_and_reported_with_its_reason() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    install(root, "missing-face", &manifest("sans", &["a.woff2"]), &[]);
    install(
        root,
        "wrong-magic",
        &manifest("sans", &["a.woff2"]),
        &[("a.woff2", b"<html>not a font")],
    );
    install(
        root,
        "mislabelled",
        &manifest("sans", &["a.woff2"]),
        &[("a.woff2", OTF)],
    );
    install(
        root,
        "collection",
        &manifest("sans", &["a.ttf"]),
        &[("a.ttf", b"ttcf a collection")],
    );
    install(root, "bad-manifest", "{not json", &[("a.woff2", WOFF2)]);
    let too_many: Vec<String> = (0..=MAX_FACES).map(|i| format!("f{i}.woff2")).collect();
    let too_many: Vec<&str> = too_many.iter().map(String::as_str).collect();
    install(root, "too-many-faces", &manifest("sans", &too_many), &[]);
    std::fs::create_dir_all(root.join("no-manifest")).unwrap();
    std::fs::write(root.join("no-manifest/a.woff2"), WOFF2).unwrap();
    install(
        root,
        "Bad_Slug",
        &manifest("sans", &["a.woff2"]),
        &[("a.woff2", WOFF2)],
    );
    install(
        root,
        "good",
        &manifest("sans", &["a.woff2"]),
        &[("a.woff2", WOFF2)],
    );

    let fonts = list_in(root);
    assert_eq!(fonts.fonts.len(), 1, "{:?}", fonts.fonts);
    assert!(fonts.find("ws-good").is_some());
    assert!(reason_for(&fonts, "ws-missing-face").contains("missing"));
    assert!(reason_for(&fonts, "ws-wrong-magic").contains("not a woff2"));
    assert!(reason_for(&fonts, "ws-mislabelled").contains("different font format"));
    assert!(reason_for(&fonts, "ws-collection").contains("collection"));
    assert!(reason_for(&fonts, "ws-bad-manifest").contains("not a valid font.json"));
    assert!(reason_for(&fonts, "ws-too-many-faces").contains(&format!("1 to {MAX_FACES}")));
    assert!(reason_for(&fonts, "ws-no-manifest").contains("no font.json"));
    assert!(reason_for(&fonts, "ws-Bad_Slug").contains("not a font directory name"));
}

#[test]
fn an_oversize_face_file_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    install(dir.path(), "big", &manifest("sans", &["a.woff2"]), &[]);
    let file = std::fs::File::create(dir.path().join("big/a.woff2")).unwrap();
    use std::io::Write;
    (&file).write_all(WOFF2).unwrap();
    file.set_len(MAX_FILE_BYTES + 1).unwrap();
    let fonts = list_in(dir.path());
    assert!(reason_for(&fonts, "ws-big").contains("MiB"));
}

/// Removing a font's files leaves its directory behind. That is no font at
/// all, not a broken one.
#[test]
fn an_empty_or_hidden_directory_is_not_a_font() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("removed")).unwrap();
    install(
        dir.path(),
        ".hidden",
        &manifest("sans", &["a.woff2"]),
        &[("a.woff2", WOFF2)],
    );
    assert_eq!(list_in(dir.path()), WorkspaceFonts::default());
}

#[cfg(unix)]
#[test]
fn a_symlink_never_becomes_a_font_or_a_face() {
    let outside = tempfile::tempdir().unwrap();
    install(
        outside.path(),
        "elsewhere",
        &manifest("sans", &["a.woff2"]),
        &[("a.woff2", WOFF2)],
    );
    let dir = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(outside.path().join("elsewhere"), dir.path().join("linked"))
        .unwrap();
    install(
        dir.path(),
        "face-link",
        &manifest("sans", &["a.woff2"]),
        &[],
    );
    std::os::unix::fs::symlink(
        outside.path().join("elsewhere/a.woff2"),
        dir.path().join("face-link/a.woff2"),
    )
    .unwrap();

    let fonts = list_in(dir.path());
    assert!(fonts.find("ws-linked").is_none());
    assert!(!fonts.invalid.iter().any(|f| f.id == "ws-linked"));
    assert!(reason_for(&fonts, "ws-face-link").contains("not a regular file"));
}

#[test]
fn a_workspace_holds_at_most_the_font_cap() {
    let dir = tempfile::tempdir().unwrap();
    for i in 0..=MAX_FONTS {
        install(
            dir.path(),
            &format!("f{i:03}"),
            &manifest("sans", &["a.woff2"]),
            &[("a.woff2", WOFF2)],
        );
    }
    let fonts = list_in(dir.path());
    assert_eq!(fonts.fonts.len(), MAX_FONTS);
    assert!(reason_for(&fonts, &format!("ws-f{MAX_FONTS:03}")).contains("at most"));
}

#[test]
fn a_missing_fonts_directory_holds_no_fonts() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(list(dir.path()), WorkspaceFonts::default());
}

// --- The manifest's own rules ---

#[test]
fn a_manifest_checks_its_label_weights_and_faces() {
    let refused = [
        r#"{"label":"","group":"sans","faces":[{"file":"a.woff2"}]}"#,
        r#"{"label":"x","group":"sans","faces":[]}"#,
        r#"{"label":"x","group":"script","faces":[{"file":"a.woff2"}]}"#,
        r#"{"label":"x","group":"sans","faces":[{"file":"a.woff2","weight":"0"}]}"#,
        r#"{"label":"x","group":"sans","faces":[{"file":"a.woff2","weight":"1001"}]}"#,
        r#"{"label":"x","group":"sans","faces":[{"file":"a.woff2","weight":"900 100"}]}"#,
        r#"{"label":"x","group":"sans","faces":[{"file":"a.woff2","weight":"bold"}]}"#,
        r#"{"label":"x","group":"sans","faces":[{"file":"a.woff2","style":"oblique"}]}"#,
        r#"{"label":"a\u0007b","group":"sans","faces":[{"file":"a.woff2"}]}"#,
    ];
    for json in refused {
        assert!(parse_manifest(json.as_bytes()).is_err(), "accepted {json}");
    }
    let long_label = format!(
        r#"{{"label":"{}","group":"sans","faces":[{{"file":"a.woff2"}}]}}"#,
        "x".repeat(MAX_LABEL_CHARS + 1)
    );
    assert!(parse_manifest(long_label.as_bytes()).is_err());

    let parsed =
        parse_manifest(br#"{"label":"x","group":"mono","faces":[{"file":"a.woff2"}]}"#).unwrap();
    assert_eq!(parsed.faces[0].weight, "400");
    assert_eq!(parsed.faces[0].style, FaceStyle::Normal);
}

// --- Ids ---

#[test]
fn a_workspace_id_is_the_prefix_and_a_slug() {
    assert!(is_workspace_id("ws-brand"));
    assert!(is_workspace_id("ws-brand-sans-2"));
    for id in [
        "brand",
        "ws-",
        "ws-Brand",
        "ws--x",
        "ws-x-",
        "ws-a/b",
        "fira-code",
    ] {
        assert!(!is_workspace_id(id), "{id}");
    }
    let longest = format!("ws-{}", "a".repeat(MAX_SLUG_LEN));
    assert!(is_workspace_id(&longest));
    assert!(!is_workspace_id(&format!("{longest}a")));
}

// --- The data route's write gate ---

#[test]
fn the_write_gate_accepts_only_a_manifest_or_a_font_file() {
    let json = manifest("sans", &["a.woff2"]);
    assert!(validate_write("brand/font.json", json.as_bytes()).is_ok());
    assert!(validate_write("brand/a.woff2", WOFF2).is_ok());
    assert!(validate_write("brand/a.otf", OTF).is_ok());

    for (rel, bytes) in [
        ("font.json", json.as_bytes()),
        ("a.woff2", WOFF2),
        ("brand/font.json", b"{}".as_slice()),
        ("brand/index.html", b"<script>".as_slice()),
        ("brand/a.woff2", b"<html>".as_slice()),
        ("brand/a.woff2", OTF),
        ("brand/a.woff2", b"".as_slice()),
        ("brand/sub/a.woff2", WOFF2),
        ("Brand/a.woff2", WOFF2),
        ("../a.woff2", WOFF2),
    ] {
        assert!(validate_write(rel, bytes).is_err(), "accepted {rel}");
    }
}

/// Either sfnt outline type may sit behind either extension, as browsers
/// load them. Only the container must match.
#[test]
fn a_ttf_or_otf_may_hold_either_outline_type() {
    assert!(validate_write("brand/a.otf", TTF).is_ok());
    assert!(validate_write("brand/a.ttf", OTF).is_ok());
    assert!(validate_write("brand/a.woff2", TTF).is_err());
}

#[test]
fn a_weight_is_plain_digits() {
    for weight in ["+400", "0400 ", " 400", "4e2", "00400", "400 +700"] {
        let json = format!(
            r#"{{"label":"x","group":"sans","faces":[{{"file":"a.woff2","weight":"{weight}"}}]}}"#
        );
        assert!(
            parse_manifest(json.as_bytes()).is_err(),
            "accepted {weight:?}"
        );
    }
}

#[cfg(unix)]
#[test]
fn a_symlinked_or_oversize_manifest_is_refused_before_it_is_read() {
    let outside = tempfile::tempdir().unwrap();
    let target = outside.path().join("secret.json");
    std::fs::write(&target, r#"{"host_secret": 1}"#).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let linked = dir.path().join("linked");
    std::fs::create_dir_all(&linked).unwrap();
    std::os::unix::fs::symlink(&target, linked.join(MANIFEST_FILE)).unwrap();
    std::fs::write(linked.join("a.woff2"), WOFF2).unwrap();
    let big = dir.path().join("big");
    std::fs::create_dir_all(&big).unwrap();
    std::fs::write(big.join(MANIFEST_FILE), " ".repeat(MAX_MANIFEST_BYTES + 1)).unwrap();

    let fonts = list_in(dir.path());
    let reason = reason_for(&fonts, "ws-linked");
    assert!(reason.contains("not a regular file"), "{reason}");
    assert!(!reason.contains("host_secret"), "{reason}");
    assert!(reason_for(&fonts, "ws-big").contains("larger than"));
}

/// A new font past the cap is refused where it is written, so it can never
/// push an installed font out of the list. Editing an installed one still works.
#[test]
fn a_new_manifest_is_refused_once_the_workspace_is_full() {
    let data = tempfile::tempdir().unwrap();
    let root = data.path().join(FONTS_DIR);
    for i in 0..MAX_FONTS {
        install(
            &root,
            &format!("f{i:03}"),
            &manifest("sans", &["a.woff2"]),
            &[("a.woff2", WOFF2)],
        );
    }
    let json = manifest("sans", &["a.woff2"]);
    let err = validate_data_write(data.path(), "fonts/aaa/font.json", json.as_bytes()).unwrap_err();
    assert!(err.contains("at most"), "{err}");
    assert!(validate_data_write(data.path(), "fonts/f000/font.json", json.as_bytes()).is_ok());
    assert!(validate_data_write(data.path(), "fonts/aaa/a.woff2", WOFF2).is_ok());
}

/// Several fonts arriving together, as a plugin brings them, need room for
/// every new one. A font already installed is a replacement and takes none.
#[test]
fn room_counts_only_the_fonts_not_yet_installed() {
    let data = tempfile::tempdir().unwrap();
    let root = data.path().join(FONTS_DIR);
    for i in 0..MAX_FONTS - 1 {
        install(
            &root,
            &format!("f{i:03}"),
            &manifest("sans", &["a.woff2"]),
            &[("a.woff2", WOFF2)],
        );
    }
    assert!(check_room(data.path(), ["new-a", "f000"]).is_ok());
    let err = check_room(data.path(), ["new-a", "new-b"]).unwrap_err();
    assert!(err.contains("at most"), "{err}");
    assert!(err.contains("remove 1"), "{err}");
}

/// Bold is judged on upright faces: a bold italic alone would still leave
/// upright bold text to be smeared.
#[test]
fn only_an_upright_bold_face_counts_as_bold() {
    let font = |faces: &[(&str, FaceStyle)]| WorkspaceFont {
        id: "ws-pixel".into(),
        label: "Pixel".into(),
        family: "ws-pixel".into(),
        stack: "'ws-pixel', monospace".into(),
        kind: FontKind::Both,
        group: FontGroup::Mono,
        license: "OFL-1.1".into(),
        ligatures: false,
        faces: faces
            .iter()
            .map(|(weight, style)| WorkspaceFontFace {
                path: "fonts/pixel/Pixel.woff2".into(),
                weight: (*weight).into(),
                style: *style,
            })
            .collect(),
    };
    assert!(!font(&[("400", FaceStyle::Normal)]).has_bold());
    assert!(!font(&[("400", FaceStyle::Normal), ("700", FaceStyle::Italic)]).has_bold());
    assert!(font(&[("400", FaceStyle::Normal), ("700", FaceStyle::Normal)]).has_bold());
    assert!(font(&[("100 900", FaceStyle::Normal)]).has_bold());
}
