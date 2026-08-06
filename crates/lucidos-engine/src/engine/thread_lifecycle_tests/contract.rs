use super::*;

const CC_ONLY_EVENTS: &[&str] = &[
    "SessionStarted",
    "SessionEnded",
    "CodingAgentTextStreamed",
    "CodingAgentThoughtStreamed",
    "CodingAgentToolCalled",
    "CodingAgentToolResult",
    "CodingAgentUserMessageSent",
    "CodingAgentPromptSent",
    "CodingAgentIdled",
    "ContinuationRequested",
    "MissingHardeningDetected",
    "CodingAgentSettingsChanged",
    "UserQuestionAsked",
    "UserQuestionAnswered",
];

/// Every `ThreadStatus` variant with its wire string, in one place.
///
/// Source for BOTH halves of the contract: the fixture's status dimension and
/// the generated TS union + `THREAD_STATUSES` list. Keeping them as two
/// separate literals is how the hardcoded fixture-size assertions went stale.
/// `all_statuses_covers_the_enum` pins it against `ThreadStatus::parse`.
const ALL_STATUSES: &[(&str, ThreadStatus)] = &[
    ("idle", ThreadStatus::Idle),
    ("running", ThreadStatus::Running),
    ("waiting", ThreadStatus::Waiting),
    (
        "waiting_for_user_answer",
        ThreadStatus::WaitingForUserAnswer,
    ),
    ("paused", ThreadStatus::Paused),
    ("failed", ThreadStatus::Failed),
];

fn generate_cross_validation_fixture() -> String {
    let thread_types = [
        ("chat", ThreadType::Chat),
        ("claude_code", ThreadType::CodingAgent),
    ];
    let statuses = ALL_STATUSES;
    let sections = [
        ("archived", ArchiveState::Archived),
        ("inbox", ArchiveState::Inbox),
    ];
    let bools = [false, true];

    let mut cases = Vec::new();

    // availableThreadActions: the full cross product of thread_types ×
    // statuses × sections × pending × descendants_block_archive ×
    // has_unsent_draft × is_saved. Adding a `ThreadStatus` variant widens the
    // fixture automatically; do not hardcode the case count here, it drifts.
    for (tt_str, tt) in &thread_types {
        for (st_str, st) in statuses {
            for (sec_str, sec) in &sections {
                for &pending in &bools {
                    for &dba in &bools {
                        for &draft in &bools {
                            for &saved in &bools {
                                let actions: Vec<&str> = available_thread_actions(
                                    *tt, *st, *sec, pending, dba, draft, saved,
                                )
                                .iter()
                                .map(|a| match a {
                                    Action::DiscardDraft => "discard_draft",
                                    Action::Discard => "discard",
                                    Action::Apply => "apply",
                                    Action::Archive => "archive",
                                    Action::Save => "save",
                                    Action::Unsave => "unsave",
                                })
                                .collect();
                                cases.push(format!(
                                        r#"    {{ "fn": "availableThreadActions", "args": [{:?}, {:?}, {:?}, {}, {}, {}, {}], "expected": [{}] }}"#,
                                        tt_str, st_str, sec_str, pending, dba, draft, saved,
                                        actions.iter().map(|a| format!("{:?}", a)).collect::<Vec<_>>().join(", ")
                                    ));
                            }
                        }
                    }
                }
            }
        }
    }

    // displaySection: the full cross product of sections × statuses × saved ×
    // activeChildren × pending × attentionDescendants.
    for (sec_str, sec) in &sections {
        for (st_str, st) in statuses {
            for &saved in &bools {
                for &active_children in &bools {
                    for &pending in &bools {
                        for &attention in &bools {
                            let result = display_section(
                                *sec,
                                *st,
                                saved,
                                active_children,
                                pending,
                                attention,
                            );
                            let result_str = match result {
                                DisplaySection::Saved => "saved",
                                DisplaySection::Current => "current",
                                DisplaySection::Archive => "archive",
                            };
                            cases.push(format!(
                                    r#"    {{ "fn": "displaySection", "args": [{:?}, {:?}, {}, {}, {}, {}], "expected": {:?} }}"#,
                                    sec_str, st_str, saved, active_children, pending, attention, result_str
                                ));
                        }
                    }
                }
            }
        }
    }

    format!("{{\n  \"cases\": [\n{}\n  ]\n}}\n", cases.join(",\n"))
}

