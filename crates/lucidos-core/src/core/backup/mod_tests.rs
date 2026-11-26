use super::*;

#[test]
fn provider_ids_match_registry() {
    let registry: Vec<&str> = PROVIDERS.iter().map(|(id, ..)| *id).collect();
    assert_eq!(PROVIDER_IDS, registry.as_slice());
}

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

#[test]
fn readiness_requires_every_declared_scope() {
    for meta in list_providers() {
        let full = meta.required_scopes.join(" ");
        assert!(missing_scopes(&full, meta.required_scopes).is_empty());
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

#[test]
fn an_unmet_requirement_is_named_as_a_scope_the_user_can_grant() {
    assert_eq!(
        name_missing_scopes(vec!["drive"], google_drive::GRANT_SCOPES),
        vec!["https://www.googleapis.com/auth/drive.file"]
    );
    assert_eq!(
        name_missing_scopes(
            vec!["files.content.read", "files.metadata.read"],
            dropbox::GRANT_SCOPES
        ),
        vec!["files.content.read", "files.metadata.read"]
    );
    assert_eq!(
        name_missing_scopes(vec!["sharing.write"], dropbox::GRANT_SCOPES),
        vec!["sharing.write"]
    );
    assert!(name_missing_scopes(Vec::new(), dropbox::GRANT_SCOPES).is_empty());
}

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

#[test]
fn readiness_is_derived_from_the_missing_scopes_it_carries() {
    let not_connected = ProviderReadiness {
        connected: false,
        missing_scopes: Vec::new(),
    };
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
    let dur = run.finished_at - run.started_at.expect("started_at parsed");
    assert_eq!(dur.num_seconds(), 53 * 60 + 51);
}

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

    assert!(backup_run_from_event("SomethingElse", &bare, created).is_none());
}

#[test]
fn apply_pg_env_errors_on_malformed_url() {
    let mut cmd = std::process::Command::new("true");
    assert!(apply_pg_env(&mut cmd, "mysql://x@y/z").is_err());
    assert!(apply_pg_env(&mut cmd, "not a url").is_err());
}

#[test]
fn resolve_pg_tool_path_bare_name_without_bin_dir() {
    let p = resolve_pg_tool_path("pg_dump", None).expect("bare name resolves");
    assert_eq!(p, std::path::PathBuf::from("pg_dump"));
}

#[test]
fn resolve_pg_tool_path_absolute_when_bin_dir_present() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("pg_dump"), b"").unwrap();
    let p = resolve_pg_tool_path("pg_dump", Some(dir.path())).expect("resolves");
    assert_eq!(p, dir.path().join("pg_dump"));
}

#[test]
fn resolve_pg_tool_path_fails_fast_when_bundled_binary_missing() {
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

#[cfg(unix)]
#[test]
fn walkdir_does_not_follow_symlinks_out_of_the_workspace() {
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("id_rsa"), b"PRIVATE KEY").unwrap();

    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("real.txt"), b"mine").unwrap();
    std::os::unix::fs::symlink(outside.path(), workspace.path().join("linked-dir")).unwrap();
    std::os::unix::fs::symlink(
        outside.path().join("id_rsa"),
        workspace.path().join("linked-file"),
    )
    .unwrap();

    let found = walkdir(workspace.path()).expect("walk succeeds");

    assert!(
        found.contains(&workspace.path().join("real.txt")),
        "the workspace's own file is still collected: {found:?}"
    );
    assert!(
        !found.iter().any(|p| p.ends_with("id_rsa")),
        "a symlinked directory must not be descended into: {found:?}"
    );
    assert!(
        !found.iter().any(|p| p.ends_with("linked-file")),
        "a symlinked file must not be collected: {found:?}"
    );
}

#[cfg(unix)]
#[test]
fn walkdir_follows_a_relocated_data_dir() {
    let elsewhere = tempfile::tempdir().unwrap();
    std::fs::create_dir(elsewhere.path().join("artifacts")).unwrap();
    std::fs::write(elsewhere.path().join("artifacts/report.md"), b"# hi").unwrap();

    let workspace = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(
        elsewhere.path(),
        workspace.path().join(crate::core::DATA_DIR),
    )
    .unwrap();

    let found = walkdir(workspace.path()).expect("walk succeeds");

    let expected = workspace
        .path()
        .join(crate::core::DATA_DIR)
        .join("artifacts/report.md");
    assert!(
        found.contains(&expected),
        "a relocated data/ must still be backed up, at its workspace-relative path: {found:?}"
    );
}

#[cfg(unix)]
#[test]
fn walkdir_refuses_a_data_symlink_that_contains_the_workspace() {
    let outer = tempfile::tempdir().unwrap();
    let workspace = outer.path().join("ws");
    std::fs::create_dir(&workspace).unwrap();
    std::fs::write(outer.path().join("sibling-secret.txt"), b"not mine").unwrap();
    std::os::unix::fs::symlink(outer.path(), workspace.join(crate::core::DATA_DIR)).unwrap();

    let err = walkdir(&workspace).expect_err("a data link onto a parent must fail the backup");
    assert!(
        err.to_string().contains("contains the workspace"),
        "the error says why: {err}"
    );
}

#[cfg(unix)]
#[test]
fn walkdir_fails_when_a_relocated_data_dir_does_not_resolve() {
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("root.txt"), b"still here").unwrap();
    std::os::unix::fs::symlink(
        workspace.path().join("no-such-volume"),
        workspace.path().join(crate::core::DATA_DIR),
    )
    .unwrap();

    let err = walkdir(workspace.path()).expect_err("an unresolvable data link must fail");
    let msg = err.to_string();
    assert!(
        msg.contains("does not resolve"),
        "the error says why: {msg}"
    );
    assert!(
        msg.contains(crate::core::DATA_DIR),
        "the error names the path: {msg}"
    );
}

