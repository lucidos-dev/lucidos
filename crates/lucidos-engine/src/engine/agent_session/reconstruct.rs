//! Phase 9: Events → conversation summary projection for STALE_RESUME recovery.
//!
//! When CC's per-session JSONL file under `~/.claude/projects/<encoded>/<sid>.jsonl`
//! is missing (machine wipe, CC version upgrade, manual delete) `--resume` returns
//! an empty Result and the engine surfaces `STALE_RESUME_ERROR`. Today the chat
//! handler retries with a fresh session and only the latest user message — CC
//! has zero memory of the prior conversation ("amnesia mode").
//!
//! This module replaces that with a TEXT projection of the thread's events so
//! the next fresh Claude Code session can be primed with a synthetic recap of what came
//! before. The recap is prepended to the user's actual message so CC sees
//! `<reconstruction>\n\n<actual user input>`.
//!
//! Caveats:
//!
//! - We do NOT try to reconstruct a stream-json conversation (CC's
//!   tool_use / tool_result pairs). The Phase 3 spike showed CC injects its
//!   own "Continue from where you left off" header on `--resume` and rejects
//!   half-formed stream-json. A FRESH session sees the recap as plain user
//!   text and that's good enough — CC has rough knowledge, not a perfect
//!   replay.
//! - The output is bounded by `max_chars` so we never blow CC's context. A
//!   typical 50K cap holds ~50 turns of moderate conversation.

use sqlx::PgPool;
use uuid::Uuid;

/// Cap for the prepended reconstruction. ~50K chars holds ~50 moderate turns
/// without blowing CC's context window.
pub(crate) const RECONSTRUCT_MAX_CHARS: usize = 50_000;

/// Header used on every reconstructed summary. Keeps the synthetic-ness
/// explicit so CC doesn't think the user typed it.
const HEADER: &str = "[Reconstructed conversation summary — your prior session was lost]";

/// Footer that bridges from the recap back into the user's actual message.
const FOOTER: &str = "[End of summary. Continue assisting the user.]";

/// Per-event truncation budget — keeps a runaway tool result from eating the
/// whole window. Picked so a single event can't consume more than a few
/// percent of the typical 50K cap.
const PER_EVENT_MAX: usize = 1500;

/// Reconstruct a textual summary of the prior conversation for `thread_id`.
///
/// Walks the events table in chronological order, projects each relevant
/// event to a short labeled line, and returns a bounded string suitable for
/// prepending to the next CC user prompt. Truncates per-event content to
/// `PER_EVENT_MAX` chars and the overall output to `max_chars`. When the
/// budget is exhausted, an `… [N earlier turn(s) elided]` notice is inserted
/// before the most recent events so the recent context wins.
///
/// Returns an empty string when the thread has no relevant events (a fresh
/// thread that somehow hit STALE_RESUME on its first turn — defensive only).
pub(crate) async fn reconstruct_summary(
    pool: &PgPool,
    thread_id: Uuid,
    max_chars: usize,
) -> String {
    let rows = match fetch_relevant_events(pool, thread_id).await {
        Ok(rows) => rows,
        Err(e) => {
            log!(
                "[Reconstruct] Failed to fetch events for thread {}: {}",
                thread_id,
                e
            );
            return String::new();
        }
    };

    if rows.is_empty() {
        return String::new();
    }

    let lines: Vec<String> = rows
        .iter()
        .filter_map(|(event_type, payload)| project_event(event_type, payload))
        .collect();

    if lines.is_empty() {
        return String::new();
    }

    assemble_with_budget(lines, max_chars)
}

