mod events;
mod images;
mod process;
mod recovery;
mod recursion_guard;
mod spawn;
mod title;

pub(crate) use events::make_message_received;
pub(crate) use title::{emit_generated_title, generate_thread_title};

// event_bus_tests reaches these via chat::* — gated to test builds since
// non-test code goes through super::recursion_guard directly.
#[cfg(test)]
pub(crate) use recursion_guard::{MAX_CHILDREN_PER_THREAD, MAX_THREAD_DEPTH};
