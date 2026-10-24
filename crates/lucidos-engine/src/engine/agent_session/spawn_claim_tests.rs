use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{Barrier, Mutex};
use uuid::Uuid;

use super::{claim_thread_for, hand_hold_to_run, name_requested_settings, ClaimFor, ClaimRefused};
use crate::engine::types::{AgentSession, SessionPhase};
use crate::runtime::CodingAgent;

type Sessions = Arc<Mutex<HashMap<Uuid, AgentSession>>>;

/// Two spawns for one thread. Spawn A claims, then parks where the real spawn
/// does its startup work (worktree, semaphore, process). Spawn B arrives in that
/// gap. Before the claim inserted its entry, B found the thread free too.
#[tokio::test]
async fn a_spawn_that_has_not_started_yet_refuses_a_second_spawn() {
    let sessions: Sessions = Arc::default();
    let thread = Uuid::new_v4();
    let a_claimed = Arc::new(Barrier::new(2));
    let b_done = Arc::new(Barrier::new(2));

    let spawn_a = tokio::spawn({
        let (sessions, a_claimed, b_done) = (sessions.clone(), a_claimed.clone(), b_done.clone());
        async move {
            let entry = claim_thread_for(
                &mut *sessions.lock().await,
                thread,
                CodingAgent::ClaudeCode,
                ClaimFor::Run,
            );
            a_claimed.wait().await;
            b_done.wait().await;
            entry.is_ok()
        }
    });
    let spawn_b = tokio::spawn({
        let sessions = sessions.clone();
        async move {
            a_claimed.wait().await;
            let outcome = claim_thread_for(
                &mut *sessions.lock().await,
                thread,
                CodingAgent::ClaudeCode,
                ClaimFor::Run,
            )
            .map(|_| ());
            b_done.wait().await;
            outcome
        }
    });

    assert!(spawn_a.await.unwrap(), "the first spawn claims the thread");
    assert!(
        matches!(spawn_b.await.unwrap(), Err(ClaimRefused::LiveSession)),
        "the second spawn sees the first while it is still starting"
    );
}

#[tokio::test]
async fn a_claimed_entry_is_starting_and_owns_the_channel() {
    let mut sessions = HashMap::new();
    let thread = Uuid::new_v4();
    let entry = claim_thread_for(&mut sessions, thread, CodingAgent::Codex, ClaimFor::Run).unwrap();
    let session = &sessions[&thread];
    assert!(session.is_starting());
    assert!(session.is_live(), "a follow-up can queue during startup");
    assert!(session.msg_tx.same_channel(&entry.msg_tx));
    assert_eq!(session.coding_agent, CodingAgent::Codex);
}

/// The commands API reads a starting entry, so a request's pick shows from the
/// claim on. A pick the request left out keeps the previous session's.
#[tokio::test]
async fn a_starting_entry_names_the_requested_model_and_keeps_the_rest() {
    let mut sessions = HashMap::new();
    let thread = Uuid::new_v4();
    let (mut previous, _rx) = AgentSession::for_test();
    previous.current_reasoning_effort = Some("high".into());
    previous.msg_tx = tokio::sync::mpsc::unbounded_channel().0;
    sessions.insert(thread, previous);

    let entry = claim_thread_for(
        &mut sessions,
        thread,
        CodingAgent::ClaudeCode,
        ClaimFor::Run,
    )
    .unwrap();
    name_requested_settings(&mut sessions, thread, &entry, Some("haiku"), None);
    let session = &sessions[&thread];
    assert!(session.is_starting());
    assert_eq!(session.current_model.as_deref(), Some("haiku"));
    assert_eq!(session.current_reasoning_effort.as_deref(), Some("high"));
}

#[tokio::test]
async fn a_stop_during_startup_cancels_the_spawn_rather_than_queueing_an_interrupt() {
    let mut sessions = HashMap::new();
    let thread = Uuid::new_v4();
    let entry = claim_thread_for(
        &mut sessions,
        thread,
        CodingAgent::ClaudeCode,
        ClaimFor::Run,
    )
    .unwrap();
    sessions[&thread].interrupt_or_cancel_startup();
    assert!(entry.startup_cancel.is_cancelled());
    let interrupted = tokio::time::timeout(std::time::Duration::ZERO, entry.interrupt.notified());
    assert!(
        interrupted.await.is_err(),
        "no interrupt permit waits for the agent's first turn"
    );
}

#[tokio::test]
async fn a_running_session_takes_the_stop_as_an_interrupt() {
    let (mut session, _rx) = AgentSession::for_test();
    session.is_waiting = false;
    let interrupt = session.interrupt.clone();
    session.interrupt_or_cancel_startup();
    let interrupted = tokio::time::timeout(std::time::Duration::ZERO, interrupt.notified());
    assert!(interrupted.await.is_ok());
}

/// A recovery spawn (hardening, a merge session, a re-attached conflict duty)
/// used to skip the check and insert over whatever held the thread.
#[tokio::test]
async fn no_spawn_replaces_a_live_session() {
    let mut sessions = HashMap::new();
    let thread = Uuid::new_v4();
    let (live, _rx) = AgentSession::for_test();
    sessions.insert(thread, live);
    assert!(matches!(
        claim_thread_for(
            &mut sessions,
            thread,
            CodingAgent::ClaudeCode,
            ClaimFor::Run
        ),
        Err(ClaimRefused::LiveSession)
    ));
    assert!(matches!(
        sessions[&thread].phase,
        SessionPhase::Running { .. }
    ));
}

