use crate::engine::aux_purpose::AuxCall;
use crate::llm::judgment::{
    for_site, system_one_for, ChatJudgmentProvider, JudgmentProvider, JudgmentSite,
};
use crate::llm::provider::{LlmProvider, LlmResponse, Message, MessageContent};
use crate::memory::{query_judgment, RETRIEVAL_MIN_IMPORTANCE};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;

/// Bump when extractor logic changes in a way that produces materially
/// different facts from the same input — new prompt, new filter, new context
/// injection. Stamped on each `memory_entries` row by `index_entry`. Lets
/// `rebuild_memory(re_extract_stale=true)` re-extract entries written by an
/// older version without paying for a full rebuild.
pub const EXTRACTOR_VERSION: i32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractedFact {
    pub fact: String,
    pub importance: f32,
    pub topic: String,
    #[serde(default)]
    pub entities: Vec<String>,
}

const EXTRACTION_PROMPT: &str = r#"Extract discrete facts ABOUT THE USER from this content. Each fact must be a single,
self-contained statement about ONE topic. Never merge information from different
subjects into a single fact — if the conversation mentions family AND a project,
those are separate facts.

Each fact must include all specific names, file names, and identifiers so it is
understandable on its own without the entities list.

Focus on facts about the user's life, work, preferences, relationships, and activities.

CRITICAL — DISTINGUISH THE USER FROM OTHER PEOPLE:
Only extract facts about the user themselves. When other people are mentioned in
conversation, extract the user's RELATIONSHIP to that person — not facts about
the other person as if they were the user.
  (BAD: "Works as [role] at [other person's company]" — this is about someone ELSE, not the user)
  (GOOD: "Knows [person] who works as [role] at [company]")
  (BAD: "Lives in [city]" — if someone else lives there, not the user)
  (GOOD: "Has a colleague named [person] who lives in [city]")
If your system instructions identify the user by name, use that to disambiguate.
When in doubt whether a statement is about the user or someone else, skip it.

CRITICAL — ONLY EXTRACT FROM THE USER MESSAGE:
Your system instructions contain background context for disambiguation only.
Do NOT extract facts from your system instructions — only from the user message content below.
Do NOT use names, dates, employers, or any details from your system instructions in extracted facts.
If the user message does not mention a person's name, do NOT attribute actions to a name from system context.