#[cfg(unix)]
#[test]
fn walkdir_terminates_on_a_symlink_loop() {
    let workspace = tempfile::tempdir().unwrap();
    let nested = workspace.path().join("nested");
    std::fs::create_dir(&nested).unwrap();
    std::os::unix::fs::symlink(workspace.path(), nested.join("loop")).unwrap();

    let found = walkdir(workspace.path()).expect("walk succeeds");
    assert!(found.is_empty(), "no regular files were created: {found:?}");
}

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
fn a_failed_restore_statement_makes_psql_exit_non_zero() {
    let args = psql_restore_args("lucidos_myws");
    let stop = args
        .windows(2)
        .any(|pair| pair == ["--set", "ON_ERROR_STOP=1"]);
    assert!(stop, "{args:?}");
    assert!(args.contains(&"--single-transaction"), "{args:?}");
    assert_eq!(args[args.len() - 2..], ["--dbname", "lucidos_myws"]);
}

#[test]
fn test_tar_and_compress_excludes_lucidos_and_postgres() {
    let workspace = tempfile::tempdir().unwrap();
    let ws = workspace.path();

    std::fs::create_dir_all(ws.join(".lucidos/cache")).unwrap();
    std::fs::write(ws.join(".lucidos/cache/index.dat"), "cache data").unwrap();

    std::fs::create_dir_all(ws.join("data/postgres/global")).unwrap();
    std::fs::write(ws.join("data/postgres/global/pg_filenode.map"), "pgdata").unwrap();

    std::fs::create_dir_all(ws.join(".git")).unwrap();
    std::fs::write(ws.join(".git/HEAD"), "ref: refs/heads/main").unwrap();

    std::fs::create_dir_all(ws.join("data/artifacts")).unwrap();
    std::fs::write(ws.join("data/artifacts/user_profile.md"), "# Profile").unwrap();

    let sql_dump = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(sql_dump.path(), "CREATE TABLE test;").unwrap();

    let output = tempfile::NamedTempFile::new().unwrap();
    tar_and_compress(
        ws,
        sql_dump.path(),
        secret_key_file().path(),
        None,
        output.path(),
        &BackupIgnore::default(),
    )
    .unwrap();

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

    assert!(
        !found_paths.iter().any(|p| p.starts_with(".lucidos")),
        "archive should not contain .lucidos/: {found_paths:?}"
    );

    assert!(
        !found_paths.iter().any(|p| p.starts_with("data/postgres")),
        "archive should not contain data/postgres/: {found_paths:?}"
    );

    assert!(
        found_paths.iter().any(|p| p.starts_with(".git")),
        "archive should contain .git/: {found_paths:?}"
    );
    assert!(
        found_paths.iter().any(|p| p.starts_with("data/")),
        "archive should contain data/: {found_paths:?}"
    );

    assert!(
        found_paths.contains(&"lucidos_backup.dump".to_string()),
        "archive should contain lucidos_backup.dump: {found_paths:?}"
    );
}

#[test]
fn test_tar_and_compress_includes_data_blobs() {
    let workspace = tempfile::tempdir().unwrap();
    let ws = workspace.path();

    let hash = "ab".to_string() + &"c".repeat(62);
    let blob_dir = ws.join("data/blobs").join(&hash[..2]);
    std::fs::create_dir_all(&blob_dir).unwrap();
    let blob_path = blob_dir.join(format!("{hash}.png"));
    std::fs::write(&blob_path, b"\x89PNG fake bytes for test").unwrap();

    let sql_dump = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(sql_dump.path(), "CREATE TABLE test;").unwrap();

    let output = tempfile::NamedTempFile::new().unwrap();
    tar_and_compress(
        ws,
        sql_dump.path(),
        secret_key_file().path(),
        None,
        output.path(),
        &BackupIgnore::default(),
    )
    .unwrap();

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
    let workspace = tempfile::tempdir().unwrap();
    let ws = workspace.path();

    std::fs::create_dir_all(ws.join("data/postgres/global")).unwrap();
    std::fs::write(ws.join("data/postgres/global/pg_filenode.map"), "live").unwrap();

    std::fs::create_dir_all(ws.join("data/postgres.migrated-20260503094805/global")).unwrap();
    std::fs::write(
        ws.join("data/postgres.migrated-20260503094805/global/pg_filenode.map"),
        "stale",
    )
    .unwrap();

    std::fs::create_dir_all(ws.join("data/artifacts")).unwrap();
    std::fs::write(ws.join("data/artifacts/profile.md"), "# Profile").unwrap();

    let sql_dump = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(sql_dump.path(), "CREATE TABLE test;").unwrap();

    let output = tempfile::NamedTempFile::new().unwrap();
    tar_and_compress(
        ws,
        sql_dump.path(),
        secret_key_file().path(),
        None,
        output.path(),
        &BackupIgnore::default(),
    )
    .unwrap();

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

    std::fs::create_dir_all(ws.join("data")).unwrap();
    std::fs::write(ws.join("data/notes.txt"), "important notes").unwrap();

    std::fs::create_dir_all(ws.join(".lucidos")).unwrap();
    std::fs::write(ws.join(".lucidos/runtime.pid"), "12345").unwrap();

    let sql_dump = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(sql_dump.path(), "INSERT INTO events VALUES (1);").unwrap();

    let compressed_path = tempfile::NamedTempFile::new().unwrap();
    tar_and_compress(
        ws,
        sql_dump.path(),
        secret_key_file().path(),
        None,
        compressed_path.path(),
        &BackupIgnore::default(),
    )
    .unwrap();

    let (identity, recipient) = crypto::generate_key_pair();
    let encrypted_path = tempfile::NamedTempFile::new().unwrap();
    {
        let input = std::fs::File::open(compressed_path.path()).unwrap();
        let mut output = std::fs::File::create(encrypted_path.path()).unwrap();
        crypto::seal(
            &crypto::SealingKey::Recipient(recipient),
            input,
            &mut output,
        )
        .unwrap();
    }

    let decrypted_path = tempfile::NamedTempFile::new().unwrap();
    {
        let input = std::fs::File::open(encrypted_path.path()).unwrap();
        let mut output = std::fs::File::create(decrypted_path.path()).unwrap();
        crypto::open(&crypto::OpeningKey::Age(identity), input, &mut output).unwrap();
    }

    let restore_dir = tempfile::tempdir().unwrap();
    {
        let file = std::fs::File::open(decrypted_path.path()).unwrap();
        let decoder = zstd::Decoder::new(file).unwrap();
        let mut archive = tar::Archive::new(decoder);
        archive.unpack(restore_dir.path()).unwrap();
    }

    let restored_notes =
        std::fs::read_to_string(restore_dir.path().join("data/notes.txt")).unwrap();
    assert_eq!(restored_notes, "important notes");

    let restored_sql =
        std::fs::read_to_string(restore_dir.path().join("lucidos_backup.dump")).unwrap();
    assert_eq!(restored_sql, "INSERT INTO events VALUES (1);");

    assert!(
        !restore_dir.path().join(".lucidos").exists(),
        ".lucidos/ should not be in the restored backup"
    );
}

