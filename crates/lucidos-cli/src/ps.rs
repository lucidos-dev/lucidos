//! `lucidos ps`: the *ps stand-in* for a sandboxed macOS agent.
//!
//! macOS refuses to exec a setuid binary from a seatbelt sandbox, and `/bin/ps`
//! is setuid root (ADR 0645). This reads the same kernel facts through libproc
//! and sysctl, which need no privilege for this user's processes. The engine
//! puts a `ps` shim that runs it first on a sandboxed agent's `PATH`.
//!
//! A process guard reads its answer, so a fact the kernel withholds is never
//! guessed. Unreadable argv prints as `(comm)`. An unreadable number prints
//! as 0. An option or field outside the list in `HELP` exits 2.

use std::fmt::Write as _;
use std::io::Write as _;

use crate::workspace::BoxError;

const HELP: &str = "\
lucidos ps: a stand-in for macOS `ps` inside an agent's sandbox.

Options: aux, ax, -A -e (every process), -a (every user), -x (no terminal
too), -p PIDS, -u USERS (effective), -U USERS (real), -f, -j, -w,
-o FIELDS (name or name=HEADER).
Fields: pid ppid pgid uid user tty tt lstart start stime stat comm command args
rss vsz etime time %cpu %mem nice pri c sess jobc.

%cpu is the average over the process lifetime, not the kernel's decayed
figure. A value the kernel withholds from an unprivileged reader prints as 0.
-E is refused: another process's environment can hold credentials.
Anything else exits 2. Sandbox-safe alternatives: `pgrep -lf`, `lsof`.
";

pub(crate) struct PsArgs {
    pub(crate) args: Vec<String>,
}

pub(crate) fn run(args: PsArgs) -> Result<u8, BoxError> {
    if args.args.iter().any(|a| a == "--help") {
        print_all(HELP)?;
        return Ok(0);
    }
    let spec = match Spec::parse(&args.args) {
        Ok(spec) => spec,
        Err(message) => return Ok(refuse(&message)),
    };
    use_locale_time();
    let snapshot = match kernel::snapshot() {
        Ok(snapshot) => snapshot,
        Err(message) => return Ok(refuse(&message)),
    };
    let rows: Vec<&Row> = snapshot
        .rows
        .iter()
        .filter(|row| spec.select.matches(row, &snapshot.me))
        .collect();
    print_all(&render(&spec, &rows, snapshot.now))?;
    Ok(if rows.is_empty() { 1 } else { 0 })
}

/// `ps aux | head` closes the pipe early, and `/bin/ps` exits quietly then.
fn print_all(text: &str) -> Result<(), BoxError> {
    match std::io::stdout().lock().write_all(text.as_bytes()) {
        Err(e) if e.kind() != std::io::ErrorKind::BrokenPipe => Err(e.into()),
        _ => Ok(()),
    }
}

fn refuse(message: &str) -> u8 {
    eprintln!(
        "ps (Lucidos stand-in): {message}. Use `pgrep -lf` or `lsof`, or see `lucidos ps --help`."
    );
    2
}

/// One process, as the kernel reports it.
// Only the macOS reader builds these; elsewhere `kernel` refuses.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
#[derive(Debug, Clone, Default)]
pub(crate) struct Row {
    pub(crate) pid: u32,
    pub(crate) ppid: u32,
    pub(crate) pgid: u32,
    /// The effective user, which `-u` and the default selection match.
    pub(crate) uid: u32,
    pub(crate) user: String,
    /// The real user, which `-U` matches.
    pub(crate) ruid: u32,
    pub(crate) ruser: String,
    /// `ttys003`, or `??` without a terminal device.
    pub(crate) tty: String,
    /// Holds a controlling terminal, which selection without `x` requires.
    pub(crate) controlled: bool,
    pub(crate) started: i64,
    pub(crate) rss_kb: u64,
    pub(crate) vsz_kb: u64,
    pub(crate) cpu_ns: u64,
    pub(crate) mem_percent: f64,
    pub(crate) nice: i32,
    pub(crate) priority: i32,
    pub(crate) jobc: u32,
    pub(crate) state: String,
    pub(crate) comm: String,
    pub(crate) argv: Option<Vec<String>>,
}

#[cfg(any(target_os = "macos", test))]
const NO_TTY: &str = "??";

