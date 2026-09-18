//! The resident-block sections, and the registry naming them.
//!
//! Adding one is a single entry in [`SECTIONS`] plus its builder. The builder
//! runs at session open, so a section reports the workspace as it is now.
//!
//! A builder is deliberately cheap: the caller is a person waiting for the
//! first word of a phone call. Read what is already projected, cap what you
//! return, and leave anything expensive to the doer.

use std::future::Future;
use std::pin::Pin;

use chrono::Utc;

use super::decision::{open_on, DecisionKind, OpenDecision};
use super::{choices_for, clip, read_pref, READ_ALOUD_CHARS};
use crate::core::store::{
    build_session_messages, EventStore, StatusFilter, ThreadSummaryFilters, UNTITLED_THREAD,
};
use crate::engine::thread_lifecycle::ThreadStatus;
use crate::engine::LucidosEngine;
use crate::scheduler::NotificationStore;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// A section's body, or the reason it could not be built.
pub type SectionFuture<'a> = Pin<Box<dyn Future<Output = Result<String, BoxError>> + Send + 'a>>;

/// One named piece of what a voice session opens knowing.
pub struct ResidentSection {
    /// What the `voice_resident_sections` preference calls it. Kebab-case,
    /// because it is a public API value, and stable, because a user typed it.
    pub id: &'static str,
    /// The heading the talker reads it under.
    pub title: &'static str,
    /// Whether a workspace that never touched the preference gets it.
    pub on_by_default: bool,
    pub build: for<'a> fn(&'a LucidosEngine, uuid::Uuid) -> SectionFuture<'a>,
}

/// How many turns of this thread the talker opens with.
///
/// The secondary cap. [`THREAD_RECALL_BYTES`] is what actually bounds the cost,
/// and this one bounds how far back a thread of one-word turns may reach.
const THREAD_TURNS: usize = 60;

/// How many bytes of those turns the block carries.
///
/// **The cap that matters, because length is what the block costs.** A
/// turn count alone bounds it only through [`READ_ALOUD_CHARS`], and the two
/// kinds of turn sit nowhere near that ceiling in the same way.
///
/// A typed message runs to hundreds of characters. A spoken row averages about
/// ninety, being one breath: a reply is written down as it is said (ADR 0188,
/// ADR 0200, ADR 0201). So a turn count tuned for typing bought a voice thread
/// a couple of minutes. One reported call had its second half fall out of the
/// block four minutes later
/// (`docs/plans/2026-09-17-a-live-caller-settles-what-is-waiting.md`).
///
/// Below the old worst case of `12 * READ_ALOUD_CHARS`, so no block got more
/// expensive. A voice thread gains most of a call. A thread of long typed
/// messages loses a turn or two it could not have afforded anyway.
///
/// Bytes rather than characters, like [`clip`]'s own bound. Multi-byte text
/// therefore stops the walk sooner than its character count would, which is
/// the safe side.
const THREAD_RECALL_BYTES: usize = 4_000;

/// How many of the thread's newest events the turn fold reads.
///
/// A bound, not a target. The caller is waiting for the first word of a call,
/// so reading a long thread whole is what makes a session slow to answer.
///
/// **It can bind before the two caps below do, and that is why a full read
/// counts as turns dropped.** One spoken row is about one event, so a voice
/// thread reaches `THREAD_TURNS` first. A typed turn folds many events into one
/// message, so a tool-heavy thread runs out of window first instead. See
/// [`earlier_turns_were_dropped`].
const THREAD_EVENT_WINDOW: i64 = 400;

/// How many of each workspace-shape list the block names.
const SHAPE_ITEMS: usize = 20;

/// What the line of threads Lucidos is working on is called.
///
/// It names the moment it was read, because nothing refreshes it. The block
/// enters the session as its first history item and is never rewritten
/// (`voice::resident`), so work finishes while the caller talks.
const RUNNING_LABEL: &str = "Threads Lucidos was working on when this call opened";

/// What the line of threads stopped on the caller is called.
///
/// A talker read "Threads waiting on their answer" out as the running list,
/// three times in one call. So the label now leads with the word that tells
/// the two apart.
const WAITING_LABEL: &str = "Threads stopped, waiting on their answer";