#[test]
fn test_estimate_weights_small_workspace() {
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

    assert!(dump_end >= 2);
    assert!(compress_end > dump_end);
    assert!(encrypt_end > compress_end);
    assert!(encrypt_end <= 97);
}

#[test]
fn test_estimate_archive_size_is_about_a_third() {
    assert_eq!(estimate_archive_size(3_000_000), 1_000_000);
    assert_eq!(estimate_archive_size(0), 0);
}

#[test]
fn test_workspace_backup_size_sums_included_files_only() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    std::fs::create_dir_all(ws.join("data/artifacts")).unwrap();
    std::fs::write(ws.join("data/artifacts/a.bin"), vec![0u8; 1000]).unwrap();
    std::fs::create_dir_all(ws.join(".lucidos")).unwrap();
    std::fs::write(ws.join(".lucidos/cache.bin"), vec![0u8; 9999]).unwrap();
    std::fs::create_dir_all(ws.join("data/postgres")).unwrap();
    std::fs::write(ws.join("data/postgres/base"), vec![0u8; 5000]).unwrap();

    let size = workspace_backup_size(ws, &BackupIgnore::default());
    assert_eq!(size, 1000, "only data/artifacts/a.bin should count");
}

#[test]
fn test_estimate_restore_weights() {
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
    let (decrypt_end, decompress_end) = estimate_restore_weights(100_000, 15);

    assert!(decrypt_end >= 17, "each phase gets at least 2%");
    assert!(decompress_end >= decrypt_end + 2);
}

#[test]
fn test_move_contents() {
    let src = tempfile::tempdir().unwrap();
    let dest = tempfile::tempdir().unwrap();

    std::fs::create_dir_all(src.path().join("data/artifacts")).unwrap();
    std::fs::write(src.path().join("data/artifacts/profile.md"), "# Profile").unwrap();
    std::fs::write(src.path().join("top.txt"), "hello").unwrap();

    std::fs::create_dir_all(dest.path().join(".lucidos")).unwrap();
    std::fs::write(dest.path().join(".lucidos/key"), "secret").unwrap();

    move_contents(src.path(), dest.path()).unwrap();

    assert_eq!(
        std::fs::read_to_string(dest.path().join("data/artifacts/profile.md")).unwrap(),
        "# Profile"
    );
    assert_eq!(
        std::fs::read_to_string(dest.path().join("top.txt")).unwrap(),
        "hello"
    );
    assert_eq!(
        std::fs::read_to_string(dest.path().join(".lucidos/key")).unwrap(),
        "secret"
    );
}

#[test]
fn test_tar_omits_user_dir_keeps_git() {
    let workspace = tempfile::tempdir().unwrap();
    let ws = workspace.path();
    std::fs::create_dir_all(ws.join("data")).unwrap();
    std::fs::write(ws.join("data/notes.txt"), "workspace content").unwrap();
    std::fs::create_dir_all(ws.join(".git/objects")).unwrap();
    std::fs::write(ws.join(".git/HEAD"), "ref: refs/heads/main").unwrap();

    let sql_dump = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(sql_dump.path(), "CREATE TABLE test;").unwrap();

    let output = tempfile::NamedTempFile::new().unwrap();
    tar_and_compress(
        ws,
        sql_dump.path(),
        secret_key_file().path(),
        None,
        output.path(),
        &BackupIgnore::default(),
    )
    .unwrap();

    let file = std::fs::File::open(output.path()).unwrap();
    let decoder = zstd::Decoder::new(file).unwrap();
    let mut archive = tar::Archive::new(decoder);
    let mut found_paths: Vec<String> = Vec::new();
    for entry in archive.entries().unwrap() {
        let entry = entry.unwrap();
        found_paths.push(entry.path().unwrap().to_string_lossy().to_string());
    }

    assert!(
        !found_paths.iter().any(|p| p.starts_with("user_dir")),
        "archive must not contain user_dir/: {found_paths:?}"
    );
    assert!(
        found_paths.iter().any(|p| p.starts_with(".git")),
        "archive must contain the workspace .git/: {found_paths:?}"
    );
    assert!(
        found_paths.iter().any(|p| p == "data/notes.txt"),
        "archive should contain data/notes.txt: {found_paths:?}"
    );
}

#[test]
fn skip_if_vanished_only_swallows_not_found() {
    use std::io::{Error, ErrorKind};

    let r: Result<Option<()>, _> =
        skip_if_vanished(Path::new("/gone"), Err(Error::from(ErrorKind::NotFound)));
    assert!(matches!(r, Ok(None)));

    let r: Result<Option<()>, _> = skip_if_vanished(
        Path::new("/denied"),
        Err(Error::from(ErrorKind::PermissionDenied)),
    );
    assert!(r.is_err());

    let r = skip_if_vanished(Path::new("/ok"), Ok(42));
    assert!(matches!(r, Ok(Some(42))));
}

#[test]
fn append_file_skips_vanished_file() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(tmp.path(), "content").unwrap();
    let path = tmp.path().to_path_buf();
    drop(tmp);

    let mut buf: Vec<u8> = Vec::new();
    {
        let mut builder = tar::Builder::new(&mut buf);
        append_file(&mut builder, &path, Path::new("gone.txt")).unwrap();
        builder.finish().unwrap();
    }

    let mut archive = tar::Archive::new(&buf[..]);
    assert_eq!(
        archive.entries().unwrap().count(),
        0,
        "vanished file should not appear in the archive"
    );
}

#[test]
fn exact_len_cuts_a_grown_source_and_zero_fills_a_shrunk_one() {
    use std::io::Read;

    let mut grown = ExactLen::new(&b"abcdef"[..], 3);
    let mut out = Vec::new();
    grown.read_to_end(&mut out).unwrap();
    assert_eq!(out, b"abc");
    assert_eq!(grown.padded, 0);

    let mut shrunk = ExactLen::new(&b"abc"[..], 5);
    let mut out = Vec::new();
    shrunk.read_to_end(&mut out).unwrap();
    assert_eq!(out, b"abc\0\0");
    assert_eq!(shrunk.padded, 2);
}

