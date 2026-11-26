use crate::engine::chat::process::ChatProcessExt;
use uuid::Uuid;

pub(crate) use crate::api::thread_reach::FollowUpReach;
use crate::engine::thread_lifecycle::ThreadStatus;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChildFollowUpError {
    UnknownChild(Uuid),
    NotYourChild(Uuid),
    ChildDiscarded(Uuid),
    ChildIsDraft(Uuid),
    SelfTarget(Uuid),
    NoCaller,
    CrossWorkspaceUnsupported,
    Internal(String),
}

impl ChildFollowUpError {
    pub fn status_code(&self) -> u16 {
        match self {
            Self::UnknownChild(_) => 404,
            Self::NotYourChild(_) => 403,
            Self::ChildDiscarded(_) | Self::ChildIsDraft(_) => 409,
            Self::SelfTarget(_) => 400,
            Self::NoCaller => 403,
            Self::CrossWorkspaceUnsupported => 400,
            Self::Internal(_) => 500,
        }
    }
}

impl std::fmt::Display for ChildFollowUpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownChild(id) => write!(f, "No thread {id} exists in this workspace."),
            Self::NotYourChild(id) => write!(
                f,
                "Thread {id} is not one of your child threads. A thread can only \
                 follow up on children it spawned itself."
            ),
            Self::ChildDiscarded(id) => {
                write!(f, "Child thread {id} was discarded and cannot be reached.")
            }
            Self::ChildIsDraft(id) => write!(
                f,
                "Thread {id} is an unsent draft the user is still writing, so it cannot \
                 be followed up."
            ),
            Self::SelfTarget(_) => write!(
                f,
                "A thread cannot follow up on itself. Address one of its child threads."
            ),
            Self::NoCaller => write!(
                f,
                "A child follow-up needs a caller thread, and this request has none."
            ),
            Self::CrossWorkspaceUnsupported => write!(
                f,
                "Cross-workspace follow-up is not supported: a cross-workspace \
                 thread is always top-level, so it has no children to follow up on."
            ),
            Self::Internal(msg) => write!(f, "Child follow-up failed: {msg}"),
        }
    }
}

impl std::error::Error for ChildFollowUpError {}

use crate::engine::FollowUpUrgency;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FollowUpDelivery {
    Running,
    Interrupted,
    WaitingForUserAnswer,
    Held,
    Revived,
}

impl FollowUpDelivery {
    fn from_status(status: ThreadStatus, urgency: FollowUpUrgency) -> Self {
        match status {
            ThreadStatus::Running if urgency.is_urgent() => Self::Interrupted,
            ThreadStatus::Running => Self::Running,
            ThreadStatus::WaitingForUserAnswer => Self::WaitingForUserAnswer,
            _ => Self::Revived,
        }
    }

    pub(crate) fn wants_pre_emit(&self) -> bool {
        matches!(self, Self::Revived)
    }

    pub(crate) fn effective_urgency(&self) -> FollowUpUrgency {
        match self {
            Self::Interrupted => FollowUpUrgency::Urgent,
            Self::Running | Self::WaitingForUserAnswer | Self::Held | Self::Revived => {
                FollowUpUrgency::Normal
            }
        }
    }

    pub fn describe(&self) -> &'static str {
        match self {
            Self::Running => {
                "The child was mid-turn, so this queues behind its current work or steers it."
            }
            Self::Interrupted => {
                "The child was mid-turn and this was urgent, so its current turn is being \
                 stopped and it will act on this next."
            }
            Self::WaitingForUserAnswer => {
                "The child is parked on a question or a permission card. It will not read \
                 this until a human answers."
            }
            Self::Held => {
                "The child is waiting on a human, so this is held. It is delivered once a human \
                 replies."
            }
            Self::Revived => "The child was not working, so a fresh turn starts now.",
        }
    }
}

#[derive(Debug, Clone)]
pub struct FollowUpAck {
    pub child_thread_id: Uuid,
    pub reach: FollowUpReach,
    pub child_title: String,
    pub delivered_to: FollowUpDelivery,
}

type ChildRowTuple = (
    Option<Uuid>,
    Option<String>,
    Option<String>,
    String,
    Option<String>,
    Option<String>,
);

