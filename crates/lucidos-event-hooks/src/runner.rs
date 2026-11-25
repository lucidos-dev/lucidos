//! The hook runner: one script, one pending event, one answer.
//!
//! It runs the script as a trigger script runs, in the workspace under the
//! agent sandbox, on the same shell and Python. It writes the input to stdin,
//! waits at most the hook's timeout, and reads the answer from stdout. It
//! fails closed: every way a run can go wrong is a refusal.

use std::path::Path;
use std::pin::pin;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use lucidos_core::core::agent_sandbox::{Sandbox, SandboxedCommand};
use lucidos_core::core::event_hooks::HookDefinition;
use lucidos_core::core::shell::{command_shell, TaskOutcome};
use lucidos_core::core::{
    apply_to_subprocess_env, inherited_secret_env, redact_subprocess_output, sanitize_for_jsonb,
    subprocess_secret_values, InheritedSecrets,
};
use lucidos_engine::runtime::python::PythonRuntime;
use lucidos_engine::runtime::spawn_env::{kill_child_process_group_now, spawn_below_engine};
use serde::Serialize;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};

use crate::answer::{parse, HookAnswer, Refusal, RefusalCause};
use crate::input::HookInput;
use crate::{DEFAULT_TIMEOUT_SECS, TIMEOUT_CEILING_SECS};

/// The most stdout an answer may take. An answer is one small JSON object.
const ANSWER_BYTES_CAP: usize = 64 * 1024;

/// The most stderr a failed run keeps, from its end.
const DETAIL_BYTES_CAP: usize = 4 * 1024;

/// How long the pipes may still deliver once the script has exited. What the
/// script itself wrote is already buffered, so this only bounds the reads.
const PIPE_GRACE_AFTER_EXIT: Duration = Duration::from_millis(500);

/// What a hook script runs under. The caller resolves each part the way it
/// does for a trigger script.
pub struct Confinement<'a> {
    pub sandbox: &'a Sandbox,
    pub python: &'a PythonRuntime,
    /// The engine's script environment. It must hold no credential, because
    /// a hook never receives one.
    pub env: &'a [(String, String)],
}

/// The script reads this object on stdin.
#[derive(Serialize)]
struct Stdin<'a> {
    hook: &'a str,
    #[serde(flatten)]
    input: &'a HookInput,
}

/// How long a hook may run: its own timeout, held under the engine-wide
/// ceiling even when the definition skipped validation.
pub fn effective_timeout(timeout_secs: Option<u64>) -> Duration {
    Duration::from_secs(
        timeout_secs
            .unwrap_or(DEFAULT_TIMEOUT_SECS)
            .clamp(1, TIMEOUT_CEILING_SECS),
    )
}

/// Run the hook `slug` of `workspace` on `input`, and return its answer.
pub async fn run_hook(
    workspace: &Path,
    slug: &str,
    definition: &HookDefinition,
    input: &HookInput,
    confinement: &Confinement<'_>,
) -> HookAnswer {
    let stdin = match serde_json::to_vec(&Stdin { hook: slug, input }) {
        Ok(stdin) => stdin,
        Err(e) => return crashed(slug, format!("could not write its input: {e}")),
    };
    // The timeout also bounds a first Python setup, since a person may be
    // waiting on the answer.
    let run = async {
        let child = spawn(workspace, slug, definition, confinement)
            .await
            .map_err(|e| format!("could not start: {e}"))?;
        converse(child, stdin)
            .await
            .map_err(|e| format!("could not be awaited: {e}"))
    };
    let timeout = effective_timeout(definition.timeout_secs);
    // Dropping the run on expiry drops the child, and `kill_on_drop` kills it.
    let finished = match tokio::time::timeout(timeout, run).await {
        Ok(Ok(finished)) => finished,
        Ok(Err(what_went_wrong)) => return crashed(slug, what_went_wrong),
        Err(_) => {
            return refuse(
                RefusalCause::TimedOut,
                format!(
                    "The hook '{slug}' did not answer within {}s.",
                    timeout.as_secs()
                ),
                None,
            )
        }
    };
    let secrets = subprocess_secret_values(confinement.env, inherited_secret_env());
    finished.answer(slug, &secrets)
}

