use super::*;
use crate::test_support::{seed_chat_thread, setup_test_db, teardown_test_db};
use std::ffi::OsString;
use std::time::{Duration, Instant};

struct Bench {
    pool: sqlx::PgPool,
    db: String,
    bus: EventBus,
    registry: TerminalRegistry,
    workspace: tempfile::TempDir,
}

impl Bench {
    async fn new() -> Bench {
        let (pool, db) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        Bench {
            pool,
            db,
            bus,
            registry: TerminalRegistry::new(),
            workspace: tempfile::tempdir().unwrap(),
        }
    }

    fn spec(&self, thread_id: Uuid) -> OpenSpec {
        let home = self.workspace.path().as_os_str().to_owned();
        OpenSpec {
            thread_id,
            cwd: self.workspace.path().to_owned(),
            env: vec![
                (OsString::from("HOME"), home.clone()),
                (OsString::from("ZDOTDIR"), home),
                (OsString::from("PATH"), OsString::from("/usr/bin:/bin")),
                (OsString::from("TERM"), OsString::from("dumb")),
            ],
            size: Size::DEFAULT,
            actor: None,
        }
    }

    async fn open(&self, thread_id: Uuid) -> Arc<Terminal> {
        self.registry
            .open(
                &self.bus,
                self.workspace.path(),
                self.spec(thread_id),
                || async { true },
            )
            .await
            .expect("open")
    }

    async fn count(&self, thread_id: Uuid, event_type: &str) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM events WHERE thread_id = $1 AND event_type = $2")
            .bind(thread_id)
            .bind(event_type)
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }

    async fn exit_cause(&self, thread_id: Uuid) -> Option<String> {
        let deadline = Instant::now() + Duration::from_secs(15);
        while Instant::now() < deadline {
            let cause: Option<String> = sqlx::query_scalar(
                "SELECT payload->>'cause' FROM events WHERE thread_id = $1 AND event_type = 'TerminalExited'",
            )
            .bind(thread_id)
            .fetch_optional(&self.pool)
            .await
            .unwrap();
            if cause.is_some() {
                return cause;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        None
    }

    async fn finish(self) {
        self.registry.teardown(Duration::from_secs(5)).await;
        self.pool.close().await;
        teardown_test_db(&self.db).await;
    }
}

/// INVARIANT: A failed test must still end its shells. The runtime waits for
/// each shell's `child.wait()` on drop, so a live shell hangs the test binary.
impl Drop for Bench {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            return;
        }
        let Ok(slots) = self.registry.slots.try_lock() else {
            return;
        };
        for slot in slots.values() {
            if let Slot::Live(terminal) = slot {
                pty::signal_group(terminal.pid, libc::SIGKILL);
            }
        }
    }
}

async fn read_until(attachment: &mut Attachment, needle: &str) -> String {
    let mut seen = match &attachment.catch_up {
        CatchUp::Bytes { data, .. } | CatchUp::Reset { data, .. } => data.clone(),
    };
    let deadline = Instant::now() + Duration::from_secs(10);
    while !String::from_utf8_lossy(&seen).contains(needle) {
        let left = deadline.saturating_duration_since(Instant::now());
        assert!(
            !left.is_zero(),
            "never saw {needle:?} in {:?}",
            String::from_utf8_lossy(&seen)
        );
        match tokio::time::timeout(left, attachment.output.recv()).await {
            Ok(Ok(Output::Bytes { data, .. })) => seen.extend_from_slice(&data),
            Ok(Ok(Output::Exited(_))) => break,
            Ok(Ok(Output::Resized { .. } | Output::Attachments { .. })) => {}
            Ok(Err(_)) | Err(_) => break,
        }
    }
    String::from_utf8_lossy(&seen).into_owned()
}

#[tokio::test]
async fn two_opens_of_one_thread_share_one_terminal() {
    let bench = Bench::new().await;
    let thread_id = seed_chat_thread(&bench.bus, "terminal").await;
    let (a, b) = tokio::join!(bench.open(thread_id), bench.open(thread_id));
    assert_eq!(a.id, b.id);
    assert_eq!(bench.count(thread_id, "TerminalOpened").await, 1);
    bench.finish().await;
}

#[tokio::test]
async fn the_shell_outlives_its_last_attachment_and_a_resume_is_exact() {
    let bench = Bench::new().await;
    let thread_id = seed_chat_thread(&bench.bus, "terminal").await;
    let terminal = bench.open(thread_id).await;
    let mut first = terminal.attach(None);
    terminal.write(b"echo fi''rst\n").unwrap();
    read_until(&mut first, "first").await;
    let held = terminal.ring.lock().unwrap().end();
    drop(first);
    assert_eq!(terminal.attachment_count(), 0);
    terminal.write(b"echo sec''ond\n").unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        pty::is_alive(terminal.pid()),
        "dropping every attachment ended the shell"
    );
    let mut again = terminal.attach(Some(held));
    assert!(matches!(again.catch_up, CatchUp::Bytes { offset, .. } if offset == held));
    let text = read_until(&mut again, "second").await;
    assert!(
        !text.contains("first"),
        "the resume repeated bytes: {text:?}"
    );
    bench.finish().await;
}

