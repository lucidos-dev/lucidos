//! The slowness warning: is Lucidos slow, and why?
//!
//! Every [`SAMPLE_INTERVAL`] the gateway takes one sample: the host's memory
//! pressure, and which workspaces the supervisor saw answering slowly since
//! the last one. Elevated samples that hold for most of a window open an
//! *episode*. While one is open, the gateway measures who holds the memory or
//! keeps the processor busy. It serves the answer on
//! `GET /~/api/v1/control/slowness` for the workspace banner.
//!
//! It lives in the gateway because the gateway is the one process per machine,
//! and it already probes every engine (ADR 0274). A slow engine opens an
//! episode; memory pressure short of the swap rule only explains one (ADR 0283).
//! A full disk or a stuck database explains one first, because each has a fix
//! of its own (ADR 0301).

mod disk;
mod pressure;
mod processes;

use chrono::{DateTime, Utc};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use crate::stack::DatabaseHealth;
use processes::{MemoryUser, ProcessorUser, Scan};

pub const SAMPLE_INTERVAL: Duration = Duration::from_secs(30);

/// Samples in the window. At 30 seconds apart, that is five minutes.
const WINDOW: usize = 10;

/// Elevated samples in a full window that open an episode.
const OPEN_AFTER: usize = 8;

/// Consecutive quiet samples that close an episode.
const CLOSE_AFTER: usize = 10;

/// Samples in the window with memory raised that make memory the reason.
const MEMORY_REASON_AFTER: usize = WINDOW / 2;

/// Samples in the window with a database sign that make the database the
/// reason.
const DATABASE_REASON_AFTER: usize = WINDOW / 2;

/// How long a first scan measures processor time when no earlier scan exists.
const PROCESSOR_BASELINE: Duration = Duration::from_secs(2);

/// What the banner reads.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum SlownessStatus {
    #[default]
    Normal,
    Slow {
        /// Stable for the life of one episode, so a dismissal can name it.
        episode_id: String,
        #[serde(flatten)]
        reason: SlownessReason,
    },
}

/// Why an open episode says Lucidos is slow, with what the user can act on.
/// The variants are in the order the reason is chosen.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum SlownessReason {
    /// The disk is almost full. Every workspace shows it.
    Disk { free_bytes: u64 },
    /// An engine answers, but its database does not. Only the windows of
    /// `slow_workspaces` show it.
    Database {
        problem: DatabaseProblem,
        slow_workspaces: Vec<String>,
    },
    /// The computer is short on memory. Every workspace shows it.
    Memory { top_users: Vec<MemoryUser> },
    /// Lucidos is slow and no known cause holds. Only the windows of
    /// `slow_workspaces` show it.
    Unclear {
        busiest_apps: Vec<ProcessorUser>,
        slow_workspaces: Vec<String>,
    },
}

/// What is wrong with a database that an engine reported down.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DatabaseProblem {
    /// The database answers, but the engine has no free pooled connection.
    PoolExhausted,
    /// The database itself does not answer.
    NotAnswering,
}

/// What one probe showed about a slow workspace. Two signs in one sample merge
/// to the greater, so a timeout outweighs a database sign.
///
/// A database sign means the engine answered promptly and its database did
/// not. That is what tells a stuck database from a starved host, where the
/// engine itself stops answering too.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SlowSign {
    Database(DatabaseProblem),
    /// The engine did not answer within the probe timeout.
    NoAnswer,
}

/// What the supervisor's probe of one engine says about Lucidos being slow.
/// Liveness is not health: only an engine that answered, or is alive and past
/// its cold boot, can be slow. A dead or booting one is the supervisor's
/// business.
pub fn slow_sign(
    outcome: crate::stack::ProbeOutcome,
    alive: bool,
    past_boot: bool,
    database: DatabaseHealth,
) -> Option<SlowSign> {
    use crate::stack::ProbeOutcome;
    match outcome {
        ProbeOutcome::Healthy => match database {
            DatabaseHealth::Reachable => None,
            DatabaseHealth::NotAnswering => Some(SlowSign::Database(DatabaseProblem::NotAnswering)),
            DatabaseHealth::PoolExhausted => {
                Some(SlowSign::Database(DatabaseProblem::PoolExhausted))
            }
        },
        ProbeOutcome::Slow => (alive && past_boot).then_some(SlowSign::NoAnswer),
        ProbeOutcome::Unreachable | ProbeOutcome::Other => None,
    }
}

