//! The wake decision check. Each candidate decides the labelled child turns in
//! `eval/wake-decision/turns.toml`, through the turn-end gate's own state,
//! question and threshold. A candidate is a chat model, or a System One row.
//!
//! The number that decides first is a missed must-wake: a parent that waits
//! for a wake that never comes. Noise woken, a wake for a report the parent
//! could read on its next turn, decides second. Plan:
//! `docs/plans/2026-10-12-wake-decision-eval.md`.

use std::path::Path;

use lucidos_engine::engine::thread_events::ChildCompletionStatus;
use lucidos_engine::engine::{ParentView, TurnRead};
use lucidos_engine::llm::judgment::JudgmentProvider;
use serde::Deserialize;

use crate::decision_check::{opening_source, Answer, Check, Labelled};
use crate::Fallible;

pub(crate) const CHECK: Check = Check {
    command: "wake-decision",
    name: "wake decision",
    chat_judge: lucidos_engine::engine::wake_decision_chat_judge,
    missed: "Missed must-wake",
    noise: "Noise woken",
};

/// What the parent should do with the child's report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum Label {
    /// The parent must act now, so the report has to wake it.
    MustWake,
    /// The parent can read the report on its next turn.
    File,
}

/// One labelled child turn.
#[derive(Debug, Deserialize)]
struct LabelledReport {
    /// The shape of the case, for the per-kind breakdown.
    kind: String,
    /// What opened the child's turn. Absent means `agent`: the parent's spawn
    /// message or follow-up.
    source: Option<String>,
    /// The message that opened the child's turn.
    message: String,
    /// The parent's latest reply.
    parent_reply: String,
    /// The child's reply.
    reply: String,
    /// `success` or `no-changes`, the two finishes the gate judges.
    status: String,
    /// The parent's other children still working.
    #[serde(default)]
    siblings_running: i64,
    label: Label,
}

#[derive(Debug, Deserialize)]
struct LabelledSet {
    report: Vec<LabelledReport>,
}

/// The finish a set names. Any other finish wakes without a call, so the eval
/// has nothing to score for it.
fn judged_status(status: &str) -> Fallible<ChildCompletionStatus> {
    match status {
        "success" => Ok(ChildCompletionStatus::Success),
        "no-changes" => Ok(ChildCompletionStatus::NoChanges),
        other => Err(format!(
            "status {other:?}: the gate judges only success and no-changes, and wakes on the rest"
        )
        .into()),
    }
}

impl LabelledReport {
    fn turn(&self) -> Fallible<TurnRead> {
        Ok(TurnRead {
            message: self.message.clone(),
            source: opening_source(self.source.as_deref().unwrap_or("agent"))?,
            reply: self.reply.clone(),
            ..TurnRead::default()
        })
    }

    fn parent(&self) -> ParentView {
        ParentView {
            latest_reply: self.parent_reply.clone(),
            siblings_running: self.siblings_running,
        }
    }
}

impl Labelled for LabelledReport {
    fn kind(&self) -> &str {
        &self.kind
    }

    fn expected(&self) -> bool {
        self.label == Label::MustWake
    }

    fn validate(&self) -> Fallible<()> {
        if self.reply.trim().is_empty() {
            return Err("an empty reply wakes the parent without a call".into());
        }
        self.turn()?;
        judged_status(&self.status).map(drop)
    }

    async fn decide(&self, provider: &dyn JudgmentProvider) -> Fallible<Answer> {
        let d = lucidos_engine::engine::decide_wake_for_eval(
            provider,
            &self.turn()?,
            judged_status(&self.status)?,
            &self.parent(),
            lucidos_engine::llm::metered::CallToken::for_eval_harness(),
        )
        .await?;
        Ok(Answer {
            yes: d.wake,
            probability: d.probability,
            input_tokens: d.input_tokens,
            output_tokens: d.output_tokens,
        })
    }
}

/// Run every candidate over the set, append each decision to `out` as JSON
/// lines, and print the table.
pub async fn run(set: &Path, backend: &str, models: &[String], out: &Path) -> Fallible<()> {
    let set: LabelledSet = toml::from_str(&std::fs::read_to_string(set)?)?;
    crate::decision_check::run(&CHECK, &set.report, backend, models, out).await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shipped set parses into reports the gate judges, and holds both
    /// labels.
    #[test]
    fn the_labelled_set_is_well_formed() {
        let set: LabelledSet =
            toml::from_str(&std::fs::read_to_string(CHECK.shipped_set()).unwrap()).unwrap();
        for item in &set.report {
            item.validate().unwrap_or_else(|e| panic!("{item:?}: {e}"));
        }
        assert!(set.report.iter().any(|r| r.label == Label::MustWake));
        assert!(set.report.iter().any(|r| r.label == Label::File));
    }

    /// A finish the gate never judges is refused, rather than scored.
    #[test]
    fn a_finish_the_gate_never_judges_is_refused() {
        assert!(judged_status("failure").is_err());
        assert!(judged_status("canceled").is_err());
    }
}
