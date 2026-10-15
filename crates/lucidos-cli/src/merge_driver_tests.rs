use super::*;

fn run_merge(kind: Kind, base: &str, ours: &str, theirs: &str) -> Merge {
    merge(kind, base.as_bytes(), ours.as_bytes(), theirs.as_bytes(), 7)
}

fn clean(m: Merge) -> String {
    match m {
        Merge::Clean(out) => String::from_utf8(out).unwrap(),
        other => panic!("expected a clean merge, got {other:?}"),
    }
}

fn conflicted(m: Merge) -> String {
    match m {
        Merge::Conflicted(out) => String::from_utf8(out).unwrap(),
        other => panic!("expected a conflict, got {other:?}"),
    }
}

fn unkeyable(m: Merge) -> String {
    match m {
        Merge::Unkeyable(why) => why,
        other => panic!("expected an unkeyable file, got {other:?}"),
    }
}

const INDEX_HEAD: &str = "# ADR index\n\nOne line per decision.\n\n";

fn index(lines: &[&str]) -> String {
    let mut s = INDEX_HEAD.to_string();
    for l in lines {
        s.push_str(&format!("- [{l}: text {l}]({l}-x.md)\n"));
    }
    s
}

#[test]
fn sorted_lines_keeps_two_concurrent_appends_in_number_order() {
    let out = run_merge(
        Kind::SortedLines,
        &index(&["0438"]),
        &index(&["0438", "0441"]),
        &index(&["0438", "0440"]),
    );
    assert_eq!(clean(out), index(&["0438", "0440", "0441"]));
}

#[test]
fn sorted_lines_keeps_one_copy_of_a_line_both_sides_hold_at_different_places() {
    // Union merge duplicated exactly this: one side had re-sorted already.
    let out = run_merge(
        Kind::SortedLines,
        &index(&["0437", "0438"]),
        &index(&["0437", "0438", "0439", "0440"]),
        &index(&["0437", "0439", "0438"]),
    );
    assert_eq!(clean(out), index(&["0437", "0438", "0439", "0440"]));
}

#[test]
fn sorted_lines_conflicts_on_one_number_claimed_twice_and_merges_the_rest() {
    let base = index(&["0001"]);
    let ours = format!("{base}- [0002: ours](0002-a.md)\n- [0003: text 0003](0003-x.md)\n");
    let theirs = format!("{base}- [0002: theirs](0002-b.md)\n");
    let out = conflicted(run_merge(Kind::SortedLines, &base, &ours, &theirs));
    let expected = format!(
        "{base}<<<<<<< ours\n- [0002: ours](0002-a.md)\n=======\n- [0002: theirs](0002-b.md)\n>>>>>>> theirs\n- [0003: text 0003](0003-x.md)\n"
    );
    assert_eq!(out, expected);
}

#[test]
fn sorted_lines_takes_a_preamble_edit_from_either_side() {
    let base = index(&["0001"]);
    let ours = index(&["0001", "0002"]);
    let theirs = base.replace("One line per decision.", "One line per ADR.");
    let out = clean(run_merge(Kind::SortedLines, &base, &ours, &theirs));
    assert_eq!(
        out,
        index(&["0001", "0002"]).replace("One line per decision.", "One line per ADR.")
    );
}

#[test]
fn sorted_lines_refuses_a_stray_line_after_the_entries() {
    let base = index(&["0001"]);
    let ours = format!("{base}\nA trailing paragraph.\n");
    let why = unkeyable(run_merge(Kind::SortedLines, &base, &ours, &base));
    assert!(why.contains("not a numbered"), "{why}");
}

#[test]
fn sorted_lines_refuses_a_number_listed_twice_on_one_side() {
    let base = index(&["0001"]);
    let ours = index(&["0001", "0001"]);
    let why = unkeyable(run_merge(Kind::SortedLines, &base, &ours, &base));
    assert!(why.contains("share the key 1"), "{why}");
}

const PRIORS_BASE: &str = "# Priors\n\nIntro.\n\n## Rust engine\n\n- **Old entry.** Body\n  continues.\n\n## Frontend\n\n- **Front entry.** Body.\n";

#[test]
fn blocks_keeps_both_entries_added_at_the_top_of_one_section() {
    // The hot anchor: every agent adds its prior right under the heading.
    let ours = PRIORS_BASE.replace("## Rust engine\n", "## Rust engine\n\n- **Ours.** New.\n");
    let theirs = PRIORS_BASE.replace("## Rust engine\n", "## Rust engine\n\n- **Theirs.** New.\n");
    let out = clean(run_merge(Kind::Blocks, PRIORS_BASE, &ours, &theirs));
    let expected = PRIORS_BASE.replace(
        "## Rust engine\n",
        "## Rust engine\n\n- **Ours.** New.\n\n- **Theirs.** New.\n",
    );
    assert_eq!(out, expected);
}

