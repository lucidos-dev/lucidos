//! Local workspace snapshots. The registry and restore journal live outside
//! the workspace database and file tree.
pub mod boot_guard;
pub mod boundary;
pub mod capacity;
pub mod check;
mod database_capacity;
pub mod events;
pub mod file_swap;
pub mod first_boot;
pub mod handle;
pub mod identity;
pub mod jobs;
pub mod journal;
pub mod manifest;
pub mod offline;
pub mod registry_binding;
pub mod restore_database;
pub mod restore_files;
pub mod service;
pub mod source_authority;
pub mod store;
pub mod worktree_exemptions;

use std::sync::{
    atomic::{AtomicU8, Ordering},
    Arc,
};

pub type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// One workspace-wide slot shared by backup, capture, check and restore.
#[derive(Clone, Default)]
pub struct ArchiveJobSlot(Arc<SlotState>);

#[derive(Default)]
struct SlotState {
    holder: AtomicU8,
    released: tokio::sync::Notify,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ArchiveJobKind {
    Backup = 1,
    SnapshotCapture = 2,
    SnapshotCheck = 3,
    SnapshotRestore = 4,
}

impl ArchiveJobSlot {
    pub fn try_acquire(&self, kind: ArchiveJobKind) -> Option<ArchiveJobGuard> {
        self.0
            .holder
            .compare_exchange(0, kind as u8, Ordering::SeqCst, Ordering::SeqCst)
            .ok()
            .map(|_| ArchiveJobGuard(self.0.clone()))
    }

    /// Take the slot for a backup, waiting up to `limit` while a snapshot job
    /// holds it. A running backup already serves the caller, so that returns
    /// `Err(Busy::Backup)` at once.
    pub async fn acquire_for_backup(
        &self,
        limit: std::time::Duration,
    ) -> Result<ArchiveJobGuard, Busy> {
        let deadline = tokio::time::Instant::now() + limit;
        loop {
            let released = self.0.released.notified();
            if let Some(guard) = self.try_acquire(ArchiveJobKind::Backup) {
                return Ok(guard);
            }
            if self.is_running(ArchiveJobKind::Backup) {
                return Err(Busy::Backup);
            }
            if tokio::time::timeout_at(deadline, released).await.is_err() {
                return Err(Busy::Snapshot);
            }
        }
    }

    pub fn is_busy(&self) -> bool {
        self.0.holder.load(Ordering::SeqCst) != 0
    }

    pub fn is_running(&self, kind: ArchiveJobKind) -> bool {
        self.0.holder.load(Ordering::SeqCst) == kind as u8
    }
}

/// Who held the slot when a backup could not take it.
#[derive(Debug, PartialEq, Eq)]
pub enum Busy {
    Backup,
    Snapshot,
}

pub struct ArchiveJobGuard(Arc<SlotState>);

impl Drop for ArchiveJobGuard {
    fn drop(&mut self) {
        self.0.holder.store(0, Ordering::SeqCst);
        self.0.released.notify_waiters();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn archive_slot_is_shared_and_released_on_unwind() {
        let slot = ArchiveJobSlot::default();
        let clone = slot.clone();
        let result = std::panic::catch_unwind(|| {
            let _guard = slot.try_acquire(ArchiveJobKind::SnapshotCapture).unwrap();
            assert!(clone.try_acquire(ArchiveJobKind::Backup).is_none());
            assert!(!slot.is_running(ArchiveJobKind::Backup));
            assert!(slot.is_running(ArchiveJobKind::SnapshotCapture));
            panic!("synthetic capture failure");
        });
        assert!(result.is_err());
        assert!(!clone.is_busy());
        let guard = clone.try_acquire(ArchiveJobKind::Backup).unwrap();
        assert!(slot.is_running(ArchiveJobKind::Backup));
        assert!(slot.try_acquire(ArchiveJobKind::SnapshotCheck).is_none());
        assert!(slot.try_acquire(ArchiveJobKind::SnapshotRestore).is_none());
        drop(guard);
        assert!(!slot.is_busy());
    }

    /// A scheduled backup must not vanish because a snapshot job held the slot.
    #[tokio::test]
    async fn a_backup_waits_out_a_snapshot_job_but_not_another_backup() {
        let slot = ArchiveJobSlot::default();
        let limit = std::time::Duration::from_secs(5);
        let check = slot.try_acquire(ArchiveJobKind::SnapshotCheck).unwrap();
        let waiting = tokio::spawn({
            let slot = slot.clone();
            async move { slot.acquire_for_backup(limit).await.map(drop) }
        });
        tokio::task::yield_now().await;
        drop(check);
        assert_eq!(waiting.await.unwrap(), Ok(()));

        let backup = slot.try_acquire(ArchiveJobKind::Backup).unwrap();
        assert_eq!(
            slot.acquire_for_backup(limit).await.err(),
            Some(Busy::Backup)
        );
        drop(backup);

        let _capture = slot.try_acquire(ArchiveJobKind::SnapshotCapture).unwrap();
        let short = std::time::Duration::from_millis(50);
        assert_eq!(
            slot.acquire_for_backup(short).await.err(),
            Some(Busy::Snapshot)
        );
    }
}
