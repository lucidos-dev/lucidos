//! The frontend's copy of the thread lifecycle rules, and the fixture that
//! cross-validates the two languages.

use super::*;
use crate::codegen::REGENERATE;

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
/// the generated TS union + `THREAD_STATUSES` list, so the two cannot disagree.
/// `ThreadStatus::ALL` is the production enumeration: the status filter's
/// errors, the CLI help and the LLM tool schema all advertise it.
pub(crate) fn all_statuses() -> [(&'static str, ThreadStatus); 6] {
    ThreadStatus::ALL.map(|status| (status.as_str(), status))
}

pub(crate) fn render_fixture() -> String {
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

    // availableThreadActions: the full cross product of thread_types, statuses,
    // sections, pending, descendants_block_archive, live_event_waits,
    // has_unsent_draft and is_saved. It leaves out the pinned-and-archived pair
    // the engine cannot hold (`is_retention_legal`, ADR 0312). A new
    // `ThreadStatus` variant widens the fixture, so never hardcode the count.
    for (tt_str, tt) in &thread_types {
        for (st_str, st) in &statuses {
            for (sec_str, sec) in &sections {
                for &pending in &bools {
                    for &dba in &bools {
                        for &waits in &bools {
                            for &draft in &bools {
                                for &saved in &bools {
                                    if !is_retention_legal(*sec, saved) {
                                        continue;
                                    }
                                    let actions: Vec<&str> = available_thread_actions(
                                        *tt, *st, *sec, pending, dba, waits, draft, saved,
                                    )
                                    .iter()
                                    .map(|a| match a {
                                        Action::DiscardDraft => "discard_draft",
                                        Action::Discard => "discard",
                                        Action::Apply => "apply",
                                        Action::SetAside => "set_aside",
                                        Action::ApplyWhenSettled => "apply_when_settled",
                                        Action::Archive => "archive",
                                        Action::Save => "save",
                                        Action::Unsave => "unsave",
                                    })
                                    .collect();
                                    cases.push(format!(
                                        r#"    {{ "fn": "availableThreadActions", "args": [{:?}, {:?}, {:?}, {}, {}, {}, {}, {}], "expected": [{}] }}"#,
                                        tt_str, st_str, sec_str, pending, dba, waits, draft, saved,
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

    // displaySection: the full cross product of sections × statuses × saved ×
    // activeChildren × pending × attentionDescendants, minus the same illegal
    // pinned-and-archived pair.
    for (sec_str, sec) in &sections {
        for (st_str, st) in &statuses {
            for &saved in &bools {
                if !is_retention_legal(*sec, saved) {
                    continue;
                }
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

    // changeWork: change state kind × holds_live_wait.
    for kind in ChangeStateKind::ALL {
        for &waits in &bools {
            cases.push(format!(
                r#"    {{ "fn": "changeWork", "args": [{:?}, {}], "expected": {:?} }}"#,
                kind.as_str(),
                waits,
                ChangeWork::of(kind, waits).as_str()
            ));
        }
    }

    // ownBlocker: the full cross product of thread_types × statuses × sections ×
    // change work × external_repo.
    for (tt_str, tt) in &thread_types {
        for (st_str, st) in &statuses {
            for (sec_str, sec) in &sections {
                for change in ChangeWork::ALL {
                    for &external in &bools {
                        let result =
                            json_own_blocker(own_blocker(*tt, *st, *sec, change, external));
                        cases.push(format!(
                            r#"    {{ "fn": "ownBlocker", "args": [{:?}, {:?}, {:?}, {:?}, {}], "expected": {} }}"#,
                            tt_str,
                            st_str,
                            sec_str,
                            change.as_str(),
                            external,
                            result
                        ));
                    }
                }
            }
        }
    }

    // actionBlocker: every own blocker (or none) × is_home × every descendant
    // blocker (or none).
    let own_or_none: Vec<Option<OwnBlocker>> = std::iter::once(None)
        .chain(OwnBlocker::ALL.map(Some))
        .collect();
    for &own in &own_or_none {
        for &home in &bools {
            for &descendant in &own_or_none {
                cases.push(format!(
                    r#"    {{ "fn": "actionBlocker", "args": [{}, {}, {}], "expected": {:?} }}"#,
                    json_own_blocker(own),
                    home,
                    json_own_blocker(descendant),
                    action_blocker(own, home, descendant).as_str()
                ));
            }
        }
    }

    format!("{{\n  \"cases\": [\n{}\n  ]\n}}\n", cases.join(",\n"))
}

/// An own blocker as the fixture's JSON: its slug, or `null`.
fn json_own_blocker(own: Option<OwnBlocker>) -> String {
    own.map_or_else(|| "null".to_string(), |o| format!("{:?}", o.as_str()))
}

/// A TS string-literal union over `slugs`.
fn ts_union(slugs: impl IntoIterator<Item = &'static str>) -> String {
    slugs
        .into_iter()
        .map(|s| format!("'{s}'"))
        .collect::<Vec<_>>()
        .join(" | ")
}

pub(crate) fn render_ts() -> String {
    let mut out = String::new();

    // Header
    out.push_str("// AUTO-GENERATED by thread_lifecycle.rs. Do not edit by hand.\n");
    out.push_str(&format!("// Regenerate: {REGENERATE}\n\n"));

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
    // The runtime list, from which the cross-validation suite DERIVES the
    // expected fixture size rather than hardcoding a product.
    out.push_str(&format!(
        "export const THREAD_STATUSES: readonly ThreadStatus[] = [{}] as const;\n",
        status_literals.join(", ")
    ));
    out.push_str("export type EventClass = 'metadata' | 'start' | 'activity' | 'terminal' | 'action_required';\n");
    out.push_str("export type Action = 'discard_draft' | 'discard' | 'apply' | 'set_aside' | 'apply_when_settled' | 'archive' | 'save' | 'unsave';\n\n");

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
    out.push_str("      if (!hasLiveEventWaits) actions.push('discard', 'apply', 'set_aside');\n");
    out.push_str("    } else if (storedSection === 'inbox' && !descendantsBlockArchive) {\n");
    out.push_str("      actions.push('archive');\n");
    out.push_str("    }\n");
    out.push_str("  }\n");
    out.push_str(
        "  const watching = hasLiveEventWaits && status !== 'waiting_for_user_answer' \
         && status !== 'failed';\n",
    );
    out.push_str(
        "  if (threadType === 'claude_code' \
         && (status === 'running' || status === 'paused' || watching)) {\n",
    );
    out.push_str("    actions.push('apply_when_settled');\n");
    out.push_str("  }\n");
    out.push_str("  actions.push(isSaved ? 'unsave' : 'save');\n");
    out.push_str("  return actions;\n");
    out.push_str("}\n");

    // ownBlocker and actionBlocker: why Archive or Delete cannot run. See
    // `own_blocker` and `action_blocker` in thread_lifecycle.rs.
    out.push_str(&format!(
        "\nexport type OwnBlocker = {};\n",
        ts_union(OwnBlocker::ALL.map(OwnBlocker::as_str))
    ));
    out.push_str(&format!(
        "export const OWN_BLOCKER_PRIORITY: readonly OwnBlocker[] = [{}] as const;\n",
        OwnBlocker::ALL
            .map(|o| format!("'{}'", o.as_str()))
            .join(", ")
    ));
    out.push_str(&format!(
        "export type Blocker = {};\n",
        ts_union(Blocker::ALL.map(Blocker::as_str))
    ));
    out.push_str(&format!(
        "\nexport type ChangeWork = {};\n",
        ts_union(ChangeWork::ALL.map(ChangeWork::as_str))
    ));
    out.push_str(&format!(
        "\nexport type ChangeStateKind = {};\n",
        ts_union(ChangeStateKind::ALL.map(ChangeStateKind::as_str))
    ));
    out.push_str("\nexport function changeWork(\n");
    out.push_str("  change: ChangeStateKind,\n");
    out.push_str("  holdsLiveWait: boolean,\n");
    out.push_str("): ChangeWork {\n");
    out.push_str("  if (change === 'proposed') return 'proposed';\n");
    out.push_str("  return change === 'unproposed' && holdsLiveWait ? 'held' : 'none';\n");
    out.push_str("}\n");
    out.push_str("\nexport function ownBlocker(\n");
    out.push_str("  threadType: ThreadType,\n");
    out.push_str("  status: ThreadStatus,\n");
    out.push_str("  archiveState: ArchiveState,\n");
    out.push_str("  change: ChangeWork,\n");
    out.push_str("  isExternalRepo: boolean,\n");
    out.push_str("): OwnBlocker | null {\n");
    out.push_str("  if (status === 'running') return 'running';\n");
    out.push_str("  if (status === 'waiting_for_user_answer') return 'question';\n");
    out.push_str(
        "  if (archiveState === 'archived' || threadType !== 'claude_code' || isExternalRepo) \
         return null;\n",
    );
    out.push_str("  if (change === 'proposed') return 'pending_change';\n");
    out.push_str("  return change === 'held' ? 'held_proposal' : null;\n");
    out.push_str("}\n");
    out.push_str("\nexport function actionBlocker(\n");
    out.push_str("  own: OwnBlocker | null,\n");
    out.push_str("  isHome: boolean,\n");
    out.push_str("  descendant: OwnBlocker | null,\n");
    out.push_str("): Blocker {\n");
    out.push_str("  if (isHome) return 'home';\n");
    out.push_str("  if (own !== null) return own;\n");
    out.push_str("  if (descendant !== null) return `descendant_${descendant}`;\n");
    out.push_str("  return 'none';\n");
    out.push_str("}\n");

    out.push_str("export const MESSAGE_COUNT_EVENTS: ReadonlySet<string> = new Set([\n");
    for event in MESSAGE_COUNT_EVENTS {
        out.push_str(&format!("  '{}',\n", event));
    }
    out.push_str("]);\n");

    out
}