/// The caller's effective user, for the default selection.
// Only the macOS reader builds these; elsewhere `kernel` refuses.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
#[derive(Debug, Clone, Default)]
pub(crate) struct Me {
    pub(crate) uid: u32,
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) struct Snapshot {
    pub(crate) rows: Vec<Row>,
    pub(crate) me: Me,
    pub(crate) now: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    Pid,
    Ppid,
    Pgid,
    Uid,
    User,
    Tty,
    Tt,
    Lstart,
    Start,
    Stat,
    Comm,
    Command,
    Rss,
    Vsz,
    Etime,
    Time,
    PCpu,
    PMem,
    Nice,
    Pri,
    C,
    Sess,
    Jobc,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Align {
    Left,
    Right,
}

impl Field {
    fn from_name(name: &str) -> Option<Field> {
        Some(match name {
            "pid" => Field::Pid,
            "ppid" => Field::Ppid,
            "pgid" => Field::Pgid,
            "uid" => Field::Uid,
            "user" => Field::User,
            "tty" => Field::Tty,
            "tt" => Field::Tt,
            "lstart" => Field::Lstart,
            "start" | "stime" => Field::Start,
            "stat" | "state" => Field::Stat,
            "comm" => Field::Comm,
            "command" | "args" => Field::Command,
            "rss" => Field::Rss,
            "vsz" | "vsize" => Field::Vsz,
            "etime" => Field::Etime,
            "time" => Field::Time,
            "%cpu" | "pcpu" => Field::PCpu,
            "%mem" | "pmem" => Field::PMem,
            "nice" | "ni" => Field::Nice,
            "pri" => Field::Pri,
            "c" => Field::C,
            "sess" => Field::Sess,
            "jobc" => Field::Jobc,
            _ => return None,
        })
    }

    /// The header `/bin/ps` prints for the field.
    fn header(self) -> &'static str {
        match self {
            Field::Pid => "PID",
            Field::Ppid => "PPID",
            Field::Pgid => "PGID",
            Field::Uid => "UID",
            Field::User => "USER",
            Field::Tty => "TTY",
            Field::Tt => "TT",
            Field::Lstart | Field::Start => "STARTED",
            Field::Stat => "STAT",
            Field::Comm => "COMM",
            Field::Command => "COMMAND",
            Field::Rss => "RSS",
            Field::Vsz => "VSZ",
            Field::Etime => "ELAPSED",
            Field::Time => "TIME",
            Field::PCpu => "%CPU",
            Field::PMem => "%MEM",
            Field::Nice => "NI",
            Field::Pri => "PRI",
            Field::C => "C",
            Field::Sess => "SESS",
            Field::Jobc => "JOBC",
        }
    }

    fn align(self) -> Align {
        match self {
            Field::User
            | Field::Tty
            | Field::Lstart
            | Field::Start
            | Field::Stat
            | Field::Comm
            | Field::Command => Align::Left,
            _ => Align::Right,
        }
    }

    fn min_width(self) -> usize {
        match self {
            Field::Pid | Field::Ppid | Field::Pgid | Field::Uid => 5,
            Field::Rss | Field::Vsz | Field::Sess => 6,
            Field::User => 16,
            Field::Tty => 8,
            Field::Stat | Field::Tt | Field::Jobc | Field::PMem | Field::Pri => 4,
            Field::C => 3,
            Field::Start | Field::Etime => 7,
            Field::Time => 9,
            Field::PCpu => 5,
            Field::Nice => 2,
            Field::Lstart => 28,
            Field::Comm | Field::Command => 0,
        }
    }
}

struct Column {
    field: Field,
    header: String,
}

fn column(field: Field) -> Column {
    Column {
        field,
        header: field.header().to_string(),
    }
}

fn named_column(field: Field, header: &str) -> Column {
    Column {
        field,
        header: header.to_string(),
    }
}

/// Which processes a command line names, by the rules of macOS `ps.c`.
#[derive(Debug, Default)]
struct Selection {
    /// `-a`, `-A` or `-e`: every user, not only the caller.
    all_users: bool,
    /// `x`, `-A` or `-e`: processes without a controlling terminal too.
    with_ttyless: bool,
    pids: Vec<u32>,
    /// `-u`, matched against the effective user.
    users: Vec<String>,
    /// `-U`, matched against the real user.
    real_users: Vec<String>,
}

impl Selection {
    fn matches(&self, row: &Row, me: &Me) -> bool {
        // A listed pid shows whatever its terminal.
        if self.pids.contains(&row.pid) {
            return true;
        }
        let names_users = !self.users.is_empty() || !self.real_users.is_empty();
        if !(self.with_ttyless || names_users || row.controlled) {
            return false;
        }
        if self.all_users {
            return true;
        }
        if self.pids.is_empty() && !names_users {
            return row.uid == me.uid;
        }
        names(&self.users, row.uid, &row.user) || names(&self.real_users, row.ruid, &row.ruser)
    }
}

fn names(list: &[String], uid: u32, user: &str) -> bool {
    list.iter().any(|u| u == user || *u == uid.to_string())
}

struct Spec {
    select: Selection,
    columns: Vec<Column>,
}

impl Spec {
    fn parse(argv: &[String]) -> Result<Spec, String> {
        let mut select = Selection::default();
        let (mut full, mut jobs, mut user_format, mut user_select) = (false, false, false, false);
        let mut explicit: Vec<Column> = Vec::new();
        let mut i = 0;
        while i < argv.len() {
            let arg = &argv[i];
            // The first word may be BSD style (`aux`), with no dash.
            let (dashed, flags) = match arg.strip_prefix('-') {
                Some(flags) if !flags.is_empty() => (true, flags),
                _ if i == 0 && !arg.is_empty() => (false, arg.as_str()),
                _ => return Err(format!("unexpected argument {arg:?}")),
            };
            for (at, flag) in flags.char_indices() {
                match (dashed, flag) {
                    (true, 'A' | 'e') => {
                        select.all_users = true;
                        select.with_ttyless = true;
                    }
                    (_, 'a') => select.all_users = true,
                    (_, 'x') => select.with_ttyless = true,
                    (_, 'w') => {}
                    (true, 'f') => full = true,
                    (true, 'j') => jobs = true,
                    (false, 'u') => user_format = true,
                    (true, 'o' | 'p' | 'U' | 'u') => {
                        let rest = &flags[at + flag.len_utf8()..];
                        let value = if rest.is_empty() {
                            i += 1;
                            argv.get(i)
                                .ok_or_else(|| format!("-{flag} needs a value"))?
                                .as_str()
                        } else {
                            rest
                        };
                        let users = split_list(value).map(str::to_string);
                        match flag {
                            'o' => parse_fields(value, &mut explicit)?,
                            'p' => parse_pids(value, &mut select.pids)?,
                            'u' => {
                                user_select = true;
                                select.users.extend(users);
                            }
                            _ => select.real_users.extend(users),
                        }
                        break;
                    }
                    (true, flag) => return Err(format!("unsupported option -{flag}")),
                    (false, flag) => return Err(format!("unsupported option {flag}")),
                }
            }
            i += 1;
        }
        let columns = if !explicit.is_empty() {
            explicit
        } else if jobs {
            jobs_columns()
        } else if user_format {
            user_columns()
        } else if full {
            full_columns()
        } else {
            let mut columns = vec![column(Field::Pid), column(Field::Tty), column(Field::Time)];
            if user_select {
                columns.insert(0, column(Field::Uid));
            }
            columns.push(named_column(Field::Command, "CMD"));
            columns
        };
        Ok(Spec { select, columns })
    }
}

