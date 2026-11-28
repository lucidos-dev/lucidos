//! What starts and ends thread terminals from outside a request: the switch,
//! thread ends, the idle timer, engine teardown and boot recovery.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use uuid::Uuid;

use super::TerminalRegistry;
use crate::engine::event_bus::{BusEvent, LagReplay, ReplayScope, SystemEvent};
use crate::engine::thread_events::{MessageOrigin, TerminalExitCause, ThreadEvent};
use crate::engine::LucidosEngine;

const TIMER: Duration = Duration::from_secs(60);
const ENDING_EVENT_TYPES: [&str; 4] = [
    "ThreadArchived",
    "ThreadDiscarded",
    "ThreadsDeleted",
    "PreferencesChanged",
];

/// The `reason` slug a refusal carries while the switch is off. A client
/// shows the off state for it, never a retry.
pub const DISABLED_REASON: &str = "terminal-disabled";

/// Whether the workspace has the thread terminal turned on. Read from the
/// store on every call, so a flip binds the next request.
pub async fn switch_is_on(pool: &sqlx::PgPool) -> bool {
    crate::core::prefs::TERMINAL_ENABLED.read(pool).await
}

/// The latest bracketed-paste switch in `data`, keeping a short tail so a
/// sequence split across two reads still counts.
pub fn bracketed_paste_change(tail: &mut Vec<u8>, data: &[u8]) -> Option<bool> {
    const ON: &[u8] = b"\x1b[?2004h";
    const OFF: &[u8] = b"\x1b[?2004l";
    let mut window = std::mem::take(tail);
    window.extend_from_slice(data);
    let last = |needle: &[u8]| window.windows(needle.len()).rposition(|w| w == needle);
    let change = match (last(ON), last(OFF)) {
        (Some(on), Some(off)) => Some(on > off),
        (Some(_), None) => Some(true),
        (None, Some(_)) => Some(false),
        (None, None) => None,
    };
    let keep = window.len().saturating_sub(ON.len() - 1);
    *tail = window.split_off(keep);
    change
}

fn sidecar_dir(workspace: &Path) -> PathBuf {
    workspace.join(".lucidos").join("terminals")
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Sidecar {
    thread_id: Uuid,
    pid: u32,
}

/// Record a live shell so boot recovery can sweep it after a crash.
pub fn write_sidecar(workspace: &Path, terminal_id: Uuid, thread_id: Uuid, pid: u32) {
    let dir = sidecar_dir(workspace);
    let written = std::fs::create_dir_all(&dir).and_then(|()| {
        let body =
            serde_json::to_vec(&Sidecar { thread_id, pid }).map_err(std::io::Error::other)?;
        std::fs::write(dir.join(format!("{terminal_id}.json")), body)
    });
    if let Err(e) = written {
        crate::log!(
            "[Terminal] could not write the sidecar for {}: {}",
            terminal_id,
            lucidos_diagnostic_log::private(e.to_string())
        );
    }
}

pub fn remove_sidecar(workspace: &Path, terminal_id: Uuid) {
    let path = sidecar_dir(workspace).join(format!("{terminal_id}.json"));
    if let Err(e) = std::fs::remove_file(&path) {
        if e.kind() != std::io::ErrorKind::NotFound {
            crate::log!(
                "[Terminal] could not remove the sidecar for {}: {}",
                terminal_id,
                lucidos_diagnostic_log::private(e.to_string())
            );
        }
    }
}

/// Sweep shells a crashed engine left behind, and any job that outlived its
/// shell in the same session. A pid reused by another process is never
/// signalled.
pub fn sweep_orphaned_shells(workspace: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(sidecar_dir(workspace)) else {
        return 0;
    };
    let mut swept = 0;
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if let Some(sidecar) = std::fs::read(&path)
            .ok()
            .and_then(|body| serde_json::from_slice::<Sidecar>(&body).ok())
        {
            if session_is_ours(sidecar.pid) {
                crate::runtime::pty::kill_session(sidecar.pid);
                swept += 1;
            }
        }
        if let Err(e) = std::fs::remove_file(&path) {
            crate::log!(
                "[Terminal] could not remove {}: {}",
                lucidos_diagnostic_log::private(path.display().to_string()),
                lucidos_diagnostic_log::private(e.to_string())
            );
        }
    }
    swept
}

/// A live pid still leading its own session is the shell. A dead one leaves
/// its session id unreusable while members remain, so they are its jobs.
fn session_is_ours(pid: u32) -> bool {
    // SAFETY: getsid only reads the kernel's record of a pid.
    let (own_session, sid) = unsafe { (libc::getsid(0), libc::getsid(pid as libc::pid_t)) };
    pid > 1
        && pid as libc::pid_t != own_session
        && (!crate::runtime::pty::is_alive(pid) || sid == pid as libc::pid_t)
}