#[derive(Debug)]
pub(crate) struct ChildRow {
    pub(crate) parent_thread_id: Option<Uuid>,
    pub(crate) source: Option<String>,
    pub(crate) state: Option<String>,
    pub(crate) status: ThreadStatus,
    pub(crate) title: Option<String>,
    pub(crate) first_message: Option<String>,
}

impl ChildRow {
    pub(crate) fn uses_coding_agent(&self) -> bool {
        self.source.as_deref() == Some("claude_code")
    }

    pub(crate) fn label(&self) -> String {
        child_label(self.title.clone(), self.first_message.as_deref())
    }
}

pub(crate) fn child_label(title: Option<String>, first_message: Option<&str>) -> String {
    title
        .or_else(|| first_message.map(|m| m.chars().take(80).collect()))
        .unwrap_or_else(|| "untitled child thread".into())
}

#[easy_ext::ext(ChildFollowUpExt)]
pub(crate) impl crate::engine::LucidosEngine {
    async fn authorize_child_follow_up(
        pool: &sqlx::PgPool,
        caller_thread_id: Option<Uuid>,
        child_thread_id: Uuid,
        caller_workspace: Option<&str>,
        urgency: FollowUpUrgency,
    ) -> Result<(ChildRow, FollowUpAck), ChildFollowUpError> {
        if caller_workspace.is_some() {
            crate::log_legacy!(
                "[ChildFollowUp] Refused: cross-workspace caller for child {}",
                child_thread_id
            );
            return Err(ChildFollowUpError::CrossWorkspaceUnsupported);
        }
        let Some(caller_thread_id) = caller_thread_id else {
            crate::log_legacy!(
                "[ChildFollowUp] Refused: no caller thread for child {}",
                child_thread_id
            );
            return Err(ChildFollowUpError::NoCaller);
        };
        if caller_thread_id == child_thread_id {
            crate::log_legacy!(
                "[ChildFollowUp] Refused: thread {} addressed itself",
                caller_thread_id
            );
            return Err(ChildFollowUpError::SelfTarget(child_thread_id));
        }

        let row: Option<ChildRowTuple> = sqlx::query_as(
            "SELECT parent_thread_id, source, state, status, title, first_message \
                 FROM thread_summaries WHERE thread_id = $1",
        )
        .bind(child_thread_id)
        .fetch_optional(pool)
        .await
        .map_err(|e| {
            crate::log_legacy!(
                "[ChildFollowUp] Failed to read child {} for caller {}: {}",
                child_thread_id,
                caller_thread_id,
                e
            );
            ChildFollowUpError::Internal(e.to_string())
        })?;

        let Some((parent_thread_id, source, state, status, title, first_message)) = row else {
            crate::log_legacy!(
                "[ChildFollowUp] Refused: caller {} addressed unknown thread {}",
                caller_thread_id,
                child_thread_id
            );
            return Err(ChildFollowUpError::UnknownChild(child_thread_id));
        };
        let row = ChildRow {
            parent_thread_id,
            source,
            state,
            status: ThreadStatus::parse(&status),
            title,
            first_message,
        };

        let reach =
            crate::api::thread_reach::follow_up_reach(pool, caller_thread_id, row.parent_thread_id)
                .await
                .map_err(|e| ChildFollowUpError::Internal(e.to_string()))?;
        let Some(reach) = reach else {
            crate::log_legacy!(
                "[ChildFollowUp] Refused: thread {} is not a child of caller {}",
                child_thread_id,
                caller_thread_id
            );
            return Err(ChildFollowUpError::NotYourChild(child_thread_id));
        };
        if row.state.as_deref() == Some("discarded") {
            crate::log_legacy!(
                "[ChildFollowUp] Refused: child {} of caller {} is discarded",
                child_thread_id,
                caller_thread_id
            );
            return Err(ChildFollowUpError::ChildDiscarded(child_thread_id));
        }
        if row.state.as_deref() == Some("composing") {
            crate::log_legacy!(
                "[ChildFollowUp] Refused: thread {} is a draft",
                child_thread_id
            );
            return Err(ChildFollowUpError::ChildIsDraft(child_thread_id));
        }

        let held = row.uses_coding_agent()
            && super::held_messages::message_is_held(
                pool,
                child_thread_id,
                crate::engine::thread_events::ActorMode::Agent,
                None,
            )
            .await;
        let ack = FollowUpAck {
            child_thread_id,
            reach,
            child_title: row.label(),
            delivered_to: if held {
                FollowUpDelivery::Held
            } else {
                FollowUpDelivery::from_status(row.status, urgency)
            },
        };
        Ok((row, ack))
    }
}

