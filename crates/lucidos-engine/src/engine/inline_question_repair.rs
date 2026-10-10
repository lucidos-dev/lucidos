//! Defense against the chat model emitting the `ask_user_question` tool call as
//! inline text instead of as a structured tool call. The user then reads raw tag
//! characters and a JSON blob, and gets no clickable card. It has happened after
//! the `ASK_USER_QUESTION_RULE` instruction explicitly told the model "STOP if
//! about to type `<ask_user_question`", so prompt wording is not the fix.
//!
//! This is a **model-tolerance measure**, tracked in
//! `docs/temporary-measures.md` §2 under the `model-tool-call-as-text`
//! investigation. The generic `<invoke>` form of the same leak lives in
//! [`super::inline_tool_call_repair`]. Remove it when the model stops leaking.
//!
//! Three leak shapes are recognised, all normalised to the questions array:
//! a JSON array body, a single-key `{"questions": [...]}` object body, and an
//! unfenced trailing payload carrying that object with no tag at all.
//!
//! Two helpers live here:
//! - [`detect_inline_ask_user_question`], the pure function the agentic loop
//!   calls after the LLM returns its final text. See [`InlineQuestionLeak`] for
//!   the outcomes.
//! - [`buffer_contains_inline_tag`], a cheap substring check the streaming
//!   callback runs on every delta so it can stop flushing once the tag starts
//!   forming. That hides the raw tag from the live view. The post-response
//!   detector still runs and re-routes the question through
//!   `walk_question_batch`.

/// What the detector found.
#[derive(Debug, Clone)]
pub(crate) enum InlineQuestionLeak {
    /// A payload that parsed AND passed the same validation
    /// `walk_question_batch` applies. The caller synthesises a real tool call
    /// from it, so the user gets the clickable card the model meant to ask for.
    Dispatch {
        /// The questions array, ready to wrap as `{"questions": ...}` for
        /// `parse_ask_user_question_inputs`.
        questions_json: serde_json::Value,
        /// The assistant text minus the leaked block and any whitespace that
        /// would otherwise dangle.
        cleaned_text: String,
    },
    /// A wrapper tag whose body is not a dispatchable payload: prose, malformed
    /// JSON, or a payload the downstream walk would reject. The tag is stripped
    /// and the body kept as ordinary prose, so the user still reads the
    /// question and can answer by typing. The caller also forces a bounded
    /// re-ask, giving the model one chance to produce a real card.
    Degenerate {
        /// The assistant text with the tags removed and the body left in place.
        cleaned_text: String,
    },
}

const OPEN: &str = "<ask_user_question>";
const CLOSE: &str = "</ask_user_question>";

/// Scan `text` for a leaked `ask_user_question`.
///
/// The wrapper tag is tried first. With no tag outside a code region, an
/// unfenced payload sitting alone at the end of the response is tried instead.
///
/// Returns `None` when neither matches, and the caller falls back to normal
/// handling.
pub(crate) fn detect_inline_ask_user_question(text: &str) -> Option<InlineQuestionLeak> {
    match first_leaked_tag(text) {
        Some(open_at) => Some(detect_tagged(text, open_at)),
        None => detect_bare_payload(text),
    }
}

/// Where the first genuinely leaked opening tag starts. A tag inside a code
/// region is the model demonstrating the format, so it is skipped rather than
/// abandoning the scan: an explainer can quote the tag and still leak one.
fn first_leaked_tag(text: &str) -> Option<usize> {
    text.match_indices(OPEN)
        .map(|(at, _)| at)
        .find(|at| !inside_code(text, *at))
}

/// True when `at` sits inside a code region: all four markdown forms, since a
/// quoted example in any of them is prose we must not eat.
///
/// - A fence, backtick or tilde. An odd count before `at` leaves one open.
/// - An indented block: four spaces or a tab, and nothing else, ahead of `at`.
/// - An inline span. An odd backtick count earlier on the line leaves one open.
///
/// Approximate rather than CommonMark-exact, and every approximation errs
/// toward "code". Missing a leak costs one visible tag, while a false positive
/// deletes prose the user cannot get back.
fn inside_code(text: &str, at: usize) -> bool {
    let before = &text[..at];
    if before.matches("```").count() % 2 == 1 || before.matches("~~~").count() % 2 == 1 {
        return true;
    }
    let line_start = before.rfind('\n').map_or(0, |nl| nl + 1);
    let line_prefix = &before[line_start..];
    let indent_only =
        !line_prefix.is_empty() && line_prefix.bytes().all(|b| b == b' ' || b == b'\t');
    if indent_only && (line_prefix.contains('\t') || line_prefix.len() >= 4) {
        return true;
    }
    line_prefix.matches('`').count() % 2 == 1
}