fn split_list(value: &str) -> impl Iterator<Item = &str> {
    value
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|part| !part.is_empty())
}

fn parse_pids(value: &str, into: &mut Vec<u32>) -> Result<(), String> {
    for part in split_list(value) {
        into.push(part.parse().map_err(|_| format!("bad pid {part:?}"))?);
    }
    Ok(())
}

fn parse_fields(value: &str, into: &mut Vec<Column>) -> Result<(), String> {
    for spec in split_list(value) {
        let (name, header) = match spec.split_once('=') {
            Some((name, header)) => (name, Some(header)),
            None => (spec, None),
        };
        let field = Field::from_name(name).ok_or_else(|| format!("unsupported field {name:?}"))?;
        into.push(Column {
            field,
            header: header.map_or_else(|| field.header().to_string(), str::to_string),
        });
    }
    Ok(())
}

fn user_columns() -> Vec<Column> {
    [
        Field::User,
        Field::Pid,
        Field::PCpu,
        Field::PMem,
        Field::Vsz,
        Field::Rss,
        Field::Tt,
        Field::Stat,
        Field::Start,
        Field::Time,
        Field::Command,
    ]
    .into_iter()
    .map(column)
    .collect()
}

fn full_columns() -> Vec<Column> {
    vec![
        column(Field::Uid),
        column(Field::Pid),
        column(Field::Ppid),
        column(Field::C),
        named_column(Field::Start, "STIME"),
        column(Field::Tty),
        column(Field::Time),
        named_column(Field::Command, "CMD"),
    ]
}

fn jobs_columns() -> Vec<Column> {
    [
        Field::User,
        Field::Pid,
        Field::Ppid,
        Field::Pgid,
        Field::Sess,
        Field::Jobc,
        Field::Stat,
        Field::Tt,
        Field::Time,
        Field::Command,
    ]
    .into_iter()
    .map(column)
    .collect()
}

/// `/bin/ps` formats times in the user's locale, and so must its stand-in.
fn strftime(format: &str, secs: i64) -> String {
    let Ok(format) = std::ffi::CString::new(format) else {
        return String::new();
    };
    let secs = secs as libc::time_t;
    let mut buf = [0u8; 128];
    // SAFETY: `tm` and `buf` outlive the calls, and strftime writes at most
    // `buf.len()` bytes.
    unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&secs, &mut tm);
        let written = libc::strftime(buf.as_mut_ptr().cast(), buf.len(), format.as_ptr(), &tm);
        String::from_utf8_lossy(&buf[..written]).into_owned()
    }
}

fn use_locale_time() {
    // SAFETY: the empty C string selects the environment's locale.
    unsafe {
        libc::setlocale(libc::LC_TIME, c"".as_ptr());
    }
}

/// `[[dd-]hh:]mm:ss`
fn clock(secs: i64) -> String {
    let secs = secs.max(0);
    let (days, hours, mins, secs) = (secs / 86400, secs / 3600 % 24, secs / 60 % 60, secs % 60);
    match (days, hours) {
        (0, 0) => format!("{mins:02}:{secs:02}"),
        (0, _) => format!("{hours:02}:{mins:02}:{secs:02}"),
        _ => format!("{days}-{hours:02}:{mins:02}:{secs:02}"),
    }
}

/// `M:SS.cc`, minutes unbounded, as `/bin/ps` prints CPU time.
fn cpu_clock(cpu_ns: u64) -> String {
    let hundredths = cpu_ns / 10_000_000;
    format!(
        "{}:{:02}.{:02}",
        hundredths / 6000,
        hundredths / 100 % 60,
        hundredths % 100
    )
}

/// `/bin/ps`'s STARTED rule: a clock time within a day, a weekday and hour
/// within a week, else a date.
fn started(secs: i64, now: i64) -> String {
    let age = now - secs;
    let format = if age < 86_400 {
        "%l:%M%p"
    } else if age < 7 * 86_400 {
        "%a%I%p"
    } else {
        "%e%b%y"
    };
    strftime(format, secs)
}

fn percent_cpu(row: &Row, now: i64) -> f64 {
    let elapsed = (now - row.started).max(1) as f64;
    row.cpu_ns as f64 / 1e9 / elapsed * 100.0
}

/// One line per process: a control character prints as `?`, as `/bin/ps` does.
fn flatten(words: &[String]) -> String {
    words
        .join(" ")
        .chars()
        .map(|c| {
            if (c as u32) < 32 || c as u32 == 127 {
                '?'
            } else {
                c
            }
        })
        .collect()
}

fn cell(field: Field, row: &Row, now: i64) -> String {
    match field {
        Field::Pid => row.pid.to_string(),
        Field::Ppid => row.ppid.to_string(),
        Field::Pgid => row.pgid.to_string(),
        Field::Uid => row.uid.to_string(),
        Field::User => row.user.clone(),
        Field::Tty => row.tty.clone(),
        Field::Tt => short_tty(&row.tty),
        Field::Lstart => strftime("%c", row.started),
        Field::Start => started(row.started, now),
        Field::Stat => row.state.clone(),
        Field::Rss => row.rss_kb.to_string(),
        Field::Vsz => row.vsz_kb.to_string(),
        Field::Etime => clock(now - row.started),
        Field::Time => cpu_clock(row.cpu_ns),
        Field::PCpu => format!("{:.1}", percent_cpu(row, now)),
        Field::PMem => format!("{:.1}", row.mem_percent),
        Field::Nice => row.nice.to_string(),
        Field::Pri => row.priority.to_string(),
        Field::C => format!("{:.0}", percent_cpu(row, now).min(99.0)),
        Field::Sess => "0".to_string(),
        Field::Jobc => row.jobc.to_string(),
        Field::Comm => match row.argv.as_deref() {
            Some([first, ..]) if !first.is_empty() => first.clone(),
            _ => row.comm.clone(),
        },
        Field::Command => match row.argv.as_deref() {
            Some(argv) if argv.first().is_some_and(|first| !first.is_empty()) => flatten(argv),
            _ => format!("({})", row.comm),
        },
    }
}

