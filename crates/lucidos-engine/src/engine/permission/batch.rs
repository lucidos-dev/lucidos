//! A *batch* (ADR 0664): one permission request that carries several exact
//! items. An item answers a card in a sub-thread below the raiser, widens one
//! trigger's grants, or classifies a group of workspaces (ADR 0671). The card
//! may be a question card, a permission request or a tool-permission card.
//!
//! The engine words the batch and hashes its items. One confirm spends it
//! once, and [`run_batch`] checks each item again as it runs. An item whose
//! card closed, whose options changed, whose plan changed or whose trigger's
//! definition digest changed is skipped, and its `BatchItemRan` receipt says
//! why.
//!
//! Four rules keep a batch from closing a circle. Only the owner's confirm
//! runs it. No batch holds a batch. Each item is one exact pick or one
//! trigger's grant, never a standing power, so no item picks an Always allow
//! and none answers an unattested caller's card. A thread's batch never holds
//! a card of its own. A secret read is never an item.

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::legacy::{self, LegacyCard, LegacyLane, LegacyPick};
use super::{
    raise_request, Action, GrantBreadth, Holder, Lifetime, PermissionChoice, PermissionRequest,
    PickedBy, RequestCard, RequestOption, Resolution,
};
use crate::engine::agent_question::{answer_pending_question_picked_by, AnswerResult};
use crate::engine::command_guard::SideEffectCategory;
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::git_ops::{plan_approves, proposed_plan_binding, HumanInput, PlanBinding};
use crate::engine::thread_events::{
    AnswerKind, EventChannel, EventMeta, MessageOrigin, ThreadEvent,
};
use crate::engine::trigger_writes::{TriggerRegistryWriter, TriggerWrite};
use crate::engine::LucidosEngine;
use crate::triggers::{TriggerConfig, TriggerRun};

/// The in-memory trigger registry a batch reads its trigger items from.
pub(crate) type Triggers = std::sync::RwLock<HashMap<String, TriggerConfig>>;

/// How many hex characters of a definition digest a batch card shows.
const SHOWN_DIGEST_CHARS: usize = 12;

/// The most items one batch carries. Each is a line the owner reads before
/// one confirm.
pub const MAX_BATCH_ITEMS: usize = 20;

/// How much of a card's question an item line quotes.
const QUESTION_EXCERPT_CHARS: usize = 120;

/// The most workspaces one classification group names.
pub const MAX_CLASSIFIED_WORKSPACES: usize = 200;

/// How many slugs a classification line names before "and N more".
const SHOWN_SLUGS: usize = 3;

const RUN_LABEL: &str = "Run all";

const ALWAYS_ALLOW_REFUSAL: &str =
    "an Always allow is the owner's own tap on the card, never a batch item.";
const REFUSE_LABEL: &str = "Don't run";

/// The items a batch carries, and the digest the engine stored when it
/// worded them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Batch {
    pub items: Vec<BatchItem>,
    /// SHA-256 of the items, in hex. A batch whose items no longer hash to it
    /// runs nothing.
    pub digest: String,
}

/// One exact item of a batch.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BatchItem {
    /// Pick one option on an open card in a sub-thread.
    CardAnswer {
        thread_id: Uuid,
        tool_use_id: String,
        option_id: String,
        option_label: String,
        /// SHA-256 of the card's question and options when the batch was built.
        card_digest: String,
        /// The plan this pick approves, when the thread waits on one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        plan: Option<PlanBinding>,
    },
    /// Pick one button on an open tool-permission card in a sub-thread. The
    /// card's own resolver stores what the button stores.
    PermissionCardAnswer {
        thread_id: Uuid,
        lane: LegacyLane,
        request_id: String,
        pick: LegacyPick,
        option_label: String,
    },
    /// Widen one trigger's grants, as long as its definition is unchanged.
    TriggerGrant {
        trigger_id: String,
        /// The trigger's definition digest when the batch was built.
        definition_digest: String,
        /// The side-effect categories the grant adds.
        side_effects: Vec<SideEffectCategory>,
        /// Whether the grant lets the trigger's runs start coding agents.
        coding_agents: bool,
    },
    /// Set the origin of a group of unclassified workspaces. Only entries
    /// still `unknown` move, and the gateway refuses any other.
    WorkspaceClassification {
        workspaces: Vec<String>,
        origin: ClassifiedOrigin,
        /// The agent's evidence beyond the names, quoted on the card.
        evidence: String,
    },
}

/// An origin a classification may set. Never `unknown`: that is where every
/// entry starts. The gateway's registry is the authority on the full set.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ClassifiedOrigin {
    Agent,
    Demo,
    Eval,
    E2e,
    User,
}

impl ClassifiedOrigin {
    /// The wire name the gateway's classify route takes.
    pub(crate) fn wire(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Demo => "demo",
            Self::Eval => "eval",
            Self::E2e => "e2e",
            Self::User => "user",
        }
    }
}

impl BatchItem {
    fn answers_a_card(&self) -> bool {
        matches!(
            self,
            Self::CardAnswer { .. } | Self::PermissionCardAnswer { .. }
        )
    }
}

/// What running one item did.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BatchItemOutcome {
    Applied,
    /// Nothing ran for this item. `reason` names what changed.
    Skipped {
        reason: String,
    },
    /// Part of the item ran, then a refusal stopped it: some workspaces of a
    /// classification moved. `reason` says how many, and what stopped it.
    PartlyApplied {
        reason: String,
    },
}

