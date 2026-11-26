//! Refuses a question card when the agent owes the user an answer first.
//!
//! The user reads only the agent's text and its cards, never a tool result or
//! the agent's reasoning. So a card may not be the first thing they get after
//! tool work nobody reported. Nor may it point "above" at words the user never
//! read. A typed answer counts as a message: it may answer the card, ask back
//! or say something else. One rule for the chat agent,
//! Claude Code and Codex, read from the thread's events so every agent sees
//! the same facts.
//!
//! Nor may it follow an artifact the agent saved and the user cannot see: a
//! picture nobody drew, or a page nobody linked.
//!
//! A card question longer than any note reports the tool work before it: the
//! user reads it in full. So does the chat card's `message`, which the user
//! reads as the agent's reply.
//!
//! Nor may a card follow a *progress note* without a `message`. The user read
//! only the provider's summary of what the agent wrote, and the agent believes
//! they read all of it. The stream marks a note, so this rule guesses nothing.
//!
//! A permission request card always passes: the engine writes it.
//!
//! A model-tolerance measure: `docs/temporary-measures.md` § "A question card
//! with no answer before it". Plans:
//! `docs/plans/2026-09-23-a-card-never-replaces-the-answer.md`,
//! `docs/plans/2026-09-24-a-card-that-points-above-at-nothing.md` and
//! `docs/plans/2026-09-25-a-card-after-a-picture-nobody-saw.md`. The registry
//! entry records the widening to every artifact.

use sqlx::PgPool;
use uuid::Uuid;

const REFUSAL_MARKER: &str = "Question card not shown.";

const ARTIFACT_REFUSAL_LEAD: &str = "Since the user's last input you saved";

const REFUSAL_WINDOW_CHARS: i32 = 200;

macro_rules! card_message_hint {
    () => {
        "in the card's `message` field if your tool has one"
    };
}
#[cfg(test)]
const CARD_MESSAGE_HINT: &str = card_message_hint!();

const QUOTED_READ_CHARS: usize = 600;

/// INVARIANT: What every refusal says the user read since their last input: the words
/// themselves, or that they read nothing. The agent's own context holds more
/// than that, since the API hands it back its hidden reasoning.
fn what_they_read(words: &str) -> String {
    match words.trim() {
        "" => "they have read nothing from you".to_string(),
        words => match words.char_indices().nth(QUOTED_READ_CHARS) {
            Some((cut, _)) => format!(
                "what they have read from you is longer than fits here. It begins: \"{}...\"",
                &words[..cut]
            ),
            None => format!("all they have read from you is this: \"{words}\""),
        },
    }
}

fn send_again_unchanged(words: &str) -> &'static str {
    if words.trim().is_empty() {
        "If they need nothing more from you, send the same question again unchanged: it will not \
         be refused twice."
    } else {
        "If the words quoted above already give them everything, send the same question again \
         unchanged: it will not be refused twice."
    }
}

const NOTHING_NEW: &str = "Nothing new has arrived since. This is not a new message. Do not \
     ask the user to resend anything.";

const QUOTED_INPUT_CHARS: usize = 200;

const NOTE_SIZED_CHARS: usize = 600;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Refusal {
    OwesWords { input: LastInput, words: String },
    PointsAboveAtNothing { words: String },
    ArtifactNotShown { paths: Vec<String>, words: String },
    NoteShownAsSummary { note: String },
}

