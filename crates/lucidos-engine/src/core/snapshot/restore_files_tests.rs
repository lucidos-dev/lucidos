use super::*;
use crate::core::{backup::capture, snapshot::manifest};

fn fixture(workspace: &Path, archive: &Path) -> Manifest {
    let captured = capture::capture_files(workspace, archive).unwrap();
    let mut manifest = manifest::fixture();
    manifest.files.count = captured.count;
    manifest.files.bytes = captured.bytes;
    manifest.files.tree_sha256 = captured.tree_sha256;
    // File staging verifies only the two file components. The restore runner
    // independently checks the authenticated manifest and database dump.
    manifest
        .components
        .retain(|c| c.name != manifest::ComponentName::Database);
    for component in &mut manifest.components {
        let (bytes, sha256) =
            check::component_hash(&archive.join(component.name.as_str())).unwrap();
        component.bytes = bytes;
        component.sha256 = sha256;
    }
    manifest
}

#[test]
fn staging_round_trips_git_and_data_without_touching_workspace() {
    let source = tempfile::tempdir().unwrap();
    let archive = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    for (path, content) in [
        (".git/HEAD", "ref: refs/heads/main\n"),
        ("data/artifacts/note.md", "saved"),
        (".lucidos/worktrees/dirty/file", "unsaved"),
    ] {
        let path = source.path().join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }
    let manifest = fixture(source.path(), archive.path());
    let staged = target.path().join("stage");
    stage(archive.path(), &manifest, &staged).unwrap();
    assert_eq!(
        std::fs::read_to_string(staged.join("data/artifacts/note.md")).unwrap(),
        "saved"
    );
    assert!(staged.join(".git/HEAD").is_file());
    assert!(!staged.join(".lucidos").exists());
    assert_eq!(
        std::fs::read_to_string(source.path().join(".lucidos/worktrees/dirty/file")).unwrap(),
        "unsaved"
    );
    verify_tree(archive.path(), &manifest, &staged).unwrap();
    std::fs::write(staged.join("data/artifacts/note.md"), "edited").unwrap();
    assert!(verify_tree(archive.path(), &manifest, &staged).is_err());
}

#[test]
fn staging_never_reuses_or_cleans_a_preexisting_directory() {
    let source = tempfile::tempdir().unwrap();
    let archive = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    std::fs::write(source.path().join("note"), "saved").unwrap();
    let manifest = fixture(source.path(), archive.path());
    std::fs::write(target.path().join("owned-by-user"), "keep").unwrap();
    assert!(stage(archive.path(), &manifest, target.path()).is_err());
    assert!(target.path().join("owned-by-user").is_file());
}

#[test]
fn corrupt_component_is_refused_before_creating_staging() {
    let source = tempfile::tempdir().unwrap();
    let archive = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    std::fs::write(source.path().join("note"), "saved").unwrap();
    let manifest = fixture(source.path(), archive.path());
    std::fs::write(archive.path().join("files.tar.zst"), "corrupt").unwrap();
    let staged = target.path().join("stage");
    assert!(stage(archive.path(), &manifest, &staged).is_err());
    assert!(!staged.exists());
}

