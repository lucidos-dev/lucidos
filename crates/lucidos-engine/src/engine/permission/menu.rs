//! Each action's *permission menu*: the scopes its permission request offers.
//!
//! The default menu offers Allow once and Don't allow. The lasting scopes depend on
//! the action. An act that destroys work, sends the owner's words or replaces
//! a workspace stays once-only.

use uuid::Uuid;

use super::{Action, GrantBreadth, Holder, Lifetime, PermissionChoice, RequestOption};

pub const ALLOW_ONCE_LABEL: &str = "Allow once";
pub const ALLOW_FOR_THIS_THREAD_LABEL: &str = "Allow for this thread";
pub const ALWAYS_ALLOW_LABEL: &str = "Always allow";
pub const DONT_ALLOW_LABEL: &str = "Don't allow";
pub const ALWAYS_DENY_LABEL: &str = "Always deny";

/// The lasting scopes one action's card offers, and how far each reaches.
/// `None` leaves that scope off the card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Menu {
    pub(crate) this_thread: Option<GrantBreadth>,
    pub(crate) always: Option<GrantBreadth>,
}

/// The menu of `action`, or `None` for an action no agent asks for: a card
/// answer, a tool call, a batch, an engine step, or a retired verb.
pub fn menu(action: Action) -> Option<Menu> {
    use GrantBreadth::{Any, Asked};
    let (this_thread, always) = match action {
        Action::CreateWorkspace => (Some(Any), Some(Any)),
        Action::StopWorkspace
        | Action::StartWorkspace
        | Action::SetWorkspaceAutostart
        | Action::RenameWorkspace
        | Action::Archive
        | Action::Cancel
        | Action::Continue
        | Action::Control
        | Action::SetAside
        | Action::BringBack
        | Action::AskSideQuestion
        | Action::DismissSideQuestion => (Some(Asked), Some(Asked)),
        Action::Apply | Action::Revert | Action::EditIntelligenceLadder => (Some(Asked), None),
        Action::RunConfinementTests
        | Action::GrantTriggerPrograms
        | Action::GrantTriggerFolders
        | Action::StartRemoteThread => (None, Some(Asked)),
        Action::Discard
        | Action::SubmitDraft
        | Action::DeleteWorkspace
        | Action::RestoreWorkspace
        | Action::GrantDeviceSession
        | Action::RestoreSnapshot
        | Action::TransferHostLogs
        | Action::ControlGateway
        | Action::ArmEventHook
        | Action::DisarmEventHook
        | Action::RemoveEventHook => (None, None),
        Action::AnswerQuestion
        | Action::ResolvePermission
        | Action::RunBatch
        | Action::RunCommand
        | Action::DenyCommand
        | Action::UseTool
        | Action::UseMcpTool
        | Action::Harden
        | Action::SetPlanMarker
        | Action::BuildApprovedPlan
        | Action::HandOff
        | Action::ReadWaitingReports
        | Action::CreateTopThread
        | Action::DetachChild
        | Action::SpawnSuccessor => return None,
    };
    Some(Menu {
        this_thread,
        always,
    })
}

