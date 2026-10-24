//! The working understanding: the model's picture of the job, written as
//! ordinary text in its own reply and rendered back at the tail of every round.
//!
//! It is the only thing that outlives a turn, and where it is written is the
//! point. A tool call costs a whole round, and text beside the next action
//! costs nothing, so it rides in a marked span of the reply.
//!
//! **The parse reads assistant text only.** The engine renders the document
//! back inside the user message. Scoping the parse to the model's own turn is
//! what stops the engine reading its own rendering as a write.
//!
//! Three sections travel inside the span and only two are stored. `[TODO]` goes
//! to the projection: the engine writes `waiting` and `abandoned` there, and a
//! stored copy of the model's own text could never show them. `[KEEP OPEN]` is
//! applied and dropped. A stored keep would be re-read as a fresh one every
//! round, which is the standing keep ADR 0085 recorded as a design error.

use std::collections::HashSet;
use std::ops::Range;

use crate::engine::thread_events::{TodoItem, TodoStatus};
use crate::engine::tools::todo::MAX_TODO_ITEMS;

/// What follows replaces the document. Also what the engine's own rendered
/// block opens with, so the model meets one heading rather than two.
pub(crate) const OPEN_REPLACE: &str = "[WORKING UNDERSTANDING]";
/// What follows is appended to it.
pub(crate) const OPEN_ADD: &str = "[WORKING UNDERSTANDING: ADD]";
/// Ends either one.
pub(crate) const CLOSE: &str = "[/WORKING UNDERSTANDING]";

/// The shared opening of both forms, and of the engine's own rendered block.
/// Enough to suppress a live stream on, and what [`strip_faulted_markup`] cuts
/// from a reply the parse could not read.
pub(crate) const MARKER_PREFIX: &str = "[WORKING UNDERSTANDING";

/// The role whose text the parse reads.
///
/// Named rather than spelled at the call site, because the whole of invariant
/// 12 is that no other role reaches [`parse_message`].
pub(crate) const ASSISTANT_ROLE: &str = "assistant";

const CONSTRAINTS_HEADING: &str = "[CONSTRAINTS]";
const TODO_HEADING: &str = "[TODO]";
const KEEP_OPEN_HEADING: &str = "[KEEP OPEN]";

/// Past this size the block's own header asks for a rewrite. Nothing is
/// refused and nothing is truncated.
///
/// Soft rather than the hard cap the scratchpad had. Under an append a refusal
/// forces a full rewrite at a moment the model did not choose. The number stays
/// high on purpose: the guidance now tells the model to paste raw content in,
/// and one compiler error runs to about a thousand chars by itself.
pub(crate) const ASK_TO_REWRITE_ABOVE_CHARS: usize = 8_000;

/// What a superseded span in the model's own reply collapses to.
pub(crate) const SUPERSEDED_SPAN: &str =
    "[an earlier version of your working understanding. The current one is below.]";

/// Which of the two forms an opening marker named.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Form {
    /// Replace the document.
    Replace,
    /// Append to it.
    Add,
}

/// One marked span, lifted out of a reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Span {
    pub(crate) form: Form,
    /// Everything before the first section heading.
    pub(crate) body: String,
    /// Present only when the span carried a `[CONSTRAINTS]` heading.
    pub(crate) constraints: Option<String>,
    /// Present only when the span carried a `[TODO]` heading.
    pub(crate) todo: Option<Vec<TodoItem>>,
    /// Addresses written under `[KEEP OPEN]`, as the model wrote them.
    pub(crate) keep_open: Vec<String>,
    /// Where the span sits in the reply, both markers included.
    pub(crate) at: Range<usize>,
    /// Whether the reply carried the closing marker. An unclosed span runs to
    /// the end of the text, so on a cut reply it holds however far the model
    /// had got. See [`ParsedReply::drop_unclosed`].
    pub(crate) closed: bool,
}

/// Every span in one reply, in the order they were written.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ParsedReply {
    pub(crate) spans: Vec<Span>,
    /// What the engine could not read, one line each. Reported in the next
    /// round's framing rather than swallowed.
    pub(crate) faults: Vec<String>,
}

impl ParsedReply {
    pub(crate) fn wrote_something(&self) -> bool {
        !self.spans.is_empty()
    }

