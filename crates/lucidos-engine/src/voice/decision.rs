//! What is waiting on the user on one thread, as one thing a caller can settle.
//!
//! Four surfaces resolve to a fixed set of choices: a question card, and a
//! permission card in each of its three lanes. This module reads them as one
//! [`OpenDecision`] and settles one by a choice id. It lives under `voice/`
//! because the union exists for the talker, and nothing else consumes it.
//!
//! **The engine issues every id**, so nothing matches a spoken word to a label.
//! That is why no fuzzy rule and no model call is needed here, and why the
//! workspace's language cannot break it. [`OpenDecision::question`] carries the
//! rest: an id names its DECISION rather than that decision's place in the set.
//!
//! **A change in review is deliberately absent.** It is the union's fourth
//! shape, and it waits on the Apply surface being rebuilt in
//! `docs/plans/2026-08-30-a-thread-acts-in-its-own-subtree.md`.

use std::sync::Arc;

use async_trait::async_trait;
use uuid::Uuid;

use super::{clip, READ_ALOUD_CHARS};
use crate::engine::agent_recovery::newest_open_question;
use crate::engine::claude_code::{derive_allow_pattern, AllowScope};
use crate::engine::thread_events::{AnswerKind, MessageOrigin, QuestionOption};
use crate::engine::LucidosEngine;

/// Which surface a decision came from.
///
/// The lane decides where a resolution goes, and it is the first half of every
/// id, so two lanes can never issue the same string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecisionKind {
    /// A question card. The only shape that also takes the caller's own words.
    Question,
    /// The Lucidos Agent's command guard paused a bash or python call.
    CommandPermission,
    /// The Lucidos Agent paused an MCP server tool call.
    McpPermission,
    /// A coding agent asked before a tool call.
    ///
    /// Reachable during a call only through a destination flip. `api::voice`
    /// refuses to place one on a coding-agent thread (ADR 0165). But a thread
    /// can move to a coding agent while a call is already up. Rare, and not
    /// dead: a caller on that thread still hears the card.
    CodingAgentPermission,
}

impl DecisionKind {
    /// The lane's half of every id it issues.
    const fn tag(self) -> &'static str {
        match self {
            Self::Question => "question",
            Self::CommandPermission => "command",
            Self::McpPermission => "mcp",
            Self::CodingAgentPermission => "agent",
        }
    }

    /// Whether a decision of this kind parks the thread's doer.
    ///
    /// Every kind here does, being a card the agent is blocked inside. A
    /// *change* in review is the shape that would answer false, and it is the
    /// shape this module deliberately cannot read. The match is exhaustive, so
    /// adding it has to answer this question.
    pub const fn parks_the_doer(self) -> bool {
        match self {
            Self::Question
            | Self::CommandPermission
            | Self::McpPermission
            | Self::CodingAgentPermission => true,
        }
    }
}

/// What settling one choice does. Private: nothing outside decides that.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Act {
    /// Answer the question card this choice came from.
    Answer {
        tool_use_id: String,
        answer: AnswerKind,
    },
    /// Answer it with the caller's own words, which only the call holds.
    TheirWords { tool_use_id: String },
    /// Resolve the permission card this choice came from. The lane comes from
    /// the decision holding it.
    Permit {
        request_id: String,
        allowed: bool,
        scope: Option<AllowScope>,
    },
}

/// One thing the caller may say back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecisionChoice {
    /// What the talker hands back to settle the decision. Issued here, and
    /// nowhere else.
    pub id: String,
    /// What the talker reads out.
    pub label: String,
    /// A sentence of detail, where the surface carried one.
    pub description: Option<String>,
    act: Act,
}

/// Something on a thread waiting on the user, with the ways it can be settled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenDecision {
    /// The decision's own id. Every choice id below starts with it.
    pub id: String,
    pub kind: DecisionKind,
    /// What is being asked, in the words the surface used.
    pub prompt: String,
    /// Never empty: a decision nothing can settle is not one worth reading out.
    pub choices: Vec<DecisionChoice>,
}

/// What the "they said something else" choice is called.
///
/// A question card is issued one, and picking it sends the caller's own
/// transcript. That mirrors the screen, where the card's options sit beside a
/// prompt textarea the user can type into instead.
const THEIR_WORDS_LABEL: &str = "Something else they said";

const THEIR_WORDS_DETAIL: &str = "Pick this for anything the choices above do \
                                  not cover, including picking more than one. \
                                  It sends what the caller actually said, word \
                                  for word.";

