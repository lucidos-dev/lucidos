use std::sync::Arc;

use async_trait::async_trait;
use uuid::Uuid;

use crate::engine::thread_events::{ActorMode, MessageOrigin};
use crate::engine::thread_lifecycle::ThreadType;
use crate::engine::{LucidosEngine, PreEmittedOrigin, RestOfSentence};

/// INVARIANT: Which agent holds a thread, as far as a call is concerned.
///
/// Four answers rather than a boolean, because each caller owes a different
/// one of them a different response. A lookup that could not run is UNKNOWN
/// and never a "yes": starting the wrong agent writes a chat-channel message
/// into a coding-agent thread, where refusing costs the caller one repeat.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadDoer {
    LucidosAgent,
    CodingAgent,
    NoSuchThread,
    Unknown,
}

/// INVARIANT: Ask a thread's projection row which agent holds it.
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
            log_legacy!(
                "[Voice] Could not read who holds thread {}: {}",
                thread_id,
                e
            );
            ThreadDoer::Unknown
        }
    }
}

/// INVARIANT: What a delegated utterance does to the thread it was spoken on.
#[async_trait]
pub trait TurnStarter: Send + Sync {
    async fn wake(
        &self,
        thread_id: Uuid,
        delegation: Uuid,
        transcript: &str,
        actor: Option<MessageOrigin>,
    ) -> bool;

    /// INVARIANT: The talker said this out loud. Offer it to a turn already running.
    ///
    /// An offer, not a wake. With no turn running there is nothing to inject
    /// into, and the reply is dropped here: `SpokenReplyGenerated` is already
    /// in the thread, so the next round reads it from history either way.
    ///
    /// What this buys is the round LEARNING it mid-flight. Otherwise the doer
    /// finishes without knowing what the caller was told in its name.
    async fn overheard(&self, thread_id: Uuid, spoken: &str);

    /// INVARIANT: The caller's sentence went on after the talker asked about it. Offer
    /// the rest to the turn that took the ask.
    ///
    /// `asked_turn` is the prompt that turn was given: the ask's
    /// `WorkDelegated` row, or the anchor of the turn the ask went into.
    /// Another turn running on the thread is never given the words.
    ///
    /// An offer, like [`Self::overheard`]. The words already have their own
    /// row, so a turn not running yet reads them from history instead.
    async fn continued(&self, thread_id: Uuid, asked_turn: Uuid, words: &str);
}

/// INVARIANT: How a spoken aside is framed for the running turn.
///
/// `Standalone` injections are pushed raw, so the framing is written here
/// rather than by `framed_injected_prompt`. It says nothing is being asked. A
/// turn reading this as an instruction would drop the work it is doing.
const SAID_ALOUD: &str = "[SAID ALOUD] The caller heard this from you just now, \
                          on the call. It is what they already know. Carry on \
                          with your work; nothing here is a new request.";

/// INVARIANT: How the rest of an asked sentence is framed for the turn it started.
///
/// The turn may already have read these words from the thread, so the framing
/// says they are the same words rather than a second request.
const SENTENCE_WENT_ON: &str = "[SENTENCE WENT ON] The caller kept talking after \
                                you were asked. These words finish the request \
                                you are working on. You may already have read \
                                them in the thread: they are the same words, \
                                not a new request.";

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
        delegation: Uuid,
        transcript: &str,
        actor: Option<MessageOrigin>,
    ) -> bool {
        let message = transcript.trim().to_string();
        if message.is_empty() {
            return true;
        }

        let doer = doer_for(self.engine.pool(), thread_id).await;
        if doer != ThreadDoer::LucidosAgent {
            log_legacy!(
                "[Voice] Not waking thread {}: a call reaches the Lucidos Agent only, and this thread reads {:?}",
                thread_id,
                doer
            );
            return false;
        }

        let device_id = match &actor {
            Some(MessageOrigin::Device { device_id, .. }) => Some(device_id.clone()),
            _ => None,
        };

        let pre_emitted = Some(PreEmittedOrigin::Message(delegation));

        let engine = self.engine.clone();
        let handle = tokio::spawn(async move {
            let _user_slot = engine
                .thread_queue
                .acquire_user_slot(
                    thread_id,
                    crate::engine::thread_queue::truncate_summary(&message),
                )
                .await;
            let result = engine
                .agent_host()
                .process_message_with_steps(
                    &engine,
                    &message,
                    None,
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
                    None,
                    ActorMode::Human,
                    None,
                    None,
                    pre_emitted,
                    None,
                    actor,
                    None,
                    crate::engine::FollowUpUrgency::Normal,
                    None,
                    None,
                )
                .await;
            match result {
                // INVARIANT: Drain the orphan chain, as every other spawn site does. An
                // utterance that lands after the loop exits but before cleanup
                // is collected and then dropped, and the caller is never
                // answered. Voice makes that the COMMON case rather than a
                // rare one: talking over a reply is how people talk.
                Ok(res) if !res.orphaned_injections.is_empty() => {
                    engine
                        .agent_host()
                        .process_orphan_chain(&engine, thread_id, res.orphaned_injections)
                        .await;
                }
                Ok(_) => {}
                // INVARIANT: No terminator here. The turn settles its own exchange,
                // anchored to its originating event, and an unanchored copy is
                // what the idempotency gate cannot match.
                Err(e) => log_legacy!("[Voice] The turn a spoken message started failed: {}", e),
            }
        });
        // INVARIANT: The same panic monitoring every other spawn site uses. Without it a
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
            log_legacy!("[Voice] The running turn was told what was said aloud");
        }
    }

    async fn continued(&self, thread_id: Uuid, asked_turn: Uuid, words: &str) {
        let words = words.trim();
        if words.is_empty() {
            return;
        }
        let offered = self.engine.inject_the_rest_of_a_sentence(
            thread_id,
            asked_turn,
            crate::engine::InjectedPrompt {
                text: format!("{}\n\n{}", SENTENCE_WENT_ON, words),
                event_id: Some(asked_turn),
                mode: ActorMode::Human,
                spawning_event_id: None,
                images: None,
                origin: None,
                kind: crate::engine::InjectedPromptKind::SpokenContinuation,
            },
        );
        log_legacy!(
            "[Voice] The rest of the asked sentence {}",
            match offered {
                RestOfSentence::Injected => "reached the turn that took the ask",
                RestOfSentence::Held => "waits on the live turn for its ask",
                RestOfSentence::LeftToHistory => "is left to the thread's history",
            }
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{setup_test_db, teardown_test_db};

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

    #[tokio::test]
    async fn a_trigger_thread_is_the_lucidos_agents() {
        let (pool, db_name) = setup_test_db().await;
        let thread_id = a_thread(&pool, "trigger", "active").await;

        assert_eq!(doer_for(&pool, thread_id).await, ThreadDoer::LucidosAgent);

        teardown_test_db(&db_name).await;
    }

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