/// One item a caller asks a batch to carry. The engine resolves it into a
/// [`BatchItem`], reading every word from the card itself.
#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BatchItemRequest {
    CardAnswer {
        thread_id: Uuid,
        /// The card to answer: a question card's id or a tool-permission
        /// card's request id. Absent means the thread's only open card.
        #[serde(default)]
        tool_use_id: Option<String>,
        /// The option's label, as the card shows it.
        option: String,
    },
    TriggerGrant {
        /// The trigger's id or slug.
        trigger: String,
        /// Side-effect category names, as the trigger's grant spells them.
        #[serde(default)]
        side_effects: Vec<String>,
        #[serde(default)]
        coding_agents: bool,
    },
    WorkspaceClassification {
        /// The workspaces' slugs.
        workspaces: Vec<String>,
        origin: ClassifiedOrigin,
        evidence: String,
    },
}

/// Why a batch was not raised.
#[derive(Debug)]
pub enum BatchError {
    /// The request breaks a batch rule. The text says which item and why.
    Refused(String),
    /// What the batch needs could not be read.
    Unreadable(String),
}

impl std::fmt::Display for BatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Refused(why) | Self::Unreadable(why) => f.write_str(why),
        }
    }
}

impl std::error::Error for BatchError {}

fn unreadable(e: impl std::fmt::Display) -> BatchError {
    BatchError::Unreadable(format!("The batch could not be built: {e}"))
}

/// An open card as a batch reads it.
struct OpenCard {
    tool_use_id: String,
    question: String,
    options: Vec<crate::engine::thread_events::QuestionOption>,
    multi_select: bool,
    permission_request: Option<PermissionRequest>,
    unattested_act: bool,
    digest: String,
}

impl OpenCard {
    fn from_payload(payload: &serde_json::Value) -> Option<Self> {
        let tool_use_id = payload.get("tool_use_id")?.as_str()?.to_string();
        let options = payload.get("options").cloned().unwrap_or_default();
        let question = payload.get("question").cloned().unwrap_or_default();
        let multi_select = payload
            .get("multi_select")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        let request = payload
            .get("permission_request")
            .cloned()
            .unwrap_or_default();
        // A request this build cannot read is no card a batch may answer.
        let permission_request: Option<PermissionRequest> =
            serde_json::from_value(request.clone()).ok()?;
        Some(Self {
            digest: sha256_hex(
                &serde_json::to_vec(&serde_json::json!({
                    "question": question,
                    "options": options,
                    "multi_select": multi_select,
                    "permission_request": request,
                }))
                .ok()?,
            ),
            tool_use_id,
            question: question.as_str().unwrap_or_default().to_string(),
            options: serde_json::from_value(options).unwrap_or_default(),
            multi_select,
            unattested_act: ["/unattested_act", "/permission_request/unattested_act"]
                .iter()
                .any(|at| payload.pointer(at).is_some_and(|v| !v.is_null())),
            permission_request,
        })
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    crate::api::hex::hex_lower(&Sha256::digest(bytes))
}

fn items_digest(items: &[BatchItem]) -> Result<String, serde_json::Error> {
    Ok(sha256_hex(&serde_json::to_vec(items)?))
}

/// Every card open on `thread_id`, newest first, by the same predicate that
/// says a thread is parked on a question.
async fn open_cards(pool: &sqlx::PgPool, thread_id: Uuid) -> Result<Vec<OpenCard>, sqlx::Error> {
    let sql = format!(
        "SELECT uqa.payload FROM events uqa WHERE {} ORDER BY uqa.sequence DESC",
        crate::engine::agent_recovery::unanswered_question_predicate_sql("$1"),
    );
    let payloads: Vec<serde_json::Value> = sqlx::query_scalar(&sql)
        .bind(thread_id.to_string())
        .fetch_all(pool)
        .await?;
    Ok(payloads.iter().filter_map(OpenCard::from_payload).collect())
}

/// Resolve `requests` into the exact items a batch raised on `raiser` would
/// carry. Refuses the whole batch when any one item breaks a rule.
pub(crate) async fn build_batch(
    pool: &sqlx::PgPool,
    triggers: &Triggers,
    raiser: Uuid,
    requests: &[BatchItemRequest],
) -> Result<Batch, BatchError> {
    if requests.is_empty() {
        return Err(BatchError::Refused(
            "A batch needs at least one item.".into(),
        ));
    }
    if requests.len() > MAX_BATCH_ITEMS {
        return Err(BatchError::Refused(format!(
            "A batch carries at most {MAX_BATCH_ITEMS} items, and this one has {}. Split it.",
            requests.len()
        )));
    }
    let mut items = Vec::with_capacity(requests.len());
    for (index, request) in requests.iter().enumerate() {
        let refused = |why: String| BatchError::Refused(format!("Item {}: {why}", index + 1));
        let item = match request {
            BatchItemRequest::CardAnswer {
                thread_id,
                tool_use_id,
                option,
            } => build_card_answer(pool, raiser, *thread_id, tool_use_id.as_deref(), option).await,
            BatchItemRequest::TriggerGrant {
                trigger,
                side_effects,
                coding_agents,
            } => build_trigger_grant(triggers, trigger, side_effects, *coding_agents),
            BatchItemRequest::WorkspaceClassification {
                workspaces,
                origin,
                evidence,
            } => build_classification(workspaces, *origin, evidence),
        }
        .map_err(|e| match e {
            BatchError::Refused(why) => refused(why),
            other => other,
        })?;
        if let Some(why) = items.iter().find_map(|seen| duplicate(seen, &item)) {
            return Err(refused(why.into()));
        }
        items.push(item);
    }
    let digest = items_digest(&items).map_err(unreadable)?;
    Ok(Batch { items, digest })
}

/// Why `item` repeats `seen`, when the two act on the same card or trigger.
fn duplicate(seen: &BatchItem, item: &BatchItem) -> Option<&'static str> {
    match (seen, item) {
        (
            BatchItem::CardAnswer {
                thread_id: a_thread,
                tool_use_id: a_card,
                ..
            },
            BatchItem::CardAnswer {
                thread_id: b_thread,
                tool_use_id: b_card,
                ..
            },
        ) if a_thread == b_thread && a_card == b_card => {
            Some("the batch already answers that card.")
        }
        (
            BatchItem::PermissionCardAnswer {
                thread_id: a_thread,
                request_id: a_card,
                ..
            },
            BatchItem::PermissionCardAnswer {
                thread_id: b_thread,
                request_id: b_card,
                ..
            },
        ) if a_thread == b_thread && a_card == b_card => {
            Some("the batch already answers that card.")
        }
        (
            BatchItem::TriggerGrant { trigger_id: a, .. },
            BatchItem::TriggerGrant { trigger_id: b, .. },
        ) if a == b => {
            Some("the batch already grants to that trigger. Name all its grants in one item.")
        }
        (
            BatchItem::WorkspaceClassification { workspaces: a, .. },
            BatchItem::WorkspaceClassification { workspaces: b, .. },
        ) if a.iter().any(|w| b.contains(w)) => {
            Some("the batch already classifies one of those workspaces.")
        }
        _ => None,
    }
}