/// Classify a wrapper tag known to start at `open_at`. Always a leak, because
/// the model typed the tool's own tag: the only question is whether its body
/// can be dispatched.
fn detect_tagged(text: &str, open_at: usize) -> InlineQuestionLeak {
    let body_start = open_at + OPEN.len();
    // An unterminated tag runs to the end of the text. The model still typed
    // the tag, so this is a leak; the body is whatever followed.
    let (body, close_end) = match text[body_start..].find(CLOSE) {
        Some(close_rel) => {
            let body_end = body_start + close_rel;
            (&text[body_start..body_end], body_end + CLOSE.len())
        }
        None => (&text[body_start..], text.len()),
    };
    let body = body.trim();
    match dispatchable_questions(body) {
        Some(questions_json) => InlineQuestionLeak::Dispatch {
            questions_json,
            cleaned_text: splice_out(text, open_at, close_end, ""),
        },
        // Keep the body: it is the only question the turn asked.
        None => InlineQuestionLeak::Degenerate {
            cleaned_text: splice_out(text, open_at, close_end, body),
        },
    }
}

/// Scan for a payload the model emitted with no wrapper tag at all, sitting
/// alone at the end of the response. Three anchors carry the false-positive
/// guard: the candidate runs to the end of the text, it is a single-key
/// `questions` object, and it is outside any code region.
///
/// **A fenced payload is deliberately NOT recovered.** The fence marks the JSON
/// as an illustration. Recovering it would delete the example from an answer
/// explaining the tool's own schema, which is the "do not swallow legitimate
/// prose" case. Unlike a wrong question card, that deletion is not something
/// the user can undo.
fn detect_bare_payload(text: &str) -> Option<InlineQuestionLeak> {
    let trimmed_end = text.trim_end();
    let (candidate_at, body) = trailing_json_object(trimmed_end)?;
    if inside_code(trimmed_end, candidate_at) {
        return None;
    }
    let questions_json = dispatchable_questions(body)?;
    Some(InlineQuestionLeak::Dispatch {
        questions_json,
        cleaned_text: splice_out(text, candidate_at, text.len(), ""),
    })
}

/// The EARLIEST line-anchored `{` from which the whole rest of `text` parses as
/// one JSON value, with its offset. Line-anchored, so a brace mid-sentence
/// cannot start a candidate. Earliest rather than last makes the match greedy:
/// a candidate that swallows trailing prose fails to parse and is skipped, so
/// the one that wins starts exactly where the trailing object starts.
fn trailing_json_object(text: &str) -> Option<(usize, &str)> {
    if !text.ends_with('}') {
        return None;
    }
    text.match_indices('{')
        .filter(|(at, _)| *at == 0 || text[..*at].ends_with('\n'))
        .map(|(at, _)| (at, &text[at..]))
        .find(|(_, body)| serde_json::from_str::<serde_json::Value>(body).is_ok())
}

/// Parse `body` and return the questions array, but only when the payload would
/// survive the walk. Accepts a bare JSON array, or an object whose ONLY
/// top-level key is `questions`. Returns `None` for anything else.
///
/// Validation is `walk_question_batch`'s own gate, deliberately: it needs at
/// least one question, each with non-empty `question` text. Synthesising a call
/// that gate would reject only turns a visible tag into an invisible tool
/// error, leaving the user with nothing at all.
fn dispatchable_questions(body: &str) -> Option<serde_json::Value> {
    let parsed: serde_json::Value = serde_json::from_str(body).ok()?;
    let questions = match parsed {
        array @ serde_json::Value::Array(_) => array,
        serde_json::Value::Object(mut map) if map.len() == 1 => map
            .remove("questions")
            .filter(serde_json::Value::is_array)?,
        _ => return None,
    };
    let parser_input = serde_json::json!({ "questions": questions });
    let walked = crate::engine::agent_session::parse_ask_user_question_inputs(&parser_input);
    if walked.is_empty() || walked.iter().any(|q| q.question.is_empty()) {
        return None;
    }
    Some(questions)
}

/// Replace `text[cut_from..cut_to]` with `replacement`, collapsing the
/// whitespace that would otherwise dangle where the block was.
fn splice_out(text: &str, cut_from: usize, cut_to: usize, replacement: &str) -> String {
    let before = text[..cut_from].trim_end();
    let after = text[cut_to..].trim_start();
    [before, replacement.trim(), after]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Cheap substring check the streaming callback runs on every token delta.
/// Returns `true` once the buffer holds the opening fragment of the tag. The
/// closing `>` is not required, so suppression starts before the body arrives.
/// Once true, the callback stops emitting `CumulativeTextUpdated` and
/// `TextStreamed` for the rest of the LLM turn, and the post-response repair
/// takes over.
///
/// Deliberately tag-only. A bare payload has no tag to match on. "Alone at the
/// end" is not decidable from a prefix, so guessing would kill live streaming
/// for a turn that merely discusses the schema. The bare form self-corrects at
/// the final flush, which emits the cleaned text.
///
/// Nor can it persist on the way. `should_flush` flushes only at a paragraph
/// boundary, and a payload alone at the end has no blank line after it. So no
/// `TextStreamed` delta ever carries one.
pub(crate) fn buffer_contains_inline_tag(buf: &str) -> bool {
    buf.contains("<ask_user_question")
}

#[cfg(test)]
#[path = "inline_question_repair_tests.rs"]
mod tests;