/// The three permission scopes a caller may reach, in the order the card draws
/// them.
///
/// Both Always-allow scopes stay on screen. They widen what every future agent
/// session may do without asking. A caller saying "always" on a phone usually
/// means "stop asking me right now".
const ALLOW_ONCE: (&str, &str) = ("Allow once", "Just this one time.");
const ALLOW_THREAD: (&str, &str) = (
    "Allow for this thread",
    "This and anything like it, for the rest of this conversation.",
);
const DENY: (&str, &str) = ("Deny", "Refuse it. Lucidos is told no and carries on.");

impl OpenDecision {
    /// A question card, as the talker meets it.
    ///
    /// **An id names its decision, never that decision's place in the set.**
    /// This one is built from the `tool_use_id`, which is unique and never
    /// reused. So re-reading a live card issues the same id, and a settled
    /// card's id can never land on the card replacing it. Within the card a
    /// choice IS numbered by position, which is safe only because a card's
    /// options are fixed the moment it is asked.
    ///
    /// The question itself is NEVER cut, unlike everything else read aloud. A
    /// truncated question is a different question, and the talker is about to
    /// state it as the one being asked.
    ///
    /// The choices are the card's own options plus one more, for the caller who
    /// said something they do not cover. A free-text question has only that
    /// one, which is right: it is the whole of what the card takes.
    pub fn question(
        tool_use_id: &str,
        question: &str,
        options: &[QuestionOption],
        multi_select: bool,
    ) -> Self {
        let id = format!("{}:{}", DecisionKind::Question.tag(), tool_use_id);
        // Numbered by position within THIS card, not by the option's own id,
        // which the asking agent wrote and could spell with our separator in
        // it. A card's option list is fixed once asked, so the number is stable.
        let mut choices: Vec<DecisionChoice> = options
            .iter()
            .enumerate()
            .map(|(index, option)| DecisionChoice {
                id: format!("{}#opt{}", id, index),
                label: option.label.clone(),
                description: option
                    .description
                    .as_deref()
                    .map(str::trim)
                    .filter(|d| !d.is_empty())
                    .map(|d| clip(d, READ_ALOUD_CHARS)),
                act: Act::Answer {
                    tool_use_id: tool_use_id.to_string(),
                    // A multi-select card is answered one option at a time out
                    // loud, because the tool takes one id. Anything wider goes
                    // through the choice below, in the caller's own words.
                    answer: if multi_select {
                        AnswerKind::MultiSelected {
                            option_ids: vec![option.id.clone()],
                            text: None,
                        }
                    } else {
                        AnswerKind::Selected {
                            option_id: option.id.clone(),
                        }
                    },
                },
            })
            .collect();
        choices.push(DecisionChoice {
            id: format!("{}#said", id),
            label: THEIR_WORDS_LABEL.to_string(),
            description: Some(THEIR_WORDS_DETAIL.to_string()),
            act: Act::TheirWords {
                tool_use_id: tool_use_id.to_string(),
            },
        });
        Self {
            id,
            kind: DecisionKind::Question,
            prompt: question.trim().to_string(),
            choices,
        }
    }

    /// A command-guard card. Its summary is a fixed risk phrase, and the
    /// command itself is cut like everything else read aloud.
    pub fn command_permission(
        request_id: &str,
        tool_name: &str,
        command: &str,
        summary: &str,
    ) -> Self {
        let grantable = crate::engine::command_permission::derive_command_allow_pattern(
            tool_name,
            command,
            AllowScope::Session,
        )
        .is_some();
        Self::permission(
            DecisionKind::CommandPermission,
            request_id,
            &format!("{}\n{}", summary.trim(), clip(command, READ_ALOUD_CHARS)),
            grantable,
        )
    }

    /// An MCP card. The server's human label, the tool, and its arguments.
    pub fn mcp_permission(
        request_id: &str,
        server_id: &str,
        server_name: &str,
        tool_name: &str,
        arguments_summary: &str,
    ) -> Self {
        let grantable = crate::engine::mcp_permission::derive_mcp_allow_pattern(
            server_id,
            tool_name,
            AllowScope::Session,
        )
        .is_some();
        Self::permission(
            DecisionKind::McpPermission,
            request_id,
            &format!(
                "{} wants to use its {} tool.\n{}",
                server_name.trim(),
                tool_name.trim(),
                clip(arguments_summary, READ_ALOUD_CHARS)
            ),
            grantable,
        )
    }

