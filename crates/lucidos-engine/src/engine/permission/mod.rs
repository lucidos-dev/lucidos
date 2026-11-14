//! The permission model (ADR 0664): *actions*, *permission requests* and
//! *permissions*.
//!
//! A permission request is a `UserQuestionAsked` the engine raised and worded,
//! carrying a [`PermissionRequest`]. The owner's pick records a
//! `PermissionGranted`, which the `permissions` table folds. A gate asks
//! [`live_permission`] and spends a once permission with [`spend`].
//!
//! Only an engine-worded option grants, and only from the owner's device. An
//! agent's answer, or an option the request does not carry, grants nothing.
//! The owner picks by a tap, or by typed or spoken words that [`words`] reads.

mod action;
pub mod batch;
pub mod legacy;
pub(crate) mod menu;
pub(crate) mod prepare;
pub(crate) mod projection;
pub mod tool_grants;
pub mod words;

pub use action::Action;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{
    AnswerKind, EventChannel, EventMeta, MessageOrigin, ThreadEvent, UnattestedAct,
};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Every permission request's card id starts with this, so a log line names
/// what raised it.
const CARD_ID_PREFIX: &str = "permission-request:";

/// The unique index that makes a once permission single-use.
const SPENT_UNIQUE_INDEX: &str = "events_permission_spent_unique";

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
    pub(crate) fn wire(self) -> &'static str {
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
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GrantBreadth {
    /// The request's own breadth: the exact target it asked about.
    #[default]
    Asked,
    /// The whole action, whatever its target.
    Any,
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
    /// Ends the thread's automation pause, and so lets the asked step run.
    Unpause,
    Refuse,
    /// Revokes the grant file lines the request lists.
    Revoke,
}

impl PermissionChoice {
    /// A grant at the request's own breadth.
    pub(crate) fn grant(lifetime: Lifetime, holder: Holder) -> Self {
        Self::Grant {
            lifetime,
            holder,
            breadth: GrantBreadth::Asked,
        }
    }

    /// May a *batch* pick this option? A grant is one exact pick for one
    /// thread, once or for the thread, never a standing power (ADR 0664).
    pub(crate) fn batch_may_pick(&self) -> bool {
        match self {
            Self::Grant {
                lifetime, holder, ..
            } => {
                matches!(lifetime, Lifetime::Once | Lifetime::ThisThread)
                    && matches!(holder, Holder::Thread { .. })
            }
            // A revoke only takes power away (`tool_grants::act_on_review`).
            Self::Unpause | Self::Refuse | Self::Revoke => true,
        }
    }
}

/// SQL over a `UserQuestionAsked` row aliased `alias`: did the engine word this
/// card? Only the owner's device answers one. The legacy keys keep a card
/// asked before permission requests closed to everyone else.
pub(crate) fn engine_card_sql(alias: &str) -> String {
    format!(
        "({alias}.payload ? 'permission_request' OR {alias}.payload ? 'owner_approval' \
          OR {alias}.payload ? 'unattested_act')"
    )
}

/// How the owner picked a permission request's option: the receipt half of a
/// `PermissionGranted`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PickedBy {
    Tap,
    /// Words the owner typed, judged to choose the option.
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
    /// The grant file lines a review card is about. A pick that grants imports
    /// them, and [`PermissionChoice::Revoke`] revokes them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lines: Option<tool_grants::GrantLines>,
    /// The parked act an [`Action::ActForUnattestedCaller`] request runs once
    /// its permission is granted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unattested_act: Option<Box<UnattestedAct>>,
}

impl PermissionRequest {
    /// Whether the owner's words can answer this request, or only a tap can.
    /// `words::open_request_before` applies the same rule in SQL.
    pub fn words_answer(&self) -> bool {
        self.prepared.is_none() && self.unattested_act.is_none()
    }
}

/// One option the engine words for a request.
pub(crate) struct RequestOption {
    pub(crate) label: String,
    pub(crate) description: String,
    pub(crate) choice: PermissionChoice,
}

/// What the engine asks: its words, and the typed request behind them.
pub(crate) struct RequestCard {
    pub(crate) question: String,
    pub(crate) header: &'static str,
    pub(crate) action: Action,
    pub(crate) breadth: Option<String>,
    pub(crate) options: Vec<RequestOption>,
    pub(crate) batch: Option<batch::Batch>,
    pub(crate) unattested_act: Option<Box<UnattestedAct>>,
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
    pub(crate) fn questions(&self) -> serde_json::Value {
        serde_json::json!([{
            "question": self.question,
            "header": self.header,
            "multiSelect": false,
            "options": self.options,
        }])
    }

