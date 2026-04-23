mod apply_now;
mod io_helpers;
mod lifecycle;
mod parsing;
mod prompts;
mod resume;
mod run_session;
mod runtime_helpers;
mod spawn;

pub(crate) use prompts::build_merge_prompt;
pub(crate) use resume::change_description_fallback;

// Test-only re-exports — non-test callers reach these via super::* inside agent_session.
#[cfg(test)]
pub(crate) use resume::CC_TURN_CLOSER_EVENTS;
#[cfg(test)]
pub(crate) use spawn::generate_cc_branch_name;