    /// Drop what a cut left half-written, so a Stop cannot rewrite the
    /// document.
    ///
    /// A block with no closing marker runs to the end of the text. When the
    /// model stopped on its own that is its own mistake, and the fault tells it
    /// so next round. When the engine did the cutting, the fragment is nothing
    /// anybody finished writing, and a whole-document REPLACE would overwrite
    /// weeks of notes with three words.
    ///
    /// The span is still cut from the text the user reads.
    /// [`strip_faulted_markup`] takes an unclosed block to the end, which is
    /// exactly the region the splice no longer covers.
    pub(crate) fn drop_unclosed(&mut self) {
        self.spans.retain(|span| span.closed);
    }
}

/// Whether the text a parse reads is all the model meant to write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReplyEnd {
    /// The model stopped on its own.
    Complete,
    /// The user pressed Stop, so the engine made the cut.
    Truncated,
}

/// The document as the thread holds it: the body and the constraints.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct WorkingUnderstanding {
    pub(crate) body: String,
    pub(crate) constraints: String,
}

impl WorkingUnderstanding {
    pub(crate) fn is_empty(&self) -> bool {
        self.body.trim().is_empty() && self.constraints.trim().is_empty()
    }

    pub(crate) fn chars(&self) -> usize {
        self.body.chars().count() + self.constraints.chars().count()
    }

    /// The one string the durable event carries.
    ///
    /// The constraints ride under their own heading, so the two halves come
    /// back apart. That is what lets an ADD accumulate each of them separately.
    /// An empty constraints section is left out rather than written blank: the
    /// render puts the heading back either way.
    pub(crate) fn to_document(&self) -> String {
        if self.constraints.trim().is_empty() {
            return self.body.trim().to_string();
        }
        format!(
            "{}\n\n{CONSTRAINTS_HEADING}\n{}",
            self.body.trim(),
            self.constraints.trim()
        )
    }

    /// Read a stored document back into its two halves.
    pub(crate) fn from_document(document: &str) -> Self {
        let mut body = String::new();
        let mut constraints = String::new();
        let mut in_constraints = false;
        for line in document.lines() {
            if !in_constraints && heading_of(line) == Some(Section::Constraints) {
                in_constraints = true;
                continue;
            }
            let target = if in_constraints {
                &mut constraints
            } else {
                &mut body
            };
            target.push_str(line);
            target.push('\n');
        }
        Self {
            body: body.trim().to_string(),
            constraints: constraints.trim().to_string(),
        }
    }
}

/// What applying a reply's spans produced, beside the new document.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Applied {
    /// The checklist to write, when a span carried one.
    pub(crate) todo: Option<Vec<TodoItem>>,
    /// Addresses to hold open, in the order written, deduplicated.
    pub(crate) keep_open: Vec<String>,
    /// What could not be read, one line each.
    pub(crate) faults: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Section {
    Body,
    Constraints,
    Todo,
    KeepOpen,
}

/// Parse one message's text, honouring the assistant-only rule.
///
/// The engine renders the document back inside the USER message, under the very
/// marker a replace-form span opens with. Scoping the parse by role is what
/// stops the engine reading its own rendering as a write. It is why the
/// document does not double every round.
pub(crate) fn parse_message(role: &str, text: &str) -> ParsedReply {
    if role != ASSISTANT_ROLE {
        return ParsedReply::default();
    }
    parse_reply(text)
}