/// Give every recorded `TerminalOpened` an exit. A restart ends every shell,
/// so an open with no exit reads as stopped by the engine.
pub async fn close_unmatched_opens(engine: &LucidosEngine) -> usize {
    let rows: Vec<(Uuid, String)> = match sqlx::query_as(
        "SELECT o.thread_id, o.payload->>'terminal_id' FROM events o \
         WHERE o.event_type = 'TerminalOpened' AND NOT EXISTS ( \
           SELECT 1 FROM events x WHERE x.event_type = 'TerminalExited' \
             AND x.thread_id = o.thread_id \
             AND x.payload->>'terminal_id' = o.payload->>'terminal_id')",
    )
    .fetch_all(engine.pool())
    .await
    {
        Ok(rows) => rows,
        Err(e) => {
            crate::log!(
                "[Terminal] could not read unmatched terminal opens: {}",
                lucidos_diagnostic_log::private(e.to_string())
            );
            return 0;
        }
    };
    let mut closed = 0;
    for (thread_id, terminal_id) in rows {
        let Ok(terminal_id) = terminal_id.parse() else {
            continue;
        };
        super::emit(
            &engine.event_bus,
            thread_id,
            ThreadEvent::TerminalExited {
                terminal_id,
                cause: TerminalExitCause::EngineStopped,
                exit_code: None,
                duration_secs: 0,
            },
            None,
        )
        .await;
        closed += 1;
    }
    closed
}

/// Boot recovery, run before the API is ready.
pub async fn recover_at_boot(engine: &LucidosEngine) {
    let swept = sweep_orphaned_shells(engine.workspace_path());
    let closed = close_unmatched_opens(engine).await;
    if swept + closed > 0 {
        crate::log!(
            "[Terminal] boot recovery swept {} shells and closed {} terminals",
            swept,
            closed
        );
    }
}

/// Start the subscriber that ends terminals when their thread ends or the
/// switch turns off, and the timer that enforces idle limits.
pub fn start(engine: &Arc<LucidosEngine>) {
    let (mut rx, mut replay) = LagReplay::subscribe(
        &engine.event_bus,
        ReplayScope::Workspace,
        Some(ENDING_EVENT_TYPES.to_vec()),
    );
    let subscriber = engine.clone();
    tokio::spawn(async move {
        while let Some(frames) = replay.read(rx.recv().await, subscriber.pool()).await {
            for frame in frames {
                react(&subscriber, &frame.typed).await;
            }
        }
    });
    let timer = engine.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(TIMER);
        loop {
            tick.tick().await;
            tick_once(&timer.terminals, timer.pool()).await;
        }
    });
}

/// One timer pass: end idle terminals, and end every terminal when a dropped
/// event left the switch off with shells still live.
pub async fn tick_once(terminals: &TerminalRegistry, pool: &sqlx::PgPool) {
    if terminals.live().await.is_empty() {
        return;
    }
    if !switch_is_on(pool).await {
        terminals.end_all(TerminalExitCause::Disabled).await;
        return;
    }
    terminals.reap_idle().await;
}

async fn react(engine: &LucidosEngine, event: &BusEvent) {
    match event {
        BusEvent::Thread {
            thread_id, event, ..
        } => {
            let cause = match event {
                ThreadEvent::ThreadArchived => TerminalExitCause::Archived,
                ThreadEvent::ThreadDiscarded { .. } => TerminalExitCause::Discarded,
                _ => return,
            };
            engine.terminals.end(*thread_id, cause).await;
        }
        BusEvent::System(SystemEvent::ThreadsDeleted { thread_ids, .. }) => {
            for thread_id in thread_ids {
                engine
                    .terminals
                    .end(*thread_id, TerminalExitCause::Deleted)
                    .await;
            }
        }
        BusEvent::System(SystemEvent::PreferencesChanged { key, actor, .. })
            if key == crate::core::prefs::TERMINAL_ENABLED.key() =>
        {
            if switch_is_on(engine.pool()).await {
                announce_turned_on(engine, actor.as_ref()).await;
            } else {
                engine.terminals.end_all(TerminalExitCause::Disabled).await;
            }
        }
        _ => {}
    }
}

/// Turning the terminal on is announced on every device (ADR 0356 decision 10).
async fn announce_turned_on(engine: &LucidosEngine, actor: Option<&MessageOrigin>) {
    let who = match actor {
        Some(MessageOrigin::Device { device_id }) => {
            crate::core::devices::DeviceStore::friendly_name(engine.pool(), device_id).await
        }
        _ => "this computer".to_owned(),
    };
    let message = format!(
        "Terminal turned on by {who}. Every paired device can now open a shell on this computer. \
         Turn it off in Settings → Developer."
    );
    if let Err(e) = engine
        .create_notification(
            "Terminal turned on",
            &message,
            None,
            None,
            None,
            Default::default(),
            actor.cloned(),
        )
        .await
    {
        crate::log!(
            "[Terminal] could not announce the switch: {}",
            lucidos_diagnostic_log::private(e.to_string())
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bracketed_paste_follows_the_last_switch_even_across_reads() {
        let mut tail = Vec::new();
        assert_eq!(
            bracketed_paste_change(&mut tail, b"prompt \x1b[?2004h$ "),
            Some(true)
        );
        assert_eq!(bracketed_paste_change(&mut tail, b"ls\r\n\x1b[?20"), None);
        assert_eq!(
            bracketed_paste_change(&mut tail, b"04l output"),
            Some(false)
        );
        assert_eq!(
            bracketed_paste_change(&mut tail, b"\x1b[?2004l\x1b[?2004h"),
            Some(true)
        );
    }

    #[test]
    fn a_sidecar_whose_pid_no_longer_leads_a_session_is_removed_unsignalled() {
        let dir = tempfile::tempdir().unwrap();
        let own = std::process::id();
        write_sidecar(dir.path(), Uuid::new_v4(), Uuid::new_v4(), own);
        assert!(!session_is_ours(own), "the test runner leads no session");
        assert_eq!(sweep_orphaned_shells(dir.path()), 0);
        assert_eq!(
            std::fs::read_dir(sidecar_dir(dir.path())).unwrap().count(),
            0
        );
    }
}