/// What one memory reading may decide.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct MemorySignal {
    /// Enough to open an episode alone.
    elevated: bool,
    /// Enough to name memory as the reason.
    raised: bool,
}

impl MemorySignal {
    /// An unreadable host says nothing about memory, and never blames it.
    fn read() -> Self {
        pressure::read().map_or_else(Self::default, |r| Self {
            elevated: r.is_elevated(),
            raised: r.is_raised(),
        })
    }
}

/// One tick's evidence.
#[derive(Debug, Clone, Default, PartialEq)]
struct Sample {
    memory: MemorySignal,
    /// Workspaces the supervisor saw answering slowly since the last tick, and
    /// the greatest sign each one showed.
    slow_workspaces: BTreeMap<String, SlowSign>,
}

impl Sample {
    fn is_elevated(&self) -> bool {
        self.memory.elevated || !self.slow_workspaces.is_empty()
    }

    /// The worst database problem any workspace showed in this sample.
    fn database_problem(&self) -> Option<DatabaseProblem> {
        self.slow_workspaces
            .values()
            .filter_map(|sign| match sign {
                SlowSign::Database(problem) => Some(*problem),
                SlowSign::NoAnswer => None,
            })
            .max()
    }
}

/// Why an episode is open, before anything is measured.
#[derive(Debug, PartialEq)]
enum Cause {
    Database {
        problem: DatabaseProblem,
        slow_workspaces: Vec<String>,
    },
    Memory,
    Unclear {
        slow_workspaces: Vec<String>,
    },
}

/// The window and the open episode. Pure: time and readings come in as
/// arguments, so a test replays a recorded sequence.
#[derive(Debug, Clone, Default)]
struct Classifier {
    window: VecDeque<Sample>,
    quiet_streak: usize,
    episode: Option<DateTime<Utc>>,
}

/// What one sample decided about the episode.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Transition {
    Opened,
    Held,
    Closed,
    Quiet,
}

impl Transition {
    /// Processes are enumerated only while an episode is open.
    fn scans(self) -> bool {
        matches!(self, Self::Opened | Self::Held)
    }
}

impl Classifier {
    fn record(&mut self, sample: Sample, now: DateTime<Utc>) -> Transition {
        let elevated = sample.is_elevated();
        if self.window.len() == WINDOW {
            self.window.pop_front();
        }
        self.window.push_back(sample);
        self.quiet_streak = if elevated { 0 } else { self.quiet_streak + 1 };

        match self.episode {
            None if self.window.len() == WINDOW
                && self.window.iter().filter(|s| s.is_elevated()).count() >= OPEN_AFTER =>
            {
                self.episode = Some(now);
                Transition::Opened
            }
            None => Transition::Quiet,
            Some(_) if self.quiet_streak >= CLOSE_AFTER => {
                self.episode = None;
                Transition::Closed
            }
            Some(_) => Transition::Held,
        }
    }

    /// The database when at least half the window holds a database sign. It
    /// comes before memory: a stuck database blamed on memory leaves the user
    /// no way out (ADR 0301).
    ///
    /// Then unclear when a workspace was slow in the window and memory was
    /// raised for less than half of it. Otherwise memory: with no slow
    /// workspace, the memory rule is the only thing that can have held the
    /// episode open.
    fn cause(&self) -> Cause {
        if let Some(database) = self.database_cause() {
            return database;
        }
        let raised = self.window.iter().filter(|s| s.memory.raised).count();
        let slow: BTreeSet<&String> = self
            .window
            .iter()
            .flat_map(|s| s.slow_workspaces.keys())
            .collect();
        if raised >= MEMORY_REASON_AFTER || slow.is_empty() {
            return Cause::Memory;
        }
        Cause::Unclear {
            slow_workspaces: slow.into_iter().cloned().collect(),
        }
    }

