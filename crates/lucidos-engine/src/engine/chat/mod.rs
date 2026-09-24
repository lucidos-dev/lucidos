pub(crate) mod child_follow_up;
mod events;
mod held_messages;
mod images;
pub(in crate::engine) mod process;
mod process_cc;
mod process_helpers;
pub(crate) mod queued_recovery;
mod recovery;
mod recursion_guard;
pub(crate) mod rerun;
mod spawn;
mod title;

pub(crate) use events::make_message_received;
pub(crate) use events::IMAGE_DESCRIPTION_PROMPT;
pub(crate) use held_messages::answer_releases_held_messages;
pub(crate) use process::PreEmittedOrigin;
pub(crate) use title::{
    emit_generated_title, exchange_has_both_speakers, generate_thread_title, spawn_naming,
    spoken_exchange_as_title_input, title_call,
};

// event_bus_tests reaches these via chat::* — gated to test builds since
// non-test code goes through super::recursion_guard directly.
#[cfg(test)]
pub(crate) use recursion_guard::{MAX_CHILDREN_PER_THREAD, MAX_THREAD_DEPTH};