#[test]
fn blocks_keeps_a_tight_list_tight_when_both_sides_append() {
    let base = "## Events\n\n- a\n- b\n\n## Next\n";
    let ours = "## Events\n\n- a\n- b\n- c\n\n## Next\n";
    let theirs = "## Events\n\n- a\n- b\n- d\n\n## Next\n";
    let out = clean(run_merge(Kind::Blocks, base, ours, theirs));
    assert_eq!(out, "## Events\n\n- a\n- b\n- c\n- d\n\n## Next\n");
}

#[test]
fn blocks_conflicts_only_on_the_entry_both_sides_edited() {
    let ours = PRIORS_BASE
        .replace("Body\n  continues.", "Ours edit.")
        .replace("Body.", "Front edit.");
    let theirs = PRIORS_BASE.replace("Body\n  continues.", "Theirs edit.");
    let out = conflicted(run_merge(Kind::Blocks, PRIORS_BASE, &ours, &theirs));
    assert!(out.contains("<<<<<<< ours\n\n- **Old entry.** Ours edit.\n=======\n\n- **Old entry.** Theirs edit.\n>>>>>>> theirs\n"), "{out}");
    assert!(out.contains("- **Front entry.** Front edit.\n"), "{out}");
    assert_eq!(out.matches("<<<<<<<").count(), 1);
}

#[test]
fn blocks_merges_edits_to_different_lines_of_one_entry() {
    // db.md shape: two threads each add a sub-bullet to one long entry.
    let base =
        "## Events\n\n- **Changes:** a list.\n  - first\n  - second\n  - third\n  - fourth\n";
    let ours = base.replace("  - first\n", "  - first\n  - ours\n");
    let theirs = base.replace("  - fourth\n", "  - fourth\n  - theirs\n");
    let out = clean(run_merge(Kind::Blocks, base, &ours, &theirs));
    assert_eq!(
        out,
        ours.replace("  - fourth\n", "  - fourth\n  - theirs\n")
    );
}

#[test]
fn blocks_conflicts_when_one_side_deletes_an_entry_the_other_edited() {
    let ours = PRIORS_BASE.replace("\n- **Front entry.** Body.\n", "\n");
    let theirs = PRIORS_BASE.replace("Body.", "Edited.");
    let out = conflicted(run_merge(Kind::Blocks, PRIORS_BASE, &ours, &theirs));
    assert!(
        out.contains("<<<<<<< ours\n=======\n\n- **Front entry.** Edited.\n>>>>>>> theirs\n"),
        "{out}"
    );
}

#[test]
fn blocks_drops_an_entry_one_side_deleted_and_the_other_left_alone() {
    let ours = PRIORS_BASE.replace("\n- **Front entry.** Body.\n", "\n");
    let out = clean(run_merge(Kind::Blocks, PRIORS_BASE, &ours, PRIORS_BASE));
    assert!(!out.contains("Front entry"), "{out}");
}

#[test]
fn blocks_keys_bullets_by_the_full_heading_path() {
    // CHANGELOG repeats `### Added` under every version.
    let base = "# Changelog\n\n## v2\n\n### Added\n\n- Two.\n\n## v1\n\n### Added\n\n- One.\n";
    let ours = base.replace("- Two.\n", "- Two.\n- Two more.\n");
    let theirs = base.replace("- One.\n", "- One.\n- One more.\n");
    let out = clean(run_merge(Kind::Blocks, base, &ours, &theirs));
    assert_eq!(
        out,
        base.replace("- Two.\n", "- Two.\n- Two more.\n")
            .replace("- One.\n", "- One.\n- One more.\n")
    );
}

#[test]
fn blocks_keeps_each_new_entry_in_the_section_its_side_put_it_in() {
    // Both sides opened `## Unreleased`; theirs also carries a release cut on main.
    let base = "# C\n\n## v2\n\n- old\n";
    let ours = "# C\n\n## Unreleased\n\n### Changed\n\n- ours change\n\n### Fixed\n\n- ours fix\n\n## v2\n\n- old\n";
    let theirs = "# C\n\n## Unreleased\n\n### Changed\n\n- theirs change\n\n### Fixed\n\n- theirs fix\n\n## v3\n\n- v3 fix\n\n## v2\n\n- old\n";
    let out = clean(run_merge(Kind::Blocks, base, ours, theirs));
    assert_eq!(
        out,
        "# C\n\n## Unreleased\n\n### Changed\n\n- ours change\n- theirs change\n\n### Fixed\n\n- ours fix\n- theirs fix\n\n## v3\n\n- v3 fix\n\n## v2\n\n- old\n"
    );
}