CRITICAL — DISTINGUISH FACTS FROM RESEARCH:
If the content is prefixed with "File: research/..." or discusses job postings,
company analyses, salary comparisons, or "what if" scenarios, this is RESEARCH
about an external topic — NOT established facts about the user.
Extract the user's INTEREST or RESEARCH ACTIVITY, not the researched subject as fact.
  (BAD: "Works at [researched company]" — this is about the company being researched)
  (GOOD: "Researched [company] as a potential employer")
  (BAD: "Salary is [amount]" — this is from a job listing, not user's salary)
  (GOOD: "Compared salary range at [company] against current salary")
When content discusses hypothetical scenarios, job comparisons, or exploratory
analyses, extract the user's CONSIDERATION or EXPLORATION, not the hypothetical as fact.

DO NOT extract:
- File operations: creating, deleting, renaming, moving, or committing files. These are
  routine system operations, not meaningful facts about the user. The user doesn't need to
  "remember" that they deleted files or made commits.
- Meta-observations about the system's own file structure, storage, or internals
  (e.g., "data is stored in the artifacts folder", "files are saved in a directory", "changes are logged in a profile file")
- Observations about the document you're reading (e.g., "the profile contains sections for projects and preferences")
- Markdown formatting or heading structure
- Trivially obvious or vague facts that carry no real information
  (BAD: "The user uses a programming language for their project" — this says nothing useful)
  (BAD: "The project uses a database" — which database? be specific or omit)
  (GOOD: "Migrated the backend from SQLite to PostgreSQL for better concurrency")
- If you cannot state a SPECIFIC detail (a name, a version, a concrete choice), the fact is too vague to extract — skip it
- Do NOT generalize specific nouns into vague categories. Use the exact object mentioned in the source.
  (BAD: "Is charging a device that stopped charging" — what device?)
  (GOOD: "Is charging [specific device name] that stopped charging at [percentage]")
  If you don't know the specific noun, use whatever the source text said — never substitute a vaguer word.
- Do NOT invent descriptions or characterizations that aren't explicitly stated in the source text.
  Use the exact terminology the user used. If they call something "an event sourcing system", don't
  rephrase it as "a database app" or "a productivity tool" — use their words or omit the description entirely.

For each fact, provide:
- fact: specific statement including all relevant names and identifiers
  (BAD: "The file does not have a public URL" — which file?)
  (GOOD: "[filename] is a local artifact without a public URL")
  (BAD: "User profile is stored in a markdown file" — meta-observation about the system)
  (GOOD: "Works as [role] at [company]")
- importance: 0.0 to 1.0
  - 0.8-1.0: Life events, decisions, project milestones, key relationships
  - 0.5-0.7: Plans, opinions, preferences, notable activities
  - 0.2-0.4: Routine tasks, minor details
  - 0.0-0.1: Small talk, greetings, filler (omit these entirely)
- topic: short label, 2-4 words (e.g., "Habit Tracker", "Work Projects")
- entities: all proper nouns, names, project names, file names, identifiers,
  and specific terms mentioned. Use the exact form as written in the source text.

Return ONLY a JSON array, no markdown fences, no extra text. If there are no meaningful facts, return [].
Example: [{"fact": "Started the [project] migration to [technology]", "importance": 0.7, "topic": "Work Projects", "entities": ["[project]", "[technology]"]}]"#;

/// Chars the extraction instruction adds to every fact-extraction request,
/// before the content itself. Read by `core::aux_context_backfill` to size a
/// reconstruction of a call whose request was never recorded.
pub(crate) fn extraction_prompt_chars() -> usize {
    EXTRACTION_PROMPT.chars().count()
}

/// Decomposition into search queries.
///
/// A judgment provider answers the three booleans, so this prompt asks for the
/// one thing it cannot produce: new text. It runs only when memory is wanted.
const SUB_QUERY_PROMPT: &str = r#"Turn this user message into search queries over the user's long-term memory.
You will receive the current message and optionally a summary of the recent conversation for context.
Use the conversation context to understand the TOPIC being discussed, not just the latest message in isolation.

Write queries about the TOPIC or CONTENT being referenced, NOT about the action being requested. Strip away action verbs like "save", "summarize", "write" and focus on the subject matter. For example, "save research about Example Org" → ["Example Org", "Example Org company analysis"].

A BARE SUBJECT NAME IS A BAD QUERY when the workspace is largely about that subject: it matches thousands of entries equally and the results come back arbitrary. So when the message asks for a JUDGEMENT, an OPINION, a COMPARISON or ADVICE about something, query that subject's STATE and OUTCOME (progress, results, adoption, recent milestones, setbacks, what happened lately), never the name on its own. "should I give up on Example Project and do something else?" → ["Example Project recent progress", "Example Project launch outcome", "Example Project adoption and traction", "Example Project setbacks"], NOT ["Example Project"]. Queries about the decision itself ("job application", "career change") retrieve nothing useful: the facts that answer such a question are facts about how the subject is going.

Return ONLY a JSON array of strings, no markdown fences, no extra text. Return [] when no useful query exists.
Example: ["habit tracker progress", "weekly exercise routine"]"#;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryClassification {
    pub needs_memory: bool,
    pub needs_file_list: bool,
    pub needs_credentials: bool,
    pub sub_queries: Vec<String>,
}

impl Default for QueryClassification {
    fn default() -> Self {
        Self {
            needs_memory: true,
            needs_file_list: true,
            needs_credentials: true,
            sub_queries: vec![],
        }
    }
}

/// Single-shot LLM call against a routed provider.
///
/// Every memory chat call funnels through here, and `capture` makes and
/// records it.
async fn chat_with_provider(
    provider: &dyn LlmProvider,
    system: &str,
    user_content: &str,
    reasoning_effort: Option<&str>,
    capture: &crate::engine::AuxCapture,
) -> Result<LlmResponse, Box<dyn std::error::Error + Send + Sync>> {
    let messages = vec![Message {
        role: "user".to_string(),
        content: MessageContent::Text(user_content.to_string()),
    }];
    capture
        .chat(
            provider,
            messages,
            vec![],
            crate::llm::ModelSelection::default().with_effort(reasoning_effort),
            Some(system),
            None,
        )
        .await
}

