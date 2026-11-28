//! Thread terminals (ADRs 0356 and 0820).
//!
//! The registry owns each shell. A socket is an attachment that comes and
//! goes, and the shell outlives every attachment until it exits, times out or
//! is ended.
//!
//! INVARIANT: No byte of terminal input or output reaches an event, a row, a
//! file or the log. Output lives only in each terminal's in-memory ring.

pub mod environment;
pub mod lifecycle;
pub mod ring;
pub mod suggestion;

use std::collections::HashMap;
use std::ffi::OsString;
use std::io::{Read, Write};
use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{broadcast, Mutex, Notify};
use uuid::Uuid;

use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{EventMeta, MessageOrigin, TerminalExitCause, ThreadEvent};
use crate::runtime::pty::{self, Size};
use ring::Ring;

pub const WORKSPACE_CAP: usize = 8;
pub const DETACHED_AT_PROMPT_LIMIT: Duration = Duration::from_secs(30 * 60);
pub const DETACHED_LIMIT: Duration = Duration::from_secs(12 * 60 * 60);
const RING_BYTES: usize = 1 << 20;
const READ_CHUNK: usize = 16 * 1024;
const BROADCAST_CHUNKS: usize = 128;
const END_GRACE: Duration = crate::runtime::claude_code::GROUP_TEARDOWN_GRACE;

/// What every attachment of one terminal receives, in order.
#[derive(Clone, Debug)]
pub enum Output {
    Bytes { offset: u64, data: Arc<[u8]> },
    Resized { size: Size },
    Attachments { count: usize },
    Exited(Exit),
}

/// How a shell ended, as every attachment hears it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Exit {
    pub cause: TerminalExitCause,
    pub exit_code: Option<i32>,
}

/// How an attachment comes current when it joins.
#[derive(Debug, PartialEq, Eq)]
pub enum CatchUp {
    /// The bytes after the client's offset, from that offset.
    Bytes { offset: u64, data: Vec<u8> },
    /// The client's offset was outside the ring: redraw from scratch.
    Reset { offset: u64, data: Vec<u8> },
}

pub struct OpenSpec {
    pub thread_id: Uuid,
    pub cwd: PathBuf,
    pub env: Vec<(OsString, OsString)>,
    pub size: Size,
    pub actor: Option<MessageOrigin>,
}

#[derive(Debug)]
pub enum OpenError {
    CapReached,
    Disabled,
    Spawn(std::io::Error),
}

#[derive(Debug, PartialEq, Eq)]
pub enum TypeError {
    /// A program other than the shell holds the foreground.
    Busy,
    Refused(suggestion::Refusal),
    Write(String),
}

pub struct Terminal {
    pub id: Uuid,
    pub thread_id: Uuid,
    pid: u32,
    master: OwnedFd,
    ring: std::sync::Mutex<Ring>,
    output: broadcast::Sender<Output>,
    attachments: AtomicUsize,
    size: std::sync::Mutex<Size>,
    detached_since: std::sync::Mutex<Option<Instant>>,
    bracketed_paste: AtomicBool,
    ending: std::sync::Mutex<Option<TerminalExitCause>>,
    exit: std::sync::Mutex<Option<Exit>>,
    opened: Instant,
}

/// One client socket's view of a terminal. Dropping it detaches.
pub struct Attachment {
    pub terminal: Arc<Terminal>,
    pub catch_up: CatchUp,
    pub output: broadcast::Receiver<Output>,
    /// Set when the shell had already ended as this attachment joined, so
    /// its exit never reaches `output`.
    pub exited: Option<Exit>,
}

impl Attachment {
    /// Rejoin the live stream after falling behind. The ring and a fresh
    /// place in the stream are read under one lock, as in `attach`.
    pub fn redraw(&mut self) -> CatchUp {
        let ring = self.terminal.ring.lock().unwrap();
        self.output = self.terminal.output.subscribe();
        self.exited = *self.terminal.exit.lock().unwrap();
        let (offset, data) = ring.snapshot();
        CatchUp::Reset { offset, data }
    }
}

impl Drop for Attachment {
    fn drop(&mut self) {
        let count = self.terminal.attachments.fetch_sub(1, Ordering::SeqCst) - 1;
        if count == 0 {
            *self.terminal.detached_since.lock().unwrap() = Some(Instant::now());
        }
        let _ = self.terminal.output.send(Output::Attachments { count });
    }
}

impl Terminal {
    pub fn pid(&self) -> u32 {
        self.pid
    }

    pub fn attachment_count(&self) -> usize {
        self.attachments.load(Ordering::SeqCst)
    }

