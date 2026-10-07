//! Refuses a question card when the agent owes the user an answer first.
//!
//! The user reads only the agent's text and its cards, never a tool result or
//! the agent's reasoning. So a card may not be the first thing they get after
//! a reply they typed into a card, or after tool work nobody reported. Nor may
//! it point "above" at words the user never read. One rule for the chat agent,
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
//! A model-tolerance measure: `docs/temporary-measures.md` § "A question card
//! with no answer before it". Plans:
//! `docs/plans/2026-09-23-a-card-never-replaces-the-answer.md`,
//! `docs/plans/2026-09-24-a-card-that-points-above-at-nothing.md` and
//! `docs/plans/2026-09-25-a-card-after-a-picture-nobody-saw.md`. The registry
//! entry records the widening to every artifact.

use sqlx::PgPool;
use uuid::Uuid;

/// Starts every refusal, and is how the query knows one was already sent.
const REFUSAL_MARKER: &str = "Question card not shown.";

/// Follows the marker in an artifact refusal, so the query can tell it apart.
const ARTIFACT_REFUSAL_LEAD: &str = "Since the user's last input you saved";

/// How far into a result the marker may sit. Codex wraps an MCP result in
/// JSON, which puts it about 40 chars in. Coding-agent results are stored
/// whole, so without a bound a grep quoting the marker would read as a refusal.
const REFUSAL_WINDOW_CHARS: i32 = 200;

/// Where a refusal sends content the user must read in full. Only the chat
/// tool has the field, so every agent reads the condition.
macro_rules! card_message_hint {
    () => {
        "in the card's `message` field if your tool has one"
    };
}
#[cfg(test)]
const CARD_MESSAGE_HINT: &str = card_message_hint!();

/// The tool result a card gets when the agent owes words, after the marker
/// and the quoted input. Every agent's notes between tool calls arrive as
/// text, so it asks for prose.
const OWES_WORDS: &str = concat!(
    "Since the user's last input you have written them nothing. They read only your text and \
     your cards, never your tool results or your reasoning. If they asked something, answer it \
     in plain prose now, or ",
    card_message_hint!(),
    ". If you ran tools, say what you found. Then ask your question again, and keep everything \
     already on the card, pictures and links included. If there is truly nothing to say, send \
     the same question again unchanged: it will not be refused twice."
);

/// A refusal that quotes the user's input reads like news of a new message.
/// An agent once asked the user to resend a reply it had already acted on.
const NOTHING_NEW: &str = "Nothing new has arrived since. This is not a new message. Do not \
     ask the user to resend anything.";

/// Long enough to recognise the input, short enough to stay one line.
const QUOTED_INPUT_CHARS: usize = 200;

/// More than any progress note, less than any reply a card could point at.
/// A model that thinks every turn shows its prose before a tool call only as
/// a note. The longest one measured was 441 characters, the shortest reply
/// before a card 1,075.
const NOTE_SIZED_CHARS: usize = 600;

/// Why a card is refused. Each reason carries its own tool result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// A typed reply got no words back, or tool work went unreported.
    /// `input` is the user's last input, quoted so it never reads as new.
    OwesWords { input: LastInput },
    /// The card points "above", and all the user read since their last input
    /// is `words`, too short to hold what it points at.
    PointsAboveAtNothing { words: String },
    /// The agent saved these artifacts since the user's last input, and
    /// neither its words nor the card show or link them.
    ArtifactNotShown { paths: Vec<String> },
    /// The user typed `reply`, and all they got back is progress notes,
    /// `words`. A note summarizes a draft and never carries it.
    OnlyNotes { reply: String, words: String },
}

