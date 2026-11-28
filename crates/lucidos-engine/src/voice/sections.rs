//! INVARIANT: The resident-block sections, and the registry naming them.
//!
//! Adding one is a single entry in [`SECTIONS`] plus its builder. The builder
//! runs at session open, so a section reports the workspace as it is now.
//!
//! A builder is deliberately cheap: the caller is a person waiting for the
//! first word of a phone call. Read what is already projected, cap what you
//! return, and leave anything expensive to the doer.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;

use chrono::Utc;

use super::decision::{open_on, DecisionKind, OpenDecision};
use super::{choices_for, clip, fenced_card_text, PERMISSION_NEEDS_THEIR_WORDS, READ_ALOUD_CHARS};
use crate::core::prefs;
use crate::core::store::{
    EventStore, SessionMessage, StatusFilter, ThreadSummaryFilters, UNTITLED_THREAD,
};
use crate::core::EventRow;
use crate::engine::thread_events::VoiceSessionEndReason;
use crate::engine::thread_lifecycle::ThreadStatus;
use crate::engine::LucidosEngine;
use crate::scheduler::NotificationStore;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

pub type SectionFuture<'a> = Pin<Box<dyn Future<Output = Result<String, BoxError>> + Send + 'a>>;

/// INVARIANT: One named piece of what a voice session opens knowing.
pub struct ResidentSection {
    /// INVARIANT: What the `voice_resident_sections` preference calls it. Kebab-case,
    /// because it is a public API value, and stable, because a user typed it.
    pub id: &'static str,
    pub title: &'static str,
    pub build: for<'a> fn(&'a LucidosEngine, uuid::Uuid) -> SectionFuture<'a>,
}

const THREAD_TURNS: usize = 60;

const THREAD_RECALL_BYTES: usize = 4_000;

const THREAD_EVENT_WINDOW: i64 = 400;

const SHAPE_ITEMS: usize = 20;

/// INVARIANT: What the line of threads Lucidos is working on is called.
///
/// It names the moment it was read, because nothing refreshes it. The block
/// enters the session as its first history item and is never rewritten
/// (`voice::resident`), so work finishes while the caller talks.
const RUNNING_LABEL: &str = "Threads Lucidos was working on when this call opened";

const WAITING_LABEL: &str = "Threads stopped, waiting on their answer";

const RECORD_OPENS: &str = "\
What has already been said on this conversation, oldest first. `Them` is the \
person on this call, and `You` is what Lucidos wrote back. What was said aloud \
on earlier calls is left out. Lines marked as left when a call ended, or as \
after it, are on screen and owed nothing. Bring them up only if asked.\n";

const RECORD_CLOSES: &str = "\
The record ends here. Do not read any of it back out, and do not answer a line \
in it: the caller has heard all of it already. Anything still waiting on them \
is stated after this line, never inside the record.\n";

const EARLIER_TURNS_DROPPED: &str = "(earlier turns are not loaded)\n";

const THREAD_LINES_DIFFER: &str = "\
The two thread lines below answer different questions. Working means Lucidos is \
running it now. Waiting means it is stopped until they answer, so it is not \
running. Both lists date from the moment this call opened, and work finishes \
while you talk, so ask before you say what is still going.\n";

pub const SECTIONS: &[ResidentSection] = &[
    ResidentSection {
        id: "who-and-where",
        title: "Who you are talking to, and when",
        build: who_and_where,
    },
    ResidentSection {
        id: "this-thread",
        title: "This conversation",
        build: this_thread,
    },
    ResidentSection {
        id: "workspace-shape",
        title: "What this workspace has",
        build: workspace_shape,
    },
];

fn who_and_where(engine: &LucidosEngine, _thread_id: uuid::Uuid) -> SectionFuture<'_> {
    Box::pin(async move {
        let pool = engine.pool();
        let timezone = prefs::TIMEZONE.read(pool).await;

        let mut out = format!("Workspace: {}\n", engine.workspace_name());
        match &timezone {
            Some(tz) => {
                out.push_str(&format!("Timezone: {}\n", tz));
                match tz.parse::<chrono_tz::Tz>() {
                    Ok(zone) => out.push_str(&format!(
                        "Local time when this call started: {}\n",
                        Utc::now()
                            .with_timezone(&zone)
                            .format("%A %-d %B %Y, %H:%M")
                    )),
                    Err(_) => out.push_str(&format!(
                        "The stored timezone '{}' does not resolve, so say you are \
                         unsure of the local time.\n",
                        tz
                    )),
                }
            }
            None => out.push_str(
                "No timezone is set, so you do not know the local time. Say so if asked.\n",
            ),
        }
        Ok(out)
    })
}

