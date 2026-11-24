use super::*;
use std::os::unix::fs::{symlink, PermissionsExt};

pub(super) fn caller() -> CreatedBy {
    CreatedBy {
        workspace_id: "demo".into(),
        thread_id: "test-thread".into(),
    }
}

pub(super) fn recipient() -> Recipient {
    Recipient {
        user: "evidence".into(),
        host: "logs.example.org".into(),
        directory: "evidence/phone".into(),
    }
}

pub(super) struct Fixture {
    _root: tempfile::TempDir,
    path: PathBuf,
}

impl Fixture {
    pub(super) fn path(&self) -> &Path {
        &self.path
    }
}

pub(super) fn fixture() -> Fixture {
    let root = tempfile::tempdir().unwrap();
    let root = Fixture {
        path: root.path().canonicalize().unwrap(),
        _root: root,
    };
    std::fs::write(root.path().join("gateway.log"), b"capture\n").unwrap();
    root
}

#[test]
fn recognized_manifest_excludes_private_siblings_and_survives_reopen() {
    let root = fixture();
    std::fs::write(root.path().join("gateway.log.1"), b"previous\n").unwrap();
    std::fs::write(root.path().join("private-key"), b"do not export").unwrap();
    let prepared = prepare(root.path(), caller(), recipient()).unwrap();
    assert_eq!(prepared.files.len(), 2);
    assert!(prepared
        .files
        .iter()
        .all(|f| !f.path.contains("private-key")));
    assert_eq!(
        load(root.path(), &prepared.id, &caller()).unwrap(),
        prepared
    );
    assert!(!operation_dir(root.path(), &prepared.id)
        .unwrap()
        .join("snapshot")
        .exists());
    let captured = snapshot(root.path(), &prepared.id, &caller()).unwrap();
    assert_eq!(captured.state, Status::Snapshotted);
    assert_eq!(captured.files[0].sha256.as_ref().unwrap().len(), 64);
    assert!(
        snapshot(root.path(), &prepared.id, &caller()).is_err(),
        "no replay"
    );
    let mode = std::fs::metadata(operation_dir(root.path(), &prepared.id).unwrap())
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o700);
}

#[test]
fn replaced_truncated_linked_and_foreign_sources_fail_closed() {
    for replacement in ["replace", "truncate", "symlink", "hardlink"] {
        let root = fixture();
        let prepared = prepare(root.path(), caller(), recipient()).unwrap();
        let source = root.path().join("gateway.log");
        match replacement {
            "truncate" => std::fs::write(&source, b"x").unwrap(),
            "hardlink" => std::fs::hard_link(&source, root.path().join("alias")).unwrap(),
            _ => {
                std::fs::rename(&source, root.path().join("old")).unwrap();
                if replacement == "symlink" {
                    symlink(root.path().join("old"), &source).unwrap();
                } else {
                    std::fs::write(&source, b"private replacement").unwrap();
                }
            }
        }
        assert!(
            snapshot(root.path(), &prepared.id, &caller()).is_err(),
            "{replacement}"
        );
        assert_eq!(
            load(root.path(), &prepared.id, &caller()).unwrap().state,
            Status::Failed
        );
    }
    let root = fixture();
    let prepared = prepare(root.path(), caller(), recipient()).unwrap();
    let mut foreign = caller();
    foreign.thread_id = "another-thread".into();
    assert!(load(root.path(), &prepared.id, &foreign).is_err());
    assert!(snapshot(root.path(), &prepared.id, &foreign).is_err());
}

#[test]
fn appends_do_not_expand_approved_capture() {
    let root = fixture();
    let prepared = prepare(root.path(), caller(), recipient()).unwrap();
    use std::io::Write;
    std::fs::OpenOptions::new()
        .append(true)
        .open(root.path().join("gateway.log"))
        .unwrap()
        .write_all(b"later\n")
        .unwrap();
    let done = snapshot(root.path(), &prepared.id, &caller()).unwrap();
    assert_eq!(done.files[0].bytes, 8);
    assert_eq!(
        std::fs::read(
            operation_dir(root.path(), &done.id)
                .unwrap()
                .join("snapshot/0.log")
        )
        .unwrap(),
        b"capture\n"
    );
}

