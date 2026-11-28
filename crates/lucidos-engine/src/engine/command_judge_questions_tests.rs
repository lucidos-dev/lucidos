use std::collections::HashMap;

use super::*;
use crate::llm::judgment::{Answer, ChoiceAnswer};

fn input(command: &str, out_of_workspace: bool) -> JudgeInput {
    JudgeInput {
        tool_name: tn::RUN_BASH.to_string(),
        command: command.to_string(),
        out_of_workspace,
        fast_path_refused: false,
        script_sources: vec![],
    }
}

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

fn with_category(answers: Answers, category: &str) -> Answers {
    with_category_at(&answers, category, 0.9)
}

const PLAIN: &str = "ls";
const SENDS_MAIL: &str = "sendmail -t < x";

const READ_ONLY_FETCH: &str = "import urllib.request\n\
     req = urllib.request.Request('https://docs.example.com/pricing', headers={'User-Agent': 'x'})\n\
     html = urllib.request.urlopen(req, timeout=20).read().decode()";

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
fn both_questions_say_where_the_command_runs() {
    let questions = questions();
    let Question::Choice {
        instructions: lane, ..
    } = &questions[0].1
    else {
        panic!("the lane is a choice");
    };
    for phrase in [
        "workspace root",
        "`data/`",
        "`.lucidos/`",
        "`lucidos data write`",
        "`target_outside_workspace` is true",
        "False means",
    ] {
        assert!(lane.contains(phrase), "{phrase}: {lane}");
    }
    let Question::Choice {
        instructions: category,
        ..
    } = &questions[1].1
    else {
        panic!("the category is a choice");
    };
    assert!(
        category.contains("under the workspace is not out_of_workspace_destruction"),
        "{category}"
    );
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
            SideEffectCategory::Other,
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

#[test]
fn the_combined_threshold_decides_whether_to_ask() {
    let threshold = SAFE_MIN_PROBABILITY;
    for (safe, reversible, expected) in [
        (threshold - 0.2, 0.1, RiskLane::IrreversibleDanger),
        (0.0, threshold, RiskLane::ReversibleDanger),
        (threshold - 0.25, 0.25, RiskLane::ReversibleDanger),
        (threshold - 0.25, 0.3, RiskLane::ReversibleDanger),
        (threshold, 0.0, RiskLane::Safe),
        (threshold + 0.1, 0.0, RiskLane::Safe),
    ] {
        assert_eq!(read_lane_of(lanes(safe, reversible, 0.0)), expected);
    }
}

#[test]
fn empty_and_malformed_lane_distributions_ask() {
    for probabilities in [
        HashMap::new(),
        HashMap::from([(SAFE.to_string(), f64::NAN)]),
        HashMap::from([(SAFE.to_string(), f64::INFINITY)]),
        HashMap::from([(SAFE.to_string(), -1.0)]),
        HashMap::from([(SAFE.to_string(), 2.0)]),
        HashMap::from([(SAFE.to_string(), 0.9), (REVERSIBLE.to_string(), 0.9)]),
        HashMap::from([(SAFE.to_string(), 1.0), ("unknown".to_string(), 0.0)]),
    ] {
        assert_eq!(
            read_lane(&ChoiceAnswer {
                choice: SAFE.to_string(),
                probabilities,
                confidence: 1.0,
            }),
            RiskLane::IrreversibleDanger
        );
    }
}

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
    assert_eq!(read(&answers, PLAIN).lane, RiskLane::IrreversibleDanger);
}

#[test]
fn a_missing_lane_answer_is_the_uncertain_verdict() {
    let verdict = read(&Answers::default(), PLAIN);
    assert_eq!(verdict, JudgeVerdict::uncertain());
    assert_eq!(verdict.lane, RiskLane::IrreversibleDanger);
    assert_eq!(verdict.category, Some(SideEffectCategory::Other));
}