#[test]
fn a_source_that_grows_or_shrinks_mid_copy_keeps_later_entries_aligned() {
    use std::io::Read;

    let dir = tempfile::tempdir().unwrap();
    let after = dir.path().join("after.txt");
    std::fs::write(&after, "intact").unwrap();

    for (declared, source_len) in [(3u64, 600usize), (2000, 100)] {
        let source = vec![b'x'; source_len];
        let mut buf: Vec<u8> = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut buf);
            let mut header = tar::Header::new_gnu();
            header.set_size(declared);
            header.set_mode(0o644);
            header.set_cksum();
            append_exact(
                &mut builder,
                &mut header,
                Path::new("drift.log"),
                &source[..],
            )
            .unwrap();
            append_file(&mut builder, &after, Path::new("after.txt")).unwrap();
            builder.finish().unwrap();
        }

        let mut archive = tar::Archive::new(&buf[..]);
        let mut entries: Vec<(String, Vec<u8>)> = Vec::new();
        for entry in archive.entries().unwrap() {
            let mut entry = entry.unwrap();
            let name = entry.path().unwrap().display().to_string();
            let mut body = Vec::new();
            entry.read_to_end(&mut body).unwrap();
            entries.push((name, body));
        }

        let kept = source_len.min(declared as usize);
        let mut expected = vec![b'x'; kept];
        expected.resize(declared as usize, 0);
        assert_eq!(
            entries,
            vec![
                ("drift.log".to_string(), expected),
                ("after.txt".to_string(), b"intact".to_vec()),
            ],
            "declared {declared} bytes, source yielded {source_len}"
        );
    }
}

#[test]
fn copy_drift_names_how_a_file_changed_during_its_read() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("live.log");
    std::fs::write(&path, "first").unwrap();
    let before = std::fs::metadata(&path).unwrap();

    assert_eq!(copy_drift(&before, &before, 0), None);
    assert_eq!(
        copy_drift(&before, &before, 2),
        Some(CopyDrift::Shrank { padded: 2 })
    );

    std::fs::write(&path, "first, and more").unwrap();
    let grown = std::fs::metadata(&path).unwrap();
    assert_eq!(
        copy_drift(&before, &grown, 0),
        Some(CopyDrift::Grew { kept: 5 })
    );

    std::fs::write(&path, "FIRST").unwrap();
    let file = std::fs::File::options().write(true).open(&path).unwrap();
    file.set_modified(before.modified().unwrap() + std::time::Duration::from_secs(60))
        .unwrap();
    let rewritten = file.metadata().unwrap();
    assert_eq!(
        copy_drift(&before, &rewritten, 0),
        Some(CopyDrift::Rewritten)
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

#[tokio::test]
async fn last_run_absent_by_default() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    assert!(load_last_run(&pool).await.is_none());
    crate::test_support::teardown_test_db(&db_name).await;
}

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

async fn emit_run(
    pool: &sqlx::PgPool,
    ok: bool,
    finished_at: DateTime<Utc>,
) -> Result<(), BoxError> {
    use crate::engine::event_bus::{BusEvent, EventBus, SystemEvent};
    let (bus, _parent_rx) = EventBus::new(pool.clone());
    let started_at = finished_at - chrono::Duration::minutes(3);
    let event = if ok {
        SystemEvent::BackupCompleted {
            filename: "lucidos-backup-myws-20260601-040254.enc".to_string(),
            size_bytes: 1024,
            started_at,
            finished_at,
        }
    } else {
        SystemEvent::BackupFailed {
            error: "upload refused".to_string(),
            started_at,
            finished_at,
        }
    };
    bus.emit(BusEvent::System(event)).await?;
    Ok(())
}

#[tokio::test]
async fn no_backup_run_reads_as_never() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    assert!(load_last_successful_backup(&pool).await.unwrap().is_none());
    crate::test_support::teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_later_failure_does_not_hide_the_last_success() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let success_at = Utc::now() - chrono::Duration::hours(5);

    emit_run(&pool, true, success_at).await.unwrap();
    emit_run(&pool, false, Utc::now()).await.unwrap();

    let at = load_last_successful_backup(&pool)
        .await
        .unwrap()
        .expect("the success is still the answer");
    assert!(
        (at - success_at).num_seconds().abs() <= 1,
        "expected the success at {success_at}, got {at}",
    );
    crate::test_support::teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn the_newest_success_wins() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let newest = Utc::now() - chrono::Duration::minutes(10);

    emit_run(&pool, true, Utc::now() - chrono::Duration::days(3))
        .await
        .unwrap();
    emit_run(&pool, true, newest).await.unwrap();

    let at = load_last_successful_backup(&pool).await.unwrap().unwrap();
    assert!((at - newest).num_seconds().abs() <= 1, "got {at}");
    crate::test_support::teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn backups_are_configured_means_a_destination_is_set() {
    use crate::core::PreferenceStore;
    use crate::engine::event_bus::EventBus;
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let (bus, _parent_rx) = EventBus::new(pool.clone());
    let write = |key: &'static str, value: &'static str| {
        let (pool, bus) = (pool.clone(), bus.clone());
        async move {
            PreferenceStore::set(&pool, &bus, key, value, None)
                .await
                .unwrap();
        }
    };

    assert!(!backups_are_configured(&pool).await.unwrap(), "nothing set");

    write(prefs::BACKUP_SCHEDULE.key(), "0 0 3 * * *").await;
    assert!(
        !backups_are_configured(&pool).await.unwrap(),
        "a cron with no destination has nowhere to upload",
    );

    write(prefs::BACKUP_PROVIDER.key(), "google_drive").await;
    assert!(backups_are_configured(&pool).await.unwrap(), "both set");

    write(prefs::BACKUP_SCHEDULE.key(), "off").await;
    assert!(
        backups_are_configured(&pool).await.unwrap(),
        "manual only is still set up",
    );

    write(prefs::BACKUP_PROVIDER.key(), "").await;
    assert!(
        !backups_are_configured(&pool).await.unwrap(),
        "an empty destination is no destination",
    );

    crate::test_support::teardown_test_db(&db_name).await;
}