/// Every marked span in the model's reply.
///
/// **A span runs to the next closing marker, or to the end of the reply.** The
/// rule is chosen on the unclosed case alone. Running to the end can
/// over-capture a closing sentence meant for the user, which the model sees
/// next round and rewrites. Stopping instead would lose the write outright,
/// with no error, so the model would never write those words again.
pub(crate) fn parse_reply(text: &str) -> ParsedReply {
    let mut parsed = ParsedReply::default();
    let mut at = 0usize;
    while let Some(found) = text[at..].find(MARKER_PREFIX) {
        let start = at + found;
        let rest = &text[start..];
        let (form, marker_len) = if rest.starts_with(OPEN_ADD) {
            (Form::Add, OPEN_ADD.len())
        } else if rest.starts_with(OPEN_REPLACE) {
            (Form::Replace, OPEN_REPLACE.len())
        } else {
            // An opening we could not read, such as a third form invented on
            // the spot. Named rather than skipped in silence: a write nobody
            // reports is one the model believes it made.
            parsed.faults.push(format!(
                "an opening marker I could not read at `{}`. Write `{OPEN_REPLACE}` or \
                 `{OPEN_ADD}`.",
                rest[..rest.floor_char_boundary(40)].trim_end()
            ));
            at = start + MARKER_PREFIX.len();
            continue;
        };
        let body_start = start + marker_len;
        let (body_end, span_end, closed) = match text[body_start..].find(CLOSE) {
            Some(offset) => (body_start + offset, body_start + offset + CLOSE.len(), true),
            None => {
                parsed.faults.push(format!(
                    "no `{CLOSE}` in your reply, so the block ran to the end of it. Anything \
                     after the opening marker is part of the document now."
                ));
                (text.len(), text.len(), false)
            }
        };
        let span = read_span(
            form,
            &text[body_start..body_end],
            start..span_end,
            closed,
            &mut parsed.faults,
        );
        parsed.spans.push(span);
        at = span_end;
    }
    parsed
}

/// Split one span's text into the body and its three marked sections.
fn read_span(
    form: Form,
    text: &str,
    at: Range<usize>,
    closed: bool,
    faults: &mut Vec<String>,
) -> Span {
    let mut section = Section::Body;
    let mut body = String::new();
    let mut constraints: Option<String> = None;
    let mut todo_lines: Option<Vec<String>> = None;
    let mut keep_lines: Vec<String> = Vec::new();

    for line in text.lines() {
        if let Some(found) = heading_of(line) {
            section = found;
            match section {
                Section::Constraints => {
                    constraints.get_or_insert_with(String::new);
                }
                Section::Todo => {
                    todo_lines.get_or_insert_with(Vec::new);
                }
                Section::KeepOpen | Section::Body => {}
            }
            continue;
        }
        if let Some(meant) = near_miss(line) {
            faults.push(format!(
                "a line reading `{}` on its own. Did you mean `{meant}`? A heading only counts \
                 inside its brackets.",
                line.trim()
            ));
        }
        match section {
            Section::Body => {
                body.push_str(line);
                body.push('\n');
            }
            Section::Constraints => {
                let into = constraints.get_or_insert_with(String::new);
                into.push_str(line);
                into.push('\n');
            }
            Section::Todo => {
                if !line.trim().is_empty() {
                    todo_lines
                        .get_or_insert_with(Vec::new)
                        .push(line.to_string());
                }
            }
            Section::KeepOpen => {
                if !line.trim().is_empty() {
                    keep_lines.push(line.to_string());
                }
            }
        }
    }

    Span {
        form,
        body: body.trim().to_string(),
        constraints: constraints.map(|c| c.trim().to_string()),
        todo: todo_lines.map(|lines| read_checklist(&lines, faults)),
        keep_open: read_keeps(&keep_lines, faults),
        at,
        closed,
    }
}

/// The section a line opens, allowing the two obvious decorations.
fn heading_of(line: &str) -> Option<Section> {
    match normalise_heading(line).as_str() {
        CONSTRAINTS_HEADING => Some(Section::Constraints),
        TODO_HEADING => Some(Section::Todo),
        KEEP_OPEN_HEADING => Some(Section::KeepOpen),
        _ => None,
    }
}

/// Strip a markdown heading level and bold marks, so `## [TODO]` and
/// `**[TODO]**` both count.
fn normalise_heading(line: &str) -> String {
    line.trim()
        .trim_start_matches('#')
        .trim()
        .trim_start_matches("**")
        .trim_end_matches("**")
        .trim()
        .trim_end_matches(':')
        .trim()
        .to_ascii_uppercase()
}

/// A bare heading word alone on a line, which the brackets exist to prevent.
///
/// A thread discussing this design writes `todo` and `constraints` constantly,
/// so the bare words are not headings. Saying so turns the collision into a
/// nudge, rather than a checklist built out of the model's prose.
fn near_miss(line: &str) -> Option<&'static str> {
    match normalise_heading(line).as_str() {
        "TODO" => Some(TODO_HEADING),
        "CONSTRAINTS" => Some(CONSTRAINTS_HEADING),
        "KEEP OPEN" => Some(KEEP_OPEN_HEADING),
        _ => None,
    }
}

