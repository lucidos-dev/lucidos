use super::*;

fn rules(path: &str, content: &str) -> Vec<(usize, AppLintRule)> {
    lint_app_file(path, content)
        .into_iter()
        .map(|f| (f.line, f.rule))
        .collect()
}

fn secondary_without_base() -> AppLintRule {
    AppLintRule::VariantWithoutBase {
        variant: SECONDARY_CLASS.to_string(),
    }
}

// ── Which files the lint reads ────────────────────────────────────────────

#[test]
fn only_app_html_css_and_js_files_are_linted() {
    let ungated = "a:hover { color: red; }";
    assert_eq!(rules("apps/demo/style.css", ungated).len(), 1);
    assert_eq!(rules("apps/demo/sub/dir/style.CSS", ungated).len(), 1);
    assert!(rules("artifacts/demo/style.css", ungated).is_empty());
    assert!(rules("apps/demo/manifest.json", ungated).is_empty());
    assert!(rules("apps/style.css", ungated).is_empty());
    assert!(rules("apps//style.css", ungated).is_empty());
}

#[test]
fn a_clean_file_and_a_non_app_path_get_no_note() {
    let clean = "@media (hover: hover) { a:hover { color: red; } }";
    assert_eq!(app_lint_note("apps/demo/style.css", clean), None);
    assert_eq!(
        app_lint_note("artifacts/page.html", "<style>a:hover{}</style>"),
        None
    );
}

// ── ungated-hover ─────────────────────────────────────────────────────────

#[test]
fn an_ungated_hover_is_found_on_its_line() {
    let css = ".row { padding: 0; }\n\n.row:hover {\n  background: var(--bg-secondary);\n}\n";
    assert_eq!(
        rules("apps/demo/style.css", css),
        vec![(3, AppLintRule::UngatedHover)]
    );
}

#[test]
fn a_hover_inside_a_hover_media_query_is_clean() {
    for gate in [
        "@media (hover: hover)",
        "@media (hover:hover)",
        "@media (hover: hover) and (pointer: fine)",
        "@media screen and (any-hover: hover)",
    ] {
        let css = format!("{gate} {{\n  .row:hover {{ background: red; }}\n}}\n");
        assert!(rules("apps/demo/a.css", &css).is_empty(), "{gate}");
    }
}

#[test]
fn a_hover_inside_a_non_hover_media_query_is_still_ungated() {
    for gate in [
        "@media (max-width: 40rem)",
        "@media (hover: none)",
        "@media not (hover: hover)",
    ] {
        let css = format!("{gate} {{\n  .row:hover {{ background: red; }}\n}}\n");
        assert_eq!(
            rules("apps/demo/a.css", &css),
            vec![(2, AppLintRule::UngatedHover)],
            "{gate}"
        );
    }
}

#[test]
fn a_gate_reaches_a_hover_nested_deeper() {
    let css = "@media (hover: hover) {\n  @supports (display: grid) {\n    .a:hover { color: red; }\n  }\n  .b { &:hover { color: red; } }\n}\n";
    assert!(rules("apps/demo/a.css", css).is_empty());
}

#[test]
fn a_nested_hover_outside_any_gate_is_found() {
    let css = ".b {\n  color: blue;\n  &:hover { color: red; }\n}\n";
    assert_eq!(
        rules("apps/demo/a.css", css),
        vec![(3, AppLintRule::UngatedHover)]
    );
}

#[test]
fn a_hover_in_a_comment_or_string_is_ignored() {
    let css = "/* .a:hover { color: red; } */\n.b::after { content: \".c:hover {\"; }\n";
    assert!(rules("apps/demo/a.css", css).is_empty());
}

#[test]
fn hovers_in_an_html_style_block_are_found_on_the_file_line() {
    let html = "<!doctype html>\n<html>\n<head>\n<style>\n  .ok { color: red; }\n  button:hover { background: grey; }\n</style>\n</head>\n<body></body>\n</html>\n";
    assert_eq!(
        rules("apps/demo/index.html", html),
        vec![(6, AppLintRule::UngatedHover)]
    );
}

#[test]
fn a_style_block_inside_an_html_comment_is_ignored() {
    let html = "<!--\n<style> a:hover { color: red; } </style>\n-->\n<p>hi</p>\n";
    assert!(rules("apps/demo/index.html", html).is_empty());
}

