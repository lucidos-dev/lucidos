use std::collections::VecDeque;
use std::ffi::CString;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

const STREAM_MINUTES: u64 = 15;
const STREAM_LIFETIME: Duration = Duration::from_secs(STREAM_MINUTES * 60);
static LONGEST_COMMAND_SECS: AtomicU64 = AtomicU64::new(0);
const READY_WAIT: Duration = Duration::from_secs(2);
const KEPT_RECORDS: usize = 512;
const ARRIVAL_WAIT: Duration = Duration::from_millis(500);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StreamProblem {
    NotRenewed,
    NotStarted,
}

enum State {
    Idle,
    Starting,
    Live {
        generation: u64,
        since: Instant,
        renewing: bool,
    },
    Failed {
        at: Instant,
    },
}

struct Watch {
    state: State,
    generation: u64,
}

static WATCH: Mutex<Watch> = Mutex::new(Watch {
    state: State::Idle,
    generation: 0,
});
static RECORDS: Mutex<VecDeque<(Instant, String)>> = Mutex::new(VecDeque::new());

fn watch() -> MutexGuard<'static, Watch> {
    WATCH.lock().unwrap_or_else(PoisonError::into_inner)
}

pub fn ensure(longest_command: Duration, report: fn(StreamProblem)) {
    LONGEST_COMMAND_SECS.store(longest_command.as_secs(), Ordering::Relaxed);
    let generation = {
        let mut guard = watch();
        let watch = &mut *guard;
        match watch.state {
            State::Starting => return,
            State::Failed { at } if at.elapsed() < STREAM_LIFETIME => return,
            State::Live {
                since,
                ref mut renewing,
                ..
            } => {
                if *renewing || since.elapsed() < STREAM_LIFETIME.saturating_sub(longest_command) {
                    return;
                }
                *renewing = true;
                watch.generation += 1;
                let generation = watch.generation;
                let renewal = std::thread::Builder::new()
                    .name("seatbelt-watch-renew".into())
                    .spawn(move || settle(generation, start(generation), report));
                if renewal.is_err() {
                    report(StreamProblem::NotRenewed);
                }
                return;
            }
            State::Idle | State::Failed { .. } => {
                watch.state = State::Starting;
                watch.generation += 1;
                watch.generation
            }
        }
    };
    settle(generation, off_the_runtime(|| start(generation)), report);
}

fn settle(generation: u64, stream: Option<Arc<AtomicBool>>, report: fn(StreamProblem)) {
    let mut watch = watch();
    if stream.is_some_and(|running| running.load(Ordering::Acquire)) {
        watch.state = State::Live {
            generation,
            since: Instant::now(),
            renewing: false,
        };
        return;
    }
    report(StreamProblem::NotStarted);
    if matches!(watch.state, State::Starting) {
        watch.state = State::Failed { at: Instant::now() };
    }
}

fn off_the_runtime<T>(wait: impl FnOnce() -> T) -> T {
    match tokio::runtime::Handle::try_current() {
        Ok(runtime) if runtime.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread => {
            tokio::task::block_in_place(wait)
        }
        _ => wait(),
    }
}

