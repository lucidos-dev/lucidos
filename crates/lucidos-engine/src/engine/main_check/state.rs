//! The main check's pure scheduler: at most one check runs, at most one red is
//! open, and nothing waits on a clock.
//!
//! The driver feeds it what happened (a landing, a verdict, a Close) together
//! with `main` as it stands and a fresh check id. It answers with the effects
//! to carry out, in order, and has already moved its own state past them.
//!
//! `main` is `None` when no check can start: git could not read it, or this
//! checkout cannot run one. That starts nothing and forgets no landing.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A check of `to_sha`, covering what landed since `from_sha`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CheckRange {
    pub(crate) check_id: Uuid,
    pub(crate) from_sha: String,
    pub(crate) to_sha: String,
}

/// One failed test, as the check script names it: the suite or browser phase it
/// ran in, its name, and the head of its error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FailedTest {
    pub suite: String,
    pub name: String,
    #[serde(default)]
    pub detail: String,
}

/// The red check: the last verdict was a failure, and its fix thread owns it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RedCheck {
    pub(crate) range: CheckRange,
    pub(crate) summary: String,
    pub(crate) tail: String,
    pub(crate) failures: Vec<FailedTest>,
    pub(crate) run_url: Option<String>,
    pub(crate) fix_thread_id: Uuid,
}

/// Flaky findings owned by one main repair. They block no check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FlakyFix {
    pub(crate) range: CheckRange,
    pub(crate) run_url: Option<String>,
    pub(crate) flaky: Vec<FailedTest>,
    pub(crate) fix_thread_id: Uuid,
}

/// How a check ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Verdict {
    Passed,
    /// A pass attaches flaky findings to the current repair or schedules one.
    PassedWithFlakyFix {
        run_url: Option<String>,
        flaky: Vec<FailedTest>,
        fix_thread_id: Uuid,
    },
    Failed {
        summary: String,
        tail: String,
        failures: Vec<FailedTest>,
        run_url: Option<String>,
        fix_thread_id: Uuid,
    },
    /// No verdict: GitHub was down, the push was refused, or the run never
    /// finished.
    Abandoned,
    /// A failed run predating a fix landing; recorded, but cannot reopen red.
    SupersededFailure,
}

/// Why a red closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RedCloseCause {
    /// A change from its fix thread landed.
    FixLanded,
    /// The maintainer closed it.
    Closed,
    PassedOnNewerMain,
}

/// What the driver carries out after a transition, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Effect {
    CloseRed {
        check_id: Uuid,
        cause: RedCloseCause,
    },
    Start(CheckRange),
}

/// Why Close was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CloseRefusal {
    NoRed,
    /// The scheduler could not read its state back after a restart.
    StateUnread,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PredatesFix {
    check_id: Uuid,
    fix_thread_id: Uuid,
}

/// The scheduler's whole state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct MainCheck {
    /// Where the next check's range starts: `main` as the last check with a
    /// verdict saw it. Before any verdict, the first landing's base.
    base: Option<String>,
    running: Option<CheckRange>,
    predates_fix: Option<PredatesFix>,
    /// A change landed while a check ran or while no check could start.
    dirty: bool,
    red: Option<RedCheck>,
    red_opening_check_id: Option<Uuid>,
    /// The last verdict, when it passed and nothing has started since.
    passed: Option<CheckRange>,
    /// Flaky findings share the red repair owner when one exists.
    flaky: Option<FlakyFix>,
}

impl MainCheck {
    pub(crate) fn running(&self) -> Option<&CheckRange> {
        self.running.as_ref()
    }

    pub(crate) fn red(&self) -> Option<&RedCheck> {
        self.red.as_ref()
    }

    pub(crate) fn red_notice_id(&self) -> Option<Uuid> {
        self.red_opening_check_id
    }

    pub(crate) fn passed(&self) -> Option<&CheckRange> {
        self.passed.as_ref()
    }

    pub(crate) fn flaky_fix(&self) -> Option<&FlakyFix> {
        self.flaky.as_ref()
    }

    #[cfg(test)]
    pub(crate) fn dirty(&self) -> bool {
        self.dirty
    }