    /// A coding-agent card. Its summary already names the tool and its target.
    pub fn coding_agent_permission(
        request_id: &str,
        tool_name: &str,
        input: &serde_json::Value,
        summary: &str,
    ) -> Self {
        let grantable = derive_allow_pattern(tool_name, input, AllowScope::Session).is_some();
        Self::permission(
            DecisionKind::CodingAgentPermission,
            request_id,
            &clip(summary, READ_ALOUD_CHARS),
            grantable,
        )
    }

    /// The shared body of the three permission lanes: one wording and one
    /// choice set, so the caller hears the same options whichever agent asked.
    ///
    /// `grantable` is whether an "Allow for this thread" click would record
    /// anything. Where it would not, the choice is left out rather than
    /// offered. The screen hides that button for the same reason, and a caller
    /// cannot see which buttons are there.
    fn permission(kind: DecisionKind, request_id: &str, prompt: &str, grantable: bool) -> Self {
        let id = format!("{}:{}", kind.tag(), request_id);
        let scope = |suffix, (label, detail): (&str, &str), allowed, persist| DecisionChoice {
            id: format!("{}#{}", id, suffix),
            label: label.to_string(),
            description: Some(detail.to_string()),
            act: Act::Permit {
                request_id: request_id.to_string(),
                allowed,
                scope: persist,
            },
        };
        let mut choices = vec![scope("allow-once", ALLOW_ONCE, true, None)];
        if grantable {
            choices.push(scope(
                "allow-thread",
                ALLOW_THREAD,
                true,
                Some(AllowScope::Session),
            ));
        }
        choices.push(scope("deny", DENY, false, None));
        Self {
            id,
            kind,
            prompt: prompt.trim().to_string(),
            choices,
        }
    }
}

/// What one attempt to settle a decision did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    /// Settled, and the caller's held transcript was not what settled it.
    Settled,
    /// Settled BY the caller's own words, which are now spent. Writing them
    /// down again would put the same sentence in the thread twice.
    SettledWithTheirWords,
    /// This choice sends the caller's own words, and none have arrived yet.
    /// The transcript and the tool call come from two models on one socket, so
    /// a short fast reply really does produce the call first.
    NeedsTheirWords,
    /// Nothing was settled. The string is what the talker is told, in words it
    /// can say out loud.
    Refused(String),
}

/// What the talker is told when nothing is waiting on the id it handed back.
///
/// One note for two cases, because the engine cannot tell them apart and must
/// not guess: an id it never issued, and an id whose card settled first.
pub const NOT_WAITING: &str = "That did not answer anything. Either it was \
                               already settled, or that was not one of the \
                               choices. Tell the caller, and read them what is \
                               still open.";

/// What the talker is told when the card settled between the read and the write.
pub const SETTLED_MEANWHILE: &str = "That settled just before the answer \
                                     reached it, so nothing changed. Tell the \
                                     caller it is already done.";

/// What a call can do about what is waiting on its own thread.
///
/// A seam, for the reason `doer.rs` gives for its own: the call loop stays
/// drivable with no engine behind it, so the tests exercise a whole call with
/// no credential and no socket.
#[async_trait]
pub trait DecisionResolver: Send + Sync {
    /// Settle one decision, by a choice id this resolver issued.
    ///
    /// `spoken` is the caller's held transcript, and exactly one choice uses
    /// it: the question card's "something else". Empty when nothing is held.
    async fn resolve(
        &self,
        thread_id: Uuid,
        choice_id: &str,
        spoken: &str,
        actor: Option<MessageOrigin>,
    ) -> Resolution;

    /// Whether this thread's doer is parked on something waiting on the user.
    ///
    /// One read, and the whole of what the refusal needs: a delegation cannot
    /// start a turn while the agent is blocked inside the call that asked.
    async fn doer_is_parked(&self, thread_id: Uuid) -> bool;
}

/// The shipping implementation: the engine's own in-process paths.
pub struct ThreadDecisions {
    engine: Arc<LucidosEngine>,
}

impl ThreadDecisions {
    pub fn new(engine: Arc<LucidosEngine>) -> Self {
        Self { engine }
    }
}

#[async_trait]
impl DecisionResolver for ThreadDecisions {
    async fn resolve(
        &self,
        thread_id: Uuid,
        choice_id: &str,
        spoken: &str,
        actor: Option<MessageOrigin>,
    ) -> Resolution {
        resolve(&self.engine, thread_id, choice_id, spoken, actor).await
    }

    async fn doer_is_parked(&self, thread_id: Uuid) -> bool {
        doer_is_parked(self.engine.pool(), thread_id).await
    }
}

