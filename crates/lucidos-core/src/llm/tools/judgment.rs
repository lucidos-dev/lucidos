//! The LLM-facing schema for `judge`, a typed judgment the agent asks for
//! itself.
//!
//! Gated on [`crate::llm::tools::Gate::JudgmentProvider`], so a workspace with
//! no judgment provider is billed none of these bytes.
//!
//! **The description's job is to stop a fan-out.** One call carries every
//! question, because independent questions over one state run in parallel
//! upstream. The engine's own loop runs a round's tool calls one after another,
//! so splitting them costs a round trip and a tool-call slot each. The
//! parameter names are the wire's (`state`, `questions`), and the question
//! shape is `crate::llm::judgment::Question`, which deserializes from exactly
//! this.

use crate::llm::provider::ToolDefinition;
use crate::llm::tool_names as tn;
use serde_json::json;

pub(super) fn judgment_tools() -> Vec<ToolDefinition> {
    vec![ToolDefinition {
        name: tn::JUDGE.to_string(),
        description: "Ask for typed judgments about one state. Each answer is a probability, not prose. \
A 'noul' question is yes/no and returns the probability of yes. A 'choice' question picks one option from a closed set, and returns a probability for every option plus a confidence.\n\n\
PUT EVERY QUESTION IN ONE CALL. Independent questions over one state are answered in parallel upstream, so one call asking 50 questions is a single round trip. Fifty separate calls are fifty round trips, run one after another, each spending a tool-call slot. To score many items, put them all in `state` (e.g. {\"items\": [...]}) and ask one question per item.\n\n\
Reach for it when you need a calibrated number, when you are ranking or filtering many things at once, or as a cheap pre-filter in front of expensive work. Do not reach for it for a judgment you can simply make yourself: that costs a round trip and tells you nothing you did not know.\n\n\
The state is sent to TypeSafe, so leave out anything that should not go there.".to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "state": {
                    "type": "object",
                    "description": "The thing being judged, as JSON. Every question is asked about this one object, so batch related items into it rather than calling once per item."
                },
                "questions": {
                    "type": "object",
                    "description": "Question id to question. The id is yours and is never sent to the model, so the meaning has to be complete inside `instructions` and `criteria`. Answers come back keyed by these ids.",
                    "additionalProperties": {
                        "type": "object",
                        "properties": {
                            "type": {
                                "type": "string",
                                "enum": ["noul", "choice"],
                                "description": "'noul' for yes/no, 'choice' for one option from a closed set."
                            },
                            "instructions": {
                                "type": "string",
                                "description": "The judgment to make, written as an instruction."
                            },
                            "criteria": {
                                "type": "object",
                                "description": "For 'noul', optional, with the keys \"true\" and \"false\" saying what a yes and a no mean. For 'choice', required: each option name mapped to its rubric text, or to null where the name speaks for itself."
                            }
                        },
                        "required": ["type", "instructions"]
                    }
                }
            },
            "required": ["state", "questions"]
        }),
    }]
}
