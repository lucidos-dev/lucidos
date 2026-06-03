use super::*;
use crate::test_util::KeywordEmbedder;

#[test]
fn parse_knowhow_with_description() {
    let text = "---\nname: Panasonic\ndescription: Controls Panasonic heatpumps via Comfort Cloud API\n---\n## API\nBase URL...";
    let (name, description, _) = parse_frontmatter(text).unwrap();
    assert_eq!(name, "Panasonic");
    assert_eq!(
        description,
        "Controls Panasonic heatpumps via Comfort Cloud API"
    );
}

#[test]
fn parse_knowhow_derives_description_from_body() {
    let text = "---\nname: Panasonic\n---\n# Heatpump API\nControls and monitors heatpumps.\nMore details.";
    let (name, description, _) = parse_frontmatter(text).unwrap();
    assert_eq!(name, "Panasonic");
    assert_eq!(description, "Panasonic: Controls and monitors heatpumps.");
}

#[test]
fn parse_knowhow_derives_description_skips_headings() {
    let text = "---\nname: Calendar\n---\n# Google Calendar\n\n## Purpose\n- Show events from imported calendars";
    let (name, description, _) = parse_frontmatter(text).unwrap();
    assert_eq!(name, "Calendar");
        assert_eq!(
            description,
            "Calendar: - Show events from imported calendars"
    );
}

#[test]
fn parse_knowhow_name_only_fallback() {
    let text = "---\nname: Empty Doc\n---\n# Just a heading\n";
    let (name, description, _) = parse_frontmatter(text).unwrap();
    assert_eq!(name, "Empty Doc");
    assert_eq!(description, "Empty Doc");
}

#[test]
fn parse_ignores_legacy_domains_and_keywords() {
    let text = "---\nname: Panasonic\ndomains:\n  - heatpump\n  - panasonic\n---\n# Heatpump API\nControls heatpumps.";
    let (name, description, _) = parse_frontmatter(text).unwrap();
    assert_eq!(name, "Panasonic");
    // Legacy domains are ignored; description is derived from body
    assert_eq!(description, "Panasonic: Controls heatpumps.");
}

#[test]
fn summary_excludes_content() {
    let summary = KnowhowSummary {
        id: "test".to_string(),
        name: "Test".to_string(),
        description: "Test description".to_string(),
    };
    let json = serde_json::to_string(&summary).unwrap();
    assert!(!json.contains("content"));
}

#[cfg(feature = "real-embedder-tests")]
#[tokio::test]
async fn discover_semantic_match() {
    let provider = crate::test_util::shared_embedder();
    let summaries = vec![KnowhowSummary {
        id: "heatpump".to_string(),
        name: "Heatpump".to_string(),
        description: "Controls and monitors Panasonic heatpumps via Comfort Cloud API"
            .to_string(),
    }];
    let result = discover_knowhow(
        "What is the heatpump temperature?",
        &summaries,
        &[],
        provider,
    )
    .await;
    assert!(
        result.contains(&"heatpump".to_string()),
        "should match heatpump semantically"
    );
}

#[cfg(feature = "real-embedder-tests")]
#[tokio::test]
async fn discover_ranks_related_above_unrelated() {
    let provider = crate::test_util::shared_embedder();
    let description = "Controls and monitors Panasonic heatpumps via Comfort Cloud API";
    let related = "What is the heatpump temperature?";
    let unrelated = "How to bake a chocolate cake with dark chocolate ganache";

    let embeddings = provider
        .embed_batch(&[description, related, unrelated])
        .await
        .expect("embedding should succeed");

    let related_sim = cosine_similarity(&embeddings[0], &embeddings[1]);
    let unrelated_sim = cosine_similarity(&embeddings[0], &embeddings[2]);

    assert!(
        related_sim > unrelated_sim + 0.02,
        "embedder should rank related above unrelated by a clear margin: \
         related={related_sim}, unrelated={unrelated_sim}"
    );
}