#[test]
fn check_sections_refuses_an_entry_under_the_wrong_heading() {
    let key = |s: &str| s.replace('>', &KEY_SEP.to_string());
    let order = [key("h:## A"), key("h:## B"), key("b:## A>moved")];
    let why = check_sections(&order).unwrap_err();
    assert!(why.contains("moved"), "{why}");
}

#[test]
fn blocks_never_joins_an_entry_that_ended_its_file_without_a_newline() {
    let base = "## A\n\n- a";
    let ours = "## A\n\n- a\n- ours";
    let theirs = "## A\n\n- a\n- theirs";
    let out = clean(run_merge(Kind::Blocks, base, ours, theirs));
    assert_eq!(out, "## A\n\n- a\n- ours\n- theirs");
}

#[test]
fn sorted_lines_never_joins_a_last_line_without_a_newline() {
    let base = "# I\n\n- [0001: a](1.md)";
    let ours = "# I\n\n- [0001: a](1.md)\n- [0003: c](3.md)";
    let theirs = "# I\n\n- [0001: a](1.md)\n- [0002: b](2.md)\n";
    let out = clean(run_merge(Kind::SortedLines, base, ours, theirs));
    assert_eq!(
        out,
        "# I\n\n- [0001: a](1.md)\n- [0002: b](2.md)\n- [0003: c](3.md)"
    );
}

#[test]
fn blocks_keeps_one_copy_of_a_paragraph_after_a_list_both_sides_appended_to() {
    let base = "## A\n\n- one\n- two\n\nA closing paragraph.\n";
    let ours = "## A\n\n- one\n- two\n- ours\n\nA closing paragraph.\n";
    let theirs = "## A\n\n- one\n- two\n- theirs\n\nA closing paragraph.\n";
    let out = clean(run_merge(Kind::Blocks, base, ours, theirs));
    assert_eq!(
        out,
        "## A\n\n- one\n- two\n- ours\n- theirs\n\nA closing paragraph.\n"
    );
}

#[test]
fn blocks_keeps_an_indented_paragraph_inside_its_bullet() {
    let doc = parse_blocks("## A\n\n- one\n\n  More about one.\n").unwrap();
    assert_eq!(doc.entries.len(), 3);
    assert!(doc.entries[2].1.contains("More about one."));
}

#[test]
fn blocks_refuses_one_entry_renamed_differently_on_both_sides() {
    let base = "## Changed\n\n- A thing changed.\n- Another.\n";
    let ours = "## Changed\n\n- A thing changed, ours.\n- Another.\n";
    let theirs = "## Changed\n\n- A thing changed, theirs.\n- Another.\n";
    let why = unkeyable(run_merge(Kind::Blocks, base, ours, theirs));
    assert!(why.contains("removed or renamed"), "{why}");
}

#[test]
fn blocks_refuses_an_entry_renamed_on_one_side_and_deleted_on_the_other() {
    let base = "## T\n\n### Token\nBody.\n\n### Other\nX.\n";
    let ours = "## T\n\n### Hook token\nBody.\n\n### Other\nX.\n";
    let theirs = "## T\n\n### Other\nX.\n";
    let why = unkeyable(run_merge(Kind::Blocks, base, ours, theirs));
    assert!(why.contains("### Token"), "{why}");
}

#[test]
fn blocks_takes_a_rename_the_other_side_left_alone() {
    let base = "## Changed\n\n- A thing changed.\n- Another.\n";
    let ours = "## Changed\n\n- A thing changed, reworded.\n- Another.\n";
    let theirs = "## Changed\n\n- A thing changed.\n- Another.\n- New.\n";
    let out = clean(run_merge(Kind::Blocks, base, ours, theirs));
    assert_eq!(
        out,
        "## Changed\n\n- A thing changed, reworded.\n- Another.\n- New.\n"
    );
}

#[test]
fn blocks_refuses_a_trailing_rule_both_sides_carried_past_their_appended_entry() {
    let base = "## 2\n\n### Last\n\n- x\n\n---\n\n## 3\n";
    let ours = "## 2\n\n### Last\n\n- x\n\n### NewO\n\n- o\n\n---\n\n## 3\n";
    let theirs = "## 2\n\n### Last\n\n- x\n\n### NewT\n\n- t\n\n---\n\n## 3\n";
    let why = unkeyable(run_merge(Kind::Blocks, base, ours, theirs));
    assert!(why.contains("---"), "{why}");
}