#[test]
fn backupignore_prefix_excludes_dir_and_descendants() {
    let ignore = BackupIgnore::parse("data/artifacts/habit-tracker/klines\n");

    assert!(ignore.matches(Path::new("data/artifacts/habit-tracker/klines")));
    assert!(ignore.matches(Path::new(
        "data/artifacts/habit-tracker/klines/2024/jan.parquet"
    )));

    assert!(!ignore.matches(Path::new(
        "data/artifacts/habit-tracker/klines-backup/x.bin"
    )));
    assert!(!ignore.matches(Path::new("data/artifacts/habit-tracker/strategy.md")));
}

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

#[test]
fn backupignore_tolerates_trailing_slash() {
    let with_slash = BackupIgnore::parse("data/cache/\n");
    let without_slash = BackupIgnore::parse("data/cache\n");

    assert!(with_slash.matches(Path::new("data/cache")));
    assert!(with_slash.matches(Path::new("data/cache/x.bin")));
    assert!(without_slash.matches(Path::new("data/cache/x.bin")));
}

#[test]
fn backupignore_supports_wildcard() {
    let ignore = BackupIgnore::parse("data/artifacts/*/klines\n");

    assert!(ignore.matches(Path::new("data/artifacts/habit-tracker/klines")));
    assert!(ignore.matches(Path::new(
        "data/artifacts/habit-tracker/klines/2024.parquet"
    )));
    assert!(ignore.matches(Path::new("data/artifacts/reversal/klines/x.bin")));

    assert!(!ignore.matches(Path::new("data/artifacts/habit-tracker/signals/x.bin")));
}

#[test]
fn backupignore_skips_malformed_glob() {
    let ignore = BackupIgnore::parse("data/cache\ndata/[invalid\n");

    assert_eq!(ignore.len(), 1, "the malformed line is dropped");
    assert!(ignore.matches(Path::new("data/cache/x")));
}

#[test]
fn backupignore_missing_file_means_only_hardcoded() {
    let dir = tempfile::tempdir().unwrap();
    let ignore = BackupIgnore::load(dir.path());

    assert!(ignore.is_empty());
    assert!(is_excluded_workspace_path(
        Path::new(".lucidos/cache/x"),
        &ignore
    ));
    assert!(is_excluded_workspace_path(
        Path::new("data/postgres/global/pg_filenode.map"),
        &ignore
    ));
    assert!(!is_excluded_workspace_path(
        Path::new("data/artifacts/profile.md"),
        &ignore
    ));
}

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
    assert!(is_excluded_workspace_path(
        Path::new("data/artifacts/cache/big.bin"),
        &ignore
    ));
    assert!(!is_excluded_workspace_path(
        Path::new("data/artifacts/profile.md"),
        &ignore
    ));
}

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

    std::fs::create_dir_all(ws.join("data/artifacts/habit-tracker/klines/2024")).unwrap();
    std::fs::write(
        ws.join("data/artifacts/habit-tracker/klines/2024/jan.parquet"),
        "big bytes",
    )
    .unwrap();

    std::fs::write(
        ws.join("data/artifacts/habit-tracker/strategy.md"),
        "# Strategy",
    )
    .unwrap();

    let ignore = BackupIgnore::load(ws);
    let sql_dump = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(sql_dump.path(), "CREATE TABLE test;").unwrap();
    let output = tempfile::NamedTempFile::new().unwrap();
    tar_and_compress(
        ws,
        sql_dump.path(),
        secret_key_file().path(),
        None,
        output.path(),
        &ignore,
    )
    .unwrap();

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

#[tokio::test]
async fn last_run_persists_documented_json_shape() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;

    persist_last_run(&pool, &BackupLastRun::failure("boom", Utc::now()))
        .await
        .unwrap();

    let raw = prefs::BACKUP_LAST_RUN
        .try_stored(&pool)
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

fn archive(id: &str, name: &str, age_secs: i64) -> BackupEntry {
    BackupEntry {
        id: id.to_string(),
        filename: name.to_string(),
        size_bytes: 1024,
        created_at: Utc::now() - chrono::Duration::seconds(age_secs),
    }
}

#[tokio::test]
async fn backup_origin_is_stable_for_its_path_and_rotates_on_path_mismatch() {
    use crate::test_support::{setup_test_db, teardown_test_db};

    let (pool, db) = setup_test_db().await;
    let original = tempfile::tempdir().unwrap();
    let moved = tempfile::tempdir().unwrap();
    assert_eq!(
        read_backup_origin(&pool, original.path()).await.unwrap(),
        None
    );
    let (first, concurrent) = tokio::join!(
        resolve_backup_origin(&pool, original.path()),
        resolve_backup_origin(&pool, original.path()),
    );
    let first = first.unwrap();
    assert_eq!(first, concurrent.unwrap());
    assert_eq!(read_backup_origin(&pool, moved.path()).await.unwrap(), None);
    let second = resolve_backup_origin(&pool, moved.path()).await.unwrap();
    assert_ne!(first, second);
    assert_eq!(
        read_backup_origin(&pool, moved.path()).await.unwrap(),
        Some(second)
    );
    pool.close().await;
    teardown_test_db(&db).await;
}

fn test_origin() -> BackupOrigin {
    BackupOrigin("012345abcdef".to_string())
}