impl Refusal {
    pub(crate) fn text(&self) -> String {
        match self {
            Self::OwesWords { input } => match input.quoted() {
                Some(quoted) => format!(
                    "{REFUSAL_MARKER} The user's last input is {quoted}. You already received \
                     it. {NOTHING_NEW} {OWES_WORDS}"
                ),
                None => format!("{REFUSAL_MARKER} {OWES_WORDS}"),
            },
            Self::PointsAboveAtNothing { words } => {
                let seen = match words.trim() {
                    "" => "they have read nothing from you".to_string(),
                    words => format!("all they have read from you is this: \"{words}\""),
                };
                format!(
                    concat!(
                        "{REFUSAL_MARKER} Your card points at something \"above\", but since \
                         the user's last input {seen}. Nothing else you drafted reached them. \
                         They never see your reasoning, and before a tool call your prose may \
                         reach them only as a short note. Write out what the card refers to as \
                         your reply, or put it on the card itself: ",
                        card_message_hint!(),
                        ", or else in the question or the option descriptions. Then ask again. \
                         If it truly is on their screen, send the same question again \
                         unchanged: it will not be refused twice."
                    ),
                    REFUSAL_MARKER = REFUSAL_MARKER,
                    seen = seen
                )
            }
            Self::ArtifactNotShown { paths } => {
                let paths = paths
                    .iter()
                    .map(|p| format!("`{p}`"))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!(
                    concat!(
                        "{REFUSAL_MARKER} {ARTIFACT_REFUSAL_LEAD} {paths}, and they cannot see \
                         it. Saving shows nothing. Words written just before a tool call may \
                         reach them only as a short summary, which drops a picture or a link. \
                         Put the line `lucidos data write` printed ON the card. A picture's \
                         `![...]` line goes ",
                        card_message_hint!(),
                        ", in the question, or in an option's `preview` if your tool has one. \
                         When the options look different, give each its own picture of only \
                         that option. The `[...](...)` link to any other file goes ",
                        card_message_hint!(),
                        " or in the question: an option shows a link as plain text. If they \
                         asked something, answer it in plain prose or ",
                        card_message_hint!(),
                        ". If you ran other tools, say what you found. Then ask again. If it \
                         truly is on their screen, or is not meant for them, send the same \
                         question again unchanged: it will not be refused twice."
                    ),
                    REFUSAL_MARKER = REFUSAL_MARKER,
                    ARTIFACT_REFUSAL_LEAD = ARTIFACT_REFUSAL_LEAD,
                    paths = paths
                )
            }
            Self::OnlyNotes { reply, words } => format!(
                concat!(
                    "{REFUSAL_MARKER} The user's last input is {}. You already received it and \
                     acted on it. {NOTHING_NEW} Since that reply, all they have read from you \
                     is this progress note: \"{}\". Before a tool call, your prose reaches them \
                     only as a short note that summarizes it. So a message, steps or a draft \
                     you wrote there reached nobody. Put what they must read on the card \
                     itself, ",
                    card_message_hint!(),
                    ", or else in the question: the card shows it in full. Then send your card \
                     again, with everything already on it. Never end your turn without it: \
                     a reply that asks for something else still leaves your questions open. \
                     If the note truly says it all, send the same question again unchanged: \
                     it will not be refused twice."
                ),
                typed_reply(reply),
                words.trim(),
                REFUSAL_MARKER = REFUSAL_MARKER,
                NOTHING_NEW = NOTHING_NEW
            ),
        }
    }
}