pub(crate) fn build_follow_up_message(
    workspace_path: &std::path::Path,
    text: &str,
    images: Option<&[crate::engine::types::ChatImage]>,
    origin: &crate::engine::thread_events::MessageOrigin,
) -> crate::engine::thread_events::ThreadEvent {
    crate::engine::chat::make_message_received(
        workspace_path,
        text,
        images,
        None,
        None,
        None,
        None,
        crate::engine::thread_events::ActorMode::Agent,
        None,
        None,
        None,
        Some(origin.clone()),
        None,
    )
}

pub(crate) fn parent_thread_link(
    parent_thread_id: Uuid,
    parent_title: Option<String>,
    spawning_event_id: Option<Uuid>,
) -> crate::engine::thread_events::MessageOrigin {
    crate::engine::thread_events::MessageOrigin::ThreadLink {
        thread_id: parent_thread_id,
        title: parent_title,
        spawning_event_id,
        mode: crate::engine::thread_events::ActorMode::Agent,
        direction: crate::engine::thread_events::ThreadDirection::Parent,
    }
}

#[easy_ext::ext(ChildFollowUpDispatchExt)]
pub(crate) impl crate::engine::LucidosEngine {
    /// INVARIANT: Deliver a follow-up from a parent thread to one of its own children.
    ///
    /// Returns an **ack**, not the child's turn. The child's outcome arrives
    /// later, the way every child outcome does: as a `ChildThreadCompleted`
    /// card on the parent plus a re-entry.
    ///
    /// ## Why it must not await the child's turn
    ///
    /// Three of the delivery modes do not return promptly: a chat child's whole
    /// agentic loop, a coding-agent `--resume` session, and a Codex live turn
    /// (which blocks up to `REDIRECT_INTERRUPT_MAX_WAIT` before it even reaches
    /// the send). This runs inside the **parent's own agentic loop**, so
    /// awaiting would park the parent for the child's entire run, and while it
    /// is parked the child can complete, firing a `ChildThreadCompleted` on the
    /// parent and injecting a re-entry into the parent's still-running turn, ahead
    /// of the tool result the parent is waiting for.
    ///
    /// That is exactly the asymmetry with the mirror image,
    /// `notify_parent_of_child_completion`: it awaits because it runs on the
    /// ParentCallback listener task. The asynchrony contract here is
    /// `chat_submit`'s instead: emit inline, spawn the turn, monitor the task,
    /// return the ack.
    ///
    /// ## The pre-emit rule
    ///
    /// Human inputs accept durably and record during execution. This Agent
    /// follow-up instead announces its own message before dispatch, including
    /// for a coding-agent child. Its routing exclusions are handled explicitly:
    ///
    /// - The **Human** exclusion is about routing (a Human message on a
    ///   question-parked thread becomes `UserQuestionAnswered`). An Agent
    ///   follow-up never takes that route, so the exclusion does not apply.
    /// - The **coding-agent** exclusion is a real ordering rule. The
    ///   coding-agent lane blocks on a required interrupt reaching a
    ///   turn boundary BEFORE emitting the follow-up's `MessageReceived`, so
    ///   the interrupted turn's terminal sequences first and coding-agent
    ///   exchanges group correctly. Pre-emitting ahead of that would reorder
    ///   the child's timeline.
    ///
    /// The two coincide: pre-emit exactly when the sampled status is **outside
    /// the in-flight set**. That is exactly when a revive re-increment is owed,
    /// and exactly when there is no live turn to reorder against. Awaiting that
    /// emit is what makes the ordering invariant true, because the revive
    /// re-increment and the `parent_callback_pending` set both run in its
    /// projection transaction, so the parent cannot finish its own turn
    /// believing it has no children in flight.
    /// ## Why the return type is boxed rather than an `async fn`
    ///
    /// This spawns a turn, that turn can run an agentic loop, that loop reaches
    /// `execute_tool`, and one of its arms calls back here. With an `async fn`
    /// the compiler would have to prove `Send` for an opaque future whose
    /// `Send`-ness depends on itself, and it reports the dead end as
    /// `cycle detected when computing type of opaque`. Naming the return type
    /// states the bound instead of deriving it, which cuts the cycle at this
    /// one edge. `run_thread` avoids the same recursion a different way, by
    /// handing its spawn to the Thread Queue's trait-object executor.
    #[allow(clippy::too_many_arguments)]
    fn follow_up_child_thread<'a>(
        self: &'a std::sync::Arc<Self>,
        caller_thread_id: Option<Uuid>,
        child_thread_id: Uuid,
        text: &'a str,
        images: Option<&'a [crate::engine::types::ChatImage]>,
        spawning_event_id: Option<Uuid>,
        caller_workspace: Option<&'a str>,
        urgency: FollowUpUrgency,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<FollowUpAck, ChildFollowUpError>> + Send + 'a>,
    > {
        Box::pin(async move {
            let (row, ack) = Self::authorize_child_follow_up(
                self.pool(),
                caller_thread_id,
                child_thread_id,
                caller_workspace,
                urgency,
            )
            .await?;
            let caller_thread_id = caller_thread_id.expect("the ladder refuses a missing caller");
            let use_coding_agent = row.uses_coding_agent();

            let parent_title: Option<String> = match sqlx::query_scalar::<_, String>(
                "SELECT COALESCE(title, first_message) FROM thread_summaries WHERE thread_id = $1",
            )
            .bind(caller_thread_id)
            .fetch_optional(self.pool())
            .await
            {
                Ok(t) => t.map(|t| t.chars().take(80).collect()),
                Err(e) => {
                    crate::log_legacy!(
                        "[ChildFollowUp] Failed to read parent {}'s title: {}; \
                         the child's message-route panel will resolve it live",
                        caller_thread_id,
                        e
                    );
                    None
                }
            };
            let origin = parent_thread_link(caller_thread_id, parent_title, spawning_event_id);

            let pre_emitted_origin = self
                .pre_emit_follow_up(child_thread_id, ack.delivered_to, text, images, &origin)
                .await?;

            crate::log_legacy!(
                "[ChildFollowUp] Parent {} follows up on child {} ({:?}, coding_agent={})",
                caller_thread_id,
                child_thread_id,
                ack.delivered_to,
                use_coding_agent
            );

            // INVARIANT: Spawn, do not await. Everything the engine can derive it derives:
            // `use_coding_agent` from the child's own row, and no mode / repo /
            // model from the caller at all.
            //
            // `parent_thread_id = None` and `spawning_event_id = None` are the two
            // load-bearing arguments. Together they keep the emitted
            // `MessageReceived` payload free of spawn linkage, which is what routes
            // the projection down the revive branch instead of the spawn branch:
            // the spawn branch would add +1 to the parent's `total_children_count`
            // for a child that already exists. The parent's identity and the
            // originating tool call live in the `origin` instead, which is where
            // the message-route panel already reads them.
            let engine = self.clone();
            let message = text.to_string();
            let images = images.map(|i| i.to_vec());
            let spawn_origin = origin.clone();
            // INVARIANT: From the ack, never from the caller's raw flag: see
            // `FollowUpDelivery::effective_urgency`. A question-parked child
            // must not be preempted, and the ack has already promised it will
            // not be.
            let urgency = ack.delivered_to.effective_urgency();
            // INVARIANT: Type-erased before it reaches `tokio::spawn`, to break an auto-trait
            // inference cycle: this spawns a turn, that turn can run an agentic
            // loop, that loop reaches `execute_tool`, and one of its arms calls
            // back into this function. Inferring `Send` for the spawned future
            // therefore depends on inferring it for itself, which rustc reports as
            // `cycle detected when computing type of opaque`. Boxing states the
            // bound instead of deriving it. Same reason `notify_parent_if_child`
            // boxes its own recursive emit.
            let turn: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> =
                Box::pin(async move {
                    let result = engine
                        .process_message_with_steps(
                            &message,
                            None,
                            None,
                            None,
                            None,
                            None,
                            images.as_deref(),
                            None,
                            Some(use_coding_agent),
                            None,
                            Some(child_thread_id),
                            None,
                            None,
                            None,
                            None,
                            None,
                            None,
                            crate::engine::thread_events::ActorMode::Agent,
                            None,
                            None,
                            pre_emitted_origin,
                            None,
                            Some(spawn_origin),
                            None,
                            urgency,
                            None,
                        )
                        .await;
                    match result {
                        // INVARIANT: Drain the orphan chain, exactly as the other two spawn
                        // sites do (`api/chat.rs` for a chat send,
                        // `thread_queue/executor.rs` for a queued turn). This
                        // site did not, so a message injected into the turn this
                        // one started was collected by `drain_turn_orphans` and
                        // then dropped on the floor: the child went idle having
                        // never read it. Harmless-looking until the urgent lane
                        // made it load-bearing, because ending a Lucidos Agent
                        // turn is precisely how an urgent follow-up gets read.
                        Ok(res) if !res.orphaned_injections.is_empty() => {
                            crate::api::chat::process_orphan_chain(
                                engine.clone(),
                                child_thread_id,
                                res.orphaned_injections,
                            )
                            .await;
                        }
                        Ok(_) => {}
                        // INVARIANT: No terminator here: the turn settles its own
                        // exchange, anchored. An unanchored copy is what the
                        // idempotency gate cannot match, so it double-fired.
                        Err(e) => crate::log_legacy!(
                            "[ChildFollowUp] Follow-up turn failed on child {}: {}",
                            child_thread_id,
                            e
                        ),
                    }
                });
            let handle = tokio::spawn(turn);
            // INVARIANT: Same panic monitoring `chat_submit` uses: a panicking child turn
            // emits ResponseFailed + SessionEnded instead of leaving the child
            // stuck in `running` and the parent waiting forever. Fire and forget,
            // exactly as there: dropping the JoinHandle detaches the watcher.
            drop(Self::monitor_cc_task(self.clone(), child_thread_id, handle));

            Ok(ack)
        })
    }

