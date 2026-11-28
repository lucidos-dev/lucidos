use serde::{Deserialize, Serialize};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FaultKind {
    Panic,
    TaskPanic,
    EngineCrash,
    BootFailure,
    #[serde(rename = "own-5xx")]
    Own5xx,
}

impl FaultKind {
    pub const ALL: [Self; 5] = [
        Self::Panic,
        Self::TaskPanic,
        Self::EngineCrash,
        Self::BootFailure,
        Self::Own5xx,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Panic => "panic",
            Self::TaskPanic => "task-panic",
            Self::EngineCrash => "engine-crash",
            Self::BootFailure => "boot-failure",
            Self::Own5xx => "own-5xx",
        }
    }
}

pub fn is_internal_engine_exit(code: i32) -> bool {
    is_internal_engine_status(Some(code), None)
        || (code >= 128 && is_internal_engine_status(None, Some(code - 128)))
}

pub fn is_internal_engine_status(code: Option<i32>, signal: Option<i32>) -> bool {
    code == Some(101)
        || signal.is_some_and(|signal| {
            [
                libc::SIGABRT,
                libc::SIGSEGV,
                libc::SIGBUS,
                libc::SIGILL,
                libc::SIGKILL,
            ]
            .contains(&signal)
        })
}

#[derive(Clone)]
pub struct Inbox(PathBuf);

impl Inbox {
    pub fn new(workspace: &Path) -> Self {
        Self(workspace.join(".lucidos/internal-faults"))
    }

    pub fn record(&self, kind: FaultKind) -> io::Result<()> {
        use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&self.0)?;
        if self.0.join(kind.as_str()).exists() {
            return Ok(());
        }
        let id = Uuid::new_v4();
        let temp = self.0.join(format!(".{id}"));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temp)?;
        let result = (|| {
            write!(file, "{id}")?;
            file.sync_all()?;
            match std::fs::hard_link(&temp, self.0.join(kind.as_str())) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Ok(()),
                Err(e) => Err(e),
            }
        })();
        let cleanup = std::fs::remove_file(temp);
        result.and(cleanup)
    }

    pub fn pending(&self) -> io::Result<Vec<(FaultKind, Uuid)>> {
        let mut pending = Vec::new();
        for kind in FaultKind::ALL {
            match std::fs::read_to_string(self.0.join(kind.as_str())) {
                Ok(id) => pending.push((kind, Uuid::parse_str(&id).map_err(io::Error::other)?)),
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
        }
        Ok(pending)
    }

    pub fn acknowledge(&self, kind: FaultKind) -> io::Result<()> {
        std::fs::remove_file(self.0.join(kind.as_str()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_status_preserves_codes_while_shell_status_decodes_signals() {
        assert!(!is_internal_engine_status(Some(128 + libc::SIGBUS), None));
        assert!(is_internal_engine_status(None, Some(libc::SIGBUS)));
        assert!(is_internal_engine_exit(128 + libc::SIGBUS));
        assert!(is_internal_engine_exit(101));
        for code in [0, 1, 2, 128 + libc::SIGUSR1, 128 + libc::SIGINT] {
            assert!(!is_internal_engine_exit(code));
        }
    }

    #[test]
    fn fault_wire_names_match_the_durable_vocabulary() {
        for kind in FaultKind::ALL {
            assert_eq!(serde_json::to_value(kind).unwrap(), kind.as_str());
        }
        assert!(serde_json::from_str::<FaultKind>("\"provider\"").is_err());
    }

    #[test]
    fn inbox_is_bounded_private_and_survives_reopening() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let inbox = Inbox::new(dir.path());
        assert!(inbox.pending().unwrap().is_empty());
        for _ in 0..10 {
            for kind in FaultKind::ALL {
                inbox.record(kind).unwrap();
            }
        }
        let pending = inbox.pending().unwrap();
        assert_eq!(pending.len(), FaultKind::ALL.len());
        assert_eq!(Inbox::new(dir.path()).pending().unwrap(), pending);
        for (kind, _) in pending {
            assert_eq!(
                std::fs::metadata(inbox.0.join(kind.as_str()))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
            inbox.acknowledge(kind).unwrap();
        }
        assert!(inbox.pending().unwrap().is_empty());
    }
}