#[test]
fn unsafe_recipients_and_paths_are_rejected() {
    for host in [
        "-oProxyCommand=bad",
        "host;touch bad",
        "host\nbad",
        "host/other",
        "user@host",
        "",
    ] {
        let mut target = recipient();
        target.host = host.into();
        assert!(target.validate().is_err(), "{host}");
    }
    for path in [
        "../private",
        "/absolute",
        "a/../b",
        "a//b",
        "a;id",
        "~/.ssh",
        "a\nb",
        "",
    ] {
        let mut target = recipient();
        target.directory = path.into();
        assert!(target.validate().is_err(), "{path}");
    }
    assert!(recipient().validate().is_ok());
    let mut ipv6 = recipient();
    ipv6.host = "2001:db8::1".into();
    assert!(ipv6.validate().is_ok());
}

#[test]
fn metadata_only_preparation_refuses_links_overflow_and_unknown_rotations() {
    let root = fixture();
    std::fs::write(root.path().join("gateway.log.backup"), b"unrecognized").unwrap();
    assert!(prepare(root.path(), caller(), recipient()).is_err());
    std::fs::remove_file(root.path().join("gateway.log.backup")).unwrap();
    std::fs::OpenOptions::new()
        .write(true)
        .open(root.path().join("gateway.log"))
        .unwrap()
        .set_len(MAX_BYTES + 1)
        .unwrap();
    assert!(prepare(root.path(), caller(), recipient()).is_err());
    let root = fixture();
    std::fs::create_dir(root.path().join("elsewhere")).unwrap();
    symlink(root.path().join("elsewhere"), root.path().join("logs")).unwrap();
    assert!(prepare(root.path(), caller(), recipient()).is_err());
}

#[test]
fn source_root_move_or_expiry_cannot_retarget_approval() {
    let root = fixture();
    let prepared = prepare(root.path(), caller(), recipient()).unwrap();
    let mut expired = prepared.clone();
    expired.expires_at = 0;
    save(root.path(), &expired).unwrap();
    assert!(snapshot(root.path(), &prepared.id, &caller()).is_err());
    assert!(operation_dir(root.path(), "../escape").is_err());

    let outer = fixture();
    let original = outer.path().join("installation");
    std::fs::create_dir(&original).unwrap();
    std::fs::write(original.join("gateway.log"), b"capture\n").unwrap();
    let operation = prepare(&original, caller(), recipient()).unwrap();
    std::fs::rename(&original, outer.path().join("moved")).unwrap();
    std::fs::create_dir(&original).unwrap();
    let new_store = store(&original).unwrap();
    std::fs::rename(
        outer
            .path()
            .join("moved/host-log-transfers")
            .join(&operation.id),
        new_store.join(&operation.id),
    )
    .unwrap();
    assert!(snapshot(&original, &operation.id, &caller()).is_err());
}

#[test]
fn consent_manifest_has_no_raw_contents_and_private_store_is_bounded() {
    let root = fixture();
    for _ in 0..MAX_OPERATIONS {
        let operation = prepare(root.path(), caller(), recipient()).unwrap();
        let encoded = serde_json::to_string(&operation).unwrap();
        assert!(!encoded.contains("capture\\n"));
        assert_eq!(operation.state, Status::Prepared);
        assert!(operation.files.iter().all(|f| f.sha256.is_none()));
    }
    assert!(prepare(root.path(), caller(), recipient()).is_err());
}