/// One classification group: a known origin, the agent's evidence, and at
/// most [`MAX_CLASSIFIED_WORKSPACES`] distinct workspace slugs.
fn build_classification(
    workspaces: &[String],
    origin: ClassifiedOrigin,
    evidence: &str,
) -> Result<BatchItem, BatchError> {
    let refused = |why: String| Err(BatchError::Refused(why));
    let evidence = evidence.trim();
    if evidence.is_empty() {
        return refused(
            "a classification needs evidence beyond the names, which the owner reads.".into(),
        );
    }
    if workspaces.is_empty() || workspaces.len() > MAX_CLASSIFIED_WORKSPACES {
        return refused(format!(
            "a classification group names 1 to {MAX_CLASSIFIED_WORKSPACES} workspaces, and \
             this one names {}.",
            workspaces.len()
        ));
    }
    if let Some(bad) = workspaces
        .iter()
        .find(|w| !lucidos_gateway_constants::is_valid_workspace_id(w))
    {
        return refused(format!("`{bad}` is not a workspace slug."));
    }
    let mut seen = std::collections::HashSet::new();
    if let Some(twice) = workspaces.iter().find(|w| !seen.insert(*w)) {
        return refused(format!("the group names `{twice}` twice."));
    }
    Ok(BatchItem::WorkspaceClassification {
        workspaces: workspaces.to_vec(),
        origin,
        evidence: evidence.to_string(),
    })
}

/// A trigger by its id, or else by its slug.
fn find_trigger(triggers: &Triggers, id_or_slug: &str) -> Option<TriggerConfig> {
    let configs = triggers.read().unwrap_or_else(|e| e.into_inner());
    configs
        .get(id_or_slug)
        .or_else(|| configs.values().find(|c| c.slug == id_or_slug))
        .cloned()
}

/// Resolve one trigger's grant into an item that adds only what the trigger
/// lacks, bound to its definition digest.
fn build_trigger_grant(
    triggers: &Triggers,
    trigger: &str,
    side_effects: &[String],
    coding_agents: bool,
) -> Result<BatchItem, BatchError> {
    let mut categories: Vec<SideEffectCategory> = Vec::new();
    for name in side_effects.iter().map(|n| n.trim()) {
        if name.to_ascii_lowercase().contains("secret") {
            return Err(BatchError::Refused(
                "a secret read is never grantable, to a trigger or to anyone.".into(),
            ));
        }
        let category = serde_json::from_value::<SideEffectCategory>(name.into())
            .ok()
            .filter(SideEffectCategory::is_grantable)
            .ok_or_else(|| {
                let names: Vec<String> = SideEffectCategory::GRANTABLE
                    .iter()
                    .filter_map(|c| serde_json::to_value(c).ok())
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect();
                BatchError::Refused(format!(
                    "\"{name}\" is not a side-effect category. Use one of: {}.",
                    names.join(", ")
                ))
            })?;
        if !categories.contains(&category) {
            categories.push(category);
        }
    }
    let config = find_trigger(triggers, trigger.trim())
        .ok_or_else(|| BatchError::Refused(format!("no trigger \"{trigger}\" exists.")))?;
    categories.retain(|c| !config.side_effect_grant.contains(c));
    let coding_agents = coding_agents && !config.coding_agent_grant;
    if categories.is_empty() && !coding_agents {
        return Err(BatchError::Refused(format!(
            "that grants nothing the trigger \"{}\" lacks.",
            config.name
        )));
    }
    Ok(BatchItem::TriggerGrant {
        definition_digest: config.definition_digest(),
        trigger_id: config.id,
        side_effects: categories,
        coding_agents,
    })
}

