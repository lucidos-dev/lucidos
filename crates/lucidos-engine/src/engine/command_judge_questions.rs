//! The command guard's classification as two typed Choice questions.
//!
//! The Jev half of [`super::command_judge`]. Two things move out of the prompt
//! and into code here, and both are the point of the exercise.
//!
//! **The tie-break is a threshold.** ADR 0002 says pick the danger lane when
//! unsure, and pick irreversible when unsure between the danger lanes. The
//! rubric prompt states that as an instruction a model may or may not follow.
//! Given a distribution, [`read`] applies it in Rust, and a test pins it.
//!
//! **The category rides along speculatively.** It only matters for the
//! irreversible lane. Asking it in the same request costs one round trip
//! rather than two, and code throws it away when the lane came back safe.
//!
//! Everything here is pure, so the questions, the state and the thresholds are
//! tested without a network.

use serde_json::{json, Value};

use super::command_guard::{JudgeInput, RiskLane, SideEffectCategory};
use super::command_judge::{parse_category, JudgeVerdict};
use crate::llm::judgment::{Answers, ChoiceAnswer, Question};
use crate::llm::tool_names as tn;

pub(crate) const LANE: &str = "lane";
pub(crate) const CATEGORY: &str = "category";

const SAFE: &str = "safe";
const REVERSIBLE: &str = "reversible";
const IRREVERSIBLE: &str = "irreversible";

/// The probability at or above which the lane reads as safe.
///
/// Well above a simple majority, because the two mistakes are not symmetric. A
/// needless permission card costs the user one tap. A command waved through
/// costs them whatever it did, and some of those cannot be undone.
pub(crate) const SAFE_MIN_PROBABILITY: f64 = 0.7;

/// The probability at or above which a danger lane reads as reversible.
///
/// The same asymmetry one step down. Reversible means the guard snapshots and
/// runs, while irreversible means it asks. So an unsure danger lane asks.
pub(crate) const REVERSIBLE_MIN_PROBABILITY: f64 = 0.7;

/// The probability at or above which the side-effect category is taken as
/// answered.
///
/// **The category is not cosmetic.** An unattended trigger runs an
/// irreversible command only when its side-effect grant names that category
/// (ADR 0002). A low-confidence guess could therefore match a grant the user
/// gave for a different side effect. Below this floor the answer is
/// [`SideEffectCategory::Other`], which a trigger must grant outright.
///
/// Lower than the lane's bar, because five options split a distribution more
/// than three do. A majority on one category is the bar.
pub(crate) const CATEGORY_MIN_PROBABILITY: f64 = 0.5;

/// What the model reads, with the command redacted as the chat path redacts it.
///
/// **The redaction is not optional and not a detail.** This text leaves the
/// machine for a third party, and a `run_bash` body routinely carries a
/// database URL. A password is not a risk signal, so nothing about the verdict
/// depends on keeping it.
pub(crate) fn state(input: &JudgeInput) -> Value {
    let tool = match input.tool_name.as_str() {
        tn::RUN_PYTHON | tn::RUN_PYTHON_BACKGROUND => "Python code",
        _ => "Shell command",
    };
    json!({
        "tool": tool,
        "command": crate::core::redact_postgres_secrets(&input.command),
        "target_outside_workspace": input.out_of_workspace,
    })
}

/// The lane rubric, carried in the question because the id never reaches the
/// model.
const LANE_INSTRUCTIONS: &str = "Classify the risk of running `command` on the user's machine. \
     A personal assistant runs it inside one workspace directory. Reading anywhere on the \
     machine is normal and expected, and so is writing under the workspace. Reading or editing \
     a file OUTSIDE the workspace is a wanted feature, not a threat. Only destruction outside \
     the workspace is. `target_outside_workspace` is what static analysis saw, and it is a \
     signal rather than a verdict.";

const CATEGORY_INSTRUCTIONS: &str = "If `command` has an irreversible real-world side-effect, \
     which kind is it? Answer for the side-effect the command would have, even when you judge \
     the command harmless overall. Pick the single best fit.";

