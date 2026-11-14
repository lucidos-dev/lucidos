//! One-time migration off the per-workspace embedding-model cache.
//!
//! The gateway used to pin every engine's `FASTEMBED_CACHE_DIR` to
//! `<workspace>/.lucidos/fastembed`, so each workspace on a machine downloaded
//! and kept its own ~465 MB copy of a byte-identical model. It now inherits one
//! shared cache instead (see
//! [`model_download::apply_default_cache_dir`](super::model_download::apply_default_cache_dir)),
//! which leaves every existing install carrying copies nothing will ever read
//! again.
//!
//! Two best-effort steps run around the background model load, and between them
//! an upgrade costs no download and reclaims the disk:
//!
//! * [`seed_shared_cache_from_legacy`] moves a per-workspace copy into the
//!   shared location when nothing is there yet. One `rename` preserves the
//!   `hf-hub` layout (blobs, snapshot symlinks, refs) exactly, so the first
//!   engine to boot after the upgrade warms the cache and every other workspace
//!   finds it complete.
//! * [`reclaim_legacy_cache`] deletes a per-workspace copy once the model has
//!   demonstrably loaded from somewhere else.
//!
//! Neither can fail the boot: both return an outcome, log what they did, and
//! leave the load to proceed exactly as it would have.
//!
//! **What the reclaim is allowed to touch is deliberately narrow**: the one path
//! the gateway itself created, only when it is a real directory (never a
//! symlink somebody wired up), and only when the active cache is somewhere else.
//! An unknown is never a licence to delete (`.claude/rules/rust.md`), and the
//! thing at stake is a multi-hundred-MB download the user may be offline for.

use std::path::{Path, PathBuf};

/// Where the gateway used to pin each engine's model cache, under the
/// workspace's own ephemeral `.lucidos/`.
const LEGACY_SUBPATH: &str = ".lucidos/fastembed";

/// The per-workspace cache path for `workspace`, whether or not it exists.
///
/// `pub(crate)` because it is also the last-resort location
/// [`apply_default_cache_dir`](super::model_download::apply_default_cache_dir)
/// falls back to when there is no per-user cache root, and the two must name the
/// same directory or that fallback would look like a leftover to reclaim.
pub(crate) fn legacy_cache_path(workspace: &Path) -> PathBuf {
    workspace.join(LEGACY_SUBPATH)
}

/// What [`seed_shared_cache_from_legacy`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SeedOutcome {
    /// No per-workspace copy to move: the steady state on every boot after the
    /// first, and on a fresh install.
    NothingToSeed,
    /// The shared cache already holds files, so the per-workspace copy is
    /// redundant rather than a seed. [`reclaim_legacy_cache`] deals with it once
    /// the model has loaded.
    SharedCacheAlreadyPopulated,
    /// The per-workspace copy IS the shared cache now.
    Seeded,
    /// The move could not be made (a different filesystem, a permission
    /// problem, another engine seeding at the same instant). Logged, and the
    /// caller downloads as it otherwise would.
    Failed,
}

/// What [`reclaim_legacy_cache`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReclaimOutcome {
    /// Nothing there: the steady state.
    NothingToReclaim,
    /// Not a real directory (a symlink, most likely deliberate: the packaged
    /// e2e run wires one up). Left alone.
    NotADirectory,
    /// This IS the cache in use, or it contains it (or sits inside it), so
    /// removing the subtree would take live data with it. Happens when something
    /// points `FASTEMBED_CACHE_DIR` back at the workspace, and when there was no
    /// per-user cache root to share so the engine fell back to this very path.
    StillInUse,
    /// Removed, reclaiming `bytes`.
    Reclaimed { bytes: u64 },
    /// The removal failed. Logged; the copy simply stays.
    Failed,
}

/// Total bytes under `dir`, counting symlinks as themselves so `hf-hub`'s
/// `snapshots/<commit>/<file>` pointers are not summed on top of the
/// `blobs/<etag>` files they target. `0` for anything unreadable.
pub(crate) fn dir_bytes(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .map(|entry| {
            let path = entry.path();
            match std::fs::symlink_metadata(&path) {
                Ok(meta) if meta.is_dir() => dir_bytes(&path),
                Ok(meta) => meta.len(),
                Err(_) => 0,
            }
        })
        .sum()
}