/// A card open on a sub-thread: a question card or a tool-permission card.
enum AnyCard {
    Question(OpenCard),
    Tool(LegacyCard),
}

/// Every card open on `thread_id`, both kinds, newest first.
async fn any_open_cards(pool: &sqlx::PgPool, thread_id: Uuid) -> Result<Vec<AnyCard>, BatchError> {
    let questions = open_cards(pool, thread_id).await.map_err(unreadable)?;
    let tools = legacy::open_cards(pool, thread_id)
        .await
        .map_err(unreadable)?;
    Ok(questions
        .into_iter()
        .map(AnyCard::Question)
        .chain(tools.into_iter().map(AnyCard::Tool))
        .collect())
}

async fn build_card_answer(
    pool: &sqlx::PgPool,
    raiser: Uuid,
    thread_id: Uuid,
    tool_use_id: Option<&str>,
    option: &str,
) -> Result<BatchItem, BatchError> {
    if thread_id == raiser {
        return Err(BatchError::Refused(
            "that card is on this thread, and a thread never answers its own card.".into(),
        ));
    }
    let below = crate::core::store::is_thread_ancestor(pool, raiser, thread_id)
        .await
        .map_err(unreadable)?;
    if !below {
        return Err(BatchError::Refused(format!(
            "thread {thread_id} is not below this thread. A batch carries cards from its own \
             sub-threads only."
        )));
    }
    let mut cards = any_open_cards(pool, thread_id).await?;
    let card = match tool_use_id {
        Some(id) => cards
            .into_iter()
            .find(|card| match card {
                AnyCard::Question(c) => c.tool_use_id == id,
                AnyCard::Tool(c) => c.request_id == id,
            })
            .ok_or_else(|| {
                BatchError::Refused(format!("card {id} is not open on thread {thread_id}."))
            })?,
        None => match cards.len() {
            0 => {
                return Err(BatchError::Refused(format!(
                    "thread {thread_id} has no open card."
                )))
            }
            1 => cards.remove(0),
            n => {
                return Err(BatchError::Refused(format!(
                    "thread {thread_id} has {n} open cards, so name one with `tool_use_id`."
                )))
            }
        },
    };
    match card {
        AnyCard::Question(card) => answer_question_card(pool, thread_id, card, option).await,
        AnyCard::Tool(card) => answer_tool_card(card, option),
    }
}

/// The option on a card whose label is `wanted`, ignoring case and spaces.
fn option_labelled<'a, T>(
    options: &'a [T],
    label: impl Fn(&T) -> &str,
    wanted: &str,
) -> Result<&'a T, BatchError> {
    let wanted = wanted.trim();
    options
        .iter()
        .find(|o| label(o).trim().eq_ignore_ascii_case(wanted))
        .ok_or_else(|| {
            let labels: Vec<&str> = options.iter().map(&label).collect();
            BatchError::Refused(format!(
                "that card offers no option \"{wanted}\". It offers: {}.",
                labels.join(", ")
            ))
        })
}

async fn answer_question_card(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    card: OpenCard,
    option: &str,
) -> Result<BatchItem, BatchError> {
    let request = card.permission_request.as_ref();
    if request.is_some_and(|r| r.batch.is_some()) {
        return Err(BatchError::Refused(
            "that card is a batch, and no batch holds a batch.".into(),
        ));
    }
    if card.unattested_act {
        return Err(BatchError::Refused(
            "that card asks for an unverified caller's act, which only a tap on the card \
             itself allows."
                .into(),
        ));
    }
    if card.multi_select {
        return Err(BatchError::Refused(
            "that card takes several picks, and a batch item picks exactly one option.".into(),
        ));
    }
    let picked = option_labelled(&card.options, |o| o.label.as_str(), option)?;
    if request.is_some_and(|r| {
        !r.choices
            .get(&picked.id)
            .is_some_and(|c| c.batch_may_pick())
    }) {
        return Err(BatchError::Refused(ALWAYS_ALLOW_REFUSAL.into()));
    }
    let approves = plan_approves(&HumanInput::Picked {
        label: Some(picked.label.clone()),
    });
    let plan = if approves {
        proposed_plan_binding(pool, thread_id)
            .await
            .map_err(BatchError::Refused)?
    } else {
        None
    };
    Ok(BatchItem::CardAnswer {
        thread_id,
        tool_use_id: card.tool_use_id,
        option_id: picked.id.clone(),
        option_label: picked.label.clone(),
        card_digest: card.digest,
        plan,
    })
}

fn answer_tool_card(card: LegacyCard, option: &str) -> Result<BatchItem, BatchError> {
    let options = card.options();
    let (pick, picked) = option_labelled(&options, |(_, o)| o.label.as_str(), option)?;
    if matches!(pick, LegacyPick::Narrow | LegacyPick::Broad) {
        return Err(BatchError::Refused(ALWAYS_ALLOW_REFUSAL.into()));
    }
    Ok(BatchItem::PermissionCardAnswer {
        thread_id: card.thread_id,
        lane: card.lane,
        request_id: card.request_id,
        pick: *pick,
        option_label: picked.label.clone(),
    })
}

/// A card's question as one line of at most [`QUESTION_EXCERPT_CHARS`].
fn excerpt(question: &str) -> String {
    let line = question.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() <= QUESTION_EXCERPT_CHARS {
        return line;
    }
    let cut: String = line.chars().take(QUESTION_EXCERPT_CHARS).collect();
    format!("{}…", cut.trim_end())
}

