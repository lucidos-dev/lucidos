#[macro_export]
macro_rules! notes_reach_user_sentence {
    () => {
        "Before a tool call, your prose may reach them only as a short note of a sentence or \
         two, however much you drafted."
    };
}

pub use crate::notes_reach_user_sentence;

pub(crate) const NOTES_NOW_SHOWN_REMINDER: &str = concat!(
    "<system-reminder>\nUpdate: your notes now reach the user. ",
    notes_reach_user_sentence!(),
    "\n</system-reminder>"
);

pub(crate) const NOTES_HIDDEN_AGAIN_REMINDER: &str = "<system-reminder>\nUpdate: your notes count \
    as hidden again until one arrives. Text you write in a message that also calls a tool may \
    NEVER reach the user. Put what they must read in a message that calls no tool, or on the \
    question card, once.\n</system-reminder>";
