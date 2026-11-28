#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Action {
    Archive,
    Cancel,
    Control,
    SubmitDraft,
    AskSideQuestion,
    DismissSideQuestion,
    Apply,
    RestoreSnapshot,
    CreateWorkspace,
    StopWorkspace,
    StartWorkspace,
    SetWorkspaceAutostart,
    RenameWorkspace,
    DeleteWorkspace,
    RestoreWorkspace,
    GrantDeviceSession,
    Discard,
    SetAside,
    BringBack,
    Revert,
    AnswerQuestion,
    ResolvePermission,
    Continue,
    Harden,
    SetPlanMarker,
    BuildApprovedPlan,
    HandOff,
    ReadWaitingReports,
    RunBatch,
    RunCommand,
    /// INVARIANT: Setting a *standing deny*: the owner's Always deny on a command card,
    /// or a line added in Settings. Its lines go to `agent-denied-commands`,
    /// and [`super::standing_deny`] reads them before any allow.
    DenyCommand,
    UseTool,
    UseMcpTool,
    RunConfinementTests,
    /// INVARIANT: Copying one host-owned log manifest to its exact SSH recipient (ADR
    /// 0712). The host transfer broker prepares this once-only permission;
    /// it never grants the asking agent raw-file or unsandboxed-tool access.
    TransferHostLogs,
    ControlGateway,
    ArmEventHook,
    DisarmEventHook,
    RemoveEventHook,
    EditIntelligenceLadder,
    GrantTriggerPrograms,
    GrantTriggerFolders,
    /// INVARIANT: Starting a thread that an agent on a paired gateway sent, when that
    /// pairing's remote start setting is Ask. The engine raises it, so no
    /// agent proposes it.
    StartRemoteThread,
    CreateTopThread,
    DetachChild,
    /// INVARIANT: Retired: a handoff starts a *successor session* in its own thread, an act
    /// that needs no authority. Kept so approval cards asked before then still
    /// deserialize.
    SpawnSuccessor,
}

