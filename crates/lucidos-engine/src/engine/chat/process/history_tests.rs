//! The older region of `[CONVERSATION HISTORY]`, and the summary cache that
//! feeds it (ADR 0102).
//!
//! Every test here pins one of the plan's invariants. The load-bearing one is
//! the first: a user turn is never represented by a summary alone.

use super::context_mode::ContextMode;
use super::history::{render_older_region, CoveredSummary, SummaryPlan};
use crate::core::store::{newest_conversation_summary, CachedSummary, SessionMessage};
use crate::core::EventRow;
use chrono::{TimeZone, Utc};
use serde_json::json;
use uuid::Uuid;

/// A message with only the fields the renderer reads.
fn msg(role: &str, content: &str, event_id: Option<&str>) -> SessionMessage {
    SessionMessage {
        role: role.to_string(),
        content: content.to_string(),
        created_at: Utc.with_ymd_and_hms(2026, 8, 22, 12, 0, 0).unwrap(),
        channel: None,
        steps: vec![],
        images: vec![],
        user_image_hashes: vec![],
        image_description: None,
        completed: None,
        canceled: false,
        aborted: false,
        text_chunks: vec![],
        events: vec![],
        request_event_id: None,
        event_id: event_id.map(str::to_string),
        thread_id: None,
    }
}

/// The renderer's formatter, reduced to what these tests need to read.
fn format_msg(m: &SessionMessage, _verbatim: bool, _img: usize, _idx: usize) -> String {
    let role = if m.role == "user" {
        "User"
    } else {
        "Assistant"
    };
    format!("{}: {}", role, m.content)
}

fn render(older: &[SessionMessage], covered: Option<CoveredSummary<'_>>) -> String {
    let starts = vec![0usize; older.len()];
    render_older_region(older, &starts, covered, &format_msg)
}

/// A long alternating thread. Each marker is terminated with `|` so a
/// `contains` for turn 2 cannot match turn 29.
fn alternating(pairs: usize) -> Vec<SessionMessage> {
    let mut out = Vec::new();
    for i in 0..pairs {
        out.push(msg(
            "user",
            &format!("ask-{i}|"),
            Some(&Uuid::new_v4().to_string()),
        ));
        out.push(msg(
            "assistant",
            &format!("answer-{i}|"),
            Some(&Uuid::new_v4().to_string()),
        ));
    }
    out
}

/// How many turns of one role the region actually printed.
fn rendered_turns(region: &str, role: &str) -> usize {
    region
        .lines()
        .filter(|l| l.starts_with(&format!("{role}: ")))
        .count()
}

/// THE invariant. Every user turn survives verbatim, however far back it sits
/// and however much of the assistant side collapsed into the paragraph.
#[test]
fn every_user_turn_is_verbatim_however_old() {
    let older = alternating(30);
    let boundary = older.len() - 1;
    let region = render(
        &older,
        Some(CoveredSummary {
            text: "Did some work.",
            boundary,
        }),
    );
    for i in 0..30 {
        assert!(
            region.contains(&format!("User: ask-{i}|")),
            "user turn {i} must be verbatim, got:\n{region}"
        );
    }
}

/// The other half of the split: covered assistant turns are gone from the
/// region, replaced by exactly one paragraph.
#[test]
fn covered_assistant_turns_collapse_into_one_paragraph() {
    let older = alternating(30);
    let boundary = older.len() - 1;
    let region = render(
        &older,
        Some(CoveredSummary {
            text: "Did some work.",
            boundary,
        }),
    );
    for i in 0..30 {
        assert!(
            !region.contains(&format!("Assistant: answer-{i}|")),
            "assistant turn {i} is covered and must not render"
        );
    }
    assert_eq!(
        region.matches("Did some work.").count(),
        1,
        "the paragraph is printed once, not per covered turn"
    );
}

