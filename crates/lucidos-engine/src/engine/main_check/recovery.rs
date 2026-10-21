//! The main check rebuilt from events after a restart. It keeps no table: its
//! own events and the landings between them are the whole state.

use super::runner::RunLink;
use super::state::{CheckRange, MainCheck, Verdict};
use uuid::Uuid;

/// One stored event the main check's state depends on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum MainCheckRecord {
    Started(CheckRange),
    RunLinked {
        check_id: Uuid,
        link: RunLink,
    },
    Ended {
        check_id: Uuid,
        verdict: Verdict,
    },
    RedClosed(Uuid),
    /// A `ChangeApplied` that carried a landing merge: `main` moved for a
    /// Lucidos-source change.
    Landed {
        thread_id: Option<Uuid>,
        pre_sha: Option<String>,
    },
}

/// What a restart found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct Recovered {
    pub(super) check: MainCheck,
    /// The check that was running when the engine stopped, before its run
    /// was on record. Its runner cancelled the run, so it is abandoned.
    pub(super) interrupted: Option<CheckRange>,
    /// The check that was running when the engine stopped, with its run on
    /// record. It stays running, and the next start reattaches to the run.
    pub(super) reattach: Option<(CheckRange, RunLink)>,
    /// The open red whose fix landed before a restart cut its close off.
    pub(super) fix_landed: Option<Uuid>,
}

