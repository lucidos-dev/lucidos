//! The SDK's copy of the routes an app may reach, which the bridge reads.

use super::*;
use crate::codegen::REGENERATE;

/// The `App` rows, and only those.
///
/// Emitting the denials as well would ship a map of the engine to every app,
/// and the client needs none of it: absence already means refused.
pub(crate) fn render() -> String {
    let mut out = String::new();
    out.push_str("// AUTO-GENERATED. Do not edit by hand.\n");
    out.push_str(&format!("// Regenerate: {REGENERATE}\n"));
    out.push_str("//\n");
    out.push_str("// Source of truth: ROUTE_REACH in crates/lucidos-engine/src/api/app_reach.rs\n");
    out.push_str("// (ADR 0231). Only app-reachable routes appear. A route absent from this\n");
    out.push_str("// table is refused, which is what makes the list safe to be short.\n\n");
    out.push_str("export interface AppReachableRoute {\n");
    out.push_str("  /** Route pattern, with `:param` and `*wildcard` segments. */\n");
    out.push_str("  path: string;\n");
    out.push_str("  methods: string[];\n");
    out.push_str("}\n\n");
    out.push_str("export const APP_REACHABLE_ROUTES: AppReachableRoute[] = [\n");
    for (path, reach, methods) in ROUTE_REACH {
        if *reach != App {
            continue;
        }
        let list = methods
            .iter()
            .map(|m| format!("'{m}'"))
            .collect::<Vec<_>>()
            .join(", ");
        out.push_str(&format!("  {{ path: '{path}', methods: [{list}] }},\n"));
    }
    out.push_str("];\n");
    out
}
