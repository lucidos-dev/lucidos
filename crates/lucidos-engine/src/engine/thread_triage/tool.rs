//! The `threads` tool's `triage` and `apply_triage` actions (ADR 0349).
//!
//! The caller is `execute_tool`'s ambient thread, never an argument. Every
//! decision lives in the pure modules beside this one. This file reads, runs
//! the approved steps through the existing paths, and words the result.

use serde::Deserialize;
use uuid::Uuid;

use super::facts::{load, TriageScope};
use super::proposal::{approved_proposal, record_proposal};
use super::report::{build_report, plan_apply, ApplyRequest, Refused};
use super::TriageAction;
use crate::api::threads::archive::{archive_family, rejection_text, PinnedMembers};
use crate::engine::agent_question::{
    answer_pending_question, lookup_pending_question_tool_use_id, AnswerResult,
};
use crate::engine::chat::agent_archive::agent_thread_actor;
use crate::engine::event_bus::BusEvent;
use crate::engine::thread_events::{AnswerKind, EventMeta, ThreadEvent};
use crate::engine::tools::ToolOutcome;

/// What the agent is told to do with a triage, beside the triage itself.
const TRIAGE_GUIDANCE: &str = "Show this to the user grouped by action, with each thread's link \
     and reason. Ask which actions to apply, then call apply_triage with the entries they \
     approved. apply_triage runs only after the user replies. Delete is never applied by \
     you: list delete candidates for the user to delete from the thread drawer, and say a \
     delete cannot be undone. Follow-up threads need the user; name what each one needs.";

#[derive(Debug, Deserialize)]
struct ApplyEntryArg {
    thread_id: String,
    action: String,
}

/// Parse `apply_triage`'s `entries` argument.
fn parse_requests(args: &serde_json::Value) -> Result<Vec<ApplyRequest>, String> {
    let entries: Vec<ApplyEntryArg> = args
        .get("entries")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|e| format!("Error: entries must be [{{thread_id, action}}]: {e}"))?
        .unwrap_or_default();
    if entries.is_empty() {
        return Err(
            "Error: apply_triage needs entries: [{thread_id, action}], from the \
                    triage the user approved."
                .to_string(),
        );
    }
    entries
        .into_iter()
        .map(|e| {
            let thread_id: Uuid = e
                .thread_id
                .parse()
                .map_err(|_| format!("Error: '{}' is not a thread id.", e.thread_id))?;
            let action = TriageAction::parse(&e.action).ok_or_else(|| {
                format!(
                    "Error: '{}' is not a triage action. Use archive, pin or dismiss_question.",
                    e.action
                )
            })?;
            Ok(ApplyRequest { thread_id, action })
        })
        .collect()
}

impl crate::engine::LucidosEngine {
    /// LLM tool: classify every inbox thread, record the proposal on the
    /// calling thread, and return it grouped for the user.
    pub(crate) async fn execute_triage_threads(&self, caller: Uuid) -> ToolOutcome {
        let rows = load(
            self.pool(),
            TriageScope::Inbox {
                except: Some(caller),
            },
        )
        .await
        .map_err(|e| format!("Error: could not read threads: {e}"))?;
        let workspace = self.workspace_name();
        let report = build_report(&rows, |id| crate::core::store::thread_link(&workspace, id));
        record_proposal(&self.event_bus, caller, report.proposal_entries())
            .await
            .map_err(|e| format!("Error: could not record the triage: {e}"))?;
        Ok(serde_json::json!({
            "counts": report.counts,
            "groups": report.groups,
            "not_listed": report.not_listed,
            "next": TRIAGE_GUIDANCE,
        })
        .to_string())
    }

    /// LLM tool: apply the entries the user approved from the newest triage.
    pub(crate) async fn execute_apply_thread_triage(
        &self,
        args: &serde_json::Value,
        caller: Uuid,
    ) -> ToolOutcome {
        let requests = parse_requests(args)?;
        let proposal = approved_proposal(self.pool(), caller)
            .await
            .map_err(|e| format!("Error: could not read the triage: {e}"))?
            .map_err(|refusal| format!("Error: {refusal}"))?;
        let ids: Vec<Uuid> = requests.iter().map(|r| r.thread_id).collect();
        let fresh = load(self.pool(), TriageScope::Ids(&ids))
            .await
            .map_err(|e| format!("Error: could not read threads: {e}"))?;
        let (run, mut refused) = plan_apply(&requests, &proposal, &fresh);

        let mut applied = Vec::new();
        for step in run {
            match self.run_triage_step(caller, step).await {
                Ok(()) => applied.push(serde_json::json!({
                    "thread_id": step.thread_id,
                    "action": step.action,
                })),
                Err(reason) => refused.push(Refused {
                    thread_id: step.thread_id,
                    action: step.action,
                    reason,
                }),
            }
        }
        Ok(serde_json::json!({ "applied": applied, "refused": refused }).to_string())
    }

    /// Run one approved step through the path the user's own button takes.
    async fn run_triage_step(&self, caller: Uuid, step: ApplyRequest) -> Result<(), String> {
        let actor = Some(agent_thread_actor(caller));
        match step.action {
            TriageAction::Archive => {
                let engine = self.clone_arc();
                // Boxed: the cascade's emits can re-enter an agentic loop.
                Box::pin(archive_family(
                    &engine,
                    step.thread_id,
                    actor,
                    PinnedMembers::ByActor,
                ))
                .await
                .map(|_| ())
                .map_err(|rejection| rejection_text(&rejection))
            }
            TriageAction::Pin => self
                .event_bus
                .emit(BusEvent::Thread {
                    thread_id: step.thread_id,
                    event: ThreadEvent::ThreadSaved,
                    meta: EventMeta::with_actor(actor),
                })
                .await
                .map(|_| ())
                .map_err(|e| format!("pinning failed: {e}")),
            TriageAction::DismissQuestion => {
                let tool_use_id = lookup_pending_question_tool_use_id(self.pool(), step.thread_id)
                    .await
                    .ok_or("no question is pending on it any more")?;
                let engine = self.clone_arc();
                match Box::pin(answer_pending_question(
                    &engine,
                    step.thread_id,
                    tool_use_id,
                    AnswerKind::Canceled,
                    actor,
                ))
                .await
                {
                    AnswerResult::Resumed => Ok(()),
                    AnswerResult::Conflict(message) => Err(message),
                }
            }
            // `plan_apply` refuses these first; this arm only keeps it honest.
            TriageAction::Delete | TriageAction::FollowUp | TriageAction::Keep => {
                Err(format!("'{}' is not applied here", step.action.as_str()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_arguments_parse_or_say_what_is_wrong() {
        let id = Uuid::new_v4();
        let ok = parse_requests(&serde_json::json!({
            "entries": [{"thread_id": id.to_string(), "action": "dismiss_question"}]
        }))
        .unwrap();
        assert_eq!(
            ok,
            vec![ApplyRequest {
                thread_id: id,
                action: TriageAction::DismissQuestion
            }]
        );
        for (args, says) in [
            (serde_json::json!({}), "needs entries"),
            (
                serde_json::json!({"entries": [{"thread_id": "Daily digest", "action": "archive"}]}),
                "is not a thread id",
            ),
            (
                serde_json::json!({"entries": [{"thread_id": id.to_string(), "action": "archive_all"}]}),
                "is not a triage action",
            ),
        ] {
            let err = parse_requests(&args).unwrap_err();
            assert!(err.contains(says), "{err}");
        }
    }
}
