//! Pins every copy of an engine value that cannot import it (ADR 0368). The
//! LLM reads `system-knowhow/` raw, and shell cannot read a Rust constant. So
//! each copy is read here and compared with the constant, rendered from the
//! constant itself. A number a reader does not need leaves the doc instead.
//!
//! `scripts/lib/harden_suites.sh` selects this test for a `system-knowhow/`
//! change, and for the scripts in its `HS_ENGINE_PINNED` list. That list must
//! name every other `file` below, and `harden_suites_test.sh` checks it.

use crate::api::proxy_timeout::{MAX_SECS as PROXY_MAX_SECS, MIN_SECS as PROXY_MIN_SECS};
use crate::core::oauth::{CALLBACK_PATH, CALLBACK_PORT};
use crate::core::plugins::MAX_ID_LEN as PLUGIN_MAX_ID_LEN;
use crate::core::prefs::{
    CHAT_MODEL, LOCAL_BASE_URL, MAX_TOOL_CALLS, PROXY_TIMEOUT_SECS, VERTEX_REGION,
};
use crate::core::shell::MAX_OUTPUT_BYTES;
use crate::core::workspace_fonts::{
    MAX_FACES, MAX_FILE_BYTES, MAX_FONTS, MAX_LABEL_CHARS, MAX_SLUG_LEN,
};
use crate::engine::chat::MAX_THREAD_DEPTH;
use crate::engine::event_wait::{MAX_LIVE_WAITS_PER_THREAD, MAX_TIMEOUT_SECS as WAIT_MAX_SECS};
use crate::engine::thread_queue::{
    DEFAULT_MAX_CONCURRENT_CHILDREN_PER_THREAD, DEFAULT_MAX_EVENT_TRIGGER_DEPTH,
};
use crate::engine::tools::bash_background::BASH_OUTPUT_MAX_WAIT_SECS;
use crate::engine::tools::todo::MAX_TODO_ITEMS;
use crate::llm::tools::{
    BG_DEFAULT_TIMEOUT_SECS, BG_MAX_TIMEOUT_SECS, MAX_TIMEOUT_SECS as RUN_MAX_SECS,
};
use crate::paths::WORKTREES_SUBPATH;
use std::path::Path;

/// One copy of a value. Every place `file` says `before`, then a slot, then
/// `after` must hold `value` in the slot, and at least one place must exist.
struct Pin {
    file: &'static str,
    before: String,
    after: String,
    value: String,
}

fn pin(
    file: &'static str,
    before: impl ToString,
    after: impl ToString,
    value: impl ToString,
) -> Pin {
    Pin {
        file,
        before: before.to_string(),
        after: after.to_string(),
        value: value.to_string(),
    }
}

const CONSTANTS_SH: &str = "scripts/lib/workspace_constants.sh";
const RUNNING_PYTHON: &str = "system-knowhow/running-python.md";
const THREAD_EVENTS: &str = "system-knowhow/thread-events.md";
const LUCIDOS_CLI: &str = "system-knowhow/lucidos-cli.md";
const JS_SDK: &str = "system-knowhow/js-sdk.md";
const OAUTH_PROVIDERS: &str = "system-knowhow/oauth-providers.md";
const GLOSSARY: &str = "system-knowhow/glossary.md";
const FONTS: &str = "system-knowhow/workspace-fonts.md";

fn script_pins() -> Vec<Pin> {
    vec![
        pin(
            CONSTANTS_SH,
            "\nWORKTREES_SUBPATH=\"",
            "\"",
            WORKTREES_SUBPATH,
        ),
        pin(
            "scripts/status.sh",
            "${VERTEX_REGION:-",
            "}\"",
            VERTEX_REGION.default_text(),
        ),
        pin(
            "README.md",
            "| `VERTEX_REGION` | `",
            "` |",
            VERTEX_REGION.default_text(),
        ),
        pin(
            "README.md",
            "| `LUCIDOS_MODEL` | `",
            "@default` |",
            CHAT_MODEL.default_text(),
        ),
    ]
}

