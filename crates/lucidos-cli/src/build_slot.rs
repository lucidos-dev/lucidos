//! `lucidos build-slot`: run a heavy build under a *build slot*.
//!
//! The pool itself lives in `lucidos-build-slot`; this is its user-facing
//! door. Wrap a command and it waits for a free slot, then runs it as a child.
//! The slot frees when the child exits, or when this process dies.
//!
//! Everything here fails OPEN. A pool that cannot be opened, an engine that is
//! down, an announcement that does not send: none of them may be the reason a
//! build does not happen.

use std::process::{Command, Stdio};
use std::time::Duration;

use lucidos_build_slot::{inherited_slot, BuildSlotPool, SlotState, ENV_HELD};

use crate::workspace::{resolve_from_env, BoxError, Workspace};

/// Exit code when `--max-wait` elapsed with no slot free. 75 is the
/// `EX_TEMPFAIL` convention `scripts/lib/host_load_guard.sh` already uses for
/// backpressure, so an orchestrator can tell it from a failing build.
const WAIT_TIMEOUT_EXIT: u8 = 75;

/// How often the wait prints a progress line. Polling is far more frequent;
/// this only governs what a human sees.
const PROGRESS_EVERY: Duration = Duration::from_secs(15);

/// Ceiling on an announcement's round trip. An engine that is hung must delay
/// a build by seconds, never by the client's default half-minute.
const ANNOUNCE_TIMEOUT: Duration = Duration::from_secs(5);

/// Longest label recorded in slot metadata and carried on an event.
const LABEL_CHARS: usize = 120;

pub(crate) struct BuildSlotArgs {
    pub status: bool,
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
    cmd_wrap(args)
}

// ── set-capacity ────────────────────────────────────────────────────────

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

// ── status ──────────────────────────────────────────────────────────────

fn cmd_status() -> Result<u8, BoxError> {
    let pool = BuildSlotPool::open()?;
    let capacity = pool.capacity();
    let states = pool.status();
    let held = states.iter().filter(|s| s.holder.is_some()).count();

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

// ── wrapping a command ──────────────────────────────────────────────────

fn cmd_wrap(args: BuildSlotArgs) -> Result<u8, BoxError> {
    if args.command.is_empty() {
        return Err("nothing to run. Usage: lucidos build-slot -- <command> [args...]".into());
    }
    let label = args
        .label
        .clone()
        .unwrap_or_else(|| args.command.join(" "))
        .chars()
        .take(LABEL_CHARS)
        .collect::<String>();

    // Already inside a wrapped build: run straight through. Without this a
    // `make test` that wraps, calling a script that also wraps, would wait for
    // a slot it is already holding.
    if let Some(index) = inherited_slot() {
        return spawn_child(&args.command, Some(&index));
    }

    let pool = match BuildSlotPool::open() {
        Ok(pool) => pool,
        Err(e) => {
            eprintln!("lucidos build-slot: no pool ({e}), running the build unrestricted");
            return spawn_child(&args.command, None);
        }
    };

    let guard = match pool.try_acquire(&label) {
        Some(guard) => guard,
        None => match wait_for_slot(&pool, &label, args.max_wait_secs) {
            Some(guard) => guard,
            None => return Ok(WAIT_TIMEOUT_EXIT),
        },
    };

    let index = guard.index().to_string();
    let code = spawn_child(&args.command, Some(&index));

    // Free the slot BEFORE announcing, so a waiter woken by the event finds it
    // already available rather than racing the drop.
    drop(guard);
    // UNCONDITIONAL, and it must stay that way. Gating this on "is anyone
    // waiting right now?" looks like a saving and is a trap: the subscriber
    // this event exists for has ALREADY given up and exited, which is why it
    // subscribed. Its flag went with it, so the probe says nobody is waiting
    // and the sleeper is never woken. One row per build is the price of the
    // `--max-wait` recovery path being real (ADR 0070).
    announce(
        "BuildSlotReleased",
        &format!("A build slot freed up after `{label}`"),
        serde_json::json!({ "label": label, "slot": index }),
    );
    code
}

/// Block until a slot frees, reporting progress. `None` means `--max-wait`
/// elapsed, which is the only way this returns without a slot.
fn wait_for_slot(
    pool: &BuildSlotPool,
    label: &str,
    max_wait_secs: Option<u64>,
) -> Option<lucidos_build_slot::BuildSlotGuard> {
    let capacity = pool.capacity().value;
    eprintln!(
        "lucidos build-slot: all {capacity} build slots are busy, waiting for one. \
         `lucidos build-slot --status` shows who holds them."
    );

    let mut next_progress = PROGRESS_EVERY;
    let mut announced = false;
    let guard = pool.acquire(label, max_wait_secs.map(Duration::from_secs), |waited| {
        // Announced from inside the wait, not before it. `acquire` raises the
        // waiting flag before this first runs. So a holder releasing during
        // the announcement's own round trip already sees a queued build, and
        // emits its release. Announcing first left that window silent.
        if !announced {
            announced = true;
            announce(
                "BuildSlotWaiting",
                &format!("A build is waiting for one of {capacity} build slots: `{label}`"),
                serde_json::json!({ "label": label, "slot_count": capacity }),
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

    match guard {
        Some(guard) => {
            announce(
                "BuildSlotAcquired",
                &format!("A waiting build took a build slot: `{label}`"),
                serde_json::json!({ "label": label, "slot": guard.index() }),
            );
            Some(guard)
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

/// Run the wrapped command with the slot marked in its environment, inheriting
/// stdio so the build looks exactly as it would unwrapped.
///
/// Deliberately NOT in a process group of its own. Staying in ours is what
/// makes a group signal reach the build too: a terminal Ctrl-C, and the
/// engine's `BuildProcessGroupGuard`, which SIGKILLs the whole group when it
/// coalesces an Apply. Calling `process_group` here would orphan the build.
/// It would then compile on with its slot already freed (ADR 0070, and the
/// "orphaned build outlives its wrapper" row in `docs/code-review-priors.md`).
fn spawn_child(command: &[String], slot: Option<&str>) -> Result<u8, BoxError> {
    let mut cmd = Command::new(&command[0]);
    cmd.args(&command[1..])
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    if let Some(index) = slot {
        cmd.env(ENV_HELD, index);
    }
    let status = cmd
        .status()
        .map_err(|e| format!("could not run `{}`: {e}", command[0]))?;
    Ok(exit_code(status))
}

/// The child's exit code, as this process's own. A signalled child reports
/// `128 + signal`, the shell convention, so a killed build never reads as a
/// pass.
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

// ── announcing ──────────────────────────────────────────────────────────

/// Emit a domain event, best effort.
///
/// Silent on every failure, because announcing must never block, slow or fail
/// a build. An engine that is down simply means nobody is told, and a waiter
/// recovers through its own polling.
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
    // Through the shared constructor, so the announcement carries the same
    // origin token and workspace assertion as every other CLI call.
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
        // Metadata is written just after the lock is taken, so a probe can
        // land on a slot that is held but not yet described.
        let held = describe_slot(&SlotState {
            index: 2,
            holder: Some(SlotHolder::default()),
        });
        assert!(held.contains("HELD"), "{held}");
        assert!(held.contains("pid ?"), "{held}");
    }

    #[test]
    fn an_ordinary_exit_code_passes_through() {
        // A failing `make lint` must fail the wrapper, or the gate is useless.
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
            set_capacity: None,
            max_wait_secs: None,
            label: None,
            command: vec![],
        })
        .expect_err("an empty command is a usage error");
        assert!(err.to_string().contains("nothing to run"), "{err}");
    }
}
