//! The main check's driver: one task owns the scheduler and takes one message
//! at a time, so a landing, a verdict and a Close never race.
//!
//! It emits the main check's events, runs each check's child, opens the fix
//! thread, and publishes the snapshot. What it needs from the engine comes in
//! as [`DriverHooks`], so tests drive it with a fake check script and a real
//! event log.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::{mpsc::UnboundedSender, oneshot};
use uuid::Uuid;

use super::fix_thread::{build_fix_thread_prompt, fix_thread_title, landed_changes_in_range};
use super::recovery::{fold, MainCheckRecord};
use super::runner::{
    no_verdict, spawn_runner, MainCheckCommand, ReportedVerdict, RunReport, RunnerSlot,
};
use super::snapshot::MainCheckSnapshot;
use super::state::{CheckRange, CloseRefusal, Effect, MainCheck, RedCheck, RedCloseCause, Verdict};
use super::RedCloseReason;
use crate::engine::event_bus::{BusEvent, EventBus, SystemEvent};
use crate::engine::git_ops::{branch_head_sha, git_answer, GitAnswer};
use crate::engine::thread_events::MessageOrigin;
use crate::engine::until_restored::UntilRestored;

/// What the driver hears.
pub(crate) enum MainCheckMsg {
    /// A Lucidos-source change from `thread_id` landed on `main`. `pre_sha` is
    /// `main` before it.
    Landed {
        thread_id: Uuid,
        pre_sha: Option<String>,
    },
    /// The running check's workflow run started.
    RunLinked { check_id: Uuid, run_url: String },
    /// The running check's child ended.
    Ended { check_id: Uuid, report: RunReport },
    /// The maintainer closes the red.
    Close {
        actor: Option<MessageOrigin>,
        reply: oneshot::Sender<Result<(), CloseRefusal>>,
    },
    /// Rebuild from events after a restart. The driver handles nothing
    /// before it succeeds (`UntilRestored`): a landing that came first would
    /// start a check on an empty scheduler, even while main is red.
    Restore,
}

/// What the driver asks of the engine.
pub(crate) struct DriverHooks {
    /// Send a fix thread its first prompt: thread id, prompt, title.
    pub(crate) open_fix_thread: Box<dyn Fn(Uuid, String, String) + Send + Sync>,
    /// Publish a changed snapshot to clients.
    pub(crate) publish: Box<dyn Fn(MainCheckSnapshot) + Send + Sync>,
}

/// Where the driver works.
pub(crate) struct DriverSetup {
    pub(crate) pool: sqlx::PgPool,
    pub(crate) bus: EventBus,
    pub(crate) repo_root: PathBuf,
    pub(crate) command: MainCheckCommand,
    /// For the thread links in a fix thread's prompt.
    pub(crate) workspace_name: String,
    pub(crate) tx: UnboundedSender<MainCheckMsg>,
    pub(crate) runner: Arc<RunnerSlot>,
}

/// The reason a check the engine's restart cut off is abandoned with.
pub(crate) const ENGINE_RESTARTED: &str = "The engine restarted while the check ran.";
/// The mirror's remote, as `E2E_GITHUB_REMOTE` in `scripts/lib/e2e_github.sh`
/// names it.
const MIRROR_REMOTE: &str = "lucidos";
/// The release library GitHub mode strips with. The public mirror lacks it.
const RELEASE_LIB: &str = "scripts/lib/release_tree.sh";

pub(crate) struct Driver {
    setup: DriverSetup,
    hooks: DriverHooks,
    check: MainCheck,
    /// Why this checkout cannot run a check, read again at each landing.
    unavailable: Option<String>,
    /// The running check's workflow run, once its child names it.
    run_url: Option<String>,
    published: Option<MainCheckSnapshot>,
    /// `main` as git last read it. `None` once a landing moved it unread.
    main: Option<String>,
    /// Every message waits here until recovery restored the check.
    until_restored: UntilRestored<MainCheckMsg>,
    /// The restore came, but its recovery query failed. The gate stays shut,
    /// and each message retries the restore first.
    restore_failed: bool,
}