/// The checklist, in the five marks one rule covers.
///
/// The model writes three. `waiting` and `abandoned` are engine-written at a
/// terminator and come back as words, so they parse too. The model rewrites the
/// list it was shown, and refusing what the engine printed would lose the rest
/// of the line with it.
fn read_checklist(lines: &[String], faults: &mut Vec<String>) -> Vec<TodoItem> {
    let mut items: Vec<TodoItem> = Vec::new();
    let mut in_progress = 0usize;
    for line in lines {
        let Some(mut item) = parse_todo_line(line) else {
            faults.push(format!(
                "a checklist line I could not read: `{}`. Write `- [ ]`, `- [>]` or `- [x]` and \
                 the item after it.",
                line.trim()
            ));
            continue;
        };
        if item.status == TodoStatus::InProgress {
            in_progress += 1;
            // At most one item is in progress, which is what the prompt bar
            // renders and what `todo_write` refuses outright. The DOCUMENT is
            // left as written. The projection has to stay a state the rest of
            // the engine can read, and the fault below says what happened.
            if in_progress > 1 {
                item.status = TodoStatus::Pending;
            }
        }
        items.push(item);
    }
    if in_progress > 1 {
        faults.push(format!(
            "{in_progress} items marked `- [>]`. One is in progress at a time, so the later ones \
             read as `- [ ]` until you say which."
        ));
    }
    if items.len() > MAX_TODO_ITEMS {
        faults.push(format!(
            "a checklist of {} items, and the cap is {MAX_TODO_ITEMS}. The first {MAX_TODO_ITEMS} \
             were taken.",
            items.len()
        ));
        items.truncate(MAX_TODO_ITEMS);
    }
    items
}

fn parse_todo_line(line: &str) -> Option<TodoItem> {
    let rest = line.trim().strip_prefix('-')?.trim_start();
    let (mark, content) = rest.strip_prefix('[')?.split_once(']')?;
    let status = match mark.trim().to_ascii_lowercase().as_str() {
        "" => TodoStatus::Pending,
        ">" => TodoStatus::InProgress,
        "x" => TodoStatus::Completed,
        "waiting" => TodoStatus::Waiting,
        "abandoned" => TodoStatus::Abandoned,
        _ => return None,
    };
    let content = content.trim();
    if content.is_empty() {
        return None;
    }
    Some(TodoItem {
        content: content.to_string(),
        // A text line carries one form of the item, so the present-continuous
        // form the prompt bar shows while it runs is the same words.
        active_form: content.to_string(),
        status,
    })
}

/// The addresses under `[KEEP OPEN]`, one per line.
///
/// A keep carries an address and nothing else. A trailing number would be a
/// duration, which asks the model to forecast again. That is the exact question
/// it could not answer.
fn read_keeps(lines: &[String], faults: &mut Vec<String>) -> Vec<String> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut keeps: Vec<String> = Vec::new();
    for line in lines {
        let written = line
            .trim()
            .trim_start_matches('-')
            .trim()
            .trim_start_matches('*')
            .trim();
        let first = written.split_whitespace().next().unwrap_or("");
        // An address is an identifier, so the model reaches for the decorations
        // an identifier usually wears. The panel prints them bare, and refusing
        // a backticked one would lose the mode's only curation lever to a habit.
        let candidate =
            first.trim_matches(|c| matches!(c, ',' | '.' | ';' | '`' | '*' | '[' | ']'));
        match crate::core::store::parse_event_address(candidate) {
            Some(id) => {
                let address = crate::engine::chat::process::context_mode::event_address(id);
                if seen.insert(address.clone()) {
                    keeps.push(address);
                }
                if written.split_whitespace().count() > 1 {
                    faults.push(format!(
                        "`{}` carries more than an address, and only the address was read. A keep \
                         takes no duration.",
                        written.trim_end()
                    ));
                }
            }
            None => faults.push(format!(
                "`{}` is not an address. A keep takes the whole evt-<hex> form the panel prints.",
                written.trim_end()
            )),
        }
    }
    keeps
}