/// Everything waiting on the user on this thread, in lane order.
///
/// One indexed read per lane, all four together, and no payload beyond the
/// newest of each. The caller is waiting for the first word of a phone call.
///
/// A read that fails contributes nothing and logs. A call that opens knowing
/// less beats one that does not open. On the answering side a lane it could
/// not read refuses, rather than settling the wrong card.
pub async fn open_on(pool: &sqlx::PgPool, thread_id: Uuid) -> Vec<OpenDecision> {
    let (question, command, mcp, agent) = tokio::join!(
        newest_open_question(pool, thread_id),
        newest_unresolved(
            pool,
            thread_id,
            "CommandPermissionRequested",
            "CommandPermissionResolved"
        ),
        newest_unresolved(
            pool,
            thread_id,
            "McpPermissionRequested",
            "McpPermissionResolved"
        ),
        newest_unresolved(
            pool,
            thread_id,
            "CodingAgentPermissionRequest",
            "CodingAgentPermissionResolved"
        ),
    );

    let mut open = Vec::new();
    if let Some(q) = question {
        open.push(OpenDecision::question(
            &q.tool_use_id,
            &q.question,
            &q.options,
            q.multi_select,
        ));
    }
    if let Some((payload, request_id)) = requested(command) {
        open.push(OpenDecision::command_permission(
            &request_id,
            &text(&payload, "tool_name").unwrap_or_default(),
            &text(&payload, "command").unwrap_or_default(),
            &text(&payload, "summary").unwrap_or_default(),
        ));
    }
    if let Some((payload, request_id)) = requested(mcp) {
        open.push(OpenDecision::mcp_permission(
            &request_id,
            &text(&payload, "server_id").unwrap_or_default(),
            &text(&payload, "server_name").unwrap_or_default(),
            &text(&payload, "tool_name").unwrap_or_default(),
            &text(&payload, "arguments_summary").unwrap_or_default(),
        ));
    }
    if let Some((payload, request_id)) = requested(agent) {
        open.push(OpenDecision::coding_agent_permission(
            &request_id,
            &text(&payload, "tool_name").unwrap_or_default(),
            payload.get("input").unwrap_or(&serde_json::Value::Null),
            &text(&payload, "summary").unwrap_or_default(),
        ));
    }
    open
}

/// A read payload paired with its `request_id`, which every id below is built
/// from. A payload carrying none is dropped: nothing could resolve it.
fn requested(payload: Option<serde_json::Value>) -> Option<(serde_json::Value, String)> {
    let payload = payload?;
    let request_id = text(&payload, "request_id")?;
    Some((payload, request_id))
}

/// A payload string, trimmed, or `None` when it is missing or blank.
fn text(payload: &serde_json::Value, key: &str) -> Option<String> {
    let value = payload.get(key)?.as_str()?.trim();
    (!value.is_empty()).then(|| value.to_string())
}

/// Whether this thread's doer is parked on something waiting on the user.
///
/// **The same read [`open_on`] makes, and deliberately not a cheaper one.** A
/// yes/no `EXISTS` over the four lanes would be one query instead of four. It
/// would also answer a DIFFERENT question, by counting a card this reader drops
/// as unanswerable: a question carrying no `tool_use_id`, say. The thread would
/// then refuse every delegation for the rest of the call, while the talker
/// holds no card to put to the caller. That is the worst state a call can be
/// in, and it is worth three extra reads to make it unreachable.
///
/// The four run concurrently, so the cost is one read of latency. It lands on a
/// path that is about to emit an event and start a turn.
pub async fn doer_is_parked(pool: &sqlx::PgPool, thread_id: Uuid) -> bool {
    open_on(pool, thread_id)
        .await
        .iter()
        .any(|decision| decision.kind.parks_the_doer())
}

/// The newest request in one permission lane with no paired resolution.
async fn newest_unresolved(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    requested: &str,
    resolved: &str,
) -> Option<serde_json::Value> {
    let sql = "SELECT q.payload FROM events q \
          WHERE q.thread_id = $1 AND q.event_type = $2 \
            AND NOT EXISTS ( \
              SELECT 1 FROM events r \
              WHERE r.thread_id = $1 AND r.event_type = $3 \
                AND r.payload->>'request_id' = q.payload->>'request_id' ) \
          ORDER BY q.sequence DESC LIMIT 1";
    match sqlx::query_scalar::<_, serde_json::Value>(sql)
        .bind(thread_id)
        .bind(requested)
        .bind(resolved)
        .fetch_optional(pool)
        .await
    {
        Ok(payload) => payload,
        Err(e) => {
            log!(
                "[Voice] Could not read {} on {}: {}",
                requested,
                thread_id,
                e
            );
            None
        }
    }
}