#[test]
fn source_directory_replacement_and_non_regular_files_are_refused() {
    let root = fixture();
    std::fs::create_dir(root.path().join("logs")).unwrap();
    std::fs::write(root.path().join("logs/gateway.err.log"), b"stderr").unwrap();
    let operation = prepare(root.path(), caller(), recipient()).unwrap();
    std::fs::rename(root.path().join("logs"), root.path().join("old-logs")).unwrap();
    symlink(root.path().join("old-logs"), root.path().join("logs")).unwrap();
    assert!(snapshot(root.path(), &operation.id, &caller()).is_err());
    let root = fixture();
    std::fs::remove_file(root.path().join("gateway.log")).unwrap();
    std::fs::create_dir(root.path().join("gateway.log")).unwrap();
    assert!(prepare(root.path(), caller(), recipient()).is_err());
}

#[tokio::test]
async fn raw_host_operation_rejects_devices_and_agent_origin_tokens() {
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;
    let state = crate::server::GatewayState::for_tests();
    for path in ["/prepare", "/transfer-abc/status", "/transfer-abc/transfer"] {
        for local in [false, true] {
            let body = if path == "/prepare" {
                serde_json::json!({ "caller": caller(), "recipient": recipient() })
            } else if path.ends_with("/transfer") {
                serde_json::json!({"caller":caller(), "approval":"test"})
            } else {
                serde_json::json!({"caller":caller()})
            };
            let mut request = Request::builder()
                .method("POST")
                .uri(path)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap();
            if local {
                request
                    .extensions_mut()
                    .insert(crate::auth::AuthenticatedLocalProcess);
                request.headers_mut().insert(
                    crate::auth::HEADER_AGENT_ORIGIN_TOKEN,
                    "agent".parse().unwrap(),
                );
            } else {
                request
                    .extensions_mut()
                    .insert(crate::auth::AuthenticatedDevice {
                        id: "device".into(),
                        label: "My phone".into(),
                    });
            }
            let response = router()
                .with_state(state.clone())
                .oneshot(request)
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::FORBIDDEN);
        }
    }
}

#[tokio::test]
async fn private_errors_never_echo_contents_but_unsupported_sources_are_explicit() {
    use axum::response::IntoResponse;
    let error: crate::BoxError = "untrusted private error text".into();
    let response = operation_failure(error).into_response();
    let bytes = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .unwrap();
    assert!(!String::from_utf8_lossy(&bytes).contains("untrusted private error text"));
    let root = fixture();
    std::fs::remove_file(root.path().join("gateway.log")).unwrap();
    let error = prepare(root.path(), caller(), recipient()).unwrap_err();
    let response = operation_failure(error).into_response();
    let bytes = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .unwrap();
    assert!(String::from_utf8_lossy(&bytes).contains("journal-only"));
}

#[tokio::test]
async fn transfer_requires_exact_manifest_and_is_not_replayed_after_failure() {
    let root = fixture();
    let prepared = prepare(root.path(), caller(), recipient()).unwrap();
    let attempt = |approval: &str| {
        transfer(
            root.path().to_path_buf(),
            prepared.id.clone(),
            caller(),
            approval.to_string(),
        )
    };
    assert!(attempt("foreign").await.is_err());
    assert_eq!(
        load(root.path(), &prepared.id, &caller()).unwrap().state,
        Status::Prepared
    );
    // Persisted in-flight status is uncertain after a crash, never replayable.
    let mut interrupted = prepared.clone();
    interrupted.state = Status::Transferring;
    save(root.path(), &interrupted).unwrap();
    assert!(attempt(&prepared.approval).await.is_err());
}

#[test]
fn a_symlinked_data_directory_still_captures_but_links_below_it_do_not() {
    let outer = fixture();
    let real = outer.path().join("real");
    std::fs::create_dir(&real).unwrap();
    std::fs::write(real.join("gateway.log"), b"capture\n").unwrap();
    let link = outer.path().join("home");
    symlink(&real, &link).unwrap();
    assert!(prepare(&link, caller(), recipient()).is_err());
    let root = installation_root(&link).unwrap();
    let operation = prepare(&root, caller(), recipient()).unwrap();
    assert_eq!(
        snapshot(&root, &operation.id, &caller()).unwrap().state,
        Status::Snapshotted
    );
}

