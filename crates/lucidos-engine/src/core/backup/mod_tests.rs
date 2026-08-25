use super::*;

/// `PROVIDER_IDS` (the agent-settable `backup_provider` enum) must list exactly
/// the ids in the `PROVIDERS` registry — otherwise the catalog enum drifts from
/// the providers `get_provider` actually accepts.
#[test]
fn provider_ids_match_registry() {
    let registry: Vec<&str> = PROVIDERS.iter().map(|(id, ..)| *id).collect();
    assert_eq!(PROVIDER_IDS, registry.as_slice());
}

/// Every provider must name at least one scope. An empty list means the
/// readiness verdict degenerates to "an account row exists", which is exactly
/// what let a Dropbox account with no file permissions read as ready and fail
/// at `files/create_folder_v2` with a 400 (2026-08-05).
#[test]
fn every_provider_requires_at_least_one_scope() {
    for meta in list_providers() {
        assert!(
            !meta.required_scopes.is_empty(),
            "{} declares no required scopes",
            meta.id
        );
    }
}

/// The registry list IS the preflight list. `provider_readiness` and each
/// provider's own scope check read the same `required_scopes`, so a partially
/// scoped account cannot read as ready on the page and then be rejected by the
/// backup it enabled.
#[test]
fn readiness_requires_every_declared_scope() {
    for meta in list_providers() {
        let full = meta.required_scopes.join(" ");
        assert!(missing_scopes(&full, meta.required_scopes).is_empty());
        // Drop one scope at a time: each must be enough to fail the verdict.
        for dropped in meta.required_scopes {
            let partial = meta
                .required_scopes
                .iter()
                .filter(|s| *s != dropped)
                .copied()
                .collect::<Vec<_>>()
                .join(" ");
            assert_eq!(
                missing_scopes(&partial, meta.required_scopes),
                vec![*dropped],
                "{} tolerated a grant missing {dropped}",
                meta.id
            );
        }
    }
}

/// A requirement reported to a human must be a scope they can find in the
/// provider's own console, not the substring the engine matches on.
#[test]
fn an_unmet_requirement_is_named_as_a_scope_the_user_can_grant() {
    // Drive's whole requirement is the FRAGMENT `drive`, which matches the full
    // URL. Reporting it verbatim names a "drive permission" that exists nowhere.
    assert_eq!(
        name_missing_scopes(vec!["drive"], google_drive::GRANT_SCOPES),
        vec!["https://www.googleapis.com/auth/drive.file"]
    );
    // Dropbox's requirements ARE its scope names, and are exactly what its App
    // Console lists, so the mapping is the identity there.
    assert_eq!(
        name_missing_scopes(
            vec!["files.content.read", "files.metadata.read"],
            dropbox::GRANT_SCOPES
        ),
        vec!["files.content.read", "files.metadata.read"]
    );
    // A matcher no grant scope contains keeps its own name. Dropping it would
    // tell the user a permission is missing without saying which.
    assert_eq!(
        name_missing_scopes(vec!["sharing.write"], dropbox::GRANT_SCOPES),
        vec!["sharing.write"]
    );
    assert!(name_missing_scopes(Vec::new(), dropbox::GRANT_SCOPES).is_empty());
}

/// The two lists must not drift: every requirement has to resolve to something
/// an authorization actually asks for, or the page and the agent would name a
/// permission that no *Grant access* press can obtain.
#[test]
fn every_requirement_resolves_to_a_scope_the_flow_requests() {
    for meta in list_providers() {
        for requirement in meta.required_scopes {
            assert!(
                meta.grant_scopes.iter().any(|g| g.contains(requirement)),
                "{}: nothing in grant_scopes contains the requirement {requirement}",
                meta.id
            );
        }
    }
}

/// `ready` is DERIVED from the missing-scope list, so a verdict cannot claim
/// ready while naming a gap. This pairing has had to be reconciled by hand
/// twice; the third time it is the type's job.
#[test]
fn readiness_is_derived_from_the_missing_scopes_it_carries() {
    let not_connected = ProviderReadiness {
        connected: false,
        missing_scopes: Vec::new(),
    };
    // Nothing connected is not "ready", and it lists no gap either: there is no
    // grant to measure, and naming every scope would describe an absent account
    // as a half-granted one.
    assert!(!not_connected.ready());
    assert!(not_connected.missing_scopes.is_empty());

    assert!(!ProviderReadiness {
        connected: true,
        missing_scopes: vec!["files.metadata.read"],
    }
    .ready());

    assert!(ProviderReadiness {
        connected: true,
        missing_scopes: Vec::new(),
    }
    .ready());
}

/// `backup_run_from_event` parses a persisted `BackupCompleted` event in the
/// real `{ "type": .., "data": { .. } }` shape (the `#[serde(tag, content)]`
/// wire format), recovering start/finish/size for the run history.
#[test]
fn backup_run_from_event_parses_completed_tagged_payload() {
    let payload = serde_json::json!({
        "type": "BackupCompleted",
        "data": {
            "filename": "lucidos-backup-myws-20260626-033846.enc",
            "size_bytes": 8_187_790_566u64,
            "started_at": "2026-06-26T03:00:00Z",
            "finished_at": "2026-06-26T03:53:51Z",
        }
    });
    let created = "2026-06-26T03:53:51Z".parse::<DateTime<Utc>>().unwrap();
    let run = backup_run_from_event("BackupCompleted", &payload, created).expect("a run");
    assert_eq!(run.status, BackupRunStatus::Success);
    assert_eq!(run.size_bytes, Some(8_187_790_566));
    assert_eq!(
        run.filename.as_deref(),
        Some("lucidos-backup-myws-20260626-033846.enc")
    );
    // Duration is finished - started = 53m 51s.
    let dur = run.finished_at - run.started_at.expect("started_at parsed");
    assert_eq!(dur.num_seconds(), 53 * 60 + 51);
}

/// A failure event carries the error; a bare (untagged) data object is tolerated;
/// and a missing `finished_at` falls back to the row's `created` timestamp.
#[test]
fn backup_run_from_event_parses_failed_and_bare_payload() {
    let bare = serde_json::json!({ "error": "pg_dump failed" });
    let created = "2026-06-26T01:00:00Z".parse::<DateTime<Utc>>().unwrap();
    let run = backup_run_from_event("BackupFailed", &bare, created).expect("a run");
    assert_eq!(run.status, BackupRunStatus::Failure);
    assert_eq!(run.error.as_deref(), Some("pg_dump failed"));
    assert!(run.started_at.is_none(), "no started_at in this payload");
    assert_eq!(
        run.finished_at, created,
        "finished_at falls back to created"
    );

    // An unrelated event type is not a backup run.
    assert!(backup_run_from_event("SomethingElse", &bare, created).is_none());
}

#[test]
fn apply_pg_env_errors_on_malformed_url() {
    // pg_env_vars returns empty Vec on parse failure; apply_pg_env must
    // surface that as a loud Err so the subprocess never spawns with
    // zero PG* vars and a misleading libpq error.
    let mut cmd = std::process::Command::new("true");
    assert!(apply_pg_env(&mut cmd, "mysql://x@y/z").is_err());
    assert!(apply_pg_env(&mut cmd, "not a url").is_err());
}