#[tokio::test]
async fn discover_includes_explicit_refs() {
    let provider = KeywordEmbedder::new(&["heatpump"]);
    let summaries = vec![KnowhowSummary {
        id: "heatpump".to_string(),
        name: "Heatpump".to_string(),
        description: "Controls Panasonic heatpumps".to_string(),
    }];
    let result = discover_knowhow(
        "unrelated message about cooking",
        &summaries,
        &["heatpump".to_string()],
        &provider,
    )
    .await;
    assert_eq!(result, vec!["heatpump"]);
}

#[tokio::test]
async fn discover_deduplicates_explicit_and_semantic() {
    let provider = KeywordEmbedder::new(&["heatpump"]);
    let summaries = vec![KnowhowSummary {
        id: "heatpump".to_string(),
        name: "Heatpump".to_string(),
        description: "Controls and monitors Panasonic heatpumps via Comfort Cloud API"
            .to_string(),
    }];
    let result = discover_knowhow(
        "Check the heatpump temperature",
        &summaries,
        &["heatpump".to_string()],
        &provider,
    )
    .await;
    assert_eq!(result, vec!["heatpump"]);
    assert_eq!(result.len(), 1);
}

#[cfg(feature = "real-embedder-tests")]
#[tokio::test]
async fn discover_ranks_by_relevance() {
    let provider = crate::test_util::shared_embedder();
    let summaries = vec![
        KnowhowSummary {
            id: "heatpump".to_string(),
            name: "Heatpump".to_string(),
            description: "Controls and monitors Panasonic heatpumps via Comfort Cloud API"
                .to_string(),
        },
        KnowhowSummary {
            id: "calendar".to_string(),
                name: "Google Calendar".to_string(),
                description:
                    "Google Calendar integration for viewing and managing events and schedules"
                    .to_string(),
        },
    ];
    let result = discover_knowhow(
        "What is the heatpump temperature set to?",
        &summaries,
        &[],
        provider,
    )
    .await;
    assert!(!result.is_empty(), "should have at least one match");
    assert_eq!(result[0], "heatpump", "heatpump should be ranked first");
}

#[tokio::test]
async fn discover_empty_message_returns_only_explicit() {
    // No embedder calls expected — empty message short-circuits before the
    // embed_batch call. Use the mock to keep this test offline-safe.
    let provider = KeywordEmbedder::new(&["test"]);
    let summaries = vec![KnowhowSummary {
        id: "test".to_string(),
        name: "Test".to_string(),
        description: "Test description".to_string(),
    }];
    let result =
        discover_knowhow("", &summaries, &["explicit".to_string()], &provider).await;
    assert_eq!(result, vec!["explicit"]);
}

#[test]
fn load_summaries_discovers_files_in_subdirectories() {
    let tmp = tempfile::tempdir().unwrap();
    let kh = tmp.path().join("knowhow");

    write_knowhow_file(&kh.join("top.md"), "Top Level", "Top-level content.");
    write_knowhow_file(
        &kh.join("lucidos").join("nested.md"),
        "Nested",
        "Nested content.",
    );
    write_knowhow_file(
        &kh.join("lucidos").join("deep").join("deep-file.md"),
        "Deep",
        "Deep content.",
    );

    let summaries = KnowhowStore::load_summaries(&kh);
    let ids: Vec<&str> = summaries.iter().map(|s| s.id.as_str()).collect();

    assert!(
        ids.contains(&"top"),
        "should find top-level file, got: {:?}",
        ids
    );
    assert!(
        ids.iter().any(|id| id.contains("nested")),
        "should find nested file, got: {:?}",
        ids
    );
    assert!(
        ids.iter().any(|id| id.contains("deep-file")),
        "should find deeply nested file, got: {:?}",
        ids
    );
    assert_eq!(summaries.len(), 3);
}

