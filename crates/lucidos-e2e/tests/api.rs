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

#[path = "api_support/ws_echo_test.rs"]
mod ws_echo_test;

#[path = "api_support/changelog_test.rs"]
mod changelog_test;

#[path = "api_support/release_notices_test.rs"]
mod release_notices_test;

#[path = "api_support/fonts_test.rs"]
mod fonts_test;

#[path = "api_support/workspace_label_test.rs"]
mod workspace_label_test;

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

#[path = "api_support/data_range_test.rs"]
mod data_range_test;

#[path = "api_support/data_mount_test.rs"]
mod data_mount_test;

#[path = "api_support/event_wait_test.rs"]
mod event_wait_test;

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

#[path = "api_support/restart_intent_test.rs"]
mod restart_intent_test;

#[path = "api_support/handshake_approval_test.rs"]
mod handshake_approval_test;

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

#[path = "api_support/event_location_test.rs"]
mod event_location_test;

#[path = "api_support/snapshot_compression_test.rs"]
mod snapshot_compression_test;

#[path = "api_support/trigger_groups_test.rs"]
mod trigger_groups_test;

#[path = "api_support/script_trigger_in_place_test.rs"]
mod script_trigger_in_place_test;

#[path = "api_support/trigger_run_test.rs"]
mod trigger_run_test;

#[path = "api_support/trigger_event_type_test.rs"]
mod trigger_event_type_test;

#[path = "api_support/command_safety_test.rs"]
mod command_safety_test;

#[path = "api_support/cascade_archive_test.rs"]
mod cascade_archive_test;

#[path = "api_support/notifications_presence_test.rs"]
mod notifications_presence_test;

#[path = "api_support/credentials_test.rs"]
mod credentials_test;

#[path = "api_support/oauth_connect_test.rs"]
mod oauth_connect_test;

#[path = "api_support/models_test.rs"]
mod models_test;

#[path = "api_support/backup_key_test.rs"]
mod backup_key_test;

#[path = "api_support/backup_schedule_test.rs"]
mod backup_schedule_test;

#[path = "api_support/backup_last_successful_test.rs"]
mod backup_last_successful_test;

#[path = "api_support/network_config_test.rs"]
mod network_config_test;

#[path = "api_support/tailnet_status_test.rs"]
mod tailnet_status_test;

#[path = "api_support/embedding_model_status_test.rs"]
mod embedding_model_status_test;

#[path = "api_support/follow_up_test.rs"]
mod follow_up_test;

#[path = "api_support/coding_agent_binaries_test.rs"]
mod coding_agent_binaries_test;

#[path = "api_support/frontend_preview_test.rs"]
mod frontend_preview_test;

#[path = "api_support/mcp_servers_test.rs"]
mod mcp_servers_test;

#[path = "api_support/webhook_delivery_test.rs"]
mod webhook_delivery_test;

#[path = "api_support/webhook_ingress_test.rs"]
mod webhook_ingress_test;