/// The paragraph sits where the run it replaces began, so the region still
/// reads in order.
#[test]
fn the_paragraph_prints_where_the_covered_run_starts() {
    let older = vec![
        msg("user", "first", Some(&Uuid::new_v4().to_string())),
        msg("assistant", "old-work", Some(&Uuid::new_v4().to_string())),
        msg("user", "second", Some(&Uuid::new_v4().to_string())),
    ];
    let region = render(
        &older,
        Some(CoveredSummary {
            text: "Summary.",
            boundary: 1,
        }),
    );
    let lines: Vec<&str> = region.lines().collect();
    assert_eq!(lines[0], "User: first");
    assert!(lines[1].contains("Summary."), "got {:?}", lines[1]);
    assert_eq!(lines[2], "User: second");
}

/// With no summary yet, nothing is silently dropped: the assistant turns
/// render compacted instead.
#[test]
fn uncovered_assistant_turns_render_rather_than_vanish() {
    let older = alternating(3);
    let region = render(&older, None);
    for i in 0..3 {
        assert!(region.contains(&format!("Assistant: answer-{i}|")));
        assert!(region.contains(&format!("User: ask-{i}|")));
    }
    assert!(
        !region.contains("resolved"),
        "no summary means no resolution claim"
    );
}

/// The uncovered side is capped, so a thread whose summariser never lands
/// cannot render every assistant turn it ever had.
#[test]
fn uncovered_assistant_turns_are_capped_and_counted() {
    let older = alternating(30);
    let region = render(&older, None);
    let shown = rendered_turns(&region, "Assistant");
    assert_eq!(
        shown,
        crate::engine::context::HISTORY_OLDER_UNCOVERED_TURNS,
        "only the newest N uncovered turns render"
    );
    assert!(
        region.contains(&format!("{} assistant turns before this", 30 - shown)),
        "the rest are counted, got:\n{region}"
    );
    assert!(
        region.contains("events(action=\"query\""),
        "the count line names the way back, using the canonical grouped tool"
    );
    // Every user turn still survives the cap: it applies to one role only.
    for i in 0..30 {
        assert!(region.contains(&format!("User: ask-{i}|")));
    }
}

/// The newest uncovered turns are the ones kept, not whichever happened to be
/// first.
#[test]
fn the_cap_keeps_the_newest_uncovered_turns() {
    let older = alternating(30);
    let region = render(&older, None);
    assert!(region.contains("Assistant: answer-29|"));
    assert!(!region.contains("Assistant: answer-0|"));
}

/// A thread that pastes very large messages hits the user budget. What falls
/// outside is counted rather than dropped in silence.
#[test]
fn oversized_user_turns_are_bounded_and_counted() {
    let big = "x".repeat(9_000);
    let older: Vec<SessionMessage> = (0..6)
        .map(|i| {
            msg(
                "user",
                &format!("{}-{}", big, i),
                Some(&Uuid::new_v4().to_string()),
            )
        })
        .collect();
    let region = render(&older, None);
    assert!(
        region.chars().count() < crate::engine::context::HISTORY_OLDER_USER_BUDGET + 1_000,
        "the region stays within the budget plus one count line"
    );
    assert!(
        region.contains("of the user's own messages before this"),
        "the elided turns are counted, got the first 200 chars:\n{}",
        region.chars().take(200).collect::<String>()
    );
    assert!(region.contains("events(action=\"query\""));
}

/// A single pasted log that fits ONCE TRUNCATED is not elided. The budget is
/// charged what the turn prints, not the raw length nobody sends.
#[test]
fn a_user_turn_is_charged_what_it_prints_not_what_it_holds() {
    let raw = crate::engine::context::HISTORY_MSG_TRUNCATE + 6_000;
    assert!(
        raw > crate::engine::context::HISTORY_OLDER_USER_BUDGET,
        "the fixture only bites while the raw length exceeds the budget"
    );
    let older = vec![
        msg("user", &"x".repeat(raw), Some(&Uuid::new_v4().to_string())),
        msg("assistant", "answer|", Some(&Uuid::new_v4().to_string())),
    ];
    let region = render(&older, None);
    assert!(
        !region.contains("of the user's own messages before this"),
        "it fits once truncated, so nothing is elided"
    );
    assert!(
        region.starts_with("User: "),
        "and it is the first thing shown"
    );
}