#[test]
fn select_prunable_keeps_newest_n_of_own_workspace_only() {
    let entries = vec![
        archive(
            "p1",
            "lucidos-backup-myws-20260601-000001-o012345abcdef.age",
            100,
        ),
        archive(
            "p2",
            "lucidos-backup-myws-20260601-000002-o012345abcdef.age",
            80,
        ),
        archive(
            "p3",
            "lucidos-backup-myws-20260601-000003-o012345abcdef.age",
            60,
        ),
        archive(
            "p4",
            "lucidos-backup-myws-20260601-000004-o012345abcdef.age",
            40,
        ),
        archive(
            "w1",
            "lucidos-backup-otherws-20260601-000004-o012345abcdef.age",
            5,
        ),
        archive(
            "foreign",
            "lucidos-backup-myws-20260601-000005-oabcdef012345.age",
            1,
        ),
        archive("legacy", "lucidos-backup-myws-20260601-000006.age", 1),
        archive("x1", "important-tax-document.pdf", 1),
    ];

    let to_delete: Vec<String> = select_prunable(entries, "myws", &test_origin(), 2)
        .iter()
        .map(|e| e.id.clone())
        .collect();

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

#[test]
fn select_prunable_keeps_all_when_own_count_within_limit() {
    let entries = vec![
        archive(
            "p1",
            "lucidos-backup-myws-20260601-000001-o012345abcdef.age",
            50,
        ),
        archive(
            "p2",
            "lucidos-backup-myws-20260601-000002-o012345abcdef.age",
            40,
        ),
        archive(
            "w1",
            "lucidos-backup-otherws-20260601-000001-o012345abcdef.age",
            30,
        ),
        archive(
            "w2",
            "lucidos-backup-otherws-20260601-000002-o012345abcdef.age",
            20,
        ),
        archive(
            "w3",
            "lucidos-backup-otherws-20260601-000003-o012345abcdef.age",
            10,
        ),
    ];

    assert!(select_prunable(entries, "myws", &test_origin(), 5).is_empty());
}

#[test]
fn is_own_backup_archive_rejects_prefix_collisions() {
    assert!(is_own_backup_archive(
        "lucidos-backup-myws-20260601-040254-o012345abcdef.age",
        "myws",
        &test_origin()
    ));
    assert!(!is_own_backup_archive(
        "lucidos-backup-myws-2-20260601-040254-o012345abcdef.age",
        "myws",
        &test_origin()
    ));
    assert!(is_own_backup_archive(
        "lucidos-backup-myws-2-20260601-040254-o012345abcdef.age",
        "myws-2",
        &test_origin()
    ));
    assert!(is_own_backup_archive(
        "lucidos-backup-e2e-test-20260601-040254-o012345abcdef.age",
        "e2e-test",
        &test_origin()
    ));
    assert!(!is_own_backup_archive(
        "lucidos-backup-e2e-test-20260601-040254-o012345abcdef.age",
        "e2e",
        &test_origin()
    ));
}

#[test]
fn is_own_backup_archive_requires_timestamp_and_archive_suffix() {
    assert!(is_own_backup_archive(
        "lucidos-backup-myws-20260601-040254-o012345abcdef.age",
        "myws",
        &test_origin()
    ));
    assert!(!is_own_backup_archive(
        "lucidos-backup-myws-notes-o012345abcdef.age",
        "myws",
        &test_origin()
    ));
    assert!(!is_own_backup_archive(
        "lucidos-backup-myws-20260601-040254.zip",
        "myws",
        &test_origin()
    ));
    assert!(!is_own_backup_archive(
        "lucidos-backup-myws-20260601-o012345abcdef.age",
        "myws",
        &test_origin()
    ));
    assert!(!is_own_backup_archive("random.txt", "myws", &test_origin()));
}

#[tokio::test]
async fn prune_old_backups_deletes_only_own_excess_oldest_first() {
    let entries = vec![
        archive(
            "p_old",
            "lucidos-backup-myws-20260601-000001-o012345abcdef.age",
            100,
        ),
        archive(
            "p_mid",
            "lucidos-backup-myws-20260601-000002-o012345abcdef.age",
            50,
        ),
        archive(
            "p_new",
            "lucidos-backup-myws-20260601-000003-o012345abcdef.age",
            10,
        ),
        archive(
            "otherws",
            "lucidos-backup-otherws-20260601-000003-o012345abcdef.age",
            1,
        ),
        archive("doc", "tax.pdf", 1),
        archive(
            "foreign",
            "lucidos-backup-myws-20260601-000004-oabcdef012345.age",
            1,
        ),
        archive("old_age", "lucidos-backup-myws-20260601-000005.age", 1),
        archive("old_enc", "lucidos-backup-myws-20260601-000006.enc", 1),
    ];
    let provider = MockProvider::new(entries);

    let deleted = prune_old_backups(&provider, "myws", &test_origin(), 1)
        .await
        .unwrap();

    assert_eq!(deleted, 2, "two own archives beyond keep=1");
    assert_eq!(
        provider.deleted_ids(),
        vec!["p_old".to_string(), "p_mid".to_string()],
        "only own archives, oldest-first"
    );
}

#[tokio::test]
async fn prune_old_backups_swallows_individual_delete_failures() {
    let entries = vec![
        archive(
            "p_old",
            "lucidos-backup-myws-20260601-000001-o012345abcdef.age",
            100,
        ),
        archive(
            "p_mid",
            "lucidos-backup-myws-20260601-000002-o012345abcdef.age",
            50,
        ),
        archive(
            "p_new",
            "lucidos-backup-myws-20260601-000003-o012345abcdef.age",
            10,
        ),
    ];
    let provider = MockProvider::new(entries).failing_delete_on(&["p_old"]);

    let deleted = prune_old_backups(&provider, "myws", &test_origin(), 1)
        .await
        .unwrap();

    assert_eq!(deleted, 1, "only the successful delete is counted");
    assert_eq!(
        provider.deleted_ids(),
        vec!["p_old".to_string(), "p_mid".to_string()],
        "both deletes attempted despite the first failing"
    );
}

#[tokio::test]
async fn prune_old_backups_propagates_list_failure() {
    let provider = MockProvider::new(vec![]).failing_list();
    assert!(prune_old_backups(&provider, "myws", &test_origin(), 5)
        .await
        .is_err());
}

#[tokio::test]
async fn prune_old_backups_keep_zero_is_a_noop() {
    let entries = vec![
        archive(
            "p1",
            "lucidos-backup-myws-20260601-000001-o012345abcdef.age",
            100,
        ),
        archive(
            "p2",
            "lucidos-backup-myws-20260601-000002-o012345abcdef.age",
            10,
        ),
    ];
    let provider = MockProvider::new(entries);

    assert_eq!(
        prune_old_backups(&provider, "myws", &test_origin(), 0)
            .await
            .unwrap(),
        0
    );
    assert!(provider.deleted_ids().is_empty());
}

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
    assert_eq!(workspace_archive_name(Path::new("/")), "workspace");
}

