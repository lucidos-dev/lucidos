use std::sync::Arc;

use serde_json::json;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;
use uuid::Uuid;

use super::legacy::{self, LegacyCard};
use super::{PermissionChoice, PermissionRequest, PickedBy};
use crate::engine::event_bus::{BusEvent, EmittedEvent};
use crate::engine::model_call::AuxCapture;
use crate::engine::thread_events::{
    AnswerKind, EventMeta, MessageOrigin, QuestionOption, ThreadEvent,
};
use crate::engine::{ContextPurpose, LucidosEngine};
use crate::llm::judgment::{
    for_site, Answers, ChatJudgmentProvider, JudgmentProvider, JudgmentSite, Question,
};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

const QUESTION_ID: &str = "pick";

/// INVARIANT: The criterion that picks nothing. No option id can collide with it: a
/// card's ids all start with `opt-`, and a tool-permission card's with
/// `legacy-`.
const NO_PICK: &str = "none";

/// INVARIANT: How sure the judgment must be before words pick an option. Granting is the
/// costly mistake, so the bar is high and a tap stays the easy path.
pub(crate) const PICK_THRESHOLD: f64 = 0.9;

const INSTRUCTIONS: &str = "The owner of this workspace said `words` while \
Lucidos was asking them `request`. Decide which option the words choose. Pick an option \
only when the words clearly choose exactly that one. A question, a doubt, a condition, a \
refusal of everything, or words about something else pick none. An option that lasts \
beyond one action needs words that ask for that reach, such as \"always\".";

const LASTING_NOTE: &str = "This lasts beyond one action, so pick it only when the words \
ask for that.";

const NO_PICK_NOTE: &str = "The words choose none of the options clearly: they ask \
something, hesitate, set a condition, or are about something else.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OwnerWords {
    pub(crate) thread_id: Uuid,
    pub(crate) event_id: Uuid,
    pub(crate) sequence: i64,
    pub(crate) text: String,
    pub(crate) device_id: String,
}

impl OwnerWords {
    pub(crate) fn from_event(
        thread_id: Uuid,
        event_id: Uuid,
        sequence: i64,
        event: &ThreadEvent,
        meta: &EventMeta,
    ) -> Option<Self> {
        let ThreadEvent::SpokenMessageReceived { text, .. } = event else {
            return None;
        };
        let MessageOrigin::Device { device_id } = meta.actor.as_ref()? else {
            return None;
        };
        let text = text.trim();
        (!text.is_empty()).then(|| Self {
            thread_id,
            event_id,
            sequence,
            text: text.to_string(),
            device_id: device_id.clone(),
        })
    }

    fn of(emitted: &EmittedEvent) -> Option<Self> {
        let BusEvent::Thread {
            thread_id,
            event,
            meta,
        } = &emitted.typed
        else {
            return None;
        };
        Self::from_event(*thread_id, emitted.event_id, emitted.seq?, event, meta)
    }

    pub(crate) fn actor(&self) -> MessageOrigin {
        MessageOrigin::Device {
            device_id: self.device_id.clone(),
        }
    }

