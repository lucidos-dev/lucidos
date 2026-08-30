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

use super::{choices_for, clip, read_pref, READ_ALOUD_CHARS};
use crate::core::store::build_session_messages;
use crate::engine::agent_recovery::{newest_open_question, OpenQuestion};
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
const THREAD_TURNS: usize = 12;

/// How many of the thread's newest events the turn fold reads.
///
/// A bound, not a target. The caller is waiting for the first word of a call,
/// so reading a long thread whole is what makes a session slow to answer. Well
/// above `THREAD_TURNS` worth of events, since one turn spans many.
const THREAD_EVENT_WINDOW: i64 = 400;

/// How many of each workspace-shape list the block names.
const SHAPE_ITEMS: usize = 20;

pub const SECTIONS: &[ResidentSection] = &[
    ResidentSection {
        id: "who-and-where",
        title: "Who you are talking to, and when",
        on_by_default: true,
        build: who_and_where,
    },
    ResidentSection {
        id: "this-thread",
        title: "This conversation so far",
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

/// This thread's title, its recent turns, and any question it is parked on.
///
/// Voice joins a conversation that already exists, so without this the talker
/// answers "what were we saying" with nothing.
///
/// The question is here rather than in a section of its own, for two reasons.
/// It is part of this conversation, and a section nobody has enabled cannot be
/// read: a workspace that already wrote `voice_resident_sections` gets exactly
/// what that row lists, so a new id would reach the readers who need it least.
fn this_thread(engine: &LucidosEngine, thread_id: uuid::Uuid) -> SectionFuture<'_> {
    Box::pin(async move {
        let store = engine.event_store();
        // Three independent reads, so they go together. A builder is paid for
        // in the silence before the talker's first word, and this section is
        // the only one making more than one trip.
        let (title, events, open) = tokio::join!(
            store.get_thread_title(thread_id),
            store.get_recent_thread_events(thread_id, THREAD_EVENT_WINDOW),
            newest_open_question(engine.pool(), thread_id),
        );

        let mut out = String::new();
        if let Some(title) = title? {
            out.push_str(&format!("Title: {}\n\n", title));
        }

        let events = events?;
        let messages = build_session_messages(&events);
        let start = messages.len().saturating_sub(THREAD_TURNS);
        if messages.len() > THREAD_TURNS {
            out.push_str("(earlier turns are not loaded)\n");
        }
        for message in &messages[start..] {
            let speaker = if message.role == "user" {
                "They said"
            } else {
                "You said"
            };
            out.push_str(&format!(
                "{}: {}\n",
                speaker,
                clip(&message.content, READ_ALOUD_CHARS)
            ));
        }

        if let Some(open) = open {
            out.push_str(&open_question_block(&open));
        }
        Ok(out)
    })
}

/// A question the thread is parked on, written as something the talker knows.
///
/// Stated as fact, not as an instruction. The block is what the talker KNOWS,
/// and where an answer goes is a fact about this workspace rather than a rule
/// it follows. That an utterance cannot settle it is the same kind of fact,
/// and saying it is what stops the talker promising to record one.
///
/// The turn fold above cannot carry this. A question is not a message, so
/// `build_session_messages` has no arm for it and never will: the agent
/// already reads its own `ask_user_question` call and result.
fn open_question_block(open: &OpenQuestion) -> String {
    // The question itself is never cut, unlike the turns above it. A
    // truncated question is a different question, and the talker is about to
    // state it as the one being asked.
    format!(
        "\nWaiting on them: Lucidos asked this and it is still unanswered. It \
         is answered on screen, so nothing said aloud settles it.\n\
         Question: {}\n{}",
        open.question.trim(),
        choices_for(&open.options, open.multi_select),
    )
}

/// The apps, triggers, unread notifications and waiting threads this
/// workspace holds.
///
/// Names only. "What have I got running" is the question voice should answer
/// without a wait, and a name is enough to answer it.
///
/// The waiting line is what makes "anything that needs me?" answerable beyond
/// the thread the call is on. A notification title is not a substitute: it
/// says "Lucidos is asking" and it disappears once read, while the question
/// stays open.
fn workspace_shape(engine: &LucidosEngine, _thread_id: uuid::Uuid) -> SectionFuture<'_> {
    Box::pin(async move {
        let mut out = String::new();

        let mut apps: Vec<String> = engine
            .app_manager()
            .list_apps()
            .map(|apps| apps.into_iter().map(|a| a.name).collect())
            .unwrap_or_default();
        apps.sort();
        out.push_str(&list_line("Apps", &apps));

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
        out.push_str(&list_line("Triggers", &triggers));

        // Both reads together, for the reason `this_thread` gives.
        let (unread, waiting) = tokio::join!(
            NotificationStore::get_filtered(engine.pool(), "unread", SHAPE_ITEMS as i64, None),
            engine
                .event_store()
                .titles_awaiting_answer(SHAPE_ITEMS as i64),
        );

        let titles: Vec<String> = unread
            .unwrap_or_default()
            .into_iter()
            .map(|n| n.title)
            .collect();
        out.push_str(&list_line("Unread notifications", &titles));
        out.push_str(&list_line(
            "Threads waiting on their answer",
            &waiting.unwrap_or_default(),
        ));

        Ok(out)
    })
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
    use super::*;
    use crate::engine::thread_events::QuestionOption;

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

    fn parked_on(multi_select: bool) -> OpenQuestion {
        OpenQuestion {
            question: "The mobile-webkit tail has no verdict. Do something now?".to_string(),
            options: vec![
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
        }
    }

    /// The talker answered "anything that needs me?" with no, over a question
    /// asked seventeen seconds earlier. The block now carries it in full.
    #[test]
    fn an_open_question_reaches_the_block_with_its_choices() {
        let block = open_question_block(&parked_on(false));
        assert!(block.contains("no verdict"), "{}", block);
        assert!(
            block.contains("- Run the tail now: Chunks 25-33"),
            "{}",
            block
        );
        assert!(block.contains("- Leave it for tonight\n"), "{}", block);
        assert!(!block.contains("more than one"), "{}", block);
    }

    /// Stated as fact, because the block is what the talker KNOWS. Where the
    /// answer goes is a fact about this workspace. That an utterance settles
    /// nothing is the fact stopping the talker promising to record one.
    #[test]
    fn the_block_says_an_utterance_cannot_answer_it() {
        let block = open_question_block(&parked_on(false));
        assert!(block.contains("answered on screen"), "{}", block);
        assert!(block.contains("nothing said aloud settles it"), "{}", block);
    }

    #[test]
    fn a_multi_select_question_says_more_than_one_may_be_picked() {
        assert!(open_question_block(&parked_on(true)).contains("more than one"));
    }

    /// A free-text question carries no choices, so the block promises none.
    #[test]
    fn a_question_with_no_choices_offers_none() {
        let open = OpenQuestion {
            question: "What should I call it?".to_string(),
            options: vec![],
            multi_select: false,
        };
        let block = open_question_block(&open);
        assert!(block.contains("What should I call it?"), "{}", block);
        assert!(!block.contains("Choices"), "{}", block);
    }

    #[test]
    fn a_long_list_is_capped_and_counts_the_rest() {
        let items: Vec<String> = (0..SHAPE_ITEMS + 3).map(|i| format!("t{}", i)).collect();
        let line = list_line("Triggers", &items);
        assert!(line.contains("(and 3 more)"), "{}", line);
        assert!(line.contains("t0"), "{}", line);
        assert!(!line.contains("t20,"), "{}", line);
    }
}
