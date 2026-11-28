mod action;
pub mod batch;
pub mod device_session;
pub mod legacy;
pub mod projection;
pub mod request;
pub mod side_effect;

pub use action::Action;

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use uuid::Uuid;

use crate::core::grants::GrantFile;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Lifetime {
    Once,
    ThisThread,
    UntilPlanLands,
    Standing,
}

impl Lifetime {
    pub fn wire(self) -> &'static str {
        match self {
            Self::Once => "once",
            Self::ThisThread => "this_thread",
            Self::UntilPlanLands => "until_plan_lands",
            Self::Standing => "standing",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Holder {
    Thread { thread_id: Uuid },
    Trigger { trigger_id: String },
    Workspace,
}

impl Holder {
    fn columns(&self) -> (&'static str, Option<String>) {
        match self {
            Self::Thread { thread_id } => ("thread", Some(thread_id.to_string())),
            Self::Trigger { trigger_id } => ("trigger", Some(trigger_id.clone())),
            Self::Workspace => ("workspace", None),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum GrantBreadth {
    #[default]
    Asked,
    Any,
    #[serde(rename = "device-session")]
    DeviceSession {
        creator_workspace: String,
        device_id: String,
        ttl_secs: u64,
    },
}

impl GrantBreadth {
    fn is_asked(&self) -> bool {
        *self == Self::Asked
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PermissionChoice {
    Grant {
        lifetime: Lifetime,
        holder: Holder,
        #[serde(default, skip_serializing_if = "GrantBreadth::is_asked")]
        breadth: GrantBreadth,
    },
    #[serde(rename = "unpause")]
    RetiredUnpause,
    Refuse,
    RefuseStanding,
    Revoke,
}

impl PermissionChoice {
    pub fn grant(lifetime: Lifetime, holder: Holder) -> Self {
        Self::Grant {
            lifetime,
            holder,
            breadth: GrantBreadth::Asked,
        }
    }

    pub fn is_once(&self) -> bool {
        matches!(
            self,
            Self::Grant {
                lifetime: Lifetime::Once,
                ..
            }
        )
    }

    pub fn batch_may_pick(&self) -> bool {
        match self {
            Self::Grant {
                lifetime, holder, ..
            } => {
                matches!(lifetime, Lifetime::Once | Lifetime::ThisThread)
                    && matches!(holder, Holder::Thread { .. })
            }
            Self::Refuse => true,
            Self::RetiredUnpause | Self::RefuseStanding | Self::Revoke => false,
        }
    }
}

/// INVARIANT: SQL over a `UserQuestionAsked` row aliased `alias`: did the engine word this
/// card? Only the owner's device answers one. The legacy key keeps a card
/// asked before permission requests closed to everyone else.
pub fn engine_card_sql(alias: &str) -> String {
    format!("({alias}.payload ? 'permission_request' OR {alias}.payload ? 'owner_approval')")
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PickedBy {
    Tap,
    Typed {
        message_event_id: Uuid,
    },
    /// INVARIANT: Words the owner said on a call. `message_event_id` names their
    /// transcript row when the pick was judged from one. A pick the call's
    /// talker made names none, since its transcript can land after it.
    Spoken {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message_event_id: Option<Uuid>,
    },
    Batch {
        permission_id: Uuid,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PermissionRequest {
    pub action: Action,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub breadth: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prepared: Option<String>,
    pub choices: BTreeMap<String, PermissionChoice>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub batch: Option<batch::Batch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lines: Option<GrantLines>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub holds_a_turn: bool,
}

impl PermissionRequest {
    /// INVARIANT: Whether the owner's words can answer this request, or only a tap can.
    /// `words::open_request_before` applies the same rule in SQL.
    pub fn words_answer(&self) -> bool {
        self.prepared.is_none() && !self.choices.is_empty()
    }

    pub fn waits_for_turn_end(&self) -> bool {
        self.prepared.is_none() && !self.holds_a_turn
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreparedCard {
    pub question: String,
    pub header: String,
    pub options: Vec<PreparedOption>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreparedOption {
    pub label: String,
    pub description: String,
}

impl PreparedCard {
    /// INVARIANT: The card as the question tool's input, so the walk and the hook's
    /// answer map read it exactly as they read an agent's own question.
    pub fn questions(&self) -> serde_json::Value {
        serde_json::json!([{
            "question": self.question,
            "header": self.header,
            "multiSelect": false,
            "options": self.options,
        }])
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GrantLines {
    pub grant_file: GrantFile,
    pub patterns: Vec<String>,
}

pub fn lane_action(file: GrantFile) -> Action {
    match file {
        GrantFile::AgentCommands => Action::RunCommand,
        GrantFile::CodingAgentTools => Action::UseTool,
        GrantFile::McpTools => Action::UseMcpTool,
        GrantFile::AgentDeniedCommands => Action::DenyCommand,
    }
}

pub fn lane_of(action: Action) -> Option<GrantFile> {
    GrantFile::ALL
        .into_iter()
        .find(|file| lane_action(*file) == action)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, sqlx::FromRow)]
pub struct PermissionRow {
    pub permission_id: Uuid,
    pub action: String,
    pub breadth: Option<String>,
    pub lifetime: String,
    pub holder_kind: String,
    pub holder_id: Option<String>,
    pub granted_in_thread_id: Option<Uuid>,
    pub granted_at: chrono::DateTime<chrono::Utc>,
    pub ended_at: Option<chrono::DateTime<chrono::Utc>>,
    pub end_reason: Option<String>,
    pub pattern: Option<String>,
    pub source: String,
    /// INVARIANT: An imported line whose shape its lane does not read. It still grants
    /// exactly what it granted in the file.
    #[sqlx(skip)]
    pub legacy_unmapped: bool,
}

impl PermissionRow {
    pub fn with_unmapped_flag(mut self) -> Self {
        let lane = serde_json::from_value(serde_json::Value::String(self.action.clone()))
            .ok()
            .and_then(lane_of);
        self.legacy_unmapped = self.source == crate::core::grants::GrantSource::LegacyImport.wire()
            && matches!((lane, &self.pattern), (Some(lane), Some(p)) if !lane.recognizes(p));
        self
    }
}

impl PermissionRow {
    pub fn is_live_for(&self, action: Action) -> bool {
        self.ended_at.is_none() && action_wire(action).is_ok_and(|wire| wire == self.action)
    }
}

#[derive(Debug, Clone, Default)]
pub struct PermissionUser {
    pub thread_id: Option<Uuid>,
    pub trigger_id: Option<String>,
}

pub const ROW_COLUMNS: &str = "permission_id, action, breadth, lifetime, holder_kind, holder_id, \
     granted_in_thread_id, granted_at, ended_at, end_reason, pattern, source";

pub static LIVE_SQL: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    format!(
        concat!(
            "(p.ended_at IS NULL AND EXISTS ( \
               SELECT 1 FROM events q \
               WHERE q.thread_id = p.granted_in_thread_id \
                 AND q.event_type = 'UserQuestionAsked' \
                 AND q.payload->>'tool_use_id' = p.request_tool_use_id \
                 AND q.sequence > ",
            crate::core::snapshot::boundary::restore_floor_sql!(),
            " AND (p.lifetime <> 'once' OR p.holder_kind <> 'thread' \
                      OR q.sequence > COALESCE(( \
                           SELECT MAX(sequence) FROM events \
                           WHERE aggregate_id = p.holder_id AND {opens}), 0))))"
        ),
        opens = crate::engine::standing_instruction::opens_current_turn_sql(),
    )
});

/// INVARIANT: The live permission that covers `action` at `breadth` for `user`, if any.
/// A lasting one is preferred over a once one, so a standing grant never
/// spends a once grant beside it. A tool grant covers only what its line
/// matches, which the tool-grant matcher answers, so none is returned here.
pub async fn live_permission<'e>(
    executor: impl sqlx::Executor<'e, Database = sqlx::Postgres>,
    user: &PermissionUser,
    action: Action,
    breadth: Option<&str>,
) -> Result<Option<PermissionRow>, BoxError> {
    Ok(sqlx::query_as(&format!(
        "SELECT {ROW_COLUMNS} FROM permissions p \
         WHERE {live} AND action = $1 AND pattern IS NULL \
           AND ((breadth IS NULL AND NOT $5) OR breadth = $2) \
           AND (holder_kind = 'workspace' \
                OR (holder_kind = 'thread' AND holder_id = $3) \
                OR (holder_kind = 'trigger' AND holder_id = $4)) \
         ORDER BY (lifetime = 'once'), granted_at LIMIT 1",
        live = *LIVE_SQL,
    ))
    .bind(action_wire(action)?)
    .bind(breadth)
    .bind(user.thread_id.map(|t| t.to_string()))
    .bind(user.trigger_id.as_deref())
    .bind(action == Action::GrantDeviceSession)
    .fetch_optional(executor)
    .await?)
}

pub async fn permission_row(
    pool: &sqlx::PgPool,
    permission_id: Uuid,
) -> Result<Option<PermissionRow>, sqlx::Error> {
    sqlx::query_as(&format!(
        "SELECT {ROW_COLUMNS} FROM permissions WHERE permission_id = $1"
    ))
    .bind(permission_id)
    .fetch_optional(pool)
    .await
}

pub fn action_wire(action: Action) -> Result<String, BoxError> {
    match serde_json::to_value(action)? {
        serde_json::Value::String(s) => Ok(s),
        other => Err(format!("action serialized as {other}, not a string").into()),
    }
}

pub fn thread_user(thread_id: Uuid) -> PermissionUser {
    PermissionUser {
        thread_id: Some(thread_id),
        trigger_id: None,
    }
}
