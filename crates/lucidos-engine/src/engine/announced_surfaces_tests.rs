mod guard_tests {
    use std::collections::{BTreeMap, BTreeSet};

    use crate::core::announced_surfaces::{
        Announcement, DataWriterRule, TableRule, DATA_WRITERS, RUNTIME_CREATED_TABLES, TABLES,
    };
    use crate::test_support::source_scan::{
        engine_source, is_test_path, production_sources, read_production_source, src_root,
    };
    use crate::test_support::{setup_test_db, teardown_test_db};

    struct FnSegment {
        name: String,
        reachable: bool,
        body: String,
    }

    fn fn_signature(line: &str) -> Option<(bool, String)> {
        static RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
            regex::Regex::new(
                r"^\s*(?P<vis>pub(?:\([^)]*\))?\s+)?(?:const\s+)?(?:async\s+)?(?:unsafe\s+)?(?:extern\s+\x22[^\x22]*\x22\s+)?fn\s+(?P<name>\w+)",
            )
            .expect("fn signature regex must compile")
        });
        let caps = RE.captures(line)?;
        Some((caps.name("vis").is_some(), caps["name"].to_string()))
    }

    fn fn_segments(text: &str) -> Vec<FnSegment> {
        let lines: Vec<&str> = text.lines().collect();
        let marks: Vec<(usize, bool, String)> = lines
            .iter()
            .enumerate()
            .filter_map(|(i, l)| fn_signature(l).map(|(vis, name)| (i, vis, name)))
            .collect();
        marks
            .iter()
            .enumerate()
            .map(|(k, (start, reachable, name))| {
                let end = marks.get(k + 1).map(|(i, _, _)| *i).unwrap_or(lines.len());
                FnSegment {
                    name: name.clone(),
                    reachable: *reachable,
                    body: lines[*start..end].join("\n"),
                }
            })
            .collect()
    }

    fn tables_written(body: &str) -> BTreeSet<String> {
        static RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
            regex::Regex::new(r#"(?i)\b(?:INSERT\s+INTO|UPDATE|DELETE\s+FROM)\s+"?([a-z_]+)"?"#)
                .expect("sql write regex must compile")
        });
        let known: BTreeSet<&str> = TABLES.iter().map(|r| r.table).collect();
        RE.captures_iter(body)
            .map(|c| c[1].to_lowercase())
            .filter(|t| known.contains(t.as_str()))
            .collect()
    }

    fn announces(body: &str) -> bool {
        static RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
            regex::Regex::new(r"\b(?:emit[a-z_]*|append_system_event)\s*\(")
                .expect("emit regex must compile")
        });
        RE.is_match(body)
    }

    fn mutates_filesystem(body: &str) -> bool {
        static RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
            regex::Regex::new(
                r"fs::(write|remove_file|remove_dir_all|create_dir_all|copy|rename)\s*\(",
            )
            .expect("fs mutation regex must compile")
        });
        RE.is_match(body)
    }

    fn announcement_of<'a>(table: &str) -> &'a Announcement
    where
        'static: 'a,
    {
        &TABLES
            .iter()
            .find(|r| r.table == table)
            .unwrap_or_else(|| panic!("no registry entry for table {table}"))
            .announcement
    }

    #[tokio::test]
    async fn every_table_in_the_schema_is_classified() {
        let (pool, db) = setup_test_db().await;
        let live: BTreeSet<String> = sqlx::query_scalar::<_, String>(
            "SELECT table_name FROM information_schema.tables \
         WHERE table_schema = 'public' AND table_type = 'BASE TABLE' \
           AND table_name <> '_sqlx_migrations'",
        )
        .fetch_all(&pool)
        .await
        .expect("read information_schema")
        .into_iter()
        .collect();
        teardown_test_db(&db).await;

        let registered: BTreeSet<String> = TABLES.iter().map(|r| r.table.to_string()).collect();

        let unclassified: Vec<&String> = live.difference(&registered).collect();
        assert!(
            unclassified.is_empty(),
            "these tables exist in the schema but have no entry in core::announced_surfaces::TABLES: \
         {unclassified:?}.\nAdd one, and decide whether mutations are Announced (the owning store \
         emits), a Projection (materialized from an already-announced event), or Silent (with the \
         reason). See the module doc."
        );

        let runtime_created: BTreeSet<&str> =
            RUNTIME_CREATED_TABLES.iter().map(|(t, _)| *t).collect();
        let stale: Vec<&String> = registered
            .difference(&live)
            .filter(|t| !runtime_created.contains(t.as_str()))
            .collect();
        assert!(
            stale.is_empty(),
            "these tables are in TABLES but no longer exist in the schema: {stale:?}. \
         Drop the registry entry with the migration that dropped the table, or list it in \
         RUNTIME_CREATED_TABLES if the engine creates it on first use."
        );
    }

    #[test]
    fn no_table_is_registered_twice() {
        let mut seen = BTreeSet::new();
        for rule in TABLES {
            assert!(
                seen.insert(rule.table),
                "table {} has more than one entry in TABLES",
                rule.table
            );
        }
    }

    #[test]
    fn raw_table_writes_live_only_in_the_owning_module() {
        let owners: BTreeMap<&str, BTreeSet<&str>> = TABLES
            .iter()
            .map(|r| (r.table, r.owners.iter().copied().collect()))
            .collect();

        let mut violations = Vec::new();
        for (rel, text) in production_sources() {
            for table in tables_written(&text) {
                let declared = &owners[table.as_str()];
                if !declared.contains(rel.as_str()) {
                    violations.push(format!(
                        "{rel} writes `{table}`, which is owned by {declared:?}"
                    ));
                }
            }
        }
        assert!(
            violations.is_empty(),
            "raw SQL writes escaped their owning module:\n  {}\n\nRoute the write through the owning \
         store so it cannot skip the announcement, or add this file to that table's `owners` if it \
         genuinely co-owns the table. (Scope: Rust sources only. A `.sql` migration may write any \
         table, because migrations run before the EventBus exists.)",
            violations.join("\n  ")
        );
    }

    #[test]
    fn every_declared_owner_file_exists() {
        let mut missing = Vec::new();
        for rule in TABLES {
            for owner in rule.owners {
                if !engine_source(owner).exists() {
                    missing.push(format!("{} (owner of table {})", owner, rule.table));
                }
            }
        }
        for rule in DATA_WRITERS {
            if !engine_source(rule.owner).exists() {
                missing.push(format!("{} (data writer for {})", rule.owner, rule.writes));
            }
        }
        assert!(
            missing.is_empty(),
            "registry names source files that do not exist:\n  {}",
            missing.join("\n  ")
        );
    }

    #[test]
    fn every_reachable_writer_of_an_announced_table_announces() {
        let mut violations = Vec::new();

        for rule in TABLES {
            let Announcement::Announced { exempt, .. } = &rule.announcement else {
                continue;
            };
            let exempt_names: BTreeSet<&str> = exempt.iter().map(|e| e.function).collect();

            for owner in rule.owners {
                let text = read_production_source(&engine_source(owner));
                for segment in fn_segments(&text) {
                    if !tables_written(&segment.body).contains(rule.table) {
                        continue;
                    }
                    if !segment.reachable || announces(&segment.body) {
                        continue;
                    }
                    if exempt_names.contains(segment.name.as_str()) {
                        continue;
                    }
                    violations.push(format!(
                        "{owner}::{} writes `{}` and is reachable from outside the module, but never \
                     emits",
                        segment.name, rule.table
                    ));
                }
            }
        }

        assert!(
            violations.is_empty(),
            "reachable writers of an announced table do not announce:\n  {}\n\nMove the emit into the \
         write path (see RepositoryStore::register), make the raw writer private and expose an \
         emitting mutator, or record an ExemptWriter with the reason on the registry entry.",
            violations.join("\n  ")
        );
    }

    #[test]
    fn every_exemption_names_a_function_that_exists() {
        let mut stale = Vec::new();

        for rule in TABLES {
            let Announcement::Announced { exempt, .. } = &rule.announcement else {
                continue;
            };
            for entry in *exempt {
                let found = rule.owners.iter().any(|owner| {
                    fn_segments(&read_production_source(&engine_source(owner)))
                        .iter()
                        .any(|s| s.name == entry.function)
                });
                if !found {
                    stale.push(format!(
                        "table {} exempts `{}`, which no owner file defines",
                        rule.table, entry.function
                    ));
                }
            }
        }
        assert!(
            stale.is_empty(),
            "stale exemptions:\n  {}\n\nDrop the ExemptWriter when the function goes away.",
            stale.join("\n  ")
        );
    }

    #[test]
    fn every_declared_event_is_a_real_system_event_variant() {
        let text = std::fs::read_to_string(src_root().join("engine/event_bus_system_event.rs"))
            .expect("read event_bus_system_event.rs");
        let variants: BTreeSet<String> = text
            .lines()
            .filter_map(|l| {
                let trimmed = l.strip_prefix("    ")?;
                if trimmed.starts_with(char::is_whitespace) {
                    return None;
                }
                let name = trimmed.trim_end().trim_end_matches(['{', ',', ' ']);
                (!name.is_empty()
                    && name.chars().next().is_some_and(char::is_uppercase)
                    && name.chars().all(|c| c.is_ascii_alphanumeric()))
                .then(|| name.to_string())
            })
            .collect();

        let declared: BTreeSet<&str> = TABLES
            .iter()
            .map(|r| &r.announcement)
            .chain(DATA_WRITERS.iter().map(|r| &r.announcement))
            .filter_map(|a| match a {
                Announcement::Announced { events, .. } => Some(*events),
                _ => None,
            })
            .flatten()
            .copied()
            .collect();

        let unknown: Vec<&&str> = declared
            .iter()
            .filter(|e| !variants.contains(**e))
            .collect();
        assert!(
            unknown.is_empty(),
            "registry names SystemEvent variants that do not exist: {unknown:?}.\nRename the registry \
         entry alongside the variant, in the same change."
        );
    }

    #[test]
    fn every_reachable_data_writer_announces() {
        let mut violations = Vec::new();

        for rule in DATA_WRITERS {
            let Announcement::Announced { exempt, .. } = &rule.announcement else {
                continue;
            };
            let exempt_names: BTreeSet<&str> = exempt.iter().map(|e| e.function).collect();
            let text = read_production_source(&engine_source(rule.owner));
            for segment in fn_segments(&text) {
                if !mutates_filesystem(&segment.body) {
                    continue;
                }
                if !segment.reachable || announces(&segment.body) {
                    continue;
                }
                if exempt_names.contains(segment.name.as_str()) {
                    continue;
                }
                violations.push(format!(
                    "{}::{} mutates the filesystem and is reachable from outside the module, but never \
                 emits",
                    rule.owner, segment.name
                ));
            }
        }

        assert!(
            violations.is_empty(),
            "reachable data/ writers do not announce:\n  {}\n\nTake the EventBus in the mutator and \
         emit from inside it, so no caller has to remember which entity event the path implies.",
            violations.join("\n  ")
        );
    }

    #[test]
    fn fn_segments_splits_on_signatures_and_reads_visibility() {
        let src = "\
impl Store {
    async fn private_writer(pool: &PgPool) {
        sqlx::query(\"DELETE FROM models WHERE id = $1\");
    }

    pub async fn reachable(pool: &PgPool) {
        Self::private_writer(pool).await;
        bus.emit_or_log(event).await;
    }
}
";
        let segments = fn_segments(src);
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].name, "private_writer");
        assert!(!segments[0].reachable);
        assert!(segments[0].body.contains("DELETE FROM models"));
        assert_eq!(segments[1].name, "reachable");
        assert!(segments[1].reachable);
        assert!(
            !segments[1].body.contains("DELETE FROM models"),
            "a segment must stop at the next signature, not swallow the previous body"
        );
    }

    #[test]
    fn announces_requires_a_call_not_a_mention() {
        assert!(announces("bus.emit_or_log(event, \"[X] Y\").await;"));
        assert!(announces(
            "self.event_bus.emit(BusEvent::System(e)).await?;"
        ));
        assert!(announces("emit_credential_event(bus, &cred).await;"));
        assert!(announces(
            "self.append_system_event(append, id, event, 0, None).await?;"
        ));
        assert!(!announces("// append_system_event will be added later"));
        assert!(
            !announces("// TODO: emit CredentialUpdated here"),
            "a comment promising an emit must not satisfy the rule"
        );
    }

    #[test]
    fn tables_written_ignores_prose_and_finds_real_sql() {
        assert!(tables_written("/// Update the stored value, then delete from disk.").is_empty());
        assert_eq!(
            tables_written("sqlx::query(\"UPDATE models SET label = $2 WHERE id = $1\")"),
            ["models".to_string()].into_iter().collect()
        );
        assert_eq!(
            tables_written("INSERT INTO credentials (service_name)"),
            ["credentials".to_string()].into_iter().collect()
        );
    }

    #[test]
    fn test_paths_are_excluded_by_convention() {
        assert!(is_test_path("core/oauth_tests.rs"));
        assert!(is_test_path("engine/event_bus_tests/mod.rs"));
        assert!(is_test_path("api/actor_test.rs"));
        assert!(is_test_path("bin/populate_memory.rs"));
        assert!(!is_test_path("core/credentials.rs"));
        assert!(!is_test_path("scheduler/push_test_log.rs"));
    }

    #[test]
    fn inline_test_modules_are_cut_before_scanning() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        let path = dir.join("sample.rs");
        std::fs::write(
            &path,
            "pub fn real() {}\n#[cfg(test)]\nmod tests {\n    fn fixture() { \"DELETE FROM models\"; }\n}\n",
        )
        .unwrap();
        let text = read_production_source(&path);
        assert!(text.contains("pub fn real"));
        assert!(
            tables_written(&text).is_empty(),
            "a fixture inside #[cfg(test)] must not read as a production write"
        );
    }

    #[test]
    fn a_test_only_file_has_no_production_text() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        let path = dir.join("helpers.rs");
        std::fs::write(
            &path,
            "//! Helpers.\n\n#![cfg(test)]\n\npub fn seed() { \"DELETE FROM models\"; }\n",
        )
        .unwrap();
        assert_eq!(read_production_source(&path), "");
        assert_eq!(
            read_production_source(&src_root().join("test_support.rs")),
            ""
        );
    }

    #[test]
    fn every_classification_carries_its_justification() {
        fn check(label: &str, announcement: &Announcement, problems: &mut Vec<String>) {
            match announcement {
                Announcement::Announced { events, exempt } => {
                    if events.is_empty() {
                        problems.push(format!("{label}: Announced with no events"));
                    }
                    for e in *exempt {
                        if e.why.trim().is_empty() {
                            problems
                                .push(format!("{label}: exemption {} has no reason", e.function));
                        }
                    }
                }
                Announcement::Projection { of } => {
                    if of.trim().is_empty() {
                        problems.push(format!("{label}: Projection with no source"));
                    }
                }
                Announcement::Silent { reason } => {
                    if reason.trim().is_empty() {
                        problems.push(format!("{label}: Silent with no reason"));
                    }
                }
            }
        }
        let mut problems = Vec::new();
        for rule in TABLES {
            check(rule.table, &rule.announcement, &mut problems);
            if matches!(rule.announcement, Announcement::Announced { .. }) {
                assert!(
                    !rule.owners.is_empty(),
                    "table {} is Announced but declares no owner file, so there is no write path to \
                 put the emit in",
                    rule.table
                );
            }
        }
        for rule in DATA_WRITERS {
            check(rule.owner, &rule.announcement, &mut problems);
        }
        assert!(problems.is_empty(), "{problems:?}");
    }

    #[test]
    fn every_runtime_created_table_is_classified_and_justified() {
        let registered: BTreeSet<&str> = TABLES.iter().map(|r| r.table).collect();
        for (table, why) in RUNTIME_CREATED_TABLES {
            assert!(
                registered.contains(table),
                "{table} is listed as runtime-created but has no TABLES entry"
            );
            assert!(
                !why.trim().is_empty(),
                "{table} is listed as runtime-created with no reason"
            );
        }
    }

    #[test]
    fn announcement_lookup_resolves_a_known_table() {
        assert!(matches!(
            announcement_of("repositories"),
            Announcement::Announced { .. }
        ));
        assert!(matches!(
            announcement_of("thread_summaries"),
            Announcement::Projection { .. }
        ));
    }

    #[test]
    fn registry_types_are_exercised() {
        let table: &TableRule = &TABLES[0];
        assert!(!table.table.is_empty());
        let data: &DataWriterRule = &DATA_WRITERS[0];
        assert!(!data.writes.is_empty());
    }
}