#[test]
fn protected_archive_is_refused_even_with_matching_hashes() {
    use sha2::{Digest, Sha256};
    use std::io::Write;
    for path in [".lucidos/worktrees/dirty/file", "data/postgres/base/file"] {
        let archive = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        let content = b"protected";
        let mut header = tar::Header::new_gnu();
        header.set_size(content.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        let encoder = zstd::Encoder::new(
            File::create(archive.path().join("files.tar.zst")).unwrap(),
            3,
        )
        .unwrap();
        let mut tar = tar::Builder::new(encoder);
        tar.append_data(&mut header, path, &content[..]).unwrap();
        tar.into_inner().unwrap().finish().unwrap();
        let record = FileIndexEntry {
            path: path.into(),
            size: content.len() as u64,
            mode: 0o644,
            sha256: format!("{:x}", Sha256::digest(content)),
        };
        let mut line = serde_json::to_vec(&record).unwrap();
        line.push(b'\n');
        let mut index = zstd::Encoder::new(
            File::create(archive.path().join("files.index.jsonl.zst")).unwrap(),
            3,
        )
        .unwrap();
        index.write_all(&line).unwrap();
        index.finish().unwrap();
        let mut manifest = manifest::fixture();
        manifest.files.count = 1;
        manifest.files.bytes = content.len() as u64;
        manifest.files.tree_sha256 = format!("{:x}", Sha256::digest(&line));
        manifest
            .components
            .retain(|c| c.name != manifest::ComponentName::Database);
        for component in &mut manifest.components {
            (component.bytes, component.sha256) =
                check::component_hash(&archive.path().join(component.name.as_str())).unwrap();
        }
        let staged = target.path().join("stage");
        assert!(stage(archive.path(), &manifest, &staged).is_err());
        assert!(!staged.exists());
    }
}

#[cfg(unix)]
#[test]
fn tree_verification_refuses_symlink_parents_and_extra_files() {
    let source = tempfile::tempdir().unwrap();
    let archive = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    std::fs::create_dir(source.path().join("data")).unwrap();
    std::fs::write(source.path().join("data/note"), "saved").unwrap();
    let manifest = fixture(source.path(), archive.path());
    let staged = target.path().join("stage");
    stage(archive.path(), &manifest, &staged).unwrap();
    std::fs::write(staged.join("extra"), "unexpected").unwrap();
    assert!(verify_tree(archive.path(), &manifest, &staged).is_err());
    std::fs::remove_file(staged.join("extra")).unwrap();
    std::fs::rename(staged.join("data"), target.path().join("elsewhere")).unwrap();
    std::os::unix::fs::symlink(target.path().join("elsewhere"), staged.join("data")).unwrap();
    assert!(verify_tree(archive.path(), &manifest, &staged).is_err());
}

#[test]
fn restore_paths_exclude_runtime_pgdata_and_nonrelative_names() {
    for path in [
        "",
        ".",
        "../escape",
        "/absolute",
        "\\absolute",
        ".lucidos/key",
        "data/postgres/base",
        "data/postgres.old/base",
    ] {
        assert!(!restorable_path(Path::new(path)), "{path}");
    }
    for path in [".git/HEAD", "data/artifacts/note", "data/postgresql.md"] {
        assert!(restorable_path(Path::new(path)), "{path}");
    }
}

#[cfg(unix)]
#[test]
fn swap_preserves_current_exclusions_links_and_relocated_data() {
    use std::os::unix::fs::symlink;
    let source = tempfile::tempdir().unwrap();
    let archive = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    let relocated = tempfile::tempdir().unwrap();
    for (base, path, bytes) in [
        (source.path(), "data/note", "saved"),
        (source.path(), "data/private/keep", "archived private"),
        (source.path(), "data/.backupignore", ""),
        (source.path(), ".git/HEAD", "saved head"),
        (target.path(), ".git/HEAD", "current head"),
        (target.path(), ".lucidos/worktrees/dirty/file", "unsaved"),
        (relocated.path(), "note", "current"),
        (relocated.path(), "extra", "remove"),
        (relocated.path(), "private/keep", "current private"),
        (relocated.path(), "postgres.old/base", "physical db"),
        (relocated.path(), ".backupignore", "data/private\n"),
    ] {
        let path = base.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }
    symlink(relocated.path(), target.path().join("data")).unwrap();
    symlink("missing", target.path().join("link")).unwrap();
    let manifest = fixture(source.path(), archive.path());
    let stage_root = archive.path().join("stage");
    stage(archive.path(), &manifest, &stage_root).unwrap();
    let swap = super::super::file_swap::FileSwap::prepare(
        archive.path(),
        &manifest,
        &stage_root,
        target.path(),
    )
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(relocated.path().join("note")).unwrap(),
        "current"
    );
    swap.apply().unwrap();
    swap.verify().unwrap();
    for (path, expected) in [
        (relocated.path().join("note"), "saved"),
        (relocated.path().join("private/keep"), "current private"),
        (relocated.path().join("postgres.old/base"), "physical db"),
        (
            target.path().join(".lucidos/worktrees/dirty/file"),
            "unsaved",
        ),
        (target.path().join(".git/HEAD"), "saved head"),
    ] {
        assert_eq!(std::fs::read_to_string(path).unwrap(), expected);
    }
    assert!(!relocated.path().join("extra").exists());
    assert!(target.path().join("data").is_symlink());
    assert!(target.path().join("link").is_symlink());
    // Verification uses the frozen policy even though the archived ignore file
    // has now replaced the current one.
    std::fs::write(relocated.path().join("note"), "corrupt").unwrap();
    assert!(swap.verify().is_err());
}

#[test]
fn swap_refuses_unreadable_or_invalid_ignore_without_live_mutation() {
    let source = tempfile::tempdir().unwrap();
    let archive = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    std::fs::write(source.path().join("note"), "saved").unwrap();
    std::fs::write(target.path().join("note"), "current").unwrap();
    std::fs::create_dir(target.path().join("data")).unwrap();
    let manifest = fixture(source.path(), archive.path());
    let staged = archive.path().join("stage");
    stage(archive.path(), &manifest, &staged).unwrap();
    for content in [b"[invalid".as_slice(), &[0xff]] {
        std::fs::write(target.path().join("data/.backupignore"), content).unwrap();
        assert!(super::super::file_swap::FileSwap::prepare(
            archive.path(),
            &manifest,
            &staged,
            target.path(),
        )
        .is_err());
        assert_eq!(
            std::fs::read_to_string(target.path().join("note")).unwrap(),
            "current"
        );
    }
}

