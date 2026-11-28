use serde::Deserialize;
use std::path::Path;

/// INVARIANT: Wire shape the bash supervisor writes. Plain JSON object so the
/// supervisor (bash + `printf`) doesn't need a JSON encoder, just
/// formatted string interpolation. New fields MUST be optional so an
/// older supervisor + newer engine still parses.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct RespawnInfo {
    old_pid: u32,
    exit_code: i32,
    died_at: chrono::DateTime<chrono::Utc>,
    supervisor_pid: u32,
}

fn parse_sidecar(json: &str) -> Result<RespawnInfo, serde_json::Error> {
    serde_json::from_str(json)
}

const SIDECAR_FILENAME: &str = "engine.last-death.json";

fn sidecar_path(workspace: &Path) -> std::path::PathBuf {
    workspace.join(".lucidos").join(SIDECAR_FILENAME)
}

fn take_sidecar(workspace: &Path) -> Option<RespawnInfo> {
    let path = sidecar_path(workspace);
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => {
            crate::log_legacy!(
                "[SupervisorRespawn] failed to read {}: {}",
                path.display(),
                e,
            );
            return None;
        }
    };
    if let Err(e) = std::fs::remove_file(&path) {
        crate::log_legacy!(
            "[SupervisorRespawn] failed to remove {}: {}",
            path.display(),
            e,
        );
    }
    match parse_sidecar(&String::from_utf8_lossy(&bytes)) {
        Ok(info) => Some(info),
        Err(e) => {
            crate::log_legacy!(
                "[SupervisorRespawn] failed to parse sidecar: {} (content: {:?})",
                e,
                String::from_utf8_lossy(&bytes),
            );
            None
        }
    }
}

pub async fn emit_if_present(workspace: &Path, bus: &crate::engine::event_bus::EventBus) {
    let Some(info) = take_sidecar(workspace) else {
        return;
    };
    crate::log_legacy!(
        "[SupervisorRespawn] previous engine pid {} died with exit {} at {} (supervisor pid {}) — emitting EngineSupervisorRespawned",
        info.old_pid,
        info.exit_code,
        info.died_at.to_rfc3339(),
        info.supervisor_pid,
    );
    if lucidos_diagnostic_log::fault::is_internal_engine_exit(info.exit_code) {
        crate::internal_faults::record_pending(crate::internal_faults::FaultKind::EngineCrash);
    }
    bus.emit_or_log(
        crate::engine::event_bus::BusEvent::System(
            crate::engine::event_bus::SystemEvent::EngineSupervisorRespawned {
                old_pid: info.old_pid,
                exit_code: info.exit_code,
                died_at: info.died_at,
                supervisor_pid: info.supervisor_pid,
            },
        ),
        "[SupervisorRespawn] EngineSupervisorRespawned",
    )
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_sidecar_happy_path() {
        let json = r#"{"old_pid":8562,"exit_code":9,"died_at":"2026-05-21T02:34:37Z","supervisor_pid":1234}"#;
        let info = parse_sidecar(json).expect("must parse");
        assert_eq!(info.old_pid, 8562);
        assert_eq!(info.exit_code, 9);
        assert_eq!(info.supervisor_pid, 1234);
        assert_eq!(info.died_at.to_rfc3339(), "2026-05-21T02:34:37+00:00");
    }

    #[test]
    fn parse_sidecar_rejects_malformed_json() {
        assert!(parse_sidecar("not json").is_err());
    }

    #[test]
    fn parse_sidecar_rejects_missing_field() {
        let json = r#"{"old_pid":1,"died_at":"2026-05-21T02:34:37Z","supervisor_pid":2}"#;
        assert!(parse_sidecar(json).is_err());
    }

    #[test]
    fn take_sidecar_absent_returns_none() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        assert!(take_sidecar(dir.path()).is_none());
    }

    #[test]
    fn take_sidecar_reads_and_deletes() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let lucidos_dir = dir.path().join(".lucidos");
        std::fs::create_dir_all(&lucidos_dir).expect("mkdir");
        let path = lucidos_dir.join(SIDECAR_FILENAME);
        std::fs::write(
            &path,
            r#"{"old_pid":42,"exit_code":137,"died_at":"2026-05-21T02:34:37Z","supervisor_pid":99}"#,
        )
        .expect("write");

        let info = take_sidecar(dir.path()).expect("must parse");
        assert_eq!(info.old_pid, 42);
        assert_eq!(info.exit_code, 137);
        assert!(!path.exists(), "sidecar must be deleted after read");
    }

    #[test]
    fn take_sidecar_malformed_logs_and_returns_none() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let lucidos_dir = dir.path().join(".lucidos");
        std::fs::create_dir_all(&lucidos_dir).expect("mkdir");
        let path = lucidos_dir.join(SIDECAR_FILENAME);
        std::fs::write(&path, "{not json").expect("write");

        assert!(take_sidecar(dir.path()).is_none());
        assert!(!path.exists(), "malformed sidecar must still be deleted");
    }
}
