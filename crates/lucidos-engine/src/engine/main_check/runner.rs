//! The child process that runs one check: `scripts/main-check.sh <sha>`.
//!
//! It runs in its own process group, detached from any request, and reports
//! to the driver over its channel. Its stdout carries the protocol in
//! `scripts/lib/main_check.sh`: a run line once the workflow run starts, then
//! one verdict line. Engine shutdown stops it through [`RunnerSlot::stop`].
//! Once the run is on record, SIGUSR2 detaches the script and leaves the run
//! going: the next start reattaches to it. Before that, SIGTERM has the script
//! cancel its run and delete its branch, and the next start abandons the check.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::driver::MainCheckMsg;
use super::state::FailedTest;

/// What runs a check, and from where.
#[derive(Debug, Clone)]
pub(crate) struct MainCheckCommand {
    pub(crate) program: PathBuf,
    pub(crate) cwd: PathBuf,
}

/// Must match `MAIN_CHECK_RUN_PREFIX` in `scripts/lib/main_check.sh`.
const RUN_PREFIX: &str = "main-check run ";
/// Must match `MAIN_CHECK_VERDICT_PREFIX` in `scripts/lib/main_check.sh`.
const VERDICT_PREFIX: &str = "main-check verdict ";
/// How long the script's own cleanup gets after the stop: one `gh run cancel`
/// and one branch delete, or nothing on a detach. The engine starts the stop
/// early in its teardown, so this fits inside the supervisor's force-kill window.
const STOP_GRACE: Duration = Duration::from_secs(10);

/// How a check's run ended, as the script reported it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct RunReport {
    pub(crate) verdict: ReportedVerdict,
    #[serde(default)]
    pub(crate) run_url: Option<String>,
    #[serde(default)]
    pub(crate) summary: String,
    #[serde(default)]
    pub(crate) tail: String,
    #[serde(default)]
    pub(crate) failures: Vec<FailedTest>,
    #[serde(default)]
    pub(crate) flaky: Vec<FailedTest>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ReportedVerdict {
    Pass,
    Fail,
    NoVerdict,
}

/// The workflow run a check's child started: its page, and the mirror branch
/// it runs on. Reattaching needs both.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RunLink {
    pub(crate) run_url: String,
    pub(crate) branch: String,
}

/// What a child checks: a commit from the start, or a commit whose run an
/// earlier engine left going.
#[derive(Debug, Clone, Copy)]
pub(crate) enum RunTarget<'a> {
    Commit(&'a str),
    Attach { sha: &'a str, link: &'a RunLink },
}

/// One protocol line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ProtocolLine {
    RunStarted(RunLink),
    Verdict(RunReport),
}

/// Read one stdout line. Anything else the script prints is not protocol.
pub(crate) fn parse_line(line: &str) -> Option<ProtocolLine> {
    if let Some(rest) = line.strip_prefix(RUN_PREFIX) {
        let mut words = rest.split_whitespace();
        let (Some(run_url), Some(branch), None) = (words.next(), words.next(), words.next()) else {
            return None;
        };
        return Some(ProtocolLine::RunStarted(RunLink {
            run_url: run_url.to_string(),
            branch: branch.to_string(),
        }));
    }
    let json = line.strip_prefix(VERDICT_PREFIX)?;
    serde_json::from_str(json).ok().map(ProtocolLine::Verdict)
}

/// A report for a run that ended without one.
pub(crate) fn no_verdict(summary: String) -> RunReport {
    RunReport {
        verdict: ReportedVerdict::NoVerdict,
        run_url: None,
        summary,
        tail: String::new(),
        failures: Vec::new(),
        flaky: Vec::new(),
    }
}

struct RunnerHandle {
    check_id: Uuid,
    /// The check's run is on record, so a stop detaches instead of cancelling.
    linked: Arc<AtomicBool>,
    cancel: CancellationToken,
    done: tokio::task::JoinHandle<()>,
}

/// The running check's child, held so engine shutdown can stop it. It holds
/// at most one: the scheduler runs one check at a time.
pub(crate) struct RunnerSlot {
    current: std::sync::Mutex<Option<RunnerHandle>>,
    /// How long a stopped child gets to clean up before SIGKILL.
    grace: Duration,
}

