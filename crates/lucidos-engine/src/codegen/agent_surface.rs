//! The engine half of the tracked agent surface. Codegen adds the clap tree.

use std::collections::{BTreeMap, BTreeSet};

use crate::capability_manifest;
use crate::engine::{event_bus::SystemEvent, thread_events::ThreadEvent};
use crate::llm::{tool_names, tools};

pub(super) fn render() -> String {
    let mut offered = BTreeSet::new();
    for memory_tree in [false, true] {
        let caps = tools::ToolCapabilities {
            memory_tree,
            ..tools::ToolCapabilities::all_open()
        };
        offered.extend(
            tools::get_default_tools(&caps)
                .into_iter()
                .chain(tools::chat_tail_tools(&caps))
                .chain(capability_manifest::llm_tools_for(&caps))
                .map(|tool| tool.name),
        );
    }
    offered.insert(tool_names::WAKE_PARENT.to_owned());
    offered.insert(tools::get_notification_tool().name);
    let mut aliases = BTreeMap::new();
    for domain in capability_manifest::domains() {
        for alias in domain.alias_names() {
            if alias != domain.tool_name {
                aliases.insert(alias, domain.tool_name);
            }
        }
    }
    aliases.insert(tool_names::RUN_CLAUDE_LEGACY, tool_names::RUN_CODING_AGENT);
    let sdk_exports = sdk_exports(include_str!(
        "../../../../packages/lucidos-sdk/src/index.ts"
    ))
    .expect("SDK exports must have a complete agent surface");
    let surface = serde_json::json!({
        "schema_version": 1,
        "regenerate": super::REGENERATE,
        "tools": {
            "offered": offered,
            "aliases": aliases,
            "retired": [tool_names::EXECUTE_INTENT],
        },
        "events": {
            "thread": ThreadEvent::RESERVED_TYPE_NAMES,
            "system": SystemEvent::RESERVED_TYPE_NAMES,
            "legacy": ThreadEvent::LEGACY_TYPE_NAME_ALIASES,
        },
        "sdk_exports": sdk_exports,
    });
    serde_json::to_string_pretty(&surface).expect("agent surface serializes") + "\n"
}

fn sdk_exports(source: &str) -> Result<BTreeSet<String>, String> {
    let start = "export const lucidos = {";
    let lines: Vec<_> = source
        .lines()
        .map(|line| line.split("//").next().unwrap().trim())
        .collect();
    if lines.iter().filter(|line| **line == start).count() != 1 {
        return Err("expected one lucidos export object".into());
    }
    let begin = lines.iter().position(|line| *line == start).unwrap() + 1;
    let end = lines[begin..]
        .iter()
        .position(|line| *line == "};")
        .map(|offset| begin + offset)
        .ok_or("SDK export object has no end")?;
    // This reader accepts only explicit identifier properties. A new syntax
    // must teach the reader its meaning before codegen can certify coverage.
    let property =
        regex::Regex::new(r"^([A-Za-z_$][A-Za-z0-9_$]*)(?:\s*:\s*[A-Za-z_$][A-Za-z0-9_$]*)?,$")
            .unwrap();
    let mut exports = BTreeSet::new();
    let mut seen = BTreeSet::new();
    for line in &lines[begin..end] {
        if line.is_empty() {
            continue;
        }
        let name = property
            .captures(line)
            .ok_or_else(|| format!("unsupported SDK export property: {line}"))?[1]
            .to_owned();
        if !seen.insert(name.clone()) {
            return Err(format!("duplicate SDK export: {name}"));
        }
        // Underscore properties are SDK internals, not app-facing API.
        if !name.starts_with('_') {
            exports.insert(name);
        }
    }
    if exports.is_empty() {
        return Err("SDK export object is empty".into());
    }
    Ok(exports)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sdk_surface_tracks_public_exports_and_rejects_unknown_syntax() {
        let source =
            "export const lucidos = {\n  alpha,\n  beta: renamed,\n  _private: hidden,\n};";
        assert_eq!(
            sdk_exports(source).unwrap(),
            BTreeSet::from(["alpha".into(), "beta".into()])
        );
        assert_eq!(
            sdk_exports(&source.replace("  beta:", "  // }; ignored comment\n  beta:"))
                .unwrap()
                .len(),
            2
        );
        for source in [
            "",
            "export const lucidos = {\n};",
            "export const lucidos = {\n ...spread,\n};",
            "export const lucidos = {\n alpha,\n alpha,\n};",
            "export const lucidos = {\n alpha,",
            "export const lucidos = {\n [computed]: value,\n};",
        ] {
            assert!(sdk_exports(source).is_err(), "{source}");
        }
        let exports = sdk_exports(include_str!(
            "../../../../packages/lucidos-sdk/src/index.ts"
        ))
        .unwrap();
        for domain in capability_manifest::domains()
            .iter()
            .filter(|domain| domain.sdk)
        {
            assert!(exports.contains(domain.name), "{}", domain.name);
        }
    }

    #[test]
    fn tool_classes_are_disjoint_and_alias_targets_are_offered() {
        let surface: serde_json::Value = serde_json::from_str(&render()).unwrap();
        let tools = &surface["tools"];
        let offered: BTreeSet<_> = tools["offered"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        for (alias, target) in tools["aliases"].as_object().unwrap() {
            assert!(!offered.contains(alias.as_str()), "{alias}");
            assert!(
                offered.contains(target.as_str().unwrap()),
                "{alias} -> {target}"
            );
        }
        for name in tools["retired"].as_array().unwrap() {
            assert!(!offered.contains(name.as_str().unwrap()));
            assert!(!tools["aliases"]
                .as_object()
                .unwrap()
                .contains_key(name.as_str().unwrap()));
        }
        assert!(!surface["sdk_exports"].as_array().unwrap().is_empty());
    }
}
