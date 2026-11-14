//! What is waiting on the user on one thread, as one thing a caller can settle.
//!
//! Five surfaces resolve to a fixed set of choices: a question card, a
//! *permission request*, and a permission card in each of its three lanes. This module reads them as one
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
use crate::engine::agent_recovery::{newest_open_question, OpenRequest};
use crate::engine::permission::legacy::{self, LegacyCard, LegacyLane, LegacyPick};
use crate::engine::permission::PickedBy;
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
    /// A *permission request* (ADR 0664). The talker reads it back but never
    /// settles it. `awaited` when an agent prepared it, so the agent's question
    /// tool waits on the answer. `words_answer` when the caller's own transcript
    /// settles it, through `permission::words`. Otherwise only a tap does.
    PermissionRequest { awaited: bool, words_answer: bool },
}

impl DecisionKind {
    /// The lane's half of every id it issues.
    const fn tag(self) -> &'static str {
        match self {
            Self::Question => "question",
            Self::CommandPermission => "command",
            Self::McpPermission => "mcp",
            Self::CodingAgentPermission => "agent",
            Self::PermissionRequest { .. } => "permission",
        }
    }

    /// Whether a decision of this kind parks the thread's doer.
    ///
    /// Every card an agent is blocked inside does, a permission request the
    /// agent prepared included. One the engine raised itself does not: no agent
    /// waits on it. A *change* in review
    /// would answer false too, and this module deliberately cannot read it. The
    /// match is exhaustive, so adding it has to answer this question.
    pub const fn parks_the_doer(self) -> bool {
        match self {
            Self::Question
            | Self::CommandPermission
            | Self::McpPermission
            | Self::CodingAgentPermission => true,
            Self::PermissionRequest { awaited, .. } => awaited,
        }
    }

    /// The kind a tool-permission card's lane reads as.
    const fn of_lane(lane: LegacyLane) -> Self {
        match lane {
            LegacyLane::Command => Self::CommandPermission,
            LegacyLane::Mcp => Self::McpPermission,
            LegacyLane::CodingAgent => Self::CodingAgentPermission,
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
    /// Resolve the tool-permission card this choice came from.
    Permit {
        lane: LegacyLane,
        request_id: String,
        pick: LegacyPick,
    },
    /// Nothing: the option is read out, never picked. A permission request's
    /// options and a card's Always allow are settled only by the caller's own
    /// transcript, through `permission::words`.
    Heard,
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
                            image_hashes: vec![],
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

    /// A *permission request*, as the talker meets it: every option read back,
    /// none of them pickable.
    pub(crate) fn permission_request(
        tool_use_id: &str,
        question: &str,
        options: &[QuestionOption],
        request: OpenRequest,
    ) -> Self {
        let kind = DecisionKind::PermissionRequest {
            awaited: request.awaited,
            words_answer: request.words_answer,
        };
        let id = format!("{}:{}", kind.tag(), tool_use_id);
        let choices = options
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
                    .map(str::to_string),
                act: Act::Heard,
            })
            .collect();
        Self {
            id,
            kind,
            prompt: question.trim().to_string(),
            choices,
        }
    }

    /// A tool-permission card, with every button the card draws.
    ///
    /// Allow once, Allow for this thread and Deny settle on the talker's pick.
    /// An Always allow widens what every future session may do, and a caller
    /// saying "always" on a phone often means "stop asking me right now". So
    /// it is read back but never picked: only the caller's own words settle it.
    pub fn tool_permission(card: &LegacyCard) -> Self {
        let kind = DecisionKind::of_lane(card.lane);
        let id = format!("{}:{}", kind.tag(), card.request_id);
        let choices = card
            .options()
            .into_iter()
            .map(|(pick, option)| DecisionChoice {
                id: format!("{}#{}", id, choice_suffix(pick)),
                label: option.label,
                description: option.description,
                act: match pick {
                    LegacyPick::Narrow | LegacyPick::Broad => Act::Heard,
                    LegacyPick::Once | LegacyPick::Session | LegacyPick::Deny => Act::Permit {
                        lane: card.lane,
                        request_id: card.request_id.clone(),
                        pick,
                    },
                },
            })
            .collect();
        Self {
            id,
            kind,
            prompt: read_aloud(&card.question),
            choices,
        }
    }

    /// The id of the choice that sends the caller's own words, when there is
    /// one.
    ///
    /// Only a question card is issued one, so this is also the question of
    /// whether a decision can be settled without interpreting anything. A
    /// permission card answers `None`: Allow and Deny cannot be derived from
    /// "they said something".
    ///
    /// Read by `Call::delegated`, which is the route a talker holding no
    /// answering tool reaches.
    pub fn their_words_choice(&self) -> Option<&str> {
        self.choices
            .iter()
            .find(|choice| matches!(choice.act, Act::TheirWords { .. }))
            .map(|choice| choice.id.as_str())
    }
}

/// A card's question as the talker reads it. The first line is the card's
/// fixed risk phrase and stays whole. The rest, a command or a tool's
/// arguments, is cut like everything else read aloud.
fn read_aloud(question: &str) -> String {
    match question.split_once('\n') {
        Some((head, rest)) => format!("{}\n{}", head.trim(), clip(rest, READ_ALOUD_CHARS)),
        None => clip(question, READ_ALOUD_CHARS),
    }
}

