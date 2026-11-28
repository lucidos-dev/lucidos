use crate::core::knowhow::KnowhowSummary;
use crate::core::{App, AppManager};
#[cfg(test)]
use crate::engine::widget_guidance::APP_DESCRIPTION_MAX_CHARS;
use crate::engine::widget_guidance::{app_description, truncate_on_word_boundary};
use std::collections::BTreeMap;

const FILE_LIST_MAX_BYTES: usize = 4_000;

pub(crate) const KNOWHOW_DESCRIPTION_MAX_CHARS: usize = 400;

fn noun(n: usize, singular: &'static str, plural: &'static str) -> &'static str {
    if n == 1 {
        singular
    } else {
        plural
    }
}

pub(crate) fn routing_description(description: &str) -> String {
    truncate_on_word_boundary(description, KNOWHOW_DESCRIPTION_MAX_CHARS)
}

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
            if section.len() > FILE_LIST_MAX_BYTES {
                return String::new();
            }
        }
    }

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

    if section.len() > FILE_LIST_MAX_BYTES {
        return String::new();
    }
    section
}

pub(crate) fn apps_section(
    app_manager: &AppManager,
    automatic_widgets: bool,
) -> Result<String, std::io::Error> {
    let (apps, reusable) = app_manager.apps_and_reusable_widgets()?;
    Ok(format!(
        "{}{}",
        build_apps_section(&apps),
        build_widgets_section(&reusable, automatic_widgets)
    ))
}

pub(crate) fn build_widgets_section(reusable: &[App], automatic: bool) -> String {
    crate::engine::widget_guidance::build_widgets_section(
        reusable,
        crate::engine::widget_guidance::WidgetAudience::Chat { automatic },
    )
}

pub(crate) fn app_line(app: &App) -> String {
    format!(
        "- **{}** (id: `{}`): {}\n",
        app.name,
        app.id,
        app_description(&app.description)
    )
}

pub(crate) fn build_apps_section(apps: &[App]) -> String {
    if apps.is_empty() {
        return String::new();
    }
    let mut section = String::from("\n\n## Available Apps\n\n");
    section.push_str(
        "Apps are interactive UIs. Use navigate_ui to open them. Some apps have \
         intents: to run one, load_knowhow its id and do its steps in this turn.\n\n",
    );
    for app in apps {
        section.push_str(&app_line(app));
    }
    section
}

pub(crate) fn build_intents_section(intents: &[crate::core::Intent]) -> String {
    if intents.is_empty() {
        return String::new();
    }
    let mut section = String::from("\n\n## Available Intents\n\n");
    section.push_str(
        "Stored descriptions of what the user wants. To run one, load_knowhow its id: \
         that returns the intent with its knowhow. Then do its steps in this turn. Use \
         run_thread only when the work truly needs its own context. Never re-load the \
         procedure the current trigger fire already runs.\n\n",
    );
    for intent in intents {
        section.push_str(&intent_line(intent));
    }
    section
}

pub(crate) fn intent_line(intent: &crate::core::Intent) -> String {
    format!("- **{}** (id: `{}`)\n", intent.name, intent.id)
}

pub(crate) fn build_user_profile_section(user_profile: &str) -> String {
    if user_profile.is_empty() {
        return String::new();
    }
    format!(
        "[USER PROFILE - ALREADY IN CONTEXT - NEVER call read_file on user_profile.md]\n{}\n[END PROFILE]",
        user_profile
    )
}

pub(crate) fn build_stored_credentials_section(creds: &[crate::core::CredentialInfo]) -> String {
    if creds.is_empty() {
        return String::new();
    }
    let lines: Vec<String> = creds.iter().map(credential_line).collect();
    format!(
        "[CONFIGURED API CREDENTIALS - auth headers are auto-injected for these services]\n{}\nYou can use http_request directly with these APIs - credentials are automatically added.\n[END CREDENTIALS]",
        lines.join("\n")
    )
}

pub(crate) fn build_credentials_section(
    creds: &[crate::core::CredentialInfo],
    builtin_proxies: &str,
) -> String {
    [
        build_stored_credentials_section(creds).as_str(),
        builtin_proxies,
    ]
    .into_iter()
    .filter(|section| !section.is_empty())
    .collect::<Vec<_>>()
    .join("\n")
}

pub(crate) fn credential_line(cred: &crate::core::CredentialInfo) -> String {
    format!("  - {} ({})", cred.service_name, cred.base_urls.join(", "))
}

