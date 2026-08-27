//! Parent to its own child: the one privileged cross-thread write.
//!
//! Child to parent already works end to end (ADR 0011): every child terminal
//! fires a typed `ChildThreadCompleted` on the parent and a re-entry that
//! resumes the parent's turn. This module is the other direction, and it is
//! deliberately not an any-to-any address space. A thread can address its own
//! **direct** children and nothing else: no sibling edge, no grandchild edge,
//! no cross-workspace edge.
//!
//! The caller never states the relationship. It is looked up from the child's
//! `thread_summaries` row and the caller identity is ambient, so on the
//! shipping surface (the in-process LLM tool, whose `thread_id` comes from
//! `execute_tool` and cannot be set by the model) the refusal ladder is a real
//! authorization boundary.
//!
//! ## Why a typed error here
//!
//! `.claude/rules/rust.md` says boxed trait objects unless a typed error is
//! consumed structurally. `ChildFollowUpError` is: `status_code()` is the HTTP
//! mapping `api/threads/follow_up.rs` and `api/threads/archive.rs` use rather
//! than re-deriving the taxonomy, and the LLM layer turns each variant into a
//! distinct, actionable tool-error string. The caller never merely formats it
//! into one message.
//!
//! ## The mirror image
//!
//! `notify_parent_of_child_completion` (`engine_impl/threads.rs`) is the same
//! cross-thread delivery with the direction flipped, and the two should stay
//! recognisably symmetric. One asymmetry is deliberate and load-bearing: that
//! one **awaits** its `process_message_with_steps` because it runs on the
//! ParentCallback listener task, and this one must NOT, because it runs inside
//! the parent's own agentic loop. See the delivery half for what that costs.

use uuid::Uuid;

use crate::engine::thread_lifecycle::ThreadStatus;

/// Why a child follow-up was refused. Every branch is a refusal: no branch
/// creates a thread, and no branch falls back to treating the call as a spawn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChildFollowUpError {
    /// No `thread_summaries` row for the target id.
    UnknownChild(Uuid),
    /// The target exists but its `parent_thread_id` is NULL or is some other
    /// thread. Deliberately one variant for both: the caller learns that the
    /// thread is not theirs and nothing about whose it is.
    NotYourChild(Uuid),
    /// The target was thrown away by the user.
    ChildDiscarded(Uuid),
    /// The caller addressed itself.
    SelfTarget(Uuid),
    /// No ambient caller identity, so there is no relationship to check.
    NoCaller,
    /// The request carried `caller_workspace`. Cross-workspace spawns require
    /// `relation = "top"` and therefore have `parent_thread_id = NULL` in the
    /// receiving workspace, so no cross-workspace caller has a child to follow
    /// up on. Refused explicitly rather than silently reinterpreted as a
    /// same-workspace call.
    CrossWorkspaceUnsupported,
    /// The row read or the delivery failed.
    Internal(String),
}

