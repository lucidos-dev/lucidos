//! Permission vocabulary and reads shared by event emission and projection.

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

/// How long a permission lasts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Lifetime {
    /// Spent by the first action it covers.
    Once,
    /// Lasts as long as its holder thread.
    ThisThread,
    /// Ends when the holder thread's change applies.
    UntilPlanLands,
    /// Lasts until someone revokes it.
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

/// Who may use a permission.
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

/// How far one granting option reaches within its action.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum GrantBreadth {
    /// The request's own breadth: the exact target it asked about.
    #[default]
    Asked,
    /// The whole action, whatever its target.
    Any,
    /// Same device and exact lifetime on targets created by this workspace.
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

/// What picking one option of a permission request does.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PermissionChoice {
    Grant {
        lifetime: Lifetime,
        holder: Holder,
        /// A card asked before options had a breadth reads as the asked one.
        #[serde(default, skip_serializing_if = "GrantBreadth::is_asked")]
        breadth: GrantBreadth,
    },
    /// The Unpause option of a retired step card. Stored cards still carry
    /// it, and picking it does nothing: the boot sweep retires such cards.
    #[serde(rename = "unpause")]
    RetiredUnpause,
    Refuse,
    /// Refuses, and refuses the same request from then on: a card's Always
    /// deny. Only an engine-worded card offers it.
    RefuseStanding,
    /// Historical option for revoking imported patterns.
    Revoke,
}

impl PermissionChoice {
    /// A grant at the request's own breadth.
    pub fn grant(lifetime: Lifetime, holder: Holder) -> Self {
        Self::Grant {
            lifetime,
            holder,
            breadth: GrantBreadth::Asked,
        }
    }

    /// Is this an Allow once: a grant the first action it covers spends?
    pub fn is_once(&self) -> bool {
        matches!(
            self,
            Self::Grant {
                lifetime: Lifetime::Once,
                ..
            }
        )
    }

    /// May a *batch* pick this option? A grant covers one stamped scope for one
    /// thread, once or for the thread, never a standing power (ADR 0664).
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

/// SQL over a `UserQuestionAsked` row aliased `alias`: did the engine word this
/// card? Only the owner's device answers one. The legacy key keeps a card
/// asked before permission requests closed to everyone else.
pub fn engine_card_sql(alias: &str) -> String {
    format!("({alias}.payload ? 'permission_request' OR {alias}.payload ? 'owner_approval')")
}

/// How the owner picked a permission request's option: the receipt half of a
/// `PermissionGranted`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PickedBy {
    Tap,
    /// Historical receipt for words the owner typed.
    Typed {
        message_event_id: Uuid,
    },
    /// Words the owner said on a call. `message_event_id` names their
    /// transcript row when the pick was judged from one. A pick the call's
    /// talker made names none, since its transcript can land after it.
    Spoken {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message_event_id: Option<Uuid>,
    },
    /// The owner confirmed a *batch* that held this pick, spending
    /// `permission_id`.
    Batch {
        permission_id: Uuid,
    },
}

/// The typed half of a *permission request*: the action it asks for, and what
/// each of its options does.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PermissionRequest {
    pub action: Action,
    /// The exact target the request asks about, such as a workspace slug or
    /// a command head. An option at [`GrantBreadth::Asked`] grants this one.
    /// Absent asks about the whole action.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub breadth: Option<String>,
    /// The id the agent asked its question tool with, when the agent prepared
    /// this request. Its call waits for the answer. Absent, the engine raised
    /// the card itself and no agent call waits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prepared: Option<String>,
    /// What each option does, keyed by option id.
    pub choices: BTreeMap<String, PermissionChoice>,
    /// The items a *batch* carries. Only a [`Action::RunBatch`] request has
    /// them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub batch: Option<batch::Batch>,
    /// Historical grant-file review payload. Its choices no longer change grants.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lines: Option<GrantLines>,
    /// A turn holds for this card's answer: the call that raised it polls, or
    /// the run parked on it writes no terminal. Such a card posts at once,
    /// even mid-turn, since the turn it would wait for never ends first.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub holds_a_turn: bool,
}

