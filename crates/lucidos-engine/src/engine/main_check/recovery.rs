//! The main check rebuilt from events after a restart. It keeps no table: its
//! own events and the landings between them are the whole state.

use super::runner::RunLink;
use super::state::{CheckRange, MainCheck, Verdict};
use uuid::Uuid;

/// One stored event the main check's state depends on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum MainCheckRecord {
    /// Restoring retains historical verdicts but expires pending follow-up work.
    RestoreBoundary,
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
    /// Fix closures inferred at landing that still need a recorded acknowledgment.
    pub(super) fix_landed: Vec<Uuid>,
}

/// Fold the records, oldest first. Nothing here starts a check.
pub(super) fn fold(records: &[MainCheckRecord]) -> Recovered {
    let mut check = MainCheck::default();
    let mut fix_landed = Vec::new();
    let mut linked: Option<(Uuid, RunLink)> = None;
    for record in records {
        match record {
            MainCheckRecord::RestoreBoundary => check.expire_followup(),
            MainCheckRecord::Started(range) => check.replay_started(range.clone()),
            MainCheckRecord::RunLinked { check_id, link } => {
                linked = Some((*check_id, link.clone()));
            }
            MainCheckRecord::Ended { check_id, verdict } => {
                check.replay_ended(*check_id, verdict.clone());
            }
            MainCheckRecord::RedClosed(check_id) => {
                check.replay_red_closed(*check_id);
                fix_landed.retain(|red| red != check_id);
            }
            MainCheckRecord::Landed { thread_id, pre_sha } => {
                if let Some(thread) = thread_id {
                    check.close_flaky_for(*thread);
                }
                if let Some(red) = check.red() {
                    if *thread_id == Some(red.fix_thread_id) {
                        let check_id = red.range.check_id;
                        fix_landed.push(check_id);
                        let owner = red.fix_thread_id;
                        check.mark_running_before_fix(owner);
                        check.replay_red_closed(check_id);
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
        assert!(recovered.fix_landed.is_empty());
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
        assert_eq!(fold(&records).fix_landed, vec![id(1)]);
        let mut closed = records;
        closed.push(MainCheckRecord::RedClosed(id(1)));
        let recovered = fold(&closed);
        assert!(recovered.fix_landed.is_empty());
        assert!(recovered.check.red().is_none());
    }

    #[test]
    fn a_late_healed_close_never_supersedes_the_newer_run() {
        for late_close in [false, true] {
            let mut records = vec![
                started(1, "a", "b"),
                ended(1, failed(9)),
                started(2, "b", "c"),
                landed(Some(9), "c"),
                ended(2, Verdict::SupersededFailure),
                started(3, "c", "d"),
                run_linked(3),
            ];
            if late_close {
                records.push(MainCheckRecord::RedClosed(id(1)));
            }
            let mut recovered = fold(&records);
            assert_eq!(recovered.check.superseded_by_fix(Some("d")), None);
            recovered.check.ended(id(3), failed(8), Some("d"), id(4));
            assert_eq!(recovered.check.red().unwrap().range.check_id, id(3));
        }
    }

    #[test]
    fn a_replayed_fix_marks_only_the_run_already_in_flight() {
        let mut recovered = fold(&[
            started(1, "a", "b"),
            ended(1, failed(9)),
            started(2, "b", "c"),
            run_linked(2),
            landed(Some(9), "c"),
        ]);
        assert_eq!(recovered.check.superseded_by_fix(Some("d")), Some(id(9)));
        recovered.check.ended(id(2), failed(8), Some("d"), id(3));
        assert!(recovered.check.red().is_none());
        assert_eq!(recovered.check.running().unwrap().check_id, id(3));
    }

    #[test]
    fn a_recorded_current_tip_failure_survives_a_missing_or_delayed_close() {
        let mut live = MainCheck::default();
        live.landed(Some("a"), false, Some("b"), id(1));
        live.ended(id(1), failed(9), Some("b"), id(2));
        live.landed(Some("b"), false, Some("d"), id(2));
        live.landed(Some("c"), true, Some("d"), id(3));
        live.ended(id(2), failed(8), Some("d"), id(4));
        for late_close in [false, true] {
            let mut records = vec![
                started(1, "a", "b"),
                ended(1, failed(9)),
                started(2, "b", "d"),
                landed(Some(9), "c"),
                ended(2, failed(8)),
            ];
            if late_close {
                records.push(MainCheckRecord::RedClosed(id(1)));
            }
            let recovered = fold(&records);
            assert_eq!(recovered.check.red(), live.red());
            assert_eq!(recovered.check.repair_owner(), live.repair_owner());
            assert_eq!(recovered.check.running(), live.running());
            assert_eq!(recovered.check.red_notice_id(), Some(id(2)));
            assert_eq!(
                recovered.fix_landed,
                if late_close { vec![] } else { vec![id(1)] }
            );
        }
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
    #[test]
    fn restore_floor_expires_owed_work_and_retains_verdicts() {
        let mut records = vec![started(1, "a", "b"), ended(1, failed(8)), landed(None, "b")];
        assert!(fold(&records).check.dirty());
        records.push(MainCheckRecord::RestoreBoundary);
        let restored = fold(&records);
        assert!(!restored.check.dirty());
        assert!(restored.check.red().is_some());
        records.push(landed(None, "c"));
        assert!(fold(&records).check.dirty(), "new work remains owed");
    }
}

#[cfg(test)]
mod continued_red_tests {
    use super::*;

    #[test]
    fn a_continued_failure_keeps_its_owner_and_a_pass_closes_red() {
        let first = Uuid::new_v4();
        let second = Uuid::new_v4();
        let owner = Uuid::new_v4();
        let failed = |fix_thread_id| Verdict::Failed {
            summary: "Failed: engine".into(),
            tail: String::new(),
            failures: Vec::new(),
            run_url: None,
            fix_thread_id,
        };
        let mut records = vec![
            MainCheckRecord::Started(CheckRange {
                check_id: first,
                from_sha: "a".into(),
                to_sha: "b".into(),
            }),
            MainCheckRecord::Ended {
                check_id: first,
                verdict: failed(owner),
            },
            MainCheckRecord::Started(CheckRange {
                check_id: second,
                from_sha: "b".into(),
                to_sha: "c".into(),
            }),
            MainCheckRecord::Ended {
                check_id: second,
                verdict: failed(owner),
            },
        ];
        let recovered = fold(&records);
        assert_eq!(recovered.check.red().unwrap().fix_thread_id, owner);
        assert_eq!(recovered.check.red().unwrap().range.check_id, second);
        let third = Uuid::new_v4();
        records.extend([
            MainCheckRecord::Started(CheckRange {
                check_id: third,
                from_sha: "c".into(),
                to_sha: "d".into(),
            }),
            MainCheckRecord::Ended {
                check_id: third,
                verdict: Verdict::Passed,
            },
            MainCheckRecord::RedClosed(second),
        ]);
        assert!(fold(&records).check.red().is_none());
        assert!(matches!(
            records[1],
            MainCheckRecord::Ended {
                verdict: Verdict::Failed { .. },
                ..
            }
        ));
    }
}
