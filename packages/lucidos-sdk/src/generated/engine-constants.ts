// AUTO-GENERATED. Do not edit by hand.
// Regenerate: cargo test -p lucidos-engine --lib generate_engine_constants_file -- --ignored
//
// Source of truth: the engine constant each export names, under
// crates/lucidos-engine/src.

/** `CLIENT_LOG_MAX_BATCH` in `api/internal.rs`. */
export const CLIENT_LOG_MAX_BATCH = 100;

/** `WIDGET_PARAMS_MAX_BYTES` in `engine/widgets.rs`. */
export const WIDGET_PARAMS_MAX_BYTES = 2048;

/** `MAX_IDS` in `api/threads/archive_all.rs`. */
export const ARCHIVE_ALL_MAX_IDS = 2000;

/** `SAME_FAMILY` in `engine/thread_triage/archive_all.rs`. */
export const ARCHIVE_ALL_SAME_FAMILY = "same_family";

/** `PINNED_SUB_THREAD` in `engine/thread_triage/archive_all.rs`. */
export const ARCHIVE_ALL_PINNED_SUB_THREAD = "pinned_sub_thread";

/** `SIDE_QUESTION_TIMEOUT` in `engine/agent_session/side_question.rs`. */
export const SIDE_QUESTION_TIMEOUT_MS = 120000;

/** `CLIENT_WAIT_SECS` in `api/proxy_timeout.rs`. */
export const PROXY_CLIENT_WAIT_SECS = 660;

/** `HEADER_DEVICE_ID` in `api/actor.rs`. */
export const HEADER_DEVICE_ID = "x-lucidos-device-id";

/** `BROAD_ALLOW_INEFFECTIVE` in `engine/claude_code/mod.rs`. */
export const BROAD_ALLOW_INEFFECTIVE = ["Edit", "ExitPlanMode", "NotebookEdit", "Write"] as const;

/** `CC_PROTECTED_PATH_MARKERS` in `engine/claude_code/mod.rs`. */
export const CC_PROTECTED_PATH_MARKERS = [".claude/", ".git/"] as const;

/** `SESSION_PATH_TOOLS` in `engine/claude_code/mod.rs`. */
export const SESSION_PATH_TOOLS = ["Edit", "Write", "NotebookEdit"] as const;

/** `CODEX_BACKEND_TOOLS` in `engine/claude_code/mod.rs`. */
export const CODEX_BACKEND_TOOLS = ["command_execution", "file_change"] as const;

/** `USER_CLICKED_CONTINUE_REASON` in `engine/agent_recovery/helpers.rs`. */
export const USER_CLICKED_CONTINUE_REASON = "user_clicked_continue";

/** `AUTO_RECOVERY_AFTER_HANG_REASON` in `engine/agent_recovery/helpers.rs`. */
export const AUTO_RECOVERY_AFTER_HANG_REASON = "auto_recovery_after_hang";

/** `AUTO_RESUME_AFTER_SWITCH_REASON` in `engine/agent_recovery/helpers.rs`. */
export const AUTO_RESUME_AFTER_SWITCH_REASON = "auto_resume_after_switch";

/** `AUTO_RESUME_AFTER_API_ERROR_REASON` in `engine/agent_recovery/helpers.rs`. */
export const AUTO_RESUME_AFTER_API_ERROR_REASON = "auto_resume_after_api_error";

/** `ENGINE_RESTART_INTERRUPT_REASON` in `engine/agent_recovery/helpers.rs`. */
export const ENGINE_RESTART_INTERRUPT_REASON = "engine_restart_interrupt";

/** `HARDEN_REQUESTED_REASON` in `engine/agent_recovery/helpers.rs`. */
export const HARDEN_REQUESTED_REASON = "harden_requested";

/** `TYPESAFE_API_KEY_ENV` in `llm/judgment/endpoint.rs`. */
export const TYPESAFE_API_KEY_ENV = "TYPESAFE_API_KEY";

/** `TYPESAFE_CREDENTIAL_SERVICE` in `llm/judgment/endpoint.rs`. */
export const TYPESAFE_CREDENTIAL_SERVICE = "typesafe";

/** `CLOUDFLARE_WORKERS_AI_CREDENTIAL_SERVICE` in `llm/judgment/endpoint.rs`. */
export const CLOUDFLARE_WORKERS_AI_CREDENTIAL_SERVICE = "cloudflare-workers-ai";

/** `SYSTEM_ONE_CUSTOM_CREDENTIAL_SERVICE` in `llm/judgment/endpoint.rs`. */
export const SYSTEM_ONE_CUSTOM_CREDENTIAL_SERVICE = "system-one-custom";

/** `CLOUDFLARE_ACCOUNTS_PREFIX` in `llm/judgment/endpoint.rs`. */
export const CLOUDFLARE_ACCOUNTS_PREFIX = "https://api.cloudflare.com/client/v4/accounts/";

/** `SystemOneEndpoint::id` in `llm/judgment/endpoint.rs`. */
export const SYSTEM_ONE_ENDPOINT_IDS = ["jev", "clef", "clef-flash", "custom"] as const;

/** `TakesEffect` in `runtime/agent_runtime.rs`. */
export const TAKES_EFFECT_VALUES = ["now", "next-turn"] as const;

/** `SECTIONS` in `voice/sections.rs`. */
export const VOICE_RESIDENT_SECTIONS = [
  { id: "who-and-where", title: "Who you are talking to, and when" },
  { id: "this-thread", title: "This conversation" },
  { id: "workspace-shape", title: "What this workspace has" },
] as const;