#[test]
fn resolve_pg_tool_path_bare_name_without_bin_dir() {
    // dev/docker: no LUCIDOS_PG_BIN_DIR → bare name for a PATH lookup.
    let p = resolve_pg_tool_path("pg_dump", None).expect("bare name resolves");
    assert_eq!(p, std::path::PathBuf::from("pg_dump"));
}

#[test]
fn resolve_pg_tool_path_absolute_when_bin_dir_present() {
    // packaged: resolves to <bin_dir>/<name> when the binary actually exists.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("pg_dump"), b"").unwrap();
    let p = resolve_pg_tool_path("pg_dump", Some(dir.path())).expect("resolves");
    assert_eq!(p, dir.path().join("pg_dump"));
}

#[test]
fn resolve_pg_tool_path_fails_fast_when_bundled_binary_missing() {
    // packaged but mis-staged: LUCIDOS_PG_BIN_DIR set, binary absent → a
    // descriptive, path-bearing error (not a cryptic ENOENT mid-backup).
    let dir = tempfile::tempdir().unwrap();
    let err = resolve_pg_tool_path("pg_dump", Some(dir.path()))
        .expect_err("missing bundled binary must error");
    let msg = err.to_string();
    assert!(msg.contains("pg_dump"), "names the tool: {msg}");
    assert!(
        msg.contains(dir.path().to_str().unwrap()),
        "names the path: {msg}"
    );
}

/// `is_cross_version_set` strips SET statements for parameters that exist in
/// newer PG versions but not older ones (e.g. `transaction_timeout` from PG 17).
/// Normal SET statements (e.g. `SET statement_timeout`) must pass through.
#[test]
fn is_cross_version_set_filters_correctly() {
    assert!(is_cross_version_set("SET transaction_timeout = 0;"));
    assert!(is_cross_version_set("SET transaction_timeout = '30s';"));
    assert!(is_cross_version_set("  SET transaction_timeout = 0;  "));
    assert!(is_cross_version_set("SET TRANSACTION_TIMEOUT = 0;"));
    assert!(is_cross_version_set("SET transaction_timeout=0;"));

    assert!(!is_cross_version_set("SET statement_timeout = 0;"));
    assert!(!is_cross_version_set("SET search_path = public;"));
    assert!(!is_cross_version_set("SELECT * FROM events;"));
    assert!(!is_cross_version_set("-- SET transaction_timeout = 0;"));
    assert!(!is_cross_version_set(""));
}

/// The restore target db name is pulled from the URL so it can go on the
/// command line; host/port/user/password keep flowing via PG* env (out of
/// argv). A malformed URL is a loud Err, never a silent empty target.
#[test]
fn pg_dbname_extracts_target_db() {
    assert_eq!(
        pg_dbname("postgres://lucidos:lucidos@localhost:5432/lucidos_myws").unwrap(),
        "lucidos_myws"
    );
    assert!(pg_dbname("mysql://x@y/z").is_err());
    assert!(pg_dbname("not a url").is_err());
}

#[test]
fn test_key_file_path() {
    let workspace = Path::new("/home/user/my-workspace");
    assert_eq!(
        key_file_path(workspace),
        PathBuf::from("/home/user/my-workspace/.lucidos/backup.key")
    );
}

#[test]
fn test_tar_and_compress_excludes_lucidos_and_postgres() {
    let workspace = tempfile::tempdir().unwrap();
    let ws = workspace.path();

    // Create .lucidos/ — should be excluded
    std::fs::create_dir_all(ws.join(".lucidos/cache")).unwrap();
    std::fs::write(ws.join(".lucidos/cache/index.dat"), "cache data").unwrap();

    // Create data/postgres/ — should be excluded (live DB, backed up via pg_dump)
    std::fs::create_dir_all(ws.join("data/postgres/global")).unwrap();
    std::fs::write(ws.join("data/postgres/global/pg_filenode.map"), "pgdata").unwrap();

    // Create .git/ — should be included
    std::fs::create_dir_all(ws.join(".git")).unwrap();
    std::fs::write(ws.join(".git/HEAD"), "ref: refs/heads/main").unwrap();

    // Create data/artifacts/ — should be included
    std::fs::create_dir_all(ws.join("data/artifacts")).unwrap();
    std::fs::write(ws.join("data/artifacts/user_profile.md"), "# Profile").unwrap();

    // Create a SQL dump
    let sql_dump = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(sql_dump.path(), "CREATE TABLE test;").unwrap();

    // Tar and compress
    let output = tempfile::NamedTempFile::new().unwrap();
    tar_and_compress(ws, sql_dump.path(), output.path(), &BackupIgnore::default()).unwrap();

    // Decompress and verify contents (streaming from file)
    let file = std::fs::File::open(output.path()).unwrap();
    let decoder = zstd::Decoder::new(file).unwrap();
    let mut archive = tar::Archive::new(decoder);

    let mut found_paths: Vec<String> = Vec::new();
    for entry in archive.entries().unwrap() {
        let entry = entry.unwrap();
        let path = entry.path().unwrap().to_string_lossy().to_string();
        found_paths.push(path);
    }

    found_paths.sort();

    // .lucidos/ should be excluded
    assert!(
        !found_paths.iter().any(|p| p.starts_with(".lucidos")),
        "archive should not contain .lucidos/: {found_paths:?}"
    );

    // data/postgres/ should be excluded (live DB)
    assert!(
        !found_paths.iter().any(|p| p.starts_with("data/postgres")),
        "archive should not contain data/postgres/: {found_paths:?}"
    );

    // .git/ and data/artifacts/ should be included
    assert!(
        found_paths.iter().any(|p| p.starts_with(".git")),
        "archive should contain .git/: {found_paths:?}"
    );
    assert!(
        found_paths.iter().any(|p| p.starts_with("data/")),
        "archive should contain data/: {found_paths:?}"
    );

    // SQL dump should be at root
    assert!(
        found_paths.contains(&"lucidos_backup.dump".to_string()),
        "archive should contain lucidos_backup.dump: {found_paths:?}"
    );
}

/// `data/blobs/` holds content-addressed image bytes. Backups MUST
/// include them — they're the only copy of the bytes referenced by
/// every MessageReceived.user_image_hashes and ThreadComposeChanged.
/// A regression in the exclusion list would orphan every historical
/// image after restore, with no way to recover them.
#[test]
fn test_tar_and_compress_includes_data_blobs() {
    let workspace = tempfile::tempdir().unwrap();
    let ws = workspace.path();

    // Realistic blob path — fan-out by leading two hex chars of the hash.
    let hash = "ab".to_string() + &"c".repeat(62);
    let blob_dir = ws.join("data/blobs").join(&hash[..2]);
    std::fs::create_dir_all(&blob_dir).unwrap();
    let blob_path = blob_dir.join(format!("{hash}.png"));
    std::fs::write(&blob_path, b"\x89PNG fake bytes for test").unwrap();

    let sql_dump = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(sql_dump.path(), "CREATE TABLE test;").unwrap();

    let output = tempfile::NamedTempFile::new().unwrap();
    tar_and_compress(ws, sql_dump.path(), output.path(), &BackupIgnore::default()).unwrap();

    let file = std::fs::File::open(output.path()).unwrap();
    let decoder = zstd::Decoder::new(file).unwrap();
    let mut archive = tar::Archive::new(decoder);
    let found_paths: Vec<String> = archive
        .entries()
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path().unwrap().to_string_lossy().to_string())
        .collect();

    let expected = format!("data/blobs/{}/{}.png", &hash[..2], hash);
    assert!(
        found_paths.contains(&expected),
        "archive must contain {expected}, got: {found_paths:?}"
    );
}