#[test]
fn load_by_id_finds_file_in_subdirectory() {
    let tmp = tempfile::tempdir().unwrap();
    let kh = tmp.path().join("knowhow");

    write_knowhow_file(
        &kh.join("lucidos").join("nested.md"),
        "Nested Doc",
        "Nested doc content.",
    );

    let summaries = KnowhowStore::load_summaries(&kh);
    assert_eq!(summaries.len(), 1);
    let id = &summaries[0].id;

    let loaded = KnowhowStore::load(&kh, id);
    assert!(loaded.is_some(), "should load nested file by id '{}'", id);
    assert_eq!(loaded.unwrap().name, "Nested Doc");
}

#[test]
fn load_summary_has_description() {
    let tmp = tempfile::tempdir().unwrap();
    let kh = tmp.path().join("knowhow");

    write_knowhow_file(
        &kh.join("test.md"),
        "Test Doc",
        "This is the first paragraph.",
    );

    let summaries = KnowhowStore::load_summaries(&kh);
    assert_eq!(summaries.len(), 1);
    assert_eq!(
        summaries[0].description,
        "Test Doc: This is the first paragraph."
    );
}

#[test]
fn load_summary_uses_frontmatter_description() {
    let tmp = tempfile::tempdir().unwrap();
    let kh = tmp.path().join("knowhow");

    std::fs::create_dir_all(&kh).unwrap();
    std::fs::write(
        kh.join("test.md"),
        "---\nname: Test\ndescription: Custom description from frontmatter\n---\nBody content.",
    )
    .unwrap();

    let summaries = KnowhowStore::load_summaries(&kh);
    assert_eq!(summaries.len(), 1);
    assert_eq!(
        summaries[0].description,
        "Custom description from frontmatter"
        );
    }

    fn dirs(shared: Option<&std::path::Path>, local: &std::path::Path) -> KnowhowDirs {
        KnowhowDirs {
            shared: shared.map(|p| p.to_path_buf()),
            local: local.to_path_buf(),
            apps: None,
            triggers: None,
        }
    }

    fn dirs_with_apps(
        shared: Option<&std::path::Path>,
        local: &std::path::Path,
        apps: &std::path::Path,
    ) -> KnowhowDirs {
        KnowhowDirs {
            shared: shared.map(|p| p.to_path_buf()),
            local: local.to_path_buf(),
            apps: Some(apps.to_path_buf()),
            triggers: None,
        }
    }

    fn dirs_with_triggers(
        local: &std::path::Path,
        triggers: &std::path::Path,
    ) -> KnowhowDirs {
        KnowhowDirs {
            shared: None,
            local: local.to_path_buf(),
            apps: None,
            triggers: Some(triggers.to_path_buf()),
        }
    }

    // --- Task 1: load_merged_summaries ---

    #[test]
    fn load_merged_summaries_local_overrides_shared() {
        let tmp = tempfile::tempdir().unwrap();
        let shared = tmp.path().join("shared");
    let local = tmp.path().join("local");

    write_knowhow_file(
        &shared.join("google-calendar.md"),
        "Google Calendar (shared)",
        "Shared version.",
    );
    write_knowhow_file(
        &shared.join("lucidos.md"),
        "Lucidos (shared)",
        "Shared Lucidos knowhow.",
    );

    write_knowhow_file(
        &local.join("google-calendar.md"),
        "Google Calendar (local)",
        "Local version.",
    );
    write_knowhow_file(&local.join("heatpump.md"), "Heatpump", "Heatpump content.");

    let summaries = KnowhowStore::load_merged_summaries(&dirs(Some(&shared), &local));
    let ids: Vec<&str> = summaries.iter().map(|s| s.id.as_str()).collect();

    assert_eq!(
        summaries.len(),
        3,
        "should have 3 unique entries: {:?}",
        ids
    );
    assert!(ids.contains(&"google-calendar"));
        assert!(ids.contains(&"lucidos"));
    assert!(ids.contains(&"heatpump"));

    let gc = summaries
        .iter()
        .find(|s| s.id == "google-calendar")
            .unwrap();
        assert!(
            gc.name.contains("local"),
        "local should override shared, got: {}",
        gc.name
    );
}

