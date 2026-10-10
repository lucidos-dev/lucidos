pub(crate) mod accepted_messages;
pub(crate) mod agent_archive;
pub(crate) mod child_detach;
pub(crate) mod child_follow_up;
mod events;
pub(crate) mod follow_up_order;
pub(crate) mod held_deliveries;
mod held_messages;
mod images;
pub(in crate::engine) mod process;
mod process_cc;
mod process_helpers;
pub(crate) mod queued_recovery;
pub(crate) mod recovery;
mod recursion_guard;
pub(crate) mod rerun;
mod spawn;
mod title;

pub(crate) use events::images_to_hashes;
pub(crate) use events::make_message_received;
pub(crate) use events::IMAGE_DESCRIPTION_PROMPT;
pub(crate) use held_messages::answer_releases_held_messages;
pub(crate) use images::{current_image_handles, handles_note};
pub(crate) use process::PreEmittedOrigin;
pub(crate) use title::{emit_generated_title, generate_thread_title, spawn_naming, title_call};

// event_bus_tests reaches this via chat::*. Gated to test builds, since
// non-test code goes through super::recursion_guard directly.
#[cfg(test)]
pub(crate) use recursion_guard::MAX_THREAD_DEPTH;