#[test]
fn test_tar_and_compress_excludes_postgres_migrated_archives() {
    // `scripts/lib/workspace.sh` archives the old PGDATA as
    // `data/postgres.migrated-<timestamp>/` when migrating from a host bind
    // mount to a Docker named volume. These can be many GB and must NOT be
    // included in the backup (the live DB is captured via pg_dump).
    let workspace = tempfile::tempdir().unwrap();
    let ws = workspace.path();

    // Live pgdata (already excluded by the existing rule)
    std::fs::create_dir_all(ws.join("data/postgres/global")).unwrap();
    std::fs::write(ws.join("data/postgres/global/pg_filenode.map"), "live").unwrap();

    // Archived pgdata sibling — the regression case
    std::fs::create_dir_all(ws.join("data/postgres.migrated-20260503094805/global")).unwrap();
    std::fs::write(
        ws.join("data/postgres.migrated-20260503094805/global/pg_filenode.map"),
        "stale",
    )
    .unwrap();

    // Real workspace content that MUST still be included
    std::fs::create_dir_all(ws.join("data/artifacts")).unwrap();
    std::fs::write(ws.join("data/artifacts/profile.md"), "# Profile").unwrap();

    let sql_dump = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(sql_dump.path(), "CREATE TABLE test;").unwrap();

    let output = tempfile::NamedTempFile::new().unwrap();
    tar_and_compress(ws, sql_dump.path(), output.path(), &BackupIgnore::default()).unwrap();

    let file = std::fs::File::open(output.path()).unwrap();
    let decoder = zstd::Decoder::new(file).unwrap();
    let mut archive = tar::Archive::new(decoder);
    let mut found_paths: Vec<String> = Vec::new();
    for entry in archive.entries().unwrap() {
        let entry = entry.unwrap();
        found_paths.push(entry.path().unwrap().to_string_lossy().to_string());
    }

    assert!(
        !found_paths
            .iter()
            .any(|p| p.starts_with("data/postgres.migrated-")),
        "archive must not contain data/postgres.migrated-*: {found_paths:?}"
    );
    assert!(
        !found_paths.iter().any(|p| p.starts_with("data/postgres/")),
        "archive must not contain data/postgres/: {found_paths:?}"
    );
    assert!(
        found_paths.iter().any(|p| p == "data/artifacts/profile.md"),
        "archive must contain data/artifacts/profile.md: {found_paths:?}"
    );
}

#[test]
fn test_full_pipeline_encrypt_decrypt() {
    let workspace = tempfile::tempdir().unwrap();
    let ws = workspace.path();

    // Create a file in the workspace
    std::fs::create_dir_all(ws.join("data")).unwrap();
    std::fs::write(ws.join("data/notes.txt"), "important notes").unwrap();

    // Also create .lucidos/ which should be excluded
    std::fs::create_dir_all(ws.join(".lucidos")).unwrap();
    std::fs::write(ws.join(".lucidos/runtime.pid"), "12345").unwrap();

    // Create a mock SQL dump
    let sql_dump = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(sql_dump.path(), "INSERT INTO events VALUES (1);").unwrap();

    // Tar + compress (streaming to file)
    let compressed_path = tempfile::NamedTempFile::new().unwrap();
    tar_and_compress(
        ws,
        sql_dump.path(),
        compressed_path.path(),
        &BackupIgnore::default(),
    )
    .unwrap();

    // Encrypt (streaming file-to-file)
    let key = crypto::generate_key();
    let encrypted_path = tempfile::NamedTempFile::new().unwrap();
    {
        let input = std::fs::File::open(compressed_path.path()).unwrap();
        let mut output = std::fs::File::create(encrypted_path.path()).unwrap();
        crypto::encrypt(&key, input, &mut output).unwrap();
    }

    // Decrypt (streaming file-to-file)
    let decrypted_path = tempfile::NamedTempFile::new().unwrap();
    {
        let input = std::fs::File::open(encrypted_path.path()).unwrap();
        let mut output = std::fs::File::create(decrypted_path.path()).unwrap();
        crypto::decrypt(&key, input, &mut output).unwrap();
    }

    // Decompress + untar (streaming from file)
    let restore_dir = tempfile::tempdir().unwrap();
    {
        let file = std::fs::File::open(decrypted_path.path()).unwrap();
        let decoder = zstd::Decoder::new(file).unwrap();
        let mut archive = tar::Archive::new(decoder);
        archive.unpack(restore_dir.path()).unwrap();
    }

    // Verify restored content matches original
    let restored_notes =
        std::fs::read_to_string(restore_dir.path().join("data/notes.txt")).unwrap();
    assert_eq!(restored_notes, "important notes");

    // Verify SQL dump is present
    let restored_sql =
        std::fs::read_to_string(restore_dir.path().join("lucidos_backup.dump")).unwrap();
    assert_eq!(restored_sql, "INSERT INTO events VALUES (1);");

    // Verify .lucidos/ was NOT included
    assert!(
        !restore_dir.path().join(".lucidos").exists(),
        ".lucidos/ should not be in the restored backup"
    );
}

#[test]
fn test_estimate_weights_small_workspace() {
    // 1 MB workspace — dump dominates (fixed 2s overhead)
    let (dump_end, compress_end, encrypt_end) = estimate_weights(1_048_576);

    assert!(dump_end >= 2, "dump should get at least 2%: {dump_end}");
    assert!(
        compress_end > dump_end,
        "compress_end should exceed dump_end"
    );
    assert!(
        encrypt_end > compress_end,
        "encrypt_end should exceed compress_end"
    );
    assert!(
        encrypt_end <= 97,
        "encrypt_end should leave room for upload: {encrypt_end}"
    );
}

#[test]
fn test_estimate_weights_empty_workspace() {
    let (dump_end, compress_end, encrypt_end) = estimate_weights(0);

    // Each phase gets at least its minimum, and they're strictly increasing
    assert!(dump_end >= 2);
    assert!(compress_end > dump_end);
    assert!(encrypt_end > compress_end);
    assert!(encrypt_end <= 97);
}

/// The uploaded archive is estimated at ≈ a third of the raw workspace (tar+zstd
/// ~3:1). Preflight's free-space check compares Drive's free space against this.
#[test]
fn test_estimate_archive_size_is_about_a_third() {
    assert_eq!(estimate_archive_size(3_000_000), 1_000_000);
    assert_eq!(estimate_archive_size(0), 0);
}