async fn spawn(
    workspace: &Path,
    slug: &str,
    definition: &HookDefinition,
    confinement: &Confinement<'_>,
) -> Result<tokio::process::Child, String> {
    let script = crate::hook_script_path(workspace, slug, &definition.script)?;
    let mut cmd = command(&script, confinement)
        .await
        .map_err(|e| e.to_string())?;
    cmd.current_dir(workspace)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    apply_to_subprocess_env(
        &mut cmd,
        confinement.env,
        InheritedSecrets::Strip(inherited_secret_env()),
    );
    // The script leads its own process group, for `ProcessGroupGuard`.
    spawn_below_engine(&mut cmd).map_err(|e| e.to_string())
}

/// The interpreter for `script`, chosen by its extension as a trigger
/// script's is.
async fn command(
    script: &Path,
    confinement: &Confinement<'_>,
) -> Result<SandboxedCommand, Box<dyn std::error::Error + Send + Sync>> {
    if script.extension().is_some_and(|ext| ext == "py") {
        confinement.python.ensure_venv().await?;
        let mut cmd = confinement
            .sandbox
            .command(confinement.python.python_bin())?;
        cmd.arg(script);
        Ok(cmd)
    } else {
        Ok(command_shell().script(confinement.sandbox, script)?)
    }
}

/// What a script left behind when it exited.
struct Finished {
    outcome: TaskOutcome,
    stdout: Vec<u8>,
    stdout_overflowed: bool,
    stderr: Vec<u8>,
}

/// Kills the script's whole process group when a run is dropped before the
/// script is reaped: on a timeout, or when the asker stops waiting.
/// `kill_on_drop` reaches only the interpreter, never a child it started.
/// Reaping disarms it, since the group id can then be recycled (ADR 0100).
struct ProcessGroupGuard {
    leader: Option<u32>,
    armed: AtomicBool,
}

impl ProcessGroupGuard {
    fn arm(leader: Option<u32>) -> Self {
        Self {
            leader,
            armed: AtomicBool::new(true),
        }
    }

    fn disarm(&self) {
        self.armed.store(false, Ordering::Relaxed);
    }
}

impl Drop for ProcessGroupGuard {
    fn drop(&mut self) {
        if let Some(leader) = self.leader.filter(|_| *self.armed.get_mut()) {
            kill_child_process_group_now(leader);
        }
    }
}

/// Feed the script its input and read it until it exits.
async fn converse(mut child: tokio::process::Child, input: Vec<u8>) -> std::io::Result<Finished> {
    // Declared after `child`, so it drops first, while the script is unreaped.
    let group = ProcessGroupGuard::arm(child.id());
    let stdin = child.stdin.take();
    let write = async move {
        if let Some(mut stdin) = stdin {
            // A hook may answer without reading its input, which closes the
            // pipe under this write. The answer decides, so the error is moot.
            let _ = stdin.write_all(&input).await;
        }
    };
    let (mut stdout_pipe, mut stderr_pipe) = (child.stdout.take(), child.stderr.take());
    let mut stdout = Capture::new(ANSWER_BYTES_CAP, Keep::Head);
    let mut stderr = Capture::new(DETAIL_BYTES_CAP, Keep::Tail);
    let wait = async {
        let status = child.wait().await;
        if status.is_ok() {
            group.disarm();
        }
        status
    };
    let drains = async {
        tokio::join!(
            drain(&mut stdout_pipe, &mut stdout),
            drain(&mut stderr_pipe, &mut stderr)
        )
    };
    let read = async {
        let (mut wait, mut drains) = (pin!(wait), pin!(drains));
        let status = tokio::select! {
            status = &mut wait => status,
            _ = &mut drains => return wait.await,
        };
        // A process the script left running may hold its pipes open. What it
        // writes after the script exited is no part of the answer.
        let _ = tokio::time::timeout(PIPE_GRACE_AFTER_EXIT, drains).await;
        status
    };
    let ((), status) = tokio::join!(write, read);
    // Closing a pipe still held would kill its holder with SIGPIPE.
    if let Some(pipe) = stdout_pipe {
        tokio::spawn(discard(pipe));
    }
    if let Some(pipe) = stderr_pipe {
        tokio::spawn(discard(pipe));
    }
    Ok(Finished {
        outcome: TaskOutcome::from_status(status?),
        stdout: stdout.kept,
        stdout_overflowed: stdout.overflowed,
        stderr: stderr.kept,
    })
}

