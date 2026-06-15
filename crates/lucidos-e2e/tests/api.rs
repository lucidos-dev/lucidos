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

#[path = "api_support/threads_list_test.rs"]
mod threads_list_test;

#[path = "api_support/filter_facets_test.rs"]
mod filter_facets_test;

#[path = "api_support/archived_count_test.rs"]
mod archived_count_test;

#[path = "api_support/sse_test.rs"]
mod sse_test;

#[path = "api_support/errors_test.rs"]
mod errors_test;

#[path = "api_support/changes_test.rs"]
mod changes_test;

#[path = "api_support/app_coding_agent_test.rs"]
mod app_coding_agent_test;

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

#[path = "api_support/blobs_test.rs"]
mod blobs_test;

#[path = "api_support/image_migration_test.rs"]
mod image_migration_test;

#[path = "api_support/load_knowhow_dedup_test.rs"]
mod load_knowhow_dedup_test;

#[path = "api_support/knowhow_read_test.rs"]
mod knowhow_read_test;

#[path = "api_support/context_capture_lazy_test.rs"]
mod context_capture_lazy_test;

#[path = "api_support/tool_result_lazy_test.rs"]
mod tool_result_lazy_test;

#[path = "api_support/trigger_groups_test.rs"]
mod trigger_groups_test;

#[path = "api_support/command_safety_test.rs"]
mod command_safety_test;

#[path = "api_support/cascade_archive_test.rs"]
mod cascade_archive_test;

#[path = "api_support/notifications_presence_test.rs"]
mod notifications_presence_test;

#[path = "api_support/credentials_test.rs"]
mod credentials_test;

#[path = "api_support/backup_key_test.rs"]
mod backup_key_test;