impl PermissionRequest {
    /// Whether the owner's words can answer this request, or only a tap can.
    /// `words::open_request_before` applies the same rule in SQL.
    pub fn words_answer(&self) -> bool {
        self.prepared.is_none() && !self.choices.is_empty()
    }

    /// Whether this card waits for a running turn to end before it posts.
    /// Lucidos raised it, and no turn holds for its answer (ADR 0803).
    pub fn waits_for_turn_end(&self) -> bool {
        self.prepared.is_none() && !self.holds_a_turn
    }
}

/// The words of a request an agent prepared: what its question tool shows.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreparedCard {
    pub question: String,
    pub header: String,
    pub options: Vec<PreparedOption>,
}

/// One option of a [`PreparedCard`], in the order the card shows it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreparedOption {
    pub label: String,
    pub description: String,
}

impl PreparedCard {
    /// The card as the question tool's input, so the walk and the hook's
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

/// The patterns a historical grant-file review listed.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GrantLines {
    pub grant_file: GrantFile,
    pub patterns: Vec<String>,
}

/// The action a lane's grants cover.
pub fn lane_action(file: GrantFile) -> Action {
    match file {
        GrantFile::AgentCommands => Action::RunCommand,
        GrantFile::CodingAgentTools => Action::UseTool,
        GrantFile::McpTools => Action::UseMcpTool,
        GrantFile::AgentDeniedCommands => Action::DenyCommand,
    }
}

/// The lane whose grants cover `action`, if it is a tool action.
pub fn lane_of(action: Action) -> Option<GrantFile> {
    GrantFile::ALL
        .into_iter()
        .find(|file| lane_action(*file) == action)
}

/// One permission as the `permissions` table holds it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, sqlx::FromRow)]
pub struct PermissionRow {
    pub permission_id: Uuid,
    pub action: String,
    pub breadth: Option<String>,
    pub lifetime: String,
    pub holder_kind: String,
    pub holder_id: Option<String>,
    /// The thread the grant was made in. `None` for the boot import, a
    /// Settings edit, or a thread deleted since.
    pub granted_in_thread_id: Option<Uuid>,
    pub granted_at: chrono::DateTime<chrono::Utc>,
    pub ended_at: Option<chrono::DateTime<chrono::Utc>>,
    /// `spent`, `revoked` or `plan_landed`, once it ended.
    pub end_reason: Option<String>,
    /// A tool grant pattern, which its lane matcher reads.
    pub pattern: Option<String>,
    /// Where it came from: `card`, `batch`, `legacy-import`, `settings` or
    /// `file-review`.
    pub source: String,
    /// An imported line whose shape its lane does not read. It still grants
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
    /// Whether this permission is live and covers `action`.
    pub fn is_live_for(&self, action: Action) -> bool {
        self.ended_at.is_none() && action_wire(action).is_ok_and(|wire| wire == self.action)
    }
}

/// Who wants to use a permission: a thread, and the trigger whose run it is.
#[derive(Debug, Clone, Default)]
pub struct PermissionUser {
    pub thread_id: Option<Uuid>,
    pub trigger_id: Option<String>,
}

pub const ROW_COLUMNS: &str = "permission_id, action, breadth, lifetime, holder_kind, holder_id, \
     granted_in_thread_id, granted_at, ended_at, end_reason, pattern, source";

/// SQL over the `permissions` row aliased `p`: is it live? It must be unended,
/// and its card must be newer than the last snapshot restore. A thread's once
/// grant also ends at that thread's next turn start. So a stale Allow once
/// never acts in a later context (ADR 0387).
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

/// The live permission that covers `action` at `breadth` for `user`, if any.
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

/// The permission `permission_id`, live or ended.
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

/// The stored spelling of an action: its kebab-case wire name.
pub fn action_wire(action: Action) -> Result<String, BoxError> {
    match serde_json::to_value(action)? {
        serde_json::Value::String(s) => Ok(s),
        other => Err(format!("action serialized as {other}, not a string").into()),
    }
}

/// The asking thread as a permission user.
pub fn thread_user(thread_id: Uuid) -> PermissionUser {
    PermissionUser {
        thread_id: Some(thread_id),
        trigger_id: None,
    }
}
