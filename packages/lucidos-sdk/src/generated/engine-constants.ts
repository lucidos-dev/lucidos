// AUTO-GENERATED. Do not edit by hand.
// Regenerate: ./scripts/codegen.sh
//
// Source of truth: the engine constant each export names, under
// crates/lucidos-engine/src.

/** `CLIENT_LOG_MAX_BATCH` in `api/internal.rs`. */
export const CLIENT_LOG_MAX_BATCH = 100;

/** `SSE_KEEPALIVE_INTERVAL` in `api/history.rs`. */
export const SSE_KEEPALIVE_INTERVAL_MS = 30000;

/** `SSE_KEEPALIVE_EVENT` in `api/history.rs`. */
export const SSE_KEEPALIVE_EVENT = "keepalive";

/** `WIDGET_PARAMS_MAX_BYTES` in `engine/widgets.rs`. */
export const WIDGET_PARAMS_MAX_BYTES = 2048;

/** `WidgetSize::ALL` in `core/apps.rs`. */
export const WIDGET_SIZES = ["small", "medium", "large"] as const;

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

/** `MEMORY_MODULE_WHILE_UNSET` in `core/preference_catalog.rs`. */
export const MEMORY_MODULE_WHILE_UNSET = "classic" as const;

/** `SOURCE_COMMIT_FILE` in `api/frontend_snapshot.rs`. */
export const SOURCE_COMMIT_FILE = "source-commit";

/** `TURN_RUNNING_REASON` in `engine/chat/rung.rs`. */
export const TURN_RUNNING_REASON = "turn-running";

/** `GUARD_SHELLS` in `engine/command_guard.rs`. */
export const GUARD_SHELLS = ["sh", "bash", "zsh", "dash", "ksh", "ash"] as const;

/** `BENIGN_PREFIX_WORDS` in `engine/command_guard.rs`. */
export const BENIGN_PREFIX_WORDS = ["sudo", "env", "command", "time", "nice", "builtin", "exec", "\\"] as const;

/** `GRANTS_NOTHING_HEADS` in `engine/command_guard.rs`. */
export const GRANTS_NOTHING_HEADS = ["sh", "bash", "zsh", "dash", "ksh", "ash", "arch", "nohup", "setsid", "caffeinate", "ionice", "stdbuf", "unbuffer", "eatmydata", "doas", "pkexec", "fakeroot", "unshare", "nsenter", "proot", "systemd-run", "strace", "ltrace", "dtruss", "watch", "parallel", "xargs", "busybox", "toybox", "gstdbuf", "gnice", "gxargs", "gionice", "timeout", "gtimeout", "chroot", "setarch", "taskset", "chrt", "su", "runuser", "script", "flock", "sudo", "env", "command", "time", "nice", "builtin", "exec", "\\", "eval", "source", "."] as const;

/** `MAX_SHELL_LAYERS` in `engine/command_guard.rs`. */
export const MAX_SHELL_LAYERS = 4;

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

/** `EFFORT_RESTART_REASON` in `engine/agent_recovery/helpers.rs`. */
export const EFFORT_RESTART_REASON = "effort_restart";

/** `KEEP_ALIVE_IDLE_REASON` in `engine/thread_events/event_impl.rs`. */
export const KEEP_ALIVE_IDLE_REASON = "cache_keep_alive";

/** `KEEP_ALIVE_ENDING_EVENT_TYPES` in `engine/thread_events/event_impl.rs`. */
export const KEEP_ALIVE_ENDING_EVENT_TYPES = ["UserQuestionAsked", "CodingAgentPermissionRequest"] as const;

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

/** `OPENROUTER_JEV_MODEL` in `llm/judgment/endpoint.rs`. */
export const OPENROUTER_JEV_MODEL = "~typesafe/jev-latest";

/** `SystemOneEndpoint::id` in `llm/judgment/endpoint.rs`. */
export const SYSTEM_ONE_ENDPOINT_IDS = ["jev", "openrouter", "clef", "clef-flash", "openai", "custom"] as const;

/** `WAKE_DECISION_DEFAULT` in `llm/judgment/select.rs`. */
export const WAKE_DECISION_DEFAULT_ENDPOINT = "openai";

/** `EntryKind::as_str` in `engine/summary_tree/log.rs`. */
export const ENTRY_KINDS = ["prompt (human)", "prompt (agent)", "prompt (engine)", "response", "call", "result", "report", "turn", "artifact"] as const;

/** `PresenceOutcome::BY_STRENGTH` in `api/presence_pong.rs`. */
export const PRESENCE_OUTCOMES_BY_STRENGTH = ["shown", "auto_read", "silenced", "not_shown"] as const;

/** `SECTIONS` in `voice/sections.rs`. */
export const VOICE_RESIDENT_SECTIONS = [
  { id: "who-and-where", title: "Who you are talking to, and when" },
  { id: "this-thread", title: "This conversation" },
  { id: "workspace-shape", title: "What this workspace has" },
] as const;
