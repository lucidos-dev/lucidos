use std::collections::HashSet;
use std::ops::Range;

use crate::engine::thread_events::{TodoItem, TodoStatus};
use crate::engine::tools::todo::MAX_TODO_ITEMS;

pub(crate) const OPEN_REPLACE: &str = "[WORKING UNDERSTANDING]";
pub(crate) const OPEN_ADD: &str = "[WORKING UNDERSTANDING: ADD]";
pub(crate) const CLOSE: &str = "[/WORKING UNDERSTANDING]";

pub(crate) const MARKER_PREFIX: &str = "[WORKING UNDERSTANDING";

pub(crate) const ASSISTANT_ROLE: &str = "assistant";

const CONSTRAINTS_HEADING: &str = "[CONSTRAINTS]";
const TODO_HEADING: &str = "[TODO]";
const KEEP_OPEN_HEADING: &str = "[KEEP OPEN]";

/// INVARIANT: Past this size the block's own header asks for a rewrite. Nothing is
/// refused and nothing is truncated.
///
/// Soft rather than the hard cap the scratchpad had. Under an append a refusal
/// forces a full rewrite at a moment the model did not choose. The number stays
/// high on purpose: the guidance now tells the model to paste raw content in,
/// and one compiler error runs to about a thousand chars by itself.
pub(crate) const ASK_TO_REWRITE_ABOVE_CHARS: usize = 8_000;

pub(crate) const SUPERSEDED_SPAN: &str =
    "[an earlier version of your working understanding. The current one is below.]";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Form {
    Replace,
    Add,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Span {
    pub(crate) form: Form,
    pub(crate) body: String,
    pub(crate) constraints: Option<String>,
    pub(crate) todo: Option<Vec<TodoItem>>,
    pub(crate) keep_open: Vec<String>,
    pub(crate) at: Range<usize>,
    pub(crate) closed: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ParsedReply {
    pub(crate) spans: Vec<Span>,
    pub(crate) faults: Vec<String>,
}

impl ParsedReply {
    pub(crate) fn wrote_something(&self) -> bool {
        !self.spans.is_empty()
    }

    pub(crate) fn drop_unclosed(&mut self) {
        self.spans.retain(|span| span.closed);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReplyEnd {
    Complete,
    Truncated,
}

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

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Applied {
    pub(crate) todo: Option<Vec<TodoItem>>,
    pub(crate) keep_open: Vec<String>,
    pub(crate) faults: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Section {
    Body,
    Constraints,
    Todo,
    KeepOpen,
}

pub(crate) fn parse_message(role: &str, text: &str) -> ParsedReply {
    if role != ASSISTANT_ROLE {
        return ParsedReply::default();
    }
    parse_reply(text)
}

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

fn heading_of(line: &str) -> Option<Section> {
    match normalise_heading(line).as_str() {
        CONSTRAINTS_HEADING => Some(Section::Constraints),
        TODO_HEADING => Some(Section::Todo),
        KEEP_OPEN_HEADING => Some(Section::KeepOpen),
        _ => None,
    }
}

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

fn near_miss(line: &str) -> Option<&'static str> {
    match normalise_heading(line).as_str() {
        "TODO" => Some(TODO_HEADING),
        "CONSTRAINTS" => Some(CONSTRAINTS_HEADING),
        "KEEP OPEN" => Some(KEEP_OPEN_HEADING),
        _ => None,
    }
}

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
        active_form: content.to_string(),
        status,
    })
}

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

pub(crate) fn splice_spans_out(text: &str, spans: &[Span]) -> String {
    replace_spans(text, spans, "")
}

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

pub(crate) fn fold_spans(text: &str) -> Option<String> {
    let parsed = parse_reply(text);
    if parsed.spans.is_empty() {
        return None;
    }
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

#[derive(Debug, Clone, Default)]
pub(crate) struct RoundNotices {
    pub(crate) faults: Vec<String>,
    pub(crate) held_open: Vec<String>,
}

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

fn mark_of(status: TodoStatus) -> &'static str {
    match status {
        TodoStatus::Pending => " ",
        TodoStatus::InProgress => ">",
        TodoStatus::Completed => "x",
        TodoStatus::Waiting => "waiting",
        TodoStatus::Abandoned => "abandoned",
    }
}

pub(crate) const SECTION: &str = "Working Understanding";

pub(crate) fn capture_section(block: &str) -> crate::engine::ContextSection {
    super::context_panel::tail_block_section(SECTION, block)
}

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