/// Apply a reply's spans, in order, each with its own form.
///
/// Under an ADD the body and the constraints accumulate, while the checklist
/// replaces. Ticking a box means rewriting the list, so last occurrence wins
/// there. Adding one constraint must not silently drop the rest, so those build
/// up. A REPLACE sets each half the span actually named.
///
/// **A section absent from the span is not written.** A rewrite that says
/// nothing about the checklist leaves it standing, and one that says nothing
/// about the constraints leaves those. Clearing what the user can see is not
/// what silence about it means.
///
/// **A span with no body of its own replaces nothing.** The guidance's own
/// `[KEEP OPEN]` and `[TODO]` examples are a marker and a heading, with no
/// prose above them. Read as a whole-document rewrite, one of those would wipe
/// twenty rounds of notes the moment the model ticked a box.
pub(crate) fn apply_spans(
    document: &WorkingUnderstanding,
    parsed: &ParsedReply,
) -> (WorkingUnderstanding, Applied) {
    let mut next = document.clone();
    let mut applied = Applied {
        faults: parsed.faults.clone(),
        ..Applied::default()
    };
    let mut seen: HashSet<String> = HashSet::new();
    for span in &parsed.spans {
        match span.form {
            // Each half replaces on its own, and a half the span never named
            // stands. Replacing the constraints from an absent heading would
            // wipe them on the one form most likely to omit it, a whole
            // rewrite. Appending instead would stack a repeated constraint
            // copy by copy.
            Form::Replace => {
                if !span.body.trim().is_empty() {
                    next.body.clone_from(&span.body);
                }
                if let Some(replacement) = &span.constraints {
                    next.constraints.clone_from(replacement);
                }
            }
            Form::Add => {
                next.body = join_nonempty(&next.body, &span.body);
                if let Some(added) = &span.constraints {
                    next.constraints = join_nonempty(&next.constraints, added);
                }
            }
        }
        if let Some(todo) = &span.todo {
            applied.todo = Some(todo.clone());
        }
        for address in &span.keep_open {
            if seen.insert(address.clone()) {
                applied.keep_open.push(address.clone());
            }
        }
    }
    (next, applied)
}

fn join_nonempty(head: &str, tail: &str) -> String {
    match (head.trim(), tail.trim()) {
        ("", tail) => tail.to_string(),
        (head, "") => head.to_string(),
        (head, tail) => format!("{head}\n{tail}"),
    }
}

/// The reply with every marked span removed.
///
/// What is left is what was addressed to the user. Nothing of the document
/// reaches the chat: not the live stream, not `ResponseGenerated`.
///
/// The N-span sibling of `inline_question_repair::splice_out`, which cuts one
/// range. Named apart so a grep for either returns one function.
pub(crate) fn splice_spans_out(text: &str, spans: &[Span]) -> String {
    replace_spans(text, spans, "")
}