/// What the record of the conversation opens with.
///
/// It names the two labels, because nothing else says which speaker is which.
/// Each label is one word on purpose: a label is inside
/// [`THREAD_RECALL_BYTES`], so every byte of it is a byte no turn can have.
///
/// **It claims no completeness.** The caps drop turns, and a claim here would
/// sit directly above [`EARLIER_TURNS_DROPPED`] saying the opposite.
const RECORD_OPENS: &str = "\
What has already been said on this conversation, oldest first. `Them` is the \
person on this call, and `You` is Lucidos.\n";

/// What closes it, and the load-bearing half (ADR 0213).
///
/// **Nothing follows the record's last line except this.** The defect is the
/// talker reading that line as a turn nobody answered, and answering it. One
/// reported call recited the whole record back out, in order, at a caller who
/// had said one word
/// (`docs/plans/2026-09-17-one-opener-buys-one-answer.md`).
///
/// **Every claim in it is scoped to the record**, and the last sentence is why.
/// A card the thread is parked on renders BELOW this line, and it says the
/// caller owes an answer. So a blanket "nothing here is unanswered" would
/// contradict the card it introduces (ADR 0205).
const RECORD_CLOSES: &str = "\
The record ends here. Do not read any of it back out, and do not answer a line \
in it: the caller has heard all of it already. Anything still waiting on them \
is stated after this line, never inside the record.\n";

/// What says the record is missing its older turns.
///
/// Inside the record, under [`RECORD_OPENS`], because it describes the lines
/// below it. Above the record it read as a caveat about something else.
const EARLIER_TURNS_DROPPED: &str = "(earlier turns are not loaded)\n";

/// The fact that tells the two thread lines apart.
///
/// Stated above them, because the labels alone are what failed. It also says
/// the two lists are a snapshot, so the talker asks rather than asserting.
const THREAD_LINES_DIFFER: &str = "\
The two thread lines below answer different questions. Working means Lucidos is \
running it now. Waiting means it is stopped until they answer, so it is not \
running. Both lists date from the moment this call opened, and work finishes \
while you talk, so ask before you say what is still going.\n";

pub const SECTIONS: &[ResidentSection] = &[
    ResidentSection {
        id: "who-and-where",
        title: "Who you are talking to, and when",
        on_by_default: true,
        build: who_and_where,
    },
    ResidentSection {
        id: "this-thread",
        title: "This conversation",
        on_by_default: true,
        build: this_thread,
    },
    ResidentSection {
        id: "workspace-shape",
        title: "What this workspace has",
        on_by_default: true,
        build: workspace_shape,
    },
];

/// The workspace's name, its timezone, and the time the call started.
///
/// A spoken assistant that cannot say what time it is has failed at the first
/// question anyone asks one.
///
/// The language is deliberately absent. Which language to speak is a rule
/// rather than something known, so it is stated once, in `instructions_for`.
fn who_and_where(engine: &LucidosEngine, _thread_id: uuid::Uuid) -> SectionFuture<'_> {
    Box::pin(async move {
        let pool = engine.pool();
        let timezone = read_pref(pool, "timezone").await;

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

/// This thread's title, its recent turns, and anything it is waiting on.
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
        // Three independent reads, so they go together. A builder is paid for
        // in the silence before the talker's first word, so a section making
        // more than one trip makes them all at once.
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
        let messages = build_session_messages(&events);
        let turns = recent_turns(&messages);
        let dropped = earlier_turns_were_dropped(events.len(), messages.len(), turns.len());
        out.push_str(&fenced_record(&turns, dropped));

        for decision in &open {
            out.push_str(&open_decision_block(decision));
        }
        Ok(out)
    })
}