impl Refusal {
    pub(crate) fn text(&self) -> String {
        match self {
            Self::OwesWords { input, words } => {
                let owed = format!(
                    concat!(
                        "Since the user's last input, {seen}. You have not told them what your \
                         tools found. They read only your text and your cards, never your tool \
                         results or your reasoning, and nothing else you wrote reached them. Say \
                         what you found in plain prose now, or ",
                        card_message_hint!(),
                        ". If they asked something, answer it the same way. Then ask your \
                         question again, and keep everything already on the card, pictures and \
                         links included. {again}"
                    ),
                    seen = what_they_read(words),
                    again = send_again_unchanged(words)
                );
                match input.quoted() {
                    Some(quoted) => format!(
                        "{REFUSAL_MARKER} The user's last input is {quoted}. You already \
                         received it. {NOTHING_NEW} {owed}"
                    ),
                    None => format!("{REFUSAL_MARKER} {owed}"),
                }
            }
            Self::PointsAboveAtNothing { words } => format!(
                concat!(
                    "{REFUSAL_MARKER} Your card points at something \"above\", but since the \
                     user's last input, {seen}. Nothing else you drafted reached them. They never \
                     see your reasoning. Write out what the card refers to as your reply, or put \
                     it on the card itself: ",
                    card_message_hint!(),
                    ", or else in the question or the option descriptions. Then ask again. \
                     {again}"
                ),
                REFUSAL_MARKER = REFUSAL_MARKER,
                seen = what_they_read(words),
                again = send_again_unchanged(words)
            ),
            Self::NoteShownAsSummary { note } => format!(
                concat!(
                    "{REFUSAL_MARKER} The text you wrote just before this card was longer than \
                     a note, so the user saw only this summary of it: \"{note}\". Nothing else \
                     of it reached them. Text written before any tool call reaches them only \
                     as such a summary. Put what they need, such as the answer, steps, links or \
                     pictures, ",
                    card_message_hint!(),
                    ", or else in the question. Then ask again. If the summary already gives \
                     them everything, send the same question again unchanged: it will not be \
                     refused twice."
                ),
                REFUSAL_MARKER = REFUSAL_MARKER,
                note = quote(note)
            ),
            Self::ArtifactNotShown { paths, words } => {
                let paths = paths
                    .iter()
                    .map(|p| format!("`{p}`"))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!(
                    concat!(
                        "{REFUSAL_MARKER} {ARTIFACT_REFUSAL_LEAD} {paths}, and they cannot see \
                         it. Saving shows nothing. Since then, {seen}. Put the line `lucidos data \
                         write` printed ON the card. A picture's \
                         `![...]` line goes ",
                        card_message_hint!(),
                        ", or in the question. An option's picture goes in its `preview` or \
                         description, whichever your tool has. When the options look different, \
                         give each its own picture of only that option. The `[...](...)` link to any other file goes ",
                        card_message_hint!(),
                        " or in the question: an option shows a link as plain text. If they \
                         asked something, answer it in plain prose or ",
                        card_message_hint!(),
                        ". If you ran other tools, say what you found. Then ask again. If it \
                         is not meant for them, send the same question again unchanged: it will \
                         not be refused twice."
                    ),
                    REFUSAL_MARKER = REFUSAL_MARKER,
                    ARTIFACT_REFUSAL_LEAD = ARTIFACT_REFUSAL_LEAD,
                    paths = paths,
                    seen = what_they_read(words)
                )
            }
        }
    }
}

fn card_text(questions: &serde_json::Value) -> String {
    let input = serde_json::json!({ "questions": questions });
    let mut text = String::new();
    for q in crate::engine::question_parsing::parse_ask_user_question_inputs(&input) {
        text.push_str(&q.question);
        for o in &q.options {
            let fields = [Some(&o.label), o.description.as_ref(), o.preview.as_ref()];
            for field in fields.into_iter().flatten() {
                text.push('\n');
                text.push_str(&without_link_targets(field));
            }
        }
        text.push('\n');
    }
    text
}

fn longest_card_question(questions: &serde_json::Value) -> String {
    let input = serde_json::json!({ "questions": questions });
    crate::engine::question_parsing::parse_ask_user_question_inputs(&input)
        .into_iter()
        .map(|q| q.question)
        .max_by_key(|question| question.trim().chars().count())
        .unwrap_or_default()
}

fn says_above(card: &str) -> bool {
    card.to_lowercase()
        .replace("of the above", "")
        .split(|c: char| !c.is_alphanumeric())
        .any(|word| word == "above")
}

fn is_shown_artifact(path: &str) -> bool {
    path.starts_with("artifacts/")
}

fn is_picture(path: &str) -> bool {
    const EXTENSIONS: &[&str] = &["gif", "jpeg", "jpg", "png", "svg", "webp"];
    path.rsplit_once('.')
        .is_some_and(|(_, ext)| EXTENSIONS.iter().any(|e| ext.eq_ignore_ascii_case(e)))
}

