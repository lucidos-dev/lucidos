//! The three tool-permission cards, read as permission requests (ADR 0664).
//!
//! Three cards ask before a tool call: the command guard's, a coding agent's
//! and an MCP tool's. Each keeps its own events, consent route and grant file.
//! This module reads an open card as a permission request over a typed
//! [`super::Action`]. A batch and the owner's words then answer it through the card's
//! own resolver, so it stores the same grant its button stores.

use uuid::Uuid;

pub use super::tool_grants::AnsweredBy;
use super::PermissionRequest;
use crate::engine::thread_events::{AllowScope, GrantOffer, OfferWithheld};
use crate::engine::thread_events::{QuestionOption, ThreadEvent};
use crate::engine::LucidosEngine;

pub use crate::engine::thread_events::permission::legacy::{LegacyLane, LegacyPick};

/// The patterns an MCP card's buttons store, keyed by scope.
pub fn mcp_offer(server_id: &str, tool_name: &str) -> GrantOffer {
    use crate::engine::mcp_permission::derive_mcp_allow_pattern;
    let pattern = |scope| derive_mcp_allow_pattern(server_id, tool_name, scope);
    GrantOffer {
        deny: None,
        broad: pattern(AllowScope::Broad),
        narrow: pattern(AllowScope::Narrow),
        session: pattern(AllowScope::Session),
        withheld: None,
    }
}

/// A pick's label and detail, as the engine words it. An Always allow names
/// the line it stores. `None` when the card offers no such button.
pub fn pick_words(pick: LegacyPick, offer: &GrantOffer) -> Option<(String, String)> {
    let line = pick.scope().and_then(|scope| offer.pattern(scope));
    match (pick, line) {
        (LegacyPick::Once, _) => Some(("Allow once".into(), "Just this one time.".into())),
        (LegacyPick::Deny, _) => Some((
            "Deny".into(),
            "Refuse it. Lucidos is told no and carries on.".into(),
        )),
        (LegacyPick::Session, Some(_)) => Some((
            "Allow for this thread".into(),
            "This and anything like it, for the rest of this conversation.".into(),
        )),
        (LegacyPick::Narrow | LegacyPick::Broad, Some(line)) => Some((
            format!("Always allow `{line}`"),
            "Every future session may do this without asking, until it is revoked.".into(),
        )),
        (LegacyPick::Session | LegacyPick::Narrow | LegacyPick::Broad, None) => None,
    }
}

/// The picks a card with `offer` draws, each with its label and detail.
pub fn offered_picks(offer: &GrantOffer) -> Vec<(LegacyPick, String, String)> {
    LegacyPick::ALL
        .into_iter()
        .filter_map(|pick| pick_words(pick, offer).map(|(label, detail)| (pick, label, detail)))
        .collect()
}

/// An open tool-permission card.
#[derive(Clone, Debug, PartialEq)]
pub struct LegacyCard {
    pub lane: LegacyLane,
    pub thread_id: Uuid,
    pub request_id: String,
    /// When the card was asked, in the event order.
    pub sequence: i64,
    /// What the card asks, as the engine reads it aloud.
    pub question: String,
    /// The tool the card is about.
    pub tool_name: String,
    pub offer: GrantOffer,
}

fn text(payload: &serde_json::Value, key: &str) -> Option<String> {
    let value = payload.get(key)?.as_str()?.trim();
    (!value.is_empty()).then(|| value.to_string())
}