/// `ttys003` prints as `s003` and `console` as `con` under `TT`.
fn short_tty(tty: &str) -> String {
    match tty {
        "console" => "con".to_string(),
        _ => tty.strip_prefix("tty").unwrap_or(tty).to_string(),
    }
}

/// `/bin/ps` cuts COMM to the kernel's 16-character name length, except as
/// the last column.
const COMM_WIDTH: usize = 16;

fn render(spec: &Spec, rows: &[&Row], now: i64) -> String {
    let last = spec.columns.len().saturating_sub(1);
    let cells: Vec<Vec<String>> = rows
        .iter()
        .map(|row| {
            spec.columns
                .iter()
                .enumerate()
                .map(|(at, c)| match c.field {
                    Field::Comm if at != last => {
                        cell(c.field, row, now).chars().take(COMM_WIDTH).collect()
                    }
                    _ => cell(c.field, row, now),
                })
                .collect()
        })
        .collect();
    let show_header = spec.columns.iter().any(|c| !c.header.is_empty());
    let widths: Vec<usize> = spec
        .columns
        .iter()
        .enumerate()
        .map(|(at, c)| {
            let content = cells
                .iter()
                .map(|r| r[at].chars().count())
                .max()
                .unwrap_or(0);
            let header = if show_header {
                c.header.chars().count()
            } else {
                0
            };
            // `/bin/ps` lets a long value overflow a fixed column, and only
            // COMM and COMMAND take the width of their content.
            let content = match c.field {
                Field::Comm | Field::Command => content,
                _ => 0,
            };
            c.field.min_width().max(header).max(content)
        })
        .collect();
    let line = |texts: &[&str]| -> String {
        let mut out = String::new();
        for (at, text) in texts.iter().enumerate() {
            if at > 0 {
                out.push(' ');
                // `/bin/ps` leaves one more space after the TT column.
                if spec.columns[at - 1].field == Field::Tt {
                    out.push(' ');
                }
            }
            let pad = widths[at].saturating_sub(text.chars().count());
            match spec.columns[at].field.align() {
                Align::Right => {
                    out.extend(std::iter::repeat_n(' ', pad));
                    out.push_str(text);
                }
                Align::Left => {
                    out.push_str(text);
                    if at != last || !matches!(spec.columns[at].field, Field::Comm | Field::Command)
                    {
                        out.extend(std::iter::repeat_n(' ', pad));
                    }
                }
            }
        }
        out
    };
    let mut out = String::new();
    if show_header {
        let headers: Vec<&str> = spec.columns.iter().map(|c| c.header.as_str()).collect();
        let _ = writeln!(out, "{}", line(&headers));
    }
    for row in &cells {
        let texts: Vec<&str> = row.iter().map(String::as_str).collect();
        let _ = writeln!(out, "{}", line(&texts));
    }
    out
}

#[cfg(target_os = "macos")]
mod kernel {
    use std::collections::HashMap;
    use std::ffi::{c_char, c_int, c_void};
    use std::mem::size_of;

    use super::{Me, Row, Snapshot, NO_TTY};

    /// Process states from `<sys/proc.h>`.
    const SIDL: u8 = 1;
    const SRUN: u8 = 2;
    const SSLEEP: u8 = 3;
    const SSTOP: u8 = 4;
    const SZOMB: u8 = 5;
    /// `p_flag` and `e_flag` bits from `<sys/proc.h>` and `<sys/sysctl.h>`.
    const P_CONTROLT: i32 = 0x2;
    const EPROC_SLEADER: i32 = 0x2;
    const NODEV: i32 = -1;

    /// `sizeof(struct kinfo_proc)` on LP64, and the offsets of the fields
    /// read below. libc does not define the struct, and `sysctl` hands back
    /// raw bytes, so each field is read at its offset. The live tests compare
    /// every one against `/bin/ps`.
    const KINFO_SIZE: usize = 648;
    const OFF_STARTED_SEC: usize = 0;
    const OFF_FLAG: usize = 32;
    const OFF_STAT: usize = 36;
    const OFF_PID: usize = 40;
    const OFF_NICE: usize = 242;
    const OFF_COMM: usize = 243;
    const COMM_LEN: usize = 16;
    const EPROC: usize = 296;
    const OFF_RUID: usize = EPROC + 96;
    const OFF_UID: usize = EPROC + 124;
    const OFF_PPID: usize = EPROC + 264;
    const OFF_PGID: usize = EPROC + 268;
    const OFF_JOBC: usize = EPROC + 272;
    const OFF_TDEV: usize = EPROC + 276;
    const OFF_TPGID: usize = EPROC + 280;
    const OFF_EFLAG: usize = EPROC + 316;

    extern "C" {
        fn devname_r(
            dev: libc::dev_t,
            mode: libc::mode_t,
            buf: *mut c_char,
            len: c_int,
        ) -> *mut c_char;
        fn mach_timebase_info(info: *mut [u32; 2]) -> c_int;
    }

    /// One process from `sysctl kern.proc.all`, which names every process to
    /// any user, root's included.
    struct Kinfo {
        pid: i32,
        ppid: i32,
        pgid: i32,
        uid: u32,
        ruid: u32,
        tdev: i32,
        tpgid: i32,
        nice: i8,
        started: i64,
        jobc: i16,
        status: u8,
        flag: i32,
        eflag: i32,
        comm: String,
    }