fn knowhow_pins() -> Vec<Pin> {
    let output_cap = format!("{} KB", MAX_OUTPUT_BYTES / 1024);
    let wait_hours = WAIT_MAX_SECS / 3600;
    let mut pins = vec![
        // The sync and background tool ceilings, and the drain.
        pin(
            RUNNING_PYTHON,
            "the first ",
            ", then a `[truncated",
            &output_cap,
        ),
        pin(RUNNING_PYTHON, "a file. | ", " s hard", RUN_MAX_SECS),
        pin(
            RUNNING_PYTHON,
            "`run_python`'s ",
            " s ceiling",
            RUN_MAX_SECS,
        ),
        pin(RUNNING_PYTHON, "hits the ", " s ceiling", RUN_MAX_SECS),
        pin(RUNNING_PYTHON, "timed out after ", "s`", RUN_MAX_SECS),
        pin(
            RUNNING_PYTHON,
            "`timeout_secs` (default ",
            " s,",
            BG_DEFAULT_TIMEOUT_SECS,
        ),
        pin(RUNNING_PYTHON, " s, max ", " s)", BG_MAX_TIMEOUT_SECS),
        pin(
            RUNNING_PYTHON,
            "| up to ",
            " s per call",
            BASH_OUTPUT_MAX_WAIT_SECS,
        ),
        pin(
            RUNNING_PYTHON,
            "- Up to ",
            " s per call",
            BASH_OUTPUT_MAX_WAIT_SECS,
        ),
        pin(
            RUNNING_PYTHON,
            "Use the full ",
            " s for",
            BASH_OUTPUT_MAX_WAIT_SECS,
        ),
        pin(
            RUNNING_PYTHON,
            "Twenty ",
            "-second drains",
            BASH_OUTPUT_MAX_WAIT_SECS,
        ),
        pin(
            RUNNING_PYTHON,
            "several `wait_secs=",
            "` drains",
            BASH_OUTPUT_MAX_WAIT_SECS,
        ),
        pin(
            RUNNING_PYTHON,
            "A drain is capped at ",
            " per stream",
            &output_cap,
        ),
        pin(
            THREAD_EVENTS,
            "`stderr` are capped at ",
            " each",
            &output_cap,
        ),
        // The trigger chain ceiling.
        pin(
            THREAD_EVENTS,
            "field, default ",
            ")",
            DEFAULT_MAX_EVENT_TRIGGER_DEPTH,
        ),
        pin(
            "system-knowhow/triggers.md",
            "field, default ",
            ")",
            DEFAULT_MAX_EVENT_TRIGGER_DEPTH,
        ),
        pin(
            "system-knowhow/coding-agent-events.md",
            "`max_event_trigger_depth` (default ",
            ")",
            DEFAULT_MAX_EVENT_TRIGGER_DEPTH,
        ),
        pin(
            "system-knowhow/thread-queue.md",
            "`max_event_trigger_depth` (",
            "):",
            DEFAULT_MAX_EVENT_TRIGGER_DEPTH,
        ),
        // The per-thread child cap.
        pin(
            "system-knowhow/thread-queue.md",
            "`max_concurrent_children_per_thread` (",
            "):",
            DEFAULT_MAX_CONCURRENT_CHILDREN_PER_THREAD,
        ),
        pin(
            "system-knowhow/orchestrating-sub-threads.md",
            "**At most ",
            " children running at the same time.**",
            DEFAULT_MAX_CONCURRENT_CHILDREN_PER_THREAD,
        ),
        pin(
            GLOSSARY,
            "of them at the same time (*capacity policy*, default ",
            ")",
            DEFAULT_MAX_CONCURRENT_CHILDREN_PER_THREAD,
        ),
        // The todo list and sub-thread depth.
        pin(GLOSSARY, "(at most ", " items", MAX_TODO_ITEMS),
        pin(THREAD_EVENTS, "enforces ≤ ", " items", MAX_TODO_ITEMS),
        pin(
            "system-knowhow/orchestrating-sub-threads.md",
            "**Depth caps at ",
            ".**",
            MAX_THREAD_DEPTH,
        ),
        pin(
            "system-knowhow/orchestrating-sub-threads.md",
            "A spawn that would make ",
            " is refused",
            MAX_THREAD_DEPTH + 1,
        ),
        // Event waits.
        pin(
            LUCIDOS_CLI,
            "`--timeout-secs` is required and capped at ",
            " (",
            WAIT_MAX_SECS,
        ),
        pin(
            LUCIDOS_CLI,
            format!("capped at {WAIT_MAX_SECS} ("),
            " h)",
            wait_hours,
        ),
        pin(
            THREAD_EVENTS,
            "**required** and capped at **",
            " hours**",
            wait_hours,
        ),
        pin(
            LUCIDOS_CLI,
            "may hold at most ",
            " live subscriptions",
            MAX_LIVE_WAITS_PER_THREAD,
        ),
        pin(
            THREAD_EVENTS,
            "may hold **",
            " live waits**",
            MAX_LIVE_WAITS_PER_THREAD,
        ),
        // Proxy timeouts.
        pin(
            LUCIDOS_CLI,
            "The engine waits **",
            " seconds**",
            PROXY_TIMEOUT_SECS.default_number(),
        ),
        pin(LUCIDOS_CLI, "both accept ", " to ", PROXY_MIN_SECS),
        pin(
            LUCIDOS_CLI,
            format!("both accept {PROXY_MIN_SECS} to "),
            " seconds",
            PROXY_MAX_SECS,
        ),
        pin(LUCIDOS_CLI, "A value outside ", " to ", PROXY_MIN_SECS),
        pin(
            LUCIDOS_CLI,
            format!("outside {PROXY_MIN_SECS} to "),
            " is refused",
            PROXY_MAX_SECS,
        ),
        pin(
            LUCIDOS_CLI,
            "capped at ",
            " seconds in total",
            PROXY_MAX_SECS,
        ),
        pin(
            JS_SDK,
            "The engine waits ",
            " seconds on the upstream",
            PROXY_TIMEOUT_SECS.default_number(),
        ),
        pin(JS_SDK, "Both accept ", " to ", PROXY_MIN_SECS),
        pin(
            JS_SDK,
            format!("Both accept {PROXY_MIN_SECS} to "),
            " (",
            PROXY_MAX_SECS,
        ),
        pin(JS_SDK, "never runs past ", " seconds", PROXY_MAX_SECS),
        pin(
            "system-knowhow/building-an-auth-handshake.md",
            "this entry, from ",
            " to ",
            PROXY_MIN_SECS,
        ),
        pin(
            "system-knowhow/building-an-auth-handshake.md",
            format!("this entry, from {PROXY_MIN_SECS} to "),
            ".",
            PROXY_MAX_SECS,
        ),
        // Workspace fonts.
        pin(FONTS, "| Up to ", " lowercase letters", MAX_SLUG_LEN),
        pin(
            FONTS,
            "each at most ",
            ".",
            format!("{} MiB", MAX_FILE_BYTES / (1024 * 1024)),
        ),
        pin(FONTS, "| 1 to ", " per font |", MAX_FACES),
        pin(FONTS, "| Fonts | ", " per workspace", MAX_FONTS),
        pin(FONTS, "| Label | 1 to ", " characters |", MAX_LABEL_CHARS),
        // Plugin ids.
        pin(
            "system-knowhow/plugins.md",
            "max ",
            " chars",
            PLUGIN_MAX_ID_LEN,
        ),
        // Preferences.
        pin(
            JS_SDK,
            "region default `",
            "`",
            VERTEX_REGION.default_text(),
        ),
        pin(
            JS_SDK,
            "Ollama default `",
            "`",
            LOCAL_BASE_URL.default_text(),
        ),
        pin(
            GLOSSARY,
            "The default is Ollama's `",
            "`",
            LOCAL_BASE_URL.default_text(),
        ),
        pin(
            "system-knowhow/preferences.md",
            "any number of at least `",
            "`",
            MAX_TOOL_CALLS.bounds().0,
        ),
        pin(
            "system-knowhow/preferences.md",
            "is raised to `",
            "`",
            MAX_TOOL_CALLS.bounds().0,
        ),
        // The OAuth listener port the user registers with a provider.
        pin(OAUTH_PROVIDERS, "loopback port (", ")", CALLBACK_PORT),
    ];
    for doc in [GLOSSARY, OAUTH_PROVIDERS] {
        for host in ["http://127.0.0.1:", "http://localhost:", "http://[::1]:"] {
            pins.push(pin(doc, host, CALLBACK_PATH, CALLBACK_PORT));
        }
    }
    pins
}

