use super::{handle::Handle, BoxError};
use crate::engine::thread_events::MessageOrigin;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Phase {
    Requested,
    Refused,
    RecoveryCapture,
    Staging,
    DbRestore,
    FileSwap,
    FilesRestored,
    Completed,
    FailedUnchanged,
    RollingBack,
    RolledBack,
    NeedsAttention,
}

impl Phase {
    pub fn allows(self, next: Self) -> bool {
        use Phase::*;
        matches!(
            (self, next),
            (Requested, Refused | RecoveryCapture)
                | (RecoveryCapture, FailedUnchanged | Staging)
                | (Staging, FailedUnchanged | DbRestore)
                | (DbRestore, FailedUnchanged | FileSwap | RollingBack)
                | (FileSwap, FilesRestored | RollingBack)
                | (FilesRestored, Completed | RollingBack)
                | (RollingBack, RolledBack | NeedsAttention)
        )
    }

    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Refused
                | Self::Completed
                | Self::FailedUnchanged
                | Self::RolledBack
                | Self::NeedsAttention
        )
    }

    /// The gateway must finish rollback before it may spawn the engine.
    pub fn blocks_engine(self) -> bool {
        matches!(
            self,
            Self::DbRestore
                | Self::FileSwap
                | Self::FilesRestored
                | Self::RollingBack
                | Self::NeedsAttention
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Journal {
    pub restore_id: Uuid,
    pub workspace_id: String,
    pub snapshot: Handle,
    pub recovery: Option<Handle>,
    #[serde(default)]
    pub files: Option<PreparedFiles>,
    #[serde(default)]
    pub announcement: Option<RestoreAnnouncement>,
    pub actor: MessageOrigin,
    pub reason: Option<String>,
    phase: Phase,
    pub updated_at: DateTime<Utc>,
}

/// External restore state. The original policy is immutable through rollback.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreparedFiles {
    pub policy: super::file_swap::FrozenPolicy,
    pub staging_roots: Vec<std::path::PathBuf>,
}

/// Frozen before the first completion append. Retrying after an interrupted
/// append must use the same safe payload even if retained directories change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RestoreAnnouncement {
    pub notification_id: Uuid,
    pub retained_worktrees: Vec<String>,
    /// Absent in legacy evidence: preserve its original notification payload.
    /// New evidence freezes the visible attribution before either append.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor_label: Option<String>,
}

impl Journal {
    pub fn new(
        workspace_id: impl Into<String>,
        snapshot: Handle,
        actor: MessageOrigin,
        reason: Option<String>,
    ) -> Self {
        Self {
            restore_id: Uuid::new_v4(),
            workspace_id: workspace_id.into(),
            snapshot,
            recovery: None,
            files: None,
            announcement: None,
            actor,
            reason,
            phase: Phase::Requested,
            updated_at: Utc::now(),
        }
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    pub fn advance(&mut self, next: Phase) -> Result<(), BoxError> {
        if !self.phase.allows(next) {
            return Err("invalid snapshot restore phase transition".into());
        }
        if matches!(next, Phase::Staging | Phase::DbRestore) && self.recovery.is_none() {
            return Err("verified recovery snapshot required".into());
        }
        self.phase = next;
        self.updated_at = Utc::now();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn journal_only_allows_the_documented_edges_and_requires_recovery() {
        use Phase::*;
        let phases = [
            Requested,
            Refused,
            RecoveryCapture,
            Staging,
            DbRestore,
            FileSwap,
            FilesRestored,
            Completed,
            FailedUnchanged,
            RollingBack,
            RolledBack,
            NeedsAttention,
        ];
        let edges = [
            (Requested, Refused),
            (Requested, RecoveryCapture),
            (RecoveryCapture, FailedUnchanged),
            (RecoveryCapture, Staging),
            (Staging, FailedUnchanged),
            (Staging, DbRestore),
            (DbRestore, FailedUnchanged),
            (DbRestore, FileSwap),
            (DbRestore, RollingBack),
            (FileSwap, FilesRestored),
            (FileSwap, RollingBack),
            (FilesRestored, Completed),
            (FilesRestored, RollingBack),
            (RollingBack, RolledBack),
            (RollingBack, NeedsAttention),
        ];
        for a in phases {
            for b in phases {
                assert_eq!(a.allows(b), edges.contains(&(a, b)), "{a:?} -> {b:?}");
            }
        }
        let manifest = super::super::manifest::fixture();
        let mut journal = Journal::new(
            manifest.workspace_id,
            manifest.handle,
            manifest.captured_by,
            None,
        );
        journal.advance(RecoveryCapture).unwrap();
        assert!(journal.advance(Staging).is_err());
        journal.recovery = Some(Handle::generate());
        journal.advance(Staging).unwrap();
        journal.advance(DbRestore).unwrap();
        assert!(journal.phase().blocks_engine());
        journal.advance(RollingBack).unwrap();
        journal.advance(RolledBack).unwrap();
        assert!(journal.phase().is_terminal());
        assert!(!journal.phase().blocks_engine());
    }
}