/// What the chat round wrote before its card. Only the chat loop knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RoundText<'a> {
    /// Text the user reads in full.
    Reply(&'a str),
    /// Only progress notes, each a short summary of what the model drafted.
    Notes(&'a str),
}

/// Everything the card renders, one field per line. `questions` is the
/// tool's `questions` array.
fn card_text(questions: &serde_json::Value) -> String {
    let input = serde_json::json!({ "questions": questions });
    let mut text = String::new();
    for q in crate::engine::agent_session::parse_ask_user_question_inputs(&input) {
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

/// The card's longest question text. Unlike an option, it reads like a reply.
/// Several short questions never add up to one.
fn longest_card_question(questions: &serde_json::Value) -> String {
    let input = serde_json::json!({ "questions": questions });
    crate::engine::agent_session::parse_ask_user_question_inputs(&input)
        .into_iter()
        .map(|q| q.question)
        .max_by_key(|question| question.trim().chars().count())
        .unwrap_or_default()
}

/// Whether `card` says "above" as a whole word. "None of the above" points
/// at the card's own options, so it never counts.
fn says_above(card: &str) -> bool {
    card.to_lowercase()
        .replace("of the above", "")
        .split(|c: char| !c.is_alphanumeric())
        .any(|word| word == "above")
}

/// A file saved as an artifact, the kind an agent saves for the user. An app
/// asset under `apps/` is saved for the app, not for the chat.
fn is_shown_artifact(path: &str) -> bool {
    path.starts_with("artifacts/")
}

/// The extensions mirror `is_image` in the CLI's `data.rs`, which prints a
/// picture's line as `![...]` and every other file's as a plain link.
fn is_picture(path: &str) -> bool {
    const EXTENSIONS: &[&str] = &["gif", "jpeg", "jpg", "png", "svg", "webp"];
    path.rsplit_once('.')
        .is_some_and(|(_, ext)| EXTENSIONS.iter().any(|e| ext.eq_ignore_ascii_case(e)))
}

/// Whether `text` holds a markdown target for `path`, as the CLI prints it or
/// with the leading `data/` the renderer also accepts. A picture needs the
/// image form, `![...](path)`: a plain link does not draw it. Any other file
/// needs a link, `[...](path)`. A bare path counts for neither. A path with a
/// space must be angle-bracketed, since markdown ends a bare target there. A
/// fragment such as the CLI's image size hint may follow the path.
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

/// For the `](` at byte `at` in `text`: `Some(true)` when it ends an image's
/// `![...]`, `Some(false)` when it ends a link's `[...]`, `None` when no `[`
/// opens it.
fn target_kind(text: &str, at: usize) -> Option<bool> {
    text[..at]
        .rfind('[')
        .map(|open| text[..open].ends_with('!'))
}

/// `field` as an option renders it. Inside the option's button a link shows
/// as its label alone, while a picture still draws. So each link loses its
/// target, and only the question can carry a link.
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

/// Tools whose calls are not work the user needs to hear about: the question
/// tools themselves, and the todo list.
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
    /// Nothing the user said is on the thread yet.
    None,
    /// A message or an injected follow-up, with its text.
    Message(String),
    /// A card answer that only picked options.
    Picked,
    /// A card answer carrying text the user typed.
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

    /// The input named and quoted, for a refusal. `None` when it has no text.
    fn quoted(&self) -> Option<String> {
        match self {
            Self::Typed(text) if !text.trim().is_empty() => Some(typed_reply(text)),
            Self::Message(text) if !text.trim().is_empty() => {
                Some(format!("this message: \"{}\"", quote(text)))
            }
            _ => None,
        }
    }
}

fn typed_reply(text: &str) -> String {
    format!("the reply they typed into your card: \"{}\"", quote(text))
}

/// `text` on one line, cut to [`QUOTED_INPUT_CHARS`].
fn quote(text: &str) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    match line.char_indices().nth(QUOTED_INPUT_CHARS) {
        Some((cut, _)) => format!("{}...", &line[..cut]),
        None => line,
    }
}

/// What happened on the thread since the user's last input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SinceLastInput {
    pub(crate) last_input: LastInput,
    pub(crate) spoke: bool,
    /// Tool calls after the agent's last words, or after the input if it said
    /// nothing. "Let me check the log" reports nothing the log said.
    pub(crate) unreported_tool_calls: i64,
    /// A card was refused since that input, for any reason.
    pub(crate) refused: bool,
    /// One of those refusals asked for an artifact.
    pub(crate) artifact_refused: bool,
    /// Everything the agent wrote since that input: all the user has read.
    pub(crate) words: String,
    /// Artifacts this thread saved through the data API since that input.
    pub(crate) saved_artifacts: Vec<String>,
    /// The chat round before the card wrote only progress notes.
    pub(crate) round_was_notes: bool,
    /// The card carries a `message`, which the user reads in full above it.
    pub(crate) card_has_message: bool,
}