/// Settle one decision, by a choice id the engine issued.
///
/// It re-reads what is open first. An id nobody issued and an id whose card has
/// settled are then refused by the same rule. No held set can go stale behind a
/// card the user answered on screen.
pub async fn resolve(
    engine: &Arc<LucidosEngine>,
    thread_id: Uuid,
    choice_id: &str,
    spoken: &str,
    actor: Option<MessageOrigin>,
) -> Resolution {
    let open = open_on(engine.pool(), thread_id).await;
    let Some((kind, act)) = choice_in(&open, choice_id) else {
        return Resolution::Refused(NOT_WAITING.to_string());
    };

    match act {
        Act::Answer {
            tool_use_id,
            answer,
        } => answered(engine, thread_id, &tool_use_id, answer, actor).await,
        Act::TheirWords { tool_use_id } => {
            let text = spoken.trim();
            if text.is_empty() {
                return Resolution::NeedsTheirWords;
            }
            // The caller's transcript, word for word. A paraphrase would be a
            // different answer, and the talker may state no fact it was not
            // given (ADR 0149).
            let answer = AnswerKind::FreeText {
                text: text.to_string(),
            };
            match answered(engine, thread_id, &tool_use_id, answer, actor).await {
                Resolution::Settled => Resolution::SettledWithTheirWords,
                other => other,
            }
        }
        Act::Permit {
            request_id,
            allowed,
            scope,
        } => permitted(engine, kind, request_id, allowed, scope, actor).await,
    }
}

/// What an id names, among what is open right now.
///
/// `None` is BOTH refusals in one rule: an id the engine never issued is not
/// here, and neither is one whose card has since settled. Split out so a test
/// can state each without an engine behind it.
fn choice_in(open: &[OpenDecision], choice_id: &str) -> Option<(DecisionKind, Act)> {
    open.iter().find_map(|decision| {
        decision
            .choices
            .iter()
            .find(|choice| choice.id == choice_id)
            .map(|choice| (decision.kind, choice.act.clone()))
    })
}

/// Route a question answer through the one path every other channel takes.
async fn answered(
    engine: &Arc<LucidosEngine>,
    thread_id: Uuid,
    tool_use_id: &str,
    answer: AnswerKind,
    actor: Option<MessageOrigin>,
) -> Resolution {
    use crate::engine::agent_question::{answer_pending_question, AnswerResult};
    match answer_pending_question(engine, thread_id, tool_use_id.to_string(), answer, actor).await {
        AnswerResult::Resumed => Resolution::Settled,
        AnswerResult::Conflict(why) => {
            log!("[Voice] A spoken answer did not land: {}", why);
            Resolution::Refused(SETTLED_MEANWHILE.to_string())
        }
    }
}

/// Route a permission answer through its lane's one in-process resolver, which
/// is the one the consent endpoint calls.
async fn permitted(
    engine: &Arc<LucidosEngine>,
    kind: DecisionKind,
    request_id: String,
    allowed: bool,
    scope: Option<AllowScope>,
    actor: Option<MessageOrigin>,
) -> Resolution {
    let settled = match kind {
        DecisionKind::CommandPermission => {
            crate::engine::command_permission::resolve_command_permission(
                engine,
                request_id,
                allowed,
                scope,
                actor,
                "[Voice] CommandPermissionResolved",
            )
            .await
        }
        DecisionKind::McpPermission => {
            crate::engine::mcp_permission::resolve_mcp_permission(
                engine,
                request_id,
                allowed,
                scope,
                actor,
                "[Voice] McpPermissionResolved",
            )
            .await
        }
        DecisionKind::CodingAgentPermission => {
            crate::engine::cc_permission::resolve_coding_agent_permission(
                engine,
                request_id,
                allowed,
                scope,
                actor,
                "[Voice] CodingAgentPermissionResolved",
            )
            .await
        }
        // Unreachable: only a permission lane issues a `Permit`, and those
        // three are all of them. Refused rather than panicked, because a live
        // call must not die over a decision it cannot place.
        DecisionKind::Question => false,
    };
    if settled {
        Resolution::Settled
    } else {
        Resolution::Refused(SETTLED_MEANWHILE.to_string())
    }
}

#[cfg(test)]
#[path = "decision_tests.rs"]
mod tests;