impl ChildFollowUpError {
    /// HTTP status for the `/threads/follow-up` and `/threads/archive` routes.
    /// Kept beside the taxonomy so the mapping cannot drift from it.
    pub fn status_code(&self) -> u16 {
        match self {
            Self::UnknownChild(_) => 404,
            Self::NotYourChild(_) => 403,
            Self::ChildDiscarded(_) => 409,
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

/// Whether the caller is willing to end the child's current turn to be read now.
///
/// A bare `bool` in the twenty-second argument slot of
/// `process_message_with_steps` would be unreadable at the call site (a lone
/// `false` among a column of `None`s) and trivially mis-passed, so the two
/// states are named. Default is `Normal`: a follow-up does not destroy work
/// unless the caller says it must.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FollowUpUrgency {
    /// Queue behind the child's current work. On Claude Code and the Lucidos
    /// Agent the message reaches the child at its next internal boundary, which
    /// a long tool call can push out by up to that tool's own timeout. That is
    /// the right trade for a steer: it never throws away an in-flight build.
    #[default]
    Normal,
    /// Preempt the child's in-flight work, accepting that its current turn ends
    /// as `ResponseCanceled { SupersededByFollowup }` and the follow-up runs as
    /// the next turn. For the messages that cannot wait for a tool timeout: a
    /// cancellation, a "stop, you are working from a wrong assumption".
    ///
    /// No-op on Codex, which always interrupts because its protocols cannot
    /// surface a queued message mid-turn at all. See
    /// `process_helpers::should_redirect_followup`.
    Urgent,
}

impl FollowUpUrgency {
    pub(crate) fn is_urgent(self) -> bool {
        matches!(self, Self::Urgent)
    }

    /// Parse the HTTP spelling, where serde has already rejected anything that
    /// is not a boolean. Absent means `Normal`, so every caller that predates
    /// the flag keeps its behaviour.
    pub fn from_flag(urgent: Option<bool>) -> Self {
        if urgent.unwrap_or(false) {
            Self::Urgent
        } else {
            Self::Normal
        }
    }

    /// Parse the LLM tool's raw argument, which has no schema enforcement
    /// behind it: the model hands us whatever JSON it emitted.
    ///
    /// Absent or `null` is `Normal`. A **present non-boolean is an error**, not
    /// a default. A model that writes `"urgent": "true"` or `"urgent": 1` means
    /// urgent, and coercing that to `Normal` fails in the one direction that is
    /// unrecoverable: the tool reports the follow-up sent, the caller believes
    /// the child was stopped, and the child keeps working. The HTTP route gets
    /// this for free (serde answers a non-boolean with a 422); this is the same
    /// answer for the path serde does not cover.
    pub fn from_tool_arg(urgent: Option<&serde_json::Value>) -> Result<Self, String> {
        match urgent {
            None | Some(serde_json::Value::Null) => Ok(Self::Normal),
            Some(serde_json::Value::Bool(b)) => Ok(Self::from_flag(Some(*b))),
            Some(other) => Err(format!(
                "`urgent` must be a boolean (true or false), got {other}. It is not \
                 defaulted, because reading a malformed urgent as 'not urgent' would \
                 report the follow-up as sent while the child kept working."
            )),
        }
    }
}

/// What the child was doing when the follow-up reached it, sampled from
/// `thread_summaries.status` before anything is delivered. Sampling in the pure
/// half is what makes it structurally impossible to derive from an await later,
/// which is the failure the delivery half is shaped to avoid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FollowUpDelivery {
    /// Mid-turn, and the caller let the turn finish. The message queues behind
    /// the current turn or steers it.
    Running,
    /// Mid-turn, and the caller marked the follow-up urgent. The child's turn is
    /// being interrupted and the follow-up runs as the next one.
    Interrupted,
    /// Parked on a question or a permission card. The message does **not**
    /// answer the question (that route requires `mode == Human`), so it sits in
    /// the channel until a human answers.
    WaitingForUserAnswer,
    /// Not in flight. A fresh turn starts now.
    Revived,
}

impl FollowUpDelivery {
    /// `urgency` only matters for a child that is actually mid-turn: there is
    /// nothing to preempt on an idle child, and a question-parked child is
    /// blocked on a human rather than on work, so urgency cannot unblock it.
    fn from_status(status: ThreadStatus, urgency: FollowUpUrgency) -> Self {
        match status {
            ThreadStatus::Running if urgency.is_urgent() => Self::Interrupted,
            ThreadStatus::Running => Self::Running,
            ThreadStatus::WaitingForUserAnswer => Self::WaitingForUserAnswer,
            _ => Self::Revived,
        }
    }

    /// The pre-emit rule, in one place: persist the follow-up's
    /// `MessageReceived` inline exactly when the child is **outside the
    /// in-flight set**.
    ///
    /// That is exactly when a revive re-increment is owed (so awaiting the emit
    /// is what makes the ordering invariant true), and exactly when there is no
    /// live turn to reorder against. When the child IS in flight the existing
    /// lane owns the emit, which is what happens today: the coding-agent lane
    /// deliberately waits for a Codex interrupt to reach a turn boundary before
    /// emitting, so pre-empting it would reorder the child's timeline.
    ///
    /// `WaitingForUserAnswer` counts as in flight here for the same reason it
    /// does everywhere else: the child never gave up its place on the parent's
    /// counter, so no re-increment is owed. `Interrupted` counts as in flight
    /// too, and most strictly of all: the redirect lane has to sequence the
    /// interrupted turn's `Canceled` terminal BEFORE the follow-up's
    /// `MessageReceived`, so pre-emitting here would invert the child's
    /// timeline.
    pub(crate) fn wants_pre_emit(&self) -> bool {
        matches!(self, Self::Revived)
    }