    /// The problem is the latest one seen, since that is what the user can
    /// still fix. The workspaces are every one with a database sign in the
    /// window.
    fn database_cause(&self) -> Option<Cause> {
        let samples = self
            .window
            .iter()
            .filter(|s| s.database_problem().is_some())
            .count();
        if samples < DATABASE_REASON_AFTER {
            return None;
        }
        let problem = self
            .window
            .iter()
            .rev()
            .find_map(Sample::database_problem)?;
        let slow_workspaces: BTreeSet<&String> = self
            .window
            .iter()
            .flat_map(|s| &s.slow_workspaces)
            .filter(|(_, sign)| matches!(sign, SlowSign::Database(_)))
            .map(|(workspace, _)| workspace)
            .collect();
        Some(Cause::Database {
            problem,
            slow_workspaces: slow_workspaces.into_iter().cloned().collect(),
        })
    }
}

#[derive(Default)]
struct State {
    classifier: Classifier,
    status: SlownessStatus,
    /// The last episode scan, as the baseline for processor time.
    last_scan: Option<Scan>,
}

/// The machine's one slowness watch, held on the gateway state.
#[derive(Default)]
pub struct Slowness {
    state: Mutex<State>,
    /// Filled by the supervisor between samples, drained by each sample.
    slow_since_sample: Mutex<BTreeMap<String, SlowSign>>,
    disk: disk::DiskReader,
}

impl Slowness {
    /// The supervisor saw this workspace answer slowly. It counts in the next
    /// sample, with the greatest sign seen since the last one.
    pub fn record_slow(&self, workspace_id: &str, sign: SlowSign) {
        self.slow_since_sample
            .lock()
            .unwrap()
            .entry(workspace_id.to_string())
            .and_modify(|seen| *seen = (*seen).max(sign))
            .or_insert(sign);
    }

    /// Take one sample. Blocking: it makes syscalls and, during an episode,
    /// reads every process, so the loop runs it on a blocking thread.
    ///
    /// `lucidos_roots` names the process trees that count as Lucidos, and
    /// `volumes` the directories whose disk counts. Both are read only during
    /// an episode.
    ///
    /// This is the only writer, so it classifies and scans on a copy and
    /// commits both at once. The status route never waits on the scan, and a
    /// new episode is never served without its list.
    pub fn sample(&self, lucidos_roots: impl FnOnce() -> Vec<i32>, volumes: &[PathBuf]) {
        let sample = Sample {
            memory: MemorySignal::read(),
            slow_workspaces: std::mem::take(&mut *self.slow_since_sample.lock().unwrap()),
        };
        let (mut classifier, last_scan) = {
            let mut state = self.state.lock().unwrap();
            (state.classifier.clone(), state.last_scan.take())
        };
        let transition = classifier.record(sample, Utc::now());
        let (status, last_scan) = match classifier.episode {
            Some(opened) if transition.scans() => {
                let free_bytes = self.disk.lowest_free_bytes(volumes);
                let (reason, scan) =
                    attribute(classifier.cause(), free_bytes, last_scan, lucidos_roots);
                if transition == Transition::Opened {
                    crate::log!("[Gateway] slowness episode opened: {}", describe(&reason));
                }
                let status = SlownessStatus::Slow {
                    episode_id: opened.to_rfc3339(),
                    reason,
                };
                (status, scan)
            }
            _ => {
                if transition == Transition::Closed {
                    crate::log!("[Gateway] slowness episode closed after five quiet minutes");
                }
                (SlownessStatus::Normal, None)
            }
        };
        *self.state.lock().unwrap() = State {
            classifier,
            status,
            last_scan,
        };
    }

    pub fn snapshot(&self) -> SlownessStatus {
        self.state.lock().unwrap().status.clone()
    }
}

