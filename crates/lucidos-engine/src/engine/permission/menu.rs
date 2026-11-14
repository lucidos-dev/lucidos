//! Each action's *permission menu*: the scopes its permission request offers.
//!
//! Every card offers Allow once and Don't allow. The lasting scopes depend on
//! the action. An act that destroys work, sends the owner's words or replaces
//! a workspace stays once-only.

use uuid::Uuid;

use super::{Action, GrantBreadth, Holder, Lifetime, PermissionChoice, RequestOption};

pub(crate) const ALLOW_ONCE_LABEL: &str = "Allow once";
pub(crate) const ALLOW_FOR_THIS_THREAD_LABEL: &str = "Allow for this thread";
pub(crate) const ALWAYS_ALLOW_LABEL: &str = "Always allow";
pub(crate) const DONT_ALLOW_LABEL: &str = "Don't allow";

/// The lasting scopes one action's card offers, and how far each reaches.
/// `None` leaves that scope off the card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Menu {
    pub(crate) this_thread: Option<GrantBreadth>,
    pub(crate) always: Option<GrantBreadth>,
}

/// The menu of `action`, or `None` for an action no agent asks for: a card
/// answer, a tool call, a batch, an engine step, or a retired verb.
pub(crate) fn menu(action: Action) -> Option<Menu> {
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
        Action::Apply | Action::Revert => (Some(Asked), None),
        Action::Discard
        | Action::SubmitDraft
        | Action::DeleteWorkspace
        | Action::RestoreWorkspace
        | Action::GrantDeviceSession
        | Action::RestoreSnapshot
        | Action::ActForUnattestedCaller => (None, None),
        Action::AnswerQuestion
        | Action::ResolvePermission
        | Action::RunBatch
        | Action::RunCommand
        | Action::UseTool
        | Action::UseMcpTool
        | Action::Harden
        | Action::SetPlanMarker
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
pub(crate) fn options(action: Action, asker: Uuid, declined: &str) -> Vec<RequestOption> {
    let Some(menu) = menu(action) else {
        return Vec::new();
    };
    let mut options = vec![RequestOption {
        label: ALLOW_ONCE_LABEL.into(),
        description: "Just this one time.".into(),
        choice: PermissionChoice::grant(Lifetime::Once, once_holder(action, asker)),
    }];
    if let Some(breadth) = menu.this_thread {
        options.push(RequestOption {
            label: ALLOW_FOR_THIS_THREAD_LABEL.into(),
            description: match breadth {
                GrantBreadth::Asked => {
                    "This again, on the same target, for the rest of this thread."
                }
                GrantBreadth::Any => "This and anything like it, for the rest of this thread.",
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

/// Who holds an Allow once. A thread's once grant ends at its next turn
/// start. An unattested caller's act may wait out the owner's own turns, and
/// an allowed start opens a turn itself. So the workspace holds that grant,
/// and the request's one-off breadth keeps it to the one act.
fn once_holder(action: Action, asker: Uuid) -> Holder {
    match action {
        Action::ActForUnattestedCaller => Holder::Workspace,
        _ => Holder::Thread { thread_id: asker },
    }
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
            Action::HandOff,
            Action::ReadWaitingReports,
            Action::RunBatch,
            Action::RunCommand,
            Action::UseTool,
            Action::UseMcpTool,
            Action::ActForUnattestedCaller,
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
                | Action::HandOff
                | Action::ReadWaitingReports
                | Action::RunBatch
                | Action::RunCommand
                | Action::UseTool
                | Action::UseMcpTool
                | Action::ActForUnattestedCaller
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
                Action::Apply | Action::Revert => thread_only.clone(),
                Action::Discard
                | Action::SubmitDraft
                | Action::DeleteWorkspace
                | Action::RestoreWorkspace
                | Action::GrantDeviceSession
                | Action::RestoreSnapshot
                | Action::ActForUnattestedCaller => once_only.clone(),
                _ => Vec::new(),
            };
            assert_eq!(labels(action), expected, "{action:?}");
        }
    }

    /// Only create reaches past its asked target. Every other lasting option
    /// names the same target, and Always allow holds on the workspace. So does
    /// an unattested caller's Allow once.
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
                let expected =
                    if lifetime == Lifetime::Standing || action == Action::ActForUnattestedCaller {
                        Holder::Workspace
                    } else {
                        Holder::Thread { thread_id: asker }
                    };
                assert_eq!(holder, expected, "{action:?} {lifetime:?}");
            }
        }
    }
}
