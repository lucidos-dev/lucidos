//! The app's copy of the notes a permission card shows (ADR 0368).

use lucidos_core::engine::thread_events::UNPROVEN_WORKSPACE_DESTRUCTION_NOTE;

use super::{quoted, REGENERATE};

pub(super) fn render() -> String {
    let mut out = String::new();
    out.push_str("// AUTO-GENERATED. Do not edit by hand.\n");
    out.push_str(&format!("// Regenerate: {REGENERATE}\n"));
    out.push_str("//\n");
    out.push_str("// Source of truth: the constant each export names, in\n");
    out.push_str("// crates/lucidos-core/src/engine/thread_events/tool_grant.rs.\n\n");
    out.push_str("/** `UNPROVEN_WORKSPACE_DESTRUCTION_NOTE` in `lucidos-core`. */\n");
    out.push_str(&format!(
        "export const UNPROVEN_WORKSPACE_DESTRUCTION_NOTE = {};\n",
        quoted(UNPROVEN_WORKSPACE_DESTRUCTION_NOTE)
    ));
    out
}