impl Finished {
    fn answer(self, slug: &str, secrets: &[String]) -> HookAnswer {
        let detail = Some(masked_text(&self.stderr, secrets)).filter(|text| !text.is_empty());
        if !self.outcome.is_success() {
            return refuse(
                RefusalCause::Crashed,
                format!("The hook '{slug}' failed ({}).", self.outcome.describe()),
                detail,
            );
        }
        if self.stdout_overflowed {
            return invalid(
                slug,
                format!("its stdout ran past {ANSWER_BYTES_CAP} bytes"),
                detail,
            );
        }
        let stdout = String::from_utf8_lossy(&self.stdout);
        match parse(&stdout) {
            Ok(answer) => masked_answer(slug, answer, secrets),
            Err(problem) => invalid(slug, redact_subprocess_output(&problem, secrets), detail),
        }
    }
}

/// `answer` with every secret the script could have echoed masked out. A
/// masked event type would name another event, so it makes no answer.
fn masked_answer(slug: &str, answer: HookAnswer, secrets: &[String]) -> HookAnswer {
    let mask = |text: &str| redact_subprocess_output(text, secrets);
    match answer {
        HookAnswer::Allow { events } => {
            if events
                .iter()
                .any(|event| mask(&event.event_type) != event.event_type)
            {
                return invalid(slug, "an event type holds a secret".to_string(), None);
            }
            HookAnswer::Allow {
                events: events
                    .into_iter()
                    .map(|event| event.masked(secrets))
                    .collect(),
            }
        }
        HookAnswer::NeedsInput { argument } => HookAnswer::NeedsInput {
            argument: mask(&argument),
        },
        HookAnswer::Refuse(refusal) => HookAnswer::Refuse(Refusal {
            reason: mask(&refusal.reason),
            ..refusal
        }),
    }
}

fn masked_text(bytes: &[u8], secrets: &[String]) -> String {
    let text = sanitize_for_jsonb(String::from_utf8_lossy(bytes).trim());
    redact_subprocess_output(&text, secrets)
}

fn refuse(cause: RefusalCause, reason: String, detail: Option<String>) -> HookAnswer {
    HookAnswer::Refuse(Refusal {
        cause,
        reason,
        detail,
    })
}

fn crashed(slug: &str, what_went_wrong: String) -> HookAnswer {
    refuse(
        RefusalCause::Crashed,
        format!("The hook '{slug}' {what_went_wrong}"),
        None,
    )
}

fn invalid(slug: &str, why: String, detail: Option<String>) -> HookAnswer {
    refuse(
        RefusalCause::InvalidAnswer,
        format!("The hook '{slug}' gave no answer: {why}"),
        detail,
    )
}

#[derive(Clone, Copy)]
enum Keep {
    Head,
    Tail,
}

/// What a pipe carried: at most `cap` bytes from the end `keep` names, and
/// whether it carried more.
struct Capture {
    kept: Vec<u8>,
    overflowed: bool,
    cap: usize,
    keep: Keep,
}

impl Capture {
    fn new(cap: usize, keep: Keep) -> Self {
        Self {
            kept: Vec::new(),
            overflowed: false,
            cap,
            keep,
        }
    }

    fn take_in(&mut self, chunk: &[u8]) {
        self.kept.extend_from_slice(chunk);
        if self.kept.len() > self.cap {
            self.overflowed = true;
            match self.keep {
                Keep::Head => self.kept.truncate(self.cap),
                Keep::Tail => {
                    let excess = self.kept.len() - self.cap;
                    self.kept.drain(..excess);
                }
            }
        }
    }
}

/// Read `pipe` into `capture` until it ends, then take the pipe. Dropping
/// this future early leaves the pipe in place, since a read is cancel safe.
async fn drain(pipe: &mut Option<impl AsyncRead + Unpin>, capture: &mut Capture) {
    let Some(reader) = pipe.as_mut() else {
        return;
    };
    let mut chunk = [0u8; 8192];
    // A read error ends the stream like EOF does. What was kept still stands.
    while let Ok(n @ 1..) = reader.read(&mut chunk).await {
        capture.take_in(&chunk[..n]);
    }
    *pipe = None;
}

/// Read `pipe` to its end and throw the bytes away.
async fn discard(mut pipe: impl AsyncRead + Unpin) {
    let mut chunk = [0u8; 8192];
    while let Ok(1..) = pipe.read(&mut chunk).await {}
}

#[cfg(test)]
#[path = "runner_tests.rs"]
mod tests;