/// Measure what the banner lists for `cause`, and return the scan for the next
/// sample's baseline. With no baseline, processor time is measured over
/// [`PROCESSOR_BASELINE`].
///
/// A critically low disk names the disk whatever the cause: a full disk is
/// what stalls the database, and it stops swap from growing. Only memory and
/// unclear list processes, so only they scan. The others return no scan, so a
/// later unclear reason measures from a fresh baseline, never a stale one.
fn attribute(
    cause: Cause,
    free_bytes: Option<u64>,
    last_scan: Option<Scan>,
    lucidos_roots: impl FnOnce() -> Vec<i32>,
) -> (SlownessReason, Option<Scan>) {
    if let Some(free_bytes) = free_bytes.filter(|b| disk::is_critical(*b)) {
        return (SlownessReason::Disk { free_bytes }, None);
    }
    match cause {
        Cause::Database {
            problem,
            slow_workspaces,
        } => {
            let reason = SlownessReason::Database {
                problem,
                slow_workspaces,
            };
            (reason, None)
        }
        Cause::Memory => {
            let scan = processes::scan();
            let top_users = processes::top_users(&scan, &lucidos_roots());
            (SlownessReason::Memory { top_users }, Some(scan))
        }
        Cause::Unclear { slow_workspaces } => {
            let earlier = last_scan.unwrap_or_else(|| {
                let earlier = processes::scan();
                std::thread::sleep(PROCESSOR_BASELINE);
                earlier
            });
            let later = processes::scan();
            let busiest_apps = processes::busiest_apps(&earlier, &later, &lucidos_roots());
            let reason = SlownessReason::Unclear {
                busiest_apps,
                slow_workspaces,
            };
            (reason, Some(later))
        }
    }
}