/// INVARIANT: This thread's title, its recent turns, and anything it is waiting on.
///
/// Voice joins a conversation that already exists, so without this the talker
/// answers "what were we saying" with nothing.
///
/// What is waiting is here rather than in a section of its own, for two
/// reasons. It is part of this conversation, and a section nobody has enabled
/// cannot be read: a workspace that already wrote `voice_resident_sections`
/// gets exactly what that row lists, so a new id would reach the readers who
/// need it least.
fn this_thread(engine: &LucidosEngine, thread_id: uuid::Uuid) -> SectionFuture<'_> {
    Box::pin(async move {
        let store = engine.event_store();
        let (title, events, open) = tokio::join!(
            store.get_thread_title(thread_id),
            store.get_recent_thread_events(thread_id, THREAD_EVENT_WINDOW),
            open_on(engine.pool(), thread_id),
        );

        let mut out = String::new();
        if let Some(title) = title? {
            out.push_str(&format!("Title: {}\n\n", title));
        }

        let events = events?;
        let messages = store.build_messages_now(&events).await;
        let record = record_of(&events, &messages);
        let lines = recent_lines(&record);
        let dropped = earlier_turns_were_dropped(events.len(), record.len(), lines.len());
        out.push_str(&fenced_record(&lines, dropped));

        for decision in &open {
            out.push_str(&open_decision_block(decision));
        }
        Ok(out)
    })
}

/// INVARIANT: Whether the block is showing less than the whole conversation.
///
/// **Two ways to lose a turn, and the talker is told about either.** The caps
/// here drop one, and so does the SQL `LIMIT` that fed the fold: a full read is
/// a read that had more to give.
///
/// The second way is the one worth stating. It is invisible from the fold
/// alone, since a window that truncated still hands back every message it
/// built. A block under both caps would then read as the whole thread, and a
/// talker that believes that asserts where it should ask.
fn earlier_turns_were_dropped(events: usize, record: usize, shown: usize) -> bool {
    shown < record || events as i64 >= THREAD_EVENT_WINDOW
}

const TALKER_ROWS: &[&str] = &["SpokenReplyGenerated", "WorkDelegated"];

enum RecordLine<'a> {
    Turn(&'a SessionMessage),
    CallEnded(CallEnd),
}

struct CallEnd {
    reason: Option<VoiceSessionEndReason>,
    mid_reply: bool,
}

/// INVARIANT: The conversation as the talker reads it: the turns nobody spoke aloud, with
/// a mark where each earlier call ended.
///
/// Filtered here and never in the fold, because the doer reads the same fold
/// and must keep every spoken row. Placed by event order, never by clock: the
/// fold dates some turns by their request rather than their answer.
fn record_of<'a>(events: &[EventRow], messages: &'a [SessionMessage]) -> Vec<RecordLine<'a>> {
    let position: HashMap<String, usize> = events
        .iter()
        .enumerate()
        .map(|(index, event)| (event.id.to_string(), index))
        .collect();
    let by_talker: HashSet<String> = events
        .iter()
        .filter(|event| TALKER_ROWS.contains(&event.event_type.as_str()))
        .map(|event| event.id.to_string())
        .collect();

    let mut at = 0usize;
    let mut turns = Vec::new();
    for message in messages {
        let id = message.event_id.as_deref();
        if id.is_some_and(|id| by_talker.contains(id)) {
            continue;
        }
        at = id.and_then(|id| position.get(id)).copied().unwrap_or(at);
        turns.push((at, message));
    }

    let mut ends = call_ends(events).into_iter().peekable();
    let mut record = Vec::new();
    for (at, message) in turns {
        while let Some((_, end)) = ends.next_if(|(end_at, _)| *end_at < at) {
            record.push(RecordLine::CallEnded(end));
        }
        record.push(RecordLine::Turn(message));
    }
    record.extend(ends.map(|(_, end)| RecordLine::CallEnded(end)));
    record
}

fn call_ends(events: &[EventRow]) -> Vec<(usize, CallEnd)> {
    let mut cut_off: HashMap<&str, bool> = HashMap::new();
    let mut ends = Vec::new();
    for (index, event) in events.iter().enumerate() {
        let payload = &event.payload;
        let session = payload
            .get("session_id")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        match event.event_type.as_str() {
            "SpokenReplyGenerated" => {
                let interrupted = payload.get("interrupted").and_then(|v| v.as_bool());
                cut_off.insert(session, interrupted.unwrap_or(false));
            }
            "SpokenMessageReceived" => {
                cut_off.insert(session, false);
            }
            "VoiceSessionEnded" => {
                let reason = payload
                    .get("reason")
                    .and_then(|v| serde_json::from_value(v.clone()).ok());
                let mid_reply = cut_off.get(session).copied().unwrap_or(false);
                ends.push((index, CallEnd { reason, mid_reply }));
            }
            _ => {}
        }
    }
    ends
}

fn call_end_line(end: &CallEnd) -> String {
    let how = match end.reason {
        Some(VoiceSessionEndReason::Hangup) => ": they hung up",
        Some(VoiceSessionEndReason::AgentHangup) => ": you rang off",
        Some(VoiceSessionEndReason::Disconnected) => ": the line dropped",
        Some(VoiceSessionEndReason::ProviderFailed | VoiceSessionEndReason::EngineShutdown) => {
            ": it failed"
        }
        None => "",
    };
    let cut = if end.mid_reply {
        ", while you were still speaking. Repeat that answer only if they ask"
    } else {
        ""
    };
    format!("(A call ended here{}{}.)\n", how, cut)
}

const LEFT_WHEN_THE_CALL_ENDED: &str = "Them, left when the call ended";

const AFTER_THE_CALL_ENDED: &str = "You, after the call ended";

