//! Waking the thread's own agent with what the caller just said.
//!
//! A delegated utterance is a message on a chat thread, so it takes the path a
//! typed one takes: `pre_emit_chat_message_received` records it, and
//! `process_message_with_steps` runs the turn. Voice adds no admission rule.
//! Single-flight admission is what turns a second utterance mid-turn into an
//! injection rather than a race.
//!
//! **A chat thread is the only thread this reaches** (ADR 0165). The Lucidos
//! Agent is the one agent voice wakes, so [`doer_for`] is asked before anything
//! is written. `api::voice::admit` asks the same question at the socket, which
//! is where a caller can act on the answer. This one is the floor under it: a
//! destination flipped mid-call moves the thread's row while the socket is
//! already open.
//!
//! **Not every utterance comes here.** The talker decides, with its `delegate`
//! tool, and one it answers alone is written down by `call.rs` instead. So this
//! is the delegated half only.
//!
//! **A seam, so the call loop stays drivable without an engine.** `call.rs`
//! talks to a [`TurnStarter`], the same way it talks to a `CallTransport`. The
//! tests supply one that records, so a whole call runs with no credential.
//!
//! Nothing here reaches back the other way. The doer is never told a
//! session is live (ADR 0149): it is shown one, by reading the talker's turns
//! in the thread.

use std::sync::Arc;

use async_trait::async_trait;
use uuid::Uuid;

use crate::engine::thread_events::{ActorMode, MessageOrigin};
use crate::engine::thread_lifecycle::ThreadType;
use crate::engine::LucidosEngine;

/// Which agent holds a thread, as far as a call is concerned.
///
/// Four answers rather than a boolean, because each caller owes a different
/// one of them a different response. A lookup that could not run is UNKNOWN
/// and never a "yes": starting the wrong agent writes a chat-channel message
/// into a coding-agent thread, where refusing costs the caller one repeat.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadDoer {
    /// The Lucidos Agent, the one agent a call reaches (ADR 0165).
    LucidosAgent,
    /// A coding agent. Voice does not reach one.
    CodingAgent,
    /// No row for this thread.
    NoSuchThread,
    /// The lookup itself failed, so who holds the thread is not known.
    Unknown,
}