    /// The options the question parser makes of this card, with their ids.
    fn parsed_options(&self) -> Vec<crate::engine::thread_events::QuestionOption> {
        crate::engine::agent_session::parse_ask_user_question_inputs(
            &serde_json::json!({ "questions": self.questions() }),
        )
        .remove(0)
        .options
    }
}

impl RequestCard {
    /// Split the card into its words and its typed request. Each choice is
    /// keyed by the id the question parser gives its option.
    fn into_parts(self) -> (PreparedCard, PermissionRequest) {
        let words = PreparedCard {
            question: self.question,
            header: self.header.to_string(),
            options: self
                .options
                .iter()
                .map(|o| PreparedOption {
                    label: o.label.clone(),
                    description: o.description.clone(),
                })
                .collect(),
        };
        let choices = words
            .parsed_options()
            .into_iter()
            .zip(self.options)
            .map(|(option, spec)| (option.id, spec.choice))
            .collect();
        let request = PermissionRequest {
            action: self.action,
            breadth: self.breadth,
            prepared: None,
            choices,
            batch: self.batch,
            lines: None,
            unattested_act: self.unattested_act,
        };
        (words, request)
    }
}

/// Raise `card` on `thread_id` and return its id. The engine wrote every word,
/// so no agent text reaches an option.
pub(crate) async fn raise_request(
    bus: &EventBus,
    thread_id: Uuid,
    card: RequestCard,
    channel: EventChannel,
) -> Result<String, BoxError> {
    raise_request_about(bus, thread_id, card, None, channel).await
}

/// [`raise_request`] for a review card about grant file `lines`.
pub(crate) async fn raise_request_about(
    bus: &EventBus,
    thread_id: Uuid,
    card: RequestCard,
    lines: Option<tool_grants::GrantLines>,
    channel: EventChannel,
) -> Result<String, BoxError> {
    let tool_use_id = format!("{CARD_ID_PREFIX}{}", Uuid::new_v4());
    let (words, mut request) = card.into_parts();
    request.lines = lines;
    let options = words.parsed_options();
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::UserQuestionAsked {
            tool_use_id: tool_use_id.clone(),
            cc_session_id: String::new(),
            question: words.question,
            options,
            worktree_path: None,
            multi_select: false,
            owner_approval: None,
            unattested_act: None,
            permission_request: Some(request),
        },
        meta: EventMeta {
            channel: Some(channel),
            ..EventMeta::NONE
        },
    })
    .await?;
    Ok(tool_use_id)
}

/// The request a card on `thread_id` carries, or `None` for any other card.
pub(crate) async fn card_request(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    tool_use_id: &str,
) -> Result<Option<PermissionRequest>, BoxError> {
    let request: Option<serde_json::Value> = sqlx::query_scalar(
        "SELECT payload->'permission_request' FROM events \
         WHERE aggregate_id = $1 AND event_type = 'UserQuestionAsked' \
           AND payload->>'tool_use_id' = $2 AND payload ? 'permission_request' \
         ORDER BY sequence DESC LIMIT 1",
    )
    .bind(thread_id.to_string())
    .bind(tool_use_id)
    .fetch_optional(pool)
    .await?;
    Ok(request.map(serde_json::from_value).transpose()?)
}

/// The newest request for `action` on `thread_id` the owner can still tap,
/// by card id: unanswered, and no later step overtook it.
pub(crate) async fn tappable_request(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    action: Action,
) -> Result<Option<String>, BoxError> {
    Ok(sqlx::query_scalar(
        "SELECT q.payload->>'tool_use_id' FROM events q \
         WHERE q.aggregate_id = $1 AND q.event_type = 'UserQuestionAsked' \
           AND q.payload->'permission_request'->>'action' = $2 \
           AND NOT EXISTS (SELECT 1 FROM events a \
                            WHERE a.aggregate_id = $1 AND a.event_type = 'UserQuestionAnswered' \
                              AND a.payload->>'tool_use_id' = q.payload->>'tool_use_id') \
           AND NOT EXISTS (SELECT 1 FROM events t \
                            WHERE t.aggregate_id = $1 AND t.sequence > q.sequence \
                              AND t.event_type = ANY($3)) \
         ORDER BY q.sequence DESC LIMIT 1",
    )
    .bind(thread_id.to_string())
    .bind(action_wire(action)?)
    .bind(ThreadEvent::QUESTION_OVERTAKEN_EVENT_TYPES)
    .fetch_optional(pool)
    .await?)
}

/// What an answer to a permission request did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Resolution {
    /// The owner picked an option that grants: `PermissionGranted` landed.
    Granted(Uuid),
    /// The owner unpaused the thread's automation.
    Unpaused,
    /// The owner picked the option that grants nothing.
    Refused,
    /// The owner's pick on a review card imported or revoked grant file lines.
    LinesChanged,
    /// The answer was no owner's pick of an option, so it decided nothing.
    Ignored,
}

