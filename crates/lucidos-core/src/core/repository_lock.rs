/// The workspace repo lock. Every in-engine writer of the workspace repo holds
/// it across its whole read, file change and commit. A merge holds it across
/// catch-up, sha read, ref move and the repo-root tree sync.
///
/// Two windows close only because both sides take it. A targeted data write
/// saves a file, then commits it, and a `checkout -f main` between the two
/// rewrites the file to its old bytes. A `commit_all_dirty` snapshot between
/// `update-ref` and the tree sync stages the old tree over the new head, which
/// commits every merged file as deleted.
///
/// Take it through [`lock_workspace_repo`], whose [`WorkspaceRepoLock`] every
/// commit helper requires. tokio's mutex is not re-entrant, so no holder may
/// call a function that takes it itself: an `AppManager` write, a merge, or
/// `commit_all_dirty`. Lock order: [`MERGE_MUTEX`] first, then this.
pub static WORKSPACE_REPO_MUTEX: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Proof that the holder owns the workspace repo lock. Only this module can mint
/// one, so a write that takes `&WorkspaceRepoLock` cannot run without the lock.
/// The guard is `'static` and `Send`, so it can move into a `spawn_blocking`
/// closure. It is then released when the blocking work really ends.
pub struct WorkspaceRepoLock {
    _guard: tokio::sync::MutexGuard<'static, ()>,
}

pub async fn lock_workspace_repo() -> WorkspaceRepoLock {
    MAIN_LOCKS.lock_repo().await
}

/// One merge to `main` at a time. Taken before [`WORKSPACE_REPO_MUTEX`].
static MERGE_MUTEX: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// The two locks a merge to `main` takes, in this order. Production passes
/// [`MAIN_LOCKS`]. A test passes a pair of its own, so no other test queues on it.
pub struct MainLocks {
    merge: &'static tokio::sync::Mutex<()>,
    repo: &'static tokio::sync::Mutex<()>,
}

pub static MAIN_LOCKS: MainLocks = MainLocks {
    merge: &MERGE_MUTEX,
    repo: &WORKSPACE_REPO_MUTEX,
};

impl MainLocks {
    pub async fn lock_merge(&self) -> tokio::sync::MutexGuard<'static, ()> {
        self.merge.lock().await
    }

    pub async fn lock_repo(&self) -> WorkspaceRepoLock {
        WorkspaceRepoLock {
            _guard: self.repo.lock().await,
        }
    }

    /// Both locks, in order, for anything that moves `main` outside a merge:
    /// a revert, or a commit on the repo root. A landing pass holds the merge
    /// lock from reading `main` to moving it. So this keeps every other mover
    /// out of that window (ADR 0463).
    pub async fn lock_main(&self) -> MainMoveLock {
        let merge = self.lock_merge().await;
        MainMoveLock {
            _repo: self.lock_repo().await,
            _merge: merge,
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn repo_is_locked_for_test(&self) -> bool {
        self.repo.try_lock().is_err()
    }

    /// A pair of locks nothing else queues on.
    #[cfg(any(test, feature = "test-support"))]
    pub fn private_for_test() -> &'static MainLocks {
        Box::leak(Box::new(MainLocks {
            merge: Box::leak(Box::new(tokio::sync::Mutex::new(()))),
            repo: Box::leak(Box::new(tokio::sync::Mutex::new(()))),
        }))
    }
}

/// Proof that the holder owns both locks of [`MainLocks::lock_main`]. The repo
/// lock is declared first, so it is released first.
pub struct MainMoveLock {
    _repo: WorkspaceRepoLock,
    _merge: tokio::sync::MutexGuard<'static, ()>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Validates that MERGE_MUTEX serializes concurrent merge attempts.
    /// Two tasks that both acquire the mutex should execute sequentially,
    /// not concurrently -- the second task must wait for the first.
    #[tokio::test]
    async fn merge_mutex_serializes_concurrent_access() {
        use std::sync::atomic::{AtomicU32, Ordering};
        use std::sync::Arc;

        let counter = Arc::new(AtomicU32::new(0));
        let max_concurrent = Arc::new(AtomicU32::new(0));

        let mut handles = Vec::new();
        for _ in 0..5 {
            let counter = counter.clone();
            let max_concurrent = max_concurrent.clone();
            handles.push(tokio::spawn(async move {
                let _guard = MERGE_MUTEX.lock().await;
                let active = counter.fetch_add(1, Ordering::SeqCst) + 1;
                // Track maximum concurrent holders
                max_concurrent.fetch_max(active, Ordering::SeqCst);
                // Simulate merge work
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                counter.fetch_sub(1, Ordering::SeqCst);
            }));
        }

        for h in handles {
            h.await.unwrap();
        }

        // At most 1 task should hold the mutex at any time
        assert_eq!(
            max_concurrent.load(Ordering::SeqCst),
            1,
            "MERGE_MUTEX must serialize access -- max concurrent holders should be 1"
        );
    }

    #[test]
    fn production_merges_take_the_global_merge_and_repo_locks() {
        assert!(std::ptr::eq(MAIN_LOCKS.merge, &MERGE_MUTEX));
        assert!(std::ptr::eq(MAIN_LOCKS.repo, &WORKSPACE_REPO_MUTEX));
    }
}
