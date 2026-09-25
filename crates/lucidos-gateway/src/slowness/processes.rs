//! Who holds the memory, and who keeps the processor busy: every readable
//! process, summed by app.
//!
//! macOS measures the physical footprint, which counts compressed pages. RSS
//! does not, and an idle process under pressure is mostly compressed. Linux
//! adds `VmSwap` to `VmRSS` for the same reason. Processes owned by another
//! user (root daemons) are unreadable and are skipped.
//!
//! Processor time is cumulative, so a share needs two scans: the busiest apps
//! are the ones whose time grew most between them.

use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

/// One group's share of memory, as the banner lists it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MemoryUser {
    pub name: String,
    pub bytes: u64,
    pub kind: UserKind,
}

/// One group's share of the processor, as the banner lists it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProcessorUser {
    pub name: String,
    /// Percent of the whole computer's processing capacity, all cores
    /// together, so 100 means every core was busy.
    pub percent: u32,
    pub kind: UserKind,
}

/// What the banner may recommend about a group. Only an `App` is something the
/// user can quit by name; a `Process` may be a system service.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UserKind {
    Lucidos,
    App,
    Process,
}

#[derive(Debug, Clone)]
struct ProcessRow {
    pid: i32,
    ppid: i32,
    path: String,
    bytes: u64,
    /// User plus system time since the process started.
    cpu: Duration,
}

/// Every readable process at one moment.
#[derive(Debug, Clone)]
pub struct Scan {
    at: Instant,
    rows: Vec<ProcessRow>,
}

/// Read every process now. Blocking: one set of syscalls or file reads per
/// process.
pub fn scan() -> Scan {
    Scan {
        at: Instant::now(),
        rows: platform::rows(),
    }
}

const LUCIDOS: &str = "Lucidos";

/// Groups the banner names, before Lucidos is appended.
const SHOWN: usize = 3;

/// `lucidos_roots` are the process trees that count as Lucidos. The gateway
/// supplies them: itself, each running engine, and the embedded postmaster.
pub fn top_users(scan: &Scan, lucidos_roots: &[i32]) -> Vec<MemoryUser> {
    group(&scan.rows, lucidos_roots, |row| row.bytes)
        .into_iter()
        .map(|g| MemoryUser {
            name: g.name,
            bytes: g.amount,
            kind: g.kind,
        })
        .collect()
}

/// The groups whose processor time grew most from `earlier` to `later`. A
/// group under one percent is left out: it is not busy.
pub fn busiest_apps(earlier: &Scan, later: &Scan, lucidos_roots: &[i32]) -> Vec<ProcessorUser> {
    let before: HashMap<i32, Duration> = earlier.rows.iter().map(|r| (r.pid, r.cpu)).collect();
    let capacity = later.at.duration_since(earlier.at)
        * std::thread::available_parallelism().map_or(1, |n| n.get() as u32);
    group(&later.rows, lucidos_roots, |row| {
        // A pid missing from the first scan, or reused since, ran only
        // between the two, so all of its time counts.
        let start = before.get(&row.pid).filter(|t| **t <= row.cpu);
        (row.cpu - start.copied().unwrap_or_default()).as_nanos() as u64
    })
    .into_iter()
    .map(|g| ProcessorUser {
        percent: percent_of(g.amount, capacity),
        name: g.name,
        kind: g.kind,
    })
    .filter(|u| u.percent >= 1)
    .collect()
}

/// `busy_nanos` as a rounded percent of `capacity`, capped at 100.
fn percent_of(busy_nanos: u64, capacity: Duration) -> u32 {
    let capacity = capacity.as_nanos();
    if capacity == 0 {
        return 0;
    }
    ((u128::from(busy_nanos) * 100 + capacity / 2) / capacity).min(100) as u32
}

/// "Google Chrome 7.0 GB, Slack 0.6 GB", for the log.
pub fn describe_memory(users: &[MemoryUser]) -> String {
    users
        .iter()
        .map(|u| format!("{} {:.1} GB", u.name, u.bytes as f64 / 1e9))
        .collect::<Vec<_>>()
        .join(", ")
}