/// Word `batch` for its card: one line per item. A card answer names the
/// thread, the question, the pick and any plan it approves. A trigger grant
/// names the trigger, its digest, what it runs, the grants and its MCP servers.
async fn word_batch(
    pool: &sqlx::PgPool,
    triggers: &Triggers,
    batch: &Batch,
) -> Result<String, BatchError> {
    let mut lines = Vec::with_capacity(batch.items.len());
    for (index, item) in batch.items.iter().enumerate() {
        let line = match item {
            BatchItem::CardAnswer {
                thread_id,
                tool_use_id,
                option_label,
                plan,
                ..
            } => {
                word_card_answer(pool, *thread_id, tool_use_id, option_label, plan.as_ref()).await?
            }
            BatchItem::PermissionCardAnswer {
                thread_id,
                request_id,
                option_label,
                ..
            } => word_tool_card_answer(pool, *thread_id, request_id, option_label).await?,
            BatchItem::TriggerGrant {
                trigger_id,
                definition_digest,
                side_effects,
                coding_agents,
            } => word_trigger_grant(
                triggers,
                trigger_id,
                definition_digest,
                side_effects,
                *coding_agents,
            ),
            BatchItem::WorkspaceClassification {
                workspaces,
                origin,
                evidence,
            } => word_classification(workspaces, *origin, evidence),
        };
        lines.push(format!("{}. {line}", index + 1));
    }
    let all = |kind: fn(&BatchItem) -> bool| batch.items.iter().all(kind);
    let noun = if all(BatchItem::answers_a_card) {
        "answer"
    } else if all(|i| matches!(i, BatchItem::TriggerGrant { .. })) {
        "grant"
    } else {
        "item"
    };
    let lead = match batch.items.len() {
        1 => format!("Run this {noun} once?"),
        n => format!("Run these {n} {noun}s once?"),
    };
    Ok(format!(
        "{lead} Lucidos checks each one again as it runs, and skips any whose card, plan, \
         trigger or workspace changed.\n\n{}",
        lines.join("\n")
    ))
}