    /// The urgency the turn actually runs with, derived from the ack rather
    /// than from what the caller asked for.
    ///
    /// **This is the only thing that may hand `Urgent` to the turn**, and the
    /// reason is that the ack is a promise. `describe()` tells a
    /// question-parked child's caller "it will not read this until a human
    /// answers", so a raw `Urgent` reaching the turn would make the engine do
    /// the opposite of what it just said: a chat child parked on
    /// `ask_user_question` is blocked INSIDE a tool call, so its `ThreadHandle`
    /// is still registered and `is_in_flight()` is still true, and the preempt
    /// would cancel the turn and throw away the question the user was about to
    /// answer. Same on the coding-agent lane, where a session parked on
    /// AskUserQuestion also reads as in-flight.
    ///
    /// Deriving both from one sampled `FollowUpDelivery` makes the ack and the
    /// behaviour the same decision, so they cannot drift apart. `Interrupted`
    /// already means "mid-turn AND the caller asked for urgent", which is
    /// exactly the case that may preempt.
    pub(crate) fn effective_urgency(&self) -> FollowUpUrgency {
        match self {
            Self::Interrupted => FollowUpUrgency::Urgent,
            // Nothing to preempt (`Revived`), or preempting would break a
            // promise (`WaitingForUserAnswer`), or the caller did not ask
            // (`Running`).
            Self::Running | Self::WaitingForUserAnswer | Self::Revived => FollowUpUrgency::Normal,
        }
    }

    /// One sentence for the LLM's tool result, so the model knows whether the
    /// child is working on this now or waiting on a human first.
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
            Self::Revived => "The child was not working, so a fresh turn starts now.",
        }
    }
}

/// What the caller gets back the moment the message is on the child's timeline.
/// Never the child's result: the child's turn is not awaited (see the delivery
/// half), and its outcome arrives later as an ordinary `ChildThreadCompleted`.
#[derive(Debug, Clone)]
pub struct FollowUpAck {
    pub child_thread_id: Uuid,
    /// The child's human-meaningful handle. The tool's success text names the
    /// child by this and never by uuid, so the model's prose stays uuid-free.
    pub child_title: String,
    pub delivered_to: FollowUpDelivery,
}

/// The child's `thread_summaries` row as it comes off the wire:
/// `(parent_thread_id, source, state, status, title, first_message)`. Parsed
/// straight into [`ChildRow`], which is what everything downstream reads.
type ChildRowTuple = (
    Option<Uuid>,
    Option<String>,
    Option<String>,
    String,
    Option<String>,
    Option<String>,
);

/// The child's row, read once and reused by the ladder, the delivery sample and
/// the derived routing.
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
    /// Coding-agent-ness is **derived**, never asserted by the caller. A
    /// mis-derived flag sends a coding-agent child down the Lucidos Agent's
    /// loop, whose `ResponseGenerated` terminal matches neither
    /// `should_callback` nor `should_decrement` for `is_coding_agent = true`,
    /// so the parent is never woken and its counter never comes down. Silent in
    /// both dimensions, which is why the caller does not get to state it.
    pub(crate) fn uses_coding_agent(&self) -> bool {
        self.source.as_deref() == Some("claude_code")
    }

    /// Title, falling back to the opening of the spawn prompt, falling back to
    /// a generic label. Mirrors the label the fan-in puts on a completion card.
    pub(crate) fn label(&self) -> String {
        self.title
            .clone()
            .or_else(|| {
                self.first_message
                    .as_ref()
                    .map(|m| m.chars().take(80).collect())
            })
            .unwrap_or_else(|| "untitled child thread".into())
    }
}