    fn picked_by(&self) -> PickedBy {
        PickedBy::Spoken {
            message_event_id: Some(self.event_id),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum OpenCard {
    Request { tool_use_id: String },
    Tool(LegacyCard),
}

#[derive(Debug, Clone, PartialEq)]
pub struct OpenRequest {
    pub(crate) card: OpenCard,
    pub(crate) question: String,
    pub(crate) options: Vec<QuestionOption>,
    pub(crate) request: PermissionRequest,
}

/// INVARIANT: The newest unanswered permission request on `thread_id` asked before
/// `sequence`. Words recorded before a request was asked never answer it.
/// Spoken words can reach a tool-permission card. A request an agent
/// prepared uses its own answer path.
/// None reach an unattested caller's card either: only a tap allows its act.
pub async fn open_request_before(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    sequence: i64,
) -> Result<Option<OpenRequest>, BoxError> {
    let row: Option<(i64, serde_json::Value)> = sqlx::query_as(
        "SELECT q.sequence, q.payload FROM events q \
         WHERE q.thread_id = $1 AND q.event_type = 'UserQuestionAsked' \
           AND q.payload ? 'permission_request' AND q.sequence < $2 \
           AND NOT q.payload->'permission_request' ? 'prepared' \
           AND NOT q.payload->'permission_request' ? 'unattested_act' \
           AND q.payload #> '{permission_request,choices}' <> '{}'::jsonb \
           AND NOT EXISTS ( \
             SELECT 1 FROM events a \
             WHERE a.thread_id = $1 AND a.event_type = 'UserQuestionAnswered' \
               AND a.payload->>'tool_use_id' = q.payload->>'tool_use_id' ) \
         ORDER BY q.sequence DESC LIMIT 1",
    )
    .bind(thread_id)
    .bind(sequence)
    .fetch_optional(pool)
    .await?;
    let tool_card = legacy::open_cards(pool, thread_id)
        .await?
        .into_iter()
        .find(|card| card.sequence < sequence);
    if let Some(card) =
        tool_card.filter(|card| row.as_ref().is_none_or(|(seq, _)| card.sequence > *seq))
    {
        let (options, request) = (card.options(), card.request());
        return Ok(Some(OpenRequest {
            question: card.question.clone(),
            options: options.into_iter().map(|(_, option)| option).collect(),
            request,
            card: OpenCard::Tool(card),
        }));
    }
    let Some((_, payload)) = row else {
        return Ok(None);
    };
    let text = |key: &str| {
        payload
            .get(key)
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| format!("permission request on {thread_id} has no {key}"))
    };
    Ok(Some(OpenRequest {
        card: OpenCard::Request {
            tool_use_id: text("tool_use_id")?,
        },
        question: text("question")?,
        options: serde_json::from_value(payload["options"].clone())?,
        request: serde_json::from_value(payload["permission_request"].clone())?,
    }))
}

pub(crate) fn judgment_state(open: &OpenRequest, words: &OwnerWords) -> serde_json::Value {
    json!({
        "request": open.question,
        "words": words.text,
        "said": "spoken",
    })
}

pub(crate) fn judgment_question(open: &OpenRequest) -> Vec<(String, Question)> {
    let mut criteria: Vec<(String, Option<String>)> = open
        .options
        .iter()
        .filter_map(|option| {
            let choice = open.request.choices.get(&option.id)?;
            let mut rubric = option.label.clone();
            if let Some(detail) = option.description.as_deref().map(str::trim) {
                if !detail.is_empty() {
                    rubric = format!("{rubric}: {detail}");
                }
            }
            let lasting = match choice {
                PermissionChoice::Grant { lifetime, .. } => *lifetime != super::Lifetime::Once,
                PermissionChoice::RetiredUnpause
                | PermissionChoice::Revoke
                | PermissionChoice::RefuseStanding => true,
                PermissionChoice::Refuse => false,
            };
            if lasting {
                rubric = format!("{rubric} {LASTING_NOTE}");
            }
            Some((option.id.clone(), Some(rubric)))
        })
        .collect();
    criteria.push((NO_PICK.to_string(), Some(NO_PICK_NOTE.to_string())));
    vec![(
        QUESTION_ID.to_string(),
        Question::Choice {
            instructions: INSTRUCTIONS.to_string(),
            criteria,
        },
    )]
}

pub(crate) fn picked_option(answers: &Answers, open: &OpenRequest) -> Option<String> {
    let answer = answers.choice(QUESTION_ID)?;
    let option = answer.choice.as_str();
    if option == NO_PICK || !open.request.choices.contains_key(option) {
        return None;
    }
    (answer.probability(option) >= PICK_THRESHOLD).then(|| option.to_string())
}

pub(crate) async fn judge_words<J: JudgmentProvider + ?Sized>(
    capture: &AuxCapture,
    judge: &J,
    open: &OpenRequest,
    words: &OwnerWords,
) -> Option<String> {
    match capture
        .judge(judge, judgment_state(open, words), judgment_question(open))
        .await
    {
        Ok(judgment) => picked_option(&judgment.answers, open),
        Err(e) => {
            crate::log_legacy!(
                "[Permission] Could not judge the owner's words on {}, so they grant nothing: {e}",
                words.thread_id
            );
            None
        }
    }
}

async fn registered(pool: &sqlx::PgPool, device_id: &str) -> bool {
    match crate::core::DeviceStore::is_registered(pool, device_id).await {
        Ok(registered) => registered,
        Err(e) => {
            crate::log_legacy!("[Permission] Could not confirm device '{device_id}': {e}");
            false
        }
    }
}

impl LucidosEngine {
    pub(crate) async fn answer_permission_in_words(self: &Arc<Self>, words: OwnerWords) {
        let open = match open_request_before(self.pool(), words.thread_id, words.sequence).await {
            Ok(Some(open)) => open,
            Ok(None) => return,
            Err(e) => {
                crate::log_legacy!(
                    "[Permission] Could not read the open request on {}: {e}",
                    words.thread_id
                );
                return;
            }
        };
        if !registered(self.pool(), &words.device_id).await {
            crate::log_legacy!(
                "[Permission] Words on {} came from no registered device, so they grant nothing",
                words.thread_id
            );
            return;
        }
        let call = self.aux_call(ContextPurpose::PermissionAnswer).await;
        let system_one = self
            .judgment_system_one(JudgmentSite::PermissionAnswer, call.attempt_timeout())
            .await;
        let chat = ChatJudgmentProvider::new(call.provider(), call.reasoning().map(str::to_string))
            .forcing_the_answer();
        let judge = for_site(system_one, chat);
        let capture = AuxCapture::new(
            &self.event_bus,
            words.thread_id,
            ContextPurpose::PermissionAnswer,
        )
        .until(tokio::time::Instant::now() + call.deadline());
        let Some(option_id) = judge_words(&capture, judge.as_ref(), &open, &words).await else {
            return;
        };
        let tool_use_id = match open.card {
            OpenCard::Request { tool_use_id } => tool_use_id,
            OpenCard::Tool(card) => {
                let Some(pick) = card.pick_of(&option_id) else {
                    return;
                };
                let context = "[Permission] tool permission resolved by the owner's words";
                let answered_by = super::legacy::AnsweredBy {
                    actor: Some(words.actor()),
                    picked_by: words.picked_by(),
                };
                if !card.resolve(self, pick, answered_by, context).await {
                    crate::log_legacy!(
                        "[Permission] The owner's words on {} found card {} already settled",
                        words.thread_id,
                        card.request_id
                    );
                }
                return;
            }
        };
        let outcome = crate::engine::agent_question::answer_pending_question_picked_by(
            self,
            words.thread_id,
            tool_use_id.clone(),
            AnswerKind::Selected { option_id },
            Some(words.actor()),
            None,
            words.picked_by(),
        )
        .await;
        if let crate::engine::agent_question::AnswerResult::Conflict(why) = outcome {
            crate::log_legacy!(
                "[Permission] The owner's words on {} did not answer {tool_use_id}: {why}",
                words.thread_id,
            );
        }
    }
}

pub fn spawn(engine: Arc<LucidosEngine>) -> tokio::task::JoinHandle<()> {
    let rx = engine.event_bus.subscribe();
    tokio::spawn(async move {
        let stream = BroadcastStream::new(rx);
        tokio::pin!(stream);
        while let Some(result) = stream.next().await {
            let emitted = match result {
                Ok(emitted) => emitted,
                Err(BroadcastStreamRecvError::Lagged(n)) => {
                    crate::log_legacy!(
                        "[Permission] Broadcast lagged by {n} events, so words among them \
                         answer no permission request"
                    );
                    continue;
                }
            };
            if let Some(words) = OwnerWords::of(&emitted) {
                let engine = engine.clone();
                tokio::spawn(async move { engine.answer_permission_in_words(words).await });
            }
        }
    })
}

#[cfg(test)]
#[path = "words_tests.rs"]
mod tests;
