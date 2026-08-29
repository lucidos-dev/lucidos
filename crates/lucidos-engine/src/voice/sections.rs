//! The resident-block sections, and the registry naming them.
//!
//! Adding one is a single entry in [`SECTIONS`] plus its builder. The builder
//! runs at session open, so a section reports the workspace as it is now.
//!
//! A builder is deliberately cheap: the caller is a person waiting for the
//! first word of a phone call. Read what is already projected, cap what you
//! return, and leave anything expensive to the reasoner.

use std::future::Future;
use std::pin::Pin;

use chrono::Utc;

use crate::core::store::build_session_messages;
use crate::core::PreferenceStore;
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

/// How long one recalled turn may be before it is cut.
const TURN_CHARS: usize = 400;

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

/// The workspace's name, the user's locale, and the time the call started.
///
/// A spoken assistant that cannot say what time it is has failed at the first
/// question anyone asks one.
fn who_and_where(engine: &LucidosEngine, _thread_id: uuid::Uuid) -> SectionFuture<'_> {
    Box::pin(async move {
        let pool = engine.pool();
        let timezone = read_pref(pool, "timezone").await;
        let language = read_pref(pool, "language").await;

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
        if let Some(language) = language {
            out.push_str(&format!("Speak {}.\n", language));
        }
        Ok(out)
    })
}

/// This thread's title and its recent turns.
///
/// Voice joins a conversation that already exists, so without this the talker
/// answers "what were we saying" with nothing.
fn this_thread(engine: &LucidosEngine, thread_id: uuid::Uuid) -> SectionFuture<'_> {
    Box::pin(async move {
        let store = engine.event_store();
        let mut out = String::new();
        if let Some(title) = store.get_thread_title(thread_id).await? {
            out.push_str(&format!("Title: {}\n\n", title));
        }

        let events = store
            .get_recent_thread_events(thread_id, THREAD_EVENT_WINDOW)
            .await?;
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
                clip(&message.content, TURN_CHARS)
            ));
        }
        Ok(out)
    })
}

/// The apps, triggers and unread notifications this workspace holds.
///
/// Names only. "What have I got running" is the question voice should answer
/// without a wait, and a name is enough to answer it.
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

        let unread =
            NotificationStore::get_filtered(engine.pool(), "unread", SHAPE_ITEMS as i64, None)
                .await
                .unwrap_or_default();
        let titles: Vec<String> = unread.into_iter().map(|n| n.title).collect();
        out.push_str(&list_line("Unread notifications", &titles));

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

/// A preference's value when set to something non-blank.
///
/// A read error reads as unset. A session that will not open over one
/// unreachable preference row is worse than one that cannot say the time.
async fn read_pref(pool: &sqlx::PgPool, key: &str) -> Option<String> {
    match PreferenceStore::get(pool, key).await {
        Ok(Some(v)) if !v.trim().is_empty() => Some(v),
        Ok(_) => None,
        Err(e) => {
            log!(
                "[Voice] Could not read {}: {}. Treating it as unset",
                key,
                e
            );
            None
        }
    }
}

/// Cut `text` to `max` chars on a char boundary, marking that it was cut.
fn clip(text: &str, max: usize) -> String {
    let flat = text.replace('\n', " ");
    if flat.chars().count() <= max {
        return flat;
    }
    let end = flat.floor_char_boundary(max);
    format!("{}…", &flat[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_list_says_none_rather_than_saying_nothing() {
        assert_eq!(list_line("Apps", &[]), "Apps: none\n");
    }

    #[test]
    fn a_long_list_is_capped_and_counts_the_rest() {
        let items: Vec<String> = (0..SHAPE_ITEMS + 3).map(|i| format!("t{}", i)).collect();
        let line = list_line("Triggers", &items);
        assert!(line.contains("(and 3 more)"), "{}", line);
        assert!(line.contains("t0"), "{}", line);
        assert!(!line.contains("t20,"), "{}", line);
    }

    #[test]
    fn a_clipped_turn_never_splits_a_character() {
        let text = "é".repeat(50);
        let clipped = clip(&text, 10);
        assert!(clipped.starts_with('é'));
        assert!(clipped.ends_with('…'));
    }

    #[test]
    fn a_turn_loses_its_newlines_so_one_turn_is_one_line() {
        assert_eq!(clip("a\nb\nc", 100), "a b c");
    }
}
