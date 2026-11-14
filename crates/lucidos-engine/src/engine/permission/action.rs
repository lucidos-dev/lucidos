//! An *action*: one thing Lucidos can do that a *permission* can cover.
//!
//! Thread buttons, tool calls, CLI commands, engine steps and voice name the
//! same entries (ADR 0664). Each variant's kebab-case name is stored in
//! events, so a rename needs a serde alias.

/// One action. As a clause-4 verb it names what a caller attempted. A refusal
/// reads "you cannot cancel that thread", so the agent knows what it just tried.
///
/// One variant per VERB, never per route. `Apply` covers the single change, the
/// batch and a coding-agent thread's own apply alike. The authority question is
/// the same one, and the caller reads the same sentence.
///
/// It is also the wire name a *permission request* names (ADR 0664).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Action {
    /// Taking a thread off the thread list: archiving an active thread, or
    /// discarding a draft thread (`DELETE /api/v1/threads/:id`).
    Archive,
    Cancel,
    /// Driving a thread's session. That covers interrupting a coding agent and
    /// changing the model or reasoning effort (`POST /api/v1/claude-code/control`,
    /// `/api/v1/chat/settings`). It also covers taking back a queued message
    /// (`POST /api/v1/chat/queued-message/remove`). Writing the draft or adding an
    /// image to it counts too (`PUT /api/v1/threads/:id/compose`,
    /// `POST /api/v1/threads/:id/blobs`).
    /// Controlling another thread's session is as much a reach as cancelling it.
    Control,
    /// Submit saved owner content in its original thread. Subtree ownership
    /// alone never authorizes sending the owner's draft (ADR 0651).
    SubmitDraft,
    /// Asking a side question, which reads the thread's whole context
    /// (`POST /api/v1/side-questions`).
    AskSideQuestion,
    /// Dismissing a side question's card (`POST /api/v1/side-questions/dismiss`).
    DismissSideQuestion,
    /// Applying a change, or arming or taking back an apply. With no target it
    /// aims at the workspace: Apply All (`POST /api/v1/changes/apply-all`), its
    /// cancel (`POST /api/v1/changes/apply-all/cancel`), and the workspace-wide
    /// disarm (`DELETE /api/v1/standing-applies`).
    Apply,
    /// Restoring one snapshot of a named workspace. Never a thread target.
    RestoreSnapshot,
    /// The *workspace broker* verbs (ADR 0671). Each targets one workspace by
    /// slug, never a thread. A restore targets a Trash entry by its id.
    CreateWorkspace,
    StopWorkspace,
    StartWorkspace,
    SetWorkspaceAutostart,
    RenameWorkspace,
    DeleteWorkspace,
    RestoreWorkspace,
    Discard,
    SetAside,
    BringBack,
    /// Reverting a change, or undoing a command checkpoint on the thread whose
    /// command made it (`POST /api/v1/command-checkpoint/undo`).
    Revert,
    AnswerQuestion,
    /// Answering a permission card: Allow once, Allow for this thread, or
    /// Deny. The two Always-allow grants are refused to every agent outright,
    /// by [`require_owner_for_always_allow`].
    ResolvePermission,
    Continue,
    /// The engine sends a paused coding agent back to run `/harden`.
    Harden,
    /// The engine sends a paused coding agent back to settle its plan marker.
    SetPlanMarker,
    /// The engine asks a paused session past its context ceiling to hand off.
    HandOff,
    /// The engine wakes a paused coding agent to read the child reports and
    /// event-wait deliveries that arrived.
    ReadWaitingReports,
    /// Running a confirmed *batch*: each of its exact items, once.
    RunBatch,
    /// Running a bash or python command the Lucidos Agent's command guard
    /// paused. Its card's grants go to `agent-allowed-commands`.
    RunCommand,
    /// A coding agent's tool call that asked first. Its card's grants go to
    /// `cc-allowed-tools`.
    UseTool,
    /// The Lucidos Agent's call to an MCP server tool. Its card's grants go to
    /// `mcp-allowed-tools`.
    UseMcpTool,
    /// Running the *confinement tests* outside the session sandbox, through
    /// `POST /api/v1/threads/:thread_id/confinement-tests`. The only spawn the
    /// engine makes bare on an agent's behalf.
    RunConfinementTests,
    /// An owner-only act an *unattested caller* asked for: one request to an
    /// owner-only route, or one coding-agent run. Its card names that exact
    /// request, so it is once-only.
    ActForUnattestedCaller,
    /// Retired: creating a top-thread needs no authority (ADR 0475). Kept so
    /// approval cards asked before then still deserialize.
    CreateTopThread,
    /// Retired with [`Self::CreateTopThread`]: an agent moves its own child to
    /// top level with no authority beyond the detach ladder (ADR 0475).
    DetachChild,
    /// Retired: a handoff starts a *successor session* in its own thread, an act
    /// that needs no authority. Kept so approval cards asked before then still
    /// deserialize.
    SpawnSuccessor,
}

