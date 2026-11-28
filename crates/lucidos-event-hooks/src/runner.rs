use std::collections::BTreeSet;
use std::path::Path;
use std::pin::pin;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use lucidos_core::core::agent_sandbox::{Sandbox, SandboxedCommand};
use lucidos_core::core::event_hooks::{
    effective_timeout, HookChoice, HookDefinition, Refusal, RefusalCause, HOOK_CHOICE_LIMIT,
};
use lucidos_core::core::shell::{command_shell, TaskOutcome};
use lucidos_core::core::{
    apply_to_subprocess_env, inherited_secret_env, redact_subprocess_output, sanitize_for_jsonb,
    subprocess_secret_values, InheritedSecrets,
};
use lucidos_engine::runtime::python::PythonRuntime;
use lucidos_engine::runtime::spawn_env::{kill_child_process_group_now, spawn_below_engine};
use serde::Serialize;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};

use crate::answer::{masked_event, parse, HookAnswer};
use crate::input::HookInput;

const ANSWER_BYTES_CAP: usize = 64 * 1024;

const DETAIL_BYTES_CAP: usize = 4 * 1024;

const PIPE_GRACE_AFTER_EXIT: Duration = Duration::from_millis(500);

pub struct Confinement<'a> {
    pub sandbox: &'a Sandbox,
    pub python: &'a PythonRuntime,
    pub env: &'a [(String, String)],
}

#[derive(Serialize)]
struct Stdin<'a> {
    hook: &'a str,
    #[serde(flatten)]
    input: &'a HookInput,
}

pub async fn run_hook(
    workspace: &Path,
    hook_dir: &Path,
    slug: &str,
    definition: &HookDefinition,
    input: &HookInput,
    confinement: &Confinement<'_>,
) -> HookAnswer {
    let stdin = match serde_json::to_vec(&Stdin { hook: slug, input }) {
        Ok(stdin) => stdin,
        Err(e) => return crashed(slug, format!("could not write its input: {e}")),
    };
    let finished = match run_script(
        workspace,
        hook_dir,
        &definition.script,
        definition.timeout_secs,
        stdin,
        confinement,
    )
    .await
    {
        Ok(finished) => finished,
        Err(RunFailure::Process(error)) => return crashed(slug, error),
        Err(RunFailure::Timeout(timeout)) => {
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

pub(crate) async fn run_choices(
    workspace: &Path,
    hook_dir: &Path,
    script: &str,
    timeout_secs: Option<u64>,
    input: serde_json::Value,
    confinement: &Confinement<'_>,
    secrets: &[String],
) -> Result<Vec<HookChoice>, String> {
    let mut input = input;
    lucidos_core::core::redact_subprocess_output_in_json(&mut input, secrets);
    let stdin = serde_json::to_vec(&input).map_err(|_| "Could not encode the choices query.")?;
    let output = run_script(
        workspace,
        hook_dir,
        script,
        timeout_secs,
        stdin,
        confinement,
    )
    .await
    .map_err(|failure| match failure {
        RunFailure::Process(error) => {
            format!(
                "The choices script {}.",
                redact_subprocess_output(&error, secrets)
            )
        }
        RunFailure::Timeout(timeout) => {
            format!("The choices script timed out after {}s.", timeout.as_secs())
        }
    })?;
    if !output.outcome.is_success() {
        return Err(format!(
            "The choices script failed ({}).",
            output.outcome.describe()
        ));
    }
    if output.stdout_overflowed {
        return Err("The choices script returned too much data.".into());
    }
    parse_choices(&output.stdout, secrets)
}

fn parse_choices(stdout: &[u8], secrets: &[String]) -> Result<Vec<HookChoice>, String> {
    let choices: Vec<HookChoice> = serde_json::from_slice(stdout)
        .map_err(|_| "The choices script must return a JSON array of values and labels.")?;
    if choices.len() > HOOK_CHOICE_LIMIT {
        return Err(format!(
            "The choices script may return at most {HOOK_CHOICE_LIMIT} rows."
        ));
    }
    let mut values = BTreeSet::new();
    let mut visible = Vec::new();
    for mut choice in choices {
        if choice.value.trim().is_empty()
            || choice.label.trim().is_empty()
            || choice.value.contains('\0')
            || choice.label.contains('\0')
        {
            return Err("Each choice needs a nonblank value and label.".into());
        }
        if !values.insert(choice.value.clone()) {
            return Err("The choices script returned a duplicate value.".into());
        }
        if redact_subprocess_output(&choice.value, secrets) != choice.value {
            continue;
        }
        choice.label = redact_subprocess_output(&choice.label, secrets);
        visible.push(choice);
    }
    Ok(visible)
}

enum RunFailure {
    Process(String),
    Timeout(Duration),
}

async fn run_script(
    workspace: &Path,
    hook_dir: &Path,
    script: &str,
    timeout_secs: Option<u64>,
    stdin: Vec<u8>,
    confinement: &Confinement<'_>,
) -> Result<Finished, RunFailure> {
    let run = async {
        let child = spawn(workspace, hook_dir, script, confinement)
            .await
            .map_err(|e| format!("could not start: {e}"))?;
        converse(child, stdin)
            .await
            .map_err(|e| format!("could not be awaited: {e}"))
    };
    let timeout = effective_timeout(timeout_secs);
    tokio::time::timeout(timeout, run)
        .await
        .map_err(|_| RunFailure::Timeout(timeout))?
        .map_err(RunFailure::Process)
}

async fn spawn(
    workspace: &Path,
    hook_dir: &Path,
    script: &str,
    confinement: &Confinement<'_>,
) -> Result<tokio::process::Child, String> {
    let script = crate::check_script(hook_dir, script)?;
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
    spawn_below_engine(&mut cmd).map_err(|e| e.to_string())
}

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

struct Finished {
    outcome: TaskOutcome,
    stdout: Vec<u8>,
    stdout_overflowed: bool,
    stderr: Vec<u8>,
}

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

async fn converse(mut child: tokio::process::Child, input: Vec<u8>) -> std::io::Result<Finished> {
    let group = ProcessGroupGuard::arm(child.id());
    let stdin = child.stdin.take();
    let write = async move {
        if let Some(mut stdin) = stdin {
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
        let _ = tokio::time::timeout(PIPE_GRACE_AFTER_EXIT, drains).await;
        status
    };
    let ((), status) = tokio::join!(write, read);
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
                    .map(|event| masked_event(event, secrets))
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

async fn drain(pipe: &mut Option<impl AsyncRead + Unpin>, capture: &mut Capture) {
    let Some(reader) = pipe.as_mut() else {
        return;
    };
    let mut chunk = [0u8; 8192];
    while let Ok(n @ 1..) = reader.read(&mut chunk).await {
        capture.take_in(&chunk[..n]);
    }
    *pipe = None;
}

async fn discard(mut pipe: impl AsyncRead + Unpin) {
    let mut chunk = [0u8; 8192];
    while let Ok(1..) = pipe.read(&mut chunk).await {}
}

#[cfg(test)]
#[path = "runner_tests.rs"]
mod tests;