impl SinceLastInput {
    /// Fold in the chat round's own text, which may not be in the events yet.
    /// Words written after the tool work report it.
    fn with_round_text(mut self, round: RoundText<'_>) -> Self {
        let round_text = match round {
            RoundText::Reply(text) | RoundText::Notes(text) => text,
        };
        if round_text.trim().is_empty() {
            return self;
        }
        self.round_was_notes = matches!(round, RoundText::Notes(_));
        self.spoke = true;
        self.unreported_tool_calls = 0;
        if self.words.contains(round_text) {
            return self;
        }
        // The persist task may have stored a prefix of the round. Counting it
        // twice would stretch a note past the threshold, so skip the overlap.
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

    /// Fold in the card's `message`, which the user reads as a reply shown
    /// just above the card. So the card may also point "above" at it.
    fn with_card_message(self, message: &str) -> Self {
        if message.trim().is_empty() {
            return self;
        }
        Self {
            card_has_message: true,
            ..self.with_round_text(RoundText::Reply(message))
        }
    }

    /// A card `question` longer than any note reports the tool work before
    /// it. It answers nothing the user typed: a re-sent card reads the same.
    fn with_card_question(mut self, question: &str) -> Self {
        if question.trim().chars().count() >= NOTE_SIZED_CHARS {
            self.unreported_tool_calls = 0;
        }
        self
    }
}

/// Refuse when a saved artifact shows nowhere. Or when a typed reply got no
/// words back, or tool work went unreported. Or when a typed reply got only
/// note-sized progress notes back. Or when the card points "above" at
/// note-sized words. A card carrying a `message` passes all but the first.
/// `card` is everything the card renders.
///
/// One refusal per input, plus one artifact refusal. The artifact goes first:
/// the work that made it is usually the unreported work. A retry after any
/// other refusal may drop an artifact the refused card showed.
pub(crate) fn should_refuse(s: &SinceLastInput, card: &str) -> Option<Refusal> {
    if !s.artifact_refused {
        let unseen: Vec<String> = s
            .saved_artifacts
            .iter()
            .filter(|path| !shows_artifact(&s.words, path) && !shows_artifact(card, path))
            .cloned()
            .collect();
        if !unseen.is_empty() {
            return Some(Refusal::ArtifactNotShown { paths: unseen });
        }
    }
    if s.refused {
        return None;
    }
    let unanswered = matches!(s.last_input, LastInput::Typed(_)) && !s.spoke;
    if unanswered || s.unreported_tool_calls > 0 {
        return Some(Refusal::OwesWords {
            input: s.last_input.clone(),
        });
    }
    let words = s.words.trim();
    if s.card_has_message || words.chars().count() >= NOTE_SIZED_CHARS {
        return None;
    }
    if let (LastInput::Typed(reply), true) = (&s.last_input, s.round_was_notes) {
        return Some(Refusal::OnlyNotes {
            reply: reply.clone(),
            words: words.to_string(),
        });
    }
    says_above(card).then(|| Refusal::PointsAboveAtNothing {
        words: words.to_string(),
    })
}

/// The refusal for the card `tool_use_id` is about to raise on `thread_id`,
/// or `None` to show it. `questions` is the tool's `questions` array,
/// `message` the chat card's `message`, and `round` whatever the chat round
/// wrote before the call.
///
/// A card already shown always passes, so a crash-recovery re-POST is safe.
/// A query error shows the card: blocking a question on a DB blip helps nobody.
pub(crate) async fn refuse_card(
    pool: &PgPool,
    thread_id: Uuid,
    tool_use_id: &str,
    questions: &serde_json::Value,
    message: Option<&str>,
    round: Option<RoundText<'_>>,
) -> Option<Refusal> {
    match read_since_last_input(pool, thread_id, tool_use_id).await {
        Ok(Some(since)) => {
            let since = match round {
                Some(round) => since.with_round_text(round),
                None => since,
            };
            let since = since
                .with_card_message(message.unwrap_or_default())
                .with_card_question(&longest_card_question(questions));
            should_refuse(&since, &card_text(questions))
        }
        Ok(None) => None,
        Err(e) => {
            crate::log!(
                "[QuestionCardGate] thread={thread_id} query failed, showing the card: {e}"
            );
            None
        }
    }
}

/// How long a coding agent's refusal waits before its second look.
const SECOND_LOOK: std::time::Duration = std::time::Duration::from_millis(500);

/// [`refuse_card`] for a coding agent's hook, which races the session loop
/// persisting the text written just before the card. A refusal needs two
/// looks, so a card that followed real text is never refused.
pub(crate) async fn refuse_coding_agent_card(
    pool: &PgPool,
    thread_id: Uuid,
    tool_use_id: &str,
    questions: &serde_json::Value,
) -> Option<Refusal> {
    refuse_card(pool, thread_id, tool_use_id, questions, None, None).await?;
    tokio::time::sleep(SECOND_LOOK).await;
    refuse_card(pool, thread_id, tool_use_id, questions, None, None).await
}

/// `None` when the card was already shown.
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
        bool,
        i64,
        bool,
        bool,
        Option<String>,
        Vec<String>,
    );
    // `DataFileWritten` has no thread_id: the saving thread is its actor. The
    // `created` bound only lets the index narrow the scan, with a minute of
    // slack for clock skew between transactions. `sequence` decides.
    let row: Row =
        sqlx::query_as(
            "WITH last_input AS ( \
               SELECT sequence, created, event_type, payload->'answer' AS answer, \
                 payload->>'text' AS text FROM events \
               WHERE thread_id = $1 \
                 AND event_type IN ('MessageReceived', 'UserPromptInjected', 'UserQuestionAnswered') \
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
               (SELECT sequence FROM last_words) IS NOT NULL, \
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
                   AND payload->'data'->'actor'->>'source_thread_id' = $1::text)",
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
        spoke,
        unreported_tool_calls,
        refused,
        artifact_refused,
        words,
        saved,
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
        spoke,
        unreported_tool_calls,
        refused,
        artifact_refused,
        words: words.unwrap_or_default(),
        saved_artifacts: saved.into_iter().filter(|p| is_shown_artifact(p)).collect(),
        round_was_notes: false,
        card_has_message: false,
    }))
}

#[cfg(test)]
#[path = "question_card_gate_tests.rs"]
mod tests;