impl Action {
    /// The verb as an infinitive taking a thread, for "you cannot {} it".
    ///
    /// Every refusal that names a target uses this one form in both of its
    /// slots. An earlier pass wrote "a thread {}s itself" and got "applys",
    /// which is why the frames below never inflect it.
    pub(crate) fn word(self) -> &'static str {
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
            Self::HandOff => "ask for a handoff on",
            Self::ReadWaitingReports => "wake the agent to read its reports on",
            Self::RunBatch => "run a batch on",
            Self::RunCommand => "run a command on",
            Self::UseTool => "use a tool on",
            Self::UseMcpTool => "call an MCP tool on",
            Self::RunConfinementTests => "run the confinement tests outside the sandbox on",
            Self::ActForUnattestedCaller => "act for an unattested caller on",
        }
    }

    /// The act as a noun phrase, for a refusal with no thread to point at.
    ///
    /// [`Self::word`] needs a target to follow it, and two refusals have none:
    /// a verb aimed at the workspace root, and a caller with no subtree at all.
    pub(crate) fn act(self) -> &'static str {
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
            Self::HandOff => "handing off the session",
            Self::ReadWaitingReports => "reading the waiting reports",
            Self::RunBatch => "running a batch",
            Self::RunCommand => "running a command",
            Self::UseTool => "using a tool",
            Self::UseMcpTool => "calling an MCP tool",
            Self::RunConfinementTests => "running the confinement tests outside the sandbox",
            Self::ActForUnattestedCaller => "acting for an unattested caller",
        }
    }

    /// May an agent ask for a permission request for this verb (ADR 0664)?
    ///
    /// Answering a question card and resolving a permission card are already
    /// the owner's own answer, on screen. A card that authorized either would
    /// put their voice on a card they never read. An engine step is asked by
    /// the engine itself, never proposed by an agent, and so is a batch. A
    /// tool call and a confinement run have their own cards.
    pub(crate) fn proposable(self) -> bool {
        !matches!(
            self,
            Self::AnswerQuestion
                | Self::ResolvePermission
                | Self::RunBatch
                | Self::RunCommand
                | Self::UseTool
                | Self::UseMcpTool
                | Self::RunConfinementTests
                | Self::ActForUnattestedCaller
        ) && !self.is_engine_step()
    }

    /// Is this one of the engine's automatic steps, which an automation
    /// pause turns into a permission request?
    pub(crate) fn is_engine_step(self) -> bool {
        matches!(
            self,
            Self::Harden | Self::SetPlanMarker | Self::HandOff | Self::ReadWaitingReports
        )
    }

    /// Is this a workspace broker verb, whose approval names a workspace?
    pub(crate) fn is_workspace_verb(self) -> bool {
        matches!(
            self,
            Self::CreateWorkspace
                | Self::StopWorkspace
                | Self::StartWorkspace
                | Self::SetWorkspaceAutostart
                | Self::RenameWorkspace
                | Self::DeleteWorkspace
                | Self::RestoreWorkspace
        )
    }

    /// Does this act answer a card? No thread does that on its own subtree or
    /// on the owner's standing instruction, only through a batch the owner
    /// confirms (ADR 0664).
    pub(crate) fn answers_a_card(self) -> bool {
        matches!(self, Self::AnswerQuestion | Self::ResolvePermission)
    }

    /// Is this still a clause-4 verb? The retired ones are not, so the gate
    /// allows them and no card proposes them.
    pub(crate) fn needs_authority(self) -> bool {
        !matches!(
            self,
            Self::CreateTopThread | Self::DetachChild | Self::SpawnSuccessor
        )
    }
}