/// Extract atomic facts from raw content.
/// Optional `context` provides background (system, user, conversation) so
/// the model can correctly identify implicit entities and assess importance.
/// `call` carries the resolved *model selection* and the call's budget.
pub(crate) async fn extract_facts(
    content: &str,
    context: Option<&str>,
    language: Option<&str>,
    call: &AuxCall,
    capture: &crate::engine::AuxCapture,
) -> Result<Vec<ExtractedFact>, Box<dyn std::error::Error + Send + Sync>> {
    let language_instruction = match language {
        Some(lang) if !lang.is_empty() => {
            format!("\n\nIMPORTANT: Write ALL extracted facts in {}.", lang)
        }
        _ => String::new(),
    };

    // System prompt: extraction instructions + background context (user profile, language).
    // User message: only the content to extract from.
    // This structural separation prevents the LLM from extracting facts from the
    // background context (e.g., user's employer from their profile).
    let system = match context {
        Some(ctx) => format!("{}{}\n\n{}", EXTRACTION_PROMPT, language_instruction, ctx),
        None => format!("{}{}", EXTRACTION_PROMPT, language_instruction),
    };
    let response = chat_with_provider(
        call.provider().as_ref(),
        &system,
        content,
        call.reasoning(),
        capture,
    )
    .await?;

    let raw = response.content.unwrap_or_default();
    let cleaned = strip_code_fences(&raw);

    // Parse via Value first — LLMs sometimes emit duplicate keys (e.g. "topic"
    // twice), which serde's derived Deserialize rejects but Value handles
    // with last-wins semantics.
    // TEMPORARY MEASURE — model-tolerance (removable; see
    // docs/temporary-measures.md § "Duplicate-key tolerant memory-extraction
    // parse", governed by .claude/rules/temporary-measures.md). Drop the
    // intermediate Value step and deserialize Vec<ExtractedFact> directly once
    // extraction responses reliably contain no duplicate keys.
    let value: serde_json::Value = serde_json::from_str(&cleaned)
        .map_err(|e| parse_failure("Failed to parse extraction JSON", e, &cleaned))?;
    let mut facts: Vec<ExtractedFact> = serde_json::from_value(value)
        .map_err(|e| parse_failure("Failed to deserialize extraction facts", e, &cleaned))?;

    // Drop facts that wouldn't survive RETRIEVAL_MIN_IMPORTANCE — pointless to embed.
    for fact in &mut facts {
        fact.importance = fact.importance.clamp(0.0, 1.0);
    }
    facts.retain(|f| {
        f.importance >= RETRIEVAL_MIN_IMPORTANCE && !is_fabricated_engine_internal_claim(&f.fact)
    });

    Ok(facts)
}

/// Classify a user query, and decompose it into search sub-queries when
/// memory is wanted and the caller reads queries. The Tree memory module
/// retrieves no recall, so it passes `wants_queries: false` and pays one call.
///
/// Optional `conversation_context` provides recent conversation summary so
/// the classifier can understand the topic (e.g. a follow-up "try again" in
/// an API conversation still needs credentials). `call` carries the
/// resolved *model selection* and the call's budget.
///
/// The three booleans are typed questions to the site's judgment provider:
/// the chat model `call` names, unless `judgment_query_classification`
/// picks a System One row (ADR 0363). A missing answer reads as yes.
///
/// **A message that needs memory costs two sequential calls, inside the
/// ONE deadline the caller wraps this in.** Each attempt is bounded, but two
/// can exceed that deadline where one would not. The caller then reads
/// `QueryClassification::default()`, which loads everything, so the
/// overrun costs latency rather than context.
pub(crate) async fn classify_query(
    pool: &PgPool,
    query: &str,
    conversation_context: Option<&str>,
    call: &AuxCall,
    wants_queries: bool,
    capture: &crate::engine::AuxCapture,
) -> Result<QueryClassification, Box<dyn std::error::Error + Send + Sync>> {
    let system_one = system_one_for(
        pool,
        JudgmentSite::QueryClassification,
        call.attempt_timeout(),
    )
    .await;
    let chat = ChatJudgmentProvider::new(call.provider(), call.reasoning().map(str::to_string));
    let judge = for_site(system_one, chat);
    let mut classification =
        judge_query(judge.as_ref(), query, conversation_context, capture).await?;
    if classification.needs_memory && wants_queries {
        classification.sub_queries =
            decompose_query(query, conversation_context, call, capture).await;
    }
    Ok(classification)
}

/// The three booleans from one judgment, with `sub_queries` left empty.
async fn judge_query(
    judge: &dyn JudgmentProvider,
    query: &str,
    conversation_context: Option<&str>,
    capture: &crate::engine::AuxCapture,
) -> Result<QueryClassification, Box<dyn std::error::Error + Send + Sync>> {
    let judgment = capture
        .judge(
            judge,
            query_judgment::state(query, conversation_context),
            query_judgment::questions(),
        )
        .await?;
    Ok(query_judgment::read(&judgment.answers))
}

