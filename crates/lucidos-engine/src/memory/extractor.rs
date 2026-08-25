use crate::engine::aux_purpose::AuxCall;
use crate::llm::openai::OpenAiProvider;
use crate::llm::provider::{LlmProvider, LlmResponse, Message, MessageContent};
use crate::llm::vertex::{location_handle, LocationHandle, TokenCache, VertexProvider};
use crate::memory::RETRIEVAL_MIN_IMPORTANCE;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;

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

const QUERY_CLASSIFICATION_PROMPT: &str = r#"Classify this user message and optionally decompose into search queries.
You will receive the current message and optionally a summary of the recent conversation for context.
Use the conversation context to understand the TOPIC being discussed, not just the latest message in isolation.

Determine:
- needs_memory: Does answering this require the user's long-term memory (past conversations, projects, preferences, personal facts)? true whenever the message references past conversations, "earlier today", "what we discussed", "the research", "last time", or any prior interaction — even if the request also involves a tool action like saving to a file. false ONLY for greetings, time queries, general knowledge, or pure tool commands that don't reference any past content (e.g. "create an empty file", "what time is it").
- needs_file_list: Does answering this require knowing what files exist in the workspace? false for greetings, time queries, memory-only questions.
- needs_credentials: Does this conversation involve external APIs, tokens, authentication, or services that might need credentials? Consider the full conversation topic, not just the latest message. A follow-up like "try again" or "the token should be active" in a conversation about API access still needs credentials.
- sub_queries: If needs_memory is true, decompose into search queries about the TOPIC or CONTENT being referenced, NOT about the action being requested. Strip away action verbs like "save", "summarize", "write" and focus on the subject matter. For example, "save research about Example Org" → ["Example Org", "Example Org company analysis"]. If needs_memory is false, return empty array.
  A BARE SUBJECT NAME IS A BAD QUERY when the workspace is largely about that subject: it matches thousands of entries equally and the results come back arbitrary. So when the message asks for a JUDGEMENT, an OPINION, a COMPARISON or ADVICE about something, query that subject's STATE and OUTCOME (progress, results, adoption, recent milestones, setbacks, what happened lately), never the name on its own. "should I give up on Example Project and do something else?" → ["Example Project recent progress", "Example Project launch outcome", "Example Project adoption and traction", "Example Project setbacks"], NOT ["Example Project"]. Queries about the decision itself ("job application", "career change") retrieve nothing useful: the facts that answer such a question are facts about how the subject is going.

Return ONLY a JSON object, no markdown fences, no extra text.
Example (memory needed): {"needs_memory": true, "needs_file_list": false, "needs_credentials": false, "sub_queries": ["habit tracker progress", "weekly exercise routine"]}
Example (referencing past content): {"needs_memory": true, "needs_file_list": true, "needs_credentials": false, "sub_queries": ["Example Org", "Example Org company analysis"]}
Example (asking for a judgement): {"needs_memory": true, "needs_file_list": false, "needs_credentials": false, "sub_queries": ["Example Project recent progress", "Example Project launch outcome", "Example Project adoption and traction"]}
Example (no memory): {"needs_memory": false, "needs_file_list": false, "needs_credentials": false, "sub_queries": []}"#;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

/// Default model the extractor falls back to when no per-call override is passed
/// (and the `PREF_MODEL_MEMORY` preference is also empty). Override at process
/// startup with `LUCIDOS_EXTRACTION_MODEL`. Mirrors the
/// `LUCIDOS_MODEL` / `LUCIDOS_EMBEDDING_MODEL` env-var convention from CLAUDE.md.
const EXTRACTION_MODEL_DEFAULT: &str = "gemini-3-flash-preview";

fn default_extraction_model() -> String {
    std::env::var("LUCIDOS_EXTRACTION_MODEL")
        .unwrap_or_else(|_| EXTRACTION_MODEL_DEFAULT.to_string())
}