#[test]
fn blocks_refuses_a_heading_rename_against_a_deleted_bullet_under_it() {
    let why = unkeyable(run_merge(
        Kind::Blocks,
        "## A\n- x\n- y\n",
        "## B\n- x\n- y\n",
        "## A\n- x\n",
    ));
    assert!(why.contains("removed or renamed"), "{why}");
}

#[test]
fn blocks_does_not_split_inside_a_code_fence() {
    let base = "## A\n\nText.\n\n```md\n# not a heading\n- not a bullet\n```\n";
    let doc = parse_blocks(base).unwrap();
    let keys: Vec<&str> = doc.keys().collect();
    assert_eq!(keys, vec![PREAMBLE, "h:## A"]);
}

#[test]
fn blocks_reads_yaml_frontmatter_as_preamble() {
    let base = "---\npaths:\n- \"x/**\"\n---\n\n# Title\n";
    let doc = parse_blocks(base).unwrap();
    assert_eq!(
        doc.entries[0],
        (
            PREAMBLE.to_string(),
            "---\npaths:\n- \"x/**\"\n---\n".to_string()
        )
    );
    assert_eq!(doc.entries[1].0, "h:# Title");
}

#[test]
fn blocks_refuses_a_file_with_two_entries_under_one_key() {
    let base = "## Terms\n\n### Token\nOne.\n\n### Token\nTwo.\n";
    let why = unkeyable(run_merge(Kind::Blocks, base, base, base));
    assert!(why.contains("### Token"), "{why}");
}

#[test]
fn blocks_keeps_one_sides_reorder_and_the_others_addition() {
    let base = "## A\n\n- a\n\n## B\n\n- b\n";
    let ours = "## B\n\n- b\n\n## A\n\n- a\n";
    let theirs = "## A\n\n- a\n- a2\n\n## B\n\n- b\n";
    let out = clean(run_merge(Kind::Blocks, base, ours, theirs));
    assert_eq!(out, "## B\n\n- b\n\n## A\n\n- a\n- a2\n");
}

#[test]
fn blocks_refuses_when_both_sides_reorder_differently() {
    let base = "## A\n## B\n## C\n";
    let why = unkeyable(run_merge(
        Kind::Blocks,
        base,
        "## B\n## A\n## C\n",
        "## A\n## C\n## B\n",
    ));
    assert!(why.contains("reordered"), "{why}");
}

#[test]
fn run_writes_the_result_over_ours_and_reports_a_conflict_with_exit_1() {
    let dir = tempfile::tempdir().unwrap();
    let (b, o, t) = (
        dir.path().join("b"),
        dir.path().join("o"),
        dir.path().join("t"),
    );
    fs::write(&b, "- [0001: a]\n").unwrap();
    fs::write(&o, "- [0001: b]\n").unwrap();
    fs::write(&t, "- [0001: c]\n").unwrap();
    assert_eq!(run(Kind::SortedLines, &b, &o, &t, 7), 1);
    assert!(fs::read_to_string(&o).unwrap().contains("<<<<<<< ours"));
    assert!(!dir.path().join("o.lucidos-merge").exists());
}

#[test]
fn run_leaves_ours_untouched_and_exits_2_when_it_cannot_key_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let (b, o, t) = (
        dir.path().join("b"),
        dir.path().join("o"),
        dir.path().join("t"),
    );
    let dup = "### Token\nOne.\n\n### Token\nTwo.\n";
    fs::write(&b, dup).unwrap();
    fs::write(&o, dup).unwrap();
    fs::write(&t, "### Token\n").unwrap();
    assert_eq!(run(Kind::Blocks, &b, &o, &t, 7), NOT_MERGED);
    assert_eq!(fs::read_to_string(&o).unwrap(), dup);
}

#[test]
fn run_exits_2_when_an_input_is_missing() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing");
    assert_eq!(
        run(Kind::Blocks, &missing, &missing, &missing, 7),
        NOT_MERGED
    );
}