impl Driver {
    pub(crate) fn new(setup: DriverSetup, hooks: DriverHooks) -> Self {
        Self {
            setup,
            hooks,
            check: MainCheck::default(),
            unavailable: None,
            run_url: None,
            published: None,
            main: None,
            until_restored: UntilRestored::default(),
            restore_failed: false,
        }
    }

    /// Take one message off the channel. Until a restore succeeds it only
    /// waits.
    pub(crate) async fn deliver(&mut self, msg: MainCheckMsg) {
        if self.restore_failed && !matches!(msg, MainCheckMsg::Restore) {
            self.pass_gate(MainCheckMsg::Restore).await;
        }
        if self.restore_failed {
            self.hold_after_failed_restore(msg);
            return;
        }
        self.pass_gate(msg).await;
    }

    /// Keep a message until a restore succeeds. A Close answers at once
    /// instead: held, the maintainer's request would wait for the next landing.
    fn hold_after_failed_restore(&mut self, msg: MainCheckMsg) {
        match msg {
            MainCheckMsg::Close { reply, .. } => {
                let _ = reply.send(Err(CloseRefusal::StateUnread));
            }
            msg => {
                self.until_restored.admit(msg, false);
            }
        }
    }

    async fn pass_gate(&mut self, msg: MainCheckMsg) {
        let is_restore = matches!(msg, MainCheckMsg::Restore);
        let mut admitted = self.until_restored.admit(msg, is_restore).into_iter();
        while let Some(msg) = admitted.next() {
            self.handle(msg).await;
            if self.restore_failed {
                // What waited must not meet the empty scheduler a failed
                // restore leaves.
                self.until_restored = UntilRestored::default();
                for held in admitted.by_ref() {
                    self.hold_after_failed_restore(held);
                }
            }
        }
    }

    pub(crate) async fn handle(&mut self, msg: MainCheckMsg) {
        match msg {
            MainCheckMsg::Landed { thread_id, pre_sha } => self.landed(thread_id, pre_sha).await,
            MainCheckMsg::RunLinked { check_id, run_url } => {
                if self.check.running().is_some_and(|r| r.check_id == check_id) {
                    self.run_url = Some(run_url);
                }
            }
            MainCheckMsg::Ended { check_id, report } => self.ended(check_id, report).await,
            MainCheckMsg::Close { actor, reply } => {
                let answer = self.close(actor).await;
                let _ = reply.send(answer);
            }
            MainCheckMsg::Restore => self.restore().await,
        }
        self.publish();
    }

    fn publish(&mut self) {
        let snapshot = MainCheckSnapshot::of(
            &self.check,
            self.unavailable.as_deref(),
            self.run_url.as_deref(),
        );
        if self.published.as_ref() != Some(&snapshot) {
            (self.hooks.publish)(snapshot.clone());
            self.published = Some(snapshot);
        }
    }

    /// Every landing reaches the scheduler, so a fix still closes its red and
    /// nothing that landed is forgotten. Only starting a check needs `main`
    /// and a checkout that can run one.
    async fn landed(&mut self, thread_id: Uuid, pre_sha: Option<String>) {
        self.unavailable =
            unavailable_reason(&self.setup.repo_root, &self.setup.command.program).await;
        self.main = None;
        let main = match &self.unavailable {
            Some(reason) => {
                log!(
                    "[MainCheck] main moved, but no check can run here: {}",
                    reason
                );
                None
            }
            None => {
                let main = self.read_main().await;
                if main.is_none() {
                    log!("[MainCheck] main moved, but git could not read it; no check starts");
                }
                main
            }
        };
        let from_fix = self
            .check
            .red()
            .is_some_and(|r| r.fix_thread_id == thread_id);
        let effects = self.check.landed(
            pre_sha.as_deref(),
            from_fix,
            main.as_deref(),
            Uuid::new_v4(),
        );
        self.carry_out(effects, None).await;
    }