#[tokio::test]
async fn ending_a_terminal_leaves_no_process_in_its_session() {
    let bench = Bench::new().await;
    let thread_id = seed_chat_thread(&bench.bus, "terminal").await;
    let terminal = bench.open(thread_id).await;
    let mut view = terminal.attach(None);
    terminal
        .write(b"trap '' HUP; (trap '' HUP; sleep 120) & echo bac''kground\n")
        .unwrap();
    read_until(&mut view, "background").await;
    let session = terminal.pid();
    assert!(
        bench
            .registry
            .end(thread_id, TerminalExitCause::Killed)
            .await
    );
    assert_eq!(bench.exit_cause(thread_id).await.as_deref(), Some("killed"));
    let deadline = Instant::now() + END_GRACE + Duration::from_secs(5);
    while !pty::session_members(session).is_empty() {
        assert!(
            Instant::now() < deadline,
            "a session member survived the end"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(!bench.registry.has_live(thread_id).await);
    bench.finish().await;
}

#[tokio::test]
async fn a_shell_that_exits_by_itself_records_exited_and_signals_nothing() {
    let bench = Bench::new().await;
    let thread_id = seed_chat_thread(&bench.bus, "terminal").await;
    let terminal = bench.open(thread_id).await;
    let mut view = terminal.attach(None);
    let marker = bench.workspace.path().join("survivor.pid");
    // The outer `sh -c` keeps the job out of the login shell's job table, so
    // zsh's running-jobs warning cannot refuse the `exit`.
    let command = format!(
        "sh -c \"nohup sh -c 'echo \\$\\$ > {}; exec sleep 30' </dev/null >/dev/null 2>&1 &\"; echo st''arted\n",
        marker.display()
    );
    terminal.write(command.as_bytes()).unwrap();
    read_until(&mut view, "started").await;
    terminal.write(b"exit\n").unwrap();
    assert_eq!(bench.exit_cause(thread_id).await.as_deref(), Some("exited"));
    let deadline = Instant::now() + Duration::from_secs(10);
    let survivor: u32 = loop {
        if let Some(pid) = std::fs::read_to_string(&marker)
            .ok()
            .and_then(|text| text.trim().parse().ok())
        {
            break pid;
        }
        assert!(
            Instant::now() < deadline,
            "the nohup job never wrote its pid"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    };
    assert!(pty::is_alive(survivor), "a natural exit killed a nohup job");
    // SAFETY: the test's own grandchild.
    unsafe { libc::kill(survivor as libc::pid_t, libc::SIGKILL) };
    bench.finish().await;
}

#[tokio::test]
async fn a_switch_turned_off_during_the_spawn_ends_the_new_shell() {
    let bench = Bench::new().await;
    let thread_id = seed_chat_thread(&bench.bus, "terminal").await;
    let result = bench
        .registry
        .open(
            &bench.bus,
            bench.workspace.path(),
            bench.spec(thread_id),
            || async { false },
        )
        .await;
    assert!(matches!(result, Err(OpenError::Disabled)));
    assert!(!bench.registry.has_live(thread_id).await);
    assert_eq!(
        bench.exit_cause(thread_id).await.as_deref(),
        Some("disabled")
    );
    bench.finish().await;
}

#[tokio::test]
async fn an_archive_during_the_spawn_ends_the_new_shell() {
    let bench = Bench::new().await;
    let thread_id = seed_chat_thread(&bench.bus, "terminal").await;
    let registry = bench.registry.clone();
    let result = bench
        .registry
        .open(
            &bench.bus,
            bench.workspace.path(),
            bench.spec(thread_id),
            || {
                let registry = registry.clone();
                async move {
                    assert!(registry.end(thread_id, TerminalExitCause::Archived).await);
                    true
                }
            },
        )
        .await;
    assert!(result.is_ok());
    assert!(!bench.registry.has_live(thread_id).await);
    assert_eq!(
        bench.exit_cause(thread_id).await.as_deref(),
        Some("archived")
    );
    bench.finish().await;
}

#[tokio::test]
async fn a_suggestion_is_typed_at_the_prompt_and_never_run() {
    let bench = Bench::new().await;
    let thread_id = seed_chat_thread(&bench.bus, "terminal").await;
    let terminal = bench.open(thread_id).await;
    let mut view = terminal.attach(None);
    terminal.write(b"echo re''ady\n").unwrap();
    read_until(&mut view, "ready").await;
    let marker = bench.workspace.path().join("ran");
    let command = format!("touch {}", marker.display());
    terminal.type_suggestion(&command).unwrap();
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert!(!marker.exists(), "the suggestion ran without Enter");
    terminal.write(b"\x15sleep 30\n").unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while terminal.shell_in_foreground() {
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(terminal.type_suggestion("ls"), Err(TypeError::Busy));
    assert!(matches!(
        terminal.type_suggestion("ls\nrm -rf /"),
        Err(TypeError::Refused(suggestion::Refusal::Unprintable))
    ));
    bench.finish().await;
}

#[tokio::test]
async fn no_terminal_byte_reaches_the_events_table() {
    let bench = Bench::new().await;
    let thread_id = seed_chat_thread(&bench.bus, "terminal").await;
    let terminal = bench.open(thread_id).await;
    let mut view = terminal.attach(None);
    terminal.write(b"echo zq''xmarker\n").unwrap();
    read_until(&mut view, "zqxmarker").await;
    bench
        .registry
        .end(thread_id, TerminalExitCause::Killed)
        .await;
    bench.exit_cause(thread_id).await;
    let leaked: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM events WHERE payload::text LIKE '%zqx%'")
            .fetch_one(&bench.pool)
            .await
            .unwrap();
    assert_eq!(leaked, 0);
    bench.finish().await;
}

#[tokio::test]
async fn teardown_ends_every_terminal_within_the_wait() {
    let bench = Bench::new().await;
    let one = seed_chat_thread(&bench.bus, "one").await;
    let two = seed_chat_thread(&bench.bus, "two").await;
    bench.open(one).await;
    bench.open(two).await;
    assert_eq!(bench.registry.teardown(Duration::from_secs(3)).await, 2);
    assert!(bench.registry.live().await.is_empty());
    assert_eq!(
        bench.exit_cause(one).await.as_deref(),
        Some("engine-stopped")
    );
    bench.finish().await;
}

#[tokio::test]
async fn the_workspace_cap_refuses_one_terminal_too_many() {
    let bench = Bench::new().await;
    for n in 0..WORKSPACE_CAP {
        let thread_id = seed_chat_thread(&bench.bus, &format!("t{n}")).await;
        bench.open(thread_id).await;
    }
    let extra = seed_chat_thread(&bench.bus, "extra").await;
    let refused = bench
        .registry
        .open(
            &bench.bus,
            bench.workspace.path(),
            bench.spec(extra),
            || async { true },
        )
        .await;
    assert!(matches!(refused, Err(OpenError::CapReached)));
    bench.finish().await;
}

#[tokio::test]
async fn a_slow_attachment_redraws_while_a_fast_one_stays_current() {
    let bench = Bench::new().await;
    let thread_id = seed_chat_thread(&bench.bus, "terminal").await;
    let terminal = bench.open(thread_id).await;
    let mut fast = terminal.attach(None);
    let mut slow = terminal.attach(None);
    let mut expected = match &fast.catch_up {
        CatchUp::Bytes { offset, data } | CatchUp::Reset { offset, data } => {
            offset + data.len() as u64
        }
    };
    // One small round at a time, each read in full by the fast attachment,
    // until the unread slow one holds more than the channel keeps.
    let mut round = 0;
    while slow.output.len() <= BROADCAST_CHUNKS {
        round += 1;
        assert!(round < 2000, "the slow attachment never filled its channel");
        let marker = format!("round{round}.");
        terminal
            .write(format!("echo round{round}''.\n").as_bytes())
            .unwrap();
        let mut tail = Vec::new();
        while !String::from_utf8_lossy(&tail).contains(&marker) {
            tail.extend_from_slice(&next_bytes(&mut fast, &mut expected).await);
        }
    }
    assert!(
        matches!(
            slow.output.recv().await,
            Err(broadcast::error::RecvError::Lagged(_))
        ),
        "the slow attachment never read, so it lagged"
    );
    let CatchUp::Reset { offset, data } = slow.redraw() else {
        panic!("a lagged attachment redraws from the ring");
    };
    let redrawn_to = offset + data.len() as u64;
    while expected < redrawn_to {
        next_bytes(&mut fast, &mut expected).await;
    }
    assert_eq!(redrawn_to, expected, "the redraw ends where the stream is");
    bench.finish().await;
}

/// The next output chunk, which must start exactly at `expected`.
async fn next_bytes(attachment: &mut Attachment, expected: &mut u64) -> Arc<[u8]> {
    loop {
        match tokio::time::timeout(Duration::from_secs(10), attachment.output.recv()).await {
            Ok(Ok(Output::Bytes { offset, data })) => {
                assert_eq!(offset, *expected, "the fast attachment saw a gap");
                *expected += data.len() as u64;
                return data;
            }
            Ok(Ok(_)) => {}
            other => panic!("the fast attachment fell behind: {other:?}"),
        }
    }
}