/// The size estimate that feeds preflight must count only files that will be in
/// the tar — the same exclusions the tar walk applies (`.lucidos/`,
/// `data/postgres/`, `.backupignore`). An over-count would falsely reject a
/// backup; an under-count would let an over-quota run through.
#[test]
fn test_workspace_backup_size_sums_included_files_only() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    std::fs::create_dir_all(ws.join("data/artifacts")).unwrap();
    std::fs::write(ws.join("data/artifacts/a.bin"), vec![0u8; 1000]).unwrap();
    // Excluded — must not count toward the estimate.
    std::fs::create_dir_all(ws.join(".lucidos")).unwrap();
    std::fs::write(ws.join(".lucidos/cache.bin"), vec![0u8; 9999]).unwrap();
    std::fs::create_dir_all(ws.join("data/postgres")).unwrap();
    std::fs::write(ws.join("data/postgres/base"), vec![0u8; 5000]).unwrap();

    let size = workspace_backup_size(ws, &BackupIgnore::default());
    assert_eq!(size, 1000, "only data/artifacts/a.bin should count");
}

#[test]
fn test_estimate_restore_weights() {
    // 10 MB encrypted file
    let (decrypt_end, decompress_end) = estimate_restore_weights(10 * 1_048_576, 15);

    assert!(decrypt_end > 15, "decrypt should start after download");
    assert!(
        decompress_end > decrypt_end,
        "decompress should follow decrypt"
    );
    assert!(
        decompress_end <= 97,
        "should leave room for restore: {decompress_end}"
    );
}

#[test]
fn test_estimate_restore_weights_tiny_file() {
    // 100 KB file — restore dominates
    let (decrypt_end, decompress_end) = estimate_restore_weights(100_000, 15);

    assert!(decrypt_end >= 17, "each phase gets at least 2%");
    assert!(decompress_end >= decrypt_end + 2);
}

#[test]
fn test_move_contents() {
    let src = tempfile::tempdir().unwrap();
    let dest = tempfile::tempdir().unwrap();

    // Create structure in src
    std::fs::create_dir_all(src.path().join("data/artifacts")).unwrap();
    std::fs::write(src.path().join("data/artifacts/profile.md"), "# Profile").unwrap();
    std::fs::write(src.path().join("top.txt"), "hello").unwrap();

    // Put existing content in dest that shouldn't be affected
    std::fs::create_dir_all(dest.path().join(".lucidos")).unwrap();
    std::fs::write(dest.path().join(".lucidos/key"), "secret").unwrap();

    move_contents(src.path(), dest.path()).unwrap();

    // Source content should now be in dest
    assert_eq!(
        std::fs::read_to_string(dest.path().join("data/artifacts/profile.md")).unwrap(),
        "# Profile"
    );
    assert_eq!(
        std::fs::read_to_string(dest.path().join("top.txt")).unwrap(),
        "hello"
    );
    // Pre-existing dest content should still be there
    assert_eq!(
        std::fs::read_to_string(dest.path().join(".lucidos/key")).unwrap(),
        "secret"
    );
}

/// Backups no longer include the user-level `~/.lucidos/` (`user_dir/`): restore
/// always discards it, so it was multi-GB of dead weight. The workspace `.git/`
/// IS kept (artifact version history). This pins the trimmed payload contract.
#[test]
fn test_tar_omits_user_dir_keeps_git() {
    let workspace = tempfile::tempdir().unwrap();
    let ws = workspace.path();
    std::fs::create_dir_all(ws.join("data")).unwrap();
    std::fs::write(ws.join("data/notes.txt"), "workspace content").unwrap();
    // Workspace .git/ MUST be kept.
    std::fs::create_dir_all(ws.join(".git/objects")).unwrap();
    std::fs::write(ws.join(".git/HEAD"), "ref: refs/heads/main").unwrap();

    let sql_dump = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(sql_dump.path(), "CREATE TABLE test;").unwrap();

    let output = tempfile::NamedTempFile::new().unwrap();
    tar_and_compress(ws, sql_dump.path(), output.path(), &BackupIgnore::default()).unwrap();

    let file = std::fs::File::open(output.path()).unwrap();
    let decoder = zstd::Decoder::new(file).unwrap();
    let mut archive = tar::Archive::new(decoder);
    let mut found_paths: Vec<String> = Vec::new();
    for entry in archive.entries().unwrap() {
        let entry = entry.unwrap();
        found_paths.push(entry.path().unwrap().to_string_lossy().to_string());
    }

    // No user_dir/ at all.
    assert!(
        !found_paths.iter().any(|p| p.starts_with("user_dir")),
        "archive must not contain user_dir/: {found_paths:?}"
    );
    // Workspace .git/ kept.
    assert!(
        found_paths.iter().any(|p| p.starts_with(".git")),
        "archive must contain the workspace .git/: {found_paths:?}"
    );
    // Workspace content kept.
    assert!(
        found_paths.iter().any(|p| p == "data/notes.txt"),
        "archive should contain data/notes.txt: {found_paths:?}"
    );
}

/// The skip-on-vanish classifier must swallow only `NotFound` (a path removed
/// mid-walk) and propagate every other error kind as a real failure.
#[test]
fn skip_if_vanished_only_swallows_not_found() {
    use std::io::{Error, ErrorKind};

    // NotFound → skip (Ok(None)), the backup continues.
    let r: Result<Option<()>, _> =
        skip_if_vanished(Path::new("/gone"), Err(Error::from(ErrorKind::NotFound)));
    assert!(matches!(r, Ok(None)));

    // Any other kind → propagate as a real error.
    let r: Result<Option<()>, _> = skip_if_vanished(
        Path::new("/denied"),
        Err(Error::from(ErrorKind::PermissionDenied)),
    );
    assert!(r.is_err());

    // Ok → pass the value through unchanged.
    let r = skip_if_vanished(Path::new("/ok"), Ok(42));
    assert!(matches!(r, Ok(Some(42))));
}

/// A file enumerated by the walk can be deleted before tar opens it — a
/// workspace's background jobs constantly tear down worktrees under .git/.
/// `append_file` must skip such a vanished file instead of aborting the backup.
#[test]
fn append_file_skips_vanished_file() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(tmp.path(), "content").unwrap();
    let metadata = std::fs::metadata(tmp.path()).unwrap();
    let path = tmp.path().to_path_buf();
    // File vanishes after metadata was captured but before append reads it.
    drop(tmp);

    let mut buf: Vec<u8> = Vec::new();
    {
        let mut builder = tar::Builder::new(&mut buf);
        // Must NOT error — the vanished file is skipped.
        append_file(&mut builder, &path, Path::new("gone.txt"), &metadata).unwrap();
        builder.finish().unwrap();
    }

    // Archive is valid and contains no entries (the vanished file was skipped).
    let mut archive = tar::Archive::new(&buf[..]);
    assert_eq!(
        archive.entries().unwrap().count(),
        0,
        "vanished file should not appear in the archive"
    );
}

#[test]
fn test_walkdir_recursive() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    std::fs::create_dir_all(root.join("a/b")).unwrap();
    std::fs::write(root.join("top.txt"), "top").unwrap();
    std::fs::write(root.join("a/mid.txt"), "mid").unwrap();
    std::fs::write(root.join("a/b/deep.txt"), "deep").unwrap();

    let files = walkdir(root).unwrap();
    assert_eq!(files.len(), 3);

    let rel_paths: Vec<String> = files
        .iter()
        .map(|p| p.strip_prefix(root).unwrap().to_string_lossy().to_string())
        .collect();

    assert!(rel_paths.contains(&"top.txt".to_string()));
    assert!(rel_paths.contains(&"a/mid.txt".to_string()));
    assert!(rel_paths.contains(&"a/b/deep.txt".to_string()));
}

