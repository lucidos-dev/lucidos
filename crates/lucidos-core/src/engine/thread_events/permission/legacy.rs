use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{Action, Holder, Lifetime, PermissionChoice};
use crate::engine::thread_events::AllowScope;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LegacyLane {
    Command,
    CodingAgent,
    Mcp,
}

impl LegacyLane {
    pub const ALL: [LegacyLane; 3] = [Self::Command, Self::CodingAgent, Self::Mcp];

    pub fn action(self) -> Action {
        match self {
            Self::Command => Action::RunCommand,
            Self::CodingAgent => Action::UseTool,
            Self::Mcp => Action::UseMcpTool,
        }
    }

    pub fn requested_event(self) -> &'static str {
        match self {
            Self::Command => "CommandPermissionRequested",
            Self::CodingAgent => "CodingAgentPermissionRequest",
            Self::Mcp => "McpPermissionRequested",
        }
    }

    pub fn resolved_event(self) -> &'static str {
        match self {
            Self::Command => "CommandPermissionResolved",
            Self::CodingAgent => "CodingAgentPermissionResolved",
            Self::Mcp => "McpPermissionResolved",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LegacyPick {
    Once,
    Session,
    Narrow,
    Broad,
    Deny,
}

impl LegacyPick {
    pub const ALL: [LegacyPick; 5] = [
        Self::Once,
        Self::Session,
        Self::Narrow,
        Self::Broad,
        Self::Deny,
    ];

    pub fn allowed(self) -> bool {
        self != Self::Deny
    }

    pub fn scope(self) -> Option<AllowScope> {
        match self {
            Self::Session => Some(AllowScope::Session),
            Self::Narrow => Some(AllowScope::Narrow),
            Self::Broad => Some(AllowScope::Broad),
            Self::Once | Self::Deny => None,
        }
    }

    pub fn choice(self, thread_id: Uuid) -> PermissionChoice {
        let grant = PermissionChoice::grant;
        match self {
            Self::Once => grant(Lifetime::Once, Holder::Thread { thread_id }),
            Self::Session => grant(Lifetime::ThisThread, Holder::Thread { thread_id }),
            Self::Narrow | Self::Broad => grant(Lifetime::Standing, Holder::Workspace),
            Self::Deny => PermissionChoice::Refuse,
        }
    }

    pub fn option_id(self) -> String {
        let name = match self {
            Self::Once => "once",
            Self::Session => "session",
            Self::Narrow => "narrow",
            Self::Broad => "broad",
            Self::Deny => "deny",
        };
        format!("legacy-{name}")
    }
}