/// Search queries for one message, or none when the model could not give
/// any.
///
/// Total on purpose. An empty list makes the retriever search the raw
/// message, which is what a workspace without decomposition already does.
/// Losing the turn over it would be the worse trade.
async fn decompose_query(
    query: &str,
    conversation_context: Option<&str>,
    call: &AuxCall,
    capture: &crate::engine::AuxCapture,
) -> Vec<String> {
    let system = match conversation_context {
        Some(ctx) if !ctx.is_empty() => {
            format!(
                "{}\n\nConversation context (recent messages): {}",
                SUB_QUERY_PROMPT, ctx
            )
        }
        _ => SUB_QUERY_PROMPT.to_string(),
    };
    let response = match chat_with_provider(
        call.provider().as_ref(),
        &system,
        query,
        call.reasoning(),
        capture,
    )
    .await
    {
        Ok(r) => r,
        Err(e) => {
            log!("[Memory] Sub-query decomposition failed: {}", e);
            return vec![];
        }
    };
    let cleaned = strip_code_fences(&response.content.unwrap_or_default());
    serde_json::from_str::<Vec<String>>(&cleaned).unwrap_or_else(|e| {
        log!(
            "[Memory] Could not read the sub-queries ({}): {}",
            e,
            cleaned
        );
        vec![]
    })
}

/// Summarize a thread's older ASSISTANT turns into one paragraph.
///
/// `turns` carries assistant turns only (ADR 0102). User turns stay
/// verbatim in the history block, so nothing the person said passes
/// through a model's judgment on its way to the prompt.
///
/// The caller caches the result as a `ConversationSummarized` event, so
/// this runs on a refresh rather than once per turn.
///
/// It takes a built `provider` rather than an `AuxCall`, because the caller
/// runs it in a detached task that outlives the turn.
pub(crate) async fn summarize_conversation(
    provider: &dyn LlmProvider,
    turns: &str,
    reasoning_effort: Option<&str>,
    capture: &crate::engine::AuxCapture,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let system = "Summarize these ASSISTANT turns for continuity with the ongoing conversation. \
        Focus on: (1) what is being worked on, (2) issues that were RESOLVED — explicitly mark them as fixed/done \
        so they are not revisited, (3) the current state of work at the end of this segment, \
        (4) dead ends and approaches that failed, so they are not retried. \
        CRITICAL: if a problem was reported and then fixed, say it \"was fixed\" — do NOT just describe the problem \
        without its resolution, as that causes the assistant to re-attempt already-completed fixes. \
        The user's own messages are NOT in this input and are supplied to the model verbatim elsewhere, \
        so do not try to reconstruct what they asked for. \
        Write concise flowing prose, no bullet points.";
    let response = chat_with_provider(provider, system, turns, reasoning_effort, capture).await?;
    let summary = response.content.unwrap_or_default().trim().to_string();
    Ok(summary)
}

/// Create a single fallback fact when structured extraction fails, or
/// `None` when the raw content isn't worth storing. The fallback bypasses
/// `extract_facts`' validation, so it must re-apply the same guard the
/// extractor uses: skip empty content and fabricated engine-internal
/// claims, which would otherwise smuggle past the filter that exists to
/// keep them out of memory. Truncates to 500 chars, importance 0.5.
pub fn fallback_fact(content: &str, topic: &str) -> Option<ExtractedFact> {
    let trimmed = content.trim();
    if trimmed.is_empty() || is_fabricated_engine_internal_claim(trimmed) {
        return None;
    }
    let chars: String = content.chars().take(500).collect();
    let truncated = if chars.len() < content.len() {
        format!("{}...", chars)
    } else {
        chars
    };

    Some(ExtractedFact {
        fact: truncated,
        importance: 0.5,
        topic: topic.to_string(),
        entities: vec![],
    })
}

/// True for facts asserting engine internals the chat agent cannot observe
/// (per-turn caps, tool-call counts, internal symbols). Filters at extract
/// time so memory rebuild can't recreate them.
pub(crate) fn is_fabricated_engine_internal_claim(text: &str) -> bool {
    // Normalize "tool-call(s)" → "tool call(s)" so we only branch on one form.
    let lower = text.to_lowercase().replace("tool-call", "tool call");

    // Internal SYMBOL names only. `max_iterations` is the pre-rename name and
    // stays listed so historical claims still filter on a memory rebuild;
    // `default_max_tool_calls` is what it became. Deliberately NOT the bare
    // `max_tool_calls`: that is a real user-facing preference key now, so "the
    // user set max_tool_calls to 2000" is a legitimate fact to remember.
    if lower.contains("max_iterations")
        || lower.contains("default_max_tool_calls")
        || lower.contains("agentic_loop.rs")
    {
        return true;
    }

    // Token-match framings so "unlimited" doesn't trigger "limit",
    // "capacity" doesn't trigger "cap", etc.
    const FRAMINGS: &[&str] = &["cap", "limit", "budget", "reached", "count", "exceeded"];
    let has_framing = || {
        lower
            .split(|c: char| !c.is_alphanumeric())
            .any(|w| FRAMINGS.contains(&w))
    };

    // "per turn" is rare in legitimate user facts — when paired with any
    // cap/limit framing it's almost always meta-commentary about the engine.
    if (lower.contains("per turn") || lower.contains("per-turn")) && has_framing() {
        return true;
    }

    // "tool call(s)" combined with cap/limit framing is meta-engine talk the
    // chat agent should not be indexing as fact about the user.
    if lower.contains("tool call") && has_framing() {
        return true;
    }

    false
}