#[test]
fn swap_handles_file_directory_shape_changes_but_refuses_preserved_children() {
    let source = tempfile::tempdir().unwrap();
    let archive = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    std::fs::create_dir(source.path().join("was-file")).unwrap();
    std::fs::write(source.path().join("was-file/child"), "saved child").unwrap();
    std::fs::write(source.path().join("was-dir"), "saved file").unwrap();
    std::fs::write(target.path().join("was-file"), "old file").unwrap();
    std::fs::create_dir(target.path().join("was-dir")).unwrap();
    std::fs::write(target.path().join("was-dir/child"), "old child").unwrap();
    std::fs::create_dir(target.path().join("data")).unwrap();
    std::fs::write(target.path().join("data/.backupignore"), "was-dir/child").unwrap();
    let manifest = fixture(source.path(), archive.path());
    let staged = archive.path().join("stage");
    stage(archive.path(), &manifest, &staged).unwrap();
    assert!(super::super::file_swap::FileSwap::prepare(
        archive.path(),
        &manifest,
        &staged,
        target.path(),
    )
    .is_err());
    assert_eq!(
        std::fs::read_to_string(target.path().join("was-file")).unwrap(),
        "old file"
    );
    std::fs::write(target.path().join("data/.backupignore"), "").unwrap();
    let swap = super::super::file_swap::FileSwap::prepare(
        archive.path(),
        &manifest,
        &staged,
        target.path(),
    )
    .unwrap();
    swap.apply().unwrap();
    assert_eq!(
        std::fs::read_to_string(target.path().join("was-file/child")).unwrap(),
        "saved child"
    );
    assert_eq!(
        std::fs::read_to_string(target.path().join("was-dir")).unwrap(),
        "saved file"
    );
}

#[test]
fn swap_uses_capture_glob_semantics_for_directory_names() {
    let source = tempfile::tempdir().unwrap();
    let archive = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    for root in [source.path(), target.path()] {
        std::fs::create_dir_all(root.join("data/private")).unwrap();
        std::fs::write(root.join("data/.backupignore"), "data/privat?\n").unwrap();
    }
    std::fs::write(source.path().join("data/private/note"), "saved").unwrap();
    std::fs::write(target.path().join("data/private/note"), "current").unwrap();
    let manifest = fixture(source.path(), archive.path());
    let staged = archive.path().join("stage");
    stage(archive.path(), &manifest, &staged).unwrap();
    let swap = super::super::file_swap::FileSwap::prepare(
        archive.path(),
        &manifest,
        &staged,
        target.path(),
    )
    .unwrap();
    swap.apply().unwrap();
    assert_eq!(
        std::fs::read_to_string(target.path().join("data/private/note")).unwrap(),
        "current"
    );
}

#[test]
fn serialized_policy_survives_ignore_replacement_and_rejects_rebinding() {
    use super::super::file_swap::{FileSwap, FrozenPolicy};
    let source = tempfile::tempdir().unwrap();
    let archive = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    for (root, path, content) in [
        (source.path(), "data/private/file", "archived"),
        (source.path(), "data/.backupignore", ""),
        (target.path(), "data/private/file", "current"),
        (target.path(), "data/.backupignore", "data/private/\n"),
    ] {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }
    let manifest = fixture(source.path(), archive.path());
    let staged = archive.path().join("stage");
    stage(archive.path(), &manifest, &staged).unwrap();
    let policy = FrozenPolicy::capture(target.path()).unwrap();
    let encoded = serde_json::to_vec(&policy).unwrap();
    std::fs::write(target.path().join("data/.backupignore"), "").unwrap();
    let policy: FrozenPolicy = serde_json::from_slice(&encoded).unwrap();
    let swap =
        FileSwap::prepare_with_policy(archive.path(), &manifest, &staged, target.path(), &policy)
            .unwrap();
    swap.apply().unwrap();
    assert_eq!(
        std::fs::read_to_string(target.path().join("data/private/file")).unwrap(),
        "current"
    );
    let other = tempfile::tempdir().unwrap();
    assert!(FileSwap::prepare_with_policy(
        archive.path(),
        &manifest,
        &staged,
        other.path(),
        &policy
    )
    .is_err());
}