impl crate::engine::LucidosEngine {
    /// Load the child's row and run the refusal ladder. Pure: reads one row,
    /// writes nothing, delivers nothing.
    ///
    /// Ladder order matters, and it is the order of the table in D2 of the
    /// plan: a self-target is a self-target even if the caller has no children,
    /// and a discarded child is reported as discarded rather than as unknown.
    ///
    /// Takes the pool rather than `&self` for the same reason
    /// `check_thread_recursion_guard` does: the whole check is one query and a
    /// ladder, so it is directly testable without standing up an engine.
    pub(crate) async fn authorize_child_follow_up(
        pool: &sqlx::PgPool,
        caller_thread_id: Option<Uuid>,
        child_thread_id: Uuid,
        caller_workspace: Option<&str>,
        urgency: FollowUpUrgency,
    ) -> Result<(ChildRow, FollowUpAck), ChildFollowUpError> {
        if caller_workspace.is_some() {
            crate::log!(
                "[ChildFollowUp] Refused: cross-workspace caller for child {}",
                child_thread_id
            );
            return Err(ChildFollowUpError::CrossWorkspaceUnsupported);
        }
        let Some(caller_thread_id) = caller_thread_id else {
            crate::log!(
                "[ChildFollowUp] Refused: no caller thread for child {}",
                child_thread_id
            );
            return Err(ChildFollowUpError::NoCaller);
        };
        if caller_thread_id == child_thread_id {
            crate::log!(
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
            crate::log!(
                "[ChildFollowUp] Failed to read child {} for caller {}: {}",
                child_thread_id,
                caller_thread_id,
                e
            );
            ChildFollowUpError::Internal(e.to_string())
        })?;

        let Some((parent_thread_id, source, state, status, title, first_message)) = row else {
            crate::log!(
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

        if row.parent_thread_id != Some(caller_thread_id) {
            crate::log!(
                "[ChildFollowUp] Refused: thread {} is not a child of caller {}",
                child_thread_id,
                caller_thread_id
            );
            return Err(ChildFollowUpError::NotYourChild(child_thread_id));
        }
        if row.state.as_deref() == Some("discarded") {
            crate::log!(
                "[ChildFollowUp] Refused: child {} of caller {} is discarded",
                child_thread_id,
                caller_thread_id
            );
            return Err(ChildFollowUpError::ChildDiscarded(child_thread_id));
        }

        let ack = FollowUpAck {
            child_thread_id,
            child_title: row.label(),
            delivered_to: FollowUpDelivery::from_status(row.status, urgency),
        };
        Ok((row, ack))
    }
}

/// The `MessageReceived` a follow-up puts on the child's timeline.
///
/// `parent_thread_id = None` and `spawning_event_id = None` are the two
/// load-bearing arguments, which is why this is its own function rather than an
/// inline call. Together they keep the payload free of spawn linkage, and that
/// is what routes the projection down the **revive** branch instead of the
/// spawn branch. The spawn branch would add `+1` to the parent's
/// `total_children_count` for a child that already exists, so the parent's
/// drawer would grow an "N sub-threads" badge for children that were never
/// spawned.
///
/// The parent's identity and the originating tool call live in the `origin`
/// instead, as a `ThreadLink` with `direction: Parent`. That is where the
/// message-route panel already reads them, so the child's timeline attributes
/// the follow-up to the parent by title rather than rendering it as "You".
pub(crate) fn build_follow_up_message(
    workspace_path: &std::path::Path,
    text: &str,
    images: Option<&[crate::api::ChatImage]>,
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
        Some(origin.clone()),
    )
}

/// The `ThreadLink` a follow-up stamps on the child's message. Mirrors
/// `MessageOrigin::thread_link_child`, which the fan-in uses in the other
/// direction, so the two edges stay recognisably symmetric.
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

impl crate::engine::LucidosEngine {
    /// Deliver a follow-up from a parent thread to one of its own children.
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
    /// `chat_message_is_pre_emittable` cannot be reused: it requires
    /// `mode == Human` and a non-coding-agent target, and a follow-up is
    /// Agent-mode and may target a coding-agent child. The two exclusions are
    /// handled rather than ignored:
    ///
    /// - The **Human** exclusion is about routing (a Human message on a
    ///   question-parked thread becomes `UserQuestionAnswered`). An Agent
    ///   follow-up never takes that route, so the exclusion does not apply.
    /// - The **coding-agent** exclusion is a real ordering rule. The
    ///   coding-agent lane deliberately blocks on a Codex interrupt reaching a
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
    // Eight with `self`, one over clippy's threshold since `urgency` joined.
    // Same allow, and the same reason, as the `process_message_with_steps` pair
    // this delegates to: these are the per-request knobs surfaced at the call
    // boundary on purpose, so a caller wires them through without re-creating a
    // builder struct. Every one of them is load-bearing and named, and two
    // (`caller_thread_id`, `caller_workspace`) exist only to be checked.
    #[allow(clippy::too_many_arguments)]
    pub fn follow_up_child_thread<'a>(
        self: &'a std::sync::Arc<Self>,
        caller_thread_id: Option<Uuid>,
        child_thread_id: Uuid,
        text: &'a str,
        images: Option<&'a [crate::api::ChatImage]>,
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
            // Authorized, so the caller is Some and is the child's parent.
            let caller_thread_id = row.parent_thread_id.expect("ladder proved parenthood");
            let use_coding_agent = row.uses_coding_agent();