/// The reply with leftover markup no span claimed removed.
///
/// Runs after the splice, over what it left. An opening the parse could not
/// read, such as `[WORKING UNDERSTANDING: REPLACE]`, produces a fault and no
/// span. The splice then has nothing to cut, and the whole block reaches the
/// user.
///
/// The cut runs to the next [`CLOSE`], or to the end of the text when there is
/// none. That is the rule [`parse_reply`] already applies to a span it could
/// read, and it is chosen the same way: over-cutting shows the model its own
/// closing sentence went missing, while leaving the block shows the user
/// machinery addressed to nobody.
pub(crate) fn strip_faulted_markup(text: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    let mut at = 0usize;
    while let Some(found) = text[at..].find(MARKER_PREFIX) {
        let start = at + found;
        parts.push(text[at..start].trim());
        at = match text[start..].find(CLOSE) {
            Some(offset) => start + offset + CLOSE.len(),
            None => text.len(),
        };
    }
    parts.push(text[at..].trim());
    parts
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// The reply with every marked span collapsed to a one-line marker.
///
/// The model's reply holds its prose, its document and its next tool call
/// together, so a superseded copy is SPLICED. Overwriting the whole block would
/// take the words to the user and the record of the action with it.
pub(crate) fn fold_spans(text: &str) -> Option<String> {
    let parsed = parse_reply(text);
    if parsed.spans.is_empty() {
        return None;
    }
    // A marker longer than what it replaces would grow the request while
    // claiming to shrink it, which is the same guard `apply_stub` applies.
    let folded = replace_spans(text, &parsed.spans, SUPERSEDED_SPAN);
    (folded.len() < text.len()).then_some(folded)
}

fn replace_spans(text: &str, spans: &[Span], replacement: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    let mut at = 0usize;
    for span in spans {
        parts.push(text[at..span.at.start].trim());
        if !replacement.is_empty() {
            parts.push(replacement);
        }
        at = span.at.end;
    }
    parts.push(text[at..].trim());
    parts
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// What the round has to say about the document, beside the document itself.
#[derive(Debug, Clone, Default)]
pub(crate) struct RoundNotices {
    /// Everything the engine could not read, one line each.
    pub(crate) faults: Vec<String>,
    /// The addresses a keep held this round.
    pub(crate) held_open: Vec<String>,
}

/// The block the request carries at its tail, after the panel.
///
/// It renders on every round, touched or not. The constraints heading renders
/// empty or not, so an omission is visible every round rather than at the
/// moment it costs something.
pub(crate) fn render(
    document: &WorkingUnderstanding,
    todo: &[TodoItem],
    notices: &RoundNotices,
) -> String {
    let mut block = String::from(OPEN_REPLACE);
    block.push_str(
        "\nYour own picture of this job, in your own words. It is not an instruction, and it can \
         be wrong: a note outlives the result that made it.\n",
    );
    if document.chars() > ASK_TO_REWRITE_ABOVE_CHARS {
        block.push_str(&format!(
            "It is {} chars, which is long. Rewrite it whole on the round before the next \
             sweep.\n",
            document.chars()
        ));
    }
    if !notices.held_open.is_empty() {
        block.push_str(&format!(
            "Held open this round: {}.\n",
            notices.held_open.join(", ")
        ));
    }
    for fault in &notices.faults {
        block.push_str(&format!("Could not read: {fault}\n"));
    }
    block.push('\n');
    if document.body.trim().is_empty() {
        block.push_str("Nothing written yet.\n");
    } else {
        block.push_str(document.body.trim());
        block.push('\n');
    }
    block.push_str(&format!("\n{CONSTRAINTS_HEADING}\n"));
    if document.constraints.trim().is_empty() {
        block.push_str("Nothing stated yet.\n");
    } else {
        block.push_str(document.constraints.trim());
        block.push('\n');
    }
    block.push_str(&format!("\n{TODO_HEADING}\n"));
    if todo.is_empty() {
        block.push_str("Nothing on the list.\n");
    } else {
        for item in todo {
            block.push_str(&format!("- [{}] {}\n", mark_of(item.status), item.content));
        }
    }
    block.push_str(CLOSE);
    block
}

/// The mark a status renders as. Three the model writes, and two the engine
/// writes at a terminator, which come back as words.
fn mark_of(status: TodoStatus) -> &'static str {
    match status {
        TodoStatus::Pending => " ",
        TodoStatus::InProgress => ">",
        TodoStatus::Completed => "x",
        TodoStatus::Waiting => "waiting",
        TodoStatus::Abandoned => "abandoned",
    }
}

/// The block's own section name, in the prompt and in the capture.
pub(crate) const SECTION: &str = "Working Understanding";

/// The block's capture row, billed and grouped like the panel's.
pub(crate) fn capture_section(block: &str) -> crate::engine::ContextSection {
    super::context_panel::tail_block_section(SECTION, block)
}

/// The thread's newest document, or `None` before its first write.
///
/// Read from the event log, so it survives an engine restart with no in-memory
/// state to rescue.
pub(crate) async fn latest_document(
    pool: &sqlx::PgPool,
    thread_id: uuid::Uuid,
) -> Result<Option<WorkingUnderstanding>, sqlx::Error> {
    let row: Option<(serde_json::Value,)> = sqlx::query_as(
        "SELECT payload FROM events \
          WHERE thread_id = $1 AND event_type = 'WorkingUnderstandingWritten' \
          ORDER BY sequence DESC LIMIT 1",
    )
    .bind(thread_id)
    .fetch_optional(pool)
    .await?;
    Ok(row
        .and_then(|(payload,)| {
            payload
                .get("document")
                .and_then(|v| v.as_str())
                .map(WorkingUnderstanding::from_document)
        })
        .filter(|document| !document.is_empty()))
}

#[cfg(test)]
#[path = "working_understanding_tests.rs"]
mod tests;