#[test]
fn load_merged_summaries_shared_none_is_local_only() {
    let tmp = tempfile::tempdir().unwrap();
    let local = tmp.path().join("local");
    write_knowhow_file(&local.join("test.md"), "Test", "Content.");

    let summaries = KnowhowStore::load_merged_summaries(&dirs(None, &local));
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].id, "test");
}

// --- load_with_fallback ---

#[test]
fn load_with_fallback_prefers_local() {
    let tmp = tempfile::tempdir().unwrap();
    let shared = tmp.path().join("shared");
    let local = tmp.path().join("local");

    write_knowhow_file(&shared.join("test.md"), "Shared Test", "Shared content.");
    write_knowhow_file(&local.join("test.md"), "Local Test", "Local content.");

    let kh = KnowhowStore::load_with_fallback(&dirs(Some(&shared), &local), "test").unwrap();
    assert_eq!(kh.name, "Local Test");
}

#[test]
fn load_with_fallback_falls_back_to_shared() {
    let tmp = tempfile::tempdir().unwrap();
    let shared = tmp.path().join("shared");
    let local = tmp.path().join("local");
    std::fs::create_dir_all(&local).unwrap();

    write_knowhow_file(
        &shared.join("only-shared.md"),
        "Only Shared",
        "Shared content.",
    );

    let kh = KnowhowStore::load_with_fallback(&dirs(Some(&shared), &local), "only-shared")
        .unwrap();
    assert_eq!(kh.name, "Only Shared");
}

#[test]
fn load_with_fallback_none_when_missing() {
    let tmp = tempfile::tempdir().unwrap();
    let local = tmp.path().join("local");
    std::fs::create_dir_all(&local).unwrap();

    let kh = KnowhowStore::load_with_fallback(&dirs(None, &local), "missing");
    assert!(kh.is_none());
}

// --- load_knowhow_sections_merged ---

#[test]
fn load_knowhow_sections_merged_uses_both_sources() {
    let tmp = tempfile::tempdir().unwrap();
    let shared = tmp.path().join("shared");
    let local = tmp.path().join("local");

    write_knowhow_file(
        &shared.join("shared-ref.md"),
        "Shared Ref",
        "Shared reference content.",
    );
    write_knowhow_file(
        &local.join("local-ref.md"),
        "Local Ref",
        "Local reference content.",
    );

    let ids = vec!["shared-ref".to_string(), "local-ref".to_string()];
    let sections = load_knowhow_sections_merged(&dirs(Some(&shared), &local), None, &ids);
    assert!(
        sections.contains("Shared Ref"),
        "should include shared knowhow"
    );
    assert!(
        sections.contains("Local Ref"),
        "should include local knowhow"
    );
}

#[test]
fn load_knowhow_sections_merged_local_overrides_shared() {
    let tmp = tempfile::tempdir().unwrap();
    let shared = tmp.path().join("shared");
    let local = tmp.path().join("local");

    write_knowhow_file(
        &shared.join("overlap.md"),
        "Shared Version",
        "Shared content.",
    );
    write_knowhow_file(&local.join("overlap.md"), "Local Version", "Local content.");

    let ids = vec!["overlap".to_string()];
    let sections = load_knowhow_sections_merged(&dirs(Some(&shared), &local), None, &ids);
    assert!(
        sections.contains("Local Version"),
        "local should win over shared"
    );
    assert!(
        !sections.contains("Shared Version"),
        "shared should not appear when local exists"
    );
}

#[test]
fn load_knowhow_sections_merged_loads_system_knowhow_with_prefix() {
    let tmp = tempfile::tempdir().unwrap();
    let local = tmp.path().join("local");
    std::fs::create_dir_all(&local).unwrap();
    let system = tmp.path().join("system");

    write_knowhow_file(
        &system.join("best-practices.md"),
        "Best Practices",
        "System body content.",
    );

    let ids = vec!["system-knowhow/best-practices".to_string()];
    let sections =
        load_knowhow_sections_merged(&dirs(None, &local), Some(&system), &ids);
    assert!(
        sections.contains("[SYSTEM-KNOWHOW: Best Practices]"),
        "should tag with SYSTEM-KNOWHOW, got: {}",
        sections
    );
    assert!(sections.contains("System body content."));
}

