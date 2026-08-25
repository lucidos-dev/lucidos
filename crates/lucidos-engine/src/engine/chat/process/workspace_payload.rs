//! Builders for the WORKSPACE-authored half of the chat prompt: the
//! `[CURRENT FILES]` listing, the Available Apps list, the Know-how routing
//! list, and the open app's know-how listing inside `[ACTIVE APP UI]`.
//!
//! Split out of `context_sections.rs` and `system_prompt.rs` because this half
//! is sized by what the USER put in their workspace, not by what the engine
//! wrote, so it needs its own shaping rules and its own ceiling. Every builder
//! here is pure over already-loaded data, so a test can assemble the payload
//! with no engine and no database.
//!
//! Two rules bind everything in this file:
//!
//! 1. **Shape the payload, never the data.** Truncation and filtering happen
//!    here, at the point the prompt block is built. The stored file, the API
//!    response and the UI all still carry the user's full text.
//! 2. **Never narrow what the agent can reach.** The file listing is an
//!    inventory or nothing, never a sample (ADR 0086 as amended). The tools it
//!    saves (`list_files`, `glob_files`, `grep_files`) still see everything.

use crate::core::knowhow::KnowhowSummary;
use crate::core::App;
use std::collections::BTreeMap;

/// Ceiling on the rendered `[CURRENT FILES]` block. Past it the block is not
/// sent at all: the agent gets an inventory or nothing, never a sample
/// (ADR 0086 as amended).
///
/// Derived twice, landing within 2% each way. A complete listing supplied the
/// path on 50% of file-touching turns at 54 to 82 files. At 47 chars per file,
/// the densest rendering observed, that range needs 3,854.
///
/// It must also pay for itself. At 2.538 chars per token a block of C chars
/// costs about `C x 5.8e-6` per turn. The round trip it saves is worth $0.0976,
/// so break-even at a 23.3% supply rate is 3,906. Move this number on that
/// arithmetic, never back to a directory count. The full derivation is in
/// `docs/plans/2026-08-18-file-listing-is-an-inventory-or-nothing.md`.
///
/// Bytes rather than chars, because a multi-byte path costs more tokens per
/// char, so charging it more of the ceiling is the right direction.
const FILE_LIST_MAX_BYTES: usize = 4_000;

/// Ceiling on a workspace knowhow `description` as rendered into the Know-how
/// routing list. A description is a ROUTING signal that gets matched
/// semantically, and the doc body one `load_knowhow` away carries the detail,
/// so length buys nothing here and is billed on every turn of every thread.
///
/// The engine's OWN system-knowhow descriptions are held to 400 chars by
/// `system_knowhow_descriptions_stay_routing_sized`, a hard-failing test. That
/// instrument does not work on text the user owns, so the engine truncates at
/// render time instead. Same intent, enforced where it can be.
pub(crate) const KNOWHOW_DESCRIPTION_MAX_CHARS: usize = 400;

/// Ceiling on an app `description` as rendered into the Available Apps list.
/// Half the knowhow ceiling because this line answers a narrower question:
/// which app the user means, and whether `navigate_ui` should open it.
const APP_DESCRIPTION_MAX_CHARS: usize = 200;

/// Pick the noun for a count, so an elision line never reads "1 more files".
fn noun(n: usize, singular: &'static str, plural: &'static str) -> &'static str {
    if n == 1 {
        singular
    } else {
        plural
    }
}

/// Cut `text` to at most `max_chars` characters on a word boundary and mark
/// the cut with an ellipsis.
///
/// The boundary is only honoured when it lands in the last quarter of the
/// budget, so a single unbroken run (a URL, a base64 blob) is still cut rather
/// than collapsing the whole line. Char-based throughout: byte slicing would
/// panic on the multi-byte text these descriptions routinely carry.
fn truncate_on_word_boundary(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let head: String = text.chars().take(max_chars).collect();
    let cut = match head.rfind(char::is_whitespace) {
        Some(idx) if head[..idx].chars().count() >= max_chars * 3 / 4 => idx,
        _ => head.len(),
    };
    format!("{}…", head[..cut].trim_end())
}