pub(crate) fn build_email_accounts_section(accounts: &[crate::core::EmailAccountInfo]) -> String {
    if accounts.is_empty() {
        return String::new();
    }
    let mut section = String::from("[CONFIGURED EMAIL ACCOUNTS]\n");
    for account in accounts {
        section.push_str(&email_account_line(account));
    }
    section.push_str("[END EMAIL ACCOUNTS]");
    section
}

pub(crate) fn email_account_line(account: &crate::core::EmailAccountInfo) -> String {
    let auth = if account.oauth_account_id.is_some() {
        " [OAuth]"
    } else {
        ""
    };
    format!("- {} ({}){}\n", account.name, account.email_address, auth)
}

pub(crate) fn build_oauth_accounts_section(accounts: &[crate::core::OAuthAccountInfo]) -> String {
    if accounts.is_empty() {
        return String::new();
    }
    let lines: Vec<String> = accounts.iter().map(oauth_account_line).collect();
    format!(
        "\n[CONNECTED OAUTH ACCOUNTS - auth tokens are auto-injected and refreshed for these providers]\n{}\nYou can use http_request with these providers' APIs — OAuth tokens are automatically added.\n[END OAUTH ACCOUNTS]",
        lines.join("\n")
    )
}

pub(crate) fn oauth_account_line(account: &crate::core::OAuthAccountInfo) -> String {
    let email = account.email.as_deref().unwrap_or("unknown");
    format!(
        "  - {} ({}) — scopes: {}",
        account.provider, email, account.scopes
    )
}

pub(crate) fn build_mcp_servers_section(lines: &[String]) -> String {
    if lines.is_empty() {
        return String::new();
    }
    format!(
        "[MCP SERVERS]\nThe MCP servers of this workspace. Only a server enabled in this thread sends its tools here.\n{}\n[END MCP SERVERS]",
        lines.join("\n")
    )
}

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
        section.push_str(&knowhow_line(kh));
    }
    for (app_id, kh) in app_summaries {
        section.push_str(&app_knowhow_routing_line(app_id, kh));
    }
    section
}

pub(crate) fn knowhow_line(kh: &KnowhowSummary) -> String {
    format!(
        "- **{}** (id: `{}`): {}\n",
        kh.name,
        kh.id,
        routing_description(&kh.description)
    )
}

pub(crate) fn app_knowhow_routing_line(app_id: &str, kh: &KnowhowSummary) -> String {
    format!(
        "- **{}** (id: `{}`, app: {}): {}\n",
        kh.name,
        app_scoped_id(app_id, &kh.id),
        app_id,
        routing_description(&kh.description)
    )
}

