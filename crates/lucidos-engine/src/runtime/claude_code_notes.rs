//! Whether a Claude Code session's progress notes reach the user.
//!
//! An always-thinking model writes its prose before a tool call into a
//! `thinking` block. The block carries text only when the request sets
//! `thinking.display: "updates"`. Claude Code sets it for current models, but
//! nothing at spawn says whether it did: a HIPAA org, the beta switch in a
//! settings file and a provider that rejects the setting all leave the block
//! empty. The agent's own context still holds the text it wrote there.
//!
//! So a session starts on the hidden rule and the stream proves the notes: the
//! parser promotes the session when a `thinking` block carries text
//! (`CcStreamState`). Nothing in the stream proves them hidden, because an
//! empty block is also the model's own reasoning. ADR 0608.
//!
//! A temporary measure: `docs/temporary-measures.md` § "Claude Code's progress
//! notes start hidden".

use crate::llm::anthropic_wire::{thinking_mode, ThinkingMode};

/// Whether the text a model writes before a tool call reaches the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotesVisibility {
    Shown,
    Hidden,
}

/// The verdict for a session about to spawn on `spawn_model`. Only a model the
/// registry knows to think adaptively without always thinking starts shown: its
/// prose before a tool call is a `text` block, which always shows. An
/// always-thinking model and an unknown one start hidden.
pub(crate) fn notes_visibility(spawn_model: Option<&str>) -> NotesVisibility {
    match spawn_model.and_then(thinking_mode) {
        Some(ThinkingMode::OffByDefault | ThinkingMode::OnByDefault) => NotesVisibility::Shown,
        Some(ThinkingMode::AlwaysOn) | None => NotesVisibility::Hidden,
    }
}

#[cfg(test)]
#[path = "claude_code_notes_tests.rs"]
mod tests;
