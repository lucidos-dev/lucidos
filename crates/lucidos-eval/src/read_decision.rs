//! The read decision check. Each candidate decides the labelled turns in
//! `eval/read-decision/turns.toml`, through the turn-end gate's own state,
//! question and threshold. A candidate is a chat model, or a System One row.
//!
//! The number that decides first is a missed must-read: a reply the user
//! needed that Review would never list. Noise passed, a yes on a reply that
//! needed no reading, decides second. Plan:
//! `docs/plans/2026-10-11-read-decision-sees-the-answer-and-a-measured-model.md`,
//! invariant I7.

use std::path::Path;

use lucidos_engine::engine::{AnsweredQuestion, InFlight, PushedNotification, TurnRead};
use lucidos_engine::llm::judgment::JudgmentProvider;
use serde::Deserialize;

use crate::decision_check::{opening_source, Answer, Check, Labelled};
use crate::Fallible;

pub(crate) const CHECK: Check = Check {
    command: "read-decision",
    name: "read decision",
    chat_judge: lucidos_engine::engine::read_decision_chat_judge,
    missed: "Missed must-read",
    noise: "Noise passed",
};

/// One labelled turn.
#[derive(Debug, Deserialize)]
struct LabelledTurn {
    /// What opened the turn: `user`, `child-report`, `trigger` or `agent`.
    source: String,
    message: String,
    question: Option<String>,
    answer: Option<String>,
    reply: String,
    #[serde(default)]
    sub_threads: i64,
    #[serde(default)]
    event_waits: i64,
    /// Whether the turn's report wakes the parent agent, which acts on it.
    #[serde(default)]
    report_wakes_parent: bool,
    /// The notifications the turn sent before its reply.
    #[serde(default)]
    pushed: Vec<PushedNotification>,
    /// Whether the user should read the reply.
    read: bool,
    /// The shape of the turn, for the per-kind breakdown.
    kind: String,
}

#[derive(Debug, Deserialize)]
struct LabelledSet {
    turn: Vec<LabelledTurn>,
}

impl LabelledTurn {
    fn turn(&self) -> Fallible<TurnRead> {
        let answered = match (&self.question, &self.answer) {
            (Some(question), Some(answer)) => Some(AnsweredQuestion {
                question: question.clone(),
                answer: answer.clone(),
            }),
            (None, None) => None,
            _ => return Err("a turn needs both a question and an answer, or neither".into()),
        };
        Ok(TurnRead {
            message: self.message.clone(),
            source: opening_source(&self.source)?,
            answered,
            reply: self.reply.clone(),
            in_flight: InFlight {
                sub_threads: self.sub_threads,
                event_waits: self.event_waits,
            },
            report_wakes_parent: self.report_wakes_parent,
            pushed: self.pushed.clone(),
            ..TurnRead::default()
        })
    }
}

impl Labelled for LabelledTurn {
    fn kind(&self) -> &str {
        &self.kind
    }

    fn expected(&self) -> bool {
        self.read
    }

    fn validate(&self) -> Fallible<()> {
        self.turn().map(drop)
    }

    async fn decide(&self, provider: &dyn JudgmentProvider) -> Fallible<Answer> {
        let d = lucidos_engine::engine::decide_read_for_eval(
            provider,
            &self.turn()?,
            lucidos_engine::llm::metered::CallToken::for_eval_harness(),
        )
        .await?;
        Ok(Answer {
            yes: d.read,
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
    crate::decision_check::run(&CHECK, &set.turn, backend, models, out).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shipped_set() -> LabelledSet {
        toml::from_str(&std::fs::read_to_string(CHECK.shipped_set()).unwrap()).unwrap()
    }

    /// The shipped set parses into turns the gate can read, and holds both
    /// labels.
    #[test]
    fn the_labelled_set_is_well_formed() {
        let set = shipped_set();
        for item in &set.turn {
            item.validate().unwrap_or_else(|e| panic!("{item:?}: {e}"));
        }
        assert!(set.turn.iter().any(|t| t.read));
        assert!(set.turn.iter().any(|t| !t.read));
    }

    /// Every trigger-run shape the plan measures is in the shipped set, and a
    /// push reaches the gate's turn.
    #[test]
    fn the_shipped_set_holds_every_trigger_shape() {
        let set = shipped_set();
        for (kind, read) in [
            ("trigger-nothing", false),
            ("trigger-pushed", false),
            ("trigger-pushed-more", true),
            ("trigger-found", true),
            ("trigger-failure", true),
        ] {
            assert!(
                set.turn.iter().any(|t| t.kind == kind && t.read == read),
                "no {kind} turn labelled {read}"
            );
        }
        let pushed = set.turn.iter().find(|t| !t.pushed.is_empty()).unwrap();
        assert_eq!(pushed.turn().unwrap().pushed, pushed.pushed);
    }
}