/// "Xcode 40%, Lucidos 12%", for the log.
pub fn describe_processor(users: &[ProcessorUser]) -> String {
    users
        .iter()
        .map(|u| format!("{} {}%", u.name, u.percent))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The outermost `.app` bundle's name in an executable path. A browser's
/// helpers live in bundles nested inside its own, so the outermost names the app.
fn app_name(path: &str) -> Option<&str> {
    let (i, _) = path.match_indices(".app/").next()?;
    path[..i].rsplit('/').next()
}

fn executable_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

const VIRTUAL_MACHINES: &str = "Virtual machines (such as Docker)";
const WEB_PAGES: &str = "Web pages in Safari and other apps";

/// A plain name for a system program the user would not recognise by its
/// executable. It stays a `Process`, so the banner still never says "quit" it.
fn process_name(executable: &str) -> &str {
    match executable {
        "com.apple.Virtualization.VirtualMachine" => VIRTUAL_MACHINES,
        name if name.starts_with("qemu-system-") => VIRTUAL_MACHINES,
        "com.apple.WebKit.WebContent" | "com.apple.WebKit.GPU" => WEB_PAGES,
        name => name,
    }
}

/// One group's summed amount, in whatever unit the caller measured.
#[derive(Debug, Clone, PartialEq)]
struct Group {
    name: String,
    amount: u64,
    kind: UserKind,
}

/// The biggest [`SHOWN`] groups by `amount`. Lucidos follows if it missed that
/// cut, so the user always sees its share next to the others.
fn group(
    rows: &[ProcessRow],
    lucidos_roots: &[i32],
    amount: impl Fn(&ProcessRow) -> u64,
) -> Vec<Group> {
    let lucidos_tree = descendants(rows, lucidos_roots);

    let mut totals: HashMap<(&str, UserKind), u64> = HashMap::new();
    for row in rows {
        let app = app_name(&row.path);
        // Any Lucidos.app counts, so the desktop app and a second install on
        // the same machine never show as a separate "Lucidos".
        let key = if lucidos_tree.contains(&row.pid) || app == Some(LUCIDOS) {
            (LUCIDOS, UserKind::Lucidos)
        } else if let Some(app) = app {
            (app, UserKind::App)
        } else {
            (process_name(executable_name(&row.path)), UserKind::Process)
        };
        *totals.entry(key).or_default() += amount(row);
    }

    let mut groups: Vec<Group> = totals
        .into_iter()
        .map(|((name, kind), amount)| Group {
            name: name.to_string(),
            amount,
            kind,
        })
        .collect();
    groups.sort_by(|a, b| b.amount.cmp(&a.amount).then_with(|| a.name.cmp(&b.name)));
    let lucidos_rank = groups.iter().position(|g| g.kind == UserKind::Lucidos);
    let lucidos_group = lucidos_rank
        .filter(|rank| *rank >= SHOWN)
        .map(|rank| groups[rank].clone());
    groups.truncate(SHOWN);
    groups.extend(lucidos_group);
    groups
}

fn descendants(rows: &[ProcessRow], roots: &[i32]) -> HashSet<i32> {
    let mut children: HashMap<i32, Vec<i32>> = HashMap::new();
    for row in rows {
        children.entry(row.ppid).or_default().push(row.pid);
    }
    let mut seen: HashSet<i32> = roots.iter().copied().collect();
    let mut stack: Vec<i32> = roots.to_vec();
    while let Some(pid) = stack.pop() {
        for child in children.get(&pid).into_iter().flatten() {
            if seen.insert(*child) {
                stack.push(*child);
            }
        }
    }
    seen
}

#[cfg(target_os = "macos")]
mod platform {
    use super::ProcessRow;
    use std::mem::{size_of, MaybeUninit};
    use std::sync::OnceLock;
    use std::time::Duration;

    pub(super) fn rows() -> Vec<ProcessRow> {
        // SAFETY: a null buffer asks only for the count.
        let count = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
        if count <= 0 {
            return Vec::new();
        }
        // Headroom for processes born between the two calls.
        let mut pids = vec![0i32; count as usize + 64];
        let bytes = (pids.len() * size_of::<i32>()) as libc::c_int;
        // SAFETY: the buffer holds `bytes` bytes of pids.
        let filled = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast(), bytes) };
        pids.truncate(filled.max(0) as usize);
        pids.into_iter()
            .filter(|pid| *pid > 0)
            .filter_map(row)
            .collect()
    }

    /// `rusage_info` reports processor time in Mach absolute-time ticks, not
    /// nanoseconds. On Apple silicon one tick is about 42 ns.
    fn ticks_to_duration(ticks: u64) -> Duration {
        static TIMEBASE: OnceLock<(u64, u64)> = OnceLock::new();
        let (numer, denom) = *TIMEBASE.get_or_init(|| {
            let mut info = mach2::mach_time::mach_timebase_info { numer: 0, denom: 0 };
            // SAFETY: the kernel fills the two-field struct it is handed.
            let rc = unsafe { mach2::mach_time::mach_timebase_info(&mut info) };
            if rc == 0 && info.denom != 0 {
                (u64::from(info.numer), u64::from(info.denom))
            } else {
                (1, 1)
            }
        });
        Duration::from_nanos((u128::from(ticks) * u128::from(numer) / u128::from(denom)) as u64)
    }

    fn row(pid: i32) -> Option<ProcessRow> {
        let mut usage = MaybeUninit::<libc::rusage_info_v2>::zeroed();
        // SAFETY: the buffer is a `rusage_info_v2`, the flavor asked for.
        let rc = unsafe {
            libc::proc_pid_rusage(
                pid,
                libc::RUSAGE_INFO_V2,
                usage.as_mut_ptr() as *mut libc::rusage_info_t,
            )
        };
        if rc != 0 {
            return None;
        }
        // SAFETY: success fills the struct.
        let usage = unsafe { usage.assume_init() };

        let mut bsd = MaybeUninit::<libc::proc_bsdinfo>::zeroed();
        let size = size_of::<libc::proc_bsdinfo>() as libc::c_int;
        // SAFETY: the buffer is `size` bytes of `proc_bsdinfo`.
        let got = unsafe {
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDTBSDINFO,
                0,
                bsd.as_mut_ptr().cast(),
                size,
            )
        };
        if got != size {
            return None;
        }
        // SAFETY: a full-size answer fills the struct.
        let ppid = unsafe { bsd.assume_init() }.pbi_ppid as i32;

        let mut path = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
        // SAFETY: the buffer is `path.len()` bytes.
        let len = unsafe { libc::proc_pidpath(pid, path.as_mut_ptr().cast(), path.len() as u32) };
        if len <= 0 {
            return None;
        }
        path.truncate(len as usize);
        Some(ProcessRow {
            pid,
            ppid,
            path: String::from_utf8_lossy(&path).into_owned(),
            bytes: usage.ri_phys_footprint,
            cpu: ticks_to_duration(usage.ri_user_time.saturating_add(usage.ri_system_time)),
        })
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use super::ProcessRow;
    use std::sync::OnceLock;
    use std::time::Duration;

    pub(super) fn rows() -> Vec<ProcessRow> {
        let Ok(entries) = std::fs::read_dir("/proc") else {
            return Vec::new();
        };
        entries
            .flatten()
            .filter_map(|e| e.file_name().to_str()?.parse::<i32>().ok())
            .filter_map(row)
            .collect()
    }

    /// `/proc/<pid>/stat` counts processor time in clock ticks.
    fn ticks_to_duration(ticks: u64) -> Duration {
        static PER_SECOND: OnceLock<u64> = OnceLock::new();
        // SAFETY: `sysconf` only reads a configuration value.
        let per_second = *PER_SECOND.get_or_init(|| {
            u64::try_from(unsafe { libc::sysconf(libc::_SC_CLK_TCK) }).unwrap_or(100)
        });
        Duration::from_nanos(ticks.saturating_mul(1_000_000_000) / per_second.max(1))
    }

    fn row(pid: i32) -> Option<ProcessRow> {
        let dir = std::path::Path::new("/proc").join(pid.to_string());
        let stat = super::parse_stat(&std::fs::read_to_string(dir.join("stat")).ok()?)?;
        let bytes = super::parse_status_bytes(&std::fs::read_to_string(dir.join("status")).ok()?)?;
        // Kernel threads have no executable and hold no user memory.
        let path = std::fs::read_link(dir.join("exe")).ok()?;
        Some(ProcessRow {
            pid,
            ppid: stat.ppid,
            path: path.to_string_lossy().into_owned(),
            bytes,
            cpu: ticks_to_duration(stat.cpu_ticks),
        })
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
mod platform {
    pub(super) fn rows() -> Vec<super::ProcessRow> {
        Vec::new()
    }
}

/// The fields of `/proc/<pid>/stat` the scan uses.
#[cfg(any(target_os = "linux", test))]
#[derive(Debug, PartialEq)]
struct Stat {
    ppid: i32,
    /// `utime` plus `stime`, in clock ticks.
    cpu_ticks: u64,
}

/// Parse `/proc/<pid>/stat`. The command name sits in parentheses and may
/// itself contain spaces or parentheses, so parse after the LAST closing one.
/// From there, field 1 is the parent pid and fields 11 and 12 are the times.
#[cfg(any(target_os = "linux", test))]
fn parse_stat(stat: &str) -> Option<Stat> {
    let (_, rest) = stat.rsplit_once(')')?;
    let fields: Vec<&str> = rest.split_whitespace().collect();
    let tick = |i: usize| fields.get(i)?.parse::<u64>().ok();
    Some(Stat {
        ppid: fields.get(1)?.parse().ok()?,
        cpu_ticks: tick(11)? + tick(12)?,
    })
}

/// `VmRSS` plus `VmSwap` from `/proc/<pid>/status`, in bytes.
#[cfg(any(target_os = "linux", test))]
fn parse_status_bytes(status: &str) -> Option<u64> {
    let kb = |key: &str| {
        status
            .lines()
            .find_map(|l| l.strip_prefix(key))?
            .split_whitespace()
            .next()?
            .parse::<u64>()
            .ok()
    };
    Some((kb("VmRSS:")? + kb("VmSwap:").unwrap_or(0)) * 1024)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MB: u64 = 1_000_000;

    fn row(pid: i32, ppid: i32, path: &str, mb: u64) -> ProcessRow {
        ProcessRow {
            pid,
            ppid,
            path: path.to_string(),
            bytes: mb * MB,
            cpu: Duration::ZERO,
        }
    }

    const CHROME: &str = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
    const CHROME_HELPER: &str = "/Applications/Google Chrome.app/Contents/Frameworks/\
        Google Chrome Framework.framework/Versions/1/Helpers/\
        Google Chrome Helper (Renderer).app/Contents/MacOS/Google Chrome Helper (Renderer)";
    const XCODE: &str = "/Applications/Xcode.app/Contents/MacOS/Xcode";

    fn table() -> Vec<ProcessRow> {
        vec![
            // The gateway, two engines, and a coding agent under one of them.
            row(
                100,
                1,
                "/Users/me/lucidos/target/release/lucidos-gateway",
                60,
            ),
            row(
                101,
                100,
                "/Users/me/lucidos/target/release/lucidos-engine",
                865,
            ),
            row(
                102,
                100,
                "/Users/me/lucidos/target/release/lucidos-engine",
                400,
            ),
            row(103, 101, "/opt/homebrew/bin/node", 300),
            // A daemonized postmaster and two backends.
            row(200, 1, "/usr/local/pgsql/bin/postgres", 50),
            row(201, 200, "/usr/local/pgsql/bin/postgres", 20),
            row(202, 200, "/usr/local/pgsql/bin/postgres", 20),
            // Chrome and three helpers.
            row(300, 1, CHROME, 400),
            row(301, 300, CHROME_HELPER, 2800),
            row(302, 300, CHROME_HELPER, 2000),
            row(303, 300, CHROME_HELPER, 1800),
            row(400, 1, "/Applications/Slack.app/Contents/MacOS/Slack", 600),
            row(500, 1, "/usr/local/bin/some-daemon", 1000),
        ]
    }

    const ROOTS: &[i32] = &[100, 200];

    fn memory_scan(rows: Vec<ProcessRow>) -> Scan {
        Scan {
            at: Instant::now(),
            rows,
        }
    }

    fn user(name: &str, mb: u64, kind: UserKind) -> MemoryUser {
        MemoryUser {
            name: name.to_string(),
            bytes: mb * MB,
            kind,
        }
    }

    #[test]
    fn lucidos_trees_and_app_helpers_each_sum_to_one_group() {
        assert_eq!(
            top_users(&memory_scan(table()), ROOTS),
            vec![
                user("Google Chrome", 7000, UserKind::App),
                user("Lucidos", 1715, UserKind::Lucidos),
                user("some-daemon", 1000, UserKind::Process),
            ]
        );
    }

    #[test]
    fn lucidos_is_appended_when_it_misses_the_cut() {
        let mut rows = table();
        rows.push(row(600, 1, XCODE, 3000));
        rows.push(row(
            700,
            1,
            "/Applications/Figma.app/Contents/MacOS/Figma",
            2000,
        ));
        let names: Vec<_> = top_users(&memory_scan(rows), ROOTS)
            .into_iter()
            .map(|u| u.name)
            .collect();
        assert_eq!(names, ["Google Chrome", "Xcode", "Figma", "Lucidos"]);
    }

    #[test]
    fn any_lucidos_app_bundle_counts_as_lucidos() {
        let mut rows = table();
        rows.push(row(
            800,
            1,
            "/Applications/Lucidos.app/Contents/MacOS/Lucidos",
            200,
        ));
        let lucidos: Vec<_> = top_users(&memory_scan(rows), ROOTS)
            .into_iter()
            .filter(|u| u.name == "Lucidos")
            .collect();
        assert_eq!(lucidos, [user("Lucidos", 1915, UserKind::Lucidos)]);
    }

    #[test]
    fn a_system_service_is_a_process_with_a_plain_name() {
        let vm = "/System/Library/Frameworks/Virtualization.framework/Versions/A/XPCServices/\
            com.apple.Virtualization.VirtualMachine.xpc/Contents/MacOS/\
            com.apple.Virtualization.VirtualMachine";
        assert_eq!(
            top_users(&memory_scan(vec![row(1, 0, vm, 3300)]), &[]),
            [user(VIRTUAL_MACHINES, 3300, UserKind::Process)]
        );
        assert_eq!(
            top_users(
                &memory_scan(vec![row(1, 0, "/usr/bin/qemu-system-aarch64", 900)]),
                &[]
            ),
            [user(VIRTUAL_MACHINES, 900, UserKind::Process)]
        );
    }

    #[test]
    fn webkit_content_and_gpu_processes_sum_to_one_plain_group() {
        let services = "/System/Library/Frameworks/WebKit.framework/Versions/A/XPCServices";
        let content = format!(
            "{services}/com.apple.WebKit.WebContent.xpc/Contents/MacOS/com.apple.WebKit.WebContent"
        );
        let gpu =
            format!("{services}/com.apple.WebKit.GPU.xpc/Contents/MacOS/com.apple.WebKit.GPU");
        let rows = vec![
            row(1, 0, &content, 500),
            row(2, 0, &content, 300),
            row(3, 0, &gpu, 200),
        ];
        assert_eq!(
            top_users(&memory_scan(rows), &[]),
            [user(WEB_PAGES, 1000, UserKind::Process)]
        );
    }

    /// Two scans ten seconds apart, where each `(pid, before, after)` names
    /// seconds of processor time.
    fn scans(times: &[(i32, u64, u64)]) -> (Scan, Scan) {
        let at = Instant::now();
        let with = |pick: fn(&(i32, u64, u64)) -> u64| -> Vec<ProcessRow> {
            table()
                .into_iter()
                .chain([row(600, 1, XCODE, 3000)])
                .filter_map(|mut r| {
                    let t = times.iter().find(|t| t.0 == r.pid)?;
                    r.cpu = Duration::from_secs(pick(t));
                    Some(r)
                })
                .collect()
        };
        (
            Scan {
                at,
                rows: with(|t| t.1),
            },
            Scan {
                at: at + Duration::from_secs(10),
                rows: with(|t| t.2),
            },
        )
    }

    fn cores() -> u64 {
        std::thread::available_parallelism().map_or(1, |n| n.get() as u64)
    }

    #[test]
    fn the_busiest_apps_are_ranked_by_growth_not_by_total_time() {
        // Ten seconds on every core is 100%. Chrome has run for hours, but
        // only Xcode and an engine grew in this interval.
        let full = 10 * cores();
        let (earlier, later) = scans(&[
            (300, 9000, 9000),
            (600, 100, 100 + full / 2),
            (101, 50, 50 + full / 5),
        ]);
        assert_eq!(
            busiest_apps(&earlier, &later, ROOTS),
            vec![
                ProcessorUser {
                    name: "Xcode".into(),
                    percent: 50,
                    kind: UserKind::App
                },
                ProcessorUser {
                    name: "Lucidos".into(),
                    percent: 20,
                    kind: UserKind::Lucidos
                },
            ]
        );
    }

    #[test]
    fn a_process_born_between_the_scans_counts_all_its_time() {
        let full = 10 * cores();
        let (earlier, mut later) = scans(&[(600, 0, full / 4)]);
        let earlier = Scan {
            rows: Vec::new(),
            ..earlier
        };
        later.rows.retain(|r| r.pid == 600);
        assert_eq!(busiest_apps(&earlier, &later, &[])[0].percent, 25);
    }

    #[test]
    fn a_reused_pid_never_underflows() {
        let (earlier, later) = scans(&[(600, 500, 3)]);
        let busiest = busiest_apps(&earlier, &later, &[]);
        assert!(busiest.iter().all(|u| u.percent <= 100));
    }

    #[test]
    fn nothing_busy_means_an_empty_list() {
        let (earlier, later) = scans(&[(300, 10, 10), (600, 5, 5)]);
        assert!(busiest_apps(&earlier, &later, ROOTS).is_empty());
    }

    #[test]
    fn percent_rounds_and_caps() {
        let ten = Duration::from_secs(10);
        assert_eq!(percent_of(5_000_000_000, ten), 50);
        assert_eq!(percent_of(49_000_000, ten), 0);
        assert_eq!(percent_of(51_000_000, ten), 1);
        assert_eq!(percent_of(30_000_000_000, ten), 100);
        assert_eq!(percent_of(1, Duration::ZERO), 0);
    }

    #[test]
    fn an_unknown_process_keeps_its_executable_name() {
        assert_eq!(process_name("some-daemon"), "some-daemon");
    }

    #[test]
    fn app_name_takes_the_outermost_bundle() {
        assert_eq!(app_name(CHROME_HELPER), Some("Google Chrome"));
        assert_eq!(app_name("/usr/bin/node"), None);
        assert_eq!(executable_name("/usr/bin/node"), "node");
    }

    #[test]
    fn describe_formats_gigabytes_and_percent() {
        assert_eq!(
            describe_memory(&[
                user("Google Chrome", 7000, UserKind::App),
                user("Slack", 600, UserKind::App)
            ]),
            "Google Chrome 7.0 GB, Slack 0.6 GB"
        );
        assert_eq!(
            describe_processor(&[ProcessorUser {
                name: "Xcode".into(),
                percent: 40,
                kind: UserKind::App
            }]),
            "Xcode 40%"
        );
    }

    #[test]
    fn stat_parsing_survives_a_command_with_parentheses() {
        let stat = "42 (a (b) c) S 7 42 42 0 -1 4194560 100 0 0 0 250 30 0 0 20 0 1";
        assert_eq!(
            parse_stat(stat),
            Some(Stat {
                ppid: 7,
                cpu_ticks: 280
            })
        );
        assert_eq!(parse_stat("42 (short) S 7 42"), None);
        assert_eq!(parse_stat("garbage"), None);
    }

    #[test]
    fn status_parsing_adds_swap_to_rss() {
        let status = "Name:\tnode\nVmRSS:\t  1000 kB\nVmSwap:\t   24 kB\n";
        assert_eq!(parse_status_bytes(status), Some(1024 * 1024));
        assert_eq!(
            parse_status_bytes("VmRSS:\t 1 kB\n"),
            Some(1024),
            "no VmSwap line means no swap"
        );
        assert_eq!(parse_status_bytes("Name:\tkthreadd\n"), None);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn this_mac_lists_the_test_process_itself_with_its_processor_time() {
        let me = std::process::id() as i32;
        // Burn a little processor time so the reading cannot be zero.
        let spin = Instant::now();
        while spin.elapsed() < Duration::from_millis(20) {
            std::hint::black_box(0u64);
        }
        let rows = platform::rows();
        let mine = rows.iter().find(|r| r.pid == me).expect("the test process");
        assert!(mine.bytes > 0);
        assert!(mine.cpu >= Duration::from_millis(10), "cpu {:?}", mine.cpu);
    }
}
