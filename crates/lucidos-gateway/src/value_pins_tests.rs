//! Pins every copy of a gateway or `lucidos-installs` value that cannot import
//! it (ADR 0368). Shell cannot read a Rust constant, `install.sh` runs alone
//! under `curl | sh`, and `tauri.conf.json` is config. So each copy is read here
//! and compared with the constant, rendered from the constant itself.
//!
//! `scripts/lib/harden_suites.sh` selects this test when one of these files
//! changes. Its `HS_GATEWAY_PINNED` list must name every `file` below, and
//! `harden_suites_test.sh` checks that it does.

use crate::postgres::{
    PG_IMAGE, PG_MAX_CONNECTIONS, PG_PASSWORD, PG_SHM_SIZE, PG_USER, SHARED_DOCKER_CONTAINER,
    SHARED_DOCKER_VOLUME,
};
use lucidos_installs::{BUNDLE_IDENTIFIER, DEFAULT_DEV_GATEWAY_PORT, DEFAULT_GATEWAY_PORT};
use std::path::Path;

/// One copy of a value. Every place `file` says `before`, then a slot, then
/// `after` must hold `value` in the slot, and at least one place must exist.
struct Pin {
    file: &'static str,
    before: String,
    after: &'static str,
    value: String,
}

const CONSTANTS_SH: &str = "scripts/lib/workspace_constants.sh";

/// A `NAME="value"` line in a sourced constants file.
fn shell_const(file: &'static str, name: &str, value: impl ToString) -> Pin {
    Pin {
        file,
        before: format!("\n{name}=\""),
        after: "\"",
        value: value.to_string(),
    }
}

fn pins() -> Vec<Pin> {
    vec![
        Pin {
            file: "install.sh",
            before: "\nLUCIDOS_PORT=\"${LUCIDOS_PORT:-".into(),
            after: "}\"",
            value: DEFAULT_GATEWAY_PORT.to_string(),
        },
        Pin {
            file: "scripts/lib/service.sh",
            before: "\nservice_desktop_default_port() { printf '".into(),
            after: "'; }",
            value: DEFAULT_GATEWAY_PORT.to_string(),
        },
        Pin {
            file: "crates/lucidos-app/tauri.conf.json",
            before: "\"identifier\": \"".into(),
            after: "\"",
            value: BUNDLE_IDENTIFIER.into(),
        },
        shell_const(
            CONSTANTS_SH,
            "DEFAULT_DEV_GATEWAY_PORT",
            DEFAULT_DEV_GATEWAY_PORT,
        ),
        shell_const(CONSTANTS_SH, "PG_IMAGE", PG_IMAGE),
        shell_const(CONSTANTS_SH, "PG_USER", PG_USER),
        shell_const(CONSTANTS_SH, "PG_PASSWORD", PG_PASSWORD),
        shell_const(CONSTANTS_SH, "PG_SHM_SIZE", PG_SHM_SIZE),
        shell_const(CONSTANTS_SH, "PG_MAX_CONNECTIONS", PG_MAX_CONNECTIONS),
        shell_const(
            CONSTANTS_SH,
            "SHARED_DOCKER_CONTAINER",
            SHARED_DOCKER_CONTAINER,
        ),
        shell_const(CONSTANTS_SH, "SHARED_DOCKER_VOLUME", SHARED_DOCKER_VOLUME),
        Pin {
            file: "README.md",
            before: "| `LUCIDOS_DEV_GATEWAY_PORT` | `".into(),
            after: "` |",
            value: DEFAULT_DEV_GATEWAY_PORT.to_string(),
        },
        Pin {
            file: "README.md",
            before: "packaged app's `".into(),
            after: "`).",
            value: DEFAULT_GATEWAY_PORT.to_string(),
        },
    ]
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

#[test]
fn every_copy_holds_its_rust_constant() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut drift = Vec::new();
    for pin in pins() {
        let text = std::fs::read_to_string(root.join(pin.file))
            .unwrap_or_else(|e| panic!("read {}: {e}", pin.file));
        let found = slots(&text, &pin.before, pin.after);
        if found.is_empty() {
            drift.push(format!(
                "{}: no `{}`…`{}` copy found; update this pin",
                pin.file,
                pin.before.trim_start(),
                pin.after
            ));
        }
        for slot in found.into_iter().filter(|slot| *slot != pin.value) {
            drift.push(format!(
                "{}: `{}`…`{}` holds {slot:?}, the Rust constant is {:?}",
                pin.file,
                pin.before.trim_start(),
                pin.after,
                pin.value
            ));
        }
    }
    assert!(drift.is_empty(), "copies drifted:\n{}", drift.join("\n"));
}

#[test]
fn a_pin_reads_every_copy_and_rejects_a_drifted_one() {
    let text = "\nX=\"1\"\nX=\"2\"\nX=\"unrelated text much longer than any slot ever is\"";
    assert_eq!(slots(text, "\nX=\"", "\""), vec!["1", "2"]);
    assert!(slots("Y=\"1\"", "\nX=\"", "\"").is_empty());
}
