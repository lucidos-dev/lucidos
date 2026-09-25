//! Refuses a question card once when the agent owes the user an answer first.
//!
//! The user reads only the agent's text and its cards, never a tool result or
//! the agent's reasoning. So a card may not be the first thing they get after
//! a reply they typed into a card, or after tool work nobody reported. Nor may
//! it point "above" at words the user never read. One rule for the chat agent,
//! Claude Code and Codex, read from the thread's events so every agent sees
//! the same facts.
//!
//! Nor may it follow a picture the agent saved and the user cannot see.
//!
//! A model-tolerance measure: `docs/temporary-measures.md` § "A question card
//! with no answer before it". Plans:
//! `docs/plans/2026-09-23-a-card-never-replaces-the-answer.md`,
//! `docs/plans/2026-09-24-a-card-that-points-above-at-nothing.md` and
//! `docs/plans/2026-09-25-a-card-after-a-picture-nobody-saw.md`.

use sqlx::PgPool;
use uuid::Uuid;

/// Starts every refusal, and is how the query knows one was already sent.
const REFUSAL_MARKER: &str = "Question card not shown.";

/// The tool result a card gets when the agent owes words. Every agent's notes
/// between tool calls arrive as text, so it asks for prose.
const CARD_REFUSAL: &str = "Question card not shown. Since the user's last input you \
     have written them nothing. They read only your text and your cards, never your tool results \
     or your reasoning. If they asked something, answer it in plain prose now. If you ran tools, \
     say what you found. Then ask your question again. If there is truly nothing to say, send \
     the same question again unchanged: it will not be refused twice.";

/// More than any progress note, less than any reply a card could point at.
/// A model that thinks every turn shows its prose before a tool call only as
/// a note. The longest one measured was 441 characters, the shortest reply
/// before a card 1,075.
const NOTE_SIZED_CHARS: usize = 600;

/// Why a card is refused. Each reason carries its own tool result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// A typed reply got no words back, or tool work went unreported.
    OwesWords,
    /// The card points "above", and all the user read since their last input
    /// is `words`, too short to hold what it points at.
    PointsAboveAtNothing { words: String },
    /// The agent saved these pictures since the user's last input, and
    /// neither its words nor the card show them.
    PictureNotShown { paths: Vec<String> },
    /// The user typed a reply, and all they got back is progress notes,
    /// `words`. A note summarizes a draft and never carries it.
    OnlyNotes { words: String },
}

impl Refusal {
    pub(crate) fn text(&self) -> String {
        match self {
            Self::OwesWords => CARD_REFUSAL.to_string(),
            Self::PointsAboveAtNothing { words } => {
                let seen = match words.trim() {
                    "" => "they have read nothing from you".to_string(),
                    words => format!("all they have read from you is this: \"{words}\""),
                };
                format!(
                    "{REFUSAL_MARKER} Your card points at something \"above\", but since the \
                     user's last input {seen}. Nothing else you drafted reached them. They never \
                     see your reasoning, and before a tool call your prose may reach them only \
                     as a short note. Write out what the card refers to as your reply, or put it \
                     on the card itself, in the question or the option descriptions. Then ask \
                     again. If it truly is on their screen, send the same question again \
                     unchanged: it will not be refused twice."
                )
            }
            Self::PictureNotShown { paths } => {
                let paths = paths
                    .iter()
                    .map(|p| format!("`{p}`"))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!(
                    "{REFUSAL_MARKER} Since the user's last input you saved {paths}, and they \
                     cannot see it. Saving shows nothing. Words written just before a tool call \
                     may reach them only as a short summary, which drops a picture. Put its `![...]` line \
                     ON the card: in the question, or in an option's `preview` if your tool has \
                     one. When the options look different, give each its own picture of only \
                     that option. If they asked something, answer it in plain prose. If you ran \
                     other tools, say what you found. Then ask again. If it truly is on their screen, send the same question \
                     again unchanged: it will not be refused twice."
                )
            }
            Self::OnlyNotes { words } => format!(
                "{REFUSAL_MARKER} The user typed you a reply. Since then, all they have read \
                 from you is this progress note: \"{}\". Before a tool call, your prose reaches \
                 them only as a short note that summarizes it. So a message, steps or a draft \
                 you wrote there reached nobody. Write your answer as your reply and end your \
                 turn with no card: a reply reaches them in full. Or put it on the card itself. \
                 If the note truly says it all, send the same question again unchanged: it will \
                 not be refused twice.",
                words.trim()
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
                text.push_str(field);
            }
        }
        text.push('\n');
    }
    text
}

/// Whether `card` says "above" as a whole word. "None of the above" points
/// at the card's own options, so it never counts.
fn says_above(card: &str) -> bool {
    card.to_lowercase()
        .replace("of the above", "")
        .split(|c: char| !c.is_alphanumeric())
        .any(|word| word == "above")
}

/// A picture saved as an artifact, the kind an agent shows the user. An app
/// asset under `apps/` is saved for the app, not for the chat. The extensions
/// mirror `is_image` in the CLI's `data.rs`, which prints the `![...]` line.
fn is_shown_picture(path: &str) -> bool {
    const EXTENSIONS: &[&str] = &["gif", "jpeg", "jpg", "png", "svg", "webp"];
    path.starts_with("artifacts/")
        && path
            .rsplit_once('.')
            .is_some_and(|(_, ext)| EXTENSIONS.iter().any(|e| ext.eq_ignore_ascii_case(e)))
}