/// Ask a thread's projection row which agent holds it.
///
/// `thread_summaries.source` is the whole answer, and it is already correct
/// before a draft's first send: the compose write mirrors `compose_mode` into
/// it, so a draft with a coding destination picked reads `claude_code` with no
/// message in it yet.
pub async fn doer_for(pool: &sqlx::PgPool, thread_id: Uuid) -> ThreadDoer {
    let source: Result<Option<(String,)>, _> =
        sqlx::query_as("SELECT source FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_optional(pool)
            .await;
    match source {
        Ok(Some((source,))) => match ThreadType::from_source(&source) {
            ThreadType::Chat => ThreadDoer::LucidosAgent,
            ThreadType::CodingAgent => ThreadDoer::CodingAgent,
        },
        Ok(None) => ThreadDoer::NoSuchThread,
        Err(e) => {
            log!(
                "[Voice] Could not read who holds thread {}: {}",
                thread_id,
                e
            );
            ThreadDoer::Unknown
        }
    }
}

/// What a delegated utterance does to the thread it was spoken on.
#[async_trait]
pub trait TurnStarter: Send + Sync {
    /// The talker asked for the doer. Record the utterance, and get it working.
    ///
    /// **Only the delegated half comes here.** An utterance the talker answers
    /// alone never reaches this: `call.rs` writes it down as a
    /// `SpokenMessageReceived`, which starts nothing. That split is what keeps
    /// a talker-only turn from leaving the thread claiming a turn.
    ///
    /// "Wake" is what happens on this side, not what the talker asked for. A
    /// turn already running absorbs this one, because single-flight admission
    /// turns a second message into an injection. The talker is never told
    /// which it got, and never has to be.
    ///
    /// Returns once the utterance is recorded, never once the turn is done. The
    /// call loop has audio to pump, and a turn can take minutes.
    ///
    /// `session_id` is what marks the message as spoken. It is the whole of
    /// how the transcript tells speech from typing: the composer stays live
    /// during a call (ADR 0148), so nothing around the message can say.
    ///
    /// Returns whether the utterance was TAKEN. `false` means no turn started
    /// and nothing was written, so the caller's words are still owed a row and
    /// `call.rs` writes one. Without that answer a refusal here would leave a
    /// `WorkDelegated` beside no record of what was actually said.
    async fn wake(
        &self,
        thread_id: Uuid,
        session_id: Uuid,
        transcript: &str,
        actor: Option<MessageOrigin>,
    ) -> bool;

    /// The talker said this out loud. Offer it to a turn already running.
    ///
    /// An offer, not a wake. With no turn running there is nothing to inject
    /// into, and the reply is dropped here: `SpokenReplyGenerated` is already
    /// in the thread, so the next round reads it from history either way.
    ///
    /// What this buys is the round LEARNING it mid-flight. Otherwise the doer
    /// finishes without knowing what the caller was told in its name.
    async fn overheard(&self, thread_id: Uuid, spoken: &str);
}

/// How a spoken aside is framed for the running turn.
///
/// `Standalone` injections are pushed raw, so the framing is written here
/// rather than by `framed_injected_prompt`. It says nothing is being asked. A
/// turn reading this as an instruction would drop the work it is doing.
const SAID_ALOUD: &str = "[SAID ALOUD] The caller heard this from you just now, \
                          on the call. It is what they already know. Carry on \
                          with your work; nothing here is a new request.";

/// The shipping implementation: the ordinary chat turn, unchanged.
pub struct ThreadTurn {
    engine: Arc<LucidosEngine>,
}

impl ThreadTurn {
    pub fn new(engine: Arc<LucidosEngine>) -> Self {
        Self { engine }
    }
}

#[async_trait]
impl TurnStarter for ThreadTurn {
    async fn wake(
        &self,
        thread_id: Uuid,
        session_id: Uuid,
        transcript: &str,
        actor: Option<MessageOrigin>,
    ) -> bool {
        let message = transcript.trim().to_string();
        if message.is_empty() {
            // Taken, in the sense that matters: there is nothing to write down
            // and nothing is owed. `call.rs` refuses to hold a wordless
            // transcript, so this is belt and braces.
            return true;
        }

        // Who holds this thread, before anything is written. Only the Lucidos
        // Agent is reachable by voice (ADR 0165), and every other answer is a
        // refusal, an unreadable row included.
        //
        // Read-then-act, not transactional. A flip landing between this read
        // and the emit below is not caught, and closing that would mean
        // locking the row across `EventBus::emit`. It is deliberately the same
        // strength as `validate_thread_continuity`, which guards the typed
        // path the same way: stronger here would still leave typing open.
        let doer = doer_for(self.engine.pool(), thread_id).await;
        if doer != ThreadDoer::LucidosAgent {
            log!(
                "[Voice] Not waking thread {}: a call reaches the Lucidos Agent only, and this thread reads {:?}",
                thread_id,
                doer
            );
            return false;
        }

        // The device the call is on, so a spoken message is attributed exactly
        // as a typed one from the same phone.
        let device_id = match &actor {
            Some(MessageOrigin::Device { device_id, .. }) => Some(device_id.clone()),
            _ => None,
        };

        // Record before the turn is queued. At pool-max the turn waits for a
        // slot, and the utterance belongs in the thread either way.
        //
        // This refuses while a question is open, and the turn then answers the
        // question instead of starting a new exchange. That is the behaviour a
        // typed answer already gets, and reading the question aloud is phase 6.
        let pre_emitted = self
            .engine
            .pre_emit_chat_message_received(
                Some(thread_id),
                true,
                ActorMode::Human,
                None,
                &message,
                None,
                device_id.as_deref(),
                None,
                None,
                None,
                actor.clone(),
                Some(session_id),
            )
            .await;

        let engine = self.engine.clone();
        let handle = tokio::spawn(async move {
            // A spoken turn is user-initiated work. So it takes a prioritized
            // slot from the one capacity pool (ADR 0008), exactly as a send from
            // the composer does. Released when this task ends.
            let _user_slot = engine
                .thread_queue
                .acquire_user_slot(
                    Some(thread_id),
                    crate::engine::thread_queue::truncate_summary(&message),
                )
                .await;
            let result = engine
                .process_message_with_steps(
                    &message,
                    None,
                    None,
                    None,
                    None,
                    None,
                    device_id.as_deref(),
                    None,
                    None,
                    Some(thread_id),
                    None,
                    None,
                    None,
                    None,
                    None,
                    ActorMode::Human,
                    None,
                    None,
                    pre_emitted,
                    None,
                    actor,
                    // Belt and braces with the pre-emit above. A pre-emit whose
                    // own write failed hands this path the message, and it must
                    // still land marked.
                    Some(session_id),
                    crate::engine::FollowUpUrgency::Normal,
                )
                .await;
            match result {
                // Drain the orphan chain, as every other spawn site does. An
                // utterance that lands after the loop exits but before cleanup
                // is collected and then dropped, and the caller is never
                // answered. Voice makes that the COMMON case rather than a
                // rare one: talking over a reply is how people talk.
                Ok(res) if !res.orphaned_injections.is_empty() => {
                    crate::api::chat::process_orphan_chain(
                        engine.clone(),
                        thread_id,
                        res.orphaned_injections,
                    )
                    .await;
                }
                Ok(_) => {}
                // No terminator here. The turn settles its own exchange,
                // anchored to its originating event, and an unanchored copy is
                // what the idempotency gate cannot match.
                Err(e) => log!("[Voice] The turn a spoken message started failed: {}", e),
            }
        });
        // The same panic monitoring every other spawn site uses. Without it a
        // panicking turn leaves the thread stuck `running`, with the caller
        // waiting for an answer that is never coming.
        drop(LucidosEngine::monitor_cc_task(
            self.engine.clone(),
            thread_id,
            handle,
        ));
        true
    }

    async fn overheard(&self, thread_id: Uuid, spoken: &str) {
        let spoken = spoken.trim();
        if spoken.is_empty() {
            return;
        }
        let injected = self.engine.inject_into_live_turn(
            thread_id,
            crate::engine::InjectedPrompt {
                text: format!("{}\n\n{}", SAID_ALOUD, spoken),
                event_id: None,
                mode: ActorMode::Agent,
                spawning_event_id: None,
                images: None,
                origin: None,
                kind: crate::engine::InjectedPromptKind::SpokenAside,
            },
        );
        if injected {
            log!("[Voice] The running turn was told what was said aloud");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{setup_test_db, teardown_test_db};

    /// A `thread_summaries` row and nothing else, which is all `doer_for` reads.
    ///
    /// The raw insert stays inside this `#[cfg(test)]` module deliberately.
    /// `announced_surfaces` reads `test_support.rs` as production, and refuses a
    /// shared seeder there. A raw write reachable from any module is one that
    /// can skip a table's announcement.
    async fn a_thread(pool: &sqlx::PgPool, source: &str, state: &str) -> Uuid {
        let thread_id = Uuid::new_v4();
        sqlx::query("INSERT INTO thread_summaries (thread_id, source, state) VALUES ($1, $2, $3)")
            .bind(thread_id)
            .bind(source)
            .bind(state)
            .execute(pool)
            .await
            .expect("create the thread");
        thread_id
    }

    /// The row is the whole answer, and it is right before the first send.
    ///
    /// A draft says who holds it as soon as the destination is picked, which
    /// is the case a call actually hits: the control is pressed on a compose
    /// view with nothing typed into it yet.
    #[tokio::test]
    async fn a_threads_source_names_who_holds_it() {
        let (pool, db_name) = setup_test_db().await;

        for state in ["composing", "active"] {
            assert_eq!(
                doer_for(&pool, a_thread(&pool, "chat", state).await).await,
                ThreadDoer::LucidosAgent
            );
            assert_eq!(
                doer_for(&pool, a_thread(&pool, "claude_code", state).await).await,
                ThreadDoer::CodingAgent
            );
        }

        teardown_test_db(&db_name).await;
    }

    /// A trigger thread runs the Lucidos Agent, so a call reaches it.
    ///
    /// The rule is which agent holds the thread, never how the thread began.
    /// Reading this as "not a chat thread" would take voice off every thread a
    /// trigger ever started.
    #[tokio::test]
    async fn a_trigger_thread_is_the_lucidos_agents() {
        let (pool, db_name) = setup_test_db().await;
        let thread_id = a_thread(&pool, "trigger", "active").await;

        assert_eq!(doer_for(&pool, thread_id).await, ThreadDoer::LucidosAgent);

        teardown_test_db(&db_name).await;
    }

    /// A thread with no row is told apart from one a coding agent holds.
    ///
    /// Both refuse a call, but they are different answers to the caller: one
    /// is a 404 and the other names the destination to switch.
    #[tokio::test]
    async fn a_missing_thread_is_its_own_answer() {
        let (pool, db_name) = setup_test_db().await;

        assert_eq!(
            doer_for(&pool, Uuid::new_v4()).await,
            ThreadDoer::NoSuchThread
        );

        teardown_test_db(&db_name).await;
    }
}