/// Render a knowhow `description` for a routing list. Shared by the workspace
/// Know-how section here and the per-trigger listing in `process_helpers`, so
/// the two cannot drift apart on what a routing line costs.
pub(crate) fn routing_description(description: &str) -> String {
    truncate_on_word_boundary(description, KNOWHOW_DESCRIPTION_MAX_CHARS)
}

/// Build the `[CURRENT FILES]` block from a data-relative file list (what
/// `ArtifactManager::list_artifacts` returns).
///
/// **An inventory or nothing.** The block is returned only when it names every
/// non-vendored file and fits [`FILE_LIST_MAX_BYTES`]. Otherwise the empty
/// string, so the caller appends nothing. A sample was measured resolving 2.4%
/// of first touches where a complete listing resolved 50.0%, which is what
/// ADR 0086's amendment records.
///
/// Vendored and build output is dropped BEFORE anything is measured, via
/// [`crate::core::is_vendored_path`]. Without that, a workspace with a
/// `node_modules` tree spends the whole block on it: on the workspace this was
/// measured against, 91 of the 100 listed paths were vendored and none of the
/// user's own files appeared at all.
///
/// What survives is listed by directory, breadth first. A long directory prefix
/// is then paid for once rather than once per file, and the user's own
/// top-level artifacts come first.
pub(crate) fn build_file_list_section(files: &[String]) -> String {
    let kept: Vec<&str> = files
        .iter()
        .map(String::as_str)
        .filter(|path| !crate::core::is_vendored_path(path))
        .collect();
    let vendored = files.len() - kept.len();
    if kept.is_empty() {
        return String::new();
    }

    // BTreeMap sorts the directory keys; the names within a directory keep the
    // caller's order, which `list_artifacts` already returns sorted.
    let mut by_dir: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for path in &kept {
        let (dir, name) = path.rsplit_once('/').unwrap_or(("", path));
        by_dir.entry(dir).or_default().push(name);
    }
    let mut dirs: Vec<(&str, Vec<&str>)> = by_dir.into_iter().collect();
    dirs.sort_by(|(a, _), (b, _)| (a.matches('/').count(), *a).cmp(&(b.matches('/').count(), *b)));

    let mut section = String::from("[CURRENT FILES]");
    for (dir, names) in &dirs {
        section.push_str(&format!("\n  {}/", dir));
        for name in names {
            section.push_str(&format!("\n    {}", name));
            // Nothing appended later shrinks the block, so a workspace already
            // past the ceiling has no inventory to send. Per file rather than
            // per directory: one directory holding 40,000 exports would
            // otherwise build a megabyte of string before throwing it away.
            if section.len() > FILE_LIST_MAX_BYTES {
                return String::new();
            }
        }
    }

    // Vendored files are counted rather than passed over in silence. A complete
    // inventory reads as exhaustive, so without this line the agent would take
    // a dependency it can see on disk to be missing.
    if vendored > 0 {
        section.push_str(&format!(
            "\n  plus {} {} under vendored or build directories, not listed",
            vendored,
            noun(vendored, "file", "files")
        ));
        section.push_str(
            "\n  list_files returns the whole tree unfiltered including those, \
             so prefer glob_files to find a specific file",
        );
    }
    section.push_str("\n[END FILES]");

    // The trailer is billed too, so the ceiling covers what is actually sent.
    if section.len() > FILE_LIST_MAX_BYTES {
        return String::new();
    }
    section
}

/// Build the `## Available Apps` section. Empty string when the workspace has
/// no app.
pub(crate) fn build_apps_section(apps: &[App]) -> String {
    if apps.is_empty() {
        return String::new();
    }
    let mut section = String::from("\n\n## Available Apps\n\n");
    section.push_str(
        "Apps are interactive UIs. Use navigate_ui to open them. Some apps have \
         intents: use execute_intent(intent_id) to fulfill a stored intent.\n\n",
    );
    for app in apps {
        section.push_str(&format!(
            "- **{}** (id: `{}`): {}\n",
            app.name,
            app.id,
            truncate_on_word_boundary(&app.description, APP_DESCRIPTION_MAX_CHARS)
        ));
    }
    section
}