#[tokio::test]
async fn get_retention_count_reads_preference() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;

    crate::test_support::seed_preference(&pool, prefs::BACKUP_RETENTION.key(), "10")
        .await
        .unwrap();
    assert_eq!(get_retention_count(&pool).await.unwrap(), 10);

    crate::test_support::teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn get_retention_count_falls_back_to_default() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;

    let default = prefs::BACKUP_RETENTION.default_number() as usize;

    assert_eq!(get_retention_count(&pool).await.unwrap(), default);

    crate::test_support::seed_preference(&pool, prefs::BACKUP_RETENTION.key(), "not-a-number")
        .await
        .unwrap();
    assert_eq!(get_retention_count(&pool).await.unwrap(), default);

    crate::test_support::teardown_test_db(&db_name).await;
}

#[test]
fn marked_backup_filename_roundtrips() {
    let workspace = workspace_archive_name(Path::new("/home/u/workspaces/my-ws"));
    let origin = test_origin();
    for suffix in [backup_archive::AGE_SUFFIX, backup_archive::LEGACY_SUFFIX] {
        let filename =
            backup_archive::format_marked(workspace, "20260601-040254", &origin.0, suffix);
        assert!(is_own_backup_archive(&filename, workspace, &origin));
    }
}

#[tokio::test]
async fn unreadable_backup_origin_prevents_upload_and_prune() {
    let workspace = tempfile::tempdir().unwrap();
    let database_url = "postgres://localhost/postgres";
    let pool = PgPool::connect_lazy(database_url).unwrap();
    pool.close().await;
    let provider = MockProvider::new(vec![]);
    let (_, recipient) = crypto::generate_key_pair();

    let error = create_backup(
        &pool,
        workspace.path(),
        database_url,
        crypto::SealingKey::Recipient(recipient),
        &provider,
        |_, _, _| {},
    )
    .await
    .unwrap_err();

    assert!(error
        .to_string()
        .starts_with("could not resolve the backup origin:"));
    assert!(provider.deleted_ids().is_empty());
}

#[tokio::test]
async fn restore_archive_into_clears_the_source_backup_origin() {
    use crate::test_support::{setup_test_db, teardown_test_db, test_db_url};

    let (source, source_db) = setup_test_db().await;
    let (_, target_db) = setup_test_db().await;
    let source_workspace = tempfile::tempdir().unwrap();
    let origin = resolve_backup_origin(&source, source_workspace.path())
        .await
        .unwrap();
    let dump = tempfile::NamedTempFile::new().unwrap();
    pg_dump(&test_db_url(&source_db), dump.path()).unwrap();

    let mut tar = tar::Builder::new(zstd::Encoder::new(Vec::new(), 3).unwrap());
    tar.append_path_with_name(dump.path(), "lucidos_backup.dump")
        .unwrap();
    let compressed = tar.into_inner().unwrap().finish().unwrap();
    let (identity, recipient) = crypto::generate_key_pair();
    let mut archive = tempfile::NamedTempFile::new().unwrap();
    crypto::seal(
        &crypto::SealingKey::Recipient(recipient),
        compressed.as_slice(),
        archive.as_file_mut(),
    )
    .unwrap();

    let target_url = test_db_url(&target_db);
    let target_workspace = tempfile::tempdir().unwrap();
    restore_archive_into(
        target_workspace.path(),
        &target_url,
        crypto::OpeningKey::Age(identity),
        archive.path(),
        |_, _, _| {},
    )
    .await
    .unwrap();

    let target = PgPool::connect(&target_url).await.unwrap();
    assert_eq!(
        read_backup_origin(&target, source_workspace.path())
            .await
            .unwrap(),
        None
    );
    assert_ne!(
        resolve_backup_origin(&target, target_workspace.path())
            .await
            .unwrap(),
        origin
    );
    assert_eq!(
        read_backup_origin(&source, source_workspace.path())
            .await
            .unwrap(),
        Some(origin)
    );

    source.close().await;
    target.close().await;
    teardown_test_db(&source_db).await;
    teardown_test_db(&target_db).await;
}

#[tokio::test]
async fn restore_archive_into_restores_workspace_but_drops_user_dir() {
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

    let compressed = tempfile::NamedTempFile::new().unwrap();
    {
        let file = std::fs::File::create(compressed.path()).unwrap();
        let encoder = zstd::Encoder::new(file, 3).unwrap();
        let mut builder = tar::Builder::new(encoder);
        builder
            .append_dir_all("data", src.path().join("data"))
            .unwrap();
        builder.append_dir_all("user_dir", user_tmp.path()).unwrap();
        builder
            .append_path_with_name(secret_key_file().path(), SECRET_STORE_KEY_ENTRY)
            .unwrap();
        let encoder = builder.into_inner().unwrap();
        encoder.finish().unwrap();
    }

    let (identity, recipient) = crypto::generate_key_pair();
    let encrypted = tempfile::NamedTempFile::new().unwrap();
    {
        let input = std::fs::File::open(compressed.path()).unwrap();
        let mut output = std::fs::File::create(encrypted.path()).unwrap();
        crypto::seal(
            &crypto::SealingKey::Recipient(recipient),
            input,
            &mut output,
        )
        .unwrap();
    }

    let ws = tempfile::tempdir().unwrap();
    let ws_dir = ws.path().join("restored");
    restore_archive_into(
        &ws_dir,
        "postgres://unused@nowhere/none-should-not-be-touched",
        crypto::OpeningKey::Age(identity),
        encrypted.path(),
        |_phase, _cur, _total| {},
    )
    .await
    .expect("restore should succeed without a dump");

    assert_eq!(
        std::fs::read_to_string(ws_dir.join("data/artifacts/profile.md")).unwrap(),
        "# Profile"
    );
    assert!(
        !ws_dir.join("user_dir").exists(),
        "the archive's user_dir/ must be dropped, not restored"
    );
    assert!(
        !ws_dir.join(SECRET_STORE_KEY_ENTRY).exists(),
        "the secret store key goes to the key store, never the workspace"
    );
}

fn secret_key_file() -> tempfile::NamedTempFile {
    let file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(
        file.path(),
        crate::core::secret_store::key().unwrap().to_base64(),
    )
    .unwrap();
    file
}