/// Whether two paths OVERLAP: the same directory, or one containing the other.
///
/// Equality alone is not the question, because `remove_dir_all` takes a whole
/// subtree with it. If the active cache sits INSIDE the legacy path (nothing
/// stops somebody pointing `FASTEMBED_CACHE_DIR` at
/// `<workspace>/.lucidos/fastembed/hub`), removing the legacy directory would
/// delete the live cache the engine just loaded from, and the next boot would
/// re-download the model or fail offline. Containment the other way is equally
/// disqualifying: the legacy directory would be a subtree of the live cache.
///
/// Symlinks are resolved when both sides exist, since the active cache can be
/// reached through one (`scripts/e2e-packaged.sh` wires one up) or simply
/// spelled differently. When either side cannot be resolved the raw paths are
/// compared, which errs toward "no overlap" for two paths that are genuinely
/// unrelated and is why the caller checks this only for a directory that exists.
fn overlaps(a: &Path, b: &Path) -> bool {
    let (a, b) = match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => (a, b),
        _ => (a.to_path_buf(), b.to_path_buf()),
    };
    a.starts_with(&b) || b.starts_with(&a)
}

/// Whether `dir` is a real directory (not a symlink to one, and not a file).
fn is_real_dir(dir: &Path) -> bool {
    std::fs::symlink_metadata(dir).is_ok_and(|m| m.is_dir())
}

/// Whether `dir` holds at least one entry. `false` for a missing or unreadable
/// directory, which is what makes an absent shared cache seedable.
fn has_entries(dir: &Path) -> bool {
    std::fs::read_dir(dir).is_ok_and(|mut entries| entries.next().is_some())
}

/// Move this workspace's leftover model cache into the shared location, when
/// there is one and nothing is there yet.
///
/// This is what makes the upgrade free: the alternative is every existing
/// multi-workspace install re-downloading a model it already has on disk. One
/// `rename` moves the whole `hf-hub` tree intact, so the first engine to boot
/// after the upgrade seeds the shared cache and the others find it complete and
/// simply reclaim their own copies.
///
/// Safe under a race by construction: `rename` onto a POPULATED directory fails,
/// so two engines seeding at the same instant produce one winner and one logged
/// no-op, never a half-merged cache. Nothing is deleted here either way, so the
/// worst case leaves both copies in place for [`reclaim_legacy_cache`].
pub fn seed_shared_cache_from_legacy(workspace: &Path, active_cache: &Path) -> SeedOutcome {
    let legacy = legacy_cache_path(workspace);
    if !is_real_dir(&legacy) || overlaps(&legacy, active_cache) {
        return SeedOutcome::NothingToSeed;
    }
    if has_entries(active_cache) {
        return SeedOutcome::SharedCacheAlreadyPopulated;
    }
    if let Some(parent) = active_cache.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            log!(
                @Memory,
                "Could not prepare {} for the shared model cache: {}",
                parent.display(),
                e
            );
            return SeedOutcome::Failed;
        }
    }
    match std::fs::rename(&legacy, active_cache) {
        Ok(()) => {
            log!(
                @Memory,
                "Moved this workspace's model cache to the shared location at {} ({} bytes), so \
                 every workspace on this machine now reads one copy",
                active_cache.display(),
                dir_bytes(active_cache)
            );
            SeedOutcome::Seeded
        }
        Err(e) => {
            // Expected and harmless when another engine seeded first, or when
            // the workspace and the cache live on different filesystems.
            log!(
                @Memory,
                "Could not move {} to the shared model cache at {}: {}. Falling back to the \
                 normal load",
                legacy.display(),
                active_cache.display(),
                e
            );
            SeedOutcome::Failed
        }
    }
}

