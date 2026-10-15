//! The main check's pure scheduler: at most one check runs, at most one red is
//! open, and nothing waits on a clock.
//!
//! The driver feeds it what happened (a landing, a verdict, a Close) together
//! with `main` as it stands and a fresh check id. It answers with the effects
//! to carry out, in order, and has already moved its own state past them.
//!
//! `main` is `None` when no check can start: git could not read it, or this
//! checkout cannot run one. That starts nothing and forgets no landing.

use uuid::Uuid;

/// A check of `to_sha`, covering what landed since `from_sha`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CheckRange {
    pub(crate) check_id: Uuid,
    pub(crate) from_sha: String,
    pub(crate) to_sha: String,
}

/// The red check: the last verdict was a failure, and its fix thread owns it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RedCheck {
    pub(crate) range: CheckRange,
    pub(crate) summary: String,
    pub(crate) tail: String,
    pub(crate) run_url: Option<String>,
    pub(crate) fix_thread_id: Uuid,
}

/// How a check ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Verdict {
    Passed,
    Failed {
        summary: String,
        tail: String,
        run_url: Option<String>,
        fix_thread_id: Uuid,
    },
    /// No verdict: GitHub was down, the push was refused, or the run never
    /// finished.
    Abandoned,
}

/// Why a red closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RedCloseCause {
    /// A change from its fix thread landed.
    FixLanded,
    /// The maintainer closed it.
    Closed,
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

/// The scheduler's whole state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct MainCheck {
    /// Where the next check's range starts: `main` as the last check with a
    /// verdict saw it. Before any verdict, the first landing's base.
    base: Option<String>,
    running: Option<CheckRange>,
    /// A change landed that no check covers yet: while a check ran, while red
    /// was open, or while no check could start.
    dirty: bool,
    red: Option<RedCheck>,
    /// The last verdict, when it passed and nothing has started since.
    passed: Option<CheckRange>,
}

impl MainCheck {
    pub(crate) fn running(&self) -> Option<&CheckRange> {
        self.running.as_ref()
    }

    pub(crate) fn red(&self) -> Option<&RedCheck> {
        self.red.as_ref()
    }

    pub(crate) fn passed(&self) -> Option<&CheckRange> {
        self.passed.as_ref()
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
        if self.running.is_some() {
            self.dirty = true;
            return Vec::new();
        }
        let Some(red) = &self.red else {
            return self.start(main, next_id).into_iter().collect();
        };
        if !from_fix {
            self.dirty = true;
            return Vec::new();
        }
        let close = Effect::CloseRed {
            check_id: red.range.check_id,
            cause: RedCloseCause::FixLanded,
        };
        self.red = None;
        std::iter::once(close)
            .chain(self.start(main, next_id))
            .collect()
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
        if self.finish(check_id, verdict) {
            self.start(main, next_id).into_iter().collect()
        } else {
            Vec::new()
        }
    }

    /// Record how `check_id` ended. True when a pass found changes that
    /// landed meanwhile, so the next check is due now.
    fn finish(&mut self, check_id: Uuid, verdict: Verdict) -> bool {
        let Some(range) = self.running.take_if(|r| r.check_id == check_id) else {
            return false;
        };
        match verdict {
            Verdict::Passed => {
                self.base = Some(range.to_sha.clone());
                self.passed = Some(range);
                std::mem::take(&mut self.dirty)
            }
            Verdict::Failed {
                summary,
                tail,
                run_url,
                fix_thread_id,
            } => {
                self.base = Some(range.to_sha.clone());
                self.red = Some(RedCheck {
                    range,
                    summary,
                    tail,
                    run_url,
                    fix_thread_id,
                });
                false
            }
            // No verdict starts nothing by itself: the next landing does.
            Verdict::Abandoned => {
                self.dirty = false;
                false
            }
        }
    }

    /// The maintainer closes the red. What landed while it was open starts
    /// the next check.
    pub(crate) fn close(
        &mut self,
        main: Option<&str>,
        next_id: Uuid,
    ) -> Result<Vec<Effect>, CloseRefusal> {
        let red = self.red.take().ok_or(CloseRefusal::NoRed)?;
        let mut effects = vec![Effect::CloseRed {
            check_id: red.range.check_id,
            cause: RedCloseCause::Closed,
        }];
        if std::mem::take(&mut self.dirty) {
            effects.extend(self.start(main, next_id));
        }
        Ok(effects)
    }

    /// Start a check of `main` from the base, unless nothing new is there. An
    /// unknown `main` starts nothing and keeps what landed owed to the next
    /// start.
    fn start(&mut self, main: Option<&str>, check_id: Uuid) -> Option<Effect> {
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
        self.passed = None;
        Some(Effect::Start(range))
    }

    // ── Replay, for recovery. These rebuild the state from its events and
    // never decide anything: what was decided is already in the log. ──

    pub(super) fn replay_started(&mut self, range: CheckRange) {
        self.running = Some(range);
        self.passed = None;
        self.dirty = false;
    }

    /// A pass with landings behind it started the next check at once, and its
    /// own `MainCheckStarted` follows in the log.
    pub(super) fn replay_ended(&mut self, check_id: Uuid, verdict: Verdict) {
        self.finish(check_id, verdict);
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
        }
    }

    /// The check a restart cut off, taken out of the state.
    pub(super) fn take_interrupted(&mut self) -> Option<CheckRange> {
        let range = self.running.take()?;
        self.dirty = false;
        Some(range)
    }

    /// Close the red a fix already closed before a restart cut the close off.
    pub(super) fn heal_fix_landed(&mut self) -> Option<Uuid> {
        let red = self.red.take()?;
        self.dirty = false;
        Some(red.range.check_id)
    }
}

#[cfg(test)]
#[path = "state_tests.rs"]
mod tests;