#[test]
fn load_knowhow_sections_merged_mixes_system_and_user_knowhow() {
    let tmp = tempfile::tempdir().unwrap();
    let local = tmp.path().join("local");
    let system = tmp.path().join("system");

    write_knowhow_file(&local.join("user-doc.md"), "User Doc", "User body.");
    write_knowhow_file(&system.join("sys-doc.md"), "Sys Doc", "Sys body.");

    let ids = vec![
        "system-knowhow/sys-doc".to_string(),
        "user-doc".to_string(),
    ];
    let sections = load_knowhow_sections_merged(&dirs(None, &local), Some(&system), &ids);
    assert!(sections.contains("[SYSTEM-KNOWHOW: Sys Doc]"));
    assert!(sections.contains("[KNOW-HOW: User Doc]"));
}

#[test]
fn load_one_knowhow_section_returns_system_or_user_format() {
    let tmp = tempfile::tempdir().unwrap();
    let local = tmp.path().join("local");
    let system = tmp.path().join("system");
    write_knowhow_file(&local.join("user-doc.md"), "User Doc", "User body.");
    write_knowhow_file(&system.join("sys-doc.md"), "Sys Doc", "Sys body.");

    let user = load_one_knowhow_section(&dirs(None, &local), Some(&system), "user-doc")
        .expect("user knowhow should resolve");
    assert!(user.contains("[KNOW-HOW: User Doc]"));
    assert!(user.contains("User body."));

    let sys = load_one_knowhow_section(
        &dirs(None, &local),
        Some(&system),
        "system-knowhow/sys-doc",
    )
    .expect("system knowhow should resolve");
    assert!(sys.contains("[SYSTEM-KNOWHOW: Sys Doc]"));
    assert!(sys.contains("Sys body."));

    let missing = load_one_knowhow_section(&dirs(None, &local), Some(&system), "no-such");
    assert!(missing.is_none());
}

#[test]
fn load_knowhow_sections_merged_handles_missing_system_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let local = tmp.path().join("local");
    std::fs::create_dir_all(&local).unwrap();

    let ids = vec!["system-knowhow/anything".to_string()];
    let sections = load_knowhow_sections_merged(&dirs(None, &local), None, &ids);
    assert_eq!(sections, "", "missing system_dir should drop system ids silently");
}

// --- App-scoped knowhow resolution ---
//
// Knowhow ids of the form `<app>/<rest>` (e.g. `finn-jobs/finn-search-workflow`)
// — surfaced in the system prompt's Know-how list — must resolve against
// `<workspace>/data/apps/<app>/knowhow/<rest>.md` in addition to the top-level
// local/shared dirs.

#[test]
fn load_knowhow_sections_merged_loads_app_scoped_knowhow() {
    let tmp = tempfile::tempdir().unwrap();
    let local = tmp.path().join("local");
    std::fs::create_dir_all(&local).unwrap();
    let apps = tmp.path().join("apps");
    write_knowhow_file(
        &apps.join("morning-log").join("knowhow").join("morning-log-data.md"),
        "Morning log data",
        "Data layout for morning log.",
    );

    let ids = vec!["morning-log/morning-log-data".to_string()];
    let sections = load_knowhow_sections_merged(
        &dirs_with_apps(None, &local, &apps),
        None,
        &ids,
    );
    assert!(
        !sections.is_empty(),
        "app-scoped section should be loaded, got empty"
    );
    assert!(
        sections.contains("Morning log data"),
        "section should reference the file's name, got: {}",
        sections
    );
    assert!(
        sections.contains("Data layout for morning log."),
        "section should include body, got: {}",
        sections
    );
}