    async fn ended(&mut self, check_id: Uuid, report: RunReport) {
        if self.check.running().is_none_or(|r| r.check_id != check_id) {
            log!(
                "[MainCheck] a verdict for check {} came after it ended",
                check_id
            );
            return;
        }
        self.run_url = None;
        let run_url = report.run_url.clone();
        let verdict = match report.verdict {
            ReportedVerdict::Pass => {
                self.emit(SystemEvent::MainCheckPassed { check_id, run_url })
                    .await;
                Verdict::Passed
            }
            ReportedVerdict::Fail => {
                let fix_thread_id = Uuid::new_v4();
                self.emit(SystemEvent::MainCheckFailed {
                    check_id,
                    summary: report.summary.clone(),
                    tail: report.tail.clone(),
                    run_url: run_url.clone(),
                    fix_thread_id,
                })
                .await;
                Verdict::Failed {
                    summary: report.summary,
                    tail: report.tail,
                    run_url,
                    fix_thread_id,
                }
            }
            ReportedVerdict::NoVerdict => {
                self.emit(SystemEvent::MainCheckAbandoned {
                    check_id,
                    reason: report.summary,
                    run_url,
                })
                .await;
                Verdict::Abandoned
            }
        };
        let main = self.main_now().await;
        let effects = self
            .check
            .ended(check_id, verdict, main.as_deref(), Uuid::new_v4());
        if let Some(red) = self
            .check
            .red()
            .filter(|r| r.range.check_id == check_id)
            .cloned()
        {
            log!("[MainCheck] main is red: {}", red.summary);
            self.open_fix_thread(&red).await;
        }
        self.carry_out(effects, None).await;
    }

    async fn close(&mut self, actor: Option<MessageOrigin>) -> Result<(), CloseRefusal> {
        if self.check.red().is_none() {
            return Err(CloseRefusal::NoRed);
        }
        let main = self.main_now().await;
        let effects = self.check.close(main.as_deref(), Uuid::new_v4())?;
        self.carry_out(effects, actor).await;
        Ok(())
    }

    /// Rebuild from events. A check the restart cut off is abandoned. A red
    /// whose fix already landed closes. A fix thread that never got its prompt
    /// gets it now, unless the maintainer deleted it. Nothing here starts a
    /// check. A failed recovery query leaves the restore to the next message.
    pub(crate) async fn restore(&mut self) {
        self.unavailable =
            unavailable_reason(&self.setup.repo_root, &self.setup.command.program).await;
        let records = match load_records(&self.setup.pool).await {
            Ok(records) => records,
            Err(e) => {
                log!(
                    "[MainCheck] recovery query failed, the next message retries it: {}",
                    e
                );
                self.restore_failed = true;
                return;
            }
        };
        self.restore_failed = false;
        let recovered = fold(&records);
        self.check = recovered.check;
        self.run_url = None;
        if let Some(range) = recovered.interrupted {
            log!(
                "[MainCheck] check {} was running at restart; abandoning it",
                range.check_id
            );
            self.emit(SystemEvent::MainCheckAbandoned {
                check_id: range.check_id,
                reason: ENGINE_RESTARTED.into(),
                run_url: None,
            })
            .await;
        }
        if let Some(check_id) = recovered.fix_landed {
            if self.check.heal_fix_landed() == Some(check_id) {
                self.emit_red_closed(check_id, RedCloseCause::FixLanded, None)
                    .await;
            }
        }
        let Some(red) = self.check.red().cloned() else {
            return;
        };
        match fix_thread_trace(&self.setup.pool, red.fix_thread_id).await {
            Ok(FixThreadTrace::Unprompted) => self.open_fix_thread(&red).await,
            Ok(FixThreadTrace::Prompted) => {}
            Ok(FixThreadTrace::Deleted) => log!(
                "[MainCheck] the maintainer deleted fix thread {}; the red waits on Close",
                red.fix_thread_id
            ),
            Err(e) => log!(
                "[MainCheck] could not read fix thread {}: {}",
                red.fix_thread_id,
                e
            ),
        }
    }

    async fn carry_out(&mut self, effects: Vec<Effect>, actor: Option<MessageOrigin>) {
        for effect in effects {
            match effect {
                Effect::CloseRed { check_id, cause } => {
                    self.emit_red_closed(check_id, cause, actor.clone()).await;
                }
                Effect::Start(range) => self.start(range).await,
            }
        }
    }