/// The longest slot read as a copy. A longer gap between `before` and `after`
/// is unrelated text that happens to share the prefix.
const SLOT_CHARS: usize = 40;

/// Every slot `text` fills between `before` and `after`.
fn slots<'a>(text: &'a str, before: &str, after: &str) -> Vec<&'a str> {
    let mut found = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find(before) {
        rest = &rest[start + before.len()..];
        if let Some(end) = rest.find(after) {
            let slot = &rest[..end];
            if !slot.contains('\n') && slot.chars().count() <= SLOT_CHARS {
                found.push(slot);
            }
        }
    }
    found
}

/// Every way the pins in `pins` drifted from their files, one line each.
fn drift(pins: Vec<Pin>) -> Vec<String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut drift = Vec::new();
    for pin in pins {
        let text = std::fs::read_to_string(root.join(pin.file))
            .unwrap_or_else(|e| panic!("read {}: {e}", pin.file));
        let found = slots(&text, &pin.before, &pin.after);
        let anchor = format!("`{}`…`{}`", pin.before.trim_start(), pin.after);
        if found.is_empty() {
            drift.push(format!(
                "{}: no {anchor} copy found; update this pin",
                pin.file
            ));
        }
        for slot in found.into_iter().filter(|slot| *slot != pin.value) {
            drift.push(format!(
                "{}: {anchor} holds {slot:?}, the Rust constant is {:?}",
                pin.file, pin.value
            ));
        }
    }
    drift
}

#[test]
fn every_script_copy_holds_its_rust_constant() {
    let drift = drift(script_pins());
    assert!(drift.is_empty(), "copies drifted:\n{}", drift.join("\n"));
}

#[test]
fn every_knowhow_copy_holds_its_rust_constant() {
    let drift = drift(knowhow_pins());
    assert!(drift.is_empty(), "copies drifted:\n{}", drift.join("\n"));
}

#[test]
fn a_pin_reads_every_copy_and_rejects_a_drifted_one() {
    let text = "max 64 chars, max 65 chars, max out a budget that runs on for far longer than any slot chars";
    assert_eq!(slots(text, "max ", " chars"), vec!["64", "65"]);
    assert!(slots("no copy here", "max ", " chars").is_empty());
}