fn speaker_label(record: &[RecordLine], index: usize, message: &SessionMessage) -> &'static str {
    let ends_a_call = |line: Option<&RecordLine>| matches!(line, Some(RecordLine::CallEnded(_)));
    if message.role == "user" {
        if ends_a_call(record.get(index + 1)) {
            LEFT_WHEN_THE_CALL_ENDED
        } else {
            "Them"
        }
    } else {
        let before = record[..index]
            .iter()
            .rev()
            .find(|line| !matches!(line, RecordLine::Turn(m) if m.role != "user"));
        if ends_a_call(before) {
            AFTER_THE_CALL_ENDED
        } else {
            "You"
        }
    }
}

/// INVARIANT: The newest record lines that fit both caps, oldest first.
///
/// Walked newest-first and reversed, because the cap is spent from the newest
/// end: the last thing said is the one the talker most needs. A line that would
/// cross the character budget stops the walk, so the budget is a ceiling rather
/// than a target.
///
/// **At least one line, whatever its length.** A single message longer than the
/// budget would otherwise leave the block claiming an empty conversation. That
/// reads as a thread nobody has spoken on.
fn recent_lines(record: &[RecordLine]) -> Vec<String> {
    let mut lines = Vec::new();
    let mut spent = 0usize;
    for (index, entry) in record.iter().enumerate().rev().take(THREAD_TURNS) {
        let line = match entry {
            RecordLine::Turn(message) => format!(
                "{}: {}\n",
                speaker_label(record, index, message),
                clip(&message.content, READ_ALOUD_CHARS)
            ),
            RecordLine::CallEnded(end) => call_end_line(end),
        };
        if spent + line.len() > THREAD_RECALL_BYTES && !lines.is_empty() {
            break;
        }
        spent += line.len();
        lines.push(line);
    }
    lines.reverse();
    lines
}

fn fenced_record(lines: &[String], earlier_dropped: bool) -> String {
    let caveat = if earlier_dropped {
        EARLIER_TURNS_DROPPED
    } else {
        ""
    };
    if lines.is_empty() {
        return caveat.to_string();
    }
    let mut out = String::from(RECORD_OPENS);
    out.push_str(caveat);
    for line in lines {
        out.push_str(line);
    }
    out.push_str(RECORD_CLOSES);
    out
}

/// INVARIANT: Something the thread is waiting on, written as what the talker KNOWS.
///
/// Stated as fact, not as an instruction. The block is knowledge, and where an
/// answer goes is a fact about this workspace rather than a rule it follows.
/// That the caller can settle it out loud is a fact of the same kind. Saying it
/// is what stops the talker sending them to the screen.
///
/// **A card's text is quoted, and a permission card carries a rule.** The text
/// may be under judgment, so the block fences it as data. Only the caller can
/// allow a permission. The mid-call note in `voice::call` says the same.
///
/// The turn fold above cannot carry this. A card is not a message, so
/// `build_session_messages_with` has no arm for one and never will: the agent
/// already reads its own tool call and result.
fn open_decision_block(decision: &OpenDecision) -> String {
    const PICK_ONE: &str = "They can settle it out loud, by picking one of the choices below.";
    let (waiting, how) = match decision.kind {
        DecisionKind::Question => (
            "Lucidos asked this and it is still unanswered",
            PICK_ONE.to_string(),
        ),
        DecisionKind::CommandPermission
        | DecisionKind::McpPermission
        | DecisionKind::CodingAgentPermission => (
            "Lucidos needs their say-so before it can carry on",
            format!("{} {}", PICK_ONE, PERMISSION_NEEDS_THEIR_WORDS),
        ),
        DecisionKind::PermissionRequest { words_answer, .. } => (
            "Lucidos asks them for a permission",
            super::permission_request_read_back(words_answer).to_string(),
        ),
    };
    // INVARIANT: The prompt itself is never cut, unlike the turns above it. A truncated
    // question is a different question, and the talker is about to state it as
    // the one being asked.
    format!(
        "\nWaiting on them: {}. {}\n{}{}",
        waiting,
        how,
        fenced_card_text(&decision.prompt),
        choices_for(&decision.choices),
    )
}

fn workspace_shape(engine: &LucidosEngine, thread_id: uuid::Uuid) -> SectionFuture<'_> {
    Box::pin(async move {
        let apps = app_names(engine.app_manager());

        let mut triggers: Vec<String> = {
            let registry = engine
                .trigger_configs
                .read()
                .expect("trigger registry lock");
            registry
                .values()
                .map(|t| {
                    if t.paused {
                        format!("{} (paused)", t.name)
                    } else {
                        t.name.clone()
                    }
                })
                .collect()
        };
        triggers.sort();

        let (unread, running, waiting) = tokio::join!(
            NotificationStore::get_filtered(
                engine.pool(),
                &crate::gateway_users::Reader::Team,
                "unread",
                SHAPE_ITEMS as i64,
                None
            ),
            running_thread_names(engine.event_store(), thread_id),
            engine
                .event_store()
                .titles_awaiting_answer(SHAPE_ITEMS as i64),
        );

        let unread: Vec<String> = unread
            .unwrap_or_default()
            .into_iter()
            .map(|n| n.title)
            .collect();
        Ok(shape_lines(&apps, &triggers, &unread, running, waiting))
    })
}