/// Format a memory-parse failure, appending the first 200 chars of the raw
/// model output. Cut on a char boundary so multibyte output never panics.
fn parse_failure(context: &str, e: impl std::fmt::Display, cleaned: &str) -> String {
    format!(
        "[Memory] {}: {} | raw: {}",
        context,
        e,
        &cleaned[..cleaned.floor_char_boundary(200)]
    )
}

/// Strip markdown code fences from LLM response.
/// Flash often wraps JSON in ```json ... ``` blocks.
fn strip_code_fences(text: &str) -> String {
    let trimmed = text.trim();

    // Check for ```json or ``` at the start
    let without_opening = if let Some(rest) = trimmed.strip_prefix("```json") {
        rest
    } else if let Some(rest) = trimmed.strip_prefix("```") {
        rest
    } else {
        return trimmed.to_string();
    };

    // Remove closing ```
    let without_closing = if let Some(rest) = without_opening.trim().strip_suffix("```") {
        rest
    } else {
        without_opening
    };

    without_closing.trim().to_string()
}

#[cfg(test)]
mod capture_tests {
    use super::*;
    use crate::engine::event_bus::EventBus;
    use crate::test_support::{
        aux_captures, setup_test_db, teardown_test_db, JudgmentChatStub, ScriptedProvider,
    };
    use uuid::Uuid;

    /// `chat_with_provider` is the choke point every memory call funnels
    /// through, so testing it covers extraction, classification and
    /// summarization at once.
    #[tokio::test]
    async fn a_memory_call_records_its_own_purpose_and_usage() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();
        let capture =
            crate::engine::AuxCapture::new(&bus, thread_id, crate::engine::ContextPurpose::Memory);

        let provider =
            ScriptedProvider::new("gemini-3-flash-preview", vec!["[]"]).reporting(1_500, 20);
        chat_with_provider(&provider, "system", "content", Some("none"), &capture)
            .await
            .expect("scripted call succeeds");