/// Whether the block is showing less than the whole conversation.
///
/// **Two ways to lose a turn, and the talker is told about either.** The caps
/// here drop one, and so does the SQL `LIMIT` that fed the fold: a full read is
/// a read that had more to give.
///
/// The second way is the one worth stating. It is invisible from the fold
/// alone, since a window that truncated still hands back every message it
/// built. A block under both caps would then read as the whole thread, and a
/// talker that believes that asserts where it should ask.
fn earlier_turns_were_dropped(events: usize, messages: usize, turns: usize) -> bool {
    turns < messages || events as i64 >= THREAD_EVENT_WINDOW
}

/// The newest turns that fit both caps, oldest first, each as its own line.
///
/// Walked newest-first and reversed, because the cap is spent from the newest
/// end: the last thing said is the one the talker most needs. A turn that would
/// cross the character budget stops the walk, so the budget is a ceiling rather
/// than a target.
///
/// **At least one turn, whatever its length.** A single message longer than the
/// budget would otherwise leave the block claiming an empty conversation. That
/// reads as a thread nobody has spoken on.
fn recent_turns(messages: &[crate::core::store::SessionMessage]) -> Vec<String> {
    let mut lines = Vec::new();
    let mut spent = 0usize;
    for message in messages.iter().rev().take(THREAD_TURNS) {
        // One word each, and no verb. `They said` reads as a beat in a script
        // the talker is next to speak, and it costs five bytes of recall a
        // turn (ADR 0213). [`RECORD_OPENS`] says which label is which.
        let speaker = if message.role == "user" {
            "Them"
        } else {
            "You"
        };
        let line = format!(
            "{}: {}\n",
            speaker,
            clip(&message.content, READ_ALOUD_CHARS)
        );
        if spent + line.len() > THREAD_RECALL_BYTES && !lines.is_empty() {
            break;
        }
        spent += line.len();
        lines.push(line);
    }
    lines.reverse();
    lines
}

/// The turns with a line saying what they are, and a line saying they are over.
///
/// **The closing line is the fix** (ADR 0213). An unfenced record reads as a
/// conversation still running, and its last line as a turn nobody answered. So
/// the talker answers it, and then performs the rest of the record as well.
///
/// Pure, so the shape is a test rather than something only a live engine can
/// exercise, exactly as `resident::assemble_block` is.
///
/// Nothing but the caveat for a thread nobody has spoken on. A fence around no
/// turns says a record exists and is empty, which is one more thing to be wrong
/// about.
fn fenced_record(turns: &[String], earlier_dropped: bool) -> String {
    let caveat = if earlier_dropped {
        EARLIER_TURNS_DROPPED
    } else {
        ""
    };
    if turns.is_empty() {
        return caveat.to_string();
    }
    let mut out = String::from(RECORD_OPENS);
    out.push_str(caveat);
    for turn in turns {
        out.push_str(turn);
    }
    out.push_str(RECORD_CLOSES);
    out
}

/// Something the thread is waiting on, written as what the talker KNOWS.
///
/// Stated as fact, not as an instruction. The block is knowledge, and where an
/// answer goes is a fact about this workspace rather than a rule it follows.
/// That the caller can settle it out loud is a fact of the same kind. Saying it
/// is what stops the talker sending them to the screen.
///
/// The turn fold above cannot carry this. A card is not a message, so
/// `build_session_messages` has no arm for one and never will: the agent
/// already reads its own tool call and result.
fn open_decision_block(decision: &OpenDecision) -> String {
    // Exhaustive, so a fourth kind has to decide what the caller hears rather
    // than inheriting the permission wording by default.
    let (waiting, label) = match decision.kind {
        DecisionKind::Question => ("Lucidos asked this and it is still unanswered", "Question"),
        DecisionKind::CommandPermission
        | DecisionKind::McpPermission
        | DecisionKind::CodingAgentPermission => (
            "Lucidos needs their say-so before it can carry on",
            "Asking",
        ),
    };
    // The prompt itself is never cut, unlike the turns above it. A truncated
    // question is a different question, and the talker is about to state it as
    // the one being asked.
    format!(
        "\nWaiting on them: {}. They can settle it out loud, by picking one of \
         the choices below.\n\
         {}: {}\n{}",
        waiting,
        label,
        decision.prompt,
        choices_for(&decision.choices),
    )
}

