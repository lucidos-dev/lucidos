//! Builders for the WORKSPACE-authored half of the chat prompt: the
//! `[CURRENT FILES]` listing, the Available Apps list, and the Know-how
//! routing list.
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
//! 2. **Never narrow what the agent can reach.** A listing here is a sample
//!    that saves a tool call, so it may elide. The tools it saves
//!    (`list_files`, `glob_files`, `grep_files`) still see everything.

use crate::core::knowhow::KnowhowSummary;
use crate::core::App;
use std::collections::BTreeMap;

/// Directories listed in `[CURRENT FILES]` before the block stops naming new
/// ones. Breadth beats depth here: the block exists so the agent can see the
/// SHAPE of the workspace without calling `list_files`.
const MAX_DIRS: usize = 40;

/// File names listed per directory before its elision line. Enough to show
/// what KIND of directory it is, which is all a routing signal needs.
const MAX_FILES_PER_DIR: usize = 8;

/// Ceiling on file names across the whole block, so a workspace with hundreds
/// of two-file directories cannot walk past the budget one directory at a time.
const MAX_FILES_TOTAL: usize = 120;

/// Ceiling on a workspace knowhow `description` as rendered into the Know-how
/// routing list. A description is a ROUTING signal that gets matched
/// semantically, and the doc body one `load_knowhow` away carries the detail,
/// so length buys nothing here and is billed on every turn of every thread.
///
/// The engine's OWN system-knowhow descriptions are held to 700 chars by
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
/// `ArtifactManager::list_artifacts` returns). Empty string when the workspace
/// has no listable file, so the caller appends nothing.
///
/// Vendored and build output is dropped BEFORE anything is taken, via
/// [`crate::core::is_vendored_path`]. Without that, a workspace with a
/// `node_modules` tree spends the whole block on it: on the workspace this was
/// measured against, 91 of the 100 listed paths were vendored and none of the
/// user's own files appeared at all.
///
/// What survives is listed by directory, breadth first, rather than as a flat
/// alphabetical `take(N)`. The naive take spends the entire cap on whatever
/// sorts first, so one deep tree can own the block even after filtering, and it
/// repeats a long directory prefix once per file. One line per directory, a
/// per-directory file cap, and directories ordered by depth then name give
/// every part of the workspace a chance to appear and pay for each prefix once.
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
    let mut listed_files = 0usize;
    let mut listed_dirs = 0usize;
    for (dir, names) in &dirs {
        if listed_dirs == MAX_DIRS || listed_files >= MAX_FILES_TOTAL {
            break;
        }
        let room = MAX_FILES_PER_DIR.min(MAX_FILES_TOTAL - listed_files);
        section.push_str(&format!("\n  {}/", dir));
        for name in names.iter().take(room) {
            section.push_str(&format!("\n    {}", name));
        }
        listed_files += names.len().min(room);
        listed_dirs += 1;
        if names.len() > room {
            let elided = names.len() - room;
            section.push_str(&format!(
                "\n    ... and {} more {} here",
                elided,
                noun(elided, "file", "files")
            ));
        }
    }

    // The partial-listing suffix. Vendored files are reported as their own
    // number rather than folded into the remainder: the agent must not read
    // "and 632 more" as the whole story and conclude a dependency it can see on
    // disk is missing.
    let remaining_files = kept.len() - listed_files;
    let remaining_dirs = dirs.len() - listed_dirs;
    if remaining_files > 0 {
        section.push_str(&format!(
            "\n  ... and {} more {}",
            remaining_files,
            noun(remaining_files, "file", "files")
        ));
        if remaining_dirs > 0 {
            section.push_str(&format!(
                " ({} {} not listed)",
                remaining_dirs,
                noun(remaining_dirs, "directory", "directories")
            ));
        }
    }
    if vendored > 0 {
        section.push_str(&format!(
            "\n  plus {} {} under vendored or build directories, not listed",
            vendored,
            noun(vendored, "file", "files")
        ));
    }
    if remaining_files > 0 || vendored > 0 {
        section.push_str(
            "\n  list_files returns the whole tree unfiltered including those, \
             so prefer glob_files to find a specific file",
        );
    }
    section.push_str("\n[END FILES]");
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
            "- **{}** (id: `{}/{}`, app: {}): {}\n",
            kh.name,
            app_id,
            kh.id,
            app_id,
            routing_description(&kh.description)
        ));
    }
    section
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

        let apps = crate::core::AppManager::new(root)
            .expect("app manager")
            .list_apps()
            .expect("list apps");
        assert_eq!(apps.len(), 20, "the fixture's apps must all load");
        let apps_section = build_apps_section(&apps);

        let knowhow = crate::core::KnowhowStore::load_summaries(&root.join("data/knowhow"));
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

    /// The failure a flat alphabetical take has even after filtering: one deep
    /// directory eats the cap. Every small directory must still be represented.
    #[test]
    fn one_big_directory_cannot_crowd_out_the_others() {
        let mut paths: Vec<String> = (0..500)
            .map(|i| format!("artifacts/aaa-huge/file{:03}.txt", i))
            .collect();
        for i in 0..10 {
            paths.push(format!("artifacts/small{:02}/note.md", i));
        }
        paths.sort();

        let section = build_file_list_section(&paths);

        for i in 0..10 {
            assert!(
                section.contains(&format!("artifacts/small{:02}/", i)),
                "small directory {i} missing:\n{section}"
            );
        }
        assert!(section.contains("... and 492 more files here"));
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

    /// The partial-listing suffix survives the reshape, and its two numbers are
    /// separate: elided-but-listable, and vendored.
    #[test]
    fn partial_listings_say_how_much_is_missing() {
        let mut paths: Vec<String> = Vec::new();
        for d in 0..60 {
            for f in 0..3 {
                paths.push(format!("artifacts/dir{:02}/f{}.md", d, f));
            }
        }
        paths.sort();
        let section = build_file_list_section(&paths);
        // 40 directories x 3 files listed, 20 directories left over.
        assert!(
            section.contains("... and 60 more files (20 directories not listed)"),
            "expected the remainder suffix:\n{section}"
        );
        assert!(!section.contains("vendored"));
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
    }
}