    async fn start(&mut self, range: CheckRange) {
        log!(
            "[MainCheck] check {} starts on {}..{}",
            range.check_id,
            range.from_sha,
            range.to_sha
        );
        self.run_url = None;
        self.emit(SystemEvent::MainCheckStarted {
            check_id: range.check_id,
            from_sha: range.from_sha.clone(),
            to_sha: range.to_sha.clone(),
        })
        .await;
        match &self.unavailable {
            Some(reason) => {
                let report = no_verdict(reason.clone());
                let _ = self.setup.tx.send(MainCheckMsg::Ended {
                    check_id: range.check_id,
                    report,
                });
            }
            None => spawn_runner(
                &self.setup.command,
                range.check_id,
                &range.to_sha,
                self.setup.tx.clone(),
                &self.setup.runner,
            ),
        }
    }

    async fn open_fix_thread(&self, red: &RedCheck) {
        let landed = landed_changes_in_range(
            &self.setup.pool,
            &self.setup.repo_root,
            &red.range.from_sha,
            &red.range.to_sha,
        )
        .await;
        let prompt = build_fix_thread_prompt(red, landed.as_deref(), &self.setup.workspace_name);
        (self.hooks.open_fix_thread)(red.fix_thread_id, prompt, fix_thread_title(&red.summary));
    }

    async fn emit_red_closed(
        &self,
        check_id: Uuid,
        cause: RedCloseCause,
        actor: Option<MessageOrigin>,
    ) {
        let reason = match cause {
            RedCloseCause::FixLanded => RedCloseReason::FixLanded,
            RedCloseCause::Closed => RedCloseReason::Closed,
        };
        self.emit(SystemEvent::MainCheckRedClosed {
            check_id,
            reason,
            actor,
        })
        .await;
    }

    async fn emit(&self, event: SystemEvent) {
        let label = format!("[MainCheck] {}", event.event_type());
        self.setup
            .bus
            .emit_or_log(BusEvent::System(event), &label)
            .await;
    }

    /// `main` as git reads it now, kept for [`Self::main_now`]. `None` when
    /// git could not read it.
    async fn read_main(&mut self) -> Option<String> {
        let read = branch_head_sha(&self.setup.repo_root, "main").await;
        if read.is_some() {
            self.main = read.clone();
        }
        read
    }

    /// `main` now, or as git last read it when git cannot read it now. Only
    /// a landing moves `main`, and each landing reads it again.
    async fn main_now(&mut self) -> Option<String> {
        let read = self.read_main().await;
        read.or_else(|| self.main.clone())
    }
}

/// Why this checkout cannot run the main check, or `None` when it can: it
/// needs the check script, the private release library and the mirror's
/// remote.
pub(crate) async fn unavailable_reason(repo_root: &Path, program: &Path) -> Option<String> {
    if !program.is_file() {
        return Some(format!("this checkout has no {}", program.display()));
    }
    if !repo_root.join(RELEASE_LIB).is_file() {
        return Some(format!(
            "this checkout has no {RELEASE_LIB}, which the public mirror does not ship"
        ));
    }
    let has_remote = git_answer(&["remote", "get-url", MIRROR_REMOTE], repo_root).await;
    missing_remote_reason(has_remote)
}

/// Why the mirror's remote rules a check out. An unknown answer is no
/// missing remote: a check that cannot push only costs a no-verdict run.
fn missing_remote_reason(has_remote: GitAnswer) -> Option<String> {
    if has_remote.or_unknown(true) {
        return None;
    }
    Some(format!(
        "there is no '{MIRROR_REMOTE}' remote for the public mirror"
    ))
}

/// What the event log holds of a red's fix thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FixThreadTrace {
    /// It has events, so its prompt went out.
    Prompted,
    /// The maintainer deleted it. `ThreadsDeleted` is the only trace a delete
    /// leaves, so the thread must not come back.
    Deleted,
    /// Nothing at all: the restart came before its prompt.
    Unprompted,
}