#[test]
fn expired_idle_operations_free_the_store_but_a_live_transfer_keeps_its_slot() {
    let root = fixture();
    let mut operations: Vec<_> = (0..MAX_OPERATIONS)
        .map(|_| prepare(root.path(), caller(), recipient()).unwrap())
        .collect();
    assert!(prepare(root.path(), caller(), recipient()).is_err());
    for operation in &mut operations {
        operation.expires_at = 0;
        save(root.path(), operation).unwrap();
    }
    let live = active_lock(root.path(), &operations[0].id).unwrap();
    fs2::FileExt::lock_exclusive(&live).unwrap();
    assert!(prepare(root.path(), caller(), recipient()).is_ok());
    assert!(read(root.path(), &operations[0].id).is_ok());
    assert!(read(root.path(), &operations[1].id).is_err());
}

#[test]
fn status_of_a_prepared_operation_never_blocks_its_transfer() {
    let root = fixture();
    let op = prepare(root.path(), caller(), recipient()).unwrap();
    let status = recover_status(root.path(), &op.id, &caller()).unwrap();
    assert_eq!(status.state, Status::Prepared);
    assert!(
        !operation_dir(root.path(), &op.id)
            .unwrap()
            .join("active")
            .exists(),
        "status never touched the transfer's lock"
    );
}

#[test]
fn consent_digest_binds_manifest_installation_caller_and_recipient() {
    let root = fixture();
    let prepared = prepare(root.path(), caller(), recipient()).unwrap();
    for changed in 0..4 {
        let mut other = prepared.clone();
        match changed {
            0 => other.recipient.directory.push_str("/elsewhere"),
            1 => other.files[0].bytes += 1,
            2 => other.installation_inode += 1,
            _ => other.caller.thread_id.push('x'),
        }
        assert_ne!(consent_digest(&other).unwrap(), prepared.approval);
    }
}

#[test]
fn interrupted_transfer_recovers_uncertain_and_cleans_only_private_staging() {
    let root = fixture();
    let op = prepare(root.path(), caller(), recipient()).unwrap();
    let mut op = snapshot(root.path(), &op.id, &caller()).unwrap();
    op.state = Status::Transferring;
    save(root.path(), &op).unwrap();
    let dir = operation_dir(root.path(), &op.id).unwrap();
    assert!(dir.join("snapshot").exists());
    let recovered = recover_status(root.path(), &op.id, &caller()).unwrap();
    assert_eq!(recovered.state, Status::Uncertain);
    assert!(!dir.join("snapshot").exists());
    assert!(dir.join("operation.json").exists());
    assert!(root.path().join("gateway.log").exists());
    assert_eq!(load(root.path(), &op.id, &caller()).unwrap(), recovered);
}

#[test]
fn active_transfer_status_never_recovers_or_removes_live_staging() {
    let root = fixture();
    let op = prepare(root.path(), caller(), recipient()).unwrap();
    let mut op = snapshot(root.path(), &op.id, &caller()).unwrap();
    let active = active_lock(root.path(), &op.id).unwrap();
    fs2::FileExt::lock_exclusive(&active).unwrap();
    op.state = Status::Transferring;
    save(root.path(), &op).unwrap();
    assert_eq!(
        recover_status(root.path(), &op.id, &caller())
            .unwrap()
            .state,
        Status::Transferring
    );
    assert!(operation_dir(root.path(), &op.id)
        .unwrap()
        .join("snapshot")
        .exists());
    drop(active);
    op.state = Status::Verified;
    save(root.path(), &op).unwrap();
    assert_eq!(
        recover_status(root.path(), &op.id, &caller())
            .unwrap()
            .state,
        Status::Verified
    );
}