fn shows_artifact(text: &str, path: &str) -> bool {
    let needs_image = is_picture(path);
    let bare_ok = !path.contains(char::is_whitespace);
    let targets = ["", "data/"].into_iter().flat_map(|prefix| {
        [
            bare_ok.then(|| (format!("]({prefix}{path}"), ")")),
            Some((format!("](<{prefix}{path}"), ">)")),
        ]
        .into_iter()
        .flatten()
    });
    targets.into_iter().any(|(start, close)| {
        text.match_indices(start.as_str()).any(|(at, _)| {
            let rest = &text[at + start.len()..];
            let rest = rest.strip_prefix('#').map_or(rest, |fragment| {
                fragment.trim_start_matches(|c: char| !matches!(c, ')' | '>') && !c.is_whitespace())
            });
            rest.starts_with(close) && target_kind(text, at) == Some(needs_image)
        })
    })
}

fn target_kind(text: &str, at: usize) -> Option<bool> {
    text[..at]
        .rfind('[')
        .map(|open| text[..open].ends_with('!'))
}

fn without_link_targets(field: &str) -> String {
    let mut out = String::with_capacity(field.len());
    let mut rest = field;
    while let Some(at) = rest.find("](") {
        let (head, target) = rest.split_at(at + 1);
        out.push_str(head);
        rest = if target_kind(rest, at) == Some(false) {
            target.find(')').map_or("", |close| &target[close + 1..])
        } else {
            out.push('(');
            &target[1..]
        };
    }
    out.push_str(rest);
    out
}

fn not_work() -> Vec<&'static str> {
    let mut names = vec![
        crate::llm::tool_names::ASK_USER_QUESTION,
        crate::llm::tool_names::TODO_WRITE,
        "TodoWrite",
    ];
    names.extend(crate::runtime::USER_QUESTION_TOOLS);
    names
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LastInput {
    None,
    Message(String),
    Picked,
    Typed(String),
}

impl LastInput {
    fn from_answer(answer: &serde_json::Value) -> Self {
        let typed_text = answer
            .get("text")
            .and_then(|t| t.as_str())
            .filter(|t| !t.trim().is_empty());
        match (answer.get("kind").and_then(|k| k.as_str()), typed_text) {
            (Some("FreeText" | "MultiSelected"), Some(text)) => Self::Typed(text.to_string()),
            _ => Self::Picked,
        }
    }

    fn quoted(&self) -> Option<String> {
        match self {
            Self::Typed(text) if !text.trim().is_empty() => Some(format!(
                "the text they typed into your card: \"{}\"",
                quote(text)
            )),
            Self::Message(text) if !text.trim().is_empty() => {
                Some(format!("this message: \"{}\"", quote(text)))
            }
            _ => None,
        }
    }
}