#[test]
fn load_with_fallback_loads_app_scoped_knowhow() {
    let tmp = tempfile::tempdir().unwrap();
    let local = tmp.path().join("local");
    std::fs::create_dir_all(&local).unwrap();
    let apps = tmp.path().join("apps");
    write_knowhow_file(
        &apps.join("foo").join("knowhow").join("bar.md"),
        "Foo Bar",
            "Body.",
    );

    let kh =
        KnowhowStore::load_with_fallback(&dirs_with_apps(None, &local, &apps), "foo/bar")
                .expect("app-scoped id should load");
    assert_eq!(kh.name, "Foo Bar");
    }

    #[test]
    fn load_with_fallback_prefers_local_over_app_scoped() {
        // If a top-level knowhow file shares the same id-shape as an app-scoped
        // one (e.g. local has `foo/bar.md`, apps also has `foo/knowhow/bar.md`),
        // the bare-id local match wins per the documented lookup order.
        let tmp = tempfile::tempdir().unwrap();
        let local = tmp.path().join("local");
    let apps = tmp.path().join("apps");
    write_knowhow_file(&local.join("foo").join("bar.md"), "Local Foo Bar", "Local.");
    write_knowhow_file(
        &apps.join("foo").join("knowhow").join("bar.md"),
        "App Foo Bar",
            "App.",
    );

    let kh =
        KnowhowStore::load_with_fallback(&dirs_with_apps(None, &local, &apps), "foo/bar")
                .expect("should load");
    assert_eq!(kh.name, "Local Foo Bar", "local must win over app-scoped");
}

// app_scoped_knowhow_path is the security boundary — an absolute or
// backslash-prefixed `rest` segment would let `Path::join("/etc/passwd.md")`
// replace the apps_dir prefix and escape to the filesystem root. The
// outer is_safe_id only sees the full id (which doesn't start with `/`),
// so the splitter has to re-validate `rest`.

#[test]
fn app_scoped_path_rejects_double_slash_escape() {
    // `foo//bar` splits to ("foo", "/bar"); `/bar.md` is absolute on Unix.
        let apps = std::path::PathBuf::from("/tmp/lucidos-test/apps");
    assert!(
        app_scoped_knowhow_path(&apps, "foo//bar").is_none(),
            "double-slash id must not produce a path",
    );
}

#[test]
fn app_scoped_path_rejects_backslash_escape() {
    let apps = std::path::PathBuf::from("/tmp/lucidos-test/apps");
    assert!(
        app_scoped_knowhow_path(&apps, "foo/\\escape").is_none(),
        "backslash-prefixed rest must not produce a path",
    );
}

#[test]
fn app_scoped_path_rejects_traversal_in_rest() {
    // Caught by the outer is_safe_id (`..` anywhere), but assert the
    // contract directly so a future refactor doesn't lose this behavior.
    let apps = std::path::PathBuf::from("/tmp/lucidos-test/apps");
    assert!(app_scoped_knowhow_path(&apps, "foo/../bar").is_none());
    }

    #[test]
    fn knowhow_not_found_body_branches_on_prefix() {
        let user = knowhow_not_found_body("foo/bar");
        assert!(user.starts_with("Know-how 'foo/bar' not found."));
    assert!(user.contains("Use the know-how list"));

    let sys = knowhow_not_found_body("system-knowhow/baz");
    assert!(sys.starts_with("System knowhow 'system-knowhow/baz' not found."));
    assert!(sys.contains("Check the System Knowhow list"));
}

#[test]
fn is_not_found_body_round_trips_with_knowhow_not_found_body() {
    // Both branches of knowhow_not_found_body must be detected as
    // not-found — the load_knowhow handler uses this to gate persistence
    // into the loaded set.
    assert!(is_not_found_body(&knowhow_not_found_body("any-id")));
    assert!(is_not_found_body(&knowhow_not_found_body(
        "system-knowhow/any-id"
    )));
}

#[test]
fn is_not_found_body_rejects_real_doc_bodies() {
    assert!(!is_not_found_body("just some random doc body"));
    // A formatted [KNOW-HOW: ...] section is the canonical "real" hit
    // shape — it must NOT match the not-found sentinel.
    assert!(!is_not_found_body(
        "[KNOW-HOW: Some Doc]\nbody\n[END KNOW-HOW]"
    ));
    assert!(!is_not_found_body(""));
}

