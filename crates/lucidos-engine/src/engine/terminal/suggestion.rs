//! Terminal suggestions (ADR 0820): a command an agent offers, which the
//! engine types into a thread terminal and never submits.

use uuid::Uuid;

use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{EventMeta, ThreadEvent};

pub const MAX_COMMAND_BYTES: usize = 4096;
pub const MAX_LABEL_CHARS: usize = 120;
/// The refusal reason when a program other than the shell holds the terminal.
pub const BUSY_REASON: &str = "terminal-busy";

#[derive(Debug, PartialEq, Eq)]
pub enum Refusal {
    Empty,
    TooLong,
    LabelTooLong,
    /// A newline, a control character or a character that reorders or hides
    /// text. Any of them could submit the line or disguise what it runs.
    Unprintable,
}

impl Refusal {
    pub fn message(&self) -> String {
        match self {
            Refusal::Empty => "The command is empty.".to_string(),
            Refusal::TooLong => {
                format!("The command is longer than {MAX_COMMAND_BYTES} bytes.")
            }
            Refusal::LabelTooLong => {
                format!("The label is longer than {MAX_LABEL_CHARS} characters.")
            }
            Refusal::Unprintable => "The command and label must be one line of printable text, \
                 with no control or direction characters."
                .to_string(),
        }
    }
}

/// INVARIANT: A suggestion never executes by itself. It holds no newline, no
/// control character and no character that changes how the line displays.
pub fn validate(command: &str) -> Result<(), Refusal> {
    if command.trim().is_empty() {
        return Err(Refusal::Empty);
    }
    if command.len() > MAX_COMMAND_BYTES {
        return Err(Refusal::TooLong);
    }
    if command.chars().any(unprintable) {
        return Err(Refusal::Unprintable);
    }
    Ok(())
}

pub fn validate_label(label: &str) -> Result<(), Refusal> {
    if label.chars().count() > MAX_LABEL_CHARS {
        return Err(Refusal::LabelTooLong);
    }
    if label.chars().any(unprintable) {
        return Err(Refusal::Unprintable);
    }
    Ok(())
}

fn unprintable(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{2028}' | '\u{2029}' | '\u{200e}' | '\u{200f}' | '\u{061c}'
        )
        || ('\u{202a}'..='\u{202e}').contains(&c)
        || ('\u{2066}'..='\u{2069}').contains(&c)
}

/// Post a suggestion to `thread_id`. Needs no live terminal, and works with
/// the switch off: the row then tells the user to turn Terminal on.
pub async fn post(
    bus: &EventBus,
    thread_id: Uuid,
    command: &str,
    label: Option<&str>,
    meta: EventMeta,
) -> Result<Uuid, PostError> {
    validate(command).map_err(PostError::Refused)?;
    let label = label.map(str::trim).filter(|label| !label.is_empty());
    if let Some(label) = label {
        validate_label(label).map_err(PostError::Refused)?;
    }
    let suggestion_id = Uuid::new_v4();
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::TerminalCommandSuggested {
            suggestion_id,
            command: command.to_string(),
            label: label.map(str::to_string),
        },
        meta,
    })
    .await
    .map_err(|e| PostError::Emit(e.to_string()))?;
    Ok(suggestion_id)
}

#[derive(Debug)]
pub enum PostError {
    Refused(Refusal),
    Emit(String),
}

/// The bytes that type `command` at a prompt. Bracketed paste wraps it when
/// the shell turned that mode on, so the line editor takes it as one paste.
pub fn typed_bytes(command: &str, bracketed_paste: bool) -> Vec<u8> {
    if bracketed_paste {
        [b"\x1b[200~", command.as_bytes(), b"\x1b[201~"].concat()
    } else {
        command.as_bytes().to_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_command_passes() {
        assert_eq!(
            validate("claude mcp login 'example-server' --no-browser"),
            Ok(())
        );
        assert_eq!(validate("echo héllo ✓"), Ok(()));
    }

    #[test]
    fn anything_that_could_submit_or_disguise_the_line_is_refused() {
        for bad in [
            "echo a\nrm -rf ~",
            "echo a\r",
            "echo \u{1b}[2K",
            "echo \u{7f}",
            "echo \u{85}",
            "echo \u{2028}",
            "echo \u{202e}fdp.exe",
            "echo \u{2066}x",
            "echo\ta",
        ] {
            assert_eq!(validate(bad), Err(Refusal::Unprintable), "{bad:?}");
        }
    }

    #[test]
    fn empty_and_oversized_commands_are_refused() {
        assert_eq!(validate("   "), Err(Refusal::Empty));
        assert_eq!(
            validate(&"a".repeat(MAX_COMMAND_BYTES + 1)),
            Err(Refusal::TooLong)
        );
    }

    #[test]
    fn typed_bytes_never_end_with_a_newline() {
        let raw = typed_bytes("ls", false);
        let wrapped = typed_bytes("ls", true);
        assert_eq!(raw, b"ls");
        assert_eq!(wrapped, b"\x1b[200~ls\x1b[201~");
        assert!(!raw.contains(&b'\n') && !wrapped.contains(&b'\n') && !wrapped.contains(&b'\r'));
    }
}