/// Act on an answer the card already recorded. Only a device's pick of one of
/// the request's own options counts. `grants_dir` holds the mirror a review
/// card's pick rewrites.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn resolve_answer(
    bus: &EventBus,
    grants_dir: &std::path::Path,
    thread_id: Uuid,
    tool_use_id: &str,
    request: &PermissionRequest,
    answer: &AnswerKind,
    actor: Option<&MessageOrigin>,
    picked_by: PickedBy,
) -> Result<Resolution, BoxError> {
    let AnswerKind::Selected { option_id } = answer else {
        return Ok(Resolution::Ignored);
    };
    if !matches!(actor, Some(MessageOrigin::Device { .. })) {
        crate::log!(
            "[Permission] Card {tool_use_id} on thread {thread_id} was answered by no device, \
             so it grants nothing"
        );
        return Ok(Resolution::Ignored);
    }
    let choice = request.choices.get(option_id);
    if matches!(picked_by, PickedBy::Batch { .. }) && !choice.is_some_and(|c| c.batch_may_pick()) {
        crate::log!(
            "[Permission] A batch picked option {option_id} of card {tool_use_id} on thread \
             {thread_id}, which only the owner's own tap may pick, so it grants nothing"
        );
        return Ok(Resolution::Ignored);
    }
    if let (Some(lines), Some(choice)) = (&request.lines, choice) {
        let changed = tool_grants::act_on_review(
            bus, grants_dir, thread_id, lines, choice, actor, &picked_by,
        )
        .await?;
        return Ok(match (changed, choice) {
            (true, _) => Resolution::LinesChanged,
            (false, PermissionChoice::Refuse) => Resolution::Refused,
            (false, _) => Resolution::Ignored,
        });
    }
    match choice {
        Some(PermissionChoice::Grant {
            lifetime,
            holder,
            breadth,
        }) => {
            let permission_id = Uuid::new_v4();
            bus.emit(BusEvent::Thread {
                thread_id,
                event: ThreadEvent::PermissionGranted {
                    permission_id,
                    action: request.action,
                    breadth: match breadth {
                        GrantBreadth::Asked => request.breadth.clone(),
                        GrantBreadth::Any => None,
                    },
                    lifetime: *lifetime,
                    holder: holder.clone(),
                    request_tool_use_id: tool_use_id.to_string(),
                    picked_by,
                    pattern: None,
                },
                meta: EventMeta::with_actor(actor.cloned()),
            })
            .await?;
            Ok(Resolution::Granted(permission_id))
        }
        Some(PermissionChoice::Unpause) => {
            crate::engine::automation_pause::unpause_automation(
                bus.pool(),
                bus,
                thread_id,
                crate::engine::thread_events::AutomationUnpauseCause::Unpause,
                actor.cloned(),
            )
            .await?;
            Ok(Resolution::Unpaused)
        }
        Some(PermissionChoice::Refuse) => Ok(Resolution::Refused),
        // Only a review card revokes, and it carries its lines.
        Some(PermissionChoice::Revoke) | None => Ok(Resolution::Ignored),
    }
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
    /// A tool grant's grant file line, which its lane's matcher reads.
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
    fn with_unmapped_flag(mut self) -> Self {
        let lane = serde_json::from_value(serde_json::Value::String(self.action.clone()))
            .ok()
            .and_then(tool_grants::lane_of);
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

const ROW_COLUMNS: &str = "permission_id, action, breadth, lifetime, holder_kind, holder_id, \
     granted_in_thread_id, granted_at, ended_at, end_reason, pattern, source";

/// SQL over the `permissions` row aliased `p`: is it live? It must be unended,
/// and its card must be newer than the last snapshot restore. A thread's once
/// grant also ends at that thread's next turn start. So a stale Allow once
/// never acts in a later context (ADR 0387).
pub(crate) static LIVE_SQL: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
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
        opens = crate::api::standing_instruction::opens_current_turn_sql(),
    )
});

