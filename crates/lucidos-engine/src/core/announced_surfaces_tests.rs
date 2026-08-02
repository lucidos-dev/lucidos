//! Tripwires for the announced-surface registry.
//!
//! These are source-scan tests in the repo's existing tripwire idiom
//! (`skeleton-guard.test.ts`, `noAutofill.test.ts`, `check-em-dashes.sh`): they
//! read the engine's own sources and fail on a structure the registry forbids.
//! That is deliberate rather than leaving it to review, because the bug they
//! exist to stop (a state write whose announcement was left to the caller)
//! looks completely normal at the call site and only surfaces as a stale UI
//! days later.
//!
//! ## The scan's shape
//!
//! Rust is not parsed. Each source file is cut into **function segments** at
//! `fn` signature lines, which rustfmt keeps at a predictable indent, and each
//! segment runs to the next signature. That is enough to answer the two
//! questions the registry asks: is this writer reachable from outside its
//! module, and does it announce? Brace matching is deliberately avoided: SQL
//! and `format!` strings carry braces, so a naive matcher would mis-scope
//! exactly the functions this checks.
//!
//! Test modules are excluded by path convention (`*_test.rs`, `*_tests.rs`, a
//! `*_tests/` directory, `bin/`) and each remaining file is truncated at its
//! first top-level `#[cfg(test)]`. A test fixture writing a row directly is
//! setup, not a production write path.
//!
//! **`.sql` migrations are out of scope**, and safely so: they run before the
//! `EventBus` exists. See the registry's module doc, § "Migrations are outside
//! this".

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use super::{
    Announcement, DataWriterRule, TableRule, DATA_WRITERS, RUNTIME_CREATED_TABLES, TABLES,
};
use crate::test_support::{setup_test_db, teardown_test_db};

/// `crates/lucidos-engine/src`.
fn src_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

/// A path convention test module. Excluded from every scan: a fixture that
/// inserts a row directly is test setup, not a production write path.
fn is_test_path(rel: &str) -> bool {
    rel.split('/').any(|part| {
        let base = part.strip_suffix(".rs").unwrap_or(part);
        base == "tests" || base == "bin" || base.ends_with("_test") || base.ends_with("_tests")
    })
}

/// Read a source file with its inline test module cut off. Inline `mod tests`
/// sits at the end of the file by convention, so truncating at the first
/// top-level `#[cfg(test)]` drops it whole.
fn read_production_source(path: &Path) -> String {
    let text =
        std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    match text.find("\n#[cfg(test)]") {
        Some(idx) => text[..idx].to_string(),
        None => text,
    }
}

/// Every non-test engine source, as `(path relative to src/, production text)`.
fn production_sources() -> Vec<(String, String)> {
    let root = src_root();
    let mut out = Vec::new();
    let mut stack = vec![root.clone()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("read_dir").flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let rel = path
                .strip_prefix(&root)
                .expect("under src")
                .to_string_lossy()
                .replace('\\', "/");
            if is_test_path(&rel) {
                continue;
            }
            let text = read_production_source(&path);
            out.push((rel, text));
        }
    }
    out.sort();
    out
}

/// One function, sliced out by signature line.
struct FnSegment {
    name: String,
    /// Carries a visibility modifier, so a caller outside the module can reach
    /// it. `pub(crate)` and `pub(super)` count: the point is whether the write
    /// is reachable from another module, not how widely.
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

/// Cut a source file into function segments. Each runs from its signature line
/// to the line before the next signature, which is enough scope to ask whether
/// a writer announces without parsing Rust.
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

/// Tables written in a chunk of source. Matches the SQL verb plus the table
/// name so it can be filtered against the registry, which keeps English prose
/// in doc comments ("update the row", "delete from disk") out of the results.
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

/// Whether a function body contains an emit CALL. Call-shaped on purpose: a
/// doc comment that merely says "emit" must not satisfy the rule.
fn announces(body: &str) -> bool {
    static RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"\bemit[a-z_]*\s*\(").expect("emit regex must compile")
    });
    RE.is_match(body)
}

/// Whether a function body mutates the filesystem. Used for the `data/` writers,
/// which have no table to key on.
fn mutates_filesystem(body: &str) -> bool {
    static RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"fs::(write|remove_file|remove_dir_all|create_dir_all|copy|rename)\s*\(")
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

// ---------------------------------------------------------------------------
// Rule 1: completeness
// ---------------------------------------------------------------------------

/// Every table the migrations create must be classified. This is the rule that
/// makes omission impossible: a new migration cannot introduce an unannounced
/// surface, because the surface has to be named here first.
///
/// DB-backed rather than parsed out of the migration files, because the
/// migrations also rename and drop tables, and only the settled schema is the
/// truth.
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

    // A runtime-created table is absent from a migrated-but-unbooted database by
    // design, so its absence is not evidence of a stale entry.
    let runtime_created: BTreeSet<&str> = RUNTIME_CREATED_TABLES.iter().map(|(t, _)| *t).collect();
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

/// A table must appear once. Two entries would let a reviewer read the lenient
/// one and miss the strict one.
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

// ---------------------------------------------------------------------------
// Rule 2: ownership
// ---------------------------------------------------------------------------

