//! What a probe resolved to.
//!
//! Five values, and none of them names a mechanism (ADR 0110 decision 5). A
//! probe passed, or it failed in one of three ways, or it measured nothing.
//!
//! The three failures are kept apart because they are not equally bad. An agent
//! that asks costs a round. One that says out loud it no longer has the fact
//! costs a round and some trust. One that gets it wrong and says nothing is the
//! failure the whole benchmark exists to see.
//!
//! ADR 0087 split PASSING four ways as well, by the route that explained it:
//! still in the prompt, read back from notes, fetched by a recovery call, or
//! unattributable. That vocabulary is retired. Two of the four were arm labels
//! rather than results. The other two had the harness guessing which tool call
//! explained a pass, by matching a regex it wrote itself. The verbatim capture
//! answers the same question exactly, by showing the call.

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::config::Fact;

/// The outcome vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Outcome {
    /// The fact survived, whatever route kept it there.
    Pass,
    /// The agent asked the user instead of knowing. A visible failure.
    Asked,
    /// The agent said it no longer had the fact. A visible failure.
    LostLoud,
    /// Wrong, and silent about it. The failure that matters most.
    LostSilent,
    /// An upstream task failed, so this probe measured nothing.
    Void,
}

impl Outcome {
    pub fn is_pass(self) -> bool {
        self == Outcome::Pass
    }

    pub fn is_scored(self) -> bool {
        self != Outcome::Void
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Pass => "pass",
            Outcome::Asked => "asked",
            Outcome::LostLoud => "lost-loud",
            Outcome::LostSilent => "lost-silent",
            Outcome::Void => "void",
        }
    }
}

/// Everything the resolution reads.
pub struct ProbeInputs<'a> {
    /// Set when a task this probe depends on failed, timed out or was voided.
    pub upstream_failed: bool,
    /// Whether the probe's assertion, or its judge, said yes.
    pub passed: bool,
    /// `None` for the probes that measure behaviour rather than a fact. Those
    /// cannot resolve to `asked`, because there is no topic to have asked about.
    pub fact: Option<&'a Fact>,
    /// Question text from every `ask_user_question` on the thread.
    pub questions: &'a [String],
    /// Whether the judge found an explicit "I do not have" in the response.
    pub disclaimed: bool,
}

/// Resolve one probe. The first matching branch wins.
pub fn resolve(inputs: &ProbeInputs) -> Outcome {
    if inputs.upstream_failed {
        return Outcome::Void;
    }
    if inputs.passed {
        return Outcome::Pass;
    }
    if asked_about_the_fact(inputs) {
        return Outcome::Asked;
    }
    if inputs.disclaimed {
        return Outcome::LostLoud;
    }
    Outcome::LostSilent
}

fn asked_about_the_fact(inputs: &ProbeInputs) -> bool {
    let Some(fact) = inputs.fact else {
        return false;
    };
    let Ok(pattern) = Regex::new(&fact.topic_regex) else {
        return false;
    };
    inputs.questions.iter().any(|q| pattern.is_match(q))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Established;

    fn fact() -> Fact {
        Fact {
            id: "F16".into(),
            statement: "The cron is 0 6 * * *".into(),
            tier: 3,
            established_in: "T01".into(),
            established_by: Established::Prompt,
            census_regex: r"0 6 \* \* \*".into(),
            stated_regex: "(?i)cron|up at six".into(),
            topic_regex: "(?i)when should|what time|schedule".into(),
        }
    }

    struct Inputs {
        upstream_failed: bool,
        passed: bool,
        with_fact: bool,
        questions: Vec<String>,
        disclaimed: bool,
    }

    impl Default for Inputs {
        fn default() -> Self {
            Inputs {
                upstream_failed: false,
                passed: true,
                with_fact: true,
                questions: vec![],
                disclaimed: false,
            }
        }
    }

    fn run(inputs: Inputs) -> Outcome {
        let fact = fact();
        resolve(&ProbeInputs {
            upstream_failed: inputs.upstream_failed,
            passed: inputs.passed,
            fact: inputs.with_fact.then_some(&fact),
            questions: &inputs.questions,
            disclaimed: inputs.disclaimed,
        })
    }

    /// I3: a failed upstream task voids the probe rather than failing it.
    #[test]
    fn a_failed_upstream_task_voids_and_never_reads_as_lost_silent() {
        let outcome = run(Inputs {
            upstream_failed: true,
            passed: false,
            ..Inputs::default()
        });
        assert_eq!(outcome, Outcome::Void);
        assert!(!outcome.is_scored());
    }

    #[test]
    fn a_void_outranks_everything_including_a_pass() {
        let outcome = run(Inputs {
            upstream_failed: true,
            passed: true,
            ..Inputs::default()
        });
        assert_eq!(outcome, Outcome::Void);
    }

    /// The whole point of retiring the routes: a pass is a pass, and nothing
    /// about the configuration changes what it is called.
    #[test]
    fn a_pass_is_a_pass_whatever_kept_the_fact() {
        assert_eq!(run(Inputs::default()), Outcome::Pass);
        assert!(Outcome::Pass.is_pass());
    }

    #[test]
    fn a_failure_after_asking_about_the_fact_is_asked() {
        let outcome = run(Inputs {
            passed: false,
            questions: vec!["What time should this run?".into()],
            ..Inputs::default()
        });
        assert_eq!(outcome, Outcome::Asked);
    }

    #[test]
    fn a_question_about_something_else_does_not_excuse_the_failure() {
        let outcome = run(Inputs {
            passed: false,
            questions: vec!["Which colour should the chart be?".into()],
            ..Inputs::default()
        });
        assert_eq!(outcome, Outcome::LostSilent);
    }

    #[test]
    fn an_explicit_disclaimer_is_lost_loud() {
        let outcome = run(Inputs {
            passed: false,
            disclaimed: true,
            ..Inputs::default()
        });
        assert_eq!(outcome, Outcome::LostLoud);
    }

    /// A probe with no fact has no topic, so asking about something cannot
    /// excuse it. It still passes and fails like any other.
    #[test]
    fn a_probe_with_no_fact_still_passes_and_fails() {
        assert_eq!(
            run(Inputs {
                with_fact: false,
                ..Inputs::default()
            }),
            Outcome::Pass
        );
        assert_eq!(
            run(Inputs {
                with_fact: false,
                passed: false,
                questions: vec!["What time should this run?".into()],
                ..Inputs::default()
            }),
            Outcome::LostSilent
        );
    }

    #[test]
    fn exactly_one_value_counts_as_a_pass() {
        assert!(Outcome::Pass.is_pass());
        for outcome in [
            Outcome::Asked,
            Outcome::LostLoud,
            Outcome::LostSilent,
            Outcome::Void,
        ] {
            assert!(!outcome.is_pass(), "{} should not pass", outcome.as_str());
        }
    }
}