/// load_last_run returns None when nothing has ever been persisted.
#[tokio::test]
async fn last_run_absent_by_default() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    assert!(load_last_run(&pool).await.is_none());
    crate::test_support::teardown_test_db(&db_name).await;
}

/// A persisted success outcome round-trips through the preference store with
/// all fields intact — this is what survives an engine restart.
#[tokio::test]
async fn last_run_success_round_trips() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;

    let entry = BackupEntry {
        id: "drive-123".to_string(),
        filename: "lucidos-backup-myws-20260530-031500.enc".to_string(),
        size_bytes: 42_000_000,
        created_at: Utc::now(),
    };
    persist_last_run(&pool, &BackupLastRun::success(&entry, Utc::now()))
        .await
        .unwrap();

    let loaded = load_last_run(&pool).await.expect("a persisted run");
    assert_eq!(loaded.status, BackupRunStatus::Success);
    assert_eq!(loaded.filename.as_deref(), Some(entry.filename.as_str()));
    assert_eq!(loaded.size_bytes, Some(entry.size_bytes));
    assert!(loaded.error.is_none());

    crate::test_support::teardown_test_db(&db_name).await;
}

/// A persisted failure outcome carries the error, and overwrites a prior
/// success (it's a single "last run" slot, not an append log).
#[tokio::test]
async fn last_run_failure_overwrites_success() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;

    let entry = BackupEntry {
        id: "drive-1".to_string(),
        filename: "ok.enc".to_string(),
        size_bytes: 1,
        created_at: Utc::now(),
    };
    persist_last_run(&pool, &BackupLastRun::success(&entry, Utc::now()))
        .await
        .unwrap();
    persist_last_run(&pool, &BackupLastRun::failure("pg_dump failed", Utc::now()))
        .await
        .unwrap();

    let loaded = load_last_run(&pool).await.expect("a persisted run");
    assert_eq!(loaded.status, BackupRunStatus::Failure);
    assert_eq!(loaded.error.as_deref(), Some("pg_dump failed"));
    assert!(loaded.started_at.is_some(), "started_at must round-trip");
    assert!(loaded.filename.is_none());

    crate::test_support::teardown_test_db(&db_name).await;
}

// --- .backupignore -------------------------------------------------------

/// A plain directory-prefix pattern excludes that directory AND everything
/// under it (component-prefix match, same semantics as the hardcoded
/// `.lucidos` / `data/postgres` checks). A sibling that merely shares a
/// string prefix is NOT excluded.
#[test]
fn backupignore_prefix_excludes_dir_and_descendants() {
    let ignore = BackupIgnore::parse("data/artifacts/habit-tracker/klines\n");

    assert!(ignore.matches(Path::new("data/artifacts/habit-tracker/klines")));
    assert!(ignore.matches(Path::new(
        "data/artifacts/habit-tracker/klines/2024/jan.parquet"
    )));

    // Component prefix, not string prefix: `klines-backup` is a different
    // component and must be kept.
    assert!(!ignore.matches(Path::new(
        "data/artifacts/habit-tracker/klines-backup/x.bin"
    )));
    assert!(!ignore.matches(Path::new("data/artifacts/habit-tracker/strategy.md")));
}

/// Blank lines and `#` comments are ignored. A blank line, if mis-parsed into
/// an empty prefix, would match every path; a comment, if mis-parsed, would
/// become a literal prefix — both regressions are asserted against.
#[test]
fn backupignore_skips_blanks_and_comments() {
    let ignore = BackupIgnore::parse("# a comment\n\n   \ndata/cache\n# another\n");

    assert_eq!(ignore.len(), 1, "only the one real pattern is parsed");
    assert!(ignore.matches(Path::new("data/cache/x.bin")));
    assert!(
        !ignore.matches(Path::new("data/keep/me.txt")),
        "a blank line must not become a match-everything pattern"
    );
    assert!(
        !ignore.matches(Path::new("# a comment")),
        "a comment line must not become a pattern"
    );
}

/// A trailing slash on a pattern is tolerated — `foo/` behaves like `foo`.
#[test]
fn backupignore_tolerates_trailing_slash() {
    let with_slash = BackupIgnore::parse("data/cache/\n");
    let without_slash = BackupIgnore::parse("data/cache\n");

    assert!(with_slash.matches(Path::new("data/cache")));
    assert!(with_slash.matches(Path::new("data/cache/x.bin")));
    assert!(without_slash.matches(Path::new("data/cache/x.bin")));
}

/// A `*` wildcard pattern is matched with `glob::Pattern` against the path and
/// each of its ancestors, so a matched directory also excludes its subtree.
#[test]
fn backupignore_supports_wildcard() {
    let ignore = BackupIgnore::parse("data/artifacts/*/klines\n");

    assert!(ignore.matches(Path::new("data/artifacts/habit-tracker/klines")));
    assert!(ignore.matches(Path::new(
        "data/artifacts/habit-tracker/klines/2024.parquet"
    )));
    assert!(ignore.matches(Path::new("data/artifacts/reversal/klines/x.bin")));

    // A different leaf under the same wildcard parent is kept.
    assert!(!ignore.matches(Path::new("data/artifacts/habit-tracker/signals/x.bin")));
}

/// A malformed glob (e.g. an unclosed character class) is logged and skipped,
/// never aborting the parse — the other valid patterns still apply.
#[test]
fn backupignore_skips_malformed_glob() {
    let ignore = BackupIgnore::parse("data/cache\ndata/[invalid\n");

    assert_eq!(ignore.len(), 1, "the malformed line is dropped");
    assert!(ignore.matches(Path::new("data/cache/x")));
}

/// A missing `.backupignore` yields no patterns, so only the hardcoded
/// exclusions apply — exactly today's behavior.
#[test]
fn backupignore_missing_file_means_only_hardcoded() {
    let dir = tempfile::tempdir().unwrap();
    let ignore = BackupIgnore::load(dir.path());

    assert!(ignore.is_empty());
    // Hardcoded exclusions still fire.
    assert!(is_excluded_workspace_path(
        Path::new(".lucidos/cache/x"),
        &ignore
    ));
    assert!(is_excluded_workspace_path(
        Path::new("data/postgres/global/pg_filenode.map"),
        &ignore
    ));
    // Ordinary content is kept.
    assert!(!is_excluded_workspace_path(
        Path::new("data/artifacts/profile.md"),
        &ignore
    ));
}

/// Permission grants are deliberately NOT backed up (ADR 0095). They live in
/// `.lucidos/`, so a restore lands a workspace with no grants and the user is
/// asked once. That is fail-closed and correct: restoring onto another machine
/// must not silently reinstate a yes.
#[test]
fn permission_grants_are_excluded_from_the_backup() {
    let dir = tempfile::tempdir().unwrap();
    let ignore = BackupIgnore::load(dir.path());
    for file in crate::core::GrantFile::ALL {
        let rel = std::path::PathBuf::from(".lucidos").join(file.file_name());
        assert!(
            is_excluded_workspace_path(&rel, &ignore),
            "{} must not travel in a backup archive",
            rel.display()
        );
    }
}