pub struct MemoryExtractor {
    provider: VertexProvider,
    project_id: String,
    location: LocationHandle,
    token_cache: TokenCache,
    /// Resolved OpenAI API key (a Settings → Providers `openai` credential or
    /// the `OPENAI_API_KEY` env var), attached via [`Self::with_openai_key`].
    /// Lets `provider_for_model` route `gpt-*` background-task models to OpenAI;
    /// `None` → a `gpt-*` model errors clearly instead of hitting Vertex.
    openai_api_key: Option<String>,
}

impl MemoryExtractor {
    pub fn new(
        project_id: String,
        location: String,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let token_cache: TokenCache = std::sync::Arc::new(std::sync::Mutex::new(None));
        Self::with_location_handle(project_id, location_handle(location), token_cache)
    }

    pub fn with_location_handle(
        project_id: String,
        location: LocationHandle,
        token_cache: TokenCache,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let provider = VertexProvider::with_location_handle(
            project_id.clone(),
            location.clone(),
            default_extraction_model(),
            token_cache.clone(),
        )?;
        Ok(Self {
            provider,
            project_id,
            location,
            token_cache,
            openai_api_key: None,
        })
    }

    /// Attach the resolved OpenAI key so background-task models named `gpt-*`
    /// (title, image description, memory, command judge) route to OpenAI.
    /// Builder-style so the existing constructor call sites stay unchanged.
    pub fn with_openai_key(mut self, key: Option<String>) -> Self {
        self.openai_api_key = key;
        self
    }

    /// Build the provider for a background-task model, routed by model id:
    /// `gpt-*` → OpenAI (requires [`Self::with_openai_key`]); everything else →
    /// Vertex (Gemini / Vertex-served Claude), preserving the prior behavior.
    /// Empty / `"default"` returns the shared default-model Vertex provider. The
    /// reqwest builder can fail — callers propagate via `?`.
    ///
    /// Only the `gpt-*`-vs-Vertex split is handled because those are the only
    /// providers a background-task model can name: the picker offers Gemini
    /// Flash, Vertex-served Haiku, and GPT-5.4. Direct-Anthropic models (Fable)
    /// are deliberately never offered as background options.
    ///
    /// `attempt_timeout` caps one HTTP attempt, so the purpose's deadline can
    /// contain the provider's retries rather than cutting one off mid-flight.
    /// Callers take it from `engine::aux_purpose::budget_for`.
    pub fn provider_for_model(
        &self,
        model: &str,
        attempt_timeout: Duration,
    ) -> Result<Arc<dyn LlmProvider>, Box<dyn std::error::Error + Send + Sync>> {
        if model.starts_with("gpt-") {
            let key = self.openai_api_key.clone().ok_or_else(|| {
                format!(
                    "OpenAI background model '{model}' selected but no OpenAI key is configured (Settings → Models → Providers or OPENAI_API_KEY)"
                )
            })?;
            Ok(Arc::new(
                OpenAiProvider::new(key, model.to_string())?
                    .with_request_timeout(attempt_timeout)?,
            ))
        } else if crate::engine::aux_purpose::is_extractor_default(model) {
            Ok(Arc::new(
                self.provider
                    .clone()
                    .with_request_timeout(attempt_timeout)?,
            ))
        } else {
            Ok(Arc::new(
                VertexProvider::with_location_handle(
                    self.project_id.clone(),
                    self.location.clone(),
                    model.to_string(),
                    self.token_cache.clone(),
                )?
                .with_request_timeout(attempt_timeout)?,
            ))
        }
    }

