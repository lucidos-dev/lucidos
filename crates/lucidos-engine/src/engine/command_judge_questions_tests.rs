//! The tie-break, the redaction, and the card sentence.

use std::collections::HashMap;

use super::*;
use crate::llm::judgment::{Answer, ChoiceAnswer};

fn input(command: &str, out_of_workspace: bool) -> JudgeInput {
    JudgeInput {
        tool_name: tn::RUN_BASH.to_string(),
        command: command.to_string(),
        out_of_workspace,
        fast_path_refused: false,
    }
}

/// Build a lane answer from its distribution. `choice` is set to the highest
/// option, which is what the wire does, so a test cannot accidentally rely on
/// it: [`read_lane`] never reads it.
fn lanes(safe: f64, reversible: f64, irreversible: f64) -> Answers {
    let probabilities = HashMap::from([
        (SAFE.to_string(), safe),
        (REVERSIBLE.to_string(), reversible),
        (IRREVERSIBLE.to_string(), irreversible),
    ]);
    let choice = probabilities
        .iter()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .map(|(k, _)| k.clone())
        .unwrap_or_default();
    Answers::new(HashMap::from([(
        LANE.to_string(),
        Answer::Choice(ChoiceAnswer {
            choice,
            probabilities,
            confidence: 0.5,
        }),
    )]))
}

/// Add a category answer beside an existing lane one, at `probability`.
fn with_category_at(answers: &Answers, category: &str, probability: f64) -> Answers {
    let mut map = HashMap::new();
    if let Some(lane) = answers.choice(LANE) {
        map.insert(LANE.to_string(), Answer::Choice(lane.clone()));
    }
    map.insert(
        CATEGORY.to_string(),
        Answer::Choice(ChoiceAnswer {
            choice: category.to_string(),
            probabilities: HashMap::from([(category.to_string(), probability)]),
            confidence: probability,
        }),
    );
    Answers::new(map)
}

/// The same, at a probability that clears [`CATEGORY_MIN_PROBABILITY`].
fn with_category(answers: Answers, category: &str) -> Answers {
    with_category_at(&answers, category, 0.9)
}

/// The invariant this whole module exists for. The command text leaves the
/// machine for a third party, so the password goes first.
#[test]
fn the_state_carries_a_redacted_command() {
    let state = state(&input(
        "psql postgresql://lucidos:hunter2@db.example.com:5432/app -c 'DROP TABLE events'",
        false,
    ));
    let serialized = serde_json::to_string(&state).expect("a serializable state");
    assert!(
        !serialized.contains("hunter2"),
        "the password must not reach TypeSafe: {serialized}"
    );
    assert!(
        serialized.contains("DROP TABLE events"),
        "the rest of the command is what gets classified: {serialized}"
    );
}

#[test]
fn the_state_names_the_tool_and_the_static_signal() {
    let shell = state(&input("ls", false));
    assert_eq!(shell["tool"], "Shell command");
    assert_eq!(shell["target_outside_workspace"], false);

    let mut python = input("open('/etc/hosts')", true);
    python.tool_name = tn::RUN_PYTHON.to_string();
    let python = state(&python);
    assert_eq!(python["tool"], "Python code");
    assert_eq!(python["target_outside_workspace"], true);
}

#[test]
fn both_questions_travel_together_as_choices() {
    let questions = questions();
    assert_eq!(questions.len(), 2, "one request, two questions");
    let ids: Vec<&str> = questions.iter().map(|(id, _)| id.as_str()).collect();
    assert_eq!(ids, vec![LANE, CATEGORY]);
    for (id, question) in &questions {
        assert!(matches!(question, Question::Choice { .. }), "{id}");
    }
}

/// `Catastrophic` is the static pass's exclusive answer, and the judge has
/// never been able to return it. Offering it as an option would change that.
#[test]
fn the_lane_question_offers_three_options_and_no_catastrophic() {
    let questions = questions();
    let Question::Choice { criteria, .. } = &questions[0].1 else {
        panic!("the lane is a choice");
    };
    let names: Vec<&str> = criteria.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, vec![SAFE, REVERSIBLE, IRREVERSIBLE]);
    for (name, description) in criteria {
        assert!(description.is_some(), "{name} carries a rubric");
    }
}

#[test]
fn every_category_option_matches_a_real_variant() {
    let questions = questions();
    let Question::Choice { criteria, .. } = &questions[1].1 else {
        panic!("the category is a choice");
    };
    let parsed: Vec<SideEffectCategory> = criteria.iter().map(|(n, _)| parse_category(n)).collect();
    assert_eq!(
        parsed,
        vec![
            SideEffectCategory::Email,
            SideEffectCategory::ExternalApi,
            SideEffectCategory::CloudCli,
            SideEffectCategory::OutOfWorkspaceDestruction,
            SideEffectCategory::Other,
        ],
        "an option name no variant parses would silently become Other"
    );
}

#[test]
fn a_confident_answer_takes_its_own_lane() {
    assert_eq!(read_lane_of(lanes(0.95, 0.03, 0.02)), RiskLane::Safe);
    assert_eq!(
        read_lane_of(lanes(0.05, 0.90, 0.05)),
        RiskLane::ReversibleDanger
    );
    assert_eq!(
        read_lane_of(lanes(0.05, 0.05, 0.90)),
        RiskLane::IrreversibleDanger
    );
}

/// ADR 0002's first tie-break, now in code. A command the model leans toward
/// safe, but not confidently, still asks.
#[test]
fn an_unsure_safe_answer_asks() {
    assert_eq!(
        read_lane_of(lanes(0.6, 0.2, 0.2)),
        RiskLane::IrreversibleDanger,
        "leading is not enough, the bar is the threshold"
    );
    assert_eq!(read_lane_of(lanes(0.7, 0.2, 0.1)), RiskLane::Safe, "at it");
}