/// The options a card for `action` offers `asker`, in the order it shows
/// them. `declined` says what Don't allow leaves in place. Empty for an
/// action with no menu.
pub fn options(action: Action, asker: Uuid, declined: &str) -> Vec<RequestOption> {
    let Some(menu) = menu(action) else {
        return Vec::new();
    };
    let mut options = vec![RequestOption {
        label: ALLOW_ONCE_LABEL.into(),
        description: "Just this one time.".into(),
        choice: PermissionChoice::grant(Lifetime::Once, Holder::Thread { thread_id: asker }),
    }];
    if let Some(breadth) = menu.this_thread {
        options.push(RequestOption {
            label: ALLOW_FOR_THIS_THREAD_LABEL.into(),
            description: match breadth {
                GrantBreadth::Asked => {
                    "This again, on the same target, for the rest of this thread."
                }
                GrantBreadth::Any => "This and anything like it, for the rest of this thread.",
                GrantBreadth::DeviceSession { .. } => {
                    "The described device-session scope, for this thread."
                }
            }
            .into(),
            choice: PermissionChoice::Grant {
                lifetime: Lifetime::ThisThread,
                holder: Holder::Thread { thread_id: asker },
                breadth,
            },
        });
    }
    if let Some(breadth) = menu.always {
        options.push(RequestOption {
            label: ALWAYS_ALLOW_LABEL.into(),
            description: match breadth {
                GrantBreadth::Asked => {
                    "Any thread may do this to the same target without asking, until you \
                     revoke it."
                }
                GrantBreadth::DeviceSession { .. } => {
                    "The described device-session scope, until revoked."
                }
                GrantBreadth::Any => {
                    "Any thread may do this and anything like it without asking, until you \
                     revoke it."
                }
            }
            .into(),
            choice: PermissionChoice::Grant {
                lifetime: Lifetime::Standing,
                holder: Holder::Workspace,
                breadth,
            },
        });
    }
    options.push(RequestOption {
        label: DONT_ALLOW_LABEL.into(),
        description: declined.into(),
        choice: PermissionChoice::Refuse,
    });
    options
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every action, so a new variant must take a row in this test.
    fn every_action() -> Vec<Action> {
        let all = [
            Action::Archive,
            Action::Cancel,
            Action::Control,
            Action::SubmitDraft,
            Action::AskSideQuestion,
            Action::DismissSideQuestion,
            Action::Apply,
            Action::RestoreSnapshot,
            Action::CreateWorkspace,
            Action::StopWorkspace,
            Action::StartWorkspace,
            Action::SetWorkspaceAutostart,
            Action::RenameWorkspace,
            Action::DeleteWorkspace,
            Action::RestoreWorkspace,
            Action::GrantDeviceSession,
            Action::Discard,
            Action::SetAside,
            Action::BringBack,
            Action::Revert,
            Action::AnswerQuestion,
            Action::ResolvePermission,
            Action::Continue,
            Action::Harden,
            Action::SetPlanMarker,
            Action::BuildApprovedPlan,
            Action::HandOff,
            Action::ReadWaitingReports,
            Action::RunBatch,
            Action::RunCommand,
            Action::DenyCommand,
            Action::UseTool,
            Action::UseMcpTool,
            Action::RunConfinementTests,
            Action::TransferHostLogs,
            Action::ControlGateway,
            Action::ArmEventHook,
            Action::DisarmEventHook,
            Action::RemoveEventHook,
            Action::EditIntelligenceLadder,
            Action::GrantTriggerPrograms,
            Action::GrantTriggerFolders,
            Action::StartRemoteThread,
            Action::CreateTopThread,
            Action::DetachChild,
            Action::SpawnSuccessor,
        ];
        for action in all {
            // A match with no wildcard: adding a variant breaks this line.
            match action {
                Action::Archive
                | Action::Cancel
                | Action::Control
                | Action::SubmitDraft
                | Action::AskSideQuestion
                | Action::DismissSideQuestion
                | Action::Apply
                | Action::RestoreSnapshot
                | Action::CreateWorkspace
                | Action::StopWorkspace
                | Action::StartWorkspace
                | Action::SetWorkspaceAutostart
                | Action::RenameWorkspace
                | Action::DeleteWorkspace
                | Action::RestoreWorkspace
                | Action::GrantDeviceSession
                | Action::Discard
                | Action::SetAside
                | Action::BringBack
                | Action::Revert
                | Action::AnswerQuestion
                | Action::ResolvePermission
                | Action::Continue
                | Action::Harden
                | Action::SetPlanMarker
                | Action::BuildApprovedPlan
                | Action::HandOff
                | Action::ReadWaitingReports
                | Action::RunBatch
                | Action::RunCommand
                | Action::DenyCommand
                | Action::UseTool
                | Action::UseMcpTool
                | Action::RunConfinementTests
                | Action::TransferHostLogs
                | Action::ControlGateway
                | Action::ArmEventHook
                | Action::DisarmEventHook
                | Action::RemoveEventHook
                | Action::EditIntelligenceLadder
                | Action::GrantTriggerPrograms
                | Action::GrantTriggerFolders
                | Action::StartRemoteThread
                | Action::CreateTopThread
                | Action::DetachChild
                | Action::SpawnSuccessor => {}
            }
        }
        all.to_vec()
    }

    fn labels(action: Action) -> Vec<String> {
        options(action, Uuid::new_v4(), "Nothing happens.")
            .into_iter()
            .map(|o| o.label)
            .collect()
    }

    /// Each card offers exactly its action's row of the menu table.
    #[test]
    fn every_card_offers_exactly_its_menu() {
        let full = vec![
            ALLOW_ONCE_LABEL,
            ALLOW_FOR_THIS_THREAD_LABEL,
            ALWAYS_ALLOW_LABEL,
            DONT_ALLOW_LABEL,
        ];
        let thread_only = vec![
            ALLOW_ONCE_LABEL,
            ALLOW_FOR_THIS_THREAD_LABEL,
            DONT_ALLOW_LABEL,
        ];
        let once_only = vec![ALLOW_ONCE_LABEL, DONT_ALLOW_LABEL];
        for action in every_action() {
            let expected: Vec<&str> = match action {
                Action::CreateWorkspace
                | Action::StopWorkspace
                | Action::StartWorkspace
                | Action::SetWorkspaceAutostart
                | Action::RenameWorkspace
                | Action::Archive
                | Action::Cancel
                | Action::Continue
                | Action::Control
                | Action::SetAside
                | Action::BringBack
                | Action::AskSideQuestion
                | Action::DismissSideQuestion => full.clone(),
                Action::Apply | Action::Revert | Action::EditIntelligenceLadder => {
                    thread_only.clone()
                }
                Action::RunConfinementTests
                | Action::GrantTriggerPrograms
                | Action::GrantTriggerFolders
                | Action::StartRemoteThread => {
                    vec![ALLOW_ONCE_LABEL, ALWAYS_ALLOW_LABEL, DONT_ALLOW_LABEL]
                }
                Action::Discard
                | Action::SubmitDraft
                | Action::DeleteWorkspace
                | Action::RestoreWorkspace
                | Action::GrantDeviceSession
                | Action::RestoreSnapshot
                | Action::TransferHostLogs
                | Action::ControlGateway
                | Action::ArmEventHook
                | Action::DisarmEventHook
                | Action::RemoveEventHook => once_only.clone(),
                _ => Vec::new(),
            };
            assert_eq!(labels(action), expected, "{action:?}");
        }
    }

    /// Only create reaches past its asked target. Every other lasting option
    /// names the same target, and Always allow holds on the workspace.
    #[test]
    fn lasting_options_reach_and_hold_as_the_table_says() {
        let asker = Uuid::new_v4();
        for action in every_action() {
            for option in options(action, asker, "") {
                let PermissionChoice::Grant {
                    lifetime,
                    holder,
                    breadth,
                } = option.choice
                else {
                    continue;
                };
                let any = action == Action::CreateWorkspace && lifetime != Lifetime::Once;
                assert_eq!(breadth == GrantBreadth::Any, any, "{action:?} {lifetime:?}");
                let expected = if lifetime == Lifetime::Standing {
                    Holder::Workspace
                } else {
                    Holder::Thread { thread_id: asker }
                };
                assert_eq!(holder, expected, "{action:?} {lifetime:?}");
            }
        }
    }

    #[test]
    fn host_log_transfer_requires_once_only_thread_bound_exact_consent() {
        let action: Action = serde_json::from_str("\"transfer-host-logs\"").unwrap();
        let asker = Uuid::new_v4();
        let offered = options(action, asker, "The host logs were not sent.");
        assert_eq!(labels(action), vec![ALLOW_ONCE_LABEL, DONT_ALLOW_LABEL]);
        assert_eq!(
            offered[0].choice,
            PermissionChoice::Grant {
                lifetime: Lifetime::Once,
                holder: Holder::Thread { thread_id: asker },
                breadth: GrantBreadth::Asked,
            }
        );
        // A generic permission request cannot stand in for the transfer
        // broker's validated source manifest and destination.
        assert!(!action.proposable());
        assert!(!action.is_workspace_verb());
        assert!(!action.is_engine_step());
    }

    #[test]
    fn gateway_control_is_once_only_and_cannot_be_proposed_generically() {
        let action = Action::ControlGateway;
        assert_eq!(labels(action), vec![ALLOW_ONCE_LABEL, DONT_ALLOW_LABEL]);
        assert!(!action.proposable());
        assert!(action.needs_own_tap());
    }
}