pub async fn denial(pid: u32, started: chrono::DateTime<chrono::Utc>) -> Option<String> {
    let live = matches!(watch().state, State::Live { .. });
    let deadline = Instant::now() + ARRIVAL_WAIT;
    loop {
        let found = RECORDS
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .find_map(|(_, line)| seatbelt_denial(line, pid, started));
        if found.is_some() || !live || Instant::now() >= deadline {
            return found;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn start(generation: u64) -> Option<Arc<AtomicBool>> {
    let marker = format!("lucidos-seatbelt-watch-{}-{generation}", std::process::id());
    let predicate = predicate(&marker);
    let Ok(mut child) = Command::new("/usr/bin/log")
        .args(["stream", "--style", "ndjson", "--timeout"])
        .arg(format!("{STREAM_MINUTES}m"))
        .arg("--predicate")
        .arg(&predicate)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return None;
    };
    let pid = child.id();
    let Some(stdout) = child.stdout.take() else {
        stop(pid);
        return None;
    };
    let (ready_tx, ready_rx) = mpsc::channel();
    let reader_marker = marker.clone();
    let running = Arc::new(AtomicBool::new(true));
    let reader_running = running.clone();
    let reader = std::thread::Builder::new()
        .name("seatbelt-watch".into())
        .spawn(move || {
            let mut ready_tx = Some(ready_tx);
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if is_marker(&line, &reader_marker) {
                    if let Some(tx) = ready_tx.take() {
                        let _ = tx.send(true);
                    }
                } else {
                    keep(line);
                }
            }
            if let Some(tx) = ready_tx {
                let _ = tx.send(false);
            }
            let _ = child.wait();
            reader_running.store(false, Ordering::Release);
            ended(generation);
        });
    if reader.is_err() {
        stop(pid);
        return None;
    }
    let Ok(message) = CString::new(marker) else {
        stop(pid);
        return None;
    };
    let deadline = Instant::now() + READY_WAIT;
    while Instant::now() < deadline {
        // SAFETY: a constant format string and one NUL-terminated argument.
        unsafe { libc::syslog(libc::LOG_NOTICE, c"%s".as_ptr(), message.as_ptr()) };
        match ready_rx.recv_timeout(Duration::from_millis(100)) {
            Ok(true) => return Some(running),
            Ok(false) | Err(RecvTimeoutError::Disconnected) => return None,
            Err(RecvTimeoutError::Timeout) => {}
        }
    }
    stop(pid);
    None
}

fn predicate(marker: &str) -> String {
    let senders = SEATBELT_SENDERS
        .iter()
        .map(|sender| format!(r#"processImagePath == "{sender}""#))
        .collect::<Vec<_>>()
        .join(" OR ");
    format!(
        r#"(({senders}) AND eventMessage CONTAINS " deny(" AND eventMessage CONTAINS " file-write") OR eventMessage == "{marker}""#
    )
}

fn is_marker(line: &str, marker: &str) -> bool {
    line.contains(marker)
        && serde_json::from_str::<serde_json::Value>(line)
            .is_ok_and(|entry| entry["eventMessage"].as_str() == Some(marker))
}

fn keep(line: String) {
    let longest_command = Duration::from_secs(LONGEST_COMMAND_SECS.load(Ordering::Relaxed));
    let mut records = RECORDS.lock().unwrap_or_else(PoisonError::into_inner);
    while records.len() >= KEPT_RECORDS
        || records
            .front()
            .is_some_and(|(at, _)| at.elapsed() > longest_command)
    {
        records.pop_front();
    }
    records.push_back((Instant::now(), line));
}

fn ended(generation: u64) {
    let mut watch = watch();
    if matches!(watch.state, State::Live { generation: live, .. } if live == generation) {
        watch.state = State::Idle;
    }
}

fn stop(pid: u32) {
    if let Ok(pid) = libc::pid_t::try_from(pid) {
        // SAFETY: `pid` is the stream this module spawned and has not reaped.
        unsafe { libc::kill(pid, libc::SIGTERM) };
    }
}

pub const SEATBELT_SENDERS: [&str; 2] = ["/kernel", "/usr/libexec/sandboxd"];

pub fn seatbelt_denial(
    ndjson: &str,
    pid: u32,
    started: chrono::DateTime<chrono::Utc>,
) -> Option<String> {
    let marker = format!("({pid}) deny(");
    ndjson.lines().find_map(|line| {
        let entry = serde_json::from_str::<serde_json::Value>(line).ok()?;
        let at = chrono::DateTime::parse_from_str(
            entry["timestamp"].as_str()?,
            "%Y-%m-%d %H:%M:%S%.f%z",
        )
        .ok()?;
        let sender = entry["processImagePath"].as_str()?;
        if at < started || !SEATBELT_SENDERS.contains(&sender) {
            return None;
        }
        let message = entry["eventMessage"].as_str()?.strip_prefix("Sandbox: ")?;
        let (_, denial) = message.split_once(&marker)?;
        let (_, operation) = denial.split_once(") ")?;
        let (kind, path) = operation.split_once(' ')?;
        (kind.starts_with("file-write") && path.starts_with('/'))
            .then(|| format!("{}{path}", crate::write_refusal::REFUSED_WRITE))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn log_line(sender: &str, timestamp: &str, message: &str) -> String {
        serde_json::json!({
            "eventMessage": message,
            "eventType": "logEvent",
            "messageType": "Error",
            "processID": 0,
            "processImagePath": sender,
            "senderImagePath": "/System/Library/Extensions/Sandbox.kext/Contents/MacOS/Sandbox",
            "subsystem": "",
            "category": "",
            "timestamp": timestamp,
        })
        .to_string()
    }

    #[test]
    fn seatbelt_denials_come_from_the_sandbox_for_the_launched_process() {
        let started = chrono::DateTime::parse_from_rfc3339("2026-11-19T10:00:00+01:00")
            .unwrap()
            .to_utc();
        let at = "2026-11-19 10:00:01.250000+0100";
        let denial = |sender: &str, at: &str, message: &str| {
            seatbelt_denial(&log_line(sender, at, message), 4242, started)
        };
        let expected = Some("Sandbox refused a write to /outside/a b".to_string());
        let create = "Sandbox: sh(4242) deny(1) file-write-create /outside/a b";
        assert_eq!(denial("/kernel", at, create), expected);
        assert_eq!(denial("/usr/libexec/sandboxd", at, create), expected);
        assert_eq!(
            denial(
                "/kernel",
                at,
                "Sandbox: sh(4242) deny(1) file-write-mode /outside/a b"
            ),
            expected
        );
        assert_eq!(denial("/usr/bin/logger", at, create), None);
        assert_eq!(denial("/bin/sh", at, create), None);
        assert_eq!(
            denial("/kernel", "2026-11-19 09:59:59.000000+0100", create),
            None
        );
        assert_eq!(
            denial(
                "/kernel",
                at,
                "Sandbox: sh(42420) deny(1) file-write-create /outside/x"
            ),
            None
        );
        assert_eq!(
            denial(
                "/kernel",
                at,
                "Sandbox: sh(4242) deny(1) file-read-data /outside/x"
            ),
            None
        );
        assert_eq!(
            denial(
                "/kernel",
                at,
                "Sandbox: sh(4242) deny(1) file-write-create outside/x"
            ),
            None
        );
        assert_eq!(seatbelt_denial("not json\n", 4242, started), None);
        let later = log_line(
            "/kernel",
            at,
            "Sandbox: sh(4242) deny(1) file-write-unlink /outside/second",
        );
        assert_eq!(
            seatbelt_denial(
                &format!("{}\n{later}", log_line("/bin/sh", at, create)),
                4242,
                started
            ),
            Some("Sandbox refused a write to /outside/second".to_string())
        );
    }

    #[test]
    fn the_stream_keeps_trusted_write_denials_and_the_marker() {
        assert_eq!(
            predicate("m"),
            r#"((processImagePath == "/kernel" OR processImagePath == "/usr/libexec/sandboxd") AND eventMessage CONTAINS " deny(" AND eventMessage CONTAINS " file-write") OR eventMessage == "m""#
        );
    }

    #[test]
    fn only_the_exact_message_is_the_marker() {
        let line = |message: &str| serde_json::json!({ "eventMessage": message }).to_string();
        assert!(is_marker(
            &line("lucidos-seatbelt-watch-1-2"),
            "lucidos-seatbelt-watch-1-2"
        ));
        assert!(!is_marker(
            &line(
                r#"log run noninteractively, args: 'eventMessage == "lucidos-seatbelt-watch-1-2"'"#
            ),
            "lucidos-seatbelt-watch-1-2"
        ));
        assert!(!is_marker(
            "lucidos-seatbelt-watch-1-2",
            "lucidos-seatbelt-watch-1-2"
        ));
    }
}