    /// A Lucidos-source change landed on `main`. `pre_sha` is `main` just
    /// before it, `from_fix` says whether the red's fix thread proposed it,
    /// and `main` is the tip now.
    pub(crate) fn landed(
        &mut self,
        pre_sha: Option<&str>,
        from_fix: bool,
        main: Option<&str>,
        next_id: Uuid,
    ) -> Vec<Effect> {
        if self.base.is_none() {
            self.base = pre_sha.map(str::to_string);
        }
        let mut effects = Vec::new();
        if from_fix {
            if let Some(red) = self.red.take() {
                self.red_opening_check_id = None;
                self.mark_running_before_fix(red.fix_thread_id);
                effects.push(Effect::CloseRed {
                    check_id: red.range.check_id,
                    cause: RedCloseCause::FixLanded,
                });
            }
        }
        if self.running.is_some() {
            self.dirty = true;
        } else {
            effects.extend(self.start(main, next_id));
        }
        effects
    }

    pub(crate) fn superseded_by_fix(&self, main: Option<&str>) -> Option<Uuid> {
        self.predates_fix
            .as_ref()
            .filter(|prior| {
                self.running.as_ref().is_some_and(|running| {
                    running.check_id == prior.check_id
                        && main.is_none_or(|tip| tip != running.to_sha)
                })
            })
            .map(|prior| prior.fix_thread_id)
    }

    pub(super) fn mark_running_before_fix(&mut self, fix_thread_id: Uuid) {
        if let Some(running) = &self.running {
            self.predates_fix = Some(PredatesFix {
                check_id: running.check_id,
                fix_thread_id,
            });
        }
    }

    pub(crate) fn replace_repair_owner(&mut self, owner: Uuid) {
        if let Some(red) = &mut self.red {
            red.fix_thread_id = owner;
        }
        if let Some(flaky) = &mut self.flaky {
            flaky.fix_thread_id = owner;
        }
    }

    pub(crate) fn repair_owner(&self) -> Option<Uuid> {
        self.red
            .as_ref()
            .map(|r| r.fix_thread_id)
            .or_else(|| self.flaky.as_ref().map(|f| f.fix_thread_id))
    }

    /// The running check `check_id` ended. A verdict for any other check is
    /// stale and changes nothing.
    pub(crate) fn ended(
        &mut self,
        check_id: Uuid,
        verdict: Verdict,
        main: Option<&str>,
        next_id: Uuid,
    ) -> Vec<Effect> {
        if self.running.as_ref().is_none_or(|r| r.check_id != check_id) {
            return Vec::new();
        }
        let mut effects = Vec::new();
        if matches!(
            verdict,
            Verdict::Passed | Verdict::PassedWithFlakyFix { .. }
        ) {
            if let Some(red) = &self.red {
                effects.push(Effect::CloseRed {
                    check_id: red.range.check_id,
                    cause: RedCloseCause::PassedOnNewerMain,
                });
            }
        }
        let verdict = if matches!(verdict, Verdict::Failed { .. })
            && self.superseded_by_fix(main).is_some()
        {
            Verdict::SupersededFailure
        } else {
            verdict
        };
        if self.finish(check_id, verdict, true) {
            effects.extend(self.start(main, next_id));
        }
        effects
    }

    /// Record a verdict. Live transitions reuse the repair owner; replay trusts
    /// the recorded owner, including independent repair threads in older logs.
    /// True when a follow-up check is due.
    fn finish(&mut self, check_id: Uuid, verdict: Verdict, reuse_owner: bool) -> bool {
        let Some(range) = self.running.take_if(|r| r.check_id == check_id) else {
            return false;
        };
        self.predates_fix = None;
        match verdict {
            Verdict::Passed => self.pass(range),
            Verdict::PassedWithFlakyFix {
                run_url,
                flaky,
                fix_thread_id,
            } => {
                let fix_thread_id = if reuse_owner {
                    self.repair_owner().unwrap_or(fix_thread_id)
                } else {
                    fix_thread_id
                };
                let mut findings = self.flaky.take().map(|f| f.flaky).unwrap_or_default();
                for finding in flaky {
                    if !findings.contains(&finding) {
                        findings.push(finding);
                    }
                }
                self.flaky = Some(FlakyFix {
                    range: range.clone(),
                    run_url,
                    flaky: findings,
                    fix_thread_id,
                });
                self.pass(range)
            }
            Verdict::Failed {
                summary,
                tail,
                failures,
                run_url,
                fix_thread_id,
            } => {
                let fix_thread_id = if reuse_owner {
                    self.repair_owner().unwrap_or(fix_thread_id)
                } else {
                    fix_thread_id
                };
                self.base = Some(range.to_sha.clone());
                if self.red.is_none() {
                    self.red_opening_check_id = Some(range.check_id);
                }
                self.red = Some(RedCheck {
                    range,
                    summary,
                    tail,
                    failures,
                    run_url,
                    fix_thread_id,
                });
                std::mem::take(&mut self.dirty)
            }
            Verdict::SupersededFailure => {
                self.base = Some(range.to_sha);
                std::mem::take(&mut self.dirty)
            }
            // No verdict starts nothing by itself: the next landing does.
            Verdict::Abandoned => {
                self.dirty = false;
                false
            }
        }
    }

