use std::path::Path;

use lucidos_agents_api::engine::turn_end::{ParentView, TurnRead};
use lucidos_engine::engine::thread_events::ChildCompletionStatus;
use lucidos_engine::llm::judgment::JudgmentProvider;
use serde::Deserialize;

use crate::decision_check::{opening_source, Answer, Check, Labelled};
use crate::Fallible;

pub(crate) const CHECK: Check = Check {
    command: "wake-decision",
    name: "wake decision",
    chat_judge: lucidos_agents_api::engine::turn_end::wake_decision_chat_judge,
    missed: "Missed must-wake",
    noise: "Noise woken",
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum Label {
    MustWake,
    File,
}

#[derive(Debug, Deserialize)]
struct LabelledReport {
    kind: String,
    source: Option<String>,
    message: String,
    parent_reply: String,
    reply: String,
    status: String,
    #[serde(default)]
    siblings_running: i64,
    label: Label,
}

#[derive(Debug, Deserialize)]
struct LabelledSet {
    report: Vec<LabelledReport>,
}

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
        let d = lucidos_agents_api::engine::turn_end::decide_wake_for_eval(
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

pub async fn run(set: &Path, backend: &str, models: &[String], out: &Path) -> Fallible<()> {
    let set: LabelledSet = toml::from_str(&std::fs::read_to_string(set)?)?;
    crate::decision_check::run(&CHECK, &set.report, backend, models, out).await
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn a_finish_the_gate_never_judges_is_refused() {
        assert!(judged_status("failure").is_err());
        assert!(judged_status("canceled").is_err());
    }
}