fn shape_lines(
    apps: &[String],
    triggers: &[String],
    unread: &[String],
    running: Result<Vec<String>, BoxError>,
    waiting: Result<Vec<String>, BoxError>,
) -> String {
    let mut out = String::new();
    out.push_str(&list_line("Apps", apps));
    out.push_str(&list_line("Triggers", triggers));
    out.push_str(&list_line("Unread notifications", unread));
    out.push_str(THREAD_LINES_DIFFER);
    out.push_str(&thread_line(RUNNING_LABEL, running));
    out.push_str(&thread_line(WAITING_LABEL, waiting));
    out
}

fn thread_line(label: &str, read: Result<Vec<String>, BoxError>) -> String {
    match read {
        Ok(names) => list_line(label, &names),
        Err(e) => {
            log_legacy!("[Voice] Could not read '{}': {}", label, e);
            format!("{}: could not be read, so say you do not know\n", label)
        }
    }
}

/// INVARIANT: Names of the threads Lucidos is working on, newest first.
///
/// The threads-list API's own query, asked with its own `status=running`
/// filter, so voice and the `threads` tool cannot come to disagree about
/// what running means. The `active` union is the wrong question here: it
/// folds in the threads parked on the caller, which is the very confusion
/// this line exists to end. Its `SHAPE_ITEMS` bound is the real cap, as the
/// waiting line's is.
///
/// **It inherits that query's scoping too, which is wider than the waiting
/// line's.** `titles_awaiting_answer` drops an archived or discarded thread,
/// on the ground that the user put it down. Nothing drops one here, so a row
/// left `running` under an archived thread is named. One definition of
/// running is worth more than that row, which no ordinary path produces.
///
/// The call's own thread is marked where it appears. A call never sets that
/// status itself (`call_tests`), so it appears only when an ordinary turn was
/// already in flight as the call opened.
async fn running_thread_names(
    store: &EventStore,
    thread_id: uuid::Uuid,
) -> Result<Vec<String>, BoxError> {
    let running = store
        .list_thread_summaries(ThreadSummaryFilters {
            status: StatusFilter::OneOf(&[ThreadStatus::Running]),
            sources: None,
            parent: None,
            has_draft: None,
            change_state: None,
            limit: SHAPE_ITEMS as i64,
        })
        .await?;
    let here = thread_id.to_string();
    Ok(running
        .into_iter()
        .map(|thread| {
            let name = if thread.title.trim().is_empty() {
                UNTITLED_THREAD.to_string()
            } else {
                thread.title
            };
            if thread.thread_id == here {
                format!("{} (this conversation)", name)
            } else {
                name
            }
        })
        .collect())
}

fn app_names(app_manager: &crate::core::AppManager) -> Vec<String> {
    let mut names: Vec<String> = app_manager
        .list_apps()
        .map(|apps| apps.into_iter().map(|a| a.name).collect())
        .unwrap_or_default();
    names.sort();
    names
}

fn list_line(label: &str, items: &[String]) -> String {
    if items.is_empty() {
        return format!("{}: none\n", label);
    }
    let shown = items.len().min(SHAPE_ITEMS);
    let mut line = format!("{}: {}", label, items[..shown].join(", "));
    if items.len() > shown {
        line.push_str(&format!(" (and {} more)", items.len() - shown));
    }
    line.push('\n');
    line
}

#[cfg(test)]
mod tests {
    use sqlx::PgPool;
    use uuid::Uuid;

    use super::*;
    use crate::engine::thread_events::QuestionOption;
    use crate::test_support::{setup_test_db, teardown_test_db};

    #[tokio::test]
    async fn the_apps_line_names_no_widget() {
        let bus = crate::test_support::offline_event_bus();
        let tmp = tempfile::tempdir().unwrap();
        let manager = crate::core::AppManager::new(tmp.path()).unwrap();
        crate::core::apps::tests::app_and_widget(&manager, &bus).await;
        assert_eq!(app_names(&manager), vec!["Habit Tracker".to_string()]);
    }

    #[test]
    fn an_empty_list_says_none_rather_than_saying_nothing() {
        assert_eq!(list_line("Apps", &[]), "Apps: none\n");
    }

    #[test]
    fn both_thread_lines_say_none_rather_than_going_missing() {
        for label in [RUNNING_LABEL, WAITING_LABEL] {
            assert_eq!(list_line(label, &[]), format!("{}: none\n", label));
        }
    }

    #[test]
    fn the_two_thread_lines_cannot_be_read_as_each_other() {
        assert!(RUNNING_LABEL.contains("working"), "{}", RUNNING_LABEL);
        assert!(WAITING_LABEL.contains("stopped"), "{}", WAITING_LABEL);
        assert!(
            THREAD_LINES_DIFFER.contains("it is not running"),
            "{}",
            THREAD_LINES_DIFFER
        );
    }

    #[test]
    fn the_running_line_says_it_is_the_state_when_the_call_opened() {
        assert!(
            RUNNING_LABEL.contains("when this call opened"),
            "{}",
            RUNNING_LABEL
        );
        assert!(
            THREAD_LINES_DIFFER.contains("work finishes while you talk"),
            "{}",
            THREAD_LINES_DIFFER
        );
    }