// ── action-btn-as-state ───────────────────────────────────────────────────

#[test]
fn a_classlist_toggle_of_the_secondary_variant_is_found() {
    let js = "for (const b of buttons) {\n  b.classList.toggle('action-btn-secondary', b.dataset.sort !== sort);\n}\n";
    assert_eq!(
        rules("apps/demo/app.js", js),
        vec![(2, AppLintRule::ActionBtnAsState)]
    );
}

#[test]
fn a_classlist_remove_or_replace_of_the_variant_is_found() {
    let js = "el.classList.remove(\"action-btn-secondary\");\nother.classList.replace('action-btn', 'action-btn-secondary');\n";
    assert_eq!(
        rules("apps/demo/app.js", js),
        vec![
            (1, AppLintRule::ActionBtnAsState),
            (2, AppLintRule::ActionBtnAsState)
        ]
    );
}

#[test]
fn a_ternary_picking_the_variant_by_selection_is_found() {
    let js =
        "btn.className = s === state.sort ? 'action-btn' : 'action-btn action-btn-secondary';\n";
    assert_eq!(
        rules("apps/demo/app.js", js),
        vec![(1, AppLintRule::ActionBtnAsState)]
    );
}

#[test]
fn a_template_ternary_in_a_script_block_is_found() {
    let html = "<script>\nconst row = (f) => `<button class=\"action-btn ${f.active ? '' : 'action-btn-secondary'}\">${f.label}</button>`;\n</script>\n";
    assert_eq!(
        rules("apps/demo/index.html", html),
        vec![(2, AppLintRule::ActionBtnAsState)]
    );
}

#[test]
fn an_if_else_assigning_the_variant_by_selection_is_found() {
    let js = "if (view === current) {\n  btn.className = 'action-btn';\n} else {\n  btn.className = 'action-btn action-btn-secondary';\n}\n";
    assert_eq!(
        rules("apps/demo/app.js", js),
        vec![(4, AppLintRule::ActionBtnAsState)]
    );
}

#[test]
fn a_segmented_control_or_pill_filter_is_clean() {
    let js = "seg.querySelectorAll('.segmented-btn').forEach((b) => b.classList.toggle('active', b.dataset.view === view));\npill.setAttribute('aria-pressed', String(on));\n";
    assert!(rules("apps/demo/app.js", js).is_empty());
}

#[test]
fn building_a_secondary_button_once_is_clean() {
    let js = "const cancel = document.createElement('button');\ncancel.classList.add('action-btn', 'action-btn-secondary');\ncancel.className = 'action-btn action-btn-secondary';\n";
    assert!(rules("apps/demo/app.js", js).is_empty());
}

#[test]
fn a_ternary_choosing_the_variant_from_data_is_clean() {
    let js = "const cls = b.primary ? 'action-btn' : 'action-btn action-btn-secondary';\n";
    assert!(rules("apps/demo/app.js", js).is_empty());
}

#[test]
fn a_swap_in_a_comment_is_ignored() {
    let js = "// b.classList.toggle('action-btn-secondary', !on);\n/* el.classList.remove('action-btn') */\nconst url = 'https://example.com//x';\n";
    assert!(rules("apps/demo/app.js", js).is_empty());
}

// ── action-btn-variant-without-base ───────────────────────────────────────

#[test]
fn a_lone_variant_in_markup_is_found() {
    let html = "<div>\n  <button class=\"action-btn-secondary\">Cancel</button>\n  <button class='action-btn-danger'>Delete</button>\n</div>\n";
    assert_eq!(
        rules("apps/demo/index.html", html),
        vec![
            (2, secondary_without_base()),
            (
                3,
                AppLintRule::VariantWithoutBase {
                    variant: "action-btn-danger".to_string()
                }
            )
        ]
    );
}

#[test]
fn a_lone_variant_in_a_class_name_assignment_is_found() {
    let js = "const b = document.createElement('button');\nb.className = 'action-btn-confirm';\n";
    assert_eq!(
        rules("apps/demo/app.js", js),
        vec![(
            2,
            AppLintRule::VariantWithoutBase {
                variant: "action-btn-confirm".to_string()
            }
        )]
    );
}