    /// A pass cleared `range`. True when changes landed meanwhile, so the next
    /// check is due now.
    fn pass(&mut self, range: CheckRange) -> bool {
        self.base = Some(range.to_sha.clone());
        self.passed = Some(range);
        self.red = None;
        self.red_opening_check_id = None;
        std::mem::take(&mut self.dirty)
    }

    /// A change from `thread_id` landed: that closes the flaky fix thread, if
    /// it is the one. True when it closed.
    pub(crate) fn close_flaky_for(&mut self, thread_id: Uuid) -> bool {
        self.flaky
            .take_if(|flaky| flaky.fix_thread_id == thread_id)
            .is_some()
    }

    /// The flaky fix thread is gone: deleted, or archived by the maintainer.
    pub(crate) fn forget_flaky(&mut self) {
        self.flaky = None;
    }

    /// Close red without dropping a running check or its owed landings.
    pub(crate) fn close(
        &mut self,
        main: Option<&str>,
        next_id: Uuid,
    ) -> Result<Vec<Effect>, CloseRefusal> {
        let red = self.red.take().ok_or(CloseRefusal::NoRed)?;
        self.red_opening_check_id = None;
        let mut effects = vec![Effect::CloseRed {
            check_id: red.range.check_id,
            cause: RedCloseCause::Closed,
        }];
        if self.running.is_none() && std::mem::take(&mut self.dirty) {
            effects.extend(self.start(main, next_id));
        }
        Ok(effects)
    }

    /// Start a check of `main` from the base, unless nothing new is there. An
    /// unknown `main` starts nothing and keeps what landed owed to the next
    /// start.
    fn start(&mut self, main: Option<&str>, check_id: Uuid) -> Option<Effect> {
        if self.running.is_some() {
            return None;
        }
        let Some(main) = main else {
            self.dirty = true;
            return None;
        };
        self.dirty = false;
        let from_sha = self.base.clone().unwrap_or_else(|| main.to_string());
        if from_sha == main {
            return None;
        }
        let range = CheckRange {
            check_id,
            from_sha,
            to_sha: main.to_string(),
        };
        self.running = Some(range.clone());
        self.predates_fix = None;
        self.passed = None;
        Some(Effect::Start(range))
    }

    // ── Replay, for recovery. These rebuild the state from its events and
    // never decide anything: what was decided is already in the log. ──

    pub(super) fn replay_started(&mut self, range: CheckRange) {
        self.running = Some(range);
        self.predates_fix = None;
        self.passed = None;
        self.dirty = false;
    }

    /// A pass with landings behind it started the next check at once, and its
    /// own `MainCheckStarted` follows in the log.
    pub(super) fn replay_ended(&mut self, check_id: Uuid, verdict: Verdict) {
        if self.finish(check_id, verdict, false) {
            self.dirty = true;
        }
    }

    pub(super) fn replay_landed(&mut self, pre_sha: Option<&str>) {
        if self.base.is_none() {
            self.base = pre_sha.map(str::to_string);
        }
        if self.running.is_some() || self.red.is_some() {
            self.dirty = true;
        }
    }

    pub(super) fn replay_red_closed(&mut self, check_id: Uuid) {
        if self
            .red
            .as_ref()
            .is_some_and(|r| r.range.check_id == check_id)
        {
            self.red = None;
            self.red_opening_check_id = None;
        }
    }

    pub(super) fn expire_followup(&mut self) {
        self.dirty = false;
    }

    /// The check a restart cut off, taken out of the state.
    pub(super) fn take_interrupted(&mut self) -> Option<CheckRange> {
        let range = self.running.take()?;
        self.predates_fix = None;
        self.dirty = false;
        Some(range)
    }
}

#[cfg(test)]
#[path = "state_tests.rs"]
mod tests;