    async fn insert_thread(pool: &PgPool, title: &str, status: ThreadStatus) -> Uuid {
        insert_nameless_thread(pool, Some(title), None, status).await
    }

    async fn insert_nameless_thread(
        pool: &PgPool,
        title: Option<&str>,
        first_message: Option<&str>,
        status: ThreadStatus,
    ) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO thread_summaries \
                (thread_id, title, first_message, source, message_count, last_activity, \
                 has_response, status) \
             VALUES ($1, $2, $3, 'chat', 0, NOW(), TRUE, $4)",
        )
        .bind(id)
        .bind(title)
        .bind(first_message)
        .bind(status.as_str())
        .execute(pool)
        .await
        .expect("insert thread_summaries");
        id
    }

    #[tokio::test]
    async fn a_running_thread_reaches_the_block() {
        let (pool, db) = setup_test_db().await;
        let store = EventStore::new(pool.clone());

        insert_thread(&pool, "UI Chat Bubbles", ThreadStatus::Running).await;

        let names = running_thread_names(&store, Uuid::new_v4())
            .await
            .expect("read what is running");
        assert_eq!(names, vec!["UI Chat Bubbles".to_string()]);

        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn a_thread_waiting_on_the_user_is_not_on_the_running_line() {
        let (pool, db) = setup_test_db().await;
        let store = EventStore::new(pool.clone());

        insert_thread(&pool, "UI Chat Bubbles", ThreadStatus::Running).await;
        insert_thread(
            &pool,
            "Run Nightly release prep",
            ThreadStatus::WaitingForUserAnswer,
        )
        .await;
        insert_thread(&pool, "Yesterday's chat", ThreadStatus::Idle).await;

        let names = running_thread_names(&store, Uuid::new_v4())
            .await
            .expect("read what is running");
        assert_eq!(names, vec!["UI Chat Bubbles".to_string()]);

        let waiting = store
            .titles_awaiting_answer(SHAPE_ITEMS as i64)
            .await
            .expect("read what is waiting");
        assert_eq!(waiting, vec!["Run Nightly release prep".to_string()]);

        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn the_calls_own_thread_is_marked_and_a_nameless_one_still_has_a_name() {
        let (pool, db) = setup_test_db().await;
        let store = EventStore::new(pool.clone());

        let here = insert_nameless_thread(&pool, None, None, ThreadStatus::Running).await;
        insert_nameless_thread(
            &pool,
            None,
            Some("Check the release notes"),
            ThreadStatus::Running,
        )
        .await;

        let mut names = running_thread_names(&store, here)
            .await
            .expect("read what is running");
        names.sort();
        assert_eq!(
            names,
            vec![
                "Check the release notes".to_string(),
                "Untitled (this conversation)".to_string(),
            ]
        );

        teardown_test_db(&db).await;
    }

    #[test]
    fn the_section_carries_both_thread_lines_under_the_fact_that_parts_them() {
        let body = shape_lines(
            &["Habit Tracker".to_string()],
            &["Nightly release prep".to_string()],
            &[],
            Ok(vec!["UI Chat Bubbles".to_string()]),
            Ok(vec!["Run Nightly release prep".to_string()]),
        );
        let running = format!("{}: UI Chat Bubbles\n", RUNNING_LABEL);
        let waiting = format!("{}: Run Nightly release prep\n", WAITING_LABEL);
        assert!(body.contains(&running), "{}", body);
        assert!(body.contains(&waiting), "{}", body);
        let fact = body.find(THREAD_LINES_DIFFER).expect("the fact is there");
        let running_at = body.find(&running).expect("the running line is there");
        assert!(fact < running_at, "{}", body);
        assert!(
            running_at < body.find(&waiting).expect("waiting"),
            "{}",
            body
        );
    }

    #[test]
    fn a_thread_line_whose_read_failed_says_so_rather_than_saying_none() {
        let body = shape_lines(&[], &[], &[], Err("pool timed out".into()), Ok(vec![]));
        assert!(
            body.contains(&format!("{}: could not be read", RUNNING_LABEL)),
            "{}",
            body
        );
        assert!(
            !body.contains(&format!("{}: none", RUNNING_LABEL)),
            "{}",
            body
        );
        assert!(
            body.contains(&format!("{}: none", WAITING_LABEL)),
            "{}",
            body
        );
    }

    fn an_open_question(multi_select: bool) -> OpenDecision {
        OpenDecision::question(
            "toolu_q0",
            "The mobile-webkit tail has no verdict. Do something now?",
            &[
                QuestionOption {
                    id: "opt-0".to_string(),
                    label: "Run the tail now".to_string(),
                    description: Some("Chunks 25-33, on the current main".to_string()),
                    preview: None,
                    widget: None,
                    quiet: false,
                    quiet_reply: None,
                },
                QuestionOption {
                    id: "opt-1".to_string(),
                    label: "Leave it for tonight".to_string(),
                    description: None,
                    preview: None,
                    widget: None,
                    quiet: false,
                    quiet_reply: None,
                },
            ],
            multi_select,
        )
    }

    #[test]
    fn an_open_question_reaches_the_block_with_its_choices() {
        let block = open_decision_block(&an_open_question(false));
        assert!(block.contains("no verdict"), "{}", block);
        assert!(
            block.contains("- Run the tail now [question:toolu_q0#opt0]: Chunks 25-33"),
            "{}",
            block
        );
        assert!(
            block.contains("- Leave it for tonight [question:toolu_q0#opt1]\n"),
            "{}",
            block
        );
    }

    #[test]
    fn the_block_says_the_caller_can_settle_it_out_loud() {
        let block = open_decision_block(&an_open_question(false));
        assert!(block.contains("settle it out loud"), "{}", block);
        assert!(!block.to_lowercase().contains("on screen"), "{}", block);
    }

    #[test]
    fn a_permission_card_reads_as_a_request_for_a_say_so() {
        let block = open_decision_block(&OpenDecision::tool_permission(
            &crate::engine::permission::legacy::LegacyCard::command(
                "req-1",
                "run_bash",
                "gh release delete v1",
                "Deletes a published release.",
            ),
        ));
        assert!(block.contains("their say-so"), "{}", block);
        assert!(block.contains("gh release delete v1"), "{}", block);
        assert!(
            block.contains("- Allow once [command:req-1#allow-once]"),
            "{}",
            block
        );
        assert!(block.contains("- Deny [command:req-1#deny]"), "{}", block);
        assert!(block.contains(PERMISSION_NEEDS_THEIR_WORDS), "{}", block);
    }

    #[test]
    fn a_permission_request_says_how_the_caller_settles_it() {
        let block = |awaited: bool, words_answer: bool| {
            open_decision_block(&OpenDecision::permission_request(
                "toolu_p0",
                "Create workspace film-take-3?",
                &[],
                crate::engine::open_question::OpenRequest {
                    awaited,
                    words_answer,
                },
            ))
        };
        let raised = block(false, true);
        assert!(raised.contains(super::super::PERMISSION_REQUEST_READ_BACK));
        let prepared = block(true, false);
        assert!(prepared.contains(super::super::PERMISSION_REQUEST_NEEDS_A_TAP));
        assert!(!prepared.contains("from their own words"), "{}", prepared);
    }

    #[test]
    fn a_card_that_argues_for_itself_stays_inside_the_fence() {
        for card in crate::voice::cards_that_argue_for_themselves() {
            let block = open_decision_block(&card);
            crate::voice::assert_the_card_text_is_fenced(&block, &card);
        }
    }

    #[test]
    fn a_question_carries_no_permission_rule() {
        let block = open_decision_block(&an_open_question(false));
        assert!(!block.contains(PERMISSION_NEEDS_THEIR_WORDS), "{}", block);
    }

    #[test]
    fn a_question_with_no_options_still_offers_their_own_words() {
        let block = open_decision_block(&OpenDecision::question(
            "toolu_q1",
            "What should I call it?",
            &[],
            false,
        ));
        assert!(block.contains("What should I call it?"), "{}", block);
        assert!(block.contains("[question:toolu_q1#said]"), "{}", block);
    }

    #[test]
    fn a_long_list_is_capped_and_counts_the_rest() {
        let items: Vec<String> = (0..SHAPE_ITEMS + 3).map(|i| format!("t{}", i)).collect();
        for label in ["Triggers", RUNNING_LABEL, WAITING_LABEL] {
            let line = list_line(label, &items);
            assert!(line.contains("(and 3 more)"), "{}", line);
            assert!(line.contains("t0"), "{}", line);
            assert!(!line.contains("t20,"), "{}", line);
        }
    }

    fn said(role: &str, content: &str) -> crate::core::store::SessionMessage {
        crate::core::store::SessionMessage {
            role: role.to_string(),
            content: content.to_string(),
            created_at: Utc::now(),
            channel: None,
            steps: vec![],
            image_handles: vec![],
            user_image_hashes: vec![],
            image_description: None,
            completed: None,
            canceled: false,
            aborted: false,
            text_chunks: vec![],
            events: vec![],
            request_event_id: None,
            event_id: None,
            thread_id: None,
            agent: None,
            sender_device_id: None,
        }
    }

    fn as_record(messages: &[SessionMessage]) -> Vec<RecordLine<'_>> {
        messages.iter().map(RecordLine::Turn).collect()
    }

    fn row(event_type: &str, payload: serde_json::Value, ms: i64) -> EventRow {
        let mut event = EventRow::new(event_type, payload);
        event.created = chrono::DateTime::UNIX_EPOCH + chrono::Duration::milliseconds(ms);
        event
    }

    fn caller(session: &str, text: &str, ms: i64) -> EventRow {
        row(
            "SpokenMessageReceived",
            serde_json::json!({"session_id": session, "text": text}),
            ms,
        )
    }

    fn talker(session: &str, text: &str, interrupted: bool, ms: i64) -> EventRow {
        row(
            "SpokenReplyGenerated",
            serde_json::json!({"session_id": session, "text": text, "interrupted": interrupted}),
            ms,
        )
    }

    fn answer(text: &str, ms: i64) -> EventRow {
        row("ResponseGenerated", serde_json::json!({"text": text}), ms)
    }

    fn call_ended(session: &str, reason: &str, ms: i64) -> EventRow {
        row(
            "VoiceSessionEnded",
            serde_json::json!({"session_id": session, "reason": reason, "duration_secs": 60}),
            ms,
        )
    }

    fn record_text(events: &[EventRow]) -> String {
        let messages = crate::core::store::messages::build_session_messages(events);
        fenced_record(&recent_lines(&record_of(events, &messages)), false)
    }

    const DROPPED: &str = "95b92809-b7bd-441a-bbf3-ec83fe667497";
    const HUNG_UP: &str = "057600ad-6d38-41ad-b95f-762ee99db172";

    fn two_calls() -> Vec<EventRow> {
        vec![
            answer("Started the review thread with the coding agent.", 0),
            caller(DROPPED, "[chuckle", 1_000),
            talker(DROPPED, "Mm-h Hmm. Hmm. Good. Mm-hmm.", false, 2_000),
            caller(DROPPED, "Um... GPT Live One", 3_000),
            talker(
                DROPPED,
                "Sure. I'm handing the transcript over.",
                false,
                7_000,
            ),
            talker(DROPPED, "That's with the coding agent.", false, 11_000),
            caller(DROPPED, "Yeah, I wasn't really asking about that", 19_000),
            caller(DROPPED, "now", 19_090),
            talker(DROPPED, "Ah. Sorry. What did you mean?", false, 21_000),
            caller(DROPPED, "Is it better to use GPT Realtime?", 35_000),
            row(
                "WorkDelegated",
                serde_json::json!({"reason": "Compare the two talker models."}),
                36_000,
            ),
            talker(DROPPED, "I'm still reading,", false, 86_000),
            talker(DROPPED, "just a minute.", false, 87_000),
            answer("Not automatically. GPT Live 1 is the better fit.", 105_000),
            talker(DROPPED, "GPT Realtime is", true, 116_000),
            call_ended(DROPPED, "disconnected", 116_040),
            caller(HUNG_UP, "a outro lado", 280_000),
            talker(HUNG_UP, "Hmm. I'm glad!", false, 286_000),
            talker(HUNG_UP, "Here's what I can say.", true, 313_000),
            call_ended(HUNG_UP, "hangup", 313_010),
        ]
    }

    #[test]
    fn a_new_call_reads_no_line_the_talker_said_aloud() {
        let events = two_calls();

        let record = record_text(&events);

        for event in &events {
            let text = event.payload["text"].as_str().unwrap_or_default();
            match event.event_type.as_str() {
                "SpokenReplyGenerated" => assert!(!record.contains(text), "{}", record),
                "SpokenMessageReceived" | "ResponseGenerated" => {
                    assert!(record.contains(text), "{} missing:\n{}", text, record)
                }
                _ => {}
            }
        }
        assert!(
            !record.contains("Compare the two talker models"),
            "{}",
            record
        );
    }

    #[test]
    fn the_doer_still_reads_what_was_said_aloud() {
        let messages = crate::core::store::messages::build_session_messages(&two_calls());
        assert!(messages.iter().any(|m| m.content == "GPT Realtime is"));
    }

    #[test]
    fn each_earlier_call_ends_with_a_mark_saying_how() {
        let record = record_text(&two_calls());

        let dropped = record
            .find("(A call ended here: the line dropped, while you were still speaking")
            .expect(&record);
        let hung_up = record
            .find("(A call ended here: they hung up, while you were still speaking")
            .expect(&record);
        let answer = record.find("Not automatically").expect(&record);
        let next_call = record.find("a outro lado").expect(&record);
        assert!(answer < dropped && dropped < next_call, "{}", record);
        assert!(next_call < hung_up, "{}", record);
    }

    #[test]
    fn a_call_ended_between_turns_says_only_that_it_ended() {
        let events = vec![
            caller(HUNG_UP, "thanks", 0),
            talker(HUNG_UP, "Bye.", false, 1_000),
            call_ended(HUNG_UP, "hangup", 2_000),
        ];

        let record = record_text(&events);

        assert!(
            record.contains("(A call ended here: they hung up.)\n"),
            "{}",
            record
        );
    }

    #[test]
    fn a_reply_the_caller_talked_over_is_not_a_missed_one() {
        let events = vec![
            talker(HUNG_UP, "The first thing is", true, 0),
            caller(HUNG_UP, "never mind, bye", 500),
            call_ended(HUNG_UP, "hangup", 2_000),
        ];

        let record = record_text(&events);

        assert!(
            !record.contains("while you were still speaking"),
            "{}",
            record
        );
    }

    #[test]
    fn a_cut_reply_is_repeated_only_on_request() {
        let record = record_text(&two_calls());
        assert!(record.contains("only if they ask"), "{}", record);
        assert!(!record.contains("did not hear"), "{}", record);
    }

    const LAST_EVENING: &str = "5b0fd3c2-8f0e-4d7e-9a43-2f5f8b1c6e10";

    #[test]
    fn an_answer_written_after_the_call_ended_is_marked_as_such() {
        let events = vec![
            caller(LAST_EVENING, "Can you check the landing?", 0),
            talker(LAST_EVENING, "On it.", false, 1_000),
            call_ended(LAST_EVENING, "hangup", 10_000),
            answer("That batch lost its landing ownership.", 12_000),
        ];

        let record = record_text(&events);

        assert!(
            record.contains(&format!(
                "{}: That batch lost its landing ownership.\n",
                AFTER_THE_CALL_ENDED
            )),
            "{}",
            record
        );
    }

    #[test]
    fn a_caller_line_the_call_ended_on_is_marked_as_left() {
        let events = vec![
            caller(LAST_EVENING, "You are gonna add a draft for me to fix", 0),
            call_ended(LAST_EVENING, "hangup", 1_000),
        ];

        let record = record_text(&events);

        assert!(
            record.contains(&format!(
                "{}: You are gonna add a draft for me to fix\n",
                LEFT_WHEN_THE_CALL_ENDED
            )),
            "{}",
            record
        );
    }

    #[test]
    fn a_typed_exchange_after_a_call_reads_plain() {
        let messages = [said("user", "typed later"), said("assistant", "answered")];
        let record = vec![
            RecordLine::CallEnded(CallEnd {
                reason: None,
                mid_reply: false,
            }),
            RecordLine::Turn(&messages[0]),
            RecordLine::Turn(&messages[1]),
        ];

        let turns = recent_lines(&record);

        assert_eq!(
            turns,
            vec![
                "(A call ended here.)\n",
                "Them: typed later\n",
                "You: answered\n"
            ]
        );
    }

    #[test]
    fn the_record_says_a_marked_line_is_owed_nothing() {
        assert!(RECORD_OPENS.contains("owed nothing"), "{}", RECORD_OPENS);
    }

    #[test]
    fn a_calls_worth_of_spoken_turns_all_reach_the_block() {
        let mut messages = vec![said("user", "he can just set up the MCP server")];
        for index in 0..27 {
            messages.push(said("assistant", &format!("row {}", index)));
        }

        let turns = recent_lines(&as_record(&messages));

        assert_eq!(turns.len(), messages.len(), "{:?}", turns);
        assert!(turns[0].contains("MCP server"), "{:?}", turns[0]);
    }

    #[test]
    fn the_turns_read_in_the_order_they_were_said() {
        let messages = vec![said("user", "first"), said("assistant", "second")];
        let turns = recent_lines(&as_record(&messages));
        assert_eq!(turns, vec!["Them: first\n", "You: second\n"]);
    }

    #[test]
    fn the_record_never_ends_on_a_turn() {
        let turns = recent_lines(&as_record(&[
            said("user", "first"),
            said("assistant", "second"),
        ]));

        let record = fenced_record(&turns, false);

        let last = record.trim_end().lines().last().unwrap_or_default();
        assert!(!last.starts_with("Them:"), "{}", last);
        assert!(!last.starts_with("You:"), "{}", last);
        assert!(record.contains("Them: first\nYou: second\n"), "{}", record);
    }

    #[test]
    fn the_dropped_turns_caveat_is_part_of_the_record() {
        let turns = recent_lines(&as_record(&[said("user", "first")]));

        let record = fenced_record(&turns, true);

        let lines: Vec<&str> = record.lines().collect();
        let caveat = lines
            .iter()
            .position(|line| *line == EARLIER_TURNS_DROPPED.trim_end())
            .expect("the caveat is in the record");
        let turn = lines
            .iter()
            .position(|line| line.starts_with("Them:"))
            .expect("the turn is in the record");
        assert!(caveat > 0, "{:?}", lines);
        assert!(caveat < turn, "{:?}", lines);
    }

    #[test]
    fn no_turns_means_no_record() {
        assert!(fenced_record(&[], false).is_empty());
    }

    #[test]
    fn no_turns_still_says_earlier_ones_were_dropped() {
        assert_eq!(fenced_record(&[], true), EARLIER_TURNS_DROPPED);
    }

    #[test]
    fn a_thread_of_long_messages_is_bounded_by_the_character_budget() {
        let long = "x".repeat(READ_ALOUD_CHARS);
        let messages: Vec<_> = (0..THREAD_TURNS * 2).map(|_| said("user", &long)).collect();

        let turns = recent_lines(&as_record(&messages));

        let spent: usize = turns.iter().map(String::len).sum();
        assert!(spent <= THREAD_RECALL_BYTES, "the block spent {}", spent);
        assert!(turns.len() < messages.len());
    }

    #[test]
    fn one_turn_over_budget_is_still_the_conversation() {
        let messages = vec![said("user", &"x".repeat(THREAD_RECALL_BYTES * 2))];
        assert_eq!(recent_lines(&as_record(&messages)).len(), 1);
    }

    #[test]
    fn a_thread_of_tiny_turns_is_bounded_by_the_turn_count() {
        let messages: Vec<_> = (0..THREAD_TURNS * 2).map(|_| said("user", "ok")).collect();
        assert_eq!(recent_lines(&as_record(&messages)).len(), THREAD_TURNS);
    }

    #[test]
    fn a_whole_conversation_claims_nothing_is_missing() {
        assert!(!earlier_turns_were_dropped(40, 12, 12));
    }

    #[test]
    fn a_capped_block_says_earlier_turns_are_missing() {
        assert!(earlier_turns_were_dropped(400, 90, THREAD_TURNS));
    }

    #[test]
    fn a_full_read_window_says_earlier_turns_are_missing() {
        let full = THREAD_EVENT_WINDOW as usize;
        assert!(earlier_turns_were_dropped(full, 25, 25));
    }
}