    impl Kinfo {
        fn parse(raw: &[u8]) -> Kinfo {
            let word = |at: usize| i32::from_ne_bytes(raw[at..at + 4].try_into().unwrap());
            let nul = raw[OFF_COMM..OFF_COMM + COMM_LEN]
                .iter()
                .position(|b| *b == 0)
                .unwrap_or(COMM_LEN);
            Kinfo {
                pid: word(OFF_PID),
                ppid: word(OFF_PPID),
                pgid: word(OFF_PGID),
                uid: word(OFF_UID) as u32,
                ruid: word(OFF_RUID) as u32,
                tdev: word(OFF_TDEV),
                tpgid: word(OFF_TPGID),
                nice: raw[OFF_NICE] as i8,
                started: i64::from_ne_bytes(
                    raw[OFF_STARTED_SEC..OFF_STARTED_SEC + 8]
                        .try_into()
                        .unwrap(),
                ),
                jobc: i16::from_ne_bytes(raw[OFF_JOBC..OFF_JOBC + 2].try_into().unwrap()),
                status: raw[OFF_STAT],
                flag: word(OFF_FLAG),
                eflag: word(OFF_EFLAG),
                comm: String::from_utf8_lossy(&raw[OFF_COMM..OFF_COMM + nul]).into_owned(),
            }
        }
    }

    fn all_procs() -> Result<Vec<Kinfo>, String> {
        let mut mib = [libc::CTL_KERN, libc::KERN_PROC, libc::KERN_PROC_ALL];
        // A process can start between the size query and the read, so leave
        // room and retry when the kernel still says the buffer is short.
        for _ in 0..4 {
            let mut size: usize = 0;
            // SAFETY: a null buffer asks for the size.
            let status = unsafe {
                libc::sysctl(
                    mib.as_mut_ptr(),
                    3,
                    std::ptr::null_mut(),
                    &mut size,
                    std::ptr::null_mut(),
                    0,
                )
            };
            if status != 0 {
                return Err("the kernel would not list processes".to_string());
            }
            let mut buf = vec![0u8; size + 16 * KINFO_SIZE];
            let mut size = buf.len();
            // SAFETY: `buf` holds `size` bytes.
            let status = unsafe {
                libc::sysctl(
                    mib.as_mut_ptr(),
                    3,
                    buf.as_mut_ptr().cast::<c_void>(),
                    &mut size,
                    std::ptr::null_mut(),
                    0,
                )
            };
            if status == 0 {
                return Ok(buf[..size]
                    .chunks_exact(KINFO_SIZE)
                    .map(Kinfo::parse)
                    .collect());
            }
        }
        Err("the kernel would not list processes".to_string())
    }