/// The live permission that covers `action` at `breadth` for `user`, if any.
/// A lasting one is preferred over a once one, so a standing grant never
/// spends a once grant beside it. A tool grant covers only what its line
/// matches, which [`tool_grants`] answers, so none is returned here.
pub async fn live_permission<'e>(
    executor: impl sqlx::Executor<'e, Database = sqlx::Postgres>,
    user: &PermissionUser,
    action: Action,
    breadth: Option<&str>,
) -> Result<Option<PermissionRow>, BoxError> {
    Ok(sqlx::query_as(&format!(
        "SELECT {ROW_COLUMNS} FROM permissions p \
         WHERE {live} AND action = $1 AND pattern IS NULL \
           AND (breadth IS NULL OR breadth = $2) \
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
    .fetch_optional(executor)
    .await?)
}

/// The permission `permission_id`, live or ended.
pub(crate) async fn permission_row(
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

/// Use `permission` for one action. A once permission records its spend, and
/// `false` means the spend did not land: a lost race or a failed write. An
/// unknown never allows the action.
pub async fn spend(bus: &EventBus, permission: &PermissionRow) -> bool {
    if permission.lifetime != Lifetime::Once.wire() {
        return true;
    }
    let Some(thread_id) = permission.granted_in_thread_id else {
        crate::log!(
            "[Permission] Permission {} has no thread to record its spend in",
            permission.permission_id
        );
        return false;
    };
    match bus
        .emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::PermissionSpent {
                permission_id: permission.permission_id,
            },
            meta: EventMeta::NONE,
        })
        .await
    {
        Ok(_) => true,
        Err(e) => {
            let why = if crate::engine::event_bus::violates_unique_index(&*e, SPENT_UNIQUE_INDEX) {
                "another action spent it first".to_string()
            } else {
                e.to_string()
            };
            crate::log!(
                "[Permission] Could not spend permission {}: {why}",
                permission.permission_id
            );
            false
        }
    }
}

/// What a revoke did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevokeOutcome {
    Revoked,
    /// It had already ended. Nothing was recorded.
    AlreadyEnded,
    NotFound,
}

/// Revoke a permission, as `actor`. Revoking only takes power away, so any
/// identified caller may do it.
///
/// A standing tool grant is revoked as a change to its lane's set, which also
/// rewrites the lane's grant file. `grants_dir` holds that file.
pub async fn revoke(
    bus: &EventBus,
    grants_dir: &std::path::Path,
    permission_id: Uuid,
    actor: Option<MessageOrigin>,
) -> Result<RevokeOutcome, BoxError> {
    let Some(row) = permission_row(bus.pool(), permission_id).await? else {
        return Ok(RevokeOutcome::NotFound);
    };
    if row.ended_at.is_some() {
        return Ok(RevokeOutcome::AlreadyEnded);
    }
    let lane = serde_json::from_value(serde_json::Value::String(row.action.clone()))
        .ok()
        .and_then(tool_grants::lane_of);
    match (
        lane,
        row.pattern,
        row.holder_kind.as_str(),
        row.granted_in_thread_id,
    ) {
        (Some(lane), Some(pattern), "workspace", _) => {
            let by = tool_grants::ChangedBy {
                source: crate::core::grants::GrantSource::Revoke,
                thread_id: None,
                actor,
            };
            let lines = [pattern];
            tool_grants::change_standing(
                bus,
                grants_dir,
                lane,
                tool_grants::StandingChange::Remove(&lines),
                by,
            )
            .await?;
        }
        (_, _, _, granted_in) => {
            // A standing grant outlives the thread it was granted in, so its
            // revoke lands in the home thread once that thread is deleted.
            let thread_id = match granted_in {
                Some(thread_id) => thread_id,
                None => crate::engine::home_thread::ensure_home_thread(bus, bus.pool()).await?,
            };
            bus.emit(BusEvent::Thread {
                thread_id,
                event: ThreadEvent::PermissionRevoked { permission_id },
                meta: EventMeta::with_actor(actor),
            })
            .await?;
        }
    }
    Ok(RevokeOutcome::Revoked)
}

/// Every permission, newest first. `live_only` keeps the live ones. A tool
/// grant often has no card, so only its end decides whether it is live.
pub async fn list(pool: &sqlx::PgPool, live_only: bool) -> Result<Vec<PermissionRow>, sqlx::Error> {
    let rows: Vec<PermissionRow> = sqlx::query_as(&format!(
        "SELECT {ROW_COLUMNS} FROM permissions p \
         WHERE NOT $1 OR (p.pattern IS NOT NULL AND p.ended_at IS NULL) OR {live} \
         ORDER BY granted_at DESC",
        live = *LIVE_SQL,
    ))
    .bind(live_only)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(PermissionRow::with_unmapped_flag)
        .collect())
}

/// The stored spelling of an action: its kebab-case wire name.
pub(crate) fn action_wire(action: Action) -> Result<String, BoxError> {
    match serde_json::to_value(action)? {
        serde_json::Value::String(s) => Ok(s),
        other => Err(format!("action serialized as {other}, not a string").into()),
    }
}

#[cfg(test)]
#[path = "permission_tests.rs"]
mod tests;