/// Whether `text` holds a markdown image, `![...](path)`, as the CLI prints
/// it or with the leading `data/` the renderer also accepts. A bare path or a
/// plain `[...](path)` link does not draw the picture, so neither counts.
fn shows_picture(text: &str, path: &str) -> bool {
    let targets = [
        format!("]({path})"),
        format!("](<{path}>)"),
        format!("](data/{path})"),
        format!("](<data/{path}>)"),
    ];
    targets.iter().any(|target| {
        text.match_indices(target.as_str()).any(|(at, _)| {
            text[..at]
                .rfind('[')
                .is_some_and(|open| text[..open].ends_with('!'))
        })
    })
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LastInput {
    /// Nothing the user said is on the thread yet.
    None,
    /// A message or an injected follow-up.
    Message,
    /// A card answer that only picked options.
    Picked,
    /// A card answer carrying text the user typed.
    Typed,
}

impl LastInput {
    fn from_answer(answer: &serde_json::Value) -> Self {
        let typed_text = answer
            .get("text")
            .and_then(|t| t.as_str())
            .is_some_and(|t| !t.trim().is_empty());
        match answer.get("kind").and_then(|k| k.as_str()) {
            Some("FreeText" | "MultiSelected") if typed_text => Self::Typed,
            _ => Self::Picked,
        }
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
    pub(crate) refused: bool,
    /// Everything the agent wrote since that input: all the user has read.
    pub(crate) words: String,
    /// Pictures this thread saved through the data API since that input.
    pub(crate) saved_pictures: Vec<String>,
    /// The chat round before the card wrote only progress notes.
    pub(crate) round_was_notes: bool,
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
}

/// Refuse when a saved picture shows nowhere. Or when a typed reply got no
/// words back, or tool work went unreported. Or when a typed reply got only
/// note-sized progress notes back. Or when the card points "above" at
/// note-sized words. Never twice for one input, so the picture goes first:
/// the work that made it is usually the unreported work, and a retry after
/// any other refusal passes without it. `card` is everything the card renders.
pub(crate) fn should_refuse(s: &SinceLastInput, card: &str) -> Option<Refusal> {
    if s.refused {
        return None;
    }
    let unseen: Vec<String> = s
        .saved_pictures
        .iter()
        .filter(|path| !shows_picture(&s.words, path) && !shows_picture(card, path))
        .cloned()
        .collect();
    if !unseen.is_empty() {
        return Some(Refusal::PictureNotShown { paths: unseen });
    }
    let unanswered = s.last_input == LastInput::Typed && !s.spoke;
    if unanswered || s.unreported_tool_calls > 0 {
        return Some(Refusal::OwesWords);
    }
    let words = s.words.trim();
    if words.chars().count() >= NOTE_SIZED_CHARS {
        return None;
    }
    if s.last_input == LastInput::Typed && s.round_was_notes {
        return Some(Refusal::OnlyNotes {
            words: words.to_string(),
        });
    }
    says_above(card).then(|| Refusal::PointsAboveAtNothing {
        words: words.to_string(),
    })
}

/// The refusal for the card `tool_use_id` is about to raise on `thread_id`,
/// or `None` to show it. `questions` is the tool's `questions` array, and
/// `round` whatever the chat round wrote before the call.
///
/// A card already shown always passes, so a crash-recovery re-POST is safe.
/// A query error shows the card: blocking a question on a DB blip helps nobody.
pub(crate) async fn refuse_card(
    pool: &PgPool,
    thread_id: Uuid,
    tool_use_id: &str,
    questions: &serde_json::Value,
    round: Option<RoundText<'_>>,
) -> Option<Refusal> {
    match read_since_last_input(pool, thread_id, tool_use_id).await {
        Ok(Some(since)) => {
            let since = match round {
                Some(round) => since.with_round_text(round),
                None => since,
            };
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
    refuse_card(pool, thread_id, tool_use_id, questions, None).await?;
    tokio::time::sleep(SECOND_LOOK).await;
    refuse_card(pool, thread_id, tool_use_id, questions, None).await
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
        bool,
        i64,
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
               SELECT sequence, created, event_type, payload->'answer' AS answer FROM events \
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
               (SELECT sequence FROM last_words) IS NOT NULL, \
               (SELECT COUNT(*) FROM since \
                 WHERE event_type IN ('ToolCalled', 'CodingAgentToolCalled') \
                   AND NOT (payload->>'name' = ANY($3)) \
                   AND sequence > COALESCE((SELECT sequence FROM last_words), 0)), \
               EXISTS (SELECT 1 FROM since \
                 WHERE event_type IN ('ToolResult', 'CodingAgentToolResult') \
                   AND strpos(payload->>'result', $4) > 0), \
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
        .fetch_one(pool)
        .await?;
    let (already_shown, input_type, answer, spoke, unreported_tool_calls, refused, words, saved) =
        row;
    if already_shown {
        return Ok(None);
    }
    let last_input = match (input_type.as_deref(), answer) {
        (None, _) => LastInput::None,
        (Some("UserQuestionAnswered"), Some(answer)) => LastInput::from_answer(&answer),
        _ => LastInput::Message,
    };
    Ok(Some(SinceLastInput {
        last_input,
        spoke,
        unreported_tool_calls,
        refused,
        words: words.unwrap_or_default(),
        saved_pictures: saved.into_iter().filter(|p| is_shown_picture(p)).collect(),
        round_was_notes: false,
    }))
}

#[cfg(test)]
#[path = "question_card_gate_tests.rs"]
mod tests;