/// One log line's worth of an episode.
fn describe(reason: &SlownessReason) -> String {
    match reason {
        SlownessReason::Disk { free_bytes } => {
            format!("disk almost full; {} bytes free", free_bytes)
        }
        SlownessReason::Database {
            problem,
            slow_workspaces,
        } => format!(
            "database {} in {}",
            match problem {
                DatabaseProblem::NotAnswering => "not answering",
                DatabaseProblem::PoolExhausted => "has no free pooled connection",
            },
            slow_workspaces.join(", ")
        ),
        SlownessReason::Memory { top_users } => format!(
            "short on memory; biggest users: {}",
            processes::describe_memory(top_users)
        ),
        SlownessReason::Unclear {
            busiest_apps,
            slow_workspaces,
        } => format!(
            "slow in {}; busiest apps: {}",
            slow_workspaces.join(", "),
            processes::describe_processor(busiest_apps)
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::processes::UserKind;
    use super::*;
    use crate::stack::ProbeOutcome;

    fn at(sample: i64) -> DateTime<Utc> {
        DateTime::<Utc>::UNIX_EPOCH + chrono::Duration::seconds(sample * 30)
    }

    /// A sample: memory elevated, memory raised, and which workspaces timed
    /// out.
    fn s(elevated: bool, raised: bool, slow: &[&str]) -> Sample {
        Sample {
            memory: MemorySignal { elevated, raised },
            ..signs(raised, slow.iter().map(|w| (*w, SlowSign::NoAnswer)))
        }
    }

    /// A sample with memory raised or not, and a sign per workspace.
    fn signs<'a>(raised: bool, slow: impl IntoIterator<Item = (&'a str, SlowSign)>) -> Sample {
        Sample {
            memory: MemorySignal {
                elevated: false,
                raised,
            },
            slow_workspaces: slow
                .into_iter()
                .map(|(w, sign)| (w.to_string(), sign))
                .collect(),
        }
    }

    const NOT_ANSWERING: SlowSign = SlowSign::Database(DatabaseProblem::NotAnswering);
    const POOL_EXHAUSTED: SlowSign = SlowSign::Database(DatabaseProblem::PoolExhausted);

    /// The dev engine answered promptly, with its database down.
    fn database_down(raised: bool) -> Sample {
        signs(raised, [("dev", NOT_ANSWERING)])
    }

    const SLOW: fn() -> Sample = || s(false, false, &["dev"]);
    const QUIET: fn() -> Sample = || s(false, false, &[]);

    fn replay(c: &mut Classifier, samples: impl IntoIterator<Item = Sample>) -> Vec<Transition> {
        samples
            .into_iter()
            .enumerate()
            .map(|(i, sample)| c.record(sample, at(i as i64)))
            .collect()
    }

    #[test]
    fn a_cold_start_waits_for_a_full_window() {
        let mut c = Classifier::default();
        let t = replay(&mut c, (0..10).map(|_| SLOW()));
        assert_eq!(t[8], Transition::Quiet);
        assert_eq!(t[9], Transition::Opened);
        assert_eq!(c.episode, Some(at(9)));
    }

    #[test]
    fn a_full_window_opens_on_the_eighth_elevated_sample() {
        let mut c = Classifier::default();
        let samples = (0..10).map(|_| QUIET()).chain((0..8).map(|_| SLOW()));
        let t = replay(&mut c, samples);
        assert_eq!(t[16], Transition::Quiet);
        assert_eq!(t[17], Transition::Opened);
    }

    #[test]
    fn the_adr_0182_idle_host_never_opens() {
        // Pressure 2 or 4 on every sample, so memory is raised throughout, but
        // no swap and no slow engine. Nothing may open.
        let mut c = Classifier::default();
        let t = replay(&mut c, (0..40).map(|_| s(false, true, &[])));
        assert!(t.iter().all(|t| *t == Transition::Quiet));
    }

    #[test]
    fn scattered_slow_samples_never_open_one() {
        let pattern = [
            true, false, true, true, false, true, false, true, true, false,
        ];
        let mut c = Classifier::default();
        let samples = pattern
            .iter()
            .cycle()
            .take(40)
            .map(|slow| if *slow { SLOW() } else { QUIET() });
        assert!(replay(&mut c, samples)
            .iter()
            .all(|t| *t == Transition::Quiet));
    }

    #[test]
    fn a_large_mac_short_on_memory_but_not_swapping_names_memory() {
        // The shape of the incident in ADR 0283: slow in 8 of 10 buckets,
        // pressure raised in 7 of them, swap never used.
        let slow = [true, false, true, true, false, true, true, true, true, true];
        let raised = [
            true, true, false, true, true, false, true, true, true, false,
        ];
        let mut c = Classifier::default();
        let samples = slow
            .iter()
            .zip(raised)
            .map(|(slow, raised)| s(false, raised, if *slow { &["dev"] } else { &[] }));
        assert_eq!(replay(&mut c, samples)[9], Transition::Opened);
        assert_eq!(c.cause(), Cause::Memory);
    }

    #[test]
    fn a_busy_processor_with_calm_memory_is_unclear_and_names_the_workspace() {
        // The second case in ADR 0283: pressure normal, every engine slow.
        let mut c = Classifier::default();
        assert_eq!(
            replay(&mut c, (0..10).map(|_| SLOW()))[9],
            Transition::Opened
        );
        assert_eq!(
            c.cause(),
            Cause::Unclear {
                slow_workspaces: vec!["dev".into()]
            }
        );
    }

    #[test]
    fn swapping_alone_still_opens_and_names_memory() {
        let mut c = Classifier::default();
        let t = replay(&mut c, (0..10).map(|_| s(true, true, &[])));
        assert_eq!(t[9], Transition::Opened);
        assert_eq!(c.cause(), Cause::Memory);
    }

    #[test]
    fn slowness_opens_on_a_host_whose_memory_cannot_be_read() {
        let mut c = Classifier::default();
        let unreadable = || Sample {
            memory: MemorySignal::default(),
            slow_workspaces: [("dev".to_string(), SlowSign::NoAnswer)].into(),
        };
        assert_eq!(
            replay(&mut c, (0..10).map(|_| unreadable()))[9],
            Transition::Opened
        );
        assert!(matches!(c.cause(), Cause::Unclear { .. }));
    }

    #[test]
    fn memory_is_named_from_half_the_window() {
        for (raised_count, memory) in [(5, true), (4, false)] {
            let mut c = Classifier::default();
            let samples = (0..10).map(|i| s(false, i < raised_count, &["dev"]));
            replay(&mut c, samples);
            assert_eq!(c.cause() == Cause::Memory, memory, "{raised_count} raised");
        }
    }

    #[test]
    fn a_memory_episode_winding_down_still_names_memory() {
        // Swap opened it with no slow engine. Pressure then eased, so memory
        // is raised in under half the window before the episode closes.
        let mut c = Classifier::default();
        replay(&mut c, (0..10).map(|_| s(true, true, &[])));
        let t = replay(&mut c, (0..7).map(|_| QUIET()));
        assert!(t.iter().all(|t| *t == Transition::Held));
        assert_eq!(c.cause(), Cause::Memory);
    }

    #[test]
    fn the_unclear_reason_names_every_workspace_slow_in_the_window() {
        let mut c = Classifier::default();
        let samples = (0..10).map(|i| match i {
            0 => s(false, false, &["notes"]),
            5 => s(false, false, &["dev", "e2e-test"]),
            _ => SLOW(),
        });
        replay(&mut c, samples);
        assert_eq!(
            c.cause(),
            Cause::Unclear {
                slow_workspaces: vec!["dev".into(), "e2e-test".into(), "notes".into()]
            }
        );
    }

    #[test]
    fn an_episode_holds_through_a_mixed_stretch_and_closes_after_ten_quiet_samples() {
        let mut c = Classifier::default();
        replay(&mut c, (0..10).map(|_| SLOW()));
        let mixed = (0..9).map(|_| QUIET()).chain([SLOW()]);
        assert!(replay(&mut c, mixed).iter().all(|t| *t == Transition::Held));
        let t = replay(&mut c, (0..10).map(|_| QUIET()));
        assert_eq!(t[8], Transition::Held);
        assert_eq!(t[9], Transition::Closed);
        assert_eq!(c.episode, None);
    }

    #[test]
    fn processes_are_scanned_only_while_an_episode_is_open() {
        assert!(Transition::Opened.scans());
        assert!(Transition::Held.scans());
        assert!(!Transition::Quiet.scans());
        assert!(!Transition::Closed.scans());
    }

    #[test]
    fn a_probe_is_slow_only_for_a_live_engine_past_its_boot() {
        use DatabaseHealth::*;
        use ProbeOutcome::*;
        // (outcome, alive, past boot, database health) -> sign
        let table = [
            (Healthy, true, true, Reachable, None),
            (Healthy, true, true, NotAnswering, Some(NOT_ANSWERING)),
            (Healthy, true, false, NotAnswering, Some(NOT_ANSWERING)),
            (Healthy, true, true, PoolExhausted, Some(POOL_EXHAUSTED)),
            (Slow, true, true, Reachable, Some(SlowSign::NoAnswer)),
            (Slow, true, false, Reachable, None),
            (Slow, false, true, Reachable, None),
            (Unreachable, true, true, NotAnswering, None),
            (Unreachable, false, true, Reachable, None),
            (Other, true, true, NotAnswering, None),
        ];
        for (outcome, alive, past_boot, database, sign) in table {
            assert_eq!(
                slow_sign(outcome, alive, past_boot, database),
                sign,
                "{outcome:?} alive={alive} past_boot={past_boot} database={database:?}"
            );
        }
    }

    #[test]
    fn the_supervisor_feed_counts_once_in_the_next_sample() {
        let watch = Slowness::default();
        watch.record_slow("dev", NOT_ANSWERING);
        watch.record_slow("dev", NOT_ANSWERING);
        watch.record_slow("notes", POOL_EXHAUSTED);
        let taken = std::mem::take(&mut *watch.slow_since_sample.lock().unwrap());
        assert_eq!(
            taken.into_iter().collect::<Vec<_>>(),
            [
                ("dev".to_string(), NOT_ANSWERING),
                ("notes".to_string(), POOL_EXHAUSTED)
            ]
        );
        assert!(watch.slow_since_sample.lock().unwrap().is_empty());
    }

    #[test]
    fn a_timeout_outweighs_a_database_sign_in_the_same_sample() {
        // Either order: the engine that stopped answering is the stronger
        // evidence, since a starved host also makes its database probe fail.
        for order in [
            [NOT_ANSWERING, SlowSign::NoAnswer],
            [SlowSign::NoAnswer, NOT_ANSWERING],
        ] {
            let watch = Slowness::default();
            for sign in order {
                watch.record_slow("dev", sign);
            }
            assert_eq!(
                watch.slow_since_sample.lock().unwrap()["dev"],
                SlowSign::NoAnswer
            );
        }
        assert!(POOL_EXHAUSTED < NOT_ANSWERING);
    }

    #[test]
    fn a_stuck_database_behind_a_prompt_engine_names_the_database() {
        // The incident in ADR 0301: the disk filled, Postgres stalled, and the
        // engine kept answering its probe with the database down.
        let mut c = Classifier::default();
        let t = replay(&mut c, (0..10).map(|_| database_down(false)));
        assert_eq!(t[9], Transition::Opened);
        assert_eq!(
            c.cause(),
            Cause::Database {
                problem: DatabaseProblem::NotAnswering,
                slow_workspaces: vec!["dev".into()]
            }
        );
    }

    #[test]
    fn a_stuck_database_is_named_even_with_memory_raised() {
        let mut c = Classifier::default();
        replay(&mut c, (0..10).map(|_| database_down(true)));
        assert!(matches!(c.cause(), Cause::Database { .. }));
    }

    #[test]
    fn a_starved_host_whose_database_flaps_still_names_memory() {
        // ADR 0283's memory incident: the engine itself timed out, and DbHealth
        // flapped in the same samples. Each sample goes through the real feed,
        // so the timeouts win and the memory rule decides.
        let mut c = Classifier::default();
        let watch = Slowness::default();
        let samples: Vec<Sample> = (0..10)
            .map(|i| {
                watch.record_slow("dev", NOT_ANSWERING);
                watch.record_slow("dev", SlowSign::NoAnswer);
                Sample {
                    memory: MemorySignal {
                        elevated: false,
                        raised: i % 3 != 0,
                    },
                    slow_workspaces: std::mem::take(&mut *watch.slow_since_sample.lock().unwrap()),
                }
            })
            .collect();
        assert_eq!(replay(&mut c, samples)[9], Transition::Opened);
        assert_eq!(c.cause(), Cause::Memory);
    }

    #[test]
    fn the_database_is_named_from_half_the_window() {
        for (database_count, database) in [(5, true), (4, false)] {
            let mut c = Classifier::default();
            let samples = (0..10).map(|i| {
                if i < database_count {
                    database_down(false)
                } else {
                    SLOW()
                }
            });
            replay(&mut c, samples);
            assert_eq!(
                matches!(c.cause(), Cause::Database { .. }),
                database,
                "{database_count} database samples"
            );
        }
    }

    #[test]
    fn the_database_problem_is_the_latest_one_seen() {
        let mut c = Classifier::default();
        let samples = (0..10).map(|i| {
            if i < 7 {
                database_down(false)
            } else {
                signs(false, [("notes", POOL_EXHAUSTED)])
            }
        });
        replay(&mut c, samples);
        assert_eq!(
            c.cause(),
            Cause::Database {
                problem: DatabaseProblem::PoolExhausted,
                slow_workspaces: vec!["dev".into(), "notes".into()]
            }
        );
    }

    #[test]
    fn the_database_reason_lists_only_workspaces_with_a_database_sign() {
        let mut c = Classifier::default();
        let samples = (0..10).map(|_| {
            signs(
                false,
                [("dev", NOT_ANSWERING), ("notes", SlowSign::NoAnswer)],
            )
        });
        replay(&mut c, samples);
        assert_eq!(
            c.cause(),
            Cause::Database {
                problem: DatabaseProblem::NotAnswering,
                slow_workspaces: vec!["dev".into()]
            }
        );
    }

    #[test]
    fn a_critically_low_disk_names_the_disk_whatever_the_cause() {
        let causes = || {
            [
                Cause::Database {
                    problem: DatabaseProblem::NotAnswering,
                    slow_workspaces: vec!["dev".into()],
                },
                Cause::Memory,
                Cause::Unclear {
                    slow_workspaces: vec!["dev".into()],
                },
            ]
        };
        for cause in causes() {
            let (reason, scan) =
                attribute(cause, Some(400_000_000), Some(processes::scan()), Vec::new);
            assert_eq!(
                reason,
                SlownessReason::Disk {
                    free_bytes: 400_000_000
                }
            );
            assert!(scan.is_none(), "a stale baseline is dropped");
        }
    }

    #[test]
    fn a_disk_with_room_or_unread_leaves_the_database_reason() {
        for free in [Some(2_000_000_000), None] {
            let cause = Cause::Database {
                problem: DatabaseProblem::PoolExhausted,
                slow_workspaces: vec!["dev".into()],
            };
            let (reason, scan) = attribute(cause, free, Some(processes::scan()), Vec::new);
            assert_eq!(
                reason,
                SlownessReason::Database {
                    problem: DatabaseProblem::PoolExhausted,
                    slow_workspaces: vec!["dev".into()]
                }
            );
            assert!(
                scan.is_none(),
                "the database reason drops the baseline, so a later unclear reason measures fresh"
            );
        }
    }

    #[test]
    fn the_snapshot_starts_normal() {
        assert_eq!(Slowness::default().snapshot(), SlownessStatus::Normal);
    }

    #[test]
    fn the_status_serializes_with_a_state_and_a_reason_tag() {
        let memory = serde_json::to_value(SlownessStatus::Slow {
            episode_id: "e".into(),
            reason: SlownessReason::Memory {
                top_users: vec![MemoryUser {
                    name: "Google Chrome".into(),
                    bytes: 7,
                    kind: UserKind::App,
                }],
            },
        })
        .unwrap();
        assert_eq!(
            memory,
            serde_json::json!({
                "state": "slow",
                "episode_id": "e",
                "reason": "memory",
                "top_users": [{ "name": "Google Chrome", "bytes": 7, "kind": "app" }],
            })
        );

        let unclear = serde_json::to_value(SlownessStatus::Slow {
            episode_id: "e".into(),
            reason: SlownessReason::Unclear {
                busiest_apps: vec![ProcessorUser {
                    name: "Lucidos".into(),
                    percent: 12,
                    kind: UserKind::Lucidos,
                }],
                slow_workspaces: vec!["dev".into()],
            },
        })
        .unwrap();
        assert_eq!(
            unclear,
            serde_json::json!({
                "state": "slow",
                "episode_id": "e",
                "reason": "unclear",
                "busiest_apps": [{ "name": "Lucidos", "percent": 12, "kind": "lucidos" }],
                "slow_workspaces": ["dev"],
            })
        );

        let database = serde_json::to_value(SlownessStatus::Slow {
            episode_id: "e".into(),
            reason: SlownessReason::Database {
                problem: DatabaseProblem::NotAnswering,
                slow_workspaces: vec!["dev".into()],
            },
        })
        .unwrap();
        assert_eq!(
            database,
            serde_json::json!({
                "state": "slow",
                "episode_id": "e",
                "reason": "database",
                "problem": "not_answering",
                "slow_workspaces": ["dev"],
            })
        );

        let disk = serde_json::to_value(SlownessStatus::Slow {
            episode_id: "e".into(),
            reason: SlownessReason::Disk { free_bytes: 7 },
        })
        .unwrap();
        assert_eq!(
            disk,
            serde_json::json!({
                "state": "slow",
                "episode_id": "e",
                "reason": "disk",
                "free_bytes": 7,
            })
        );

        assert_eq!(
            serde_json::to_value(SlownessStatus::Normal).unwrap(),
            serde_json::json!({ "state": "normal" })
        );
    }
}