/// A loaded `.backupignore` excludes its patterns IN ADDITION to the hardcoded
/// `.lucidos` / `data/postgres` exclusions — neither overrides the other.
#[test]
fn backupignore_combines_with_hardcoded_exclusions() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("data")).unwrap();
    std::fs::write(
        dir.path().join("data/.backupignore"),
        "data/artifacts/cache\n",
    )
    .unwrap();
    let ignore = BackupIgnore::load(dir.path());

    // Hardcoded still excluded.
    assert!(is_excluded_workspace_path(
        Path::new(".lucidos/runtime.pid"),
        &ignore
    ));
    assert!(is_excluded_workspace_path(
        Path::new("data/postgres/base/1"),
        &ignore
    ));
    assert!(is_excluded_workspace_path(
        Path::new("data/postgres.migrated-20260503094805/global/x"),
        &ignore
    ));
    // Ignore-file pattern excluded.
    assert!(is_excluded_workspace_path(
        Path::new("data/artifacts/cache/big.bin"),
        &ignore
    ));
    // Unrelated content kept.
    assert!(!is_excluded_workspace_path(
        Path::new("data/artifacts/profile.md"),
        &ignore
    ));
}

/// End-to-end: the tar walk honors a real `.backupignore` on disk — the
/// excluded subtree is dropped, real content is kept, and the `.backupignore`
/// file itself IS included (it documents what was excluded).
#[test]
fn tar_and_compress_honors_backupignore() {
    let workspace = tempfile::tempdir().unwrap();
    let ws = workspace.path();

    std::fs::create_dir_all(ws.join("data")).unwrap();
    std::fs::write(
        ws.join("data/.backupignore"),
        "# big re-downloadable cache\ndata/artifacts/habit-tracker/klines/\n",
    )
    .unwrap();

    // Excluded subtree.
    std::fs::create_dir_all(ws.join("data/artifacts/habit-tracker/klines/2024")).unwrap();
    std::fs::write(
        ws.join("data/artifacts/habit-tracker/klines/2024/jan.parquet"),
        "big bytes",
    )
    .unwrap();

    // Kept content alongside it.
    std::fs::write(
        ws.join("data/artifacts/habit-tracker/strategy.md"),
        "# Strategy",
    )
    .unwrap();

    let ignore = BackupIgnore::load(ws);
    let sql_dump = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(sql_dump.path(), "CREATE TABLE test;").unwrap();
    let output = tempfile::NamedTempFile::new().unwrap();
    tar_and_compress(ws, sql_dump.path(), output.path(), &ignore).unwrap();

    let file = std::fs::File::open(output.path()).unwrap();
    let decoder = zstd::Decoder::new(file).unwrap();
    let mut archive = tar::Archive::new(decoder);
    let found: Vec<String> = archive
        .entries()
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path().unwrap().to_string_lossy().to_string())
        .collect();

    assert!(
        !found
            .iter()
            .any(|p| p.starts_with("data/artifacts/habit-tracker/klines")),
        "the ignored klines subtree must be excluded: {found:?}"
    );
    assert!(
        found
            .iter()
            .any(|p| p == "data/artifacts/habit-tracker/strategy.md"),
        "sibling content must be kept: {found:?}"
    );
    assert!(
        found.iter().any(|p| p == "data/.backupignore"),
        ".backupignore itself must be included: {found:?}"
    );
}

/// The persisted JSON uses the documented lowercase `status` discriminator and
/// RFC-3339 `at` — the frontend contract depends on this exact shape.
#[tokio::test]
async fn last_run_persists_documented_json_shape() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;

    persist_last_run(&pool, &BackupLastRun::failure("boom", Utc::now()))
        .await
        .unwrap();

    let raw = crate::core::PreferenceStore::get(&pool, PREF_BACKUP_LAST_RUN)
        .await
        .unwrap()
        .expect("a stored value");
    let json: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(json["status"], "failure");
    assert_eq!(json["error"], "boom");
    assert!(
        json["at"].as_str().unwrap().contains('T'),
        "RFC-3339 timestamp"
    );
    assert!(
        json["started_at"].as_str().unwrap().contains('T'),
        "started_at RFC-3339 timestamp"
    );

    crate::test_support::teardown_test_db(&db_name).await;
}

// --- prune / retention ---------------------------------------------------

/// A `BackupProvider` test double for the prune path: serves a fixed
/// `list_backups` result and records every `delete`. `list_fails` makes the
/// listing itself error; `fail_delete_ids` makes specific deletes error. Only
/// `list_backups` and `delete` are exercised by pruning — the rest panic if
/// reached, which would signal the prune path drifted into unexpected calls.
struct MockProvider {
    entries: Vec<BackupEntry>,
    list_fails: bool,
    fail_delete_ids: std::collections::HashSet<String>,
    deleted: std::sync::Mutex<Vec<String>>,
}

impl MockProvider {
    fn new(entries: Vec<BackupEntry>) -> Self {
        Self {
            entries,
            list_fails: false,
            fail_delete_ids: std::collections::HashSet::new(),
            deleted: std::sync::Mutex::new(Vec::new()),
        }
    }

    fn failing_list(mut self) -> Self {
        self.list_fails = true;
        self
    }

    fn failing_delete_on(mut self, ids: &[&str]) -> Self {
        self.fail_delete_ids = ids.iter().map(|s| s.to_string()).collect();
        self
    }

