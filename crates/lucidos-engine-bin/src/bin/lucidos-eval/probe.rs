use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::config::Fact;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Outcome {
    Pass,
    Asked,
    LostLoud,
    LostSilent,
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

pub struct ProbeInputs<'a> {
    pub upstream_failed: bool,
    pub passed: bool,
    pub fact: Option<&'a Fact>,
    pub questions: &'a [String],
    pub disclaimed: bool,
}

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