/// A tool-permission choice's half of its id. Stable, so a talker holding one
/// from the opening block can still use it a minute later.
const fn choice_suffix(pick: LegacyPick) -> &'static str {
    match pick {
        LegacyPick::Once => "allow-once",
        LegacyPick::Session => "allow-thread",
        LegacyPick::Narrow => "always-narrow",
        LegacyPick::Broad => "always-broad",
        LegacyPick::Deny => "deny",
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

    /// What this thread's doer is parked on, if anything.
    ///
    /// One read, and the whole of what a delegation needs. A parked doer cannot
    /// start a turn, being blocked inside the call that asked. WHICH card it is
    /// then decides whether the caller can settle it out loud.
    async fn parked_on(&self, thread_id: Uuid) -> Option<OpenDecision>;
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

    async fn parked_on(&self, thread_id: Uuid) -> Option<OpenDecision> {
        parked_on(self.engine.pool(), thread_id).await
    }
}

/// Everything waiting on the user on this thread, in lane order: the question
/// card, then the newest card in each tool-permission lane.
///
/// Two reads, run together. The caller is waiting for the first word of a
/// phone call.
///
/// A read that fails contributes nothing and logs. A call that opens knowing
/// less beats one that does not open. On the answering side a lane it could
/// not read refuses, rather than settling the wrong card.
pub async fn open_on(pool: &sqlx::PgPool, thread_id: Uuid) -> Vec<OpenDecision> {
    let (question, cards) = tokio::join!(
        newest_open_question(pool, thread_id),
        legacy::open_cards(pool, thread_id),
    );

    let mut open = Vec::new();
    if let Some(q) = question {
        open.push(match q.permission_request {
            Some(request) => {
                OpenDecision::permission_request(&q.tool_use_id, &q.question, &q.options, request)
            }
            None => OpenDecision::question(&q.tool_use_id, &q.question, &q.options, q.multi_select),
        });
    }
    let cards = cards.unwrap_or_else(|e| {
        log!("[Voice] Could not read the permission cards on {thread_id}: {e}");
        Vec::new()
    });
    for lane in [
        LegacyLane::Command,
        LegacyLane::Mcp,
        LegacyLane::CodingAgent,
    ] {
        // Newest first, so the first card in a lane is its newest.
        if let Some(card) = cards.iter().find(|card| card.lane == lane) {
            open.push(OpenDecision::tool_permission(card));
        }
    }
    open
}

/// What this thread's doer is parked on, if anything.
///
/// **The same read [`open_on`] makes, and deliberately not a cheaper one.** A
/// yes/no `EXISTS` over the four lanes would be one query. It would also
/// answer a DIFFERENT question, by counting a card this reader drops as
/// unanswerable: a question carrying no `tool_use_id`, say. The thread would
/// then refuse every delegation for the rest of the call, while the talker
/// holds no card to put to the caller. That is the worst state a call can be
/// in, and it is worth the extra reads to make it unreachable.
///
/// It lands on a path that is about to emit an event and start a turn.
///
/// The FIRST parking decision, in [`open_on`]'s lane order, which leads with the
/// question card. A doer is blocked in one place at a time. Where two lanes
/// somehow both read open, the settleable one is worth putting to the caller.
pub async fn parked_on(pool: &sqlx::PgPool, thread_id: Uuid) -> Option<OpenDecision> {
    open_on(pool, thread_id)
        .await
        .into_iter()
        .find(|decision| decision.kind.parks_the_doer())
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
    let Some((_, act)) = choice_in(&open, choice_id) else {
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
                image_hashes: vec![],
            };
            match answered(engine, thread_id, &tool_use_id, answer, actor).await {
                Resolution::Settled => Resolution::SettledWithTheirWords,
                other => other,
            }
        }
        Act::Permit {
            lane,
            request_id,
            pick,
        } => permitted(engine, lane, request_id, pick, actor).await,
        Act::Heard => Resolution::Refused(PERMISSION_REQUEST_HEARD.to_string()),
    }
}

/// What the talker is told when it picks an option only the caller's own
/// words settle: a permission request's, or a card's Always allow.
pub const PERMISSION_REQUEST_HEARD: &str = "That did not answer it. Lucidos \
                                            settles this from what the caller \
                                            says themselves. Read them the \
                                            choices and let them say one.";

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
    lane: LegacyLane,
    request_id: String,
    pick: LegacyPick,
    actor: Option<MessageOrigin>,
) -> Resolution {
    if legacy::resolve_pick(
        engine,
        lane,
        request_id,
        pick,
        legacy::AnsweredBy {
            actor,
            picked_by: PickedBy::Spoken {
                message_event_id: None,
            },
        },
        "[Voice] tool permission resolved",
    )
    .await
    {
        Resolution::Settled
    } else {
        Resolution::Refused(SETTLED_MEANWHILE.to_string())
    }
}

#[cfg(test)]
#[path = "decision_tests.rs"]
mod tests;