/// A real input never joins a hidden keep-alive turn. A live keep-alive run
/// reads as a hold, so a real claim waits for it to end, and a second
/// keep-alive skips.
#[tokio::test]
async fn a_live_keep_alive_run_holds_its_thread_against_real_input() {
    let mut sessions = HashMap::new();
    let thread = Uuid::new_v4();
    let entry = claim_thread_for(
        &mut sessions,
        thread,
        CodingAgent::ClaudeCode,
        ClaimFor::KeepAlive,
    )
    .unwrap();
    assert!(sessions[&thread].keep_alive);
    for purpose in [ClaimFor::Run, ClaimFor::KeepAlive] {
        assert!(matches!(
            claim_thread_for(&mut sessions, thread, CodingAgent::ClaudeCode, purpose),
            Err(ClaimRefused::Held { .. })
        ));
    }
    drop(entry);
    let real = claim_thread_for(
        &mut sessions,
        thread,
        CodingAgent::ClaudeCode,
        ClaimFor::Run,
    );
    assert!(
        real.is_ok(),
        "once the keep-alive lets go, the real run claims"
    );
    assert!(!sessions[&thread].keep_alive);
}

/// `finalize_direct_agent` marks the process exited first, then works in the
/// worktree. A spawn must wait for it to let go, not start beside it.
#[tokio::test]
async fn a_finishing_run_holds_its_thread_until_it_lets_go() {
    let mut sessions = HashMap::new();
    let thread = Uuid::new_v4();
    let (mut finishing, rx) = AgentSession::for_test();
    finishing.process_exited = true;
    finishing.current_model = Some("opus".into());
    sessions.insert(thread, finishing);

    let Err(ClaimRefused::Held { holder: sender, .. }) = claim_thread_for(
        &mut sessions,
        thread,
        CodingAgent::ClaudeCode,
        ClaimFor::Run,
    ) else {
        panic!("a finishing run still holds the thread");
    };
    drop(rx);
    sender.closed().await;

    claim_thread_for(
        &mut sessions,
        thread,
        CodingAgent::ClaudeCode,
        ClaimFor::Run,
    )
    .unwrap();
    assert!(sessions[&thread].is_starting());
    assert_eq!(
        sessions[&thread].current_model.as_deref(),
        Some("opus"),
        "the replaced entry's model carries over to the next spawn"
    );
}

/// ADR 0249 accepted that a child's wake starts a turn in the worktree a
/// Tier-2 apply is resetting. The apply now holds the thread, and the wake's
/// claim waits until the apply lets go.
#[tokio::test]
async fn a_spawn_waits_while_an_apply_resets_the_worktree() {
    let mut sessions = HashMap::new();
    let thread = Uuid::new_v4();
    let hold = claim_thread_for(
        &mut sessions,
        thread,
        CodingAgent::ClaudeCode,
        ClaimFor::Apply,
    )
    .unwrap();
    assert!(matches!(sessions[&thread].phase, SessionPhase::Applying));

    let Err(ClaimRefused::Held { holder, .. }) = claim_thread_for(
        &mut sessions,
        thread,
        CodingAgent::ClaudeCode,
        ClaimFor::Run,
    ) else {
        panic!("a wake must not start a turn while the apply holds the thread");
    };
    let released = holder.closed();
    drop(hold);
    released.await;

    claim_thread_for(
        &mut sessions,
        thread,
        CodingAgent::ClaudeCode,
        ClaimFor::Run,
    )
    .unwrap();
    assert!(
        sessions[&thread].is_starting(),
        "the wake runs once the apply is done"
    );
}

/// The apply's own merge session takes the thread over in one step. A spawn
/// waiting on the hold wakes and finds a live run, so its input routes there.
#[tokio::test]
async fn an_apply_hands_the_thread_to_its_merge_session_without_a_gap() {
    let mut sessions = HashMap::new();
    let thread = Uuid::new_v4();
    let hold = claim_thread_for(
        &mut sessions,
        thread,
        CodingAgent::ClaudeCode,
        ClaimFor::Apply,
    )
    .unwrap();
    let Err(ClaimRefused::Held { phase_changed, .. }) = claim_thread_for(
        &mut sessions,
        thread,
        CodingAgent::ClaudeCode,
        ClaimFor::Run,
    ) else {
        panic!("the apply holds the thread");
    };
    let handed_off = phase_changed.notified_owned();

    hand_hold_to_run(&mut sessions, thread, &hold);

    let woke = tokio::time::timeout(std::time::Duration::ZERO, handed_off);
    assert!(
        woke.await.is_ok(),
        "the waiting spawn wakes on the hand-off"
    );
    assert!(sessions[&thread].is_starting());
    assert!(matches!(
        claim_thread_for(
            &mut sessions,
            thread,
            CodingAgent::ClaudeCode,
            ClaimFor::Run
        ),
        Err(ClaimRefused::LiveSession)
    ));
}