pub(crate) fn app_scoped_id(app_id: &str, knowhow_id: &str) -> String {
    format!("{}/{}", app_id, knowhow_id)
}

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
    use crate::core::{AppKind, AppReveal};
    use std::fs;
    use std::path::Path;

    fn files(paths: &[&str]) -> Vec<String> {
        paths.iter().map(|p| p.to_string()).collect()
    }

    const WORKSPACE_PROMPT_FOOTPRINT_FIXTURE_BUDGET_CHARS: usize = 12_800;

    fn busy_workspace() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        let data = dir.path().join("data");
        let write = |rel: &str, body: &str| {
            let path: &Path = &data.join(rel);
            fs::create_dir_all(path.parent().expect("has parent")).expect("mkdir");
            fs::write(path, body).expect("write");
        };

        for i in 0..300 {
            write(
                &format!("apps/demo-director/remotion/node_modules/pkg{i:03}/index.js"),
                "module.exports = {};",
            );
        }

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
        write(
            "apps/app00/knowhow/flow.md",
            &format!(
                "---\nname: Flow\ndescription: {}\n---\n# Flow\n\nBody.\n",
                knowhow_description.replace('\n', " ")
            ),
        );

        for i in 0..12 {
            write(&format!("artifacts/report{i:02}.md"), "# Report");
            write(&format!("artifacts/screenshots/shot{i:02}.png"), "png");
            write(&format!("artifacts/deep/nested/tree/note{i:02}.md"), "note");
        }
        write("triggers/nightly/run.intent", "name: nightly");

        dir
    }

    #[test]
    fn busy_workspace_prompt_footprint_stays_under_budget() {
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
        crate::log_legacy!("workspace prompt footprint:\n{breakdown}\n  {total:>7} chars  TOTAL");

        assert!(
            total <= WORKSPACE_PROMPT_FOOTPRINT_FIXTURE_BUDGET_CHARS,
            "workspace prompt footprint is {total} chars, over the \
             {WORKSPACE_PROMPT_FOOTPRINT_FIXTURE_BUDGET_CHARS} ceiling. Every character is billed \
             on every turn of every thread in this workspace, so tighten a \
             builder or raise the ceiling in a change that says why:\n{breakdown}"
        );
    }

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

    #[test]
    fn an_unbroken_run_is_cut_at_the_budget() {
        let text = "y".repeat(500);
        let cut = truncate_on_word_boundary(&text, 100);
        assert_eq!(cut.chars().count(), 101);
    }

    #[test]
    fn truncation_never_splits_a_multibyte_character() {
        let text = "æøå ñ ünïcödé ".repeat(60);
        let cut = truncate_on_word_boundary(&text, 50);
        assert!(cut.chars().count() <= 51);
        assert!(!cut.contains('\u{fffd}'));
    }

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
            reveal: AppReveal::OnLoad,
            kind: AppKind::App,
            origin: None,
            reusable: false,
            params: Default::default(),
            built_in: false,
            sizes: Vec::new(),
            transparent: false,
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
            reveal: AppReveal::OnLoad,
            kind: AppKind::App,
            origin: None,
            reusable: false,
            params: Default::default(),
            built_in: false,
            sizes: Vec::new(),
            transparent: false,
        }];
        assert!(build_apps_section(&apps)
            .contains("- **Documents** (id: `documents`): Browse and search saved documents.\n"));
    }

    #[test]
    fn the_apps_and_intents_sections_run_an_intent_through_load_knowhow() {
        let apps = vec![App {
            id: "documents".to_string(),
            name: "Documents".to_string(),
            description: "Browse saved documents.".to_string(),
            icon: None,
            reveal: AppReveal::OnLoad,
            kind: AppKind::App,
            origin: None,
            reusable: false,
            params: Default::default(),
            built_in: false,
            sizes: Vec::new(),
            transparent: false,
        }];
        let intents = vec![crate::core::Intent {
            id: "weekly-review".to_string(),
            name: "Weekly review".to_string(),
            knowhow: Vec::new(),
            content: String::new(),
        }];
        let apps_section = build_apps_section(&apps);
        let intents_section = build_intents_section(&intents);
        for section in [&apps_section, &intents_section] {
            assert!(
                !section.contains("execute_intent"),
                "retired tool leaked: {section}"
            );
            assert!(section.contains("load_knowhow"), "got: {section}");
            assert!(section.contains("in this turn"), "got: {section}");
        }
        for rule in [
            "run_thread only when the work truly needs its own context",
            "Never re-load the procedure the current trigger fire already runs",
        ] {
            assert!(
                intents_section.contains(rule),
                "missing {rule:?}: {intents_section}"
            );
        }
    }

    #[tokio::test]
    async fn the_apps_section_names_no_widget() {
        let bus = crate::test_support::offline_event_bus();
        let tmp = tempfile::tempdir().unwrap();
        let manager = AppManager::new(tmp.path()).unwrap();
        crate::core::apps::tests::app_and_widget(&manager, &bus).await;
        let section = apps_section(&manager, true).unwrap();
        assert!(section.contains("`habit-tracker`"), "got: {section}");
        assert!(!section.contains("fare-grid"), "a widget leaked: {section}");

        manager
            .set_widget_reusable(&bus, "fare-grid", true, None)
            .await
            .unwrap();
        let section = apps_section(&manager, true).unwrap();
        let (apps, widgets) = section.split_once("## Widgets").unwrap();
        assert!(
            !apps.contains("fare-grid"),
            "a widget leaked into apps: {apps}"
        );
        assert!(
            widgets.contains("`fare-grid`"),
            "the reusable list names it: {widgets}"
        );
    }

    #[test]
    fn automatic_widgets_off_stops_only_the_agents_own_choice() {
        let on = build_widgets_section(&[], true);
        assert!(on.contains("Choose one yourself"), "got: {on}");
        assert!(
            on.contains("never list those as question options"),
            "a pick is a widget, never question-card options: {on}"
        );
        let off = build_widgets_section(&[], false);
        assert!(!off.contains("Choose one yourself"), "got: {off}");
        assert!(
            !off.contains("never list those as question options"),
            "off, a pick is offered as an option instead: {off}"
        );
        assert!(
            off.contains("only when the user asks"),
            "a requested widget is still made: {off}"
        );
        assert!(
            off.contains("kind=\"widget\""),
            "the way to make one stays: {off}"
        );
        assert!(
            off.contains("offer one as a question option"),
            "a pick still gets a widget offered: {off}"
        );
    }

    #[test]
    fn the_widgets_section_says_where_how_big_what_name_and_when() {
        for automatic in [true, false] {
            let section = build_widgets_section(&[], automatic);
            for rule in [
                "then embed it in your reply",
                "fixed box of its size, the same in its thread card and on a dashboard",
                "style under html[data-widget-size]",
                "never by shrinking text or scrolling inside",
                "two or three words (\"Flight picker\")",
                "pick among data items",
            ] {
                assert!(section.contains(rule), "missing {rule:?}: {section}");
            }
        }
    }

    #[test]
    fn the_widgets_section_says_when_to_size_how_to_move_and_where_data_comes_from() {
        for automatic in [true, false] {
            let section = build_widgets_section(&[], automatic);
            assert!(!section.contains("pin"), "no pin (I8): {section}");
            for rule in [
                "resize a card only when the user asks in this turn",
                "never choose a size yourself",
                "Animate what a tap changes",
                "--duration-* tokens",
                "data-motion=\"reduce\"",
                "Read data through lucidos.*",
                "listen on lucidos.sse",
                "instead of baking it into the HTML",
            ] {
                assert!(section.contains(rule), "missing {rule:?}: {section}");
            }
        }
    }

    #[test]
    fn empty_inputs_produce_no_sections() {
        assert!(build_apps_section(&[]).is_empty());
        assert!(!build_widgets_section(&[], true).contains("Widgets to embed"));
        assert!(build_knowhow_section(&[], &[]).is_empty());
        assert!(build_app_knowhow_listing("demo", &[]).is_empty());
        assert!(crate::engine::widget_guidance::widgets_list(&[]).is_empty());
        assert!(build_intents_section(&[]).is_empty());
        assert!(build_user_profile_section("").is_empty());
        assert!(build_stored_credentials_section(&[]).is_empty());
        assert!(build_email_accounts_section(&[]).is_empty());
        assert!(build_oauth_accounts_section(&[]).is_empty());
        assert!(build_mcp_servers_section(&[]).is_empty());
    }

    fn summary(id: &str, name: &str, description: &str) -> KnowhowSummary {
        KnowhowSummary {
            id: id.to_string(),
            name: name.to_string(),
            description: description.to_string(),
        }
    }

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

    #[test]
    fn the_two_surfaces_print_the_same_app_scoped_id() {
        let kh = summary("nested/deep", "Deep dive", "The long version.");
        let listing = build_app_knowhow_listing("habit-tracker", std::slice::from_ref(&kh));
        let routing = build_knowhow_section(&[], &[("habit-tracker".to_string(), kh)]);

        let id = "(id: `habit-tracker/nested/deep`";
        assert!(listing.contains(id), "{listing}");
        assert!(routing.contains(id), "{routing}");
    }

    #[test]
    fn a_printed_app_scoped_id_loads_the_apps_doc() {
        let tmp = tempfile::tempdir().unwrap();
        let local = tmp.path().join("local");
        fs::create_dir_all(&local).unwrap();
        let apps = tmp.path().join("apps");
        let doc_dir = apps.join("habit-tracker").join("knowhow").join("nested");
        fs::create_dir_all(&doc_dir).unwrap();
        fs::write(
            doc_dir.join("deep.md"),
            "---\nname: Deep dive\ndescription: The long version.\n---\nBody.",
        )
        .unwrap();

        let dirs = crate::core::knowhow::KnowhowDirs {
            shared: None,
            local,
            apps: Some(apps),
            triggers: None,
        };
        let id = app_scoped_id("habit-tracker", "nested/deep");
        let loaded = crate::core::knowhow::KnowhowStore::load_with_fallback(&dirs, &id)
            .expect("the id the listing prints must load");
        assert_eq!(loaded.name, "Deep dive");
    }

    #[test]
    fn the_app_listing_size_does_not_grow_with_a_docs_text() {
        let medium =
            build_app_knowhow_listing("demo", &[summary("flow", "Flow", &"word ".repeat(400))]);
        let huge =
            build_app_knowhow_listing("demo", &[summary("flow", "Flow", &"word ".repeat(40_000))]);
        assert_eq!(medium.len(), huge.len());
    }
}