/// A thread's display title, or its id when it has none.
async fn thread_title(pool: &sqlx::PgPool, thread_id: Uuid) -> Result<String, BatchError> {
    let title: Option<(Option<String>, Option<String>)> =
        sqlx::query_as("SELECT title, first_message FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_optional(pool)
            .await
            .map_err(unreadable)?;
    Ok(title
        .map(|(title, first)| crate::core::store::format_display_title(title, first))
        .map(|title| title.trim().to_string())
        .unwrap_or_else(|| thread_id.to_string()))
}

/// What a permission request asks for, worded from its typed request and
/// never from the card's question, which quotes the agent's reason.
async fn word_permission_request(
    pool: &sqlx::PgPool,
    request: &PermissionRequest,
) -> Result<String, BatchError> {
    let Some(breadth) = request.breadth.as_deref() else {
        return Ok(format!(
            "the permission request for {}",
            request.action.act()
        ));
    };
    let target = match Uuid::parse_str(breadth) {
        Ok(thread_id) => format!("**{}**", thread_title(pool, thread_id).await?),
        Err(_) => format!("`{breadth}`"),
    };
    Ok(format!(
        "the permission request to {} {target}",
        request.action.word()
    ))
}

async fn word_card_answer(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    tool_use_id: &str,
    option_label: &str,
    plan: Option<&PlanBinding>,
) -> Result<String, BatchError> {
    let title = thread_title(pool, thread_id).await?;
    let card = open_cards(pool, thread_id)
        .await
        .map_err(unreadable)?
        .into_iter()
        .find(|c| c.tool_use_id == tool_use_id);
    let (asks, any_target) = match card {
        Some(OpenCard {
            permission_request: Some(request),
            options,
            ..
        }) => {
            let picked = options.iter().find(|o| o.label == option_label);
            let any_target = picked
                .and_then(|o| request.choices.get(&o.id))
                .is_some_and(|c| {
                    matches!(
                        c,
                        PermissionChoice::Grant {
                            breadth: GrantBreadth::Any,
                            ..
                        }
                    )
                });
            (word_permission_request(pool, &request).await?, any_target)
        }
        card => (
            format!(
                "\"{}\"",
                card.map(|c| excerpt(&c.question)).unwrap_or_default()
            ),
            false,
        ),
    };
    let mut line = format!("**{title}**: answer {asks} with **{option_label}**.");
    if any_target {
        line.push_str(" That allows it for any target, not only this one.");
    }
    if let Some(plan) = plan {
        line.push_str(&format!(
            " That approves the plan `{}` as committed now.",
            plan.plan_path
        ));
    }
    Ok(line)
}

async fn word_tool_card_answer(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    request_id: &str,
    option_label: &str,
) -> Result<String, BatchError> {
    let title = thread_title(pool, thread_id).await?;
    let question = legacy::open_cards(pool, thread_id)
        .await
        .map_err(unreadable)?
        .into_iter()
        .find(|c| c.request_id == request_id)
        .map(|c| excerpt(&c.question))
        .unwrap_or_default();
    Ok(format!(
        "**{title}**: answer the permission card \"{question}\" with **{option_label}**."
    ))
}

fn word_trigger_grant(
    triggers: &Triggers,
    trigger_id: &str,
    definition_digest: &str,
    side_effects: &[SideEffectCategory],
    coding_agents: bool,
) -> String {
    let shown = &definition_digest[..definition_digest.len().min(SHOWN_DIGEST_CHARS)];
    let mut grants: Vec<&str> = Vec::new();
    if coding_agents {
        grants.push("Start coding agents");
    }
    grants.extend(side_effects.iter().map(SideEffectCategory::label));
    let grants = grants
        .iter()
        .map(|g| format!("**{g}**"))
        .collect::<Vec<_>>()
        .join(", ");
    let Some(config) = find_trigger(triggers, trigger_id) else {
        return format!("Trigger `{trigger_id}` (definition `{shown}`): grant {grants}.");
    };
    let runs = match &config.run {
        TriggerRun::Intent { intent } => format!("runs \"{}\"", excerpt(intent)),
        TriggerRun::Script { path } => format!("runs the script `{path}`"),
    };
    let servers = if config.mcp_servers.is_empty() {
        "It uses no MCP servers.".to_string()
    } else {
        format!("MCP servers: {}.", config.mcp_servers.join(", "))
    };
    format!(
        "Trigger **{}** (definition `{shown}`), which {runs}: grant {grants}. {servers}",
        config.name.trim()
    )
}

/// One line per group: the count, up to three slugs, the origin, and the
/// agent's evidence, quoted on one line so it cannot pose as engine text.
fn word_classification(workspaces: &[String], origin: ClassifiedOrigin, evidence: &str) -> String {
    let mut shown = workspaces
        .iter()
        .take(SHOWN_SLUGS)
        .map(|w| format!("`{w}`"))
        .collect::<Vec<_>>()
        .join(", ");
    if workspaces.len() > SHOWN_SLUGS {
        shown.push_str(&format!(" and {} more", workspaces.len() - SHOWN_SLUGS));
    }
    let count = match workspaces.len() {
        1 => "1 workspace".to_string(),
        n => format!("{n} workspaces"),
    };
    format!(
        "Classify **{count}** ({shown}) as **{}**, if still unclassified. The agent's \
         evidence: \u{201c}{}\u{201d}",
        origin.wire(),
        crate::api::permission_requests::one_line(evidence)
    )
}

/// Build, word and raise a batch on `raiser`, returning its card id.
pub(crate) async fn raise_batch(
    bus: &EventBus,
    triggers: &Triggers,
    raiser: Uuid,
    requests: &[BatchItemRequest],
) -> Result<String, BatchError> {
    let batch = build_batch(bus.pool(), triggers, raiser, requests).await?;
    let question = word_batch(bus.pool(), triggers, &batch).await?;
    let is_coding_agent: Option<bool> =
        sqlx::query_scalar("SELECT is_coding_agent FROM thread_summaries WHERE thread_id = $1")
            .bind(raiser)
            .fetch_optional(bus.pool())
            .await
            .map_err(unreadable)?;
    let channel = if is_coding_agent == Some(true) {
        EventChannel::ClaudeCode
    } else {
        EventChannel::Chat
    };
    let card = RequestCard {
        question,
        header: "Batch",
        action: Action::RunBatch,
        breadth: None,
        options: vec![
            RequestOption {
                label: RUN_LABEL.into(),
                description: "Run each item once, as listed. Lucidos skips and names any item \
                              whose card, plan or trigger changed."
                    .into(),
                choice: PermissionChoice::grant(
                    Lifetime::Once,
                    Holder::Thread { thread_id: raiser },
                ),
            },
            RequestOption {
                label: REFUSE_LABEL.into(),
                description: "Run nothing. Each card stays open in its own thread, and each \
                              trigger keeps the grants it has."
                    .into(),
                choice: PermissionChoice::Refuse,
            },
        ],
        batch: Some(batch),
        unattested_act: None,
    };
    raise_request(bus, raiser, card, channel)
        .await
        .map_err(|e| BatchError::Unreadable(format!("The batch could not be raised: {e}")))
}

/// Why `item` may not run now, or `None` when its card, plan or trigger is
/// exactly as the batch showed it.
async fn item_drift(
    pool: &sqlx::PgPool,
    triggers: &Triggers,
    item: &BatchItem,
) -> Result<Option<String>, String> {
    let (thread_id, tool_use_id, card_digest, plan) = match item {
        BatchItem::CardAnswer {
            thread_id,
            tool_use_id,
            card_digest,
            plan,
            ..
        } => (*thread_id, tool_use_id, card_digest, plan),
        BatchItem::PermissionCardAnswer {
            thread_id,
            lane,
            request_id,
            pick,
            option_label,
        } => {
            let cards = legacy::open_cards(pool, *thread_id)
                .await
                .map_err(|e| format!("the card could not be read ({e})"))?;
            let Some(card) = cards
                .into_iter()
                .find(|c| c.lane == *lane && &c.request_id == request_id)
            else {
                return Ok(Some("the card is no longer open".into()));
            };
            let offered = card
                .options()
                .into_iter()
                .any(|(p, o)| p == *pick && &o.label == option_label);
            return Ok((!offered).then(|| "the card's options changed".into()));
        }
        BatchItem::TriggerGrant {
            trigger_id,
            definition_digest,
            ..
        } => return Ok(trigger_drift(triggers, trigger_id, definition_digest)),
        BatchItem::WorkspaceClassification { workspaces, .. } => {
            return crate::api::workspace_broker::unclassified_drift(workspaces).await
        }
    };
    let cards = open_cards(pool, thread_id)
        .await
        .map_err(|e| format!("the card could not be read ({e})"))?;
    let Some(card) = cards.into_iter().find(|c| &c.tool_use_id == tool_use_id) else {
        return Ok(Some("the card is no longer open".into()));
    };
    if &card.digest != card_digest {
        return Ok(Some("the card's question or options changed".into()));
    }
    let Some(plan) = plan else {
        return Ok(None);
    };
    match proposed_plan_binding(pool, thread_id).await {
        Ok(Some(current)) if &current == plan => Ok(None),
        Ok(Some(current)) if current.marked_at != plan.marked_at => {
            Ok(Some("the plan was marked again".into()))
        }
        Ok(Some(_)) => Ok(Some("the plan's committed text changed".into())),
        Ok(None) => Ok(Some("the plan no longer waits for approval".into())),
        Err(why) => Ok(Some(why)),
    }
}

/// Why a trigger grant may not run, or `None` when the trigger still has the
/// definition digest the batch showed.
fn trigger_drift(triggers: &Triggers, trigger_id: &str, shown: &str) -> Option<String> {
    let configs = triggers.read().unwrap_or_else(|e| e.into_inner());
    match configs.get(trigger_id) {
        None => Some("the trigger no longer exists".into()),
        Some(config) if config.definition_digest() != shown => {
            Some("the trigger's definition changed".into())
        }
        Some(_) => None,
    }
}

/// Record one item's receipt on the batch's thread.
async fn record_item(
    bus: &EventBus,
    raiser: Uuid,
    permission_id: Uuid,
    index: usize,
    item: &BatchItem,
    outcome: BatchItemOutcome,
) {
    let index = u32::try_from(index).unwrap_or(u32::MAX);
    if let Err(e) = bus
        .emit(BusEvent::Thread {
            thread_id: raiser,
            event: ThreadEvent::BatchItemRan {
                permission_id,
                index,
                item: item.clone(),
                outcome,
            },
            meta: EventMeta::NONE,
        })
        .await
    {
        crate::log!("[Batch] Receipt for item {index} of {permission_id} not recorded: {e}");
    }
}

/// Spend a confirmed batch's grant once, then run each item that is still
/// exactly as the batch showed it. A lost spend runs nothing.
pub(crate) fn run_batch(
    engine: Arc<LucidosEngine>,
    raiser: Uuid,
    request: PermissionRequest,
    resolution: Resolution,
    actor: Option<MessageOrigin>,
) -> futures::future::BoxFuture<'static, ()> {
    Box::pin(async move {
        let (Some(batch), Resolution::Granted(permission_id)) = (&request.batch, resolution) else {
            return;
        };
        let bus = &engine.event_bus;
        let permission = match super::permission_row(bus.pool(), permission_id).await {
            Ok(Some(permission)) => permission,
            Ok(None) => return,
            Err(e) => {
                crate::log!("[Batch] Grant {permission_id} on {raiser} unreadable: {e}");
                return;
            }
        };
        if !super::spend(bus, &permission).await {
            return;
        }
        let intact = items_digest(&batch.items).is_ok_and(|d| d == batch.digest);
        for (index, item) in batch.items.iter().enumerate() {
            let drift = if intact {
                item_drift(bus.pool(), &engine.trigger_configs, item).await
            } else {
                Ok(Some(
                    "the batch's items no longer match what it showed".into(),
                ))
            };
            let outcome = match drift {
                Ok(None) => apply_item(&engine, item, permission_id, actor.clone()).await,
                Ok(Some(reason)) | Err(reason) => BatchItemOutcome::Skipped { reason },
            };
            record_item(bus, raiser, permission_id, index, item, outcome).await;
        }
    })
}