/// Build the `## Know-how` section from the workspace's own knowhow docs plus
/// the app-scoped ones. Empty string when the workspace has neither.
pub(crate) fn build_knowhow_section(
    summaries: &[KnowhowSummary],
    app_summaries: &[(String, KnowhowSummary)],
) -> String {
    if summaries.is_empty() && app_summaries.is_empty() {
        return String::new();
    }
    let mut section = String::from(
        "\n\n## Know-how\n\n\
        Know-how files contain domain knowledge, procedures, and reference material. \
        When a user's request relates to a topic below, use `load_knowhow` to load the full content before responding.\n\n",
    );
    for kh in summaries {
        section.push_str(&format!(
            "- **{}** (id: `{}`): {}\n",
            kh.name,
            kh.id,
            routing_description(&kh.description)
        ));
    }
    for (app_id, kh) in app_summaries {
        section.push_str(&format!(
            "- **{}** (id: `{}`, app: {}): {}\n",
            kh.name,
            app_scoped_id(app_id, &kh.id),
            app_id,
            routing_description(&kh.description)
        ));
    }
    section
}

/// The id an app-scoped knowhow doc answers to: `<app_id>/<rest>`, which
/// [`crate::core::KnowhowStore::load_with_fallback`] resolves to
/// `data/apps/<app_id>/knowhow/<rest>.md`.
///
/// Shared by the two surfaces that name these docs: the Know-how routing list
/// above, and [`build_app_knowhow_listing`] below. An id that differs between
/// them is an id the agent cannot load.
fn app_scoped_id(app_id: &str, knowhow_id: &str) -> String {
    format!("{}/{}", app_id, knowhow_id)
}