impl Action {
    pub fn word(self) -> &'static str {
        match self {
            Self::Archive => "archive",
            Self::Cancel => "cancel",
            Self::Control => "control the session on",
            Self::SubmitDraft => "submit the saved draft on",
            Self::AskSideQuestion => "ask a side question of",
            Self::DismissSideQuestion => "dismiss a side question on",
            Self::Apply => "apply a change from",
            Self::RestoreSnapshot => "restore a snapshot of",
            Self::CreateWorkspace => "create",
            Self::StopWorkspace => "stop",
            Self::StartWorkspace => "start",
            Self::SetWorkspaceAutostart => "change autostart on",
            Self::RenameWorkspace => "rename",
            Self::DeleteWorkspace => "delete",
            Self::RestoreWorkspace => "restore from Trash",
            Self::GrantDeviceSession => "grant a device session for",
            Self::Discard => "discard a change from",
            Self::SetAside => "set aside a change from",
            Self::BringBack => "bring back a change from",
            Self::Revert => "revert a change from",
            Self::AnswerQuestion => "answer a question card on",
            Self::ResolvePermission => "answer a permission card on",
            Self::Continue => "restart the turn on",
            Self::CreateTopThread => "create a top-thread beside",
            Self::DetachChild => "detach",
            Self::SpawnSuccessor => "start a legacy successor thread under",
            Self::Harden => "send the agent back to harden on",
            Self::SetPlanMarker => "send the agent back to its plan on",
            Self::BuildApprovedPlan => "send the agent back to build its approved plan on",
            Self::HandOff => "ask for a handoff on",
            Self::ReadWaitingReports => "wake the agent to read its reports on",
            Self::RunBatch => "run a batch on",
            Self::RunCommand => "run a command on",
            Self::DenyCommand => "set a standing deny from",
            Self::UseTool => "use a tool on",
            Self::UseMcpTool => "call an MCP tool on",
            Self::RunConfinementTests => "run the confinement tests outside the sandbox on",
            Self::TransferHostLogs => "transfer the approved host logs for",
            Self::ControlGateway => "send the approved gateway control request for",
            Self::ArmEventHook => "arm",
            Self::DisarmEventHook => "disarm",
            Self::RemoveEventHook => "remove",
            Self::EditIntelligenceLadder => "edit the intelligence ladder from",
            Self::GrantTriggerPrograms => "grant programs to the trigger",
            Self::GrantTriggerFolders => "grant folders to the trigger",
            Self::StartRemoteThread => "start a thread from a paired gateway on",
        }
    }

    pub fn act(self) -> &'static str {
        match self {
            Self::Archive => "archiving a thread",
            Self::Cancel => "cancelling a turn",
            Self::Control => "controlling a session",
            Self::SubmitDraft => "submitting a saved draft",
            Self::AskSideQuestion => "asking a side question",
            Self::DismissSideQuestion => "dismissing a side question",
            Self::Apply => "applying a change",
            Self::RestoreSnapshot => "restoring a workspace snapshot",
            Self::CreateWorkspace => "creating a workspace",
            Self::StopWorkspace => "stopping a workspace",
            Self::StartWorkspace => "starting a workspace",
            Self::SetWorkspaceAutostart => "changing a workspace's autostart",
            Self::RenameWorkspace => "renaming a workspace",
            Self::DeleteWorkspace => "deleting a workspace",
            Self::RestoreWorkspace => "restoring a workspace from Trash",
            Self::GrantDeviceSession => "granting a device session",
            Self::Discard => "discarding a change",
            Self::SetAside => "setting a change aside",
            Self::BringBack => "bringing a change back",
            Self::Revert => "reverting a change",
            Self::AnswerQuestion => "answering a question card",
            Self::ResolvePermission => "answering a permission card",
            Self::Continue => "restarting a turn",
            Self::CreateTopThread => "creating a top-thread",
            Self::DetachChild => "moving a child thread to top level",
            Self::SpawnSuccessor => "starting a legacy successor thread",
            Self::Harden => "continuing the hardening",
            Self::SetPlanMarker => "returning to the plan",
            Self::BuildApprovedPlan => "building the approved plan",
            Self::HandOff => "handing off the session",
            Self::ReadWaitingReports => "reading the waiting reports",
            Self::RunBatch => "running a batch",
            Self::RunCommand => "running a command",
            Self::DenyCommand => "setting a standing deny",
            Self::UseTool => "using a tool",
            Self::UseMcpTool => "calling an MCP tool",
            Self::RunConfinementTests => "running the confinement tests outside the sandbox",
            Self::TransferHostLogs => "transferring the approved host logs",
            Self::ControlGateway => "sending a gateway control request",
            Self::ArmEventHook => "arming an event hook",
            Self::DisarmEventHook => "disarming an event hook",
            Self::RemoveEventHook => "removing an event hook",
            Self::EditIntelligenceLadder => "editing an intelligence ladder",
            Self::GrantTriggerPrograms => "granting programs to a trigger",
            Self::GrantTriggerFolders => "granting folders to a trigger",
            Self::StartRemoteThread => "starting a thread from a paired gateway",
        }
    }

    pub fn proposable(self) -> bool {
        !matches!(
            self,
            Self::AnswerQuestion
                | Self::ResolvePermission
                | Self::RunBatch
                | Self::RunCommand
                | Self::DenyCommand
                | Self::UseTool
                | Self::UseMcpTool
                | Self::RunConfinementTests
                | Self::TransferHostLogs
                | Self::ControlGateway
                | Self::ArmEventHook
                | Self::DisarmEventHook
                | Self::RemoveEventHook
                | Self::StartRemoteThread
        ) && !self.is_engine_step()
    }

    pub fn is_trigger_grant(self) -> bool {
        matches!(self, Self::GrantTriggerPrograms | Self::GrantTriggerFolders)
    }

    pub fn needs_own_tap(self) -> bool {
        matches!(
            self,
            Self::TransferHostLogs
                | Self::ControlGateway
                | Self::ArmEventHook
                | Self::GrantTriggerPrograms
                | Self::GrantTriggerFolders
                | Self::StartRemoteThread
        )
    }

    pub fn is_engine_step(self) -> bool {
        Self::ENGINE_STEPS.contains(&self)
    }

    pub const ENGINE_STEPS: [Self; 5] = [
        Self::Harden,
        Self::SetPlanMarker,
        Self::BuildApprovedPlan,
        Self::HandOff,
        Self::ReadWaitingReports,
    ];

    pub fn is_workspace_verb(self) -> bool {
        matches!(
            self,
            Self::CreateWorkspace
                | Self::StopWorkspace
                | Self::StartWorkspace
                | Self::SetWorkspaceAutostart
                | Self::RenameWorkspace
                | Self::DeleteWorkspace
                | Self::RestoreWorkspace
                | Self::GrantDeviceSession
        )
    }

    pub fn answers_a_card(self) -> bool {
        matches!(self, Self::AnswerQuestion | Self::ResolvePermission)
    }

    pub fn needs_authority(self) -> bool {
        !matches!(
            self,
            Self::CreateTopThread | Self::DetachChild | Self::SpawnSuccessor
        )
    }
}