    /// Ids passed to `delete`, in call order.
    fn deleted_ids(&self) -> Vec<String> {
        self.deleted.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl BackupProvider for MockProvider {
    fn name(&self) -> &str {
        "mock"
    }
    fn id(&self) -> &str {
        "mock"
    }
    fn oauth_provider(&self) -> &str {
        "mock"
    }
    async fn folder_url(&self) -> Option<String> {
        None
    }
    async fn preflight(&self, _estimated_upload_bytes: u64) -> Result<(), BoxError> {
        Ok(())
    }
    async fn upload(
        &self,
        _file_path: &Path,
        _filename: &str,
        _progress: &(dyn Fn(u64, u64) + Send + Sync),
    ) -> Result<String, BoxError> {
        unreachable!("upload is not exercised by prune tests")
    }
    async fn list_backups(&self) -> Result<Vec<BackupEntry>, BoxError> {
        if self.list_fails {
            return Err("simulated list_backups failure".into());
        }
        Ok(self.entries.clone())
    }
    async fn download(
        &self,
        _backup_id: &str,
        _dest: &Path,
        _progress: &(dyn Fn(u64, u64) + Send + Sync),
    ) -> Result<(), BoxError> {
        unreachable!("download is not exercised by prune tests")
    }
    async fn delete(&self, backup_id: &str) -> Result<(), BoxError> {
        self.deleted.lock().unwrap().push(backup_id.to_string());
        if self.fail_delete_ids.contains(backup_id) {
            Err(format!("simulated delete failure for {backup_id}").into())
        } else {
            Ok(())
        }
    }
}

/// Build a backup entry `age_secs` old. The older the file, the larger the age.
fn archive(id: &str, name: &str, age_secs: i64) -> BackupEntry {
    BackupEntry {
        id: id.to_string(),
        filename: name.to_string(),
        size_bytes: 1024,
        created_at: Utc::now() - chrono::Duration::seconds(age_secs),
    }
}

/// Keep the newest `keep` of THIS workspace's archives and return the rest
/// oldest-first. Foreign-workspace archives and unrelated files are invisible
/// to the selection — the safety property: pruning `myws` must never pick
/// `otherws`'s backups or a stray file in the shared folder. (This assertion
/// fails under the old global newest-N logic, which would delete `w1`/`x1`.)
#[test]
fn select_prunable_keeps_newest_n_of_own_workspace_only() {
    let entries = vec![
        archive("p1", "lucidos-backup-myws-20260601-000001.enc", 100), // oldest own
        archive("p2", "lucidos-backup-myws-20260601-000002.enc", 80),
        archive("p3", "lucidos-backup-myws-20260601-000003.enc", 60),
        archive("p4", "lucidos-backup-myws-20260601-000004.enc", 40), // newest own
        archive("w1", "lucidos-backup-otherws-20260601-000004.enc", 5), // foreign workspace
        archive("x1", "important-tax-document.pdf", 1),               // unrelated file
    ];

    let to_delete: Vec<String> = select_prunable(entries, "myws", 2)
        .iter()
        .map(|e| e.id.clone())
        .collect();

    // Newest two own (p4, p3) kept → delete p2 then p1, oldest-first.
    assert_eq!(to_delete, vec!["p1".to_string(), "p2".to_string()]);
    assert!(
        !to_delete.contains(&"w1".to_string()),
        "never a foreign backup"
    );
    assert!(
        !to_delete.contains(&"x1".to_string()),
        "never an unrelated file"
    );
}

/// With fewer own archives than the retention limit, nothing is pruned — even
/// when foreign/unrelated files push the FOLDER total above `keep`. The old
/// logic compared the folder total to `keep` and would delete here.
#[test]
fn select_prunable_keeps_all_when_own_count_within_limit() {
    let entries = vec![
        archive("p1", "lucidos-backup-myws-20260601-000001.enc", 50),
        archive("p2", "lucidos-backup-myws-20260601-000002.enc", 40),
        archive("w1", "lucidos-backup-otherws-20260601-000001.enc", 30),
        archive("w2", "lucidos-backup-otherws-20260601-000002.enc", 20),
        archive("w3", "lucidos-backup-otherws-20260601-000003.enc", 10),
    ];

    // 2 own, keep 5 → nothing to delete, despite 5 files in the folder.
    assert!(select_prunable(entries, "myws", 5).is_empty());
}

/// A workspace whose name is a prefix of another's must not match the other's
/// archives. The timestamp-shape check after the prefix is what enforces it.
#[test]
fn is_own_backup_archive_rejects_prefix_collisions() {
    assert!(is_own_backup_archive(
        "lucidos-backup-myws-20260601-040254.enc",
        "myws"
    ));
    // Belongs to `myws-2`, not `myws`.
    assert!(!is_own_backup_archive(
        "lucidos-backup-myws-2-20260601-040254.enc",
        "myws"
    ));
    assert!(is_own_backup_archive(
        "lucidos-backup-myws-2-20260601-040254.enc",
        "myws-2"
    ));
    // A hyphenated workspace name round-trips...
    assert!(is_own_backup_archive(
        "lucidos-backup-e2e-test-20260601-040254.enc",
        "e2e-test"
    ));
    // ...and `e2e-test`'s file is not `e2e`'s.
    assert!(!is_own_backup_archive(
        "lucidos-backup-e2e-test-20260601-040254.enc",
        "e2e"
    ));
}

/// Files in the folder that aren't archive-shaped are never pruned: a wrong
/// tail, a wrong extension, a missing date/time split, or a non-Lucidos name
/// all fail to match.
#[test]
fn is_own_backup_archive_requires_timestamp_and_enc_suffix() {
    assert!(!is_own_backup_archive(
        "lucidos-backup-myws-notes.enc",
        "myws"
    ));
    assert!(!is_own_backup_archive(
        "lucidos-backup-myws-20260601-040254.zip",
        "myws"
    ));
    assert!(!is_own_backup_archive(
        "lucidos-backup-myws-20260601.enc",
        "myws"
    ));
    assert!(!is_own_backup_archive("random.txt", "myws"));
}

/// End-to-end through the provider double: prune deletes exactly this
/// workspace's oldest archives beyond the limit, oldest-first, and issues no
/// delete for a foreign backup or an unrelated file in the same folder.
#[tokio::test]
async fn prune_old_backups_deletes_only_own_excess_oldest_first() {
    let entries = vec![
        archive("p_old", "lucidos-backup-myws-20260601-000001.enc", 100),
        archive("p_mid", "lucidos-backup-myws-20260601-000002.enc", 50),
        archive("p_new", "lucidos-backup-myws-20260601-000003.enc", 10),
        archive("otherws", "lucidos-backup-otherws-20260601-000003.enc", 1),
        archive("doc", "tax.pdf", 1),
    ];
    let provider = MockProvider::new(entries);

    let deleted = prune_old_backups(&provider, "myws", 1).await.unwrap();

    assert_eq!(deleted, 2, "two own archives beyond keep=1");
    assert_eq!(
        provider.deleted_ids(),
        vec!["p_old".to_string(), "p_mid".to_string()],
        "only own archives, oldest-first"
    );
}

/// A failing `delete` mid-prune is logged and skipped — prune still returns Ok
/// with the count of the deletes that DID succeed, so one stuck archive never
/// aborts cleanup (nor fails the backup, which had already succeeded). This is
/// the "prune error is swallowed" property at the provider granularity.
#[tokio::test]
async fn prune_old_backups_swallows_individual_delete_failures() {
    let entries = vec![
        archive("p_old", "lucidos-backup-myws-20260601-000001.enc", 100),
        archive("p_mid", "lucidos-backup-myws-20260601-000002.enc", 50),
        archive("p_new", "lucidos-backup-myws-20260601-000003.enc", 10),
    ];
    let provider = MockProvider::new(entries).failing_delete_on(&["p_old"]);

    // keep=1 → attempt p_old (fails) then p_mid (ok).
    let deleted = prune_old_backups(&provider, "myws", 1).await.unwrap();

    assert_eq!(deleted, 1, "only the successful delete is counted");
    assert_eq!(
        provider.deleted_ids(),
        vec!["p_old".to_string(), "p_mid".to_string()],
        "both deletes attempted despite the first failing"
    );
}

/// When the provider can't even list backups, prune surfaces the error to its
/// caller. `run_backup` (in `scheduler::backup`) is the layer that
/// logs-and-continues — the backup has already succeeded by then, so a prune
/// error is non-fatal there.
#[tokio::test]
async fn prune_old_backups_propagates_list_failure() {
    let provider = MockProvider::new(vec![]).failing_list();
    assert!(prune_old_backups(&provider, "myws", 5).await.is_err());
}

/// A retention of 0 is a guard, not "delete everything" — prune no-ops without
/// even listing.
#[tokio::test]
async fn prune_old_backups_keep_zero_is_a_noop() {
    let entries = vec![
        archive("p1", "lucidos-backup-myws-20260601-000001.enc", 100),
        archive("p2", "lucidos-backup-myws-20260601-000002.enc", 10),
    ];
    let provider = MockProvider::new(entries);

    assert_eq!(prune_old_backups(&provider, "myws", 0).await.unwrap(), 0);
    assert!(provider.deleted_ids().is_empty());
}

/// The archive workspace name is the path's final component, matching the
/// `{name}` segment baked into the upload filename — so the prune matcher and
/// the upload always agree on what "own" means.
#[test]
fn workspace_archive_name_uses_final_component() {
    assert_eq!(
        workspace_archive_name(Path::new("/home/u/workspaces/myws")),
        "myws"
    );
    assert_eq!(
        workspace_archive_name(Path::new("/home/u/workspaces/e2e-test")),
        "e2e-test"
    );
    // No final component → the same fallback the upload uses.
    assert_eq!(workspace_archive_name(Path::new("/")), "workspace");
}

/// Retention comes from the `backup_retention` preference when set — the
/// user's configured value (10) wins over the default.
#[tokio::test]
async fn get_retention_count_reads_preference() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;

    crate::test_support::seed_preference(&pool, PREF_BACKUP_RETENTION, "10")
        .await
        .unwrap();
    assert_eq!(get_retention_count(&pool).await.unwrap(), 10);

    crate::test_support::teardown_test_db(&db_name).await;
}

/// With no preference (or an unparseable one), retention falls back to
/// DEFAULT_BACKUP_RETENTION — never silently 0, which would mean "delete
/// everything".
#[tokio::test]
async fn get_retention_count_falls_back_to_default() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;

