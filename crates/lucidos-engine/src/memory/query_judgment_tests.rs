//! The three questions, the state they read, and the thresholding.

use std::collections::HashMap;

use super::*;
use crate::llm::judgment::{Answer, NoulAnswer};

fn answered(pairs: &[(&str, f64)]) -> Answers {
    Answers::new(
        pairs
            .iter()
            .map(|(id, p)| (id.to_string(), Answer::Noul(NoulAnswer { noul: *p })))
            .collect::<HashMap<_, _>>(),
    )
}

#[test]
fn the_three_questions_travel_together_as_nouls() {
    let questions = questions();
    assert_eq!(questions.len(), 3, "one request answers all three");
    let ids: Vec<&str> = questions.iter().map(|(id, _)| id.as_str()).collect();
    assert_eq!(ids, vec![NEEDS_MEMORY, NEEDS_FILE_LIST, NEEDS_CREDENTIALS]);
    for (id, question) in &questions {
        assert!(
            matches!(question, Question::Noul { .. }),
            "{id} is a yes/no judgment"
        );
    }
}

/// The id is never sent to the model, so every question has to carry its own
/// meaning. A bare id like `needs_memory` in the instructions would not.
#[test]
fn every_question_states_its_meaning_without_relying_on_its_id() {
    for (id, question) in questions() {
        let Question::Noul {
            instructions,
            criteria,
        } = question
        else {
            panic!("{id} should be a noul");
        };
        assert!(instructions.len() > 40, "{id} has real instructions");
        assert!(!instructions.contains(&id), "{id} does not lean on its id");
        let criteria = criteria.unwrap_or_else(|| panic!("{id} defines yes and no"));
        assert!(!criteria.yes.is_empty() && !criteria.no.is_empty());
    }
}

#[test]
fn the_state_names_the_message_and_omits_an_empty_conversation() {
    assert_eq!(
        state("what did we decide?", None),
        serde_json::json!({ "message": "what did we decide?" })
    );
    assert_eq!(
        state("try again", Some("")),
        serde_json::json!({ "message": "try again" }),
        "an empty summary is no context at all"
    );
    assert_eq!(
        state("try again", Some("about the API token")),
        serde_json::json!({ "message": "try again", "conversation": "about the API token" })
    );
}

#[test]
fn a_confident_no_skips_and_a_confident_yes_loads() {
    let answers = answered(&[
        (NEEDS_MEMORY, 0.95),
        (NEEDS_FILE_LIST, 0.02),
        (NEEDS_CREDENTIALS, 0.01),
    ]);
    let read = read(&answers);
    assert!(read.needs_memory);
    assert!(!read.needs_file_list);
    assert!(!read.needs_credentials);
    assert!(read.sub_queries.is_empty(), "the caller fills these");
}

/// The tie goes to loading. A split answer costs a little latency; a skipped
/// retrieval costs the user an answer that ignores what they asked about.
#[test]
fn a_split_answer_loads() {
    let read = read(&answered(&[
        (NEEDS_MEMORY, 0.5),
        (NEEDS_FILE_LIST, 0.36),
        (NEEDS_CREDENTIALS, 0.34),
    ]));
    assert!(read.needs_memory);
    assert!(read.needs_file_list, "0.36 is at or above the threshold");
    assert!(
        !read.needs_credentials,
        "0.34 is below it, so a leaning-no answer still skips"
    );
}

/// An answer that never arrived must not silently starve the turn of context.
#[test]
fn a_missing_answer_reads_as_yes() {
    assert_eq!(read(&Answers::default()), QueryClassification::default());
    let partial = read(&answered(&[(NEEDS_MEMORY, 0.01)]));
    assert!(!partial.needs_memory);
    assert!(partial.needs_file_list, "absent, so it loads");
    assert!(partial.needs_credentials, "absent, so it loads");
}

/// The threshold leaning below 0.5 is what `a_split_answer_loads` proves, at
/// the behavior rather than at the constant. A bare comparison of the constant
/// would assert the code against itself.
#[test]
fn an_exactly_neutral_answer_loads() {
    assert!(read(&answered(&[(NEEDS_MEMORY, 0.5)])).needs_memory);
}