/// A confirmed batch as boot reads it: the grant, its thread, how it ended,
/// the batch, and the indexes that already have a receipt.
type ConfirmedBatchRow = (Uuid, Uuid, Option<String>, serde_json::Value, Vec<i64>);

/// The receipt reason for an item a restart cut off.
const RESTARTED_REASON: &str = "Lucidos restarted before this item ran";

/// Close out every batch a restart cut off. A confirmed batch runs after its
/// confirm returns, so a restart can stop it between items. Each item with no
/// receipt gets one, skipped, so the owner reads which cards still wait. A
/// confirm the restart caught before its spend is spent here, unrun. Runs at
/// boot, before any batch can run.
pub async fn close_cut_off_batches_on_startup(bus: &EventBus) {
    let rows: Vec<ConfirmedBatchRow> = match sqlx::query_as(
        "SELECT p.permission_id, p.granted_in_thread_id, p.end_reason, \
                    q.payload->'permission_request'->'batch', \
                    ARRAY(SELECT (r.payload->>'index')::bigint FROM events r \
                           WHERE r.thread_id = p.granted_in_thread_id \
                             AND r.event_type = 'BatchItemRan' \
                             AND r.payload->>'permission_id' = p.permission_id::text) \
               FROM permissions p \
               JOIN events g ON g.thread_id = p.granted_in_thread_id \
                            AND g.event_type = 'PermissionGranted' \
                            AND g.payload->>'permission_id' = p.permission_id::text \
               JOIN events q ON q.thread_id = g.thread_id \
                            AND q.event_type = 'UserQuestionAsked' \
                            AND q.payload->>'tool_use_id' = g.payload->>'request_tool_use_id' \
              WHERE p.action = 'run-batch' \
                AND (p.ended_at IS NULL OR p.end_reason = 'spent')",
    )
    .fetch_all(bus.pool())
    .await
    {
        Ok(rows) => rows,
        Err(e) => {
            crate::log!("[Batch] Cut-off batches unreadable at boot: {e}");
            return;
        }
    };
    for (permission_id, raiser, end_reason, batch, ran) in rows {
        let Ok(batch) = serde_json::from_value::<Batch>(batch) else {
            continue;
        };
        if ran.len() >= batch.items.len() {
            continue;
        }
        if end_reason.is_none() {
            match super::permission_row(bus.pool(), permission_id).await {
                Ok(Some(permission)) if super::spend(bus, &permission).await => {}
                _ => continue,
            }
        }
        for (index, item) in batch.items.iter().enumerate() {
            if ran.contains(&i64::try_from(index).unwrap_or(i64::MAX)) {
                continue;
            }
            let outcome = BatchItemOutcome::Skipped {
                reason: RESTARTED_REASON.into(),
            };
            record_item(bus, raiser, permission_id, index, item, outcome).await;
        }
    }
}