fn quote(text: &str) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    match line.char_indices().nth(QUOTED_INPUT_CHARS) {
        Some((cut, _)) => format!("{}...", &line[..cut]),
        None => line,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SinceLastInput {
    pub(crate) last_input: LastInput,
    pub(crate) unreported_tool_calls: i64,
    pub(crate) refused: bool,
    pub(crate) artifact_refused: bool,
    pub(crate) words: String,
    pub(crate) saved_artifacts: Vec<String>,
    pub(crate) card_has_message: bool,
    pub(crate) last_progress_note: Option<String>,
}

impl SinceLastInput {
    fn with_round_text(mut self, round_text: &str) -> Self {
        if round_text.trim().is_empty() {
            return self;
        }
        self.unreported_tool_calls = 0;
        if self.words.contains(round_text) {
            return self;
        }
        let overlap = round_text
            .char_indices()
            .map(|(i, _)| i)
            .chain([round_text.len()])
            .rev()
            .find(|&end| self.words.ends_with(&round_text[..end]))
            .unwrap_or(0);
        self.words.push_str(&round_text[overlap..]);
        self
    }

    fn with_round(self, round: ChatRound<'_>) -> Self {
        let mut since = self.with_round_text(round.text);
        since.last_progress_note = round
            .progress_notes
            .last()
            .filter(|note| round.text.trim_end().ends_with(note.trim()))
            .cloned();
        since
    }

    fn with_card_message(self, message: &str) -> Self {
        if message.trim().is_empty() {
            return self;
        }
        Self {
            card_has_message: true,
            ..self.with_round_text(message)
        }
    }

    fn with_card_question(mut self, question: &str) -> Self {
        if question.trim().chars().count() >= NOTE_SIZED_CHARS {
            self.unreported_tool_calls = 0;
        }
        self
    }
}

pub(crate) fn should_refuse(s: &SinceLastInput, card: &str) -> Option<Refusal> {
    if !s.artifact_refused {
        let unseen: Vec<String> = s
            .saved_artifacts
            .iter()
            .filter(|path| !shows_artifact(&s.words, path) && !shows_artifact(card, path))
            .cloned()
            .collect();
        if !unseen.is_empty() {
            return Some(Refusal::ArtifactNotShown {
                paths: unseen,
                words: s.words.trim().to_string(),
            });
        }
    }
    if s.refused {
        return None;
    }
    if let Some(note) = s
        .last_progress_note
        .as_ref()
        .filter(|_| !s.card_has_message)
    {
        return Some(Refusal::NoteShownAsSummary { note: note.clone() });
    }
    if s.unreported_tool_calls > 0 {
        return Some(Refusal::OwesWords {
            input: s.last_input.clone(),
            words: s.words.trim().to_string(),
        });
    }
    let words = s.words.trim();
    if s.card_has_message || words.chars().count() >= NOTE_SIZED_CHARS {
        return None;
    }
    says_above(card).then(|| Refusal::PointsAboveAtNothing {
        words: words.to_string(),
    })
}

/// INVARIANT: The refusal for the card `tool_use_id` is about to raise on `thread_id`,
/// or `None` to show it. `questions` is the tool's `questions` array,
/// `message` the chat card's `message`, and `round` whatever the chat round
/// wrote before the call, with the progress notes among it.
///
/// A card already shown always passes, so a crash-recovery re-POST is safe.
/// A query error shows the card: blocking a question on a DB blip helps nobody.
///
/// A permission request always passes. The engine writes that card, and it
/// names the act the agent's ask-permission call prepared. `questions` must be
/// the ask as the agent sent it, before the engine's card replaced it.
pub(crate) async fn refuse_card(
    pool: &PgPool,
    thread_id: Uuid,
    tool_use_id: &str,
    questions: &serde_json::Value,
    message: Option<&str>,
    round: Option<ChatRound<'_>>,
) -> Option<Refusal> {
    if crate::engine::permission::prepare::names_permission_request(questions) {
        return None;
    }
    match read_since_last_input(pool, thread_id, tool_use_id).await {
        Ok(Some(since)) => {
            let since = match round {
                Some(round) => since.with_round(round),
                None => since,
            };
            let since = since
                .with_card_message(message.unwrap_or_default())
                .with_card_question(&longest_card_question(questions));
            should_refuse(&since, &card_text(questions))
        }
        Ok(None) => None,
        Err(e) => {
            crate::log_legacy!(
                "[QuestionCardGate] thread={thread_id} query failed, showing the card: {e}"
            );
            None
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct ChatRound<'a> {
    pub(crate) text: &'a str,
    pub(crate) progress_notes: &'a [String],
}

pub(crate) async fn refuse_coding_agent_card(
    pool: &PgPool,
    thread_id: Uuid,
    tool_use_id: &str,
    questions: &serde_json::Value,
    text_persisted: impl std::future::Future<Output = ()>,
) -> Option<Refusal> {
    refuse_card(pool, thread_id, tool_use_id, questions, None, None).await?;
    text_persisted.await;
    refuse_card(pool, thread_id, tool_use_id, questions, None, None).await
}

async fn read_since_last_input(
    pool: &PgPool,
    thread_id: Uuid,
    tool_use_id: &str,
) -> Result<Option<SinceLastInput>, sqlx::Error> {
    type Row = (
        bool,
        Option<String>,
        Option<serde_json::Value>,
        Option<String>,
        i64,
        bool,
        bool,
        Option<String>,
        Vec<String>,
        Option<String>,
    );
    let row: Row = sqlx::query_as(
        "WITH last_input AS ( \
               SELECT sequence, created, event_type, payload->'answer' AS answer, \
                 payload->>'text' AS text FROM events \
               WHERE thread_id = $1 \
                 AND event_type IN ('MessageReceived', 'PromptInjected', 'UserQuestionAnswered') \
               ORDER BY sequence DESC LIMIT 1 \
             ), since AS ( \
               SELECT sequence, event_type, payload FROM events \
               WHERE thread_id = $1 \
                 AND sequence > COALESCE((SELECT sequence FROM last_input), 0) \
                 AND event_type IN ('TextStreamed', 'CodingAgentTextStreamed', 'ToolCalled', \
                   'CodingAgentToolCalled', 'ToolResult', 'CodingAgentToolResult') \
             ), last_words AS ( \
               SELECT MAX(sequence) AS sequence FROM since \
               WHERE event_type IN ('TextStreamed', 'CodingAgentTextStreamed') \
                 AND btrim(payload->>'text', E' \\t\\r\\n') <> '' \
             ) \
             SELECT \
               EXISTS (SELECT 1 FROM events WHERE thread_id = $1 \
                 AND event_type = 'UserQuestionAsked' \
                 AND starts_with(payload->>'tool_use_id', $2 || '#')), \
               (SELECT event_type FROM last_input), \
               (SELECT answer FROM last_input), \
               (SELECT text FROM last_input), \
               (SELECT COUNT(*) FROM since \
                 WHERE event_type IN ('ToolCalled', 'CodingAgentToolCalled') \
                   AND NOT (payload->>'name' = ANY($3)) \
                   AND sequence > COALESCE((SELECT sequence FROM last_words), 0)), \
               EXISTS (SELECT 1 FROM since \
                 WHERE event_type IN ('ToolResult', 'CodingAgentToolResult') \
                   AND strpos(left(payload->>'result', $5), $4) > 0), \
               EXISTS (SELECT 1 FROM since \
                 WHERE event_type IN ('ToolResult', 'CodingAgentToolResult') \
                   AND strpos(left(payload->>'result', $5), $6) > 0), \
               (SELECT string_agg(payload->>'text', '' ORDER BY sequence) FROM since \
                 WHERE event_type IN ('TextStreamed', 'CodingAgentTextStreamed')), \
               (SELECT COALESCE(array_agg(DISTINCT payload->'data'->>'path'), '{}') FROM events \
                 WHERE event_type = 'DataFileWritten' \
                   AND created >= COALESCE((SELECT created FROM last_input) - interval '1 minute', \
                     '-infinity'::timestamptz) \
                   AND sequence > COALESCE((SELECT sequence FROM last_input), 0) \
                   AND payload->'data'->'actor'->>'source_thread_id' = $1::text), \
               (SELECT CASE WHEN COALESCE((payload->>'progress_note')::boolean, false) \
                   THEN btrim(payload->>'text', E' \\t\\r\\n') END FROM since \
                 WHERE event_type = 'CodingAgentTextStreamed' \
                   AND payload->>'parent_tool_use_id' IS NULL \
                   AND btrim(payload->>'text', E' \\t\\r\\n') <> '' \
                 ORDER BY sequence DESC LIMIT 1)",
    )
    .bind(thread_id)
    .bind(tool_use_id)
    .bind(not_work())
    .bind(REFUSAL_MARKER)
    .bind(REFUSAL_WINDOW_CHARS)
    .bind(format!("{REFUSAL_MARKER} {ARTIFACT_REFUSAL_LEAD}"))
    .fetch_one(pool)
    .await?;
    let (
        already_shown,
        input_type,
        answer,
        message,
        unreported_tool_calls,
        refused,
        artifact_refused,
        words,
        saved,
        last_progress_note,
    ) = row;
    if already_shown {
        return Ok(None);
    }
    let last_input = match (input_type.as_deref(), answer) {
        (None, _) => LastInput::None,
        (Some("UserQuestionAnswered"), Some(answer)) => LastInput::from_answer(&answer),
        _ => LastInput::Message(message.unwrap_or_default()),
    };
    Ok(Some(SinceLastInput {
        last_input,
        unreported_tool_calls,
        refused,
        artifact_refused,
        words: words.unwrap_or_default(),
        saved_artifacts: saved.into_iter().filter(|p| is_shown_artifact(p)).collect(),
        card_has_message: false,
        last_progress_note,
    }))
}

#[cfg(test)]
#[path = "question_card_gate_tests.rs"]
mod tests;
