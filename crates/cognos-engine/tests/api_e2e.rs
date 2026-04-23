//! E2E API integration tests for CognOS.
//!
//! These tests hit a running CognOS workspace and are marked `#[ignore]` so a
//! bare `cargo test -p cognos-engine` passes without infra. Run via:
//!   ./scripts/e2e-api.sh
//! which boots the e2e-test workspace and passes `--ignored` to libtest.

#[path = "api_e2e_support/mod.rs"]
mod support;

#[path = "api_e2e_support/health_test.rs"]
mod health_test;

#[path = "api_e2e_support/chat_test.rs"]
mod chat_test;

#[path = "api_e2e_support/threads_test.rs"]
mod threads_test;

#[path = "api_e2e_support/sse_test.rs"]
mod sse_test;

#[path = "api_e2e_support/errors_test.rs"]
mod errors_test;

#[path = "api_e2e_support/changes_test.rs"]
mod changes_test;

#[path = "api_e2e_support/repo_files_test.rs"]
mod repo_files_test;

#[path = "api_e2e_support/file_edit_test.rs"]
mod file_edit_test;

#[path = "api_e2e_support/agent_question_test.rs"]
mod agent_question_test;

#[path = "api_e2e_support/cognos_cli_test.rs"]
mod cognos_cli_test;

#[path = "api_e2e_support/permission_prompt_test.rs"]
mod permission_prompt_test;