/// Both questions, asked together over one state.
pub(crate) fn questions() -> Vec<(String, Question)> {
    vec![
        (
            LANE.to_string(),
            Question::Choice {
                instructions: LANE_INSTRUCTIONS.to_string(),
                criteria: vec![
                    (
                        SAFE.to_string(),
                        Some(
                            "Reads, data crunching, builds and tests, downloads over HTTP GET, \
                             writes under the workspace, and reading or editing a file anywhere \
                             on the machine. Nothing irreversible, and no destruction outside \
                             the workspace."
                                .to_string(),
                        ),
                    ),
                    (
                        REVERSIBLE.to_string(),
                        Some(
                            "Destruction confined to the workspace, such as removing or \
                             overwriting files under it. Recoverable from version control."
                                .to_string(),
                        ),
                    ),
                    (
                        IRREVERSIBLE.to_string(),
                        Some(
                            "Either an irreversible real-world side-effect, such as sending a \
                             message, a mutating HTTP request, a cloud-service mutation, \
                             publishing, or spending money. Or destruction of content outside \
                             the workspace, such as removing or overwriting a system path, the \
                             home directory, or another repository."
                                .to_string(),
                        ),
                    ),
                ],
            },
        ),
        (
            CATEGORY.to_string(),
            Question::Choice {
                instructions: CATEGORY_INSTRUCTIONS.to_string(),
                criteria: vec![
                    (
                        "email".to_string(),
                        Some(
                            "Sending email or messages: mail, sendmail, AppleScript driving \
                             Mail or Messages, Python smtplib."
                                .to_string(),
                        ),
                    ),
                    (
                        "external_api".to_string(),
                        Some(
                            "A mutating outbound HTTP request: POST, PUT, DELETE or PATCH, or \
                             a data upload. Python requests or httpx writes count."
                                .to_string(),
                        ),
                    ),
                    (
                        "cloud_cli".to_string(),
                        Some("A cloud-service mutation through gh, aws or gcloud.".to_string()),
                    ),
                    (
                        "out_of_workspace_destruction".to_string(),
                        Some(
                            "Deleting or overwriting files outside the workspace directory."
                                .to_string(),
                        ),
                    ),
                    (
                        "other".to_string(),
                        Some(
                            "Any other irreversible side-effect, or none of these fits."
                                .to_string(),
                        ),
                    ),
                ],
            },
        ),
    ]
}

/// Apply ADR 0002's tie-break to the distribution.
///
/// Reads the probability of each harmless option rather than the model's
/// chosen one. An option the answer never mentioned reads as zero, so a
/// surprising answer shape can only ever push toward asking.
pub(crate) fn read_lane(answer: &ChoiceAnswer) -> RiskLane {
    if answer.probability(SAFE) >= SAFE_MIN_PROBABILITY {
        RiskLane::Safe
    } else if answer.probability(REVERSIBLE) >= REVERSIBLE_MIN_PROBABILITY {
        RiskLane::ReversibleDanger
    } else {
        RiskLane::IrreversibleDanger
    }
}

/// The side-effect category, or `Other` when the answer did not settle on one.
///
/// Reads the chosen option's own probability against
/// [`CATEGORY_MIN_PROBABILITY`]. An option the distribution never mentions
/// reads as zero, so a surprising answer shape lands on `Other` too.
pub(crate) fn read_category(answer: &ChoiceAnswer) -> SideEffectCategory {
    if answer.probability(&answer.choice) >= CATEGORY_MIN_PROBABILITY {
        parse_category(&answer.choice)
    } else {
        SideEffectCategory::Other
    }
}

/// Turn the answers into a verdict.
///
/// A missing lane answer is [`JudgeVerdict::uncertain`], the same *ask* the
/// chat path produces for a response it cannot read.
pub(crate) fn read(answers: &Answers) -> JudgeVerdict {
    let Some(lane_answer) = answers.choice(LANE) else {
        return JudgeVerdict::uncertain();
    };
    let lane = read_lane(lane_answer);
    let category = (lane == RiskLane::IrreversibleDanger).then(|| {
        answers
            .choice(CATEGORY)
            .map(read_category)
            .unwrap_or(SideEffectCategory::Other)
    });
    JudgeVerdict {
        lane,
        category,
        summary: summary(lane, category),
        reason: distribution(lane_answer),
    }
}

/// The card sentence, derived in code because Jev writes no prose.
///
/// The chat path keeps the model's own sentence. This one is built from the
/// lane and the category, so the same command always reads the same way.
fn summary(lane: RiskLane, category: Option<SideEffectCategory>) -> String {
    match lane {
        RiskLane::Safe => "Runs a command with no irreversible side-effect.".to_string(),
        RiskLane::ReversibleDanger => {
            "Deletes or overwrites files inside the workspace (recoverable).".to_string()
        }
        // `read_lane` never answers Catastrophic: that lane belongs to the
        // static pass alone. It shares this arm because both mean *ask*.
        RiskLane::Catastrophic | RiskLane::IrreversibleDanger => match category {
            Some(category) => format!("May perform {}.", category.reason()),
            None => "May cause an irreversible real-world side-effect.".to_string(),
        },
    }
}

/// The distribution, for the log. More useful than a model's prose, because it
/// says how close the verdict was to the other lanes.
fn distribution(answer: &ChoiceAnswer) -> String {
    format!(
        "jev: safe {:.2}, reversible {:.2}, irreversible {:.2} (confidence {:.2})",
        answer.probability(SAFE),
        answer.probability(REVERSIBLE),
        answer.probability(IRREVERSIBLE),
        answer.confidence,
    )
}

#[cfg(test)]
#[path = "command_judge_questions_tests.rs"]
mod tests;