/// Fold the records, oldest first. Nothing here starts a check.
pub(super) fn fold(records: &[MainCheckRecord]) -> Recovered {
    let mut check = MainCheck::default();
    let mut fix_landed = None;
    let mut linked: Option<(Uuid, RunLink)> = None;
    for record in records {
        match record {
            MainCheckRecord::Started(range) => check.replay_started(range.clone()),
            MainCheckRecord::RunLinked { check_id, link } => {
                linked = Some((*check_id, link.clone()));
            }
            MainCheckRecord::Ended { check_id, verdict } => {
                check.replay_ended(*check_id, verdict.clone());
            }
            MainCheckRecord::RedClosed(check_id) => {
                check.replay_red_closed(*check_id);
                fix_landed = fix_landed.filter(|red| red != check_id);
            }
            MainCheckRecord::Landed { thread_id, pre_sha } => {
                if let Some(thread) = thread_id {
                    check.close_flaky_for(*thread);
                }
                if let Some(red) = check.red() {
                    if *thread_id == Some(red.fix_thread_id) {
                        fix_landed = Some(red.range.check_id);
                    }
                }
                check.replay_landed(pre_sha.as_deref());
            }
        }
    }
    let reattach = check.running().cloned().and_then(|range| {
        let (_, link) = linked.filter(|(id, _)| *id == range.check_id)?;
        Some((range, link))
    });
    let interrupted = if reattach.is_some() {
        None
    } else {
        check.take_interrupted()
    };
    let fix_landed = fix_landed.filter(|red| check.red().is_some_and(|r| r.range.check_id == *red));
    Recovered {
        check,
        interrupted,
        reattach,
        fix_landed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u8) -> Uuid {
        Uuid::from_bytes([n; 16])
    }

    fn started(n: u8, from: &str, to: &str) -> MainCheckRecord {
        MainCheckRecord::Started(CheckRange {
            check_id: id(n),
            from_sha: from.into(),
            to_sha: to.into(),
        })
    }

    fn landed(thread: Option<u8>, pre: &str) -> MainCheckRecord {
        MainCheckRecord::Landed {
            thread_id: thread.map(id),
            pre_sha: Some(pre.into()),
        }
    }

    fn ended(n: u8, verdict: Verdict) -> MainCheckRecord {
        MainCheckRecord::Ended {
            check_id: id(n),
            verdict,
        }
    }

    fn failed(fix: u8) -> Verdict {
        Verdict::Failed {
            summary: "Failed: engine.".into(),
            tail: String::new(),
            failures: Vec::new(),
            run_url: None,
            fix_thread_id: id(fix),
        }
    }

    #[test]
    fn no_events_rebuild_an_idle_check() {
        assert_eq!(fold(&[]), Recovered::default());
    }

    /// A restart mid-run finds the running check and takes it out: its runner
    /// died with the engine.
    #[test]
    fn a_check_running_at_restart_is_interrupted() {
        let recovered = fold(&[landed(None, "a"), started(1, "a", "b")]);
        assert_eq!(recovered.interrupted.map(|r| r.check_id), Some(id(1)));
        assert!(recovered.check.running().is_none());
    }

    fn run_linked(n: u8) -> MainCheckRecord {
        MainCheckRecord::RunLinked {
            check_id: id(n),
            link: RunLink {
                run_url: format!("https://r/{n}"),
                branch: format!("e2e/{n}"),
            },
        }
    }

    /// A restart after the run was on record keeps the check running and
    /// names the run to reattach to. What landed meanwhile stays owed.
    #[test]
    fn a_check_whose_run_is_on_record_is_reattached() {
        let recovered = fold(&[
            landed(None, "a"),
            started(1, "a", "b"),
            run_linked(1),
            landed(None, "b"),
        ]);
        let (range, link) = recovered.reattach.expect("reattach");
        assert_eq!(range.check_id, id(1));
        assert_eq!(link.branch, "e2e/1");
        assert!(recovered.interrupted.is_none());
        assert_eq!(recovered.check.running().map(|r| r.check_id), Some(id(1)));
        assert!(
            recovered.check.dirty(),
            "the landing during the run is owed"
        );
    }

    /// A link to an earlier check never reattaches a later one.
    #[test]
    fn an_earlier_checks_link_does_not_reattach_the_running_one() {
        let recovered = fold(&[
            started(1, "a", "b"),
            run_linked(1),
            ended(1, Verdict::Passed),
            started(2, "b", "c"),
        ]);
        assert!(recovered.reattach.is_none());
        assert_eq!(recovered.interrupted.map(|r| r.check_id), Some(id(2)));
    }

    #[test]
    fn a_red_survives_a_restart_with_its_fix_thread() {
        let recovered = fold(&[
            landed(None, "a"),
            started(1, "a", "b"),
            landed(Some(5), "b"),
            ended(1, failed(9)),
        ]);
        let red = recovered.check.red().expect("red");
        assert_eq!(red.fix_thread_id, id(9));
        assert!(recovered.interrupted.is_none());
        assert!(recovered.fix_landed.is_none());
        assert!(
            recovered.check.dirty(),
            "the landing during the run is unchecked"
        );
    }

    /// The fix landed, and the restart came before the red closed.
    #[test]
    fn a_fix_that_landed_before_the_close_is_found() {
        let records = vec![
            started(1, "a", "b"),
            ended(1, failed(9)),
            landed(Some(9), "b"),
        ];
        assert_eq!(fold(&records).fix_landed, Some(id(1)));
        let mut closed = records;
        closed.push(MainCheckRecord::RedClosed(id(1)));
        let recovered = fold(&closed);
        assert_eq!(recovered.fix_landed, None);
        assert!(recovered.check.red().is_none());
    }

    #[test]
    fn a_pass_then_a_coalesced_check_rebuilds_from_the_log() {
        let recovered = fold(&[
            landed(None, "a"),
            started(1, "a", "b"),
            landed(None, "b"),
            ended(1, Verdict::Passed),
            started(2, "b", "c"),
            ended(2, Verdict::Passed),
        ]);
        assert_eq!(recovered.check.passed().map(|r| r.check_id), Some(id(2)));
        assert!(recovered.interrupted.is_none());
    }

    fn passed_flaky(fix: u8) -> Verdict {
        Verdict::PassedWithFlakyFix {
            run_url: None,
            flaky: Vec::new(),
            fix_thread_id: id(fix),
        }
    }

    /// An open flaky fix thread survives a restart, and a landing from it
    /// closes it in the replay.
    #[test]
    fn a_flaky_fix_thread_rebuilds_open_until_a_landing_from_it() {
        let open = vec![started(1, "a", "b"), ended(1, passed_flaky(8))];
        let recovered = fold(&open);
        assert_eq!(
            recovered.check.flaky_fix().map(|f| f.fix_thread_id),
            Some(id(8))
        );
        let mut closed = open.clone();
        closed.push(landed(Some(7), "b"));
        assert!(
            fold(&closed).check.flaky_fix().is_some(),
            "another thread's landing"
        );
        closed.push(landed(Some(8), "c"));
        assert!(fold(&closed).check.flaky_fix().is_none());
    }

    #[test]
    fn an_abandoned_check_rebuilds_as_idle() {
        let recovered = fold(&[started(1, "a", "b"), ended(1, Verdict::Abandoned)]);
        assert_eq!(recovered.check.running(), None);
        assert_eq!(recovered.check.red(), None);
        assert_eq!(recovered.check.passed(), None);
    }
}
