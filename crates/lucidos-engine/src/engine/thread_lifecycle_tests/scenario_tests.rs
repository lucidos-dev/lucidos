use super::*;
use serde::Deserialize;

#[derive(Deserialize)]
struct ScenarioFile {
    scenarios: Vec<Scenario>,
}

#[derive(Deserialize)]
struct Scenario {
    name: String,
    thread_type: String,
    // Deserialized for fixture readability; the test code never inspects it.
    #[allow(dead_code)]
    description: String,
    #[serde(default)]
    steps: Vec<Step>,
    #[serde(default)]
    assert_invariant: Option<Invariant>,
    #[serde(default)]
    is_unattended: Option<bool>,
}

#[derive(Deserialize)]
struct Step {
    emit: String,
    #[serde(default)]
    expected: Option<Expected>,
    #[serde(default)]
    expect_error: Option<String>,
    #[serde(default)]
    set_pending_changes: Option<bool>,
}

#[derive(Deserialize)]
struct Expected {
    #[serde(default)]
    stored_section: Option<String>,
    #[serde(default)]
    expected_actions: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct Invariant {
    thread_type: String,
    forbidden_section: String,
}

fn load_scenarios() -> ScenarioFile {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("tests/thread-lifecycle-scenarios.json");
    let content = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("Failed to load {}: {}", path.display(), e));
    serde_json::from_str(&content).expect("Failed to parse scenarios JSON")
}

fn parse_thread_type(s: &str) -> ThreadType {
    match s {
        "chat" => ThreadType::Chat,
        "claude_code" => ThreadType::CodingAgent,
        _ => panic!("Unknown thread type: {}", s),
    }
}

#[test]
fn all_scenarios_pass() {
    let file = load_scenarios();
    for scenario in &file.scenarios {
        if scenario.steps.is_empty() {
            continue;
        }
        let thread_type = parse_thread_type(&scenario.thread_type);
        let is_unattended = scenario.is_unattended.unwrap_or(false);
        let mut current_section = ArchiveState::Archived;
        let mut has_pending_changes = false;

        for (i, step) in scenario.steps.iter().enumerate() {
            if let Some(val) = step.set_pending_changes {
                has_pending_changes = val;
            }
            let result =
                resolve_transition(&step.emit, thread_type, current_section, is_unattended);

            if let Some(expected_error) = &step.expect_error {
                assert!(
                    result.is_err(),
                    "Scenario '{}' step {} ({}): expected error containing '{}' but got Ok",
                    scenario.name,
                    i,
                    step.emit,
                    expected_error
                );
                continue;
            }

            let result = result.unwrap_or_else(|e| {
                panic!(
                    "Scenario '{}' step {} ({}): unexpected error: {}",
                    scenario.name, i, step.emit, e
                )
            });

            if let Some(new_section) = result.new_section {
                current_section = new_section;
            }

            if let Some(expected) = &step.expected {
                if let Some(expected_section) = &expected.stored_section {
                    assert_eq!(
                        current_section.as_str(),
                        expected_section.as_str(),
                        "Scenario '{}' step {} ({}): expected stored_section='{}' got '{}'",
                        scenario.name,
                        i,
                        step.emit,
                        expected_section,
                        current_section.as_str()
                    );
                }
            }

            if let Some(ea) = step
                .expected
                .as_ref()
                .and_then(|e| e.expected_actions.as_ref())
            {
                // Derive a reasonable status from thread state
                let status = if current_section == ArchiveState::Inbox {
                    match thread_type {
                        ThreadType::CodingAgent => ThreadStatus::Waiting,
                        ThreadType::Chat => ThreadStatus::Idle,
                    }
                } else {
                    ThreadStatus::Idle
                };
                let actions = available_thread_actions(
                    thread_type,
                    status,
                    current_section,
                    has_pending_changes,
                    false, // descendants_block_archive: the cross-validation contract exhausts
                    // this axis; this scenario file covers no-blocking-descendants only
                    false, // has_live_event_waits: no scenario parks on a subscription
                    false, // has_active_children: no scenario spawns one
                    false, // has_unsent_draft: scenarios predate the draft layer
                    false, // is_saved: scenarios assert the close set only
                );
                // Scenarios assert the CLOSE set (archive/apply/discard) — the
                // Save/Unsave toggle, the DiscardDraft layer and the standing
                // apply all postdate these fixtures, so filter them out of the
                // comparison.
                let action_strs: Vec<String> = actions
                    .iter()
                    .filter_map(|a| match a {
                        Action::Archive => Some("archive".to_string()),
                        Action::Apply => Some("apply".to_string()),
                        Action::Discard => Some("discard".to_string()),
                        Action::ApplyWhenSettled
                        | Action::Save
                        | Action::Unsave
                        | Action::DiscardDraft => None,
                    })
                    .collect();
                assert_eq!(
                    &action_strs, ea,
                    "Scenario '{}' step {} ({}): expected actions {:?} got {:?}",
                    scenario.name, i, step.emit, ea, action_strs
                );
            }
        }
    }
}

#[test]
fn all_invariants_hold() {
    let file = load_scenarios();
    for scenario in &file.scenarios {
        if let Some(inv) = &scenario.assert_invariant {
            let tt = parse_thread_type(&inv.thread_type);
            let forbidden = ArchiveState::parse(&inv.forbidden_section);
            // "waiting" no longer exists as a section — parse maps it to Default.
            // Only check invariants for sections that still exist.
            if inv.forbidden_section == "waiting" {
                continue;
            }
            assert!(
                !is_section_legal(tt, forbidden),
                "Invariant '{}': {:?} allows forbidden section {:?}",
                scenario.name,
                tt,
                forbidden
            );
        }
    }
}