            // The child's timeline attributes this to the parent thread, by title,
            // with `direction: Parent`. That is what keeps the follow-up from
            // rendering as "You" in the child's message-route panel.
            // Not fatal if it fails: the route panel resolves the parent's title
            // live from `threadMap` first and only falls back to this cached one,
            // so a missing title costs nothing visible. Logged rather than
            // swallowed, because a query error here means something is wrong with
            // the projection read and nothing else would say so.
            let parent_title: Option<String> = match sqlx::query_scalar::<_, String>(
                "SELECT COALESCE(title, first_message) FROM thread_summaries WHERE thread_id = $1",
            )
            .bind(caller_thread_id)
            .fetch_optional(self.pool())
            .await
            {
                Ok(t) => t.map(|t| t.chars().take(80).collect()),
                Err(e) => {
                    crate::log!(
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

            crate::log!(
                "[ChildFollowUp] Parent {} follows up on child {} ({:?}, coding_agent={})",
                caller_thread_id,
                child_thread_id,
                ack.delivered_to,
                use_coding_agent
            );

            // Spawn, do not await. Everything the engine can derive it derives:
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
            // From the ack, never from the caller's raw flag: see
            // `FollowUpDelivery::effective_urgency`. A question-parked child
            // must not be preempted, and the ack has already promised it will
            // not be.
            let urgency = ack.delivered_to.effective_urgency();
            // Type-erased before it reaches `tokio::spawn`, to break an auto-trait
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
                            crate::engine::thread_events::ActorMode::Agent,
                            None,
                            None,
                            pre_emitted_origin,
                            None,
                            Some(spawn_origin),
                            urgency,
                        )
                        .await;
                    match result {
                        // Drain the orphan chain, exactly as the other two spawn
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
                        // No terminator here: the turn settles its own
                        // exchange, anchored. An unanchored copy is what the
                        // idempotency gate cannot match, so it double-fired.
                        Err(e) => crate::log!(
                            "[ChildFollowUp] Follow-up turn failed on child {}: {}",
                            child_thread_id,
                            e
                        ),
                    }
                });
            let handle = tokio::spawn(turn);
            // Same panic monitoring `chat_submit` uses: a panicking child turn
            // emits ResponseFailed + SessionEnded instead of leaving the child
            // stuck in `running` and the parent waiting forever. Fire and forget,
            // exactly as there: dropping the JoinHandle detaches the watcher.
            drop(Self::monitor_cc_task(self.clone(), child_thread_id, handle));

            Ok(ack)
        })
    }

    /// Persist the follow-up's `MessageReceived` inline, iff the pre-emit rule
    /// says to. Returns the `PreEmittedOrigin` the spawned turn must carry so
    /// neither fast path emits the event a second time.
    ///
    /// `PreEmittedOrigin::Message`, never `EngineReentry`: a redirect from the
    /// parent is a real message the child should acknowledge with its own
    /// `CodingAgentPromptSent` / `UserPromptInjected`, not a silent engine
    /// re-entry like a child's completion re-opening its parent.
    async fn pre_emit_follow_up(
        self: &std::sync::Arc<Self>,
        child_thread_id: Uuid,
        delivered_to: FollowUpDelivery,
        text: &str,
        images: Option<&[crate::api::ChatImage]>,
        origin: &crate::engine::thread_events::MessageOrigin,
    ) -> Result<Option<crate::engine::chat::PreEmittedOrigin>, ChildFollowUpError> {
        if !delivered_to.wants_pre_emit() {
            return Ok(None);
        }
        // The child can start running between the row read and this emit. The
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
            crate::log!(
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
                crate::log!(
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