    /// Single-shot LLM call against a routed provider.
    ///
    /// Every memory call funnels through here, so this is the one place that
    /// records them for token accounting. A caller with no thread to anchor
    /// to passes no capture, and the call goes unrecorded rather than
    /// inventing a thread: artifact indexing is the case that has none.
    ///
    /// Associated rather than a method: it reads no `self` state, and that is
    /// what lets a detached task make one of these calls holding only the
    /// provider.
    async fn chat_with_provider(
        provider: &dyn LlmProvider,
        system: &str,
        user_content: &str,
        reasoning_effort: Option<&str>,
        capture: Option<&crate::engine::AuxCapture>,
    ) -> Result<LlmResponse, Box<dyn std::error::Error + Send + Sync>> {
        let messages = vec![Message {
            role: "user".to_string(),
            content: MessageContent::Text(user_content.to_string()),
        }];
        let response = provider
            .chat(messages, vec![], None, Some(system), None, reasoning_effort)
            .await?;
        if let Some(capture) = capture {
            let request_chars = system.chars().count() + user_content.chars().count();
            capture
                .record(provider.default_model(), request_chars, &response)
                .await;
        }
        Ok(response)
    }

    /// Extract atomic facts from raw content.
    /// Optional `context` provides background (system, user, conversation) so
    /// the model can correctly identify implicit entities and assess importance.
    /// `call` carries the resolved *model selection* and the call's budget.
    pub(crate) async fn extract_facts(
        &self,
        content: &str,
        context: Option<&str>,
        language: Option<&str>,
        call: &AuxCall,
        capture: Option<&crate::engine::AuxCapture>,
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
        let provider = self.provider_for_model(call.model(), call.attempt_timeout())?;
        let response = Self::chat_with_provider(
            provider.as_ref(),
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
        let value: serde_json::Value = serde_json::from_str(&cleaned).map_err(|e| {
            format!(
                "[Memory] Failed to parse extraction JSON: {} | raw: {}",
                e,
                &cleaned[..cleaned.floor_char_boundary(200)]
            )
        })?;
        let mut facts: Vec<ExtractedFact> = serde_json::from_value(value).map_err(|e| {
            format!(
                "[Memory] Failed to deserialize extraction facts: {} | raw: {}",
                e,
                &cleaned[..cleaned.floor_char_boundary(200)]
            )
        })?;

        // Drop facts that wouldn't survive RETRIEVAL_MIN_IMPORTANCE — pointless to embed.
        for fact in &mut facts {
            fact.importance = fact.importance.clamp(0.0, 1.0);
        }
        facts.retain(|f| {
            f.importance >= RETRIEVAL_MIN_IMPORTANCE
                && !is_fabricated_engine_internal_claim(&f.fact)
        });

        Ok(facts)
    }

    /// Classify a user query and optionally decompose into search sub-queries.
    /// Optional `conversation_context` provides recent conversation summary so the
    /// classifier can understand the topic (e.g. a follow-up "try again" in an API
    /// conversation still needs credentials).
    /// `call` carries the resolved *model selection* and the call's budget.
    pub(crate) async fn classify_query(
        &self,
        query: &str,
        conversation_context: Option<&str>,
        call: &AuxCall,
        capture: Option<&crate::engine::AuxCapture>,
    ) -> Result<QueryClassification, Box<dyn std::error::Error + Send + Sync>> {
        let system = match conversation_context {
            Some(ctx) if !ctx.is_empty() => format!(
                "{}\n\nConversation context (recent messages): {}",
                QUERY_CLASSIFICATION_PROMPT, ctx
            ),
            _ => QUERY_CLASSIFICATION_PROMPT.to_string(),
        };
        let provider = self.provider_for_model(call.model(), call.attempt_timeout())?;
        let response =
            Self::chat_with_provider(provider.as_ref(), &system, query, call.reasoning(), capture)
                .await?;

        let raw = response.content.unwrap_or_default();
        let cleaned = strip_code_fences(&raw);

        let classification: QueryClassification = serde_json::from_str(&cleaned).map_err(|e| {
            format!(
                "[Memory] Failed to parse query classification: {} | raw: {}",
                e,
                &cleaned[..cleaned.floor_char_boundary(200)]
            )
        })?;

        Ok(classification)
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
    /// runs it in a detached task that outlives the turn. Resolving the model
    /// selection needs the extractor; making the call does not.
    pub(crate) async fn summarize_conversation(
        provider: &dyn LlmProvider,
        turns: &str,
        reasoning_effort: Option<&str>,
        capture: Option<&crate::engine::AuxCapture>,
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
        let response =
            Self::chat_with_provider(provider, system, turns, reasoning_effort, capture).await?;
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
    use crate::test_support::{aux_captures, setup_test_db, teardown_test_db, ScriptedProvider};
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
        MemoryExtractor::chat_with_provider(
            &provider,
            "system",
            "content",
            Some("none"),
            Some(&capture),
        )
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

    /// Artifact indexing and a thread-less event both reach here with no
    /// capture. The call must still run; only the bookkeeping is skipped.
    #[tokio::test]
    async fn a_memory_call_with_no_thread_records_nothing() {
        let (pool, db_name) = setup_test_db().await;
        let thread_id = Uuid::new_v4();

        let provider = ScriptedProvider::new("gemini-3-flash-preview", vec!["[]"]);
        MemoryExtractor::chat_with_provider(&provider, "system", "content", Some("none"), None)
            .await
            .expect("scripted call succeeds");

        assert!(aux_captures(&pool, thread_id, "memory").await.is_empty());

        pool.close().await;
        teardown_test_db(&db_name).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Any finite attempt cap does for these: they check which provider comes
    /// back, not how long it waits. The real ones come from
    /// `engine::aux_purpose::budget_for`.
    const TEST_TIMEOUT: Duration = Duration::from_secs(5);

    /// A `gpt-*` background-task model routes to OpenAI (with the key attached)
    /// — the model is baked into the returned provider.
    #[test]
    fn provider_for_model_routes_gpt_to_openai() {
        let ext = MemoryExtractor::new("proj".into(), "europe-west1".into())
            .expect("extractor builds")
            .with_openai_key(Some("sk-test".into()));
        let provider = ext
            .provider_for_model("gpt-5.4", TEST_TIMEOUT)
            .expect("openai provider builds");
        assert_eq!(provider.default_model(), "gpt-5.4");
    }

    /// A `gpt-*` model without an OpenAI key errors clearly rather than silently
    /// falling through to Vertex (which can't serve it).
    #[test]
    fn provider_for_model_gpt_without_key_errors() {
        let ext =
            MemoryExtractor::new("proj".into(), "europe-west1".into()).expect("extractor builds");
        assert!(ext.provider_for_model("gpt-5.4", TEST_TIMEOUT).is_err());
    }

    /// Non-`gpt-*` models stay on Vertex even when an OpenAI key is present.
    #[test]
    fn provider_for_model_routes_non_gpt_to_vertex() {
        let ext = MemoryExtractor::new("proj".into(), "europe-west1".into())
            .expect("extractor builds")
            .with_openai_key(Some("sk-test".into()));
        let provider = ext
            .provider_for_model("gemini-3-flash-preview", TEST_TIMEOUT)
            .expect("vertex provider builds");
        assert_eq!(provider.default_model(), "gemini-3-flash-preview");
    }

    /// Empty / "default" returns the shared extractor base provider (the default
    /// extraction model on Vertex), unchanged from the pre-routing behavior.
    #[test]
    fn provider_for_model_default_uses_base_model() {
        let ext =
            MemoryExtractor::new("proj".into(), "europe-west1".into()).expect("extractor builds");
        let provider = ext
            .provider_for_model("", TEST_TIMEOUT)
            .expect("base provider");
        assert_eq!(provider.default_model(), default_extraction_model());
    }

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
        let fact = MemoryExtractor::fallback_fact("Short content", "General").unwrap();
        assert_eq!(fact.fact, "Short content");
        assert_eq!(fact.importance, 0.5);
        assert_eq!(fact.topic, "General");
        assert!(fact.entities.is_empty());
    }

    #[test]
    fn test_fallback_fact_truncation() {
        let long_content = "a".repeat(600);
        let fact = MemoryExtractor::fallback_fact(&long_content, "Test").unwrap();
        assert_eq!(fact.fact.len(), 503); // 500 chars + "..."
        assert!(fact.fact.ends_with("..."));
    }

    /// The fallback runs only after structured extraction fails, and it
    /// bypasses `extract_facts`' filtering — so empty/whitespace content must
    /// not be stored as a "fact".
    #[test]
    fn fallback_fact_rejects_empty_content() {
        assert!(MemoryExtractor::fallback_fact("", "General").is_none());
        assert!(MemoryExtractor::fallback_fact("   \n  ", "General").is_none());
    }

    /// The fallback must apply the same `is_fabricated_engine_internal_claim`
    /// guard `extract_facts` does, or a fabricated engine-internal claim
    /// slips into memory whenever structured extraction happens to fail.
    #[test]
    fn fallback_fact_rejects_fabricated_engine_internal_claim() {
        let claim = "The agentic loop hit its max_iterations cap after 25 tool calls";
        assert!(is_fabricated_engine_internal_claim(claim));
        assert!(MemoryExtractor::fallback_fact(claim, "General").is_none());
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

    #[test]
    fn test_query_classification_with_memory() {
        let json = r#"{"needs_memory": true, "needs_file_list": false, "needs_credentials": false, "sub_queries": ["habit tracker", "exercise plan"]}"#;
        let c: QueryClassification = serde_json::from_str(json).unwrap();
        assert!(c.needs_memory);
        assert!(!c.needs_file_list);
        assert!(!c.needs_credentials);
        assert_eq!(c.sub_queries, vec!["habit tracker", "exercise plan"]);
    }

    #[test]
    fn test_query_classification_no_memory() {
        let json = r#"{"needs_memory": false, "needs_file_list": false, "needs_credentials": false, "sub_queries": []}"#;
        let c: QueryClassification = serde_json::from_str(json).unwrap();
        assert!(!c.needs_memory);
        assert!(c.sub_queries.is_empty());
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
            QUERY_CLASSIFICATION_PROMPT.contains("A BARE SUBJECT NAME IS A BAD QUERY"),
            "the non-discriminating-query rule must be stated"
        );
        assert!(
            QUERY_CLASSIFICATION_PROMPT.contains("STATE and OUTCOME"),
            "it must say what to query instead of the name"
        );
        assert!(
            QUERY_CLASSIFICATION_PROMPT.contains("JUDGEMENT"),
            "it must name the case that triggers the rule"
        );
    }

    /// The worked example carries the whole rule for a model that skims, so it
    /// has to show BOTH mistakes being avoided: no bare subject name, and no
    /// query about the decision rather than the subject.
    #[test]
    fn the_evaluative_example_shows_state_queries_and_not_the_bare_name() {
        let example = QUERY_CLASSIFICATION_PROMPT
            .lines()
            .find(|l| l.starts_with("Example (asking for a judgement)"))
            .expect("the evaluative example must exist");

        let c: QueryClassification =
            serde_json::from_str(example.split_once(": ").expect("example has a JSON body").1)
                .expect("the example must be valid JSON, or it teaches a broken shape");

        assert!(c.needs_memory);
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
                crate::log!("[MemoryExtractor] Skipping: VERTEX_PROJECT_ID not set");
                return;
            }
        };
        let location =
            std::env::var("VERTEX_REGION").unwrap_or_else(|_| "europe-west1".to_string());

        let extractor = MemoryExtractor::new(project_id, location).expect("extractor builds");

        let context = "Background:\n- The user is Jane Smith, born 01.01.1990, works at FakeCorp as a data scientist";
        let content = "Temperature control loop completed. Adjusted 3 heat pumps down by 1°C each. Outside temp 2°C.";

        let facts = extractor
            .extract_facts(
                content,
                Some(context),
                None,
                &AuxCall::defaults(crate::engine::ContextPurpose::Memory),
                None,
            )
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