    /// Attach a client. `resume_from` is the offset it already holds.
    pub fn attach(self: &Arc<Self>, resume_from: Option<u64>) -> Attachment {
        // INVARIANT: Subscribe and read the ring under the ring lock. The
        // reader appends and broadcasts under the same lock, so no byte is
        // missed or doubled between the catch-up and the live stream.
        let ring = self.ring.lock().unwrap();
        let output = self.output.subscribe();
        let exited = *self.exit.lock().unwrap();
        let catch_up =
            match resume_from.and_then(|offset| ring.since(offset).map(|data| (offset, data))) {
                Some((offset, data)) => CatchUp::Bytes { offset, data },
                None => {
                    let (offset, data) = ring.snapshot();
                    CatchUp::Reset { offset, data }
                }
            };
        drop(ring);
        let count = self.attachments.fetch_add(1, Ordering::SeqCst) + 1;
        *self.detached_since.lock().unwrap() = None;
        let _ = self.output.send(Output::Attachments { count });
        Attachment {
            terminal: self.clone(),
            catch_up,
            output,
            exited,
        }
    }

    pub fn write(&self, bytes: &[u8]) -> std::io::Result<()> {
        let mut master = std::fs::File::from(self.master.try_clone()?);
        master.write_all(bytes)
    }

    pub fn size(&self) -> Size {
        *self.size.lock().unwrap()
    }

    /// Resize to the most recently active client's view, and tell every
    /// attachment the new size.
    pub fn resize(&self, size: Size) -> std::io::Result<()> {
        let mut current = self.size.lock().unwrap();
        if *current == size {
            return Ok(());
        }
        pty::resize(&self.master, size)?;
        *current = size;
        let _ = self.output.send(Output::Resized { size });
        Ok(())
    }

    pub fn shell_in_foreground(&self) -> bool {
        pty::shell_in_foreground(&self.master, self.pid)
    }

    /// Wait, up to `limit`, until the shell has printed something. A fresh
    /// shell's first output is its prompt, and text typed before it would echo
    /// ahead of the prompt.
    pub async fn await_first_output(&self, limit: Duration) {
        let mut output = self.output.subscribe();
        if self.ring.lock().unwrap().end() > 0 {
            return;
        }
        let _ = tokio::time::timeout(limit, async {
            while let Ok(frame) = output.recv().await {
                if matches!(frame, Output::Bytes { .. } | Output::Exited(_)) {
                    return;
                }
            }
        })
        .await;
    }

    /// Type a terminal suggestion at the prompt, without Enter.
    pub fn type_suggestion(&self, command: &str) -> Result<(), TypeError> {
        suggestion::validate(command).map_err(TypeError::Refused)?;
        if !self.shell_in_foreground() {
            return Err(TypeError::Busy);
        }
        let bytes = suggestion::typed_bytes(command, self.bracketed_paste.load(Ordering::SeqCst));
        self.write(&bytes)
            .map_err(|e| TypeError::Write(e.to_string()))
    }

    fn detached_for(&self) -> Option<Duration> {
        (self.attachment_count() == 0)
            .then(|| {
                self.detached_since
                    .lock()
                    .unwrap()
                    .map(|since| since.elapsed())
            })
            .flatten()
    }

    fn mark_ending(&self, cause: TerminalExitCause) -> bool {
        let mut ending = self.ending.lock().unwrap();
        if ending.is_some() {
            return false;
        }
        *ending = Some(cause);
        true
    }

    /// Hang up: SIGHUP the shell and the foreground job, as closing a
    /// terminal always has. The shell passes it on to its own jobs.
    fn hang_up(&self) {
        if let Some(group) = pty::foreground_group(&self.master) {
            pty::signal_group(group, libc::SIGHUP);
        }
        pty::signal_group(self.pid, libc::SIGHUP);
    }

    /// Under the ring lock, as `attach` reads it, so every attachment hears
    /// the exit exactly once.
    fn record_exit(&self, exit: Exit) {
        let _ring = self.ring.lock().unwrap();
        *self.exit.lock().unwrap() = Some(exit);
        let _ = self.output.send(Output::Exited(exit));
    }

    fn has_exited(&self) -> bool {
        self.exit.lock().unwrap().is_some()
    }

    fn record_output(&self, data: &[u8], paste_tail: &mut Vec<u8>) {
        let mut ring = self.ring.lock().unwrap();
        let offset = ring.end();
        ring.push(data);
        if let Some(on) = lifecycle::bracketed_paste_change(paste_tail, data) {
            self.bracketed_paste.store(on, Ordering::SeqCst);
        }
        let _ = self.output.send(Output::Bytes {
            offset,
            data: Arc::from(data),
        });
    }
}

enum Slot {
    /// A shell spawning, with any end requested before it could be published.
    Opening(Arc<Notify>, Option<TerminalExitCause>),
    Live(Arc<Terminal>),
}