/// Run one item as the owner who confirmed the batch.
async fn apply_item(
    engine: &Arc<LucidosEngine>,
    item: &BatchItem,
    permission_id: Uuid,
    actor: Option<MessageOrigin>,
) -> BatchItemOutcome {
    let (thread_id, tool_use_id, option_id) = match item {
        BatchItem::CardAnswer {
            thread_id,
            tool_use_id,
            option_id,
            ..
        } => (*thread_id, tool_use_id, option_id),
        BatchItem::PermissionCardAnswer {
            lane,
            request_id,
            pick,
            ..
        } => {
            let answered_by = legacy::AnsweredBy {
                actor,
                picked_by: super::PickedBy::Batch { permission_id },
            };
            let resolved = legacy::resolve_pick(
                engine,
                *lane,
                request_id.clone(),
                *pick,
                answered_by,
                "[Batch]",
            )
            .await;
            return if resolved {
                BatchItemOutcome::Applied
            } else {
                BatchItemOutcome::Skipped {
                    reason: "the card is no longer open".into(),
                }
            };
        }
        BatchItem::TriggerGrant { .. } => {
            return apply_trigger_grant(&engine.trigger_registry_writer(), item, actor).await
        }
        BatchItem::WorkspaceClassification {
            workspaces, origin, ..
        } => {
            return match crate::api::workspace_broker::classify(workspaces, *origin).await {
                Ok(()) => BatchItemOutcome::Applied,
                Err(stop) if stop.moved == 0 => BatchItemOutcome::Skipped {
                    reason: stop.reason,
                },
                Err(stop) => BatchItemOutcome::PartlyApplied {
                    reason: stop.reason,
                },
            }
        }
    };
    match answer_pending_question_picked_by(
        engine,
        thread_id,
        tool_use_id.clone(),
        AnswerKind::Selected {
            option_id: option_id.clone(),
        },
        actor,
        None,
        PickedBy::Batch { permission_id },
    )
    .await
    {
        AnswerResult::Resumed => BatchItemOutcome::Applied,
        AnswerResult::Conflict(reason) => BatchItemOutcome::Skipped { reason },
    }
}

/// Widen a trigger's grants by `item`. Under the registry's write lock, the
/// trigger must still have the digest the batch showed and the grants it held
/// a moment ago.
async fn apply_trigger_grant(
    writer: &TriggerRegistryWriter<'_>,
    item: &BatchItem,
    actor: Option<MessageOrigin>,
) -> BatchItemOutcome {
    let BatchItem::TriggerGrant {
        trigger_id,
        definition_digest,
        side_effects,
        coding_agents,
    } = item
    else {
        return BatchItemOutcome::Skipped {
            reason: "the item is not a trigger grant".into(),
        };
    };
    let held = writer
        .trigger_configs
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .get(trigger_id)
        .map(|c| (c.side_effect_grant.clone(), c.coding_agent_grant));
    let Some((held_side_effects, held_coding_agents)) = held else {
        return BatchItemOutcome::Skipped {
            reason: "the trigger no longer exists".into(),
        };
    };
    let mut granted = held_side_effects.clone();
    granted.extend(
        side_effects
            .iter()
            .filter(|c| !held_side_effects.contains(c)),
    );
    let payload = serde_json::json!({
        "trigger_id": trigger_id,
        "side_effect_grant": granted,
        "coding_agent_grant": held_coding_agents || *coding_agents,
    });
    let unchanged = |current: Option<&TriggerConfig>, _: &serde_json::Value| {
        current.is_some_and(|c| {
            c.definition_digest() == *definition_digest
                && c.side_effect_grant == held_side_effects
                && c.coding_agent_grant == held_coding_agents
        })
    };
    match writer
        .write_if(TriggerWrite::Updated, trigger_id, payload, actor, unchanged)
        .await
    {
        Ok(true) => BatchItemOutcome::Applied,
        Ok(false) => BatchItemOutcome::Skipped {
            reason: "the trigger changed while the batch ran".into(),
        },
        Err(e) => BatchItemOutcome::Skipped {
            reason: format!("the grant could not be recorded ({e})"),
        },
    }
}

#[cfg(test)]
#[path = "batch_tests.rs"]
mod tests;