/// Build the know-how listing for the app the user has OPEN, for the
/// `[ACTIVE APP UI]` block. Empty string when the app has no knowhow docs.
///
/// A POINTER, never a body. The block is rebuilt on every round of the
/// agentic loop, so its cost is paid hundreds of times per thread. It is
/// billed whether or not the turn is about the app. It used to carry every
/// doc's full text.
///
/// On the workspace that motivated this, one app's single doc rendered the
/// block at 136,065 chars: about 47% of a 200k-token model's whole budget.
/// That body bought nothing new, because the doc is one `load_knowhow` call
/// away under an id the routing list already carries. See
/// `docs/adr/0111-app-know-how-is-a-pointer-not-a-body.md`.
///
/// The consequence for anyone editing this: the rendered size must stay a
/// function of how MANY docs the app has, never of how big they are.
pub(crate) fn build_app_knowhow_listing(app_id: &str, summaries: &[KnowhowSummary]) -> String {
    if summaries.is_empty() {
        return String::new();
    }
    let mut listing = String::from(
        "This app's know-how is NOT loaded. Call load_knowhow with an id below \
         when the turn needs it.\n",
    );
    for kh in summaries {
        listing.push_str(&format!(
            "- **{}** (id: `{}`): {}\n",
            kh.name,
            app_scoped_id(app_id, &kh.id),
            routing_description(&kh.description)
        ));
    }
    listing
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;

    fn files(paths: &[&str]) -> Vec<String> {
        paths.iter().map(|p| p.to_string()).collect()
    }

    /// Ceiling on the WORKSPACE-authored text a chat turn pays for, measured
    /// against [`busy_workspace`]. The engine-authored side has its own
    /// ratchet, `always_loaded_context_stays_under_budget`, which deliberately
    /// excludes everything measured here: neither number moves when the other
    /// side changes, which is the only way a breach names the right owner.
    ///
    /// A RATCHET, not a target. It is a fixture measurement, so it tracks no
    /// real workspace; what it pins is the SHAPING, since the same fixture cost
    /// roughly double this before the vendored filter and the two description
    /// ceilings landed. Raising it means a builder in this file got more
    /// generous, which is a change that has to say why it is worth paying on
    /// every turn of every thread.
    const WORKSPACE_PAYLOAD_BUDGET_CHARS: usize = 12_800;

    /// A workspace shaped like the one that motivated this: a big vendored
    /// tree, more knowhow docs than anyone reads in a turn, every description
    /// far past its ceiling, and a real but modest set of the user's own files.
    ///
    /// Returns the tempdir (kept alive by the caller) so the loaders below run
    /// against real files rather than hand-built structs. The point is that the
    /// walk, the frontmatter parse and the manifest parse are the production
    /// ones; only the content is synthetic.
    fn busy_workspace() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        let data = dir.path().join("data");
        let write = |rel: &str, body: &str| {
            let path: &Path = &data.join(rel);
            fs::create_dir_all(path.parent().expect("has parent")).expect("mkdir");
            fs::write(path, body).expect("write");
        };

        // The vendored tree: what 91 of the old block's 100 lines were.
        for i in 0..300 {
            write(
                &format!("apps/demo-director/remotion/node_modules/pkg{i:03}/index.js"),
                "module.exports = {};",
            );
        }

        // Twenty apps, each with an over-long description.
        let app_description = "Tracks and visualises everything the user cares \
            about in this domain, with a dashboard, filters, and export. "
            .repeat(4);
        for i in 0..20 {
            write(
                &format!("apps/app{i:02}/manifest.json"),
                &format!(
                    r#"{{"name": "App {i:02}", "description": {}}}"#,
                    serde_json::to_string(&app_description).expect("json string")
                ),
            );
            write(&format!("apps/app{i:02}/index.html"), "<h1>app</h1>");
        }

        // A dozen knowhow docs whose descriptions run to 800 chars.
        let knowhow_description = "Covers the API, its quirks, the auth \
            handshake, the retry policy, and the failure modes worth knowing. "
            .repeat(8);
        assert!(
            knowhow_description.chars().count() > 2 * KNOWHOW_DESCRIPTION_MAX_CHARS,
            "the fixture must be well past the ceiling or it proves nothing"
        );
        for i in 0..12 {
            write(
                &format!("knowhow/topic{i:02}.md"),
                &format!(
                    "---\nname: Topic {i:02}\ndescription: {}\n---\n# Topic\n\nBody.\n",
                    knowhow_description.replace('\n', " ")
                ),
            );
        }
        // Plus an app-scoped one, which renders through the same list.
        write(
            "apps/app00/knowhow/flow.md",
            &format!(
                "---\nname: Flow\ndescription: {}\n---\n# Flow\n\nBody.\n",
                knowhow_description.replace('\n', " ")
            ),
        );

        // The user's own files, spread the way a real workspace spreads them.
        for i in 0..12 {
            write(&format!("artifacts/report{i:02}.md"), "# Report");
            write(&format!("artifacts/screenshots/shot{i:02}.png"), "png");
            write(&format!("artifacts/deep/nested/tree/note{i:02}.md"), "note");
        }
        write("triggers/nightly/run.intent", "name: nightly");

        dir
    }

    /// The gate. Assembles every workspace-authored block the way a chat turn
    /// does and holds the total under a declared ceiling, printing the
    /// per-area breakdown on failure and under `--nocapture` so re-measuring
    /// after a deliberate change costs one command.
    ///
    /// The vendored tree is the thing this proves is excluded, because that is
    /// the bug that shipped.
    #[test]
    fn busy_workspace_payload_stays_under_budget() {
        let ws = busy_workspace();
        let root = ws.path();

        let all_files: Vec<String> = crate::core::list_searchable_data_files(root)
            .expect("walk the fixture")
            .into_iter()
            .map(|(rel, _)| rel)
            .collect();
        let vendored = all_files
            .iter()
            .filter(|p| p.contains("node_modules"))
            .count();
        assert!(
            vendored >= 300,
            "the fixture lost its vendored tree, so exclusion proves nothing"
        );

        let file_list = build_file_list_section(&all_files);
        assert!(
            !file_list.contains("node_modules"),
            "vendored files reached the prompt:\n{file_list}"
        );
        assert!(
            file_list.contains("report00.md"),
            "the user's own files must be what the block shows:\n{file_list}"
        );
        assert!(
            file_list.contains("report11.md"),
            "an emitted block names every file, not the first eight:\n{file_list}"
        );

        let apps = crate::core::AppManager::new(root)
            .expect("app manager")
            .list_apps()
            .expect("list apps");
        assert_eq!(apps.len(), 20, "the fixture's apps must all load");
        let apps_section = build_apps_section(&apps);

        let knowhow = crate::core::KnowhowStore::load_summaries(
            &root.join("data/knowhow"),
            crate::core::KnowhowListDepth::FilesAndGroups,
        );
        let app_knowhow = crate::core::KnowhowStore::load_app_summaries(&root.join("data/apps"));
        assert_eq!(knowhow.len(), 12);
        assert_eq!(app_knowhow.len(), 1);
        let knowhow_section = build_knowhow_section(&knowhow, &app_knowhow);

        let areas = [
            ("[CURRENT FILES] listing", file_list.chars().count()),
            ("Available Apps list", apps_section.chars().count()),
            ("Know-how routing list", knowhow_section.chars().count()),
        ];
        let total: usize = areas.iter().map(|(_, n)| n).sum();
        let breakdown = areas
            .iter()
            .map(|(name, n)| format!("  {n:>7} chars  {name}"))
            .collect::<Vec<_>>()
            .join("\n");
        println!("workspace payload:\n{breakdown}\n  {total:>7} chars  TOTAL");

        assert!(
            total <= WORKSPACE_PAYLOAD_BUDGET_CHARS,
            "workspace payload is {total} chars, over the \
             {WORKSPACE_PAYLOAD_BUDGET_CHARS} ceiling. Every character is billed \
             on every turn of every thread in this workspace, so tighten a \
             builder or raise the ceiling in a change that says why:\n{breakdown}"
        );
    }

    /// The bug that shipped: a vendored tree owns the whole block and the
    /// user's own files never appear. Filtering happens BEFORE anything is
    /// taken, so the 250:1 ratio here cannot crowd them out.
    #[test]
    fn vendored_tree_is_excluded_and_real_files_survive() {
        let mut paths: Vec<String> = (0..500)
            .map(|i| format!("apps/demo/remotion/node_modules/pkg{:03}/index.js", i))
            .collect();
        paths.push("artifacts/report.md".to_string());
        paths.push("knowhow/github.md".to_string());
        paths.sort();

        let section = build_file_list_section(&paths);

        assert!(
            !section.contains("node_modules"),
            "vendored paths must not be listed:\n{section}"
        );
        assert!(section.contains("report.md"));
        assert!(section.contains("github.md"));
        assert!(
            section.contains("500 files under vendored or build directories, not listed"),
            "the count must be reported rather than silently dropped:\n{section}"
        );
        assert!(
            section.contains("prefer glob_files"),
            "the agent must be told list_files is unfiltered:\n{section}"
        );
        assert!(
            !section.contains("... and"),
            "a vendored tree elides nothing the user owns:\n{section}"
        );
    }

    /// Every segment is tested, not just the first, and a file the user named
    /// `build` or `out` is still theirs.
    #[test]
    fn vendored_matching_is_per_segment_and_ignores_the_file_name() {
        let section = build_file_list_section(&files(&[
            "apps/site/dist/bundle.js",
            "artifacts/deep/nest/target/debug/x.rlib",
            "artifacts/notes/build",
            "artifacts/notes/out",
        ]));
        assert!(!section.contains("bundle.js"));
        assert!(!section.contains("x.rlib"));
        assert!(section.contains("build"));
        assert!(section.contains("out"));
    }

    /// The rule ADR 0086's amendment records. One directory too big to name in
    /// full costs the whole block, including the small directories beside it
    /// that would have fitted. A sample the agent cannot trust is worth less
    /// than nothing.
    #[test]
    fn a_workspace_too_big_to_list_whole_gets_no_block() {
        let mut paths: Vec<String> = (0..500)
            .map(|i| format!("artifacts/aaa-huge/file{:03}.txt", i))
            .collect();
        for i in 0..10 {
            paths.push(format!("artifacts/small{:02}/note.md", i));
        }
        paths.sort();

        let section = build_file_list_section(&paths);

        assert!(section.is_empty(), "expected nothing at all:\n{section}");
    }

    /// The predicate is what the block COSTS, not how many files it names. The
    /// same 60 files are refused as deep paths and listed whole as short ones.
    #[test]
    fn the_ceiling_is_the_rendered_size_not_a_file_count() {
        let deep = "artifacts/projects/quarterly-planning/attachments/generated";
        let long: Vec<String> = (0..60)
            .map(|i| format!("{deep}/section-{i:02}/a-rather-long-document-name-{i:02}.md"))
            .collect();
        let short: Vec<String> = (0..60).map(|i| format!("artifacts/n{i:02}.md")).collect();

        assert!(
            build_file_list_section(&long).is_empty(),
            "60 long paths render past the ceiling and must be refused"
        );
        assert!(
            !build_file_list_section(&short).is_empty(),
            "60 short paths fit and must be listed"
        );
    }

    /// Shallow before deep, so the user's own top-level artifacts are what the
    /// agent sees first.
    #[test]
    fn directories_are_listed_breadth_first() {
        let section = build_file_list_section(&files(&[
            "artifacts/a/b/c/deep.md",
            "artifacts/shallow.md",
            "knowhow/topic.md",
        ]));
        let shallow = section
            .find("  artifacts/\n")
            .expect("root artifacts dir listed");
        let deep = section.find("artifacts/a/b/c/").expect("deep dir listed");
        assert!(shallow < deep, "shallow directories come first:\n{section}");
    }

    /// A directory count decides nothing. 60 directories holding 180 short
    /// paths render well under the ceiling, so the workspace is listed whole.
    #[test]
    fn many_directories_do_not_refuse_a_block_that_fits() {
        let mut paths: Vec<String> = Vec::new();
        for d in 0..60 {
            for f in 0..3 {
                paths.push(format!("artifacts/dir{:02}/f{}.md", d, f));
            }
        }
        paths.sort();

        let section = build_file_list_section(&paths);

        assert!(
            section.contains("artifacts/dir59/"),
            "the last directory must be named:\n{section}"
        );
        let named = section.lines().filter(|l| l.starts_with("    ")).count();
        assert_eq!(named, 180, "every file must be named:\n{section}");
        assert!(!section.contains("... and"));
    }

    /// There is no partial listing: no header, no remainder line, no elision
    /// marker, nothing. Same shape as the test above, three times the files.
    #[test]
    fn nothing_is_emitted_rather_than_a_partial_listing() {
        let mut paths: Vec<String> = Vec::new();
        for d in 0..100 {
            for f in 0..3 {
                paths.push(format!("artifacts/dir{:03}/f{}.md", d, f));
            }
        }
        paths.sort();

        assert_eq!(build_file_list_section(&paths), "");
    }

    /// Both halves of the rule, swept across the ceiling. An emitted block
    /// names every kept file and fits the ceiling; the only alternative is
    /// nothing. The sweep must cross the boundary or it proves neither half.
    #[test]
    fn every_emitted_block_is_complete_and_within_the_ceiling() {
        let mut paths: Vec<String> = Vec::new();
        let (mut emitted, mut refused) = (0usize, 0usize);

        for i in 0..400 {
            paths.push(format!("artifacts/topic{:02}/note-{:03}.md", i % 25, i));
            paths.sort();

            let section = build_file_list_section(&paths);
            if section.is_empty() {
                refused += 1;
                continue;
            }
            emitted += 1;

            assert!(
                section.len() <= FILE_LIST_MAX_BYTES,
                "{} files rendered {} bytes, past the ceiling",
                paths.len(),
                section.len()
            );
            assert!(
                !section.contains("... and"),
                "an emitted block carries no elision marker:\n{section}"
            );
            for path in &paths {
                let name = path.rsplit_once('/').expect("has a directory").1;
                assert!(
                    section.contains(name),
                    "{name} missing from what must be an inventory:\n{section}"
                );
            }
        }

        assert!(emitted > 0, "the sweep never emitted a block");
        assert!(refused > 0, "the sweep never crossed the ceiling");
    }

    /// A workspace whose only files are vendored produces no block at all,
    /// matching the pre-existing "no files, no section" behaviour.
    #[test]
    fn a_workspace_with_nothing_listable_produces_no_block() {
        assert!(build_file_list_section(&[]).is_empty());
        assert!(build_file_list_section(&files(&["apps/x/node_modules/a.js"])).is_empty());
    }

    #[test]
    fn a_small_workspace_is_listed_whole_with_no_suffix() {
        let section = build_file_list_section(&files(&[
            "artifacts/a.md",
            "artifacts/b.md",
            "knowhow/c.md",
        ]));
        assert_eq!(
            section,
            "[CURRENT FILES]\n  artifacts/\n    a.md\n    b.md\n  knowhow/\n    c.md\n[END FILES]"
        );
    }

    #[test]
    fn descriptions_are_cut_on_a_word_boundary_with_an_ellipsis() {
        let text = "alpha beta gamma delta epsilon zeta eta theta";
        let cut = truncate_on_word_boundary(text, 20);
        assert!(cut.ends_with('…'), "{cut}");
        assert!(!cut.contains("epsi"), "cut mid-word: {cut}");
        assert!(cut.chars().count() <= 21);
    }

    #[test]
    fn a_short_description_is_left_exactly_as_written() {
        let text = "Controls the heat pump over the vendor's cloud API.";
        assert_eq!(truncate_on_word_boundary(text, 400), text);
    }

    /// An unbroken run has no boundary to honour, and must still be cut rather
    /// than collapsing the line to a lone ellipsis.
    #[test]
    fn an_unbroken_run_is_cut_at_the_budget() {
        let text = "y".repeat(500);
        let cut = truncate_on_word_boundary(&text, 100);
        assert_eq!(cut.chars().count(), 101);
    }

    /// `.claude/rules/rust.md`: never slice by byte index. These descriptions
    /// are routinely Norwegian or Polish.
    #[test]
    fn truncation_never_splits_a_multibyte_character() {
        let text = "æøå ñ ünïcödé ".repeat(60);
        let cut = truncate_on_word_boundary(&text, 50);
        assert!(cut.chars().count() <= 51);
        assert!(!cut.contains('\u{fffd}'));
    }

    /// The ceilings are RENDER-time. The loaded summary keeps the user's full
    /// text, which is what every other surface serves.
    #[test]
    fn oversized_knowhow_and_app_descriptions_are_capped_at_render_time() {
        let long = "word ".repeat(400);
        let summaries = vec![KnowhowSummary {
            id: "ops/nightly".to_string(),
            name: "Nightly".to_string(),
            description: long.clone(),
        }];
        let app_summaries = vec![(
            "demo".to_string(),
            KnowhowSummary {
                id: "flow".to_string(),
                name: "Flow".to_string(),
                description: long.clone(),
            },
        )];
        let knowhow = build_knowhow_section(&summaries, &app_summaries);
        let bullets: Vec<&str> = knowhow.lines().filter(|l| l.starts_with("- **")).collect();
        assert_eq!(bullets.len(), 2);
        for line in bullets {
            // Budget plus the bullet's own name/id scaffolding.
            assert!(
                line.chars().count() <= KNOWHOW_DESCRIPTION_MAX_CHARS + 80,
                "knowhow bullet not capped: {} chars",
                line.chars().count()
            );
            assert!(line.ends_with('…'), "no ellipsis marker: {line}");
        }

        let apps = vec![App {
            id: "demo".to_string(),
            name: "Demo".to_string(),
            description: long.clone(),
            icon: None,
        }];
        let rendered = build_apps_section(&apps);
        let bullet = rendered
            .lines()
            .find(|l| l.starts_with("- **"))
            .expect("app bullet");
        assert!(
            bullet.chars().count() <= APP_DESCRIPTION_MAX_CHARS + 40,
            "app bullet not capped: {} chars",
            bullet.chars().count()
        );

        assert_eq!(summaries[0].description, long);
        assert_eq!(apps[0].description, long);
    }

    #[test]
    fn a_short_app_description_is_rendered_verbatim() {
        let apps = vec![App {
            id: "documents".to_string(),
            name: "Documents".to_string(),
            description: "Browse and search saved documents.".to_string(),
            icon: None,
        }];
        assert!(build_apps_section(&apps)
            .contains("- **Documents** (id: `documents`): Browse and search saved documents.\n"));
    }

    #[test]
    fn empty_inputs_produce_no_sections() {
        assert!(build_apps_section(&[]).is_empty());
        assert!(build_knowhow_section(&[], &[]).is_empty());
        assert!(build_app_knowhow_listing("demo", &[]).is_empty());
    }

    fn summary(id: &str, name: &str, description: &str) -> KnowhowSummary {
        KnowhowSummary {
            id: id.to_string(),
            name: name.to_string(),
            description: description.to_string(),
        }
    }

    /// One line per doc, and a sentence saying what to do with the ids.
    #[test]
    fn the_app_listing_names_each_doc_and_says_how_to_load_it() {
        let listing = build_app_knowhow_listing(
            "habit-tracker",
            &[
                summary(
                    "reach-metrics",
                    "Reach metrics",
                    "Where the numbers come from.",
                ),
                summary("nested/deep", "Deep dive", "The long version."),
            ],
        );

        assert_eq!(
            listing,
            "This app's know-how is NOT loaded. Call load_knowhow with an id below \
             when the turn needs it.\n\
             - **Reach metrics** (id: `habit-tracker/reach-metrics`): Where the numbers come from.\n\
             - **Deep dive** (id: `habit-tracker/nested/deep`): The long version.\n"
        );
    }

    /// The same render-time ceiling the routing list uses. A user who writes
    /// an essay into `description:` cannot reintroduce the cost the bodies
    /// used to carry.
    #[test]
    fn an_app_listing_line_is_capped_like_a_routing_line() {
        let long = "word ".repeat(400);
        let listing = build_app_knowhow_listing("demo", &[summary("flow", "Flow", &long)]);

        let line = listing
            .lines()
            .find(|l| l.starts_with("- **"))
            .expect("a bullet");
        assert!(
            line.chars().count() <= KNOWHOW_DESCRIPTION_MAX_CHARS + 80,
            "listing line not capped: {} chars",
            line.chars().count()
        );
        assert!(line.ends_with('…'), "no ellipsis marker: {line}");
    }

    /// The id text is the load-bearing part, so it is built in one place. A
    /// divergence here is a doc the agent is told about and cannot load.
    #[test]
    fn the_two_surfaces_print_the_same_app_scoped_id() {
        let kh = summary("nested/deep", "Deep dive", "The long version.");
        let listing = build_app_knowhow_listing("habit-tracker", std::slice::from_ref(&kh));
        let routing = build_knowhow_section(&[], &[("habit-tracker".to_string(), kh)]);

        let id = "(id: `habit-tracker/nested/deep`";
        assert!(listing.contains(id), "{listing}");
        assert!(routing.contains(id), "{routing}");
    }
}