impl LegacyCard {
    fn from_payload(
        lane: LegacyLane,
        thread_id: Uuid,
        sequence: i64,
        payload: &serde_json::Value,
    ) -> Option<Self> {
        let request_id = text(payload, "request_id")?;
        let tool_name = text(payload, "tool_name").unwrap_or_default();
        let summary = text(payload, "summary").unwrap_or_default();
        let (question, offer) = match lane {
            LegacyLane::Command => {
                let command = text(payload, "command").unwrap_or_default();
                let stamped = payload
                    .get("grant_offer")
                    .and_then(|offer| serde_json::from_value(offer.clone()).ok());
                (format!("{summary}\n{command}"), stamped.unwrap_or_default())
            }
            LegacyLane::Mcp => {
                let server_id = text(payload, "server_id").unwrap_or_default();
                let server_name = text(payload, "server_name").unwrap_or_else(|| server_id.clone());
                let arguments = text(payload, "arguments_summary").unwrap_or_default();
                (
                    format!("{server_name} wants to use its {tool_name} tool.\n{arguments}"),
                    mcp_offer(&server_id, &tool_name),
                )
            }
            LegacyLane::CodingAgent => (
                summary,
                payload
                    .get("grant_offer")
                    .and_then(|offer| serde_json::from_value(offer.clone()).ok())
                    .unwrap_or_default(),
            ),
        };
        let question = if offer
            .withheld
            .is_some_and(OfferWithheld::standing_deny_unavailable)
        {
            let note =
                "Couldn’t check Always deny rules. Review this command before allowing it once.";
            let secret_note =
                if offer.withheld == Some(OfferWithheld::StandingDenyUnavailableSecretRead) {
                    " This reads Lucidos’s private files."
                } else {
                    ""
                };
            format!("{note}{secret_note}\n{question}")
        } else {
            question
        };
        Some(Self {
            lane,
            thread_id,
            request_id,
            sequence,
            question: question.trim().to_string(),
            tool_name,
            offer,
        })
    }

    /// The card a request event asks, or `None` for any other event.
    pub fn from_event(thread_id: Uuid, sequence: i64, event: &ThreadEvent) -> Option<Self> {
        let lane = match event {
            ThreadEvent::CommandPermissionRequested { .. } => LegacyLane::Command,
            ThreadEvent::CodingAgentPermissionRequest { .. } => LegacyLane::CodingAgent,
            ThreadEvent::McpPermissionRequested { .. } => LegacyLane::Mcp,
            _ => return None,
        };
        let payload = serde_json::to_value(event).ok()?;
        Self::from_payload(lane, thread_id, sequence, &payload)
    }

    /// The card's buttons as question options, with stable engine ids.
    pub fn options(&self) -> Vec<(LegacyPick, QuestionOption)> {
        offered_picks(&self.offer)
            .into_iter()
            .map(|(pick, label, detail)| {
                let option = QuestionOption {
                    id: pick.option_id(),
                    label,
                    description: Some(detail),
                    preview: None,
                    widget: None,
                    quiet: false,
                    quiet_reply: None,
                };
                (pick, option)
            })
            .collect()
    }

    /// The pick an engine option id names, when this card offers it.
    pub fn pick_of(&self, option_id: &str) -> Option<LegacyPick> {
        self.options()
            .into_iter()
            .find(|(_, option)| option.id == option_id)
            .map(|(pick, _)| pick)
    }

    /// The card as a permission request: its typed action, the tool it is
    /// about, and each option's choice.
    pub fn request(&self) -> PermissionRequest {
        PermissionRequest {
            action: self.lane.action(),
            breadth: (!self.tool_name.is_empty()).then(|| self.tool_name.clone()),
            prepared: None,
            choices: self
                .options()
                .into_iter()
                .map(|(pick, option)| (option.id, pick.choice(self.thread_id)))
                .collect(),
            batch: None,
            lines: None,
        }
    }

    /// Answer the card with `pick`. See [`resolve_pick`].
    pub async fn resolve(
        &self,
        engine: &LucidosEngine,
        pick: LegacyPick,
        answered_by: AnsweredBy,
        log_context: &str,
    ) -> bool {
        resolve_pick(
            engine,
            self.lane,
            self.request_id.clone(),
            pick,
            answered_by,
            log_context,
        )
        .await
    }
}