        let captures = aux_captures(&pool, thread_id, "memory").await;
        assert_eq!(captures.len(), 1);
        assert_eq!(captures[0]["producer"], "auxiliary");
        assert_eq!(captures[0]["model"], "gemini-3-flash-preview");
        assert_eq!(captures[0]["usage"]["input_tokens"], 1_500);
        assert_eq!(captures[0]["usage"]["output_tokens"], 20);
        // The estimate counts both halves of what was sent.
        assert_eq!(
            captures[0]["sections"][0]["budget_delta_chars"],
            ("system".len() + "content".len()) as i64
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    /// Query classification on the default backend: typed questions to the
    /// chat model, its cost recorded under the site's own purpose.
    #[tokio::test]
    async fn query_classification_asks_the_chat_model_typed_questions() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();
        let capture = crate::engine::AuxCapture::new(
            &bus,
            thread_id,
            crate::engine::ContextPurpose::QueryClassification,
        );
        let stub = std::sync::Arc::new(JudgmentChatStub::answering(serde_json::json!({
            "needs_memory": 0.9,
            "needs_file_list": 0.1,
            "needs_credentials": 0.0,
        })));
        let chat = ChatJudgmentProvider::new(stub.clone(), Some("none".to_string()));

        let c = judge_query(
            &chat,
            "what did we decide last time?",
            Some("a talk about the habit tracker"),
            &capture,
        )
        .await
        .expect("answered");
        assert!(c.needs_memory);
        assert!(!c.needs_file_list);
        assert!(!c.needs_credentials);
        assert!(
            stub.messages()[0].contains("habit tracker"),
            "the context rides along"
        );

        let captures = aux_captures(&pool, thread_id, "query_classification").await;
        assert_eq!(captures.len(), 1, "one call, one row: {captures:?}");
        assert_eq!(captures[0]["usage"]["input_tokens"], 210);

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    /// The model calls one classification makes when memory is wanted.
    async fn classification_calls(wants_queries: bool) -> usize {
        let (pool, db_name) = setup_test_db().await;
        let stub = std::sync::Arc::new(JudgmentChatStub::answering(serde_json::json!({
            "needs_memory": 0.9,
            "needs_file_list": 0.1,
            "needs_credentials": 0.0,
        })));
        let call = AuxCall::over(
            stub.clone(),
            crate::engine::ContextPurpose::QueryClassification,
        );
        let c = classify_query(
            &pool,
            "what did we decide?",
            None,
            &call,
            wants_queries,
            &crate::engine::AuxCapture::discarding(
                crate::engine::ContextPurpose::QueryClassification,
            ),
        )
        .await
        .expect("answered");
        assert!(c.needs_memory);
        pool.close().await;
        teardown_test_db(&db_name).await;
        stub.messages().len()
    }

    /// The classic module searches with the queries, so it asks for them.
    #[tokio::test]
    async fn classic_classification_also_writes_search_queries() {
        assert_eq!(classification_calls(true).await, 2);
    }

    /// Tree retrieves no recall, so the second call would write queries
    /// nothing reads.
    #[tokio::test]
    async fn tree_classification_makes_one_call() {
        assert_eq!(classification_calls(false).await, 1);
    }

    /// An unreadable reply loads everything, never starves the turn.
    #[tokio::test]
    async fn an_unreadable_classification_loads_everything() {
        let chat = ChatJudgmentProvider::new(
            std::sync::Arc::new(JudgmentChatStub::replying("not sure")),
            None,
        );
        let capture = crate::engine::AuxCapture::discarding(
            crate::engine::ContextPurpose::QueryClassification,
        );
        let c = judge_query(&chat, "hi", None, &capture)
            .await
            .expect("a reply is not an error");
        assert_eq!(c, QueryClassification::default());
    }

    /// Artifact indexing and a thread-less event reach here with no thread.
    /// The call still runs, and records on the home thread.
    #[tokio::test]
    async fn a_memory_call_with_no_thread_records_on_the_home_thread() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = crate::engine::event_bus::EventBus::new(pool.clone());
        let capture = crate::engine::AuxCapture::for_thread_or_home(
            &bus,
            None,
            crate::engine::ContextPurpose::Memory,
        );

        let provider = ScriptedProvider::new("gemini-3-flash-preview", vec!["[]"]);
        chat_with_provider(&provider, "system", "content", Some("none"), &capture)
            .await
            .expect("scripted call succeeds");

        let home: Uuid = sqlx::query_scalar("SELECT thread_id FROM thread_summaries WHERE is_home")
            .fetch_one(&pool)
            .await
            .expect("a home thread to record on");
        assert_eq!(aux_captures(&pool, home, "memory").await.len(), 1);

        pool.close().await;
        teardown_test_db(&db_name).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_code_fences_json() {
        let input = "```json\n[{\"fact\": \"test\"}]\n```";
        assert_eq!(strip_code_fences(input), "[{\"fact\": \"test\"}]");
    }

    #[test]
    fn test_strip_code_fences_plain() {
        let input = "```\n[]\n```";
        assert_eq!(strip_code_fences(input), "[]");
    }

    #[test]
    fn test_strip_code_fences_no_fences() {
        let input = "[{\"fact\": \"hello\"}]";
        assert_eq!(strip_code_fences(input), input);
    }

    #[test]
    fn test_fallback_fact_short() {
        let fact = fallback_fact("Short content", "General").unwrap();
        assert_eq!(fact.fact, "Short content");
        assert_eq!(fact.importance, 0.5);
        assert_eq!(fact.topic, "General");
        assert!(fact.entities.is_empty());
    }

    #[test]
    fn test_fallback_fact_truncation() {
        let long_content = "a".repeat(600);
        let fact = fallback_fact(&long_content, "Test").unwrap();
        assert_eq!(fact.fact.len(), 503); // 500 chars + "..."
        assert!(fact.fact.ends_with("..."));
    }

    /// The fallback runs only after structured extraction fails, and it
    /// bypasses `extract_facts`' filtering — so empty/whitespace content must
    /// not be stored as a "fact".
    #[test]
    fn fallback_fact_rejects_empty_content() {
        assert!(fallback_fact("", "General").is_none());
        assert!(fallback_fact("   \n  ", "General").is_none());
    }

    /// The fallback must apply the same `is_fabricated_engine_internal_claim`
    /// guard `extract_facts` does, or a fabricated engine-internal claim
    /// slips into memory whenever structured extraction happens to fail.
    #[test]
    fn fallback_fact_rejects_fabricated_engine_internal_claim() {
        let claim = "The agentic loop hit its max_iterations cap after 25 tool calls";
        assert!(is_fabricated_engine_internal_claim(claim));
        assert!(fallback_fact(claim, "General").is_none());
    }

    #[test]
    fn test_extracted_fact_deserialization() {
        let json = r#"[{"fact": "Started project X", "importance": 0.8, "topic": "Work", "entities": ["ProjectX"]}]"#;
        let facts: Vec<ExtractedFact> = serde_json::from_str(json).unwrap();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].fact, "Started project X");
        assert_eq!(facts[0].importance, 0.8);
        assert_eq!(facts[0].topic, "Work");
        assert_eq!(facts[0].entities, vec!["ProjectX"]);
    }

    #[test]
    fn test_extracted_fact_empty_entities() {
        let json = r#"[{"fact": "Something happened", "importance": 0.5, "topic": "General"}]"#;
        let facts: Vec<ExtractedFact> = serde_json::from_str(json).unwrap();
        assert_eq!(facts.len(), 1);
        assert!(facts[0].entities.is_empty());
    }

    /// The evaluative-question failure, at the layer where it happens.
    ///
    /// A question of the shape "should I give up on Example Project and do
    /// something else?" decomposed to `["Example Project", "<the alternative>",
    /// "Example Project job application", "<the alternative> career"]`. The last
    /// two are about the frame rather than the subject, which the prompt already
    /// forbade. The first two obeyed it and were worse: in a workspace of tens of
    /// thousands of memory entries that are overwhelmingly about one project, a
    /// query of that project's bare name separates nothing, so the top 25 comes
    /// back arbitrary. The facts that would have answered the question were in
    /// memory the whole time.
    ///
    /// Ranking was not the problem and is untouched: recency is already
    /// weighted, and the entries that won were the same age as the ones that
    /// lost. What was missing is a rule for the evaluative case, so this pins
    /// the prompt's two halves of it.
    #[test]
    fn the_prompt_forbids_a_bare_subject_name_for_an_evaluative_question() {
        assert!(
            SUB_QUERY_PROMPT.contains("A BARE SUBJECT NAME IS A BAD QUERY"),
            "the non-discriminating-query rule must be stated"
        );
        assert!(
            SUB_QUERY_PROMPT.contains("STATE and OUTCOME"),
            "it must say what to query instead of the name"
        );
        assert!(
            SUB_QUERY_PROMPT.contains("JUDGEMENT"),
            "it must name the case that triggers the rule"
        );
    }

    /// The worked example carries the whole rule for a model that skims, so it
    /// has to show BOTH mistakes being avoided: no bare subject name, and no
    /// query about the decision rather than the subject.
    #[test]
    fn the_evaluative_example_shows_state_queries_and_not_the_bare_name() {
        let after = SUB_QUERY_PROMPT
            .split_once("do something else?\" → ")
            .expect("the evaluative example must exist")
            .1;
        let list = &after[..=after.find(']').expect("the example closes its list")];
        let sub_queries: Vec<String> = serde_json::from_str(list)
            .expect("the example must be valid JSON, or it teaches a broken shape");
        let c = QueryClassification {
            sub_queries,
            ..QueryClassification::default()
        };

        assert!(
            c.sub_queries.len() >= 2,
            "one query cannot show a decomposition"
        );
        assert!(
            !c.sub_queries.iter().any(|q| q == "Example Project"),
            "the example must not contain the bare subject name: {:?}",
            c.sub_queries
        );
        assert!(
            c.sub_queries
                .iter()
                .all(|q| q.starts_with("Example Project ")),
            "every query must be about the subject, not the decision: {:?}",
            c.sub_queries
        );
    }

    #[test]
    fn test_filter_drops_tool_call_cap_phrasings() {
        let contaminated = [
            "Is testing a system that has a per-turn tool-call limit of approximately 25, with a hard cap at 100 defined in agentic_loop.rs:103.",
            "Flagged a system failure when the tool-call count reached 114.",
            "Pointed out that a ~25-call soft cap per turn was being reached due to cumulative tool calls during changelog refinement.",
            "Advocated for fixing a misleading error message blaming the user when the MAX_ITERATIONS tool call limit was reached.",
            "Considered adding a guardrail to prevent the LLM from claiming it hit a tool-call cap.",
            "Per turn limit observed during release flow.",
            "Identified a system cap issue at 114 tool calls on April 26.",
        ];
        for text in contaminated {
            assert!(
                is_fabricated_engine_internal_claim(text),
                "should filter: {}",
                text
            );
        }
    }

    #[test]
    fn test_filter_keeps_legitimate_facts() {
        let legitimate = [
            "Started the cognos migration to Rust.",
            "Has a colleague named Anna who works as a designer.",
            "Prefers integration tests over mocks for refactors.",
            "Set a daily reading limit of 30 minutes.",
            "Uses pgvector for vector search.",
            // Substring collisions that would fire if we matched substrings:
            // "unlimited" / "limited", "capacity" / "escape", "discount" / "account",
            // "budgeted", "encountered". None contain "tool call" or "per turn".
            "Has unlimited cloud storage and a 5 GB capacity ceiling.",
            "Discount applied to the user's account.",
            "Budgeted 30 minutes for the call with Anna.",
            "Encountered an error when reaching the API.",
        ];
        for text in legitimate {
            assert!(
                !is_fabricated_engine_internal_claim(text),
                "should keep: {}",
                text
            );
        }
    }

    /// The symbol list follows the `MAX_ITERATIONS` rename, and stops there.
    /// `default_max_tool_calls` is the renamed internal constant and filters;
    /// `max_tool_calls` on its own is now a real user-facing preference key, so
    /// a fact about the user changing it is legitimate memory and must survive.
    ///
    /// The last case documents a deliberate false positive: phrase the same
    /// setting as "per-turn tool call limit" and it filters, because that
    /// wording is indistinguishable from the fabricated claims above. Losing it
    /// costs nothing; the current value lives in the preference, not in memory.
    #[test]
    fn test_filter_follows_the_rename_without_eating_the_preference_key() {
        assert!(is_fabricated_engine_internal_claim(
            "The turn stopped at DEFAULT_MAX_TOOL_CALLS."
        ));
        assert!(
            !is_fabricated_engine_internal_claim("Set max_tool_calls to 2000 in Settings."),
            "the preference key is a legitimate thing to remember the user changing"
        );
        assert!(is_fabricated_engine_internal_claim(
            "Raised the per-turn tool call limit."
        ));
    }

    #[test]
    fn test_filter_is_case_insensitive() {
        assert!(is_fabricated_engine_internal_claim("TOOL-CALL CAP hit"));
        assert!(is_fabricated_engine_internal_claim(
            "Per-Turn Limit reached"
        ));
        assert!(is_fabricated_engine_internal_claim("see Agentic_Loop.RS"));
    }

    #[test]
    fn test_query_classification_default() {
        let c = QueryClassification::default();
        assert!(c.needs_memory);
        assert!(c.needs_file_list);
        assert!(c.needs_credentials);
        assert!(c.sub_queries.is_empty());
    }

    #[tokio::test]
    async fn test_extraction_does_not_leak_system_context() {
        let project_id = match std::env::var("VERTEX_PROJECT_ID") {
            Ok(id) => id,
            Err(_) => {
                crate::log!("[Memory] Skipping: VERTEX_PROJECT_ID not set");
                return;
            }
        };
        let location =
            std::env::var("VERTEX_REGION").unwrap_or_else(|_| "europe-west1".to_string());

        let vertex = crate::llm::VertexProvider::new(
            project_id,
            location,
            crate::core::prefs::MODEL_MEMORY.default_text().to_string(),
        )
        .expect("vertex provider builds");
        let call = AuxCall::over(
            std::sync::Arc::new(vertex),
            crate::engine::ContextPurpose::Memory,
        );

        let context = "Background:\n- The user is Jane Smith, born 01.01.1990, works at FakeCorp as a data scientist";
        let content = "Temperature control loop completed. Adjusted 3 heat pumps down by 1°C each. Outside temp 2°C.";

        let capture = crate::engine::AuxCapture::discarding(crate::engine::ContextPurpose::Memory);
        let facts = extract_facts(content, Some(context), None, &call, &capture)
            .await
            .expect("extraction should succeed");

        let all_text: String = facts
            .iter()
            .map(|f| format!("{} {}", f.fact, f.entities.join(" ")))
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();

        // Must not contain system context facts
        assert!(
            !all_text.contains("jane smith"),
            "Leaked user name from system context: {}",
            all_text
        );
        assert!(
            !all_text.contains("1990"),
            "Leaked birth date from system context: {}",
            all_text
        );
        assert!(
            !all_text.contains("fakecorp"),
            "Leaked employer from system context: {}",
            all_text
        );
        assert!(
            !all_text.contains("data scientist"),
            "Leaked job title from system context: {}",
            all_text
        );
    }
}