/// ADR 0002's second tie-break. Split between the two danger lanes means ask,
/// never snapshot-and-run.
#[test]
fn an_unsure_danger_answer_is_irreversible() {
    assert_eq!(
        read_lane_of(lanes(0.1, 0.5, 0.4)),
        RiskLane::IrreversibleDanger
    );
    assert_eq!(
        read_lane_of(lanes(0.1, 0.7, 0.2)),
        RiskLane::ReversibleDanger
    );
}

/// A distribution naming options nobody asked for reads as all-zero, so it
/// lands on ask rather than on whatever the model happened to pick.
#[test]
fn an_unrecognized_distribution_asks() {
    let answers = Answers::new(HashMap::from([(
        LANE.to_string(),
        Answer::Choice(ChoiceAnswer {
            choice: "totally_fine".to_string(),
            probabilities: HashMap::from([("totally_fine".to_string(), 1.0)]),
            confidence: 1.0,
        }),
    )]));
    assert_eq!(read(&answers).lane, RiskLane::IrreversibleDanger);
}

#[test]
fn a_missing_lane_answer_is_the_uncertain_verdict() {
    let verdict = read(&Answers::default());
    assert_eq!(verdict, JudgeVerdict::uncertain());
    assert_eq!(verdict.lane, RiskLane::IrreversibleDanger);
    assert_eq!(verdict.category, Some(SideEffectCategory::Other));
}

#[test]
fn the_category_is_read_only_for_the_irreversible_lane() {
    let irreversible = read(&with_category(lanes(0.05, 0.05, 0.9), "email"));
    assert_eq!(irreversible.category, Some(SideEffectCategory::Email));

    let safe = read(&with_category(lanes(0.95, 0.03, 0.02), "email"));
    assert_eq!(
        safe.category, None,
        "the speculative answer is thrown away when the lane came back safe"
    );
}

#[test]
fn an_irreversible_lane_with_no_category_answer_falls_to_other() {
    let verdict = read(&lanes(0.05, 0.05, 0.9));
    assert_eq!(verdict.category, Some(SideEffectCategory::Other));
}

/// An unattended trigger runs an irreversible command only when its grant
/// names the category. So an unsure category must not name one the user
/// happens to have granted for a different side effect.
#[test]
fn an_unsure_category_falls_to_other() {
    let irreversible = lanes(0.05, 0.05, 0.9);
    assert_eq!(
        read(&with_category_at(&irreversible, "cloud_cli", 0.49)).category,
        Some(SideEffectCategory::Other),
        "below the floor, a granted cloud_cli must not be matched"
    );
    assert_eq!(
        read(&with_category_at(&irreversible, "cloud_cli", 0.5)).category,
        Some(SideEffectCategory::CloudCli),
        "at the floor it is taken"
    );
}

/// The same all-zero reading the lane gets. A category the distribution never
/// mentions cannot clear the floor.
#[test]
fn a_category_missing_from_its_own_distribution_falls_to_other() {
    let answers = Answers::new(HashMap::from([
        (
            LANE.to_string(),
            Answer::Choice(ChoiceAnswer {
                choice: IRREVERSIBLE.to_string(),
                probabilities: HashMap::from([(IRREVERSIBLE.to_string(), 1.0)]),
                confidence: 1.0,
            }),
        ),
        (
            CATEGORY.to_string(),
            Answer::Choice(ChoiceAnswer {
                choice: "email".to_string(),
                probabilities: HashMap::new(),
                confidence: 1.0,
            }),
        ),
    ]));
    assert_eq!(read(&answers).category, Some(SideEffectCategory::Other));
}

/// Jev writes no prose, so every lane and category pair has to produce a
/// sentence here. A blank card is worse than a general one.
#[test]
fn every_verdict_carries_a_card_sentence() {
    for (probabilities, category) in [
        (lanes(0.95, 0.03, 0.02), None),
        (lanes(0.05, 0.9, 0.05), None),
        (lanes(0.05, 0.05, 0.9), Some("email")),
        (lanes(0.05, 0.05, 0.9), Some("external_api")),
        (lanes(0.05, 0.05, 0.9), Some("cloud_cli")),
        (lanes(0.05, 0.05, 0.9), Some("out_of_workspace_destruction")),
        (lanes(0.05, 0.05, 0.9), Some("other")),
    ] {
        let answers = match category {
            Some(c) => with_category(probabilities, c),
            None => probabilities,
        };
        let verdict = read(&answers);
        assert!(
            !verdict.summary.trim().is_empty() && verdict.summary.ends_with('.'),
            "{:?} / {:?} produced {:?}",
            verdict.lane,
            verdict.category,
            verdict.summary
        );
    }
}

/// Two irreversible commands of different kinds must not read the same, or the
/// card stops telling the user anything.
#[test]
fn two_categories_produce_two_different_sentences() {
    let email = read(&with_category(lanes(0.05, 0.05, 0.9), "email")).summary;
    let cloud = read(&with_category(lanes(0.05, 0.05, 0.9), "cloud_cli")).summary;
    assert_ne!(email, cloud);
}

/// The logged reason carries the distribution rather than model prose, so a
/// reader can see how close the verdict was.
#[test]
fn the_reason_records_the_distribution() {
    let verdict = read(&lanes(0.1, 0.2, 0.7));
    assert!(verdict.reason.contains("safe 0.10"), "{}", verdict.reason);
    assert!(
        verdict.reason.contains("irreversible 0.70"),
        "{}",
        verdict.reason
    );
}

fn read_lane_of(answers: Answers) -> RiskLane {
    read(&answers).lane
}