/// Fetch every event for a thread that contributes to the conversation
/// summary, in chronological order. Filters out lifecycle / projection
/// noise (`SessionStarted`, `CodingAgentIdled`, `ChangeProposed`, etc.) at
/// the SQL level so the projection doesn't have to.
async fn fetch_relevant_events(
    pool: &PgPool,
    thread_id: Uuid,
) -> Result<Vec<(String, serde_json::Value)>, sqlx::Error> {
    let rows: Vec<(String, serde_json::Value)> = sqlx::query_as(
        "SELECT event_type, payload FROM events \
         WHERE aggregate_id = $1 \
           AND event_type IN ( \
                'MessageReceived', \
                'CodingAgentTextStreamed', \
                'CodingAgentToolCalled', \
                'CodingAgentToolResult', \
                'CodingAgentUserMessageSent', \
                'ResponseGenerated', \
                'ResponseFailed' \
           ) \
         ORDER BY sequence ASC",
    )
    .bind(thread_id.to_string())
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Project a single event row to one summary line, or `None` if the event
/// has nothing user-visible to record (e.g. an empty text streamed delta).
fn project_event(event_type: &str, payload: &serde_json::Value) -> Option<String> {
    let text_field = |key: &str| -> Option<String> {
        payload
            .get(key)
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| truncate(s, PER_EVENT_MAX))
    };

    match event_type {
        "MessageReceived" | "CodingAgentUserMessageSent" => {
            let text = text_field("text")?;
            Some(format!("User: {}", text))
        }
        "CodingAgentTextStreamed" => {
            let text = text_field("text")?;
            Some(format!("You (assistant): {}", text))
        }
        "ResponseGenerated" => match text_field("text") {
            Some(text) => Some(format!("You (assistant): {}", text)),
            // A benign empty completion (model ended its turn cleanly with no
            // text) — surface the gap so an orchestrator re-reading its own
            // history sees the child produced no text, without mislabeling it
            // a failure.
            None => Some("You (assistant): (no text response)".to_string()),
        },
        // Surface failures (incl. empty completions) so an orchestrator
        // re-reading its own history sees that it once produced no response —
        // otherwise the gap is invisible and looks like the turn succeeded.
        "ResponseFailed" => {
            let reason = text_field("error").unwrap_or_else(|| "no response".to_string());
            Some(format!("You (assistant): (failed: {})", reason))
        }
        "CodingAgentToolCalled" => {
            let name = payload
                .get("name")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .unwrap_or("<unknown tool>");
            // Optional one-line description — `describe_cc_tool` produces it
            // for most CC tools and it's far more readable than raw args.
            let desc = payload
                .get("description")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty());
            let line = match desc {
                Some(d) => format!("You called tool: {} — {}", name, truncate(d, PER_EVENT_MAX)),
                None => format!("You called tool: {}", name),
            };
            Some(line)
        }
        "CodingAgentToolResult" => {
            let result = text_field("result")?;
            Some(format!("Tool result: {}", result))
        }
        _ => None,
    }
}

/// Assemble the summary, dropping older lines when needed to honor
/// `max_chars`. Recent context is always kept — the most useful prompt for
/// CC's next turn is "what just happened", not "what happened five turns ago".
fn assemble_with_budget(lines: Vec<String>, max_chars: usize) -> String {
    let header_len = HEADER.len() + 2; // header + "\n\n"
    let footer_len = FOOTER.len() + 2; // "\n\n" + footer
    let elision_template = "… [{} earlier turn(s) elided]\n";
    // Loose upper bound for the elision line — fits any plausible elided count.
    let elision_reserve = elision_template.len() + 16;

    // Sanity: tiny budget can't hold even the chrome. Bail with the header
    // alone rather than silently emit garbage.
    if max_chars <= header_len + footer_len + elision_reserve {
        return format!("{}\n\n{}", HEADER, FOOTER);
    }

    let body_budget = max_chars - header_len - footer_len;

    // Walk lines from newest to oldest, keeping each as long as the running
    // total fits. This guarantees the most recent turns survive truncation.
    let mut kept: Vec<&str> = Vec::with_capacity(lines.len());
    let mut running = 0usize;
    let mut elided = 0usize;
    for line in lines.iter().rev() {
        // +1 for the newline separator between lines.
        let cost = line.len() + 1;
        // Reserve elision_reserve up-front in case we need to drop anything;
        // checking it on every iteration is cheap and avoids an off-by-one
        // when the very first dropped line would have just barely fit.
        if running + cost + elision_reserve > body_budget {
            elided = lines.len() - kept.len();
            break;
        }
        kept.push(line.as_str());
        running += cost;
    }
    kept.reverse();

    let mut out = String::with_capacity(max_chars);
    out.push_str(HEADER);
    out.push_str("\n\n");
    if elided > 0 {
        out.push_str(&format!("… [{} earlier turn(s) elided]\n", elided));
    }
    for line in kept {
        out.push_str(line);
        out.push('\n');
    }
    out.push('\n');
    out.push_str(FOOTER);
    out
}