/// A raw write to a registered table may appear only in that table's declared
/// owner files. Without this the private-writer rule below is worthless: a
/// private writer guards nothing if another module can issue the same SQL
/// behind its back.
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

/// Every declared owner path must resolve. A renamed file would otherwise leave
/// the ownership rule enforcing nothing for that table.
#[test]
fn every_declared_owner_file_exists() {
    let root = src_root();
    let mut missing = Vec::new();
    for rule in TABLES {
        for owner in rule.owners {
            if !root.join(owner).exists() {
                missing.push(format!("{} (owner of table {})", owner, rule.table));
            }
        }
    }
    for rule in DATA_WRITERS {
        if !root.join(rule.owner).exists() {
            missing.push(format!("{} (data writer for {})", rule.owner, rule.writes));
        }
    }
    assert!(
        missing.is_empty(),
        "registry names source files that do not exist:\n  {}",
        missing.join("\n  ")
    );
}

// ---------------------------------------------------------------------------
// Rule 3: a reachable writer of an Announced table must announce
// ---------------------------------------------------------------------------

/// The load-bearing rule, and the one that would have caught the
/// `manage_repositories` bug at the source rather than at the symptom.
///
/// In an owner file of an `Announced` table, a function that writes the table
/// must either be private (unreachable, so it cannot be called without going
/// through something that does announce) or contain an emit call. A deliberate
/// exception is allowed only with a reason recorded on the registry entry.
#[test]
fn every_reachable_writer_of_an_announced_table_announces() {
    let root = src_root();
    let mut violations = Vec::new();

    for rule in TABLES {
        let Announcement::Announced { exempt, .. } = &rule.announcement else {
            continue;
        };
        let exempt_names: BTreeSet<&str> = exempt.iter().map(|e| e.function).collect();

        for owner in rule.owners {
            let text = read_production_source(&root.join(owner));
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

/// An exemption must name a function that exists. A stale one silently widens
/// the hole it was meant to document.
#[test]
fn every_exemption_names_a_function_that_exists() {
    let root = src_root();
    let mut stale = Vec::new();

    for rule in TABLES {
        let Announcement::Announced { exempt, .. } = &rule.announcement else {
            continue;
        };
        for entry in *exempt {
            let found = rule.owners.iter().any(|owner| {
                fn_segments(&read_production_source(&root.join(owner)))
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

// ---------------------------------------------------------------------------
// Rule 4: declared events are real
// ---------------------------------------------------------------------------

/// A registry entry that names a `SystemEvent` variant which no longer exists
/// documents a guarantee nothing provides. Catches a rename that updated the
/// emitters but not the registry.
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

// ---------------------------------------------------------------------------
// Rule 5: the data/ writers
// ---------------------------------------------------------------------------

/// The file-backed half of rule 3. `data/` has no table to key on, so the unit
/// is the module: a reachable function in a declared `data/` writer that
/// touches the filesystem must announce.
///
/// This is what makes `SystemEvent::artifact_change` unnecessary at call sites.
/// That helper exists today only because five callers each had to decide
/// Created-vs-Updated for themselves, and one of them (the image tool) never
/// did.
#[test]
fn every_reachable_data_writer_announces() {
    let root = src_root();
    let mut violations = Vec::new();

    for rule in DATA_WRITERS {
        let Announcement::Announced { exempt, .. } = &rule.announcement else {
            continue;
        };
        let exempt_names: BTreeSet<&str> = exempt.iter().map(|e| e.function).collect();
        let text = read_production_source(&root.join(rule.owner));
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

// ---------------------------------------------------------------------------
// The scanner's own behaviour
// ---------------------------------------------------------------------------

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
    assert!(
        !announces("// TODO: emit CredentialUpdated here"),
        "a comment promising an emit must not satisfy the rule"
    );
}

#[test]
fn tables_written_ignores_prose_and_finds_real_sql() {
    // "update the stored value" and "delete from disk" are prose, not writes.
    assert!(tables_written("/// Update the stored value, then delete from disk.").is_empty());
    assert_eq!(
        tables_written("sqlx::query(\"UPDATE models SET enabled = $2 WHERE id = $1\")"),
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
    let dir = std::env::temp_dir().join(format!("lucidos-scan-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
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
    std::fs::remove_dir_all(&dir).ok();
}

/// The registry is documentation as much as a gate, so every entry has to say
/// something. An empty reason is the same failure as no entry at all.
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
                        problems.push(format!("{label}: exemption {} has no reason", e.function));
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
        // Empty owners is a legal claim ("nothing writes this"), enforced by
        // the ownership scan. It is incoherent for an announced surface though:
        // there would be no write to announce.
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

/// A runtime-created table still has to be classified, and still has to say why
/// it skips the migration path. Without this, the exemption would be a place to
/// hide an unclassified table.
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

/// Keeps the helper honest about the registry it reads.
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

/// Types used only by the scan would otherwise read as dead code to a reader
/// skimming the registry.
#[test]
fn registry_types_are_exercised() {
    let table: &TableRule = &TABLES[0];
    assert!(!table.table.is_empty());
    let data: &DataWriterRule = &DATA_WRITERS[0];
    assert!(!data.writes.is_empty());
}