    // Absent → default.
    assert_eq!(
        get_retention_count(&pool).await.unwrap(),
        DEFAULT_BACKUP_RETENTION
    );

    // Unparseable → default, not 0.
    crate::test_support::seed_preference(&pool, PREF_BACKUP_RETENTION, "not-a-number")
        .await
        .unwrap();
    assert_eq!(
        get_retention_count(&pool).await.unwrap(),
        DEFAULT_BACKUP_RETENTION
    );

    crate::test_support::teardown_test_db(&db_name).await;
}

// ── restore (gateway picker path) ──────────────────────────────────────────

/// The default workspace name for a picker restore is parsed back out of the
/// archive filename `create_backup` stamped. A hyphenated workspace name must
/// round-trip, and a non-archive filename yields `None` so the picker asks.
#[test]
fn parse_workspace_name_from_archive_roundtrips_and_rejects_non_archives() {
    assert_eq!(
        parse_workspace_name_from_archive("lucidos-backup-myws-20260601-040254.enc").as_deref(),
        Some("myws")
    );
    // Hyphenated workspace name — the timestamp tail is stripped, not the name.
    assert_eq!(
        parse_workspace_name_from_archive("lucidos-backup-e2e-test-20260601-040254.enc").as_deref(),
        Some("e2e-test")
    );
    // This is `parse`/`workspace_archive_name`'s inverse: a name produced by the
    // upload path comes back unchanged.
    let fname = format!(
        "lucidos-backup-{}-20260601-040254.enc",
        workspace_archive_name(Path::new("/home/u/workspaces/my-ws"))
    );
    assert_eq!(
        parse_workspace_name_from_archive(&fname).as_deref(),
        Some("my-ws")
    );

    // Not archive-shaped → None (the picker then requires a typed name).
    assert!(parse_workspace_name_from_archive("random.txt").is_none());
    assert!(parse_workspace_name_from_archive("lucidos-backup-myws.enc").is_none());
    assert!(
        parse_workspace_name_from_archive("lucidos-backup-myws-20260601.enc").is_none(),
        "a missing HHMMSS half is not a valid timestamp tail"
    );
    assert!(
        parse_workspace_name_from_archive("lucidos-backup-20260601-040254.enc").is_none(),
        "no name segment before the timestamp"
    );
    assert!(parse_workspace_name_from_archive("other-backup-myws-20260601-040254.enc").is_none());
}

/// `restore_archive_into` must restore the workspace files but MUST NOT apply the
/// archive's `user_dir/` (the source machine's `~/.lucidos`) — doing so would
/// clobber the target's gateway registry. With no `lucidos_backup.dump` in the
/// archive the DB phase is skipped, so this exercises the decrypt/decompress/
/// move/user_dir-drop logic with zero Postgres dependency.
#[tokio::test]
async fn restore_archive_into_restores_workspace_but_drops_user_dir() {
    // Build a source "workspace" + a user_dir, archive + encrypt it.
    let src = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(src.path().join("data/artifacts")).unwrap();
    std::fs::write(src.path().join("data/artifacts/profile.md"), "# Profile").unwrap();

    let user_tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(user_tmp.path().join("gateway/config")).unwrap();
    std::fs::write(
        user_tmp.path().join("gateway/config/workspaces.json"),
        r#"{"workspaces":[{"id":"SOURCE-MACHINE"}]}"#,
    )
    .unwrap();

    // tar+compress with the user_dir, but NO sql dump (skip the DB phase). A
    // throwaway empty dump file is required by tar_and_compress's signature, so
    // give it one and then strip it from the archive by restoring into a dir and
    // checking — simpler: pass an empty dump; restore skips DB work only when the
    // archive has no `lucidos_backup.dump`. So craft the archive WITHOUT a dump
    // by tarring manually is overkill; instead point at a non-existent dump is
    // not allowed. Use a real (empty) dump file but assert DB phase is a no-op by
    // using a bogus URL that would error if reached.
    let compressed = tempfile::NamedTempFile::new().unwrap();
    {
        // Build the tar WITHOUT the dump entry: reuse tar_and_compress but it
        // always appends the dump at root. To get a dump-less archive, build it
        // by hand here.
        let file = std::fs::File::create(compressed.path()).unwrap();
        let encoder = zstd::Encoder::new(file, 3).unwrap();
        let mut builder = tar::Builder::new(encoder);
        builder
            .append_dir_all("data", src.path().join("data"))
            .unwrap();
        builder.append_dir_all("user_dir", user_tmp.path()).unwrap();
        let encoder = builder.into_inner().unwrap();
        encoder.finish().unwrap();
    }

    let key = crypto::generate_key();
    let encrypted = tempfile::NamedTempFile::new().unwrap();
    {
        let input = std::fs::File::open(compressed.path()).unwrap();
        let mut output = std::fs::File::create(encrypted.path()).unwrap();
        crypto::encrypt(&key, input, &mut output).unwrap();
    }

    // Restore into a fresh workspace dir. A bogus DB URL would error if the DB
    // phase ran — but with no dump in the archive it must be skipped.
    let ws = tempfile::tempdir().unwrap();
    let ws_dir = ws.path().join("restored");
    restore_archive_into(
        &ws_dir,
        "postgres://unused@nowhere/none-should-not-be-touched",
        &key,
        encrypted.path(),
        |_phase, _cur, _total| {},
    )
    .await
    .expect("restore should succeed without a dump");

    // Workspace files restored.
    assert_eq!(
        std::fs::read_to_string(ws_dir.join("data/artifacts/profile.md")).unwrap(),
        "# Profile"
    );
    // user_dir/ MUST NOT be present in the restored workspace (Invariant 1: it is
    // dropped, never applied to ~/.lucidos nor littered into the workspace).
    assert!(
        !ws_dir.join("user_dir").exists(),
        "the archive's user_dir/ must be dropped, not restored"
    );
}