fn generate_typescript() -> String {
    let mut out = String::new();

    // Header
    out.push_str("// AUTO-GENERATED by thread_lifecycle.rs — do not edit by hand.\n");
    out.push_str(
        "// Regenerate: cargo test -p lucidos-engine generate_typescript_file -- --ignored\n\n",
    );

    // Type aliases
    out.push_str("export type EventChannel = 'chat' | 'claude_code' | 'trigger';\n");
    out.push_str("export const EVENT_CHANNELS: readonly EventChannel[] = ['chat', 'claude_code', 'trigger'] as const;\n");
    out.push_str("export type SessionEndReason = 'shutdown' | 'panic' | 'closed' | 'stale_resume' | 'legacy_non_terminal';\n");
    out.push_str("export const SESSION_END_REASONS: readonly SessionEndReason[] = ['shutdown', 'panic', 'closed', 'stale_resume', 'legacy_non_terminal'] as const;\n");
    out.push_str("export type ThreadType = 'chat' | 'claude_code';\n");
    out.push_str("export type ArchiveState = 'archived' | 'inbox';\n");
    out.push_str("export type DisplaySection = 'saved' | 'current' | 'archive';\n");
    // Both spellings come off ALL_STATUSES, so the union, the runtime list and
    // the fixture's status dimension cannot disagree.
    let status_literals: Vec<String> = ALL_STATUSES
        .iter()
        .map(|(s, _)| format!("'{}'", s))
        .collect();
    out.push_str(&format!(
        "export type ThreadStatus = {};\n",
        status_literals.join(" | ")
    ));
    // The runtime list, so the cross-validation suite can DERIVE the expected
    // fixture size from the number of statuses instead of hardcoding a product
    // that goes stale the next time a variant lands (it did, twice).
    out.push_str(&format!(
        "export const THREAD_STATUSES: readonly ThreadStatus[] = [{}] as const;\n",
        status_literals.join(", ")
    ));
    out.push_str("export type EventClass = 'metadata' | 'start' | 'activity' | 'terminal' | 'action_required';\n");
    out.push_str("export type Action = 'discard_draft' | 'discard' | 'apply' | 'archive' | 'save' | 'unsave';\n");
    out.push_str("export type MessageLabel = 'Requesting' | 'Working' | 'Waiting' | 'Canceled' | 'Aborted';\n\n");

    // LEGAL_SECTIONS
    out.push_str(
        "export const LEGAL_SECTIONS: Readonly<Record<ThreadType, readonly ArchiveState[]>> = {\n",
    );
    out.push_str("  chat: ['archived', 'inbox'],\n");
    out.push_str("  claude_code: ['archived', 'inbox'],\n");
    out.push_str("} as const;\n\n");

    // EVENT_CLASSIFICATION
    out.push_str("export const EVENT_CLASSIFICATION: Readonly<Record<string, EventClass>> = {\n");
    for event_type in all_persisted_event_types() {
        if let Some(class) = classify_event(event_type) {
            let class_str = match class {
                EventClass::Metadata => "metadata",
                EventClass::Start => "start",
                EventClass::Activity => "activity",
                EventClass::Terminal => "terminal",
                EventClass::ActionRequired => "action_required",
            };
            out.push_str(&format!("  {}: '{}',\n", event_type, class_str));
        }
    }
    out.push_str("} as const;\n\n");

    // CC_ONLY_EVENTS
    out.push_str("export const CC_ONLY_EVENTS: ReadonlySet<string> = new Set([\n");
    for event in CC_ONLY_EVENTS {
        out.push_str(&format!("  '{}',\n", event));
    }
    out.push_str("]);\n\n");

    // LAST_ACTIVITY_EVENTS
    out.push_str("export const LAST_ACTIVITY_EVENTS: ReadonlySet<string> = new Set([\n");
    for event in LAST_ACTIVITY_EVENTS {
        out.push_str(&format!("  '{}',\n", event));
    }
    out.push_str("]);\n\n");

    // isSectionLegal
    out.push_str("export function isSectionLegal(threadType: ThreadType, section: ArchiveState): boolean {\n");
    out.push_str("  return (LEGAL_SECTIONS[threadType] as readonly string[]).includes(section);\n");
    out.push_str("}\n\n");

    // displaySection
    out.push_str("export function displaySection(\n");
    out.push_str("  stored: ArchiveState,\n");
    out.push_str("  status: ThreadStatus,\n");
    out.push_str("  isSaved: boolean,\n");
    out.push_str("  hasActiveChildren: boolean,\n");
    out.push_str("  hasPendingChanges: boolean,\n");
    out.push_str("  hasAttentionDescendants: boolean,\n");
    out.push_str("): DisplaySection {\n");
    out.push_str("  if (isSaved) return 'saved';\n");
    out.push_str("  const demandsSurface = status === 'running' || hasActiveChildren || hasPendingChanges || hasAttentionDescendants;\n");
    out.push_str("  if (stored === 'archived' && !demandsSurface) return 'archive';\n");
    out.push_str("  return 'current';\n");
    out.push_str("}\n\n");

    // isCcOnlyEvent
    out.push_str("export function isCcOnlyEvent(eventType: string): boolean {\n");
    out.push_str("  return CC_ONLY_EVENTS.has(eventType);\n");
    out.push_str("}\n");

    // availableThreadActions
    out.push_str("\nexport function availableThreadActions(\n");
    out.push_str("  threadType: ThreadType,\n");
    out.push_str("  status: ThreadStatus,\n");
    out.push_str("  storedSection: ArchiveState,\n");
    out.push_str("  hasPendingChanges: boolean,\n");
    out.push_str("  descendantsBlockArchive: boolean,\n");
    out.push_str("  hasUnsentDraft: boolean,\n");
    out.push_str("  isSaved: boolean,\n");
    out.push_str("): Action[] {\n");
    out.push_str("  const actions: Action[] = [];\n");
    out.push_str("  const live = status === 'running' || status === 'waiting_for_user_answer';\n");
    out.push_str(
        "  const codingAgentPending = hasPendingChanges && threadType === 'claude_code';\n",
    );
    out.push_str("  if (hasUnsentDraft) actions.push('discard_draft');\n");
    out.push_str("  if (!live) {\n");
    out.push_str("    if (codingAgentPending) {\n");
    out.push_str("      actions.push('discard', 'apply');\n");
    out.push_str("    } else if (storedSection === 'inbox' && !descendantsBlockArchive) {\n");
    out.push_str("      actions.push('archive');\n");
    out.push_str("    }\n");
    out.push_str("  }\n");
    out.push_str("  actions.push(isSaved ? 'unsave' : 'save');\n");
    out.push_str("  return actions;\n");
    out.push_str("}\n");

    // STATUS_TRANSITIONS / SECTION_TRANSITIONS removed in Phase 5: the
    // frontend now sources thread.meta.status and thread.meta.section
    // exclusively from the per-event ThreadAggregate snapshot, so the
    // generated lookup tables and their supporting types (StatusRule,
    // CcFlagRule, StatusTransition) are no longer consumed.

    out.push_str("export const MESSAGE_COUNT_EVENTS: ReadonlySet<string> = new Set([\n");
    for event in MESSAGE_COUNT_EVENTS {
        out.push_str(&format!("  '{}',\n", event));
    }
    out.push_str("]);\n");

    out
}

