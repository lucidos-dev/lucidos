mod helpers;
pub(crate) mod run;

pub(crate) use helpers::*;

#[path = "../agentic_loop_special_tool.rs"]
mod special_tool;

#[cfg(test)]
use crate::engine::agentic_loop::special_tool::SpecialToolDispatchExt;

#[cfg(test)]
pub(crate) use special_tool::sub_thread_route_overrides;

#[cfg(test)]
#[path = "../agentic_loop_unit_tests/flush.rs"]
mod flush_unit_tests;

#[cfg(test)]
#[path = "../agentic_loop_unit_tests/call_key.rs"]
mod call_key_unit_tests;

#[cfg(test)]
#[path = "../agentic_loop_unit_tests/serialize.rs"]
mod serialize_unit_tests;

#[cfg(test)]
#[path = "../agentic_loop_unit_tests/flush_text.rs"]
mod flush_text_unit_tests;

#[cfg(test)]
#[path = "../agentic_loop_unit_tests/sentinel.rs"]
mod sentinel_unit_tests;

#[cfg(test)]
#[path = "../agentic_loop_unit_tests/capture.rs"]
mod capture_unit_tests;

#[cfg(test)]
#[path = "../agentic_loop_unit_tests/injection_coalescing.rs"]
mod injection_coalescing_unit_tests;

#[cfg(test)]
#[path = "../agentic_loop_unit_tests/spoken_aside.rs"]
mod spoken_aside_unit_tests;

#[cfg(test)]
#[path = "../agentic_loop_unit_tests/question_reask.rs"]
mod question_reask_unit_tests;

#[cfg(test)]
#[path = "../agentic_loop_unit_tests/question_card.rs"]
mod question_card_unit_tests;

#[cfg(test)]
#[path = "../agentic_loop_unit_tests/todo_wake_nudge.rs"]
mod todo_wake_nudge_unit_tests;

#[cfg(test)]
#[path = "../agentic_loop_unit_tests/failure_breaker.rs"]
mod failure_breaker_unit_tests;

#[cfg(test)]
#[path = "../agentic_loop_unit_tests/image_pins.rs"]
mod image_pins_unit_tests;

#[cfg(test)]
#[path = "../agentic_loop_unit_tests/tool_result_split.rs"]
mod tool_result_split_unit_tests;

#[cfg(test)]
#[path = "../agentic_loop_unit_tests/turn_tools.rs"]
mod turn_tools_unit_tests;

#[cfg(test)]
#[path = "../agentic_loop_unit_tests/parallel_reads.rs"]
mod parallel_reads_unit_tests;

#[cfg(test)]
#[path = "../agentic_loop_unit_tests/turn_end_decisions.rs"]
mod turn_end_decisions_unit_tests;

#[cfg(test)]
#[path = "../agentic_loop_tests.rs"]
mod agentic_loop_db_tests;

#[cfg(test)]
pub(crate) async fn ask_prepared_batch_for_test(
    engine: &crate::engine::LucidosEngine,
    thread: uuid::Uuid,
    tool: &str,
    id: &str,
) -> String {
    engine
        .handle_chat_ask_user_question(
            thread,
            tool,
            &serde_json::json!({"questions": [{"question": id}]}),
            &crate::engine::thread_events::EventMeta::NONE,
        )
        .await
        .result
}