    fn task_info(pid: i32) -> Option<libc::proc_taskinfo> {
        // SAFETY: an all-zero proc_taskinfo is valid, and proc_pidinfo writes
        // at most the size passed.
        let mut info: libc::proc_taskinfo = unsafe { std::mem::zeroed() };
        let size = size_of::<libc::proc_taskinfo>() as c_int;
        let got = unsafe {
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDTASKINFO,
                0,
                (&mut info as *mut libc::proc_taskinfo).cast::<c_void>(),
                size,
            )
        };
        (got == size).then_some(info)
    }

    fn sysctl_u64(name: &std::ffi::CStr) -> Option<u64> {
        let mut value: u64 = 0;
        let mut size = size_of::<u64>();
        // SAFETY: `value` is 8 bytes and `size` says so.
        let status = unsafe {
            libc::sysctlbyname(
                name.as_ptr(),
                (&mut value as *mut u64).cast::<c_void>(),
                &mut size,
                std::ptr::null_mut(),
                0,
            )
        };
        (status == 0).then_some(value)
    }

    fn arg_max() -> usize {
        let mut max: c_int = 0;
        let mut size = size_of::<c_int>();
        let mut mib = [libc::CTL_KERN, libc::KERN_ARGMAX];
        // SAFETY: `max` is a c_int and `size` says so.
        let status = unsafe {
            libc::sysctl(
                mib.as_mut_ptr(),
                2,
                (&mut max as *mut c_int).cast::<c_void>(),
                &mut size,
                std::ptr::null_mut(),
                0,
            )
        };
        if status == 0 && max > 0 {
            max as usize
        } else {
            0
        }
    }

    /// argv from `KERN_PROCARGS2`, or `None` when the kernel withholds it.
    fn proc_args(pid: i32, max: usize) -> Option<Vec<String>> {
        if max == 0 {
            return None;
        }
        let mut buf = vec![0u8; max];
        let mut size = max;
        let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
        // SAFETY: `buf` holds `size` bytes.
        let status = unsafe {
            libc::sysctl(
                mib.as_mut_ptr(),
                3,
                buf.as_mut_ptr().cast::<c_void>(),
                &mut size,
                std::ptr::null_mut(),
                0,
            )
        };
        if status != 0 || size < 4 {
            return None;
        }
        let raw = &buf[..size];
        let argc = usize::try_from(i32::from_ne_bytes(raw[..4].try_into().ok()?)).ok()?;
        let after_argc = &raw[4..];
        let exec_end = after_argc.iter().position(|b| *b == 0)?;
        let rest = &after_argc[exec_end..];
        let skip = rest.iter().position(|b| *b != 0).unwrap_or(rest.len());
        let strings: Vec<&[u8]> = rest[skip..].split(|b| *b == 0).collect();
        if strings.len() < argc {
            return None;
        }
        Some(
            strings[..argc]
                .iter()
                .map(|b| String::from_utf8_lossy(b).into_owned())
                .collect(),
        )
    }

    fn user_name(uid: u32, cache: &mut HashMap<u32, String>) -> String {
        cache
            .entry(uid)
            .or_insert_with(|| {
                // SAFETY: the buffers outlive the call, and the name is read
                // only when the call reports success and sets `found`.
                unsafe {
                    let mut pwd: libc::passwd = std::mem::zeroed();
                    let mut buf = vec![0 as c_char; 1024];
                    let mut found: *mut libc::passwd = std::ptr::null_mut();
                    let status =
                        libc::getpwuid_r(uid, &mut pwd, buf.as_mut_ptr(), buf.len(), &mut found);
                    if status == 0 && !found.is_null() {
                        std::ffi::CStr::from_ptr(pwd.pw_name)
                            .to_string_lossy()
                            .into_owned()
                    } else {
                        uid.to_string()
                    }
                }
            })
            .clone()
    }

    /// `/dev/console` is device 0, so only `NODEV` means no terminal.
    fn tty_name(dev: i32) -> String {
        if dev == NODEV {
            return NO_TTY.to_string();
        }
        let mut buf = [0 as c_char; 64];
        // SAFETY: `buf` holds the length passed.
        let name = unsafe { devname_r(dev as libc::dev_t, libc::S_IFCHR, buf.as_mut_ptr(), 64) };
        if name.is_null() {
            return NO_TTY.to_string();
        }
        let name = unsafe { std::ffi::CStr::from_ptr(buf.as_ptr()) }.to_string_lossy();
        // devname_r answers `#C<major>:<minor>` when the node is unknown.
        if name.is_empty() || name.starts_with('#') {
            NO_TTY.to_string()
        } else {
            name.into_owned()
        }
    }

    /// The first stat letter, then `<` or `N` for nice, `s` for a session
    /// leader and `+` for the foreground process group. The first letter
    /// needs the task's thread count, which the kernel withholds for another
    /// user's process, so it prints as `?` there.
    fn state(info: &Kinfo, task: Option<&libc::proc_taskinfo>) -> String {
        let letter = match info.status {
            SIDL => 'I',
            SSTOP => 'T',
            SZOMB => 'Z',
            SRUN | SSLEEP => match task {
                Some(task) if task.pti_numrunning > 0 => 'R',
                Some(_) => 'S',
                None => '?',
            },
            _ => '?',
        };
        let mut out = letter.to_string();
        match info.nice {
            n if n < 0 => out.push('<'),
            n if n > 0 => out.push('N'),
            _ => {}
        }
        if info.eflag & EPROC_SLEADER != 0 {
            out.push('s');
        }
        if info.flag & P_CONTROLT != 0 && info.pgid == info.tpgid {
            out.push('+');
        }
        out
    }

    /// The mach time ratio, as (numerator, denominator) of nanoseconds per
    /// tick. Apple Silicon ticks are not nanoseconds.
    fn nanos_per_tick() -> (u64, u64) {
        let mut base = [0u32; 2];
        // SAFETY: `base` is the two-word `mach_timebase_info` out parameter.
        unsafe { mach_timebase_info(&mut base) };
        if base[1] == 0 {
            (1, 1)
        } else {
            (u64::from(base[0]), u64::from(base[1]))
        }
    }

    pub(super) fn snapshot() -> Result<Snapshot, String> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64);
        let memory = sysctl_u64(c"hw.memsize").unwrap_or(0) as f64;
        let max = arg_max();
        let (numer, denom) = nanos_per_tick();
        let mut procs = all_procs()?;
        procs.retain(|info| info.pid > 0);
        procs.sort_unstable_by_key(|info| info.pid);
        let mut users = HashMap::new();
        let mut rows = Vec::new();
        for info in &procs {
            let task = task_info(info.pid);
            let rss_kb = task.as_ref().map_or(0, |t| t.pti_resident_size / 1024);
            let ticks = task
                .as_ref()
                .map_or(0, |t| t.pti_total_user + t.pti_total_system);
            rows.push(Row {
                pid: info.pid as u32,
                ppid: info.ppid as u32,
                pgid: info.pgid as u32,
                uid: info.uid,
                user: user_name(info.uid, &mut users),
                ruid: info.ruid,
                ruser: user_name(info.ruid, &mut users),
                tty: tty_name(info.tdev),
                controlled: info.tdev != NODEV && info.flag & P_CONTROLT != 0,
                started: info.started,
                rss_kb,
                vsz_kb: task.as_ref().map_or(0, |t| t.pti_virtual_size / 1024),
                cpu_ns: ticks.saturating_mul(numer) / denom,
                mem_percent: if memory > 0.0 {
                    rss_kb as f64 * 1024.0 / memory * 100.0
                } else {
                    0.0
                },
                nice: i32::from(info.nice),
                priority: task.as_ref().map_or(0, |t| t.pti_priority),
                jobc: info.jobc.max(0) as u32,
                state: state(info, task.as_ref()),
                comm: info.comm.clone(),
                argv: proc_args(info.pid, max),
            });
        }
        // SAFETY: geteuid has no preconditions and cannot fail.
        let me = Me {
            uid: unsafe { libc::geteuid() },
        };
        Ok(Snapshot { rows, me, now })
    }
}

#[cfg(not(target_os = "macos"))]
mod kernel {
    use super::Snapshot;