// 22c. cross_validation_fixture_is_up_to_date
/// `ALL_STATUSES` drives the fixture's status dimension AND the generated TS
/// union, so a `ThreadStatus` variant missing from it silently shrinks the
/// contract surface rather than failing. Nothing enumerates the enum, so pin it
/// from the other side: every entry must round-trip through `as_str` / `parse`,
/// and the count must match the enum's own arm count.
#[test]
fn all_statuses_covers_the_enum() {
    for (wire, status) in ALL_STATUSES {
        assert_eq!(
            status.as_str(),
            *wire,
            "ALL_STATUSES wire string disagrees with ThreadStatus::as_str",
        );
        assert_eq!(
            ThreadStatus::parse(wire),
            *status,
            "ThreadStatus::parse does not round-trip '{wire}'; a status missing \
             from `parse` falls back to Idle and the projection would silently \
             read the wrong state off the column",
        );
    }
    // Bumped deliberately when a variant is added, which is the prompt to add
    // it above. `parse`'s catch-all means a missing entry cannot be detected by
    // round-tripping alone.
    assert_eq!(
        ALL_STATUSES.len(),
        6,
        "a ThreadStatus variant was added or removed: update ALL_STATUSES (and \
         this count) so the fixture and the generated TS union cover it",
    );
}

#[test]
fn cross_validation_fixture_is_up_to_date() {
    let generated = generate_cross_validation_fixture();
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("lucidos-app/src/generated/cross-validation-fixture.json");
    if let Ok(existing) = std::fs::read_to_string(&path) {
        assert_eq!(
                existing, generated,
                "Cross-validation fixture is stale. Run: cargo test -p lucidos-engine generate_cross_validation_fixture_file -- --ignored"
            );
    } else {
        panic!(
                "Cross-validation fixture does not exist at {}. Run: cargo test -p lucidos-engine generate_cross_validation_fixture_file -- --ignored",
                path.display()
            );
    }
}

// 23. generated_typescript_is_up_to_date
#[test]
fn generated_typescript_is_up_to_date() {
    let generated = generate_typescript();
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("lucidos-app/src/generated/thread-lifecycle.ts");
    if let Ok(existing) = std::fs::read_to_string(&path) {
        assert_eq!(
                existing, generated,
                "Generated TypeScript is stale. Run: cargo test -p lucidos-engine generate_typescript_file -- --ignored"
            );
    } else {
        panic!(
                "Generated file does not exist at {}. Run: cargo test -p lucidos-engine generate_typescript_file -- --ignored",
                path.display()
            );
    }
}

// 24. generate_typescript_file (run explicitly to regenerate)
#[test]
#[ignore]
fn generate_typescript_file() {
    let generated = generate_typescript();
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("lucidos-app/src/generated/thread-lifecycle.ts");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, &generated).unwrap();
    crate::log!("[ContractTest] Generated: {}", path.display());
}

// 24b. generate_cross_validation_fixture_file (run explicitly to regenerate)
#[test]
#[ignore]
fn generate_cross_validation_fixture_file() {
    let generated = generate_cross_validation_fixture();
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("lucidos-app/src/generated/cross-validation-fixture.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, &generated).unwrap();
    crate::log!("[ContractTest] Generated: {}", path.display());
}