/// Answer card `request_id` in `lane` with `pick`, through the lane's own
/// resolver. It stores exactly what the card's button stores, and the answer
/// differs from a tap only in who answered. `false` when the card is no
/// longer pending.
pub async fn resolve_pick(
    engine: &LucidosEngine,
    lane: LegacyLane,
    request_id: String,
    pick: LegacyPick,
    answered_by: AnsweredBy,
    log_context: &str,
) -> bool {
    let (allowed, scope) = (pick.allowed(), pick.scope());
    match lane {
        LegacyLane::Command => {
            crate::engine::command_permission::resolve_command_permission(
                engine,
                request_id,
                allowed,
                scope,
                // Words and batches pick Deny, never Always deny: only a tap
                // on the card sets a standing rule.
                false,
                answered_by,
                log_context,
            )
            .await
            .unwrap_or(false)
        }
        LegacyLane::CodingAgent => crate::engine::cc_permission::resolve_coding_agent_permission(
            engine,
            request_id,
            allowed,
            scope,
            false,
            answered_by,
            log_context,
        )
        .await
        .unwrap_or(false),
        LegacyLane::Mcp => {
            crate::engine::mcp_permission::resolve_mcp_permission(
                engine,
                request_id,
                allowed,
                scope,
                answered_by,
                log_context,
            )
            .await
        }
    }
}

/// Cards built through [`LegacyCard::from_event`], for tests.
#[cfg(test)]
impl LegacyCard {
    fn of(event: ThreadEvent) -> Self {
        Self::from_event(Uuid::nil(), 0, &event).expect("a tool-permission request")
    }

    pub(crate) fn command(request_id: &str, tool_name: &str, command: &str, summary: &str) -> Self {
        Self::of(ThreadEvent::CommandPermissionRequested {
            decision_trace: None,
            request_id: request_id.into(),
            tool_use_id: format!("toolu-{request_id}"),
            tool_name: tool_name.into(),
            command: command.into(),
            summary: summary.into(),
            grant_head: None,
            grant_offer: None,
        })
    }

    pub(crate) fn mcp(
        request_id: &str,
        server_id: &str,
        server_name: &str,
        tool_name: &str,
        arguments_summary: &str,
    ) -> Self {
        Self::of(ThreadEvent::McpPermissionRequested {
            request_id: request_id.into(),
            tool_use_id: format!("toolu-{request_id}"),
            server_id: server_id.into(),
            server_name: server_name.into(),
            tool_name: tool_name.into(),
            arguments_summary: arguments_summary.into(),
        })
    }

    pub(crate) fn coding_agent(request_id: &str, offer: Option<GrantOffer>, summary: &str) -> Self {
        Self::of(ThreadEvent::CodingAgentPermissionRequest {
            request_id: request_id.into(),
            tool_use_id: format!("toolu-{request_id}"),
            tool_name: "Bash".into(),
            input: serde_json::json!({}),
            summary: summary.into(),
            grant_offer: offer,
        })
    }
}

/// Every tool-permission card still open on `thread_id`, newest first.
pub async fn open_cards(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
) -> Result<Vec<LegacyCard>, sqlx::Error> {
    let mut cards = Vec::new();
    for lane in LegacyLane::ALL {
        let rows: Vec<(i64, serde_json::Value)> = sqlx::query_as(
            "SELECT q.sequence, q.payload FROM events q \
             WHERE q.thread_id = $1 AND q.event_type = $2 \
               AND NOT EXISTS ( \
                 SELECT 1 FROM events r \
                 WHERE r.thread_id = $1 AND r.event_type = $3 \
                   AND r.payload->>'request_id' = q.payload->>'request_id' )",
        )
        .bind(thread_id)
        .bind(lane.requested_event())
        .bind(lane.resolved_event())
        .fetch_all(pool)
        .await?;
        cards.extend(rows.iter().filter_map(|(sequence, payload)| {
            LegacyCard::from_payload(lane, thread_id, *sequence, payload)
        }));
    }
    cards.sort_by_key(|card| std::cmp::Reverse(card.sequence));
    Ok(cards)
}

#[cfg(test)]
#[path = "legacy_tests.rs"]
mod tests;