/// The count for elided assistant turns sits where those turns were, not above
/// the summary that covers OLDER material.
#[test]
fn the_elided_count_sits_between_the_summary_and_the_turns_still_shown() {
    let older = alternating(30);
    // Cover the oldest third, leaving far more than the cap uncovered.
    let region = render(
        &older,
        Some(CoveredSummary {
            text: "The oldest work.",
            boundary: 19,
        }),
    );
    let summary_at = region
        .lines()
        .position(|l| l.contains("The oldest work."))
        .expect("the paragraph is printed");
    let count_at = region
        .lines()
        .position(|l| l.contains("assistant turns before this are not shown"))
        .expect("the elided turns are counted");
    assert!(
        summary_at < count_at,
        "the summary covers older turns, so it comes first:\n{region}"
    );
    let first_kept_at = region
        .lines()
        .position(|l| l.starts_with("Assistant: "))
        .expect("some uncovered turns are still shown");
    assert!(
        count_at < first_kept_at,
        "and the count sits directly above the turns that survived"
    );
}

/// The resolved framing is conditional on a paragraph actually being present.
/// It is the claim that made a failed summariser worse than silence.
#[test]
fn the_resolved_claim_rides_only_with_a_real_paragraph() {
    let older = alternating(2);
    assert!(!render(&older, None).contains("resolved"));
    let with = render(
        &older,
        Some(CoveredSummary {
            text: "Everything landed.",
            boundary: older.len() - 1,
        }),
    );
    assert!(with.contains("resolved: do NOT re-attempt"));
}

fn summary_event(summary: &str, covers: &str, at_secs: u32) -> EventRow {
    EventRow {
        id: Uuid::new_v4(),
        event_type: "ConversationSummarized".into(),
        payload: json!({
            "summary": summary,
            "covers_through_event_id": covers,
            "covered_count": 4,
            "model": "gemini-3.5-flash",
        }),
        created: Utc.with_ymd_and_hms(2026, 8, 22, 12, 0, at_secs).unwrap(),
        thread_id: None,
        sequence: Some(at_secs as i64),
    }
}

/// The cache read: newest wins, so a refresh supersedes what came before.
#[test]
fn the_newest_cached_summary_wins() {
    let events = vec![
        summary_event("older paragraph", "aaa", 1),
        summary_event("newer paragraph", "bbb", 2),
    ];
    let found = newest_conversation_summary(&events).expect("a cached summary");
    assert_eq!(found.summary, "newer paragraph");
    assert_eq!(found.covers_through_event_id, "bbb");
}

/// A row missing either half is not a cache entry. A paragraph with no
/// boundary cannot be checked for staleness, and a boundary with no text is
/// not a summary.
#[test]
fn a_half_written_summary_row_is_ignored() {
    let mut no_text = summary_event("", "aaa", 1);
    no_text.payload = json!({ "covers_through_event_id": "aaa" });
    let mut no_boundary = summary_event("something", "", 2);
    no_boundary.payload = json!({ "summary": "something" });
    assert!(newest_conversation_summary(&[no_text, no_boundary]).is_none());
}

#[test]
fn a_thread_with_no_summary_event_has_no_cache() {
    assert!(newest_conversation_summary(&[]).is_none());
}

fn cached_through(older: &[SessionMessage], idx: usize, text: &str) -> CachedSummary {
    CachedSummary {
        summary: text.to_string(),
        covers_through_event_id: older[idx].event_id.clone().expect("addressable turn"),
    }
}

