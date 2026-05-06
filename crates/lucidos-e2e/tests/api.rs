//! E2E API integration tests for Lucidos.
//!
//! These tests hit a running Lucidos workspace booted by `./scripts/e2e-api.sh`.
//! Run via:
//!   ./scripts/e2e-api.sh
//! which boots the e2e-test workspace and runs `cargo test -p lucidos-e2e --test api`.

#[path = "api_support/mod.rs"]
mod support;

#[path = "api_support/health_test.rs"]
mod health_test;

#[path = "api_support/chat_test.rs"]
mod chat_test;

#[path = "api_support/threads_test.rs"]
mod threads_test;

#[path = "api_support/sse_test.rs"]
mod sse_test;

#[path = "api_support/errors_test.rs"]
mod errors_test;

#[path = "api_support/changes_test.rs"]
mod changes_test;

#[path = "api_support/cc_diff_test.rs"]
mod cc_diff_test;

#[path = "api_support/repo_files_test.rs"]
mod repo_files_test;

#[path = "api_support/file_edit_test.rs"]
mod file_edit_test;

#[path = "api_support/agent_question_test.rs"]
mod agent_question_test;

#[path = "api_support/lucidos_cli_test.rs"]
mod lucidos_cli_test;

#[path = "api_support/permission_prompt_test.rs"]
mod permission_prompt_test;

#[path = "api_support/ask_user_question_hook_test.rs"]
mod ask_user_question_hook_test;

#[path = "api_support/threads_compose_test.rs"]
mod threads_compose_test;

#[path = "api_support/client_log_test.rs"]
mod client_log_test;

#[path = "api_support/proxy_test.rs"]
mod proxy_test;