/// The apps, triggers, unread notifications, running threads and waiting
/// threads this workspace holds.
///
/// Names only. "What have I got running" is the question voice should answer
/// without a wait, and a name is enough to answer it.
///
/// **Running and waiting are two lines, and each label says which it is.** The
/// block used to carry the waiting one alone. Asked what was running, the
/// talker read that list out instead, and it could not know better: a missing
/// line is invisible, since [`list_line`] says "none" only for a list that
/// exists.
///
/// The waiting line is what makes "anything that needs me?" answerable beyond
/// the thread the call is on. A notification title is not a substitute: it
/// says "Lucidos is asking" and it disappears once read, while the question
/// stays open.
fn workspace_shape(engine: &LucidosEngine, thread_id: uuid::Uuid) -> SectionFuture<'_> {
    Box::pin(async move {
        let mut apps: Vec<String> = engine
            .app_manager()
            .list_apps()
            .map(|apps| apps.into_iter().map(|a| a.name).collect())
            .unwrap_or_default();
        apps.sort();

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

        // All three reads together, for the reason `this_thread` gives.
        let (unread, running, waiting) = tokio::join!(
            NotificationStore::get_filtered(engine.pool(), "unread", SHAPE_ITEMS as i64, None),
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

/// Lay the five lists out as the lines the talker reads.
///
/// Split from the builder so the layout is a test rather than something only a
/// live engine can exercise, exactly as `resident::assemble_block` is. Dropping
/// a line is the defect this section was fixed for, and nothing else here
/// would notice.
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

/// One thread line, saying plainly when the read did not come back.
///
/// **A failed read is not "none".** The talker answered "what is running" with
/// total confidence and got it wrong. A list of nothing from an unreachable
/// database is the same wrong answer by another route. The other three lines
/// still say "none", and both of these used to.
fn thread_line(label: &str, read: Result<Vec<String>, BoxError>) -> String {
    match read {
        Ok(names) => list_line(label, &names),
        Err(e) => {
            log!("[Voice] Could not read '{}': {}", label, e);
            format!("{}: could not be read, so say you do not know\n", label)
        }
    }
}

/// Names of the threads Lucidos is working on, newest first.
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

/// One `Label: a, b, c` line, capped, saying plainly when there are none.
///
/// An empty list is stated rather than omitted. Read nothing about triggers and
/// the talker cannot tell "none" from "not loaded". The honesty rule then
/// forbids it from answering at all.
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

    /// The frontend's mirror of this registry, read at compile time. Same reach
    /// `voice::language` makes for the Locale dropdown.
    const MIRROR: &str = include_str!("../../../lucidos-app/src/store/actions/preferences.ts");

    /// The toggles are drawn from a TS copy of [`SECTIONS`]. A section added
    /// here and nowhere else can never be turned off. A title changed here
    /// leaves the settings screen naming the old one.
    ///
    /// A `.ts`-only diff does not compile this, so `/harden` Phase 4.5 carries
    /// a row pointing `preferences.ts` at `voice::sections`.
    #[test]
    fn the_settings_toggles_mirror_this_registry() {
        let start = MIRROR
            .find("export const VOICE_RESIDENT_SECTIONS")
            .expect("the frontend still declares VOICE_RESIDENT_SECTIONS");
        let list = &MIRROR[start..];
        let body = &list[..list.find("];").expect("the list is still closed")];

        // One entry per line, so a reformat that joins them fails loudly here
        // rather than passing by reading half the list. An entry carries all
        // three keys, which is what tells it from the type annotation above the
        // array: that spreads its own `id:` and `onByDefault:` over two lines.
        let entries: Vec<&str> = body
            .lines()
            .filter(|l| l.contains("id:") && l.contains("onByDefault:"))
            .collect();
        assert_eq!(
            entries.len(),
            SECTIONS.len(),
            "the mirror lists {} sections and the registry has {}",
            entries.len(),
            SECTIONS.len()
        );

        // Quoted, so the match is EXACT: a bare substring would pass on a
        // mirror whose title merely contains the registry's, which is the
        // shortening case the guard exists to catch. Either quote style is
        // accepted, so a title carrying an apostrophe stays spellable.
        let written = |key: &str, value: &str| {
            [
                format!("{}: '{}'", key, value),
                format!("{}: \"{}\"", key, value),
            ]
        };
        for (entry, section) in entries.iter().zip(SECTIONS) {
            assert!(
                written("id", section.id).iter().any(|w| entry.contains(w)),
                "the mirror's row {:?} is not '{}'",
                entry,
                section.id
            );
            assert!(
                written("title", section.title)
                    .iter()
                    .any(|w| entry.contains(w)),
                "'{}' is titled {:?} here, and something else in the mirror",
                section.id,
                section.title
            );
            assert!(
                entry.contains(&format!("onByDefault: {}", section.on_by_default)),
                "'{}' ships {} here, and the other way in the mirror",
                section.id,
                section.on_by_default
            );
        }
    }

    /// The three engine defaults the settings screen renders as the resolved
    /// current value, mirrored into the same TS module with no other guard.
    ///
    /// Drift here is silent and user-visible: change a catalog default and
    /// Settings keeps showing the old one as what a fresh workspace uses,
    /// while every call opens on the new one.
    #[test]
    fn the_settings_defaults_mirror_the_catalog() {
        use crate::core::preference_catalog;

        for (key, constant) in [
            ("model_voice_talker", "DEFAULT_VOICE_TALKER_MODEL"),
            ("model_voice_transcriber", "DEFAULT_VOICE_TRANSCRIBER_MODEL"),
            ("voice_talker_voice", "DEFAULT_VOICE_TALKER_VOICE"),
        ] {
            let default = preference_catalog::lookup(key)
                .unwrap_or_else(|| panic!("{} is not in the catalog", key))
                .default;
            let declared = format!("export const {} = '{}';", constant, default);
            assert!(
                MIRROR.contains(&declared),
                "the catalog default for {} is {:?}, and the mirror does not \
                 declare `{}`",
                key,
                default,
                declared
            );
        }
    }

    #[test]
    fn an_empty_list_says_none_rather_than_saying_nothing() {
        assert_eq!(list_line("Apps", &[]), "Apps: none\n");
    }

    /// Both thread lines say "none" rather than going missing. A line that is
    /// not there is invisible, and that is how the waiting list came to be
    /// read out as the running one.
    #[test]
    fn both_thread_lines_say_none_rather_than_going_missing() {
        for label in [RUNNING_LABEL, WAITING_LABEL] {
            assert_eq!(list_line(label, &[]), format!("{}: none\n", label));
        }
    }

    /// The labels are what failed. A parked thread and a working thread are
    /// different answers to one spoken question. Each label carries the word
    /// that tells them apart, and the fact above them says it again.
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

    /// Nothing refreshes this block mid-call, so the running line says when it
    /// was read. A line that reads as live is a fresh way to state a wrong
    /// fact: work finishes while the caller is still talking.
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

    /// The projection leaves `title` NULL until one is generated, and fills
    /// `first_message` from the thread's first words. So both columns are
    /// nullable here, and a test that wants a nameless thread says so.
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

    /// The whole defect. A caller asked what was running. The block carried no
    /// running work at all, so the talker read out the one thread that was
    /// parked.
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

    /// The other half, and the one the caller heard. The parked thread stays
    /// on the waiting line and off the running one: it is blocked on a person
    /// rather than working.
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

    /// A thread the caller cannot place by name is the call's own, so it says
    /// so. Untitled work is named by its first words, and a thread with
    /// neither still gets a name rather than a gap in the list.
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

    /// The whole section, laid out. Drop the running line and the talker is
    /// back where it started, with one thread-shaped list to read out.
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
        // The fact reads above both, and the running line above the waiting
        // one: the caller asked what was running.
        let fact = body.find(THREAD_LINES_DIFFER).expect("the fact is there");
        let running_at = body.find(&running).expect("the running line is there");
        assert!(fact < running_at, "{}", body);
        assert!(
            running_at < body.find(&waiting).expect("waiting"),
            "{}",
            body
        );
    }

    /// A read that never came back is not "none". Saying "none" is how the
    /// talker stated a wrong fact with total confidence, and an unreachable
    /// database is another way to reach it.
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

    /// A question card the doer is parked on, as the block meets it.
    fn an_open_question(multi_select: bool) -> OpenDecision {
        OpenDecision::question(
            "toolu_q0",
            "The mobile-webkit tail has no verdict. Do something now?",
            &[
                QuestionOption {
                    id: "opt-0".to_string(),
                    label: "Run the tail now".to_string(),
                    description: Some("Chunks 25-33, on the current main".to_string()),
                },
                QuestionOption {
                    id: "opt-1".to_string(),
                    label: "Leave it for tonight".to_string(),
                    description: None,
                },
            ],
            multi_select,
        )
    }

    /// The talker answered "anything that needs me?" with no, over a question
    /// asked seventeen seconds earlier. The block now carries it in full.
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

    /// Stated as fact, because the block is what the talker KNOWS. Where the
    /// answer goes is a fact about this workspace, and it is no longer the
    /// screen: the caller settles it out loud.
    #[test]
    fn the_block_says_the_caller_can_settle_it_out_loud() {
        let block = open_decision_block(&an_open_question(false));
        assert!(block.contains("settle it out loud"), "{}", block);
        assert!(!block.to_lowercase().contains("on screen"), "{}", block);
    }

    /// A permission card reads as one, rather than as a question Lucidos asked.
    #[test]
    fn a_permission_card_reads_as_a_request_for_a_say_so() {
        let block = open_decision_block(&OpenDecision::command_permission(
            "req-1",
            "run_bash",
            "gh release delete v1",
            "Deletes a published release.",
        ));
        assert!(block.contains("their say-so"), "{}", block);
        assert!(block.contains("gh release delete v1"), "{}", block);
        assert!(
            block.contains("- Allow once [command:req-1#allow-once]"),
            "{}",
            block
        );
        assert!(block.contains("- Deny [command:req-1#deny]"), "{}", block);
    }

    /// A free-text question still carries the one choice that answers it: the
    /// caller's own words. Nothing else can settle it.
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

    /// A long list is one line, and it says how many it left out. Every label
    /// goes through the one formatter, so the two thread lines cap and count
    /// exactly as Apps and Triggers do.
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

    // -----------------------------------------------------------------------
    // What the block remembers of this conversation
    // -----------------------------------------------------------------------

    /// One folded turn. Only the role and the words reach the block. The rest
    /// is spelled out because `SessionMessage` has no `Default`, exactly as its
    /// production builders spell it.
    fn said(role: &str, content: &str) -> crate::core::store::SessionMessage {
        crate::core::store::SessionMessage {
            role: role.to_string(),
            content: content.to_string(),
            created_at: Utc::now(),
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
            event_id: None,
            thread_id: None,
            agent: None,
        }
    }

    /// A whole call's worth of short spoken rows fits, which is the report.
    ///
    /// A caller came back to a thread four minutes later and found the talker
    /// re-asking what they had already settled. One call there was 28 turns,
    /// and the block carried the last twelve.
    #[test]
    fn a_calls_worth_of_spoken_turns_all_reach_the_block() {
        let mut messages = vec![said("user", "he can just set up the MCP server")];
        for index in 0..27 {
            messages.push(said("assistant", &format!("row {}", index)));
        }

        let turns = recent_turns(&messages);

        assert_eq!(turns.len(), messages.len(), "{:?}", turns);
        assert!(turns[0].contains("MCP server"), "{:?}", turns[0]);
    }

    /// Oldest first, so the talker reads the conversation in the order it
    /// happened. The walk runs the other way, spending the budget from the
    /// newest end.
    #[test]
    fn the_turns_read_in_the_order_they_were_said() {
        let messages = vec![said("user", "first"), said("assistant", "second")];
        let turns = recent_turns(&messages);
        assert_eq!(turns, vec!["Them: first\n", "You: second\n"]);
    }

    /// **The recitation's own fix** (ADR 0213). The record's last line is never
    /// a turn.
    ///
    /// A talker reads an unfenced record as a conversation still running, and
    /// its last line as something nobody answered. One reported call performed
    /// the whole thing back, in order.
    #[test]
    fn the_record_never_ends_on_a_turn() {
        let turns = recent_turns(&[said("user", "first"), said("assistant", "second")]);

        let record = fenced_record(&turns, false);

        let last = record.trim_end().lines().last().unwrap_or_default();
        assert!(!last.starts_with("Them:"), "{}", last);
        assert!(!last.starts_with("You:"), "{}", last);
        assert!(record.contains("Them: first\nYou: second\n"), "{}", record);
    }

    /// The caveat sits INSIDE the fence, describing the turns below it.
    ///
    /// Above the fence it landed directly under a line naming the record, so
    /// the two read as a claim and its own contradiction.
    #[test]
    fn the_dropped_turns_caveat_is_part_of_the_record() {
        let turns = recent_turns(&[said("user", "first")]);

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

    /// A thread nobody has spoken on gets no record at all.
    ///
    /// A fence around nothing claims a record exists and is empty, which the
    /// talker can then be wrong about out loud.
    #[test]
    fn no_turns_means_no_record() {
        assert!(fenced_record(&[], false).is_empty());
    }

    /// With no turns the caveat still stands alone, as it always did.
    ///
    /// A thread of 400 tool events and no messages reaches it. Losing it there
    /// would leave a talker asserting over a thread it never read.
    #[test]
    fn no_turns_still_says_earlier_ones_were_dropped() {
        assert_eq!(fenced_record(&[], true), EARLIER_TURNS_DROPPED);
    }

    /// Characters are what the block costs, so characters are what bound it. A
    /// thread of long typed messages cannot make a session slow to answer.
    #[test]
    fn a_thread_of_long_messages_is_bounded_by_the_character_budget() {
        let long = "x".repeat(READ_ALOUD_CHARS);
        let messages: Vec<_> = (0..THREAD_TURNS * 2).map(|_| said("user", &long)).collect();

        let turns = recent_turns(&messages);

        let spent: usize = turns.iter().map(String::len).sum();
        assert!(spent <= THREAD_RECALL_BYTES, "the block spent {}", spent);
        assert!(turns.len() < messages.len());
    }

    /// One message longer than the whole budget still reaches the talker.
    /// Dropped, the block would claim a thread nobody has spoken on.
    #[test]
    fn one_turn_over_budget_is_still_the_conversation() {
        let messages = vec![said("user", &"x".repeat(THREAD_RECALL_BYTES * 2))];
        assert_eq!(recent_turns(&messages).len(), 1);
    }

    /// A turn count is still a cap, for a thread of one-word turns that the
    /// character budget would never stop.
    #[test]
    fn a_thread_of_tiny_turns_is_bounded_by_the_turn_count() {
        let messages: Vec<_> = (0..THREAD_TURNS * 2).map(|_| said("user", "ok")).collect();
        assert_eq!(recent_turns(&messages).len(), THREAD_TURNS);
    }

    /// A whole conversation says nothing about earlier turns, because there are
    /// none. The talker reads the line as permission to ask, so a thread that
    /// lost nothing must not carry it.
    #[test]
    fn a_whole_conversation_claims_nothing_is_missing() {
        assert!(!earlier_turns_were_dropped(40, 12, 12));
    }

    /// The caps dropped a turn, so the talker is told.
    #[test]
    fn a_capped_block_says_earlier_turns_are_missing() {
        assert!(earlier_turns_were_dropped(400, 90, THREAD_TURNS));
    }

    /// The READ dropped a turn, which the fold alone cannot see.
    ///
    /// A full window is a window that had more to give. Read off the fold
    /// alone, a tool-heavy thread fits under both caps. The block would then
    /// claim a conversation that starts where the SQL `LIMIT` cut it.
    #[test]
    fn a_full_read_window_says_earlier_turns_are_missing() {
        let full = THREAD_EVENT_WINDOW as usize;
        assert!(earlier_turns_were_dropped(full, 25, 25));
    }
}