/// One success sticks. A later refresh that fails changes nothing, so the
/// thread keeps the paragraph instead of falling back to a bare count line.
#[test]
fn a_failed_refresh_keeps_the_cached_paragraph() {
    let older = alternating(20);
    let cached = cached_through(&older, 1, "What happened earlier.");
    let mut plan = SummaryPlan::for_region(&older, Some(&cached));
    assert!(
        plan.needs_refresh(ContextMode::Off),
        "19 uncovered turns is well past the bar"
    );

    plan.apply_refresh(&older, None);

    let covered = plan.covered().expect("the cached paragraph survives");
    assert_eq!(covered.text, "What happened earlier.");
    assert_eq!(covered.boundary, 1, "and it still covers what it covered");
}

/// A successful refresh takes over, and now covers every assistant turn in
/// the region.
#[test]
fn a_successful_refresh_replaces_the_paragraph_and_widens_it() {
    let older = alternating(20);
    let cached = cached_through(&older, 1, "stale");
    let mut plan = SummaryPlan::for_region(&older, Some(&cached));

    plan.apply_refresh(&older, Some("fresh".to_string()));

    let covered = plan.covered().expect("the fresh paragraph");
    assert_eq!(covered.text, "fresh");
    assert_eq!(covered.boundary, older.len() - 1);
    assert!(
        !plan.needs_refresh(ContextMode::Off),
        "nothing is uncovered right after one"
    );
}

/// A cache that already reaches the newest turn is reused with no model call.
#[test]
fn a_current_cache_needs_no_refresh() {
    let older = alternating(20);
    let cached = cached_through(&older, older.len() - 1, "current");
    let plan = SummaryPlan::for_region(&older, Some(&cached));
    assert!(!plan.needs_refresh(ContextMode::Off));
    assert_eq!(plan.covered().expect("reused").text, "current");
}

/// A few uncovered turns do not justify the call. They render compacted, and
/// the thread waits.
#[test]
fn a_small_gap_waits_rather_than_calling_the_model() {
    let older = alternating(20);
    let gap = crate::engine::context::HISTORY_SUMMARY_REFRESH_AFTER;
    // Boundary chosen so exactly `gap` assistant turns sit past it.
    let boundary = older.len() - 1 - (gap * 2);
    let cached = cached_through(&older, boundary, "recent enough");
    let plan = SummaryPlan::for_region(&older, Some(&cached));
    assert!(
        !plan.needs_refresh(ContextMode::Off),
        "{gap} uncovered turns is at the bar"
    );
}

/// A cached paragraph whose boundary has aged out of the region is unusable:
/// nothing says how wide it is any more.
#[test]
fn a_cache_whose_boundary_left_the_region_is_dropped() {
    let older = alternating(6);
    let stale = CachedSummary {
        summary: "covers turns nobody can point at".to_string(),
        covers_through_event_id: Uuid::new_v4().to_string(),
    };
    let plan = SummaryPlan::for_region(&older, Some(&stale));
    assert!(plan.covered().is_none());
    assert!(
        plan.needs_refresh(ContextMode::Off),
        "so the whole region is uncovered"
    );
}

/// With no cache at all and only a handful of assistant turns, the thread
/// renders them compacted rather than paying for a paragraph.
#[test]
fn a_short_older_region_never_summarises() {
    let older = alternating(2);
    let plan = SummaryPlan::for_region(&older, None);
    assert!(!plan.needs_refresh(ContextMode::Off));
    assert!(plan.covered().is_none());
}

/// ADR 0109: no `ConversationSummary` auxiliary call runs under the context
/// mode. The model writes notes as it goes, so a second pass over the same
/// region is a worse summary at a fee.
#[test]
fn the_context_mode_never_refreshes_the_summary() {
    // Far past the refresh bar, so only the mode can hold the call back.
    let older = alternating(20);
    let plan = SummaryPlan::for_region(&older, None);
    assert!(
        plan.needs_refresh(ContextMode::Off),
        "the control arm still refreshes once the region piles up"
    );
    assert!(
        !plan.needs_refresh(ContextMode::On),
        "the lean arm must never call the summariser"
    );
}