/// Every keep / edit / delete / add combination over three entries, checked
/// against the textbook rule: a side that did not change an entry yields to
/// the side that did, and two different changes conflict. The output must
/// also re-parse to exactly the expected entries.
#[test]
fn blocks_matches_the_three_way_rule_for_every_combination() {
    #[derive(Clone, Copy)]
    enum Side {
        Keep,
        Edit(u8),
        Delete,
    }
    let sides = [Side::Keep, Side::Edit(1), Side::Edit(2), Side::Delete];
    let names = ["a", "b", "c"];
    let entry = |name: &str, version: u8| format!("- **{name}** v{version}\n");
    let doc_for =
        |present: &[bool; 3], choice: &[Side; 3]| -> (String, HashMap<String, Option<String>>) {
            let mut text = "## List\n\n".to_string();
            let mut state = HashMap::new();
            for i in 0..3 {
                let line = match (present[i], choice[i]) {
                    (true, Side::Keep) => Some(entry(names[i], 0)),
                    (_, Side::Edit(v)) => Some(entry(names[i], v)),
                    (false, Side::Keep) | (_, Side::Delete) => None,
                };
                if let Some(l) = &line {
                    text.push_str(l);
                }
                state.insert(names[i].to_string(), line);
            }
            (text, state)
        };
    let mut checked = 0;
    for mask in 0..8u8 {
        let present = [mask & 1 != 0, mask & 2 != 0, mask & 4 != 0];
        let (base, base_state) = doc_for(&present, &[Side::Keep; 3]);
        for ours_pick in 0..64 {
            for theirs_pick in 0..64 {
                let pick = |n: usize| [sides[n % 4], sides[(n / 4) % 4], sides[(n / 16) % 4]];
                let (ours, ours_state) = doc_for(&present, &pick(ours_pick));
                let (theirs, theirs_state) = doc_for(&present, &pick(theirs_pick));
                let mut expect_conflict = false;
                let mut expected: Vec<(String, String)> = Vec::new();
                for name in names {
                    let (b, o, t) = (&base_state[name], &ours_state[name], &theirs_state[name]);
                    let result = if o == b {
                        t
                    } else if t == b || o == t {
                        o
                    } else {
                        expect_conflict = true;
                        o
                    };
                    if let Some(line) = result {
                        expected.push((format!("b:## List{KEY_SEP}{name}"), line.clone()));
                    }
                }
                // Gone from both sides beside a new entry: refused, since it
                // may be a rename (`renamed_on_both_sides`).
                let gone_from_both = names.iter().any(|n| {
                    base_state[*n].is_some()
                        && ours_state[*n].is_none()
                        && theirs_state[*n].is_none()
                });
                let added = names.iter().any(|n| {
                    base_state[*n].is_none()
                        && (ours_state[*n].is_some() || theirs_state[*n].is_some())
                });
                let expect_refusal = gone_from_both && added;
                match run_merge(Kind::Blocks, &base, &ours, &theirs) {
                    Merge::Unkeyable(_) if expect_refusal => {}
                    _ if expect_refusal => panic!("missed a refusal:\n{base}\n{ours}\n{theirs}"),
                    Merge::Clean(out) => {
                        assert!(
                            !expect_conflict,
                            "missed a conflict:\n{base}\n{ours}\n{theirs}"
                        );
                        let doc = parse_blocks(std::str::from_utf8(&out).unwrap()).unwrap();
                        // The first bullet carries the blank line under the heading.
                        let mut got: Vec<(String, String)> = doc.entries[2..]
                            .iter()
                            .map(|(k, t)| (k.clone(), t.trim_start_matches('\n').to_string()))
                            .collect();
                        got.sort();
                        expected.sort();
                        assert_eq!(got, expected, "base:\n{base}ours:\n{ours}theirs:\n{theirs}");
                    }
                    Merge::Conflicted(_) => {
                        assert!(
                            expect_conflict,
                            "spurious conflict:\n{base}\n{ours}\n{theirs}"
                        )
                    }
                    Merge::Unkeyable(why) => panic!("unkeyable: {why}"),
                }
                checked += 1;
            }
        }
    }
    assert_eq!(checked, 8 * 64 * 64);
}

/// Pins the kind names three places spell: this enum, the registration list
/// in shell, and `.gitattributes`. A name only one of them knows merges as
/// plain text without a word.
#[test]
fn the_registration_and_gitattributes_name_only_these_kinds() {
    let kinds: Vec<String> = Kind::value_variants()
        .iter()
        .map(|k| k.to_possible_value().expect("named").get_name().to_string())
        .collect();
    let lib = include_str!("../../../scripts/lib/merge_drivers.sh");
    let registered: Vec<&str> = lib
        .lines()
        .find_map(|l| l.strip_prefix("MERGE_DRIVER_KINDS=("))
        .expect("scripts/lib/merge_drivers.sh defines MERGE_DRIVER_KINDS")
        .trim_end_matches(')')
        .split_whitespace()
        .collect();
    assert_eq!(registered, kinds);
    let attributes = include_str!("../../../.gitattributes");
    for named in attributes
        .split_whitespace()
        .filter_map(|w| w.strip_prefix("merge=lucidos-"))
    {
        assert!(kinds.iter().any(|k| k == named), "no driver kind {named}");
    }
}