#[test]
fn the_category_is_read_only_for_the_irreversible_lane() {
    let irreversible = read(&with_category(lanes(0.05, 0.05, 0.9), "email"), SENDS_MAIL);
    assert_eq!(irreversible.category, Some(SideEffectCategory::Email));

    let safe = read(&with_category(lanes(0.95, 0.03, 0.02), "email"), SENDS_MAIL);
    assert_eq!(
        safe.category, None,
        "the speculative answer is thrown away when the lane came back safe"
    );
}

#[test]
fn an_irreversible_lane_with_no_category_answer_falls_to_other() {
    let verdict = read(&lanes(0.05, 0.05, 0.9), PLAIN);
    assert_eq!(verdict.category, Some(SideEffectCategory::Other));
}

#[test]
fn an_unsure_category_falls_to_other() {
    let irreversible = lanes(0.05, 0.05, 0.9);
    assert_eq!(
        read(&with_category_at(&irreversible, "cloud_cli", 0.49), PLAIN).category,
        Some(SideEffectCategory::Other),
        "below the floor, a granted cloud_cli must not be matched"
    );
    assert_eq!(
        read(&with_category_at(&irreversible, "cloud_cli", 0.5), PLAIN).category,
        Some(SideEffectCategory::CloudCli),
        "at the floor it is taken"
    );
}

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
    assert_eq!(
        read(&answers, SENDS_MAIL).category,
        Some(SideEffectCategory::Other)
    );
}

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
        let verdict = read(&answers, SENDS_MAIL);
        assert!(
            !verdict.summary.trim().is_empty() && verdict.summary.ends_with('.'),
            "{:?} / {:?} produced {:?}",
            verdict.lane,
            verdict.category,
            verdict.summary
        );
    }
}

#[test]
fn two_categories_produce_two_different_sentences() {
    let email = read(&with_category(lanes(0.05, 0.05, 0.9), "email"), SENDS_MAIL).summary;
    let cloud = read(&with_category(lanes(0.05, 0.05, 0.9), "cloud_cli"), PLAIN).summary;
    assert_ne!(email, cloud);
}

#[test]
fn the_reason_records_the_distribution() {
    let verdict = read(&lanes(0.1, 0.2, 0.7), PLAIN);
    assert!(verdict.reason.contains("safe 0.10"), "{}", verdict.reason);
    assert!(
        verdict.reason.contains("irreversible 0.70"),
        "{}",
        verdict.reason
    );
}

#[test]
fn an_email_answer_without_a_messaging_signal_falls_to_other() {
    let verdict = read(
        &with_category(lanes(0.05, 0.05, 0.9), "email"),
        READ_ONLY_FETCH,
    );
    assert_eq!(verdict.category, Some(SideEffectCategory::Other));
    assert_eq!(verdict.lane, RiskLane::IrreversibleDanger);
    assert!(
        !verdict.summary.contains("email"),
        "the card must not name email: {}",
        verdict.summary
    );
}

#[test]
fn a_command_that_sends_a_message_still_reads_as_email() {
    for command in [
        "import smtplib; smtplib.SMTP('localhost').sendmail(a, b, m)",
        "sendmail -t < x",
        "echo hi | mail -s subj a@b.com",
        "osascript -e 'tell application \"Messages\" to send'",
    ] {
        let verdict = read(&with_category(lanes(0.05, 0.05, 0.9), "email"), command);
        assert_eq!(
            verdict.category,
            Some(SideEffectCategory::Email),
            "{command}"
        );
    }
}

#[test]
fn a_none_answer_reads_as_other() {
    assert_eq!(parse_category("none"), SideEffectCategory::Other);
    let verdict = read(
        &with_category(lanes(0.05, 0.05, 0.9), "none"),
        READ_ONLY_FETCH,
    );
    assert_eq!(verdict.category, Some(SideEffectCategory::Other));
}

#[test]
fn the_category_question_lists_none_first() {
    let questions = questions();
    let Question::Choice { criteria, .. } = &questions[1].1 else {
        panic!("the category is a choice");
    };
    assert_eq!(criteria[0].0, "none");
}

fn read_lane_of(answers: Answers) -> RiskLane {
    read(&answers, PLAIN).lane
}