// --- Trigger-scoped resolution via load_with_fallback ---

#[test]
fn load_with_fallback_resolves_trigger_scoped_id() {
    let tmp = tempfile::tempdir().unwrap();
    let local = tmp.path().join("local");
    std::fs::create_dir_all(&local).unwrap();
    let triggers = tmp.path().join("triggers");
    write_knowhow_file(
        &triggers
            .join("nightly-build")
            .join("knowhow")
            .join("orchestration.md"),
        "Orchestration",
        "Body.",
    );

    let kh = KnowhowStore::load_with_fallback(
        &dirs_with_triggers(&local, &triggers),
        "triggers/nightly-build/orchestration",
    )
    .expect("trigger-scoped id should resolve");
    assert_eq!(kh.name, "Orchestration");
    assert_eq!(kh.id, "triggers/nightly-build/orchestration");
}

#[test]
fn load_with_fallback_returns_none_for_unknown_trigger_id() {
    let tmp = tempfile::tempdir().unwrap();
    let local = tmp.path().join("local");
    std::fs::create_dir_all(&local).unwrap();
    let triggers = tmp.path().join("triggers");
    std::fs::create_dir_all(&triggers).unwrap();

    let kh = KnowhowStore::load_with_fallback(
        &dirs_with_triggers(&local, &triggers),
        "triggers/unknown/foo",
    );
    assert!(kh.is_none());
}

#[test]
fn trigger_scoped_path_rejects_traversal() {
    let triggers = std::path::PathBuf::from("/tmp/lucidos-test/triggers");
    // outer is_safe_id catches `..`
    assert!(trigger_scoped_knowhow_path(&triggers, "triggers/foo/../bar").is_none());
        // empty slug
        assert!(trigger_scoped_knowhow_path(&triggers, "triggers//bar").is_none());
        // empty rest
        assert!(trigger_scoped_knowhow_path(&triggers, "triggers/foo/").is_none());
    // missing prefix
    assert!(trigger_scoped_knowhow_path(&triggers, "foo/bar").is_none());
    }

    // --- Per-trigger knowhow summaries ---

    #[test]
    fn load_trigger_summaries_reads_files_under_slug_knowhow_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let triggers = tmp.path().join("triggers");
    let kh_dir = triggers.join("nightly-build").join("knowhow");
    write_knowhow_file(
        &kh_dir.join("orchestration.md"),
        "Orchestration",
        "How nightly orchestrates each phase.",
    );
    write_knowhow_file(
        &kh_dir.join("rollback.md"),
        "Rollback",
        "Rollback procedure when a phase fails.",
    );

    let summaries = KnowhowStore::load_trigger_summaries(&triggers, "nightly-build");
    let ids: Vec<&str> = summaries.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(summaries.len(), 2, "got: {:?}", ids);
    assert!(ids.contains(&"orchestration"));
    assert!(ids.contains(&"rollback"));
    let orch = summaries.iter().find(|s| s.id == "orchestration").unwrap();
    assert_eq!(orch.name, "Orchestration");
}

#[test]
fn load_trigger_summaries_returns_empty_when_no_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let triggers = tmp.path().join("triggers");
    std::fs::create_dir_all(&triggers).unwrap();
    let summaries = KnowhowStore::load_trigger_summaries(&triggers, "no-such-slug");
    assert!(summaries.is_empty());
}

#[test]
fn app_scoped_path_builds_well_formed_path_for_safe_id() {
    let apps = std::path::PathBuf::from("/ws/data/apps");
    let p = app_scoped_knowhow_path(&apps, "finn-jobs/finn-search-workflow")
        .expect("safe id should produce a path");
    assert_eq!(
        p,
        std::path::PathBuf::from(
            "/ws/data/apps/finn-jobs/knowhow/finn-search-workflow.md"
        )
    );
}