#[tokio::test]
async fn tar_and_compress_carries_the_pending_change_bundles() {
    use pending_bundles::tests::{pending_branch, repo_at};
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path().join("ws");
    std::fs::create_dir_all(ws.join("data")).unwrap();
    let repo = repo_at(&tmp.path().join("repo")).await;
    pending_branch(&repo, "change").await;
    let bundles = tmp.path().join(pending_bundles::ARCHIVE_DIR);
    let rows = [(repo.to_string_lossy().into_owned(), "change".to_string())];
    assert!(pending_bundles::write_bundles(&ws, &rows, &bundles)
        .await
        .unwrap());

    let sql_dump = tempfile::NamedTempFile::new().unwrap();
    let output = tempfile::NamedTempFile::new().unwrap();
    tar_and_compress(
        &ws,
        sql_dump.path(),
        secret_key_file().path(),
        Some(&bundles),
        output.path(),
        &BackupIgnore::default(),
    )
    .unwrap();

    let decoder = zstd::Decoder::new(std::fs::File::open(output.path()).unwrap()).unwrap();
    let names: Vec<String> = tar::Archive::new(decoder)
        .entries()
        .unwrap()
        .map(|e| e.unwrap().path().unwrap().to_string_lossy().into_owned())
        .collect();
    let dir = pending_bundles::ARCHIVE_DIR;
    for expected in [format!("{dir}/manifest.json"), format!("{dir}/0.bundle")] {
        assert!(
            names.contains(&expected),
            "{expected} missing from {names:?}"
        );
    }
}

#[tokio::test]
async fn restore_archive_into_brings_back_pending_change_branches() {
    use pending_bundles::tests::{git, pending_branch, repo_at};
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    std::fs::create_dir_all(src.join("data")).unwrap();
    std::fs::write(src.join("data/note.md"), "note").unwrap();
    let repo = repo_at(&tmp.path().join("repo")).await;
    let tip = pending_branch(&repo, "change").await;
    let bundles = tmp.path().join(pending_bundles::ARCHIVE_DIR);
    let rows = [(repo.to_string_lossy().into_owned(), "change".to_string())];
    assert!(pending_bundles::write_bundles(&src, &rows, &bundles)
        .await
        .unwrap());

    let compressed = tempfile::NamedTempFile::new().unwrap();
    {
        let file = std::fs::File::create(compressed.path()).unwrap();
        let mut builder = tar::Builder::new(zstd::Encoder::new(file, 3).unwrap());
        builder.append_dir_all("data", src.join("data")).unwrap();
        builder
            .append_dir_all(pending_bundles::ARCHIVE_DIR, &bundles)
            .unwrap();
        builder.into_inner().unwrap().finish().unwrap();
    }
    let (identity, recipient) = crypto::generate_key_pair();
    let encrypted = tempfile::NamedTempFile::new().unwrap();
    crypto::seal(
        &crypto::SealingKey::Recipient(recipient),
        std::fs::File::open(compressed.path()).unwrap(),
        &mut std::fs::File::create(encrypted.path()).unwrap(),
    )
    .unwrap();
    git(&repo, &["branch", "-D", "change"]).await;

    let ws_dir = tmp.path().join("restored");
    let warnings = restore_archive_into(
        &ws_dir,
        "postgres://unused@nowhere/none-should-not-be-touched",
        crypto::OpeningKey::Age(identity),
        encrypted.path(),
        |_phase, _cur, _total| {},
    )
    .await
    .unwrap();

    assert_eq!(warnings, Vec::<String>::new());
    assert_eq!(git(&repo, &["rev-parse", "refs/heads/change"]).await, tip);
    assert!(ws_dir.join("data/note.md").exists());
    assert!(
        !ws_dir.join(pending_bundles::ARCHIVE_DIR).exists(),
        "the bundles go back into their repositories, not the workspace"
    );
}

#[test]
fn captured_backup_preserves_payload_bytes_and_layout() {
    use std::io::Read;
    let ws = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(ws.path().join("data/blobs")).unwrap();
    std::fs::write(ws.path().join("data/blobs/sample"), b"blob bytes").unwrap();
    let dump = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(dump.path(), b"dump bytes").unwrap();
    let key = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(key.path(), b"synthetic backup key").unwrap();
    let output = tempfile::NamedTempFile::new().unwrap();
    tar_and_compress(
        ws.path(),
        dump.path(),
        key.path(),
        None,
        output.path(),
        &BackupIgnore::default(),
    )
    .unwrap();
    let decoder = zstd::Decoder::new(std::fs::File::open(output.path()).unwrap()).unwrap();
    let mut archive = tar::Archive::new(decoder);
    let mut entries = std::collections::BTreeMap::new();
    for entry in archive.entries().unwrap() {
        let mut entry = entry.unwrap();
        let path = entry.path().unwrap().into_owned();
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).unwrap();
        assert!(entries.insert(path, bytes).is_none());
    }
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[Path::new("data/blobs/sample")], b"blob bytes");
    assert_eq!(entries[Path::new("lucidos_backup.dump")], b"dump bytes");
    assert_eq!(
        entries[Path::new(SECRET_STORE_KEY_ENTRY)],
        b"synthetic backup key"
    );
}

#[test]
fn the_size_sum_skips_a_file_that_vanished_after_the_walk() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    std::fs::create_dir_all(ws.join("data")).unwrap();
    std::fs::write(ws.join("data/kept.bin"), vec![0u8; 700]).unwrap();
    let walked = vec![ws.join("data/kept.bin"), ws.join("data/pruned.bin")];
    for unreadable in [Unreadable::Skip, Unreadable::Refuse] {
        assert_eq!(
            sum_file_bytes(ws, walked.clone(), &BackupIgnore::default(), unreadable).unwrap(),
            700
        );
    }
}

#[test]
fn only_the_snapshot_estimate_refuses_an_unreadable_file() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    std::fs::create_dir_all(ws.join("data")).unwrap();
    std::fs::write(ws.join("data/kept.bin"), vec![0u8; 700]).unwrap();
    let walked = vec![ws.join("data/kept.bin"), ws.join("data/kept.bin/child")];
    let ignore = BackupIgnore::default();
    assert_eq!(
        sum_file_bytes(ws, walked.clone(), &ignore, Unreadable::Skip).unwrap(),
        700
    );
    assert!(sum_file_bytes(ws, walked, &ignore, Unreadable::Refuse).is_err());
}