/// Truncate `s` to at most `max` chars on a UTF-8 boundary, appending `…`
/// when truncation occurs. Chars (not bytes) so multi-byte text is safe.
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

/// Build the user-message text the engine sends to a fresh Claude Code subprocess
/// when prior context is unavailable (CC's local JSONL is gone, or we're
/// deliberately starting fresh): `<reconstruction>\n\n<message>`. Returns
/// the raw message when the thread has no events to reconstruct.
pub(crate) async fn prepend_reconstruction(
    pool: &PgPool,
    thread_id: Uuid,
    message: &str,
) -> String {
    let reconstruction = reconstruct_summary(pool, thread_id, RECONSTRUCT_MAX_CHARS).await;
    if reconstruction.is_empty() {
        message.to_string()
    } else {
        format!("{}\n\n{}", reconstruction, message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::event_bus::{BusEvent, EventBus};
    use crate::engine::thread_events::{ActorMode, EventChannel, EventMeta, ThreadEvent};
    use crate::test_support::{setup_test_db, teardown_test_db};
    use serde_json::json;

    fn cc_meta() -> EventMeta {
        EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        }
    }

    async fn emit(bus: &EventBus, thread_id: Uuid, event: ThreadEvent) {
        bus.emit(BusEvent::Thread {
            thread_id,
            event,
            meta: cc_meta(),
        })
        .await
        .unwrap();
    }

    fn user_msg(text: &str) -> ThreadEvent {
        ThreadEvent::MessageReceived {
            voice_session_id: None,
            text: text.into(),
            user_image_hashes: vec![],
            device_id: None,
            device: None,
            image_description: None,
            parent_thread_id: None,
            spawning_event_id: None,
            mode: ActorMode::Human,
            model: None,
            reasoning_effort: None,
            origin: None,
        }
    }

    fn cc_text(text: &str) -> ThreadEvent {
        ThreadEvent::CodingAgentTextStreamed {
            text: text.into(),
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
        }
    }

    fn cc_tool_called(name: &str, description: &str) -> ThreadEvent {
        ThreadEvent::CodingAgentToolCalled {
            name: name.into(),
            args: json!({}),
            description: description.into(),
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            tool_use_id: String::new(),
        }
    }

    fn cc_tool_result(result: &str) -> ThreadEvent {
        ThreadEvent::CodingAgentToolResult {
            name: String::new(),
            result: result.into(),
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            tool_use_id: String::new(),
        }
    }

    #[tokio::test]
    async fn empty_thread_yields_empty_summary() {
        let (pool, db_name) = setup_test_db().await;
        let thread_id = Uuid::new_v4();
        let summary = reconstruct_summary(&pool, thread_id, 50_000).await;
        assert!(summary.is_empty());
        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn projects_a_simple_two_turn_conversation() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();

        emit(&bus, thread_id, user_msg("add a feature flag for X")).await;
        emit(
            &bus,
            thread_id,
            cc_text("Sure, let me look at the flag system."),
        )
        .await;
        emit(&bus, thread_id, cc_tool_called("Read", "Read src/flags.rs")).await;
        emit(
            &bus,
            thread_id,
            cc_tool_result("pub fn enabled(name) -> bool"),
        )
        .await;
        emit(&bus, thread_id, user_msg("now write the migration")).await;

        let summary = reconstruct_summary(&pool, thread_id, 50_000).await;

        assert!(summary.contains(HEADER));
        assert!(summary.contains(FOOTER));
        assert!(summary.contains("User: add a feature flag for X"));
        assert!(summary.contains("You (assistant): Sure, let me look at the flag system."));
        assert!(summary.contains("You called tool: Read — Read src/flags.rs"));
        assert!(summary.contains("Tool result: pub fn enabled(name) -> bool"));
        assert!(summary.contains("User: now write the migration"));

        // Order: user → assistant → tool call → tool result → user
        let positions: Vec<usize> = [
            "User: add a feature flag for X",
            "You (assistant): Sure",
            "You called tool: Read",
            "Tool result: pub fn enabled",
            "User: now write the migration",
        ]
        .iter()
        .map(|needle| summary.find(needle).expect(needle))
        .collect();
        for w in positions.windows(2) {
            assert!(w[0] < w[1], "events must appear in chronological order");
        }

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn skips_irrelevant_events() {
        // SessionStarted, CodingAgentIdled, ThreadTitleGenerated should all be
        // filtered out of the projection. They're not conversation content.
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();

        emit(&bus, thread_id, user_msg("hi")).await;
        emit(
            &bus,
            thread_id,
            ThreadEvent::SessionStarted {
                coding_agent: crate::runtime::CodingAgent::ClaudeCode,
                session_id: "sess-1".into(),
                branch: "claude-code/foo".into(),
                repo_id: None,
                coding_agent_kind: Default::default(),
                coding_agent_folder: String::new(),
                app_id: None,
            },
        )
        .await;
        emit(&bus, thread_id, cc_text("hello back")).await;
        emit(
            &bus,
            thread_id,
            ThreadEvent::CodingAgentIdled {
                has_changes: false,
                is_external_repo: false,
                requires_restart: false,
                cc_session_id: Some("sess-1".into()),
                coding_agent: crate::runtime::CodingAgent::ClaudeCode,
                reason: None,
                worktree_path: None,
                worktree_head_sha: None,
                bg_bash_pending: false,
            },
        )
        .await;
        emit(
            &bus,
            thread_id,
            ThreadEvent::ThreadTitleGenerated {
                title: "Greeting".into(),
            },
        )
        .await;

        let summary = reconstruct_summary(&pool, thread_id, 50_000).await;
        assert!(summary.contains("User: hi"));
        assert!(summary.contains("You (assistant): hello back"));
        assert!(!summary.contains("Greeting"));
        assert!(!summary.contains("SessionStarted"));
        assert!(!summary.contains("CodingAgentIdled"));

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn truncates_per_event_to_per_event_max() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();

        // Promote the thread to CC lifecycle before emitting CC-only events.
        emit(
            &bus,
            thread_id,
            ThreadEvent::SessionStarted {
                coding_agent: crate::runtime::CodingAgent::ClaudeCode,
                session_id: "sess-1".into(),
                branch: "claude-code/trunc".into(),
                repo_id: None,
                coding_agent_kind: Default::default(),
                coding_agent_folder: String::new(),
                app_id: None,
            },
        )
        .await;
        let huge = "X".repeat(PER_EVENT_MAX * 4);
        emit(&bus, thread_id, cc_tool_result(&huge)).await;

        let summary = reconstruct_summary(&pool, thread_id, 50_000).await;
        // The truncated content must end with the ellipsis sentinel.
        assert!(summary.contains("Tool result:"));
        assert!(summary.contains('…'));
        // The whole summary must be far shorter than the original.
        assert!(summary.len() < PER_EVENT_MAX * 2);

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn budget_drops_oldest_lines_first() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();

        // 30 short turns; budget that holds maybe 10.
        for i in 0..30 {
            emit(&bus, thread_id, user_msg(&format!("ask number {}", i))).await;
            emit(&bus, thread_id, cc_text(&format!("answer number {}", i))).await;
        }

        // Generous per-event truncation so the budget is the only force at
        // play; pick a body budget that demonstrably cuts most of the history.
        let summary = reconstruct_summary(&pool, thread_id, 1_000).await;
        assert!(summary.contains(HEADER));
        assert!(summary.contains(FOOTER));
        // The most recent turn must always survive.
        assert!(
            summary.contains("ask number 29"),
            "most recent user turn missing; summary was:\n{}",
            summary
        );
        assert!(
            summary.contains("answer number 29"),
            "most recent assistant turn missing; summary was:\n{}",
            summary
        );
        // The oldest turn must have been elided.
        assert!(
            !summary.contains("ask number 0\n"),
            "oldest user turn should have been elided"
        );
        // Elision notice present.
        assert!(
            summary.contains("earlier turn(s) elided"),
            "elision notice missing"
        );
        // Honors the budget (with a small slack for header/footer rounding).
        assert!(
            summary.len() <= 1_000,
            "summary len {} exceeds 1000 budget",
            summary.len()
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    /// Integration-style: simulate the chat handler's STALE_RESUME retry by
    /// seeding a thread with prior CC turns, calling `reconstruct_summary`,
    /// and asserting the resulting "fresh-session input" — `reconstruction +
    /// "\n\n" + actual user message` — is shaped the way the retry path
    /// expects (CC must see the recap FIRST, then the new instruction).
    #[tokio::test]
    async fn stale_resume_retry_input_combines_reconstruction_then_user_message() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();

        // Prior turns on the thread before STALE_RESUME hits.
        emit(&bus, thread_id, user_msg("set up the project")).await;
        emit(&bus, thread_id, cc_text("Done — created scaffolding.")).await;

        // The stale-resume retry's actual user message.
        let actual_user_message = "now add a CHANGELOG file";

        // Mirror chat/process.rs:799-848 exactly — reconstruct, then prepend.
        let reconstruction = reconstruct_summary(&pool, thread_id, 50_000).await;
        assert!(
            !reconstruction.is_empty(),
            "thread had prior turns — reconstruction must be non-empty"
        );
        let retry_text = format!("{}\n\n{}", reconstruction, actual_user_message);

        // Reconstruction comes FIRST so CC reads the recap before acting.
        let recon_pos = retry_text.find(HEADER).expect("header present");
        let user_pos = retry_text
            .find(actual_user_message)
            .expect("actual user message present");
        assert!(
            recon_pos < user_pos,
            "reconstruction must precede the user's actual message"
        );
        assert!(
            retry_text.contains("User: set up the project"),
            "prior user turn missing from retry input"
        );
        assert!(
            retry_text.contains("You (assistant): Done — created scaffolding."),
            "prior assistant turn missing from retry input"
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    /// Defensive: a thread that hits STALE_RESUME without ANY prior content
    /// (rare — a corrupted JSONL on the very first turn) yields an empty
    /// reconstruction, and the retry input is then the raw user message.
    /// The chat handler's `if reconstruction.is_empty()` branch covers this.
    #[tokio::test]
    async fn stale_resume_retry_with_no_prior_events_uses_raw_user_message() {
        let (pool, db_name) = setup_test_db().await;
        let thread_id = Uuid::new_v4();

        let reconstruction = reconstruct_summary(&pool, thread_id, 50_000).await;
        assert!(
            reconstruction.is_empty(),
            "thread with no events must yield empty reconstruction"
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn skips_empty_text_events() {
        // CodingAgentTextStreamed deltas can legitimately be empty (whitespace
        // squashed or zero-length flush). They must not show up as bare
        // "You (assistant):" lines.
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();

        emit(&bus, thread_id, user_msg("hi")).await;
        emit(&bus, thread_id, cc_text("   ")).await;
        emit(&bus, thread_id, cc_text("real reply")).await;

        let summary = reconstruct_summary(&pool, thread_id, 50_000).await;
        assert!(summary.contains("You (assistant): real reply"));
        // Not a bare "You (assistant):" with empty text.
        assert!(
            !summary.contains("You (assistant): \n"),
            "empty assistant deltas leaked into summary"
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }
}