/// Delete this workspace's leftover model cache, now that the model has loaded
/// from somewhere else.
///
/// Call it ONLY after a provider has actually been built from `active_cache`:
/// that is the proof the shared copy is complete and usable, and without it this
/// would be deleting the only copy on the machine.
pub fn reclaim_legacy_cache(workspace: &Path, active_cache: &Path) -> ReclaimOutcome {
    let legacy = legacy_cache_path(workspace);
    if std::fs::symlink_metadata(&legacy).is_err() {
        return ReclaimOutcome::NothingToReclaim;
    }
    if !is_real_dir(&legacy) {
        log!(
            @Memory,
            "Leaving {} alone: it is not a plain directory, so somebody put it there deliberately",
            legacy.display()
        );
        return ReclaimOutcome::NotADirectory;
    }
    if overlaps(&legacy, active_cache) {
        return ReclaimOutcome::StillInUse;
    }
    let bytes = dir_bytes(&legacy);
    match std::fs::remove_dir_all(&legacy) {
        Ok(()) => {
            log!(
                @Memory,
                "Reclaimed {} bytes: removed this workspace's leftover model cache at {}, \
                 superseded by the shared cache at {}",
                bytes,
                legacy.display(),
                active_cache.display()
            );
            ReclaimOutcome::Reclaimed { bytes }
        }
        Err(e) => {
            log!(
                @Memory,
                "Could not remove the leftover model cache at {}: {}. It is unused but still \
                 taking {} bytes",
                legacy.display(),
                e,
                bytes
            );
            ReclaimOutcome::Failed
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// A workspace with a per-workspace cache holding one file, plus a separate
    /// (empty, unless `populate_shared`) shared cache directory.
    struct Fixture {
        _root: tempfile::TempDir,
        workspace: PathBuf,
        shared: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let root = tempfile::tempdir().expect("tempdir");
            let workspace = root.path().join("workspace");
            let shared = root.path().join("cache/lucidos/fastembed");
            fs::create_dir_all(&workspace).expect("workspace");
            Self {
                _root: root,
                workspace,
                shared,
            }
        }

        fn with_legacy_copy(self) -> Self {
            let legacy = legacy_cache_path(&self.workspace);
            fs::create_dir_all(legacy.join("blobs")).expect("legacy blobs");
            fs::write(legacy.join("blobs/etag"), vec![7u8; 64]).expect("legacy blob");
            self
        }

        fn with_populated_shared_cache(self) -> Self {
            fs::create_dir_all(self.shared.join("blobs")).expect("shared blobs");
            fs::write(self.shared.join("blobs/etag"), vec![1u8; 8]).expect("shared blob");
            self
        }

        fn legacy(&self) -> PathBuf {
            legacy_cache_path(&self.workspace)
        }
    }

    #[test]
    fn nothing_to_seed_without_a_leftover_copy() {
        let f = Fixture::new();
        assert_eq!(
            seed_shared_cache_from_legacy(&f.workspace, &f.shared),
            SeedOutcome::NothingToSeed
        );
        assert!(!f.shared.exists(), "the seed must not create anything");
    }

    /// The upgrade path that makes this free: the whole tree moves, so no
    /// existing install re-downloads a model it already has.
    #[test]
    fn the_leftover_copy_becomes_the_shared_cache() {
        let f = Fixture::new().with_legacy_copy();
        // The applier creates the shared directory empty before the load runs,
        // which is the shape this has to handle.
        fs::create_dir_all(&f.shared).expect("empty shared cache");

        assert_eq!(
            seed_shared_cache_from_legacy(&f.workspace, &f.shared),
            SeedOutcome::Seeded
        );
        assert!(
            f.shared.join("blobs/etag").exists(),
            "the tree must arrive intact"
        );
        assert!(
            !f.legacy().exists(),
            "the copy moved, it was not duplicated"
        );
    }

    /// The guard that makes a parallel seed safe: whoever gets there first wins
    /// and nobody's cache is clobbered.
    #[test]
    fn a_populated_shared_cache_is_never_overwritten() {
        let f = Fixture::new()
            .with_legacy_copy()
            .with_populated_shared_cache();

        assert_eq!(
            seed_shared_cache_from_legacy(&f.workspace, &f.shared),
            SeedOutcome::SharedCacheAlreadyPopulated
        );
        assert_eq!(
            fs::read(f.shared.join("blobs/etag"))
                .expect("shared blob")
                .len(),
            8,
            "the shared cache must be exactly as it was"
        );
        assert!(
            f.legacy().exists(),
            "the leftover stays for the reclaim to handle, it is never dropped here"
        );
    }

    #[test]
    fn reclaim_is_a_silent_no_op_when_there_is_nothing_left_over() {
        let f = Fixture::new();
        assert_eq!(
            reclaim_legacy_cache(&f.workspace, &f.shared),
            ReclaimOutcome::NothingToReclaim
        );
    }

    /// The whole point of the reclaim, and the byte count that makes the log
    /// line worth reading.
    #[test]
    fn reclaim_removes_the_leftover_copy_and_reports_the_bytes() {
        let f = Fixture::new()
            .with_legacy_copy()
            .with_populated_shared_cache();

        assert_eq!(
            reclaim_legacy_cache(&f.workspace, &f.shared),
            ReclaimOutcome::Reclaimed { bytes: 64 }
        );
        assert!(!f.legacy().exists());
        assert!(
            f.shared.join("blobs/etag").exists(),
            "the shared cache is untouched"
        );
    }

    /// If something still points the cache at the workspace, that directory is
    /// the model in use, not a leftover. Deleting it would take memory down and
    /// cost a fresh download.
    #[test]
    fn reclaim_refuses_while_the_leftover_path_is_the_active_cache() {
        let f = Fixture::new().with_legacy_copy();
        let active = f.legacy();

        assert_eq!(
            reclaim_legacy_cache(&f.workspace, &active),
            ReclaimOutcome::StillInUse
        );
        assert!(f.legacy().join("blobs/etag").exists());
    }

    /// Equality is not enough, because the removal takes a whole subtree. An
    /// active cache NESTED under the leftover path would be deleted along with
    /// its parent, costing the user the model they just loaded and breaking the
    /// next offline boot.
    #[test]
    fn reclaim_refuses_when_the_active_cache_lives_inside_the_leftover() {
        let f = Fixture::new().with_legacy_copy();
        let nested = f.legacy().join("hub");
        fs::create_dir_all(&nested).expect("nested active cache");

        assert_eq!(
            reclaim_legacy_cache(&f.workspace, &nested),
            ReclaimOutcome::StillInUse
        );
        assert!(nested.exists(), "the live cache must survive");
        assert!(f.legacy().join("blobs/etag").exists());
    }

    /// The same guard the other way round: a leftover path that CONTAINS the
    /// active cache is not a leftover either.
    #[test]
    fn seeding_declines_when_the_two_paths_overlap() {
        let f = Fixture::new().with_legacy_copy();
        let nested = f.legacy().join("hub");

        assert_eq!(
            seed_shared_cache_from_legacy(&f.workspace, &nested),
            SeedOutcome::NothingToSeed,
            "moving a directory into itself is not a migration"
        );
        assert!(f.legacy().join("blobs/etag").exists());
    }

    /// Reached through a symlink it is somebody's deliberate wiring (the
    /// packaged e2e run makes one), and following it would delete a cache that
    /// lives somewhere else entirely.
    #[cfg(unix)]
    #[test]
    fn reclaim_never_follows_a_symlink() {
        let f = Fixture::new().with_populated_shared_cache();
        let elsewhere = f.workspace.join("real-cache");
        fs::create_dir_all(elsewhere.join("blobs")).expect("target");
        fs::write(elsewhere.join("blobs/etag"), vec![3u8; 16]).expect("target blob");
        fs::create_dir_all(f.workspace.join(".lucidos")).expect(".lucidos");
        std::os::unix::fs::symlink(&elsewhere, f.legacy()).expect("symlink");

        assert_eq!(
            reclaim_legacy_cache(&f.workspace, &f.shared),
            ReclaimOutcome::NotADirectory
        );
        assert!(
            elsewhere.join("blobs/etag").exists(),
            "the symlink's target must be untouched"
        );
        assert!(f.legacy().exists(), "the link itself stays too");
    }

    /// A removal that cannot happen is reported and survived. Boot never depends
    /// on this working, and the leftover is dead weight either way, so a
    /// half-emptied tree is an acceptable end state: what matters is that the
    /// failure is reported rather than swallowed.
    #[cfg(unix)]
    #[test]
    fn a_removal_that_cannot_happen_is_survived() {
        use std::os::unix::fs::PermissionsExt;

        let f = Fixture::new()
            .with_legacy_copy()
            .with_populated_shared_cache();
        let parent = f.workspace.join(".lucidos");
        let original = fs::metadata(&parent).expect("parent").permissions();
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o500)).expect("read-only parent");

        let outcome = reclaim_legacy_cache(&f.workspace, &f.shared);

        // Restore before asserting, so a failure still leaves a removable tree.
        fs::set_permissions(&parent, original).expect("restore");
        assert_eq!(outcome, ReclaimOutcome::Failed);
        assert!(f.legacy().exists(), "the directory could not be unlinked");
    }
}
