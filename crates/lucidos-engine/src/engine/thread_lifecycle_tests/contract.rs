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

/// Every `ThreadStatus` variant with its wire string, DERIVED from
/// [`ThreadStatus::ALL`] rather than restated.
///
/// Source for BOTH halves of the contract: the fixture's status dimension and
/// the generated TS union + `THREAD_STATUSES` list. Keeping them as two
/// separate literals is how the hardcoded fixture-size assertions went stale,
/// and this used to be a third literal spelling out the same six statuses.
/// `ThreadStatus::ALL` is now the production enumeration (the status filter's
/// error messages, the CLI help and the LLM tool schema all advertise it), so
/// deriving from it means `all_statuses_covers_the_enum`'s count assertion
/// pins that constant too instead of only this file's copy.
fn all_statuses() -> [(&'static str, ThreadStatus); 6] {
    ThreadStatus::ALL.map(|status| (status.as_str(), status))
}

fn generate_cross_validation_fixture() -> String {
    let thread_types = [
        ("chat", ThreadType::Chat),
        ("claude_code", ThreadType::CodingAgent),
    ];
    let statuses = all_statuses();
    let sections = [
        ("archived", ArchiveState::Archived),
        ("inbox", ArchiveState::Inbox),
    ];
    let bools = [false, true];

    let mut cases = Vec::new();

    // availableThreadActions: the full cross product of thread_types ×
    // statuses × sections × pending × descendants_block_archive × live_event_waits
    // × active_children × has_unsent_draft × is_saved. Adding a `ThreadStatus`
    // variant widens the fixture automatically; do not hardcode the case count
    // here, it drifts.
    for (tt_str, tt) in &thread_types {
        for (st_str, st) in &statuses {
            for (sec_str, sec) in &sections {
                for &pending in &bools {
                    for &dba in &bools {
                        for &waits in &bools {
                            for &children in &bools {
                                for &draft in &bools {
                                    for &saved in &bools {
                                        let actions: Vec<&str> = available_thread_actions(
                                            *tt, *st, *sec, pending, dba, waits, children, draft,
                                            saved,
                                        )
                                        .iter()
                                        .map(|a| match a {
                                            Action::DiscardDraft => "discard_draft",
                                            Action::Discard => "discard",
                                            Action::Apply => "apply",
                                            Action::ApplyWhenSettled => "apply_when_settled",
                                            Action::Archive => "archive",
                                            Action::Save => "save",
                                            Action::Unsave => "unsave",
                                        })
                                        .collect();
                                        cases.push(format!(
                                        r#"    {{ "fn": "availableThreadActions", "args": [{:?}, {:?}, {:?}, {}, {}, {}, {}, {}, {}], "expected": [{}] }}"#,
                                        tt_str, st_str, sec_str, pending, dba, waits, children, draft, saved,
                                        actions.iter().map(|a| format!("{:?}", a)).collect::<Vec<_>>().join(", ")
                                    ));
                                    }
                                }
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
        for (st_str, st) in &statuses {
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
    // Both spellings come off `all_statuses`, so the union, the runtime list and
    // the fixture's status dimension cannot disagree.
    let status_literals: Vec<String> = all_statuses()
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
    out.push_str("export type Action = 'discard_draft' | 'discard' | 'apply' | 'apply_when_settled' | 'archive' | 'save' | 'unsave';\n");
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
    out.push_str("  hasLiveEventWaits: boolean,\n");
    out.push_str("  hasActiveChildren: boolean,\n");
    out.push_str("  hasUnsentDraft: boolean,\n");
    out.push_str("  isSaved: boolean,\n");
    out.push_str("): Action[] {\n");
    out.push_str("  const actions: Action[] = [];\n");
    out.push_str("  const live = status === 'running' || status === 'waiting_for_user_answer';\n");
    out.push_str("  const willResume = hasLiveEventWaits || hasActiveChildren;\n");
    out.push_str(
        "  const codingAgentPending = hasPendingChanges && threadType === 'claude_code';\n",
    );
    out.push_str("  if (hasUnsentDraft) actions.push('discard_draft');\n");
    out.push_str("  if (!live) {\n");
    out.push_str("    if (codingAgentPending) {\n");
    out.push_str("      if (!willResume) actions.push('discard', 'apply');\n");
    out.push_str("    } else if (storedSection === 'inbox' && !descendantsBlockArchive) {\n");
    out.push_str("      actions.push('archive');\n");
    out.push_str("    }\n");
    out.push_str("  }\n");
    out.push_str(
        "  if (threadType === 'claude_code' && (status === 'running' || status === 'paused')) {\n",
    );
    out.push_str("    actions.push('apply_when_settled');\n");
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
/// [`ThreadStatus::ALL`] drives the fixture's status dimension, the generated
/// TS union, the status filter's accepted values, the CLI help and the LLM tool
/// schema, so an entry that is wrong shrinks or corrupts all five at once. Every
/// entry must round-trip through `as_str` / `parse` / `try_parse`, and no two
/// may share a wire string.
///
/// What this test does NOT catch, stated plainly because a guard nobody has is
/// less dangerous than one they think they have: a `ThreadStatus` variant added
/// to the enum and never added to `ALL`. Nothing here can see it, because every
/// enumeration available to a test is `ALL` itself. The two forcing functions
/// are elsewhere and both are the compiler, not this file. Widening `ALL`
/// without widening `all_statuses`'s return type is a type error, and adding a
/// variant at all is a non-exhaustive-match error in `ThreadStatus::as_str`,
/// which is where the instruction to update `ALL` lives. The count assertion
/// this test used to carry looked like a third guard and was not one: `.len()`
/// on a `[Self; 6]` is a compile-time constant, so it could never fail.
#[test]
fn all_statuses_covers_the_enum() {
    let mut seen: Vec<&str> = Vec::new();
    for (wire, status) in all_statuses() {
        assert_eq!(
            status.as_str(),
            wire,
            "ThreadStatus::ALL wire string disagrees with ThreadStatus::as_str",
        );
        assert_eq!(
            ThreadStatus::parse(wire),
            status,
            "ThreadStatus::parse does not round-trip '{wire}'; a status missing \
             from `parse` falls back to Idle and the projection would silently \
             read the wrong state off the column",
        );
        assert_eq!(
            ThreadStatus::try_parse(wire),
            Some(status),
            "ThreadStatus::try_parse does not round-trip '{wire}'; a status it \
             rejects cannot be named by the `status` filter on threads \
             list / count",
        );
        assert!(
            !seen.contains(&wire),
            "'{wire}' appears twice in ThreadStatus::ALL; a duplicate widens the \
             fixture's status dimension with a redundant case and makes the \
             generated TS union and the tool schema enum list it twice",
        );
        seen.push(wire);
    }
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
