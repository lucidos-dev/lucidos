use std::io::{self, BufRead, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use uuid::Uuid;

use lucidos_build_slot::{
    host_ncpu, inherited_slot, resolve_class_with_legacy, BuildLimits, BuildSlotGuard,
    BuildSlotPool, SlotClass, SlotState, ENV_CARGO_JOBS, ENV_CLASS, ENV_HELD, ENV_LEASE_NICE,
    ENV_LEGACY_PRIORITY, ENV_NICE,
};

use crate::workspace::{resolve_from_env, BoxError, Workspace};

const WAIT_TIMEOUT_EXIT: u8 = 75;

const PROGRESS_EVERY: Duration = Duration::from_secs(15);

const ANNOUNCE_TIMEOUT: Duration = Duration::from_secs(5);

const LABEL_CHARS: usize = 120;

const ENV_REQUEST_ID: &str = "LUCIDOS_BUILD_SLOT_REQUEST_ID";
const ENV_LEASE_RECORDS: &str = "LUCIDOS_BUILD_SLOT_LEASE_RECORDS";
const ENV_GRANTED_CLASS: &str = "LUCIDOS_BUILD_SLOT_GRANTED_CLASS";

pub(crate) struct BuildSlotArgs {
    pub status: bool,
    pub lease: bool,
    pub set_capacity: Option<usize>,
    pub max_wait_secs: Option<u64>,
    pub label: Option<String>,
    pub command: Vec<String>,
}

pub(crate) fn run(args: BuildSlotArgs) -> Result<u8, BoxError> {
    if let Some(value) = args.set_capacity {
        return cmd_set_capacity(value);
    }
    if args.status {
        return cmd_status();
    }
    if args.lease {
        return cmd_lease(args);
    }
    cmd_wrap(args)
}

fn cmd_set_capacity(value: usize) -> Result<u8, BoxError> {
    let pool = BuildSlotPool::open()?;
    pool.set_capacity(value)?;
    println!(
        "build slots: capacity set to {} for this host ({})",
        value,
        pool.dir().display()
    );
    println!("Every build on this machine reads it, so set it once, not per workspace.");
    Ok(0)
}

fn cmd_status() -> Result<u8, BoxError> {
    let pool = BuildSlotPool::open()?;
    let capacity = pool.capacity();
    let states = pool.status();
    let held = BuildSlotPool::count_held(&states);

    println!(
        "build slots: {}/{} held, capacity from {}",
        held,
        capacity.value,
        capacity.source.as_str()
    );
    println!("pool: {}", pool.dir().display());
    if pool.anyone_waiting() {
        println!("at least one build is waiting for a slot");
    }
    if pool.priority_waiting() {
        println!("a priority build is waiting: the next free slot goes to it");
    }
    for state in &states {
        println!("{}", describe_slot(state));
    }
    Ok(0)
}

fn describe_slot(state: &SlotState) -> String {
    match &state.holder {
        None => format!("  slot {}  free", state.index),
        Some(h) => {
            let pid = h.pid.map(|p| p.to_string()).unwrap_or_else(|| "?".into());
            let age = h.held_secs().map(fmt_secs).unwrap_or_else(|| "?".into());
            let label = if h.label.is_empty() { "?" } else { &h.label };
            format!(
                "  slot {}  HELD  {}  pid {}  {}  {}",
                state.index, label, pid, age, h.cwd
            )
        }
    }
}

fn fmt_secs(total: u64) -> String {
    if total < 60 {
        return format!("{total}s");
    }
    format!("{}m{:02}s", total / 60, total % 60)
}

fn cmd_wrap(args: BuildSlotArgs) -> Result<u8, BoxError> {
    if args.command.is_empty() {
        return Err("nothing to run. Usage: lucidos build-slot -- <command> [args...]".into());
    }
    let label = default_label(args.label.as_deref(), &args.command);

    if let Some(index) = inherited_slot() {
        let limits = BuildLimits::nested(std::env::var(ENV_LEASE_NICE).ok().as_deref());
        lower_own_priority(limits.nice);
        let request_id = std::env::var(ENV_REQUEST_ID)
            .ok()
            .and_then(|id| Uuid::parse_str(&id).ok());
        let class = granted_class();
        let started = Instant::now();
        let code = spawn_child(&args.command, Some(&index), limits, request_id, class);
        let exit_code = code.as_ref().ok().copied();
        announce(
            "BuildSlotStepCompleted",
            &format!("A step under a build slot completed: `{label}`"),
            serde_json::json!({
                "label": label,
                "slot": index,
                "request_id": request_id.map(|id| id.to_string()),
                "class": class.map(SlotClass::as_str),
                "duration_ms": started.elapsed().as_millis(),
                "exit_code": exit_code,
                "outcome": command_outcome(exit_code),
            }),
        );
        return code;
    }

    let Some(pool) = open_pool() else {
        return spawn_child(&args.command, None, BuildLimits::NONE, None, None);
    };
    let Some((hold, limits)) = acquire(&pool, &label, args.max_wait_secs, None) else {
        return Ok(WAIT_TIMEOUT_EXIT);
    };
    let index = hold.guard.index().to_string();
    lower_own_priority(limits.nice);
    let code = spawn_child(
        &args.command,
        Some(&index),
        limits,
        Some(hold.request_id),
        Some(hold.class),
    );
    hold.release(&label, code.as_ref().ok().copied());
    code
}

fn cmd_lease(args: BuildSlotArgs) -> Result<u8, BoxError> {
    let parent = parent_pid();
    if let Ok(owner) = std::env::var("LUCIDOS_BUILD_SLOT_LEASE_OWNER") {
        if owner.parse::<u32>().ok() != Some(parent) {
            return Ok(0);
        }
    }
    if inherited_slot().is_some() {
        println!("unleased");
        return Ok(0);
    }
    let Some(pool) = open_pool() else {
        println!("unleased");
        return Ok(0);
    };
    let label = default_label(Some(args.label.as_deref().unwrap_or("build lease")), &[]);
    let Some((hold, limits)) = acquire(&pool, &label, args.max_wait_secs, Some(parent)) else {
        println!("error");
        return Ok(WAIT_TIMEOUT_EXIT);
    };
    let result = (|| -> io::Result<Option<u8>> {
        let jobs = limits
            .jobs
            .map_or_else(|| "-".into(), |jobs| jobs.to_string());
        {
            let mut stdout = io::stdout().lock();
            let index = hold.guard.index();
            let nice = limits.nice;
            if std::env::var_os(ENV_LEASE_RECORDS).is_some() {
                writeln!(
                    stdout,
                    "granted {index} {jobs} {nice} {} {}",
                    hold.request_id,
                    hold.class.as_str()
                )?;
            } else {
                writeln!(stdout, "granted {index} {jobs} {nice}")?;
            }
            stdout.flush()?;
        }
        wait_for_lease_release(parent)
    })();
    hold.release(&label, result.as_ref().ok().copied().flatten());
    result?;
    Ok(0)
}

#[cfg(unix)]
fn parent_pid() -> u32 {
    // SAFETY: getppid has no arguments and reads the calling process's state.
    unsafe { libc::getppid() as u32 }
}

#[cfg(not(unix))]
fn parent_pid() -> u32 {
    0
}

fn wait_for_lease_release(parent: u32) -> io::Result<Option<u8>> {
    let (done, release) = std::sync::mpsc::channel();
    std::thread::Builder::new().spawn(move || {
        let mut line = Vec::new();
        let result = io::stdin().lock().read_until(b'\n', &mut line).map(|_| {
            std::str::from_utf8(&line)
                .ok()?
                .trim_end()
                .strip_prefix("release ")?
                .parse::<u8>()
                .ok()
        });
        let _ = done.send(result);
    })?;
    while parent_pid() == parent {
        match release.recv_timeout(Duration::from_secs(1)) {
            Ok(result) => return result,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    Ok(None)
}

fn open_pool() -> Option<BuildSlotPool> {
    match BuildSlotPool::open() {
        Ok(pool) => Some(pool),
        Err(e) => {
            eprintln!("lucidos build-slot: no pool ({e}), running the build unrestricted");
            None
        }
    }
}

fn acquire(
    pool: &BuildSlotPool,
    label: &str,
    max_wait_secs: Option<u64>,
    lease_parent: Option<u32>,
) -> Option<(SlotHold, BuildLimits)> {
    let request_id = Uuid::new_v4();
    let requested_at = Instant::now();
    let class = resolve_class_with_legacy(
        std::env::var(ENV_CLASS).ok().as_deref(),
        std::env::var(ENV_LEGACY_PRIORITY).ok().as_deref(),
    );
    let (guard, acquired_at) = pool
        .try_acquire_as(label, class)
        .map(|guard| (guard, Instant::now()))
        .or_else(|| wait_for_slot(pool, label, class, max_wait_secs, lease_parent, request_id))?;
    let hold = SlotHold {
        guard,
        request_id,
        class,
        wait: acquired_at.duration_since(requested_at),
        acquired_at,
    };
    let index = hold.guard.index().to_string();
    let caller_jobs = std::env::var(ENV_CARGO_JOBS).ok();
    let nice_raw = std::env::var(ENV_NICE).ok();
    let limits = pool.granted_limits(
        host_ncpu(),
        caller_jobs.as_deref(),
        nice_raw.as_deref(),
        class,
    );
    eprintln!(
        "{}",
        describe_limits(&index, pool.capacity().value, &limits)
    );
    Some((hold, limits))
}

struct SlotHold {
    guard: BuildSlotGuard,
    request_id: Uuid,
    class: SlotClass,
    wait: Duration,
    acquired_at: Instant,
}

impl SlotHold {
    fn release(self, label: &str, exit_code: Option<u8>) {
        let index = self.guard.index().to_string();
        let hold_ms = self.acquired_at.elapsed().as_millis();
        drop(self.guard);
        announce(
            "BuildSlotReleased",
            &format!("A build slot freed up after `{label}`"),
            serde_json::json!({
                "label": label,
                "slot": index,
                "request_id": self.request_id.to_string(),
                "class": self.class.as_str(),
                "wait_ms": self.wait.as_millis(),
                "hold_ms": hold_ms,
                "exit_code": exit_code,
                "outcome": command_outcome(exit_code),
            }),
        );
    }
}

fn granted_class() -> Option<SlotClass> {
    SlotClass::parse(&std::env::var(ENV_GRANTED_CLASS).ok()?)
}

fn command_outcome(exit_code: Option<u8>) -> &'static str {
    match exit_code {
        Some(0) => "succeeded",
        Some(_) => "failed",
        None => "unknown",
    }
}

fn default_label(label: Option<&str>, command: &[String]) -> String {
    let label = label.unwrap_or_else(|| {
        let first = command.first().map(String::as_str).unwrap_or_default();
        let program = first
            .split(|c: char| c.is_whitespace() || c == '=')
            .next()
            .unwrap_or_default();
        Path::new(program)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(program)
    });
    label.chars().take(LABEL_CHARS).collect()
}

fn wait_for_slot(
    pool: &BuildSlotPool,
    label: &str,
    class: SlotClass,
    max_wait_secs: Option<u64>,
    lease_parent: Option<u32>,
    request_id: Uuid,
) -> Option<(BuildSlotGuard, Instant)> {
    let capacity = pool.capacity().value;
    let turn = match class {
        SlotClass::Priority => "as a priority build, so the next free one is ours",
        SlotClass::Ordinary => "for one",
        SlotClass::Background => "behind chosen work",
    };
    eprintln!(
        "lucidos build-slot: all {capacity} build slots are busy, waiting {turn}. \
         `lucidos build-slot --status` shows who holds them."
    );

    let mut next_progress = PROGRESS_EVERY;
    let mut announced = false;
    #[cfg(unix)]
    let mut background_wait = None;
    let max_wait = max_wait_secs.map(Duration::from_secs);
    let guard = pool.acquire(label, max_wait, class, |waited| {
        if lease_parent.is_some_and(|parent| parent_pid() != parent) {
            std::process::exit(0);
        }
        if !announced {
            announced = true;
            #[cfg(unix)]
            {
                background_wait = lucidos_build_slot::notify_background_wait();
            }
            announce(
                "BuildSlotWaiting",
                &format!("A build is waiting for one of {capacity} build slots: `{label}`"),
                serde_json::json!({
                    "label": label,
                    "slot_count": capacity,
                    "request_id": request_id.to_string(),
                    "class": class.as_str(),
                }),
            );
        }
        if waited >= next_progress {
            eprintln!(
                "lucidos build-slot: still waiting after {}",
                fmt_secs(waited.as_secs())
            );
            next_progress = waited + PROGRESS_EVERY;
        }
    });
    #[cfg(unix)]
    drop(background_wait);

    match guard {
        Some(guard) => {
            let acquired_at = Instant::now();
            announce(
                "BuildSlotAcquired",
                &format!("A waiting build took a build slot: `{label}`"),
                serde_json::json!({
                    "label": label,
                    "slot": guard.index(),
                    "request_id": request_id.to_string(),
                    "class": class.as_str(),
                }),
            );
            Some((guard, acquired_at))
        }
        None => {
            let secs = max_wait_secs.unwrap_or_default();
            eprintln!(
                "lucidos build-slot: no slot after {secs}s, giving up (exit {WAIT_TIMEOUT_EXIT}). \
                 Subscribe instead of retrying on a timer: \
                 `lucidos await-event --on BuildSlotReleased --timeout-secs 3600 \
                 --reason \"a build slot to free up\"`, then end your turn."
            );
            None
        }
    }
}

fn describe_limits(index: &str, capacity: usize, limits: &BuildLimits) -> String {
    let priority = match limits.nice {
        0 => "priority unchanged".to_string(),
        n => format!("nice +{n}"),
    };
    let cores = match limits.jobs {
        Some(n) => format!("{n} cores"),
        None => "cores unchanged".to_string(),
    };
    format!("lucidos build-slot: slot {index} of {capacity}, {priority}, {cores}")
}

#[cfg(unix)]
fn lower_own_priority(increment: i32) {
    if increment <= 0 {
        return;
    }
    // SAFETY: a bare syscall on this process, with no pointer arguments.
    unsafe {
        libc::nice(increment);
    }
}

#[cfg(not(unix))]
fn lower_own_priority(_increment: i32) {}

fn spawn_child(
    command: &[String],
    slot: Option<&str>,
    limits: BuildLimits,
    request_id: Option<Uuid>,
    granted_class: Option<SlotClass>,
) -> Result<u8, BoxError> {
    let mut cmd = Command::new(&command[0]);
    cmd.args(&command[1..])
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    if let Some(index) = slot {
        cmd.env(ENV_HELD, index);
    }
    if let Some(id) = request_id {
        cmd.env(ENV_REQUEST_ID, id.to_string());
    } else {
        cmd.env_remove(ENV_REQUEST_ID);
    }
    match granted_class {
        Some(class) => cmd.env(ENV_GRANTED_CLASS, class.as_str()),
        None => cmd.env_remove(ENV_GRANTED_CLASS),
    };
    cmd.env_remove(ENV_LEASE_NICE);
    cmd.env_remove(ENV_CLASS);
    cmd.env_remove(ENV_LEGACY_PRIORITY);
    if let Some(jobs) = limits.jobs {
        cmd.env(ENV_CARGO_JOBS, jobs.to_string());
    }
    let status = cmd
        .status()
        .map_err(|e| format!("could not run `{}`: {e}", command[0]))?;
    Ok(exit_code(status))
}

fn exit_code(status: std::process::ExitStatus) -> u8 {
    if let Some(code) = status.code() {
        return u8::try_from(code).unwrap_or(1);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return u8::try_from(128 + signal).unwrap_or(1);
        }
    }
    1
}

fn announce(event_type: &str, summary: &str, mut payload: serde_json::Value) {
    let Ok(ws) = resolve_from_env() else {
        return;
    };
    if let Some(obj) = payload.as_object_mut() {
        obj.insert("summary".into(), serde_json::Value::String(summary.into()));
    }
    let _ = post_event(&ws, event_type, &payload);
}

fn post_event(
    ws: &Workspace,
    event_type: &str,
    payload: &serde_json::Value,
) -> Result<(), BoxError> {
    let client = crate::http::client_with_timeout(ANNOUNCE_TIMEOUT)?;
    let url = format!("{}/api/v1/events/emit", ws.base_url());
    client
        .post(&url)
        .json(&serde_json::json!({ "event_type": event_type, "payload": payload }))
        .send()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use lucidos_build_slot::SlotHolder;

    #[test]
    fn a_nested_step_under_a_lease_applies_the_lease_nice_once() {
        assert_eq!(
            BuildLimits::nested(Some("10")),
            BuildLimits {
                nice: 10,
                jobs: None
            }
        );
        assert_eq!(BuildLimits::nested(Some("-10")), BuildLimits::NONE);
    }

    #[test]
    fn a_nested_step_without_a_lease_still_imposes_nothing() {
        assert_eq!(BuildLimits::nested(None), BuildLimits::NONE);
        assert_eq!(BuildLimits::nested(Some("invalid")), BuildLimits::NONE);
    }

    #[test]
    fn an_unavailable_engine_does_not_delay_an_announcement() {
        let address = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap();
        let ws = Workspace {
            root: std::path::PathBuf::from("/unused"),
            api_port: address.port(),
            proto: "http".into(),
            api_base_override: Some(format!("http://{address}")),
        };
        let start = std::time::Instant::now();
        assert!(post_event(&ws, "BuildSlotReleased", &serde_json::json!({})).is_err());
        assert!(start.elapsed() < ANNOUNCE_TIMEOUT);
    }

    #[test]
    fn seconds_render_as_a_human_duration() {
        assert_eq!(fmt_secs(0), "0s");
        assert_eq!(fmt_secs(59), "59s");
        assert_eq!(fmt_secs(60), "1m00s");
        assert_eq!(fmt_secs(605), "10m05s");
    }

    #[test]
    fn a_free_slot_reads_as_free_and_a_held_one_names_its_holder() {
        let free = describe_slot(&SlotState {
            index: 1,
            holder: None,
        });
        assert_eq!(free, "  slot 1  free");

        let held = describe_slot(&SlotState {
            index: 0,
            holder: Some(SlotHolder::parse("PID=42\nLABEL=make lint\nCWD=/w\n")),
        });
        assert!(held.contains("HELD"), "{held}");
        assert!(held.contains("make lint"), "{held}");
        assert!(held.contains("pid 42"), "{held}");
        assert!(held.contains("/w"), "{held}");
    }

    #[test]
    fn a_holder_that_recorded_nothing_still_renders() {
        let held = describe_slot(&SlotState {
            index: 2,
            holder: Some(SlotHolder::default()),
        });
        assert!(held.contains("HELD"), "{held}");
        assert!(held.contains("pid ?"), "{held}");
    }

    #[test]
    fn the_granted_limits_are_reported_in_one_line() {
        let shaped = describe_limits(
            "1",
            3,
            &BuildLimits {
                nice: 10,
                jobs: Some(9),
            },
        );
        assert!(shaped.contains("slot 1 of 3"), "{shaped}");
        assert!(shaped.contains("nice +10"), "{shaped}");
        assert!(shaped.contains("9 cores"), "{shaped}");
    }

    #[test]
    fn imposing_nothing_says_so_rather_than_claiming_a_limit() {
        let inert = describe_limits("0", 1, &BuildLimits::NONE);
        assert!(inert.contains("priority unchanged"), "{inert}");
        assert!(inert.contains("cores unchanged"), "{inert}");
    }

    #[test]
    fn an_ordinary_exit_code_passes_through() {
        let status = std::process::Command::new("sh")
            .args(["-c", "exit 7"])
            .status()
            .expect("run sh");
        assert_eq!(exit_code(status), 7);
    }

    #[cfg(unix)]
    #[test]
    fn a_killed_child_never_reads_as_a_pass() {
        let status = std::process::Command::new("sh")
            .args(["-c", "kill -9 $$"])
            .status()
            .expect("run sh");
        assert_eq!(exit_code(status), 128 + 9);
    }

    #[test]
    fn the_wrapper_refuses_an_empty_command() {
        let err = run(BuildSlotArgs {
            status: false,
            lease: false,
            set_capacity: None,
            max_wait_secs: None,
            label: None,
            command: vec![],
        })
        .expect_err("an empty command is a usage error");
        assert!(err.to_string().contains("nothing to run"), "{err}");
    }

    fn argv(words: &[&str]) -> Vec<String> {
        words.iter().map(|w| w.to_string()).collect()
    }

    #[test]
    fn the_default_label_names_the_program_and_never_its_arguments() {
        let label = default_label(None, &argv(&["./deploy.sh", "--token", "s3cr3t"]));
        assert_eq!(label, "deploy.sh");
        assert!(!label.contains("s3cr3t"), "{label}");
    }

    #[test]
    fn a_program_word_that_carries_arguments_still_yields_only_the_program() {
        for (word, want) in [
            ("./deploy.sh --token s3cr3t", "deploy.sh"),
            ("deploy.sh --token=s3/cr3t", "deploy.sh"),
            ("API_KEY=s3/cr3t", "API_KEY"),
        ] {
            let label = default_label(None, &argv(&[word, "make"]));
            assert_eq!(label, want, "for {word:?}");
            assert!(!label.contains("cr3t"), "{label}");
        }
    }

    #[test]
    fn an_explicit_label_wins() {
        let label = default_label(Some("x"), &argv(&["./deploy.sh", "--token", "s3cr3t"]));
        assert_eq!(label, "x");
    }

    #[test]
    fn a_default_label_is_cut_to_the_label_limit() {
        let program = format!("/opt/{}", "b".repeat(LABEL_CHARS * 2));
        assert_eq!(default_label(None, &[program]).chars().count(), LABEL_CHARS);
    }
}