impl Default for RunnerSlot {
    fn default() -> Self {
        Self::with_grace(STOP_GRACE)
    }
}

impl RunnerSlot {
    pub(crate) fn with_grace(grace: Duration) -> Self {
        Self {
            current: std::sync::Mutex::default(),
            grace,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Option<RunnerHandle>> {
        self.current
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn take(&self) -> Option<RunnerHandle> {
        self.lock().take()
    }

    /// Let go of `check_id`'s child once it has exited.
    fn release(&self, check_id: Uuid) {
        let mut slot = self.lock();
        if slot.as_ref().is_some_and(|held| held.check_id == check_id) {
            *slot = None;
        }
    }

    /// `check_id`'s run is on record: from now on a stop detaches from it.
    pub(crate) fn mark_linked(&self, check_id: Uuid) {
        if let Some(held) = self.lock().as_ref().filter(|h| h.check_id == check_id) {
            held.linked.store(true, Ordering::SeqCst);
        }
    }

    /// Stop the running check's child and wait for it to go. It reports
    /// nothing: the next start finds the check running, and reattaches to a
    /// run on record or abandons the check.
    pub(crate) async fn stop(&self) {
        let Some(handle) = self.take() else {
            return;
        };
        handle.cancel.cancel();
        if tokio::time::timeout(self.grace * 2, handle.done)
            .await
            .is_err()
        {
            log!("[MainCheck] the check's child did not stop in time");
        }
    }
}

/// Start the child for `check_id` on `target`. Its run line and its verdict
/// come back over `tx`. While another check's child still runs, nothing starts
/// and the check ends with no verdict: two children never run at once.
pub(crate) fn spawn_runner(
    command: &MainCheckCommand,
    check_id: Uuid,
    target: RunTarget<'_>,
    tx: UnboundedSender<MainCheckMsg>,
    slot: &Arc<RunnerSlot>,
) {
    // Held until the handle is in, so the child's own release cannot come
    // first and leave a finished handle behind.
    let mut held = slot.lock();
    if let Some(running) = held.as_ref() {
        log!(
            "[MainCheck] check {}'s child still runs; check {} starts no second one",
            running.check_id,
            check_id
        );
        let report = no_verdict("Another check's child was still running.".into());
        let _ = tx.send(MainCheckMsg::Ended { check_id, report });
        return;
    }
    let mut cmd = tokio::process::Command::new(&command.program);
    match target {
        RunTarget::Commit(sha) => cmd.arg(sha),
        RunTarget::Attach { sha, link } => cmd.args([sha, "--attach", &link.run_url, &link.branch]),
    };
    cmd.current_dir(&command.cwd)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    crate::runtime::spawn_env::isolate_in_process_group(&mut cmd);
    let child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => {
            let report = no_verdict(format!(
                "Could not start {}: {e}",
                command.program.display()
            ));
            let _ = tx.send(MainCheckMsg::Ended { check_id, report });
            return;
        }
    };
    let cancel = CancellationToken::new();
    let linked = Arc::new(AtomicBool::new(matches!(target, RunTarget::Attach { .. })));
    let child = RunningChild {
        child,
        check_id,
        linked: linked.clone(),
    };
    let done = tokio::spawn(run_child(child, tx, cancel.clone(), slot.clone()));
    *held = Some(RunnerHandle {
        check_id,
        linked,
        cancel,
        done,
    });
}

struct RunningChild {
    child: tokio::process::Child,
    check_id: Uuid,
    linked: Arc<AtomicBool>,
}

async fn run_child(
    running: RunningChild,
    tx: UnboundedSender<MainCheckMsg>,
    cancel: CancellationToken,
    slot: Arc<RunnerSlot>,
) {
    let RunningChild {
        mut child,
        check_id,
        linked,
    } = running;
    let pid = child.id();
    let mut lines = child.stdout.take().map(|out| BufReader::new(out).lines());
    let mut report = None;
    loop {
        let next = async {
            match lines.as_mut() {
                Some(lines) => lines.next_line().await.ok().flatten(),
                None => None,
            }
        };
        tokio::select! {
            () = cancel.cancelled() => {
                let detach = linked.load(Ordering::SeqCst);
                stop_child(&mut child, pid, slot.grace, detach).await;
                return;
            }
            line = next => match line.as_deref().and_then(parse_line) {
                Some(ProtocolLine::RunStarted(link)) => {
                    let _ = tx.send(MainCheckMsg::RunLinked { check_id, link });
                }
                Some(ProtocolLine::Verdict(verdict)) => report = Some(verdict),
                None if line.is_none() => break,
                None => {}
            },
        }
    }
    let status = child.wait().await;
    slot.release(check_id);
    let report = report.unwrap_or_else(|| {
        no_verdict(match status {
            Ok(status) => format!("main-check.sh ended without a verdict ({status})."),
            Err(e) => format!("main-check.sh could not be waited on: {e}"),
        })
    });
    let _ = tx.send(MainCheckMsg::Ended { check_id, report });
}

/// Signal the child's group, SIGUSR2 to `detach` and SIGTERM to cancel, and
/// wait up to `grace` for the script to finish and exit. Only then SIGKILL
/// what is left. The group is signalled only while the leader is unreaped, so
/// its pid cannot be reused.
async fn stop_child(
    child: &mut tokio::process::Child,
    pid: Option<u32>,
    grace: Duration,
    detach: bool,
) {
    let Some(pid) = pid else {
        let _ = child.wait().await;
        return;
    };
    if detach {
        crate::runtime::spawn_env::detach_child_process_group(pid);
    } else {
        crate::runtime::spawn_env::terminate_child_process_group(pid);
    }
    if tokio::time::timeout(grace, child.wait()).await.is_err() {
        log!("[MainCheck] the check's child outlived its grace; killing it");
        crate::runtime::spawn_env::kill_child_process_group_now(pid);
        let _ = child.wait().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_run_line_names_the_run_and_its_branch() {
        assert_eq!(
            parse_line(
                "main-check run https://github.com/o/r/actions/runs/1 e2e/20261019-120000-ab"
            ),
            Some(ProtocolLine::RunStarted(RunLink {
                run_url: "https://github.com/o/r/actions/runs/1".into(),
                branch: "e2e/20261019-120000-ab".into(),
            }))
        );
        assert_eq!(parse_line("main-check run "), None);
        assert_eq!(
            parse_line("main-check run https://github.com/o/r/actions/runs/1"),
            None,
            "a run with no branch cannot be reattached"
        );
        assert_eq!(parse_line("main-check run u b extra"), None);
    }

    #[test]
    fn the_verdict_line_carries_every_field() {
        let line = r#"main-check verdict {"verdict": "fail", "run_url": "https://r", "summary": "Failed: engine.", "tail": "t", "failures": [{"suite": "engine", "name": "a::b", "detail": "d"}], "log": "/x"}"#;
        assert_eq!(
            parse_line(line),
            Some(ProtocolLine::Verdict(RunReport {
                verdict: ReportedVerdict::Fail,
                run_url: Some("https://r".into()),
                summary: "Failed: engine.".into(),
                tail: "t".into(),
                failures: vec![FailedTest {
                    suite: "engine".into(),
                    name: "a::b".into(),
                    detail: "d".into(),
                }],
                flaky: Vec::new(),
            }))
        );
        let none = r#"main-check verdict {"verdict": "no-verdict", "run_url": null, "summary": "s", "tail": ""}"#;
        assert!(matches!(
            parse_line(none),
            Some(ProtocolLine::Verdict(RunReport {
                verdict: ReportedVerdict::NoVerdict,
                run_url: None,
                ..
            }))
        ));
    }

    #[test]
    fn other_output_and_a_garbled_verdict_are_not_protocol() {
        assert_eq!(parse_line("[e2e-github] run x: remote engine"), None);
        assert_eq!(parse_line("main-check verdict {not json"), None);
        assert_eq!(
            parse_line(r#"main-check verdict {"verdict": "maybe"}"#),
            None
        );
    }

    /// The prefixes are the shell library's, word for word.
    #[test]
    fn the_prefixes_match_the_shell_library() {
        let lib = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../scripts/lib/main_check.sh"),
        )
        .expect("scripts/lib/main_check.sh");
        assert!(lib.contains(&format!("MAIN_CHECK_RUN_PREFIX=\"{RUN_PREFIX}\"")));
        assert!(lib.contains(&format!("MAIN_CHECK_VERDICT_PREFIX=\"{VERDICT_PREFIX}\"")));
    }
}