#[test]
fn a_variant_with_its_base_is_clean() {
    let html = "<button class=\"action-btn action-btn-secondary\">Cancel</button>\n<button class=\"action-btn action-btn-confirm\">Apply</button>\n<button class=\"action-btn\">Save</button>\n<script>\nb.className = 'action-btn action-btn-danger';\nconst h = `<button class=\"${'action-btn'} action-btn-secondary\">x</button>`;\n</script>\n";
    assert!(rules("apps/demo/index.html", html).is_empty());
}

#[test]
fn a_lone_variant_in_an_html_comment_is_ignored() {
    let html = "<!-- <button class=\"action-btn-secondary\">old</button> -->\n<p>hi</p>\n";
    assert!(rules("apps/demo/index.html", html).is_empty());
}

#[test]
fn a_class_selector_in_css_is_not_a_class_list() {
    let css = ".action-btn-secondary { margin: 0; }\n";
    assert!(rules("apps/demo/a.css", css).is_empty());
}

// ── The note ──────────────────────────────────────────────────────────────

#[test]
fn the_note_names_the_path_each_line_and_its_fix() {
    let html = "<style>\nbutton:hover { background: grey; }\n</style>\n<button class=\"action-btn-secondary\">x</button>\n";
    let note = app_lint_note("apps/demo/index.html", html).expect("two findings");
    assert!(note.starts_with("\n\n[APP LINT] apps/demo/index.html: 2 problem(s)."));
    assert!(note.contains("line 2: a `:hover` rule outside `@media (hover: hover)`"));
    assert!(note.contains("line 4: `action-btn-secondary` without the base `action-btn`"));
    assert!(note.contains("write `class=\"action-btn action-btn-secondary\"`"));
}

#[test]
fn the_note_lists_at_most_the_cap_and_counts_the_rest() {
    let css: String = (0..MAX_LISTED_FINDINGS + 3)
        .map(|i| format!(".r{i}:hover {{ color: red; }}\n"))
        .collect();
    let note = app_lint_note("apps/demo/a.css", &css).expect("findings");
    assert_eq!(note.matches("\n  line ").count(), MAX_LISTED_FINDINGS);
    assert!(note.ends_with("…and 3 more of the same kinds."));
}

#[test]
fn a_file_with_multibyte_text_keeps_its_line_numbers() {
    let css = "/* Schöne Grüße 👋 */\n.a::before { content: \"→\"; }\n.a:hover { color: red; }\n";
    assert_eq!(
        rules("apps/demo/a.css", css),
        vec![(3, AppLintRule::UngatedHover)]
    );
}

#[test]
fn a_repeated_finding_on_one_line_is_listed_once() {
    let html = "<button class=\"action-btn-danger\">A</button><button class=\"action-btn-secondary\">B</button><button class=\"action-btn-danger\">C</button>\n";
    assert_eq!(
        rules("apps/demo/index.html", html),
        vec![
            (
                1,
                AppLintRule::VariantWithoutBase {
                    variant: "action-btn-danger".to_string()
                }
            ),
            (1, secondary_without_base())
        ]
    );
}

#[test]
fn a_media_query_with_a_branch_that_allows_touch_is_no_gate() {
    for gate in [
        "@media (hover: hover), (max-width: 40rem)",
        "@media (max-width: 40rem), (hover: hover)",
        "@media (hover: hover) or (max-width: 40rem)",
    ] {
        let css = format!("{gate} {{\n  .row:hover {{ background: red; }}\n}}\n");
        assert_eq!(
            rules("apps/demo/a.css", &css),
            vec![(2, AppLintRule::UngatedHover)],
            "{gate}"
        );
    }
    let both = "@media (hover: hover), (any-hover: hover) {\n  .row:hover { color: red; }\n}\n";
    assert!(rules("apps/demo/a.css", both).is_empty());
}

#[test]
fn a_class_attribute_selector_is_not_a_class_list() {
    let html = "<style>\n  [class=\"action-btn-secondary\"] { margin: 0; }\n</style>\n<script>\nconst b = document.querySelector('[class=\"action-btn-danger\"]');\nconst c = document.querySelector('button[ class = \"action-btn-confirm\" ]');\n</script>\n";
    assert!(rules("apps/demo/index.html", html).is_empty());
    let js = "const b = document.querySelector('[class=\"action-btn-danger\"]');\n";
    assert!(rules("apps/demo/app.js", js).is_empty());
}
