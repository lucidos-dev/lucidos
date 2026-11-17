//! Classified diagnostic records shared by Lucidos processes.

extern crate self as lucidos_diagnostic_log;

pub mod bundle;
pub mod fault;
pub mod redaction;
pub mod sanitize;
pub mod snapshot;
pub mod stderr;
mod values;
pub mod writer;

pub use lucidos_diagnostic_log_macros::{line, log};
pub use values::{check_value, private, ErrorKind, LogValue, Private};

use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Destination {
    Maintainer,
    Owner,
}

/// Every field remains classified on disk; no cached rendered message is
/// trusted when preparing a maintainer bundle.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "class", rename_all = "kebab-case")]
pub enum Fragment {
    Literal { text: String },
    Shareable { text: String },
    Private { text: String, chars: usize },
}

impl Fragment {
    pub fn literal(text: &'static str) -> Self {
        Self::Literal {
            text: sanitize::for_write(text, false),
        }
    }

    pub fn value<T: LogValue + ?Sized>(value: &T, formatted: String) -> Self {
        let text = sanitize::for_write(&formatted, false);
        if value.is_private() {
            Self::Private {
                text,
                chars: formatted.chars().count(),
            }
        } else {
            Self::Shareable { text }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Line {
    Typed {
        template: String,
        fragments: Vec<Fragment>,
    },
    Legacy {
        text: String,
    },
}

impl Line {
    pub fn typed(template: &'static str, fragments: Vec<Fragment>) -> Self {
        Self::Typed {
            template: template.into(),
            fragments,
        }
    }

    pub fn legacy(text: String) -> Self {
        let (text, cut) = sanitize::bound_raw(text);
        Self::Legacy {
            text: sanitize::for_write(&text, cut),
        }
    }

    /// Bound structured payloads as well as cached text before queuing them.
    pub(crate) fn bounded(self) -> Self {
        match self {
            Self::Typed {
                template,
                fragments,
            } => {
                let template = sanitize::for_write(&template, false);
                let mut remaining = 16 * 1024usize;
                let mut kept = Vec::new();
                for fragment in fragments {
                    let text = match &fragment {
                        Fragment::Literal { text }
                        | Fragment::Shareable { text }
                        | Fragment::Private { text, .. } => text,
                    };
                    let cost = text.len().saturating_add(64);
                    if cost > remaining {
                        kept.push(Fragment::literal(" [truncated]"));
                        break;
                    }
                    remaining -= cost;
                    kept.push(fragment);
                }
                Self::Typed {
                    template,
                    fragments: kept,
                }
            }
            Self::Legacy { text } => Self::legacy(text),
        }
    }

    pub fn render(&self, destination: Destination) -> String {
        match self {
            Self::Legacy { .. } if destination == Destination::Maintainer => {
                "<unstructured line>".into()
            }
            Self::Legacy { text } => sanitize::for_read(text),
            Self::Typed { fragments, .. } => sanitize::for_read(
                &fragments
                    .iter()
                    .map(|part| match part {
                        Fragment::Private { chars, .. }
                            if destination == Destination::Maintainer =>
                        {
                            format!("<text {chars} chars>")
                        }
                        Fragment::Literal { text }
                        | Fragment::Shareable { text }
                        | Fragment::Private { text, .. } => text.clone(),
                    })
                    .collect::<String>(),
            ),
        }
    }
}

static SINK: OnceLock<writer::Sink> = OnceLock::new();

pub fn start(dir: &Path) -> std::io::Result<()> {
    if SINK.get().is_some() {
        return Ok(());
    }
    let segments = writer::Segments::open(dir)?;
    let (sink, rx, dropped) = writer::channel(writer::CHANNEL_CAPACITY);
    std::thread::Builder::new()
        .name("diagnostic-log".into())
        .spawn(move || writer::drain(rx, segments, &dropped))?;
    let _ = SINK.set(sink);
    Ok(())
}

pub fn started() -> bool {
    SINK.get().is_some()
}

pub fn record_legacy(label: &'static str, msg: String) {
    if let Some(sink) = SINK.get() {
        sink.record(label, msg);
    }
}

pub fn record(label: &'static str, line: Line) {
    if let Some(sink) = SINK.get() {
        sink.record_line(label, line);
    }
}

/// Wait at most `timeout` for records already queued. A process without a sink
/// has nothing to flush. Used at process exit, including fatal boot failures.
pub fn flush(timeout: std::time::Duration) -> bool {
    SINK.get().is_none_or(|sink| sink.flush(timeout))
}

/// The hook also calls the previous hook, preserving the host's panic output.
pub fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current();
        let message = info
            .payload()
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| info.payload().downcast_ref::<&str>().copied())
            .unwrap_or("non-text panic payload");
        let location = info.location();
        let thread_name = thread.name().unwrap_or("unnamed");
        let file = location.map(|loc| loc.file()).unwrap_or("unknown");
        let row = location.map(|loc| loc.line()).unwrap_or(0);
        record("Panic", panic_line(thread_name, file, row, message));
        // A fatal main-thread panic must not exit with its record still queued.
        flush(std::time::Duration::from_secs(1));
        previous(info);
    }));
}

fn panic_line(thread_name: &str, file: &str, row: u32, message: &str) -> Line {
    // Thread names and locations originate in code; the payload is private.
    Line::typed(
        "panic",
        vec![
            Fragment::Shareable {
                text: sanitize::for_write(&format!("panic {thread_name} {file}:{row}: "), false),
            },
            Fragment::value(&private(message), message.into()),
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panic_messages_are_private_and_sanitized_before_storage() {
        let line = panic_line(
            "worker",
            "src/worker.rs",
            7,
            "PRIVATE_PANIC_MARKER token=opaque-panic-token",
        );
        let maintainer = line.render(Destination::Maintainer);
        assert!(maintainer.contains("worker src/worker.rs:7"));
        assert!(!maintainer.contains("PRIVATE_PANIC_MARKER"));
        assert!(!serde_json::to_string(&line)
            .unwrap()
            .contains("opaque-panic-token"));
    }
}