async fn fix_thread_trace(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
) -> Result<FixThreadTrace, sqlx::Error> {
    let (prompted, deleted): (bool, bool) = sqlx::query_as(
        "SELECT EXISTS (SELECT 1 FROM events WHERE thread_id = $1), \
                EXISTS (SELECT 1 FROM events WHERE event_type = 'ThreadsDeleted' \
                        AND payload->'data'->'thread_ids' @> jsonb_build_array($1::text))",
    )
    .bind(thread_id)
    .fetch_one(pool)
    .await?;
    Ok(match (prompted, deleted) {
        (true, _) => FixThreadTrace::Prompted,
        (false, true) => FixThreadTrace::Deleted,
        (false, false) => FixThreadTrace::Unprompted,
    })
}

/// Every event the main check's state depends on, oldest first.
pub(super) async fn load_records(pool: &sqlx::PgPool) -> Result<Vec<MainCheckRecord>, sqlx::Error> {
    let rows: Vec<(String, serde_json::Value, Option<Uuid>)> = sqlx::query_as(
        "SELECT event_type, payload, thread_id FROM events \
         WHERE event_type IN ('MainCheckStarted', 'MainCheckPassed', 'MainCheckFailed', \
                              'MainCheckAbandoned', 'MainCheckRedClosed') \
            OR (event_type = 'ChangeApplied' AND payload->>'landing_merge_sha' IS NOT NULL) \
         ORDER BY sequence",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .iter()
        .filter_map(|(event_type, payload, thread_id)| record(event_type, payload, *thread_id))
        .collect())
}

/// One stored event as a [`MainCheckRecord`]. Thread events store their
/// fields flat, system events under `data`.
fn record(
    event_type: &str,
    payload: &serde_json::Value,
    thread_id: Option<Uuid>,
) -> Option<MainCheckRecord> {
    let data = payload.get("data");
    let text = |name: &str| data.and_then(|d| d.get(name)).and_then(|v| v.as_str());
    let uuid = |name: &str| text(name).and_then(|s| Uuid::parse_str(s).ok());
    let check_id = || uuid("check_id");
    Some(match event_type {
        "MainCheckStarted" => MainCheckRecord::Started(CheckRange {
            check_id: check_id()?,
            from_sha: text("from_sha")?.to_string(),
            to_sha: text("to_sha")?.to_string(),
        }),
        "MainCheckPassed" => MainCheckRecord::Ended {
            check_id: check_id()?,
            verdict: Verdict::Passed,
        },
        "MainCheckFailed" => MainCheckRecord::Ended {
            check_id: check_id()?,
            verdict: Verdict::Failed {
                summary: text("summary").unwrap_or_default().to_string(),
                tail: text("tail").unwrap_or_default().to_string(),
                run_url: text("run_url").map(str::to_string),
                fix_thread_id: uuid("fix_thread_id")?,
            },
        },
        "MainCheckAbandoned" => MainCheckRecord::Ended {
            check_id: check_id()?,
            verdict: Verdict::Abandoned,
        },
        "MainCheckRedClosed" => MainCheckRecord::RedClosed(check_id()?),
        "ChangeApplied" => MainCheckRecord::Landed {
            thread_id,
            pre_sha: payload
                .get("pre_merge_sha")
                .and_then(|v| v.as_str())
                .map(str::to_string),
        },
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The remote and the library are the shell's, word for word.
    #[test]
    fn the_preconditions_match_the_e2e_driver() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let lib = std::fs::read_to_string(root.join("scripts/lib/e2e_github.sh"))
            .expect("scripts/lib/e2e_github.sh");
        assert!(lib.contains(&format!("E2E_GITHUB_REMOTE=\"{MIRROR_REMOTE}\"")));
        assert!(lib.contains(&format!("$repo/{RELEASE_LIB}")));
    }

    /// A `git remote` that timed out never reads as a missing remote.
    #[test]
    fn an_unknown_remote_answer_is_no_missing_remote() {
        assert_eq!(missing_remote_reason(GitAnswer::Unknown), None);
        assert_eq!(missing_remote_reason(GitAnswer::Yes), None);
        assert!(missing_remote_reason(GitAnswer::No).is_some());
    }
}