    /// INVARIANT: Persist the follow-up's `MessageReceived` inline, iff the pre-emit rule
    /// says to. Returns the `PreEmittedOrigin` the spawned turn must carry so
    /// neither fast path emits the event a second time.
    ///
    /// `PreEmittedOrigin::Message`, never `EngineReentry`: a redirect from the
    /// parent is a real message the child should acknowledge with its own
    /// `CodingAgentPromptSent` / `PromptInjected`, not a silent engine
    /// re-entry like a child's completion re-opening its parent.
    async fn pre_emit_follow_up(
        self: &std::sync::Arc<Self>,
        child_thread_id: Uuid,
        delivered_to: FollowUpDelivery,
        text: &str,
        images: Option<&[crate::engine::types::ChatImage]>,
        origin: &crate::engine::thread_events::MessageOrigin,
    ) -> Result<Option<crate::engine::chat::PreEmittedOrigin>, ChildFollowUpError> {
        if !delivered_to.wants_pre_emit() {
            return Ok(None);
        }
        // INVARIANT: The child can start running between the row read and this emit. The
        // cost of losing that race is bounded (one mis-sequenced exchange
        // boundary in the child's timeline, never a lost message or a wrong
        // counter), but a live coding-agent session is the case where the
        // ordering actually matters, so re-check under the lock and let the
        // coding-agent lane own the emit if one appeared.
        let live_session = {
            let sessions = self.agent_sessions.lock().await;
            sessions
                .get(&child_thread_id)
                .map(|s| s.is_live())
                .unwrap_or(false)
        };
        if live_session {
            crate::log_legacy!(
                "[ChildFollowUp] Child {} came alive between the row read and the \
                 pre-emit; letting the coding-agent lane sequence the message",
                child_thread_id
            );
            return Ok(None);
        }

        let event = build_follow_up_message(&self.workspace_path, text, images, origin);
        let emitted = self
            .event_bus
            .emit(crate::engine::event_bus::BusEvent::Thread {
                thread_id: child_thread_id,
                event,
                meta: crate::engine::thread_events::EventMeta::NONE,
            })
            .await
            .map_err(|e| {
                crate::log_legacy!(
                    "[ChildFollowUp] Failed to pre-emit the follow-up on child {}: {}",
                    child_thread_id,
                    e
                );
                ChildFollowUpError::Internal(e.to_string())
            })?;
        Ok(emitted.map(|r| crate::engine::chat::PreEmittedOrigin::Message(r.event_id)))
    }
}

#[cfg(test)]
#[path = "child_follow_up_tests.rs"]
mod tests;