#[derive(Clone)]
pub struct TerminalRegistry {
    slots: Arc<Mutex<HashMap<Uuid, Slot>>>,
}

impl Default for TerminalRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl TerminalRegistry {
    pub fn new() -> TerminalRegistry {
        TerminalRegistry {
            slots: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn get(&self, thread_id: Uuid) -> Option<Arc<Terminal>> {
        match self.slots.lock().await.get(&thread_id) {
            Some(Slot::Live(terminal)) => Some(terminal.clone()),
            _ => None,
        }
    }

    pub async fn has_live(&self, thread_id: Uuid) -> bool {
        self.slots.lock().await.contains_key(&thread_id)
    }

    pub async fn live(&self) -> Vec<Arc<Terminal>> {
        self.slots
            .lock()
            .await
            .values()
            .filter_map(|slot| match slot {
                Slot::Live(terminal) => Some(terminal.clone()),
                Slot::Opening(..) => None,
            })
            .collect()
    }

    /// Open the thread's terminal, or return the one it already has.
    ///
    /// `switch_on` is re-read after the spawn, under the registry lock, so a
    /// flip during the spawn ends the new shell instead of leaving it live.
    pub async fn open<F, Fut>(
        &self,
        bus: &EventBus,
        workspace: &Path,
        spec: OpenSpec,
        switch_on: F,
    ) -> Result<Arc<Terminal>, OpenError>
    where
        F: Fn() -> Fut,
        Fut: std::future::Future<Output = bool>,
    {
        loop {
            let mut slots = self.slots.lock().await;
            match slots.get(&spec.thread_id) {
                Some(Slot::Live(terminal)) => return Ok(terminal.clone()),
                Some(Slot::Opening(notify, _)) => {
                    let notified = notify.clone().notified_owned();
                    drop(slots);
                    notified.await;
                    continue;
                }
                None => {}
            }
            if slots.len() >= WORKSPACE_CAP {
                return Err(OpenError::CapReached);
            }
            let notify = Arc::new(Notify::new());
            slots.insert(spec.thread_id, Slot::Opening(notify.clone(), None));
            drop(slots);
            let result = self.spawn(bus, workspace, &spec, &switch_on).await;
            let mut slots = self.slots.lock().await;
            let requested_end = match slots.remove(&spec.thread_id) {
                Some(Slot::Opening(_, cause)) => cause,
                _ => None,
            };
            if let Ok(terminal) = &result {
                if let Some(cause) = requested_end {
                    self.end_terminal(terminal, cause, true);
                } else if !terminal.has_exited() {
                    slots.insert(spec.thread_id, Slot::Live(terminal.clone()));
                }
            }
            drop(slots);
            notify.notify_waiters();
            return result;
        }
    }

    async fn spawn<F, Fut>(
        &self,
        bus: &EventBus,
        workspace: &Path,
        spec: &OpenSpec,
        switch_on: &F,
    ) -> Result<Arc<Terminal>, OpenError>
    where
        F: Fn() -> Fut,
        Fut: std::future::Future<Output = bool>,
    {
        let id = Uuid::new_v4();
        let mut env = spec.env.clone();
        env.push((environment::TERMINAL_ID_VAR.into(), id.to_string().into()));
        let shell = pty::spawn_shell(pty::ShellSpec {
            program: pty::login_shell(),
            cwd: spec.cwd.clone(),
            env,
            size: spec.size,
        })
        .map_err(OpenError::Spawn)?;
        let pid = shell.child.id();
        lifecycle::write_sidecar(workspace, id, spec.thread_id, pid);
        let reader = shell.master.try_clone().map_err(OpenError::Spawn)?;
        let (output, _) = broadcast::channel(BROADCAST_CHUNKS);
        let terminal = Arc::new(Terminal {
            id,
            thread_id: spec.thread_id,
            pid,
            master: shell.master,
            ring: std::sync::Mutex::new(Ring::new(RING_BYTES)),
            output,
            attachments: AtomicUsize::new(0),
            size: std::sync::Mutex::new(spec.size),
            detached_since: std::sync::Mutex::new(Some(Instant::now())),
            bracketed_paste: AtomicBool::new(false),
            ending: std::sync::Mutex::new(None),
            exit: std::sync::Mutex::new(None),
            opened: Instant::now(),
        });
        start_reader(terminal.clone(), reader);
        self.watch_exit(
            bus.clone(),
            workspace.to_owned(),
            terminal.clone(),
            shell.child,
        );
        emit(
            bus,
            spec.thread_id,
            ThreadEvent::TerminalOpened { terminal_id: id },
            spec.actor.clone(),
        )
        .await;
        if !switch_on().await {
            self.end_terminal(&terminal, TerminalExitCause::Disabled, true);
            return Err(OpenError::Disabled);
        }
        Ok(terminal)
    }

    fn watch_exit(
        &self,
        bus: EventBus,
        workspace: PathBuf,
        terminal: Arc<Terminal>,
        mut child: std::process::Child,
    ) {
        let slots = self.slots.clone();
        tokio::spawn(async move {
            let status = tokio::task::spawn_blocking(move || child.wait()).await;
            let exit_code = match status {
                Ok(Ok(status)) => {
                    use std::os::unix::process::ExitStatusExt;
                    status
                        .code()
                        .or_else(|| status.signal().map(|signal| 128 + signal))
                }
                _ => None,
            };
            let cause = terminal
                .ending
                .lock()
                .unwrap()
                .unwrap_or(TerminalExitCause::Exited);
            {
                let mut slots = slots.lock().await;
                terminal.record_exit(Exit { cause, exit_code });
                if matches!(slots.get(&terminal.thread_id), Some(Slot::Live(live)) if live.id == terminal.id)
                {
                    slots.remove(&terminal.thread_id);
                }
            }
            lifecycle::remove_sidecar(&workspace, terminal.id);
            emit(
                &bus,
                terminal.thread_id,
                ThreadEvent::TerminalExited {
                    terminal_id: terminal.id,
                    cause,
                    exit_code,
                    duration_secs: terminal.opened.elapsed().as_secs(),
                },
                None,
            )
            .await;
        });
    }

    /// End one terminal: hang up, wait the grace, then sweep its session.
    /// A shell that exits by itself never comes through here (ADR 0263).
    fn end_terminal(&self, terminal: &Arc<Terminal>, cause: TerminalExitCause, grace: bool) {
        if !terminal.mark_ending(cause) {
            return;
        }
        terminal.hang_up();
        let session = terminal.pid;
        if grace {
            tokio::spawn(async move {
                tokio::time::sleep(END_GRACE).await;
                pty::kill_session(session);
            });
        } else {
            pty::kill_session(session);
        }
    }

    /// End the thread's terminal. One still opening ends once it spawns.
    pub async fn end(&self, thread_id: Uuid, cause: TerminalExitCause) -> bool {
        match self.slots.lock().await.get_mut(&thread_id) {
            Some(Slot::Live(terminal)) => {
                self.end_terminal(terminal, cause, true);
                true
            }
            Some(Slot::Opening(_, requested)) => {
                requested.get_or_insert(cause);
                true
            }
            None => false,
        }
    }

    pub async fn end_all(&self, cause: TerminalExitCause) {
        for terminal in self.live().await {
            self.end_terminal(&terminal, cause, true);
        }
    }

    /// End every terminal for an engine stop, with no grace, and wait up to
    /// `wait` for their exits to be recorded.
    pub async fn teardown(&self, wait: Duration) -> usize {
        let live = self.live().await;
        for terminal in &live {
            self.end_terminal(terminal, TerminalExitCause::EngineStopped, false);
        }
        let deadline = Instant::now() + wait;
        while Instant::now() < deadline && !self.slots.lock().await.is_empty() {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        live.len()
    }

    /// End detached terminals past their idle limits.
    pub async fn reap_idle(&self) -> usize {
        let mut ended = 0;
        for terminal in self.live().await {
            let Some(detached) = terminal.detached_for() else {
                continue;
            };
            if detached >= DETACHED_LIMIT
                || (detached >= DETACHED_AT_PROMPT_LIMIT && terminal.shell_in_foreground())
            {
                self.end_terminal(&terminal, TerminalExitCause::IdleTimeout, true);
                ended += 1;
            }
        }
        ended
    }
}

fn start_reader(terminal: Arc<Terminal>, master: OwnedFd) {
    let spawned = std::thread::Builder::new()
        .name("terminal-reader".into())
        .stack_size(256 * 1024)
        .spawn(move || {
            let mut master = std::fs::File::from(master);
            let mut buf = vec![0u8; READ_CHUNK];
            let mut paste_tail = Vec::new();
            loop {
                match master.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => terminal.record_output(&buf[..n], &mut paste_tail),
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
            }
        });
    if let Err(e) = spawned {
        crate::log!(
            "[Terminal] could not start the reader thread: {}",
            lucidos_diagnostic_log::private(e.to_string())
        );
    }
}

async fn emit(bus: &EventBus, thread_id: Uuid, event: ThreadEvent, actor: Option<MessageOrigin>) {
    let name = event.event_type();
    bus.emit_or_log(
        BusEvent::Thread {
            thread_id,
            event,
            meta: EventMeta::with_actor(actor),
        },
        &format!("[Terminal] {name}"),
    )
    .await;
}

#[cfg(test)]
#[path = "terminal_tests.rs"]
mod tests;