    pub(super) fn snapshot() -> Result<Snapshot, String> {
        Err(
            "this stand-in reads macOS kernel interfaces, and other systems ship a working ps"
                .to_string(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(words: &[&str]) -> Vec<String> {
        words.iter().map(|w| w.to_string()).collect()
    }

    fn row(pid: u32, uid: u32, tty: &str) -> Row {
        Row {
            pid,
            uid,
            user: format!("user{uid}"),
            ruid: uid,
            ruser: format!("user{uid}"),
            tty: tty.to_string(),
            controlled: tty != NO_TTY,
            comm: "tool".to_string(),
            argv: Some(args(&["/bin/tool", "--flag"])),
            started: 1_000,
            ..Row::default()
        }
    }

    fn me() -> Me {
        Me { uid: 501 }
    }

    fn headers(spec: &Spec) -> Vec<String> {
        spec.columns.iter().map(|c| c.header.clone()).collect()
    }

    #[test]
    fn aux_ax_and_ef_pick_their_usual_column_sets() {
        let aux = Spec::parse(&args(&["aux"])).unwrap();
        assert_eq!(
            headers(&aux),
            [
                "USER", "PID", "%CPU", "%MEM", "VSZ", "RSS", "TT", "STAT", "STARTED", "TIME",
                "COMMAND"
            ]
        );
        let ef = Spec::parse(&args(&["-ef"])).unwrap();
        assert_eq!(
            headers(&ef),
            ["UID", "PID", "PPID", "C", "STIME", "TTY", "TIME", "CMD"]
        );
        assert!(ef.select.all_users && ef.select.with_ttyless);
        let default = Spec::parse(&[]).unwrap();
        assert_eq!(headers(&default), ["PID", "TTY", "TIME", "CMD"]);
    }

    #[test]
    fn o_takes_a_value_glued_or_separate_and_either_separator() {
        for words in [
            args(&["-o", "pid,command", "-p", "1"]),
            args(&["-opid command", "-p1"]),
            args(&["-p", "1", "-o", "pid", "-ocommand"]),
        ] {
            let spec = Spec::parse(&words).unwrap();
            assert_eq!(spec.columns.len(), 2, "{words:?}");
            assert_eq!(spec.select.pids, [1]);
        }
    }

    #[test]
    fn an_empty_header_drops_the_header_line() {
        let spec = Spec::parse(&args(&["-o", "pid=,command=", "-p", "7"])).unwrap();
        let r = row(7, 501, "??");
        assert_eq!(render(&spec, &[&r], 2_000), "    7 /bin/tool --flag\n");
    }

    #[test]
    fn a_renamed_header_is_kept() {
        let spec = Spec::parse(&args(&["-o", "pid=ID,command=", "-p", "7"])).unwrap();
        let r = row(7, 501, "??");
        assert_eq!(
            render(&spec, &[&r], 2_000),
            "   ID \n    7 /bin/tool --flag\n"
        );
    }

    #[test]
    fn unsupported_options_and_fields_are_refused() {
        for words in [
            args(&["-l"]),
            args(&["-M"]),
            args(&["aeux"]),
            args(&["-o", "wchan"]),
            args(&["-p", "x1"]),
            args(&["-o"]),
            args(&["-p", "1", "stray"]),
            args(&["-E", "-p", "1"]),
        ] {
            assert!(Spec::parse(&words).is_err(), "{words:?}");
        }
    }

    #[test]
    fn the_dashed_e_selects_every_process_and_the_bare_e_is_refused() {
        let every = Spec::parse(&args(&["-e"])).unwrap().select;
        assert!(every.all_users && every.with_ttyless);
        assert!(Spec::parse(&args(&["e"])).is_err());
    }

    /// Each expectation was checked against macOS `/bin/ps`.
    #[test]
    fn selection_follows_macos_ps() {
        let mine = row(10, 501, "ttys001");
        let mine_elsewhere = row(11, 501, "ttys002");
        let other_user = row(12, 0, "ttys001");
        let daemon = row(13, 0, "??");
        let my_daemon = row(14, 501, "??");
        // Started by 501, running as root, as a setuid helper does.
        let mut setuid = row(15, 0, "??");
        setuid.ruid = 501;
        setuid.ruser = "user501".to_string();
        let pick = |words: &[&str]| -> Vec<u32> {
            let spec = Spec::parse(&args(words)).unwrap();
            [
                &mine,
                &mine_elsewhere,
                &other_user,
                &daemon,
                &my_daemon,
                &setuid,
            ]
            .into_iter()
            .filter(|r| spec.select.matches(r, &me()))
            .map(|r| r.pid)
            .collect()
        };
        assert_eq!(pick(&[]), [10, 11], "own processes on any terminal");
        assert_eq!(pick(&["-A"]), [10, 11, 12, 13, 14, 15]);
        assert_eq!(pick(&["ax"]), [10, 11, 12, 13, 14, 15]);
        assert_eq!(pick(&["aux"]), [10, 11, 12, 13, 14, 15]);
        assert_eq!(pick(&["a"]), [10, 11, 12]);
        assert_eq!(pick(&["x"]), [10, 11, 14]);
        assert_eq!(pick(&["-p", "13"]), [13]);
        assert_eq!(pick(&["-x", "-p", "99"]), Vec::<u32>::new());
        assert_eq!(pick(&["-a", "-p", "13"]), [10, 11, 12, 13]);
        assert_eq!(pick(&["-a", "-u", "0"]), [10, 11, 12, 13, 14, 15]);
        assert_eq!(pick(&["-u", "0"]), [12, 13, 15], "effective user");
        assert_eq!(pick(&["-U", "0"]), [12, 13], "real user");
        assert_eq!(pick(&["-p", "13", "-U", "user501"]), [10, 11, 13, 14, 15]);
    }

    #[test]
    fn unreadable_argv_prints_the_command_name_in_parentheses() {
        let mut r = row(1, 0, "??");
        r.argv = None;
        assert_eq!(cell(Field::Command, &r, 2_000), "(tool)");
        assert_eq!(cell(Field::Comm, &r, 2_000), "tool");
        r.argv = Some(Vec::new());
        assert_eq!(cell(Field::Command, &r, 2_000), "(tool)");
    }

    #[test]
    fn a_control_character_in_argv_prints_as_a_question_mark() {
        let mut r = row(1, 0, "??");
        r.argv = Some(args(&["sh", "-c", "a\nb\u{7f}"]));
        assert_eq!(cell(Field::Command, &r, 2_000), "sh -c a?b?");
    }

    #[test]
    fn numbers_pad_to_the_widths_bin_ps_uses() {
        let spec = Spec::parse(&args(&["-o", "pid,ppid,pgid,uid,rss,comm", "-p", "9"])).unwrap();
        let mut r = row(9, 501, "??");
        r.ppid = 1;
        r.pgid = 9;
        r.rss_kb = 3232;
        assert_eq!(
            render(&spec, &[&r], 2_000),
            "  PID  PPID  PGID   UID    RSS COMM\n    9     1     9   501   3232 /bin/tool\n"
        );
    }

    #[test]
    fn a_wide_pid_overflows_its_column_as_in_bin_ps() {
        let spec = Spec::parse(&args(&["-o", "pid,comm", "-A"])).unwrap();
        let (a, b) = (row(9, 0, "??"), row(1234567, 0, "??"));
        assert_eq!(
            render(&spec, &[&a, &b], 2_000),
            "  PID COMM\n    9 /bin/tool\n1234567 /bin/tool\n"
        );
    }

    #[test]
    fn clocks_format_like_bin_ps() {
        assert_eq!(clock(5), "00:05");
        assert_eq!(clock(125), "02:05");
        assert_eq!(clock(3 * 3600 + 61), "03:01:01");
        assert_eq!(clock(2 * 86_400 + 3600), "2-01:00:00");
        assert_eq!(cpu_clock(0), "0:00.00");
        assert_eq!(cpu_clock(1_230_000_000), "0:01.23");
        assert_eq!(cpu_clock(72_245_000_000_000), "1204:05.00");
    }

    #[test]
    fn percent_cpu_is_the_lifetime_average() {
        let mut r = row(1, 0, "??");
        r.started = 1_000;
        r.cpu_ns = 5_000_000_000;
        assert_eq!(cell(Field::PCpu, &r, 1_010), "50.0");
        assert_eq!(cell(Field::C, &r, 1_010), "50");
    }

    #[test]
    fn the_short_terminal_name_drops_the_tty_prefix() {
        assert_eq!(short_tty("ttys003"), "s003");
        assert_eq!(short_tty("console"), "con");
        assert_eq!(short_tty("??"), "??");
    }

    #[test]
    fn an_empty_selection_prints_a_header_only() {
        let spec = Spec::parse(&args(&["-o", "pid", "-p", "5"])).unwrap();
        assert_eq!(render(&spec, &[], 2_000), "  PID\n");
    }
}

/// Compares the stand-in with the real `/bin/ps` on live processes. Each test
/// skips itself where `/bin/ps` cannot run, which is inside the very sandbox
/// this stand-in exists for.
#[cfg(all(test, target_os = "macos"))]
mod live_tests {
    use super::*;

    fn bin_ps(args: &[&str]) -> Option<String> {
        let out = std::process::Command::new("/bin/ps")
            .args(args)
            .output()
            .ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
    }

    fn stand_in(words: &[&str]) -> String {
        let words: Vec<String> = words.iter().map(|w| w.to_string()).collect();
        let spec = Spec::parse(&words).unwrap();
        let snapshot = kernel::snapshot().unwrap();
        let rows: Vec<&Row> = snapshot
            .rows
            .iter()
            .filter(|row| spec.select.matches(row, &snapshot.me))
            .collect();
        render(&spec, &rows, snapshot.now)
    }

    fn live_pids() -> [String; 2] {
        [
            std::process::id().to_string(),
            std::os::unix::process::parent_id().to_string(),
        ]
    }

    #[test]
    fn the_scripts_fields_match_bin_ps_byte_for_byte() {
        let fields = "pid,ppid,pgid,uid,lstart,comm,command";
        for pid in live_pids() {
            let Some(real) = bin_ps(&["-o", fields, "-p", &pid]) else {
                return;
            };
            assert_eq!(stand_in(&["-o", fields, "-p", &pid]), real, "pid {pid}");
        }
    }

    #[test]
    fn the_stat_modifiers_match_bin_ps() {
        // The first letter is a live thread count that can change between the
        // two reads, so compare what follows it.
        for pid in live_pids() {
            let Some(real) = bin_ps(&["-o", "stat=", "-p", &pid]) else {
                return;
            };
            let ours = stand_in(&["-o", "stat=", "-p", &pid]);
            assert_eq!(ours.trim()[1..], real.trim()[1..], "pid {pid}");
        }
    }

    #[test]
    fn each_selection_picks_the_processes_bin_ps_picks() {
        let me = live_pids()[0].clone();
        // SAFETY: geteuid has no preconditions and cannot fail.
        let user = unsafe { libc::geteuid() }.to_string();
        for selection in [
            &[][..],
            &["-x"],
            &["-a"],
            &["-a", "-p", &me],
            &["-u", &user],
            &["-U", &user],
        ] {
            let words: Vec<&str> = selection.iter().copied().chain(["-o", "pid="]).collect();
            // No answer means a sandbox, or nothing selected, which exits 1.
            let Some(real) = bin_ps(&words) else {
                continue;
            };
            let ours = stand_in(&words);
            let ours: std::collections::HashSet<&str> = ours.lines().collect();
            let real: std::collections::HashSet<&str> = real.lines().collect();
            // A process can start or end between the two reads.
            let common = real.intersection(&ours).count();
            assert!(
                common * 10 >= real.len().max(ours.len()) * 9,
                "{selection:?}: {common} shared, {} from /bin/ps, {} ours",
                real.len(),
                ours.len()
            );
        }
    }

    #[test]
    fn every_visible_process_matches_bin_ps_on_the_kernel_facts() {
        let fields = "pid=,ppid=,pgid=,uid=,tty=,nice=,lstart=";
        let Some(real) = bin_ps(&["-A", "-o", fields]) else {
            return;
        };
        let ours = stand_in(&["-A", "-o", fields]);
        // A process can start or end between the two reads, so compare the
        // lines both runs saw.
        let ours: std::collections::HashSet<&str> = ours.lines().collect();
        let common = real.lines().filter(|line| ours.contains(line)).count();
        assert!(
            common * 10 >= real.lines().count() * 9,
            "only {common} of {} lines agree",
            real.lines().count()
        );
    }
}
