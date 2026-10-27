// AUTO-GENERATED. Do not edit by hand.
// Regenerate: ./scripts/codegen.sh
//
// Source of truth: CATALOG in crates/lucidos-engine/src/core/preference_catalog.rs.
// `fallback` is the value an unset preference resolves to, after any
// inheritance; `null` means it has none.

/** The spellings a stored switch reads as on, and as off, in lowercase. */
export const FLAG_ON_VALUES = ["1", "true", "yes", "on"] as const;
export const FLAG_OFF_VALUES = ["0", "false", "no", "off"] as const;

/** A stored switch as the engine reads it (`prefs::parse_flag`): on, off, or
 *  null for a spelling it reads as neither. Case-insensitive. */
export function parseFlag(raw: string): boolean | null {
  const spelled = raw.trim().toLowerCase();
  if ((FLAG_ON_VALUES as readonly string[]).includes(spelled)) return true;
  if ((FLAG_OFF_VALUES as readonly string[]).includes(spelled)) return false;
  return null;
}

// One export per key, so a bundle imports only what it reads.

export const PREF_LANGUAGE = {
  key: "language",
  scope: "global",
  type: "text",
  fallback: null,
} as const;

export const PREF_TIMEZONE = {
  key: "timezone",
  scope: "global",
  type: "text",
  fallback: null,
} as const;

export const PREF_CHAT_MODEL = {
  key: "chat_model",
  scope: "global",
  type: "text",
  fallback: "claude-opus-5-5",
} as const;

export const PREF_TRIGGER_MODEL = {
  key: "trigger_model",
  scope: "global",
  type: "text",
  fallback: "claude-sonnet-5-5",
} as const;

export const PREF_CHAT_REASONING_EFFORTS = {
  key: "chat_reasoning_efforts",
  scope: "global",
  type: "text",
  fallback: null,
} as const;

export const PREF_RESPONSE_STYLE = {
  key: "response_style",
  scope: "global",
  type: "text",
  fallback: "standard",
} as const;

export const PREF_RESPONSE_STYLES = {
  key: "response_styles",
  scope: "global",
  type: "text",
  fallback: null,
} as const;

export const PREF_TITLE_STYLE = {
  key: "title_style",
  scope: "global",
  type: "text",
  maxChars: 500,
  standard: "a very short title (3-6 words)",
  fallback: null,
} as const;

export const PREF_TECHNICAL_LITERACY = {
  key: "technical_literacy",
  scope: "global",
  type: "enum",
  values: ["not-set", "non-technical", "technical", "developer"],
  fallback: "not-set",
} as const;

export const PREF_IMAGE_MODEL = {
  key: "image_model",
  scope: "global",
  type: "enum",
  values: ["auto", "imagen-4", "gpt-image-2.5-flare", "gpt-image-2.5-sunburst", "gpt-image-2"],
  fallback: "auto",
} as const;

export const PREF_MODEL_TITLE = {
  key: "model_title",
  scope: "global",
  type: "text",
  fallback: "claude-haiku-5-5",
} as const;

export const PREF_REASONING_TITLE = {
  key: "reasoning_title",
  scope: "global",
  type: "enum",
  values: ["none", "low", "medium", "high", "xhigh", "max"],
  fallback: "none",
} as const;

export const PREF_MODEL_IMAGE_DESCRIPTION = {
  key: "model_image_description",
  scope: "global",
  type: "text",
  fallback: "claude-haiku-5-5",
} as const;

export const PREF_REASONING_IMAGE_DESCRIPTION = {
  key: "reasoning_image_description",
  scope: "global",
  type: "enum",
  values: ["none", "low", "medium", "high", "xhigh", "max"],
  fallback: "none",
} as const;

export const PREF_MODEL_CHANGE_SUMMARY = {
  key: "model_change_summary",
  scope: "global",
  type: "text",
  inherits: "model_title",
  fallback: "claude-haiku-5-5",
} as const;

export const PREF_REASONING_CHANGE_SUMMARY = {
  key: "reasoning_change_summary",
  scope: "global",
  type: "enum",
  values: ["none", "low", "medium", "high", "xhigh", "max"],
  inherits: "reasoning_title",
  fallback: "none",
} as const;

export const PREF_MODEL_SUMMARY_COMPACTION = {
  key: "model_summary_compaction",
  scope: "global",
  type: "text",
  fallback: null,
} as const;

export const PREF_REASONING_SUMMARY_COMPACTION = {
  key: "reasoning_summary_compaction",
  scope: "global",
  type: "enum",
  values: ["none", "low", "medium", "high", "xhigh", "max"],
  fallback: null,
} as const;

export const PREF_MODEL_MEMORY_FIND = {
  key: "model_memory_find",
  scope: "global",
  type: "text",
  fallback: "claude-haiku-5-5",
} as const;

export const PREF_REASONING_MEMORY_FIND = {
  key: "reasoning_memory_find",
  scope: "global",
  type: "enum",
  values: ["none", "low", "medium", "high", "xhigh", "max"],
  fallback: "none",
} as const;

export const PREF_MODEL_READ_DECISION = {
  key: "model_read_decision",
  scope: "global",
  type: "text",
  fallback: "claude-haiku-5-5",
} as const;

export const PREF_REASONING_READ_DECISION = {
  key: "reasoning_read_decision",
  scope: "global",
  type: "enum",
  values: ["none", "low", "medium", "high", "xhigh", "max"],
  fallback: "none",
} as const;

export const PREF_MODEL_WAKE_DECISION = {
  key: "model_wake_decision",
  scope: "global",
  type: "text",
  fallback: "gpt-6-luna",
} as const;

export const PREF_REASONING_WAKE_DECISION = {
  key: "reasoning_wake_decision",
  scope: "global",
  type: "enum",
  values: ["none", "low", "medium", "high", "xhigh", "max"],
  fallback: "none",
} as const;

export const PREF_MODEL_MEMORY = {
  key: "model_memory",
  scope: "global",
  type: "text",
  fallback: "claude-haiku-5-5",
} as const;

export const PREF_REASONING_MEMORY = {
  key: "reasoning_memory",
  scope: "global",
  type: "enum",
  values: ["none", "low", "medium", "high", "xhigh", "max"],
  fallback: "low",
} as const;

export const PREF_MODEL_QUERY_CLASSIFICATION = {
  key: "model_query_classification",
  scope: "global",
  type: "text",
  fallback: "claude-haiku-5-5",
} as const;

export const PREF_REASONING_QUERY_CLASSIFICATION = {
  key: "reasoning_query_classification",
  scope: "global",
  type: "enum",
  values: ["none", "low", "medium", "high", "xhigh", "max"],
  fallback: "none",
} as const;

export const PREF_MODEL_CONVERSATION_SUMMARY = {
  key: "model_conversation_summary",
  scope: "global",
  type: "text",
  fallback: "claude-haiku-5-5",
} as const;

export const PREF_VOICE_ENABLED = {
  key: "voice_enabled",
  scope: "global",
  type: "flag",
  fallback: "false",
} as const;

export const PREF_MODEL_VOICE_TALKER = {
  key: "model_voice_talker",
  scope: "global",
  type: "text",
  fallback: "gpt-realtime-2.1",
} as const;

export const PREF_MODEL_VOICE_TRANSCRIBER = {
  key: "model_voice_transcriber",
  scope: "global",
  type: "text",
  fallback: "gpt-transcribe",
} as const;

export const PREF_VOICE_TALKER_VOICE = {
  key: "voice_talker_voice",
  scope: "global",
  type: "text",
  fallback: "marin",
} as const;

export const PREF_VOICE_RESIDENT_SECTIONS = {
  key: "voice_resident_sections",
  scope: "global",
  type: "text",
  fallback: "who-and-where,this-thread,workspace-shape",
} as const;

export const PREF_REASONING_CONVERSATION_SUMMARY = {
  key: "reasoning_conversation_summary",
  scope: "global",
  type: "enum",
  values: ["none", "low", "medium", "high", "xhigh", "max"],
  fallback: "low",
} as const;

export const PREF_VERTEX_REGION = {
  key: "vertex_region",
  scope: "global",
  type: "text",
  fallback: "europe-west1",
} as const;

export const PREF_OPENCODE_FREE_ENABLED = {
  key: "opencode_free_enabled",
  scope: "global",
  type: "flag",
  fallback: "false",
} as const;

export const PREF_PROXY_TIMEOUT_SECS = {
  key: "proxy_timeout_secs",
  scope: "global",
  type: "number",
  min: 1,
  max: 600,
  fallback: "30",
} as const;

export const PREF_NOTIFICATIONS_FILTER = {
  key: "notifications_filter",
  scope: "global",
  type: "enum",
  values: ["all", "unread"],
  fallback: "all",
} as const;

export const PREF_NOTIFICATION_TOASTS = {
  key: "notification_toasts",
  scope: "global",
  type: "flag",
  fallback: "true",
} as const;

export const PREF_COMPOSER_ENTER_KEY = {
  key: "composer_enter_key",
  scope: "global",
  type: "enum",
  values: ["send", "newline"],
  fallback: "send",
} as const;

export const PREF_MOBILE_DYNAMIC_BARS = {
  key: "mobile_dynamic_bars",
  scope: "global",
  type: "flag",
  fallback: "false",
} as const;

export const PREF_AUTOMATIC_WIDGETS = {
  key: "automatic_widgets",
  scope: "global",
  type: "flag",
  fallback: "true",
} as const;

export const PREF_EXTERNAL_LINK_TARGET = {
  key: "external_link_target",
  scope: "global",
  type: "enum",
  values: ["safari", "ask", "in-app"],
  fallback: "safari",
} as const;

export const PREF_WELCOME_SUGGESTIONS_DISMISSED = {
  key: "welcome_suggestions_dismissed",
  scope: "global",
  type: "flag",
  fallback: "false",
} as const;

export const PREF_SELF_CURATED_CONTEXT_MODE = {
  key: "self_curated_context_mode",
  scope: "global",
  type: "flag",
  fallback: "false",
} as const;

export const PREF_SELF_CURATED_CONTEXT_EXPIRE_AFTER_ROUNDS = {
  key: "self_curated_context_expire_after_rounds",
  scope: "global",
  type: "number",
  min: 1,
  max: 1000,
  fallback: "5",
} as const;

export const PREF_SELF_CURATED_CONTEXT_SWEEP_EVERY_ROUNDS = {
  key: "self_curated_context_sweep_every_rounds",
  scope: "global",
  type: "number",
  min: 1,
  max: 1000,
  fallback: "10",
} as const;

export const PREF_WORKSPACE_PROMPT_FOOTPRINT_SECTION_CEILING = {
  key: "workspace_prompt_footprint_section_ceiling",
  scope: "global",
  type: "number",
  min: 100,
  max: 1000000,
  fallback: "6000",
} as const;

export const PREF_WORKSPACE_PROMPT_FOOTPRINT_TOTAL_CEILING = {
  key: "workspace_prompt_footprint_total_ceiling",
  scope: "global",
  type: "number",
  min: 100,
  max: 1000000,
  fallback: "20000",
} as const;

export const PREF_WORKSPACE_PROMPT_FOOTPRINT_UNUSED_DAYS = {
  key: "workspace_prompt_footprint_unused_days",
  scope: "global",
  type: "number",
  min: 1,
  max: 3650,
  fallback: "60",
} as const;

export const PREF_MEMORY_MODULE = {
  key: "memory_module",
  scope: "global",
  type: "enum",
  values: ["classic", "tree"],
  fallback: null,
} as const;

export const PREF_WORKSPACE_VIEW_BYTES_HOME = {
  key: "workspace_view_bytes_home",
  scope: "global",
  type: "number",
  min: 0,
  max: 262144,
  fallback: "65536",
} as const;

export const PREF_WORKSPACE_VIEW_BYTES_CHAT = {
  key: "workspace_view_bytes_chat",
  scope: "global",
  type: "number",
  min: 0,
  max: 262144,
  fallback: "16384",
} as const;

export const PREF_WORKSPACE_VIEW_BYTES_TRIGGER = {
  key: "workspace_view_bytes_trigger",
  scope: "global",
  type: "number",
  min: 0,
  max: 262144,
  fallback: "16384",
} as const;

export const PREF_WORKSPACE_VIEW_BYTES_CODING_AGENT = {
  key: "workspace_view_bytes_coding_agent",
  scope: "global",
  type: "number",
  min: 0,
  max: 262144,
  fallback: "0",
} as const;

export const PREF_THREAD_VIEW_BYTES = {
  key: "thread_view_bytes",
  scope: "global",
  type: "number",
  min: 0,
  max: 262144,
  fallback: "65536",
} as const;

export const PREF_MEMORY_VIEW_MODEL_CAPS = {
  key: "memory_view_model_caps",
  scope: "global",
  type: "text",
  fallback: null,
} as const;

export const PREF_CODING_AGENT_DEFAULT = {
  key: "coding_agent_default",
  scope: "global",
  type: "enum",
  values: ["claude-code", "codex"],
  fallback: "claude-code",
} as const;

export const PREF_CODING_AGENT_CLAUDE_PATH = {
  key: "coding_agent_claude_path",
  scope: "global",
  type: "text",
  fallback: null,
} as const;

export const PREF_CODING_AGENT_CODEX_PATH = {
  key: "coding_agent_codex_path",
  scope: "global",
  type: "text",
  fallback: null,
} as const;

export const PREF_CODING_AGENT_CLAUDE_PERMISSION_MODE = {
  key: "coding_agent_claude_permission_mode",
  scope: "global",
  type: "enum",
  values: ["accept-edits", "auto"],
  fallback: "accept-edits",
} as const;

export const PREF_CODING_AGENT_AUTO_COMPACT_TRIAL = {
  key: "coding_agent_auto_compact_trial",
  scope: "global",
  type: "enum",
  values: ["off", "ab", "on"],
  fallback: "off",
} as const;

export const PREF_CACHE_KEEP_ALIVE_MODE = {
  key: "cache_keep_alive_mode",
  scope: "global",
  type: "enum",
  values: ["break-even", "workspace-data", "off"],
  fallback: "off",
} as const;

export const PREF_MODEL_CLAUDE_CODE = {
  key: "model_claude_code",
  scope: "global",
  type: "coding-agent-model",
  fallback: "claude-opus-5-5",
} as const;

export const PREF_REASONING_CLAUDE_CODE = {
  key: "reasoning_claude_code",
  scope: "global",
  type: "enum",
  values: ["low", "medium", "high", "xhigh", "max"],
  fallback: "medium",
} as const;

export const PREF_MODEL_CODEX = {
  key: "model_codex",
  scope: "global",
  type: "coding-agent-model",
  fallback: "gpt-6.1-sol",
} as const;

export const PREF_REASONING_CODEX = {
  key: "reasoning_codex",
  scope: "global",
  type: "enum",
  values: ["low", "medium", "high", "xhigh", "max"],
  fallback: "medium",
} as const;

export const PREF_INTELLIGENCE_LADDER_CHAT = {
  key: "intelligence_ladder_chat",
  scope: "global",
  type: "text",
  fallback: null,
} as const;

export const PREF_INTELLIGENCE_LADDER_CLAUDE_CODE = {
  key: "intelligence_ladder_claude_code",
  scope: "global",
  type: "text",
  fallback: null,
} as const;

export const PREF_INTELLIGENCE_LADDER_CODEX = {
  key: "intelligence_ladder_codex",
  scope: "global",
  type: "text",
  fallback: null,
} as const;

export const PREF_LONG_PROMPT_SHARE = {
  key: "long_prompt_share",
  scope: "global",
  type: "number",
  min: 0,
  max: 100,
  fallback: null,
} as const;

export const PREF_THEME_MODE = {
  key: "theme-mode",
  scope: "device",
  type: "enum",
  values: ["light", "dark", "system"],
  fallback: "system",
} as const;

export const PREF_FONT_FAMILY = {
  key: "font-family",
  scope: "device",
  type: "text",
  fallback: "theme",
} as const;

export const PREF_UI_SCALE = {
  key: "ui-scale",
  scope: "device",
  type: "number",
  min: 75,
  max: 200,
  fallback: "100",
} as const;

export const PREF_MOTION = {
  key: "motion",
  scope: "device",
  type: "enum",
  values: ["system", "reduce", "full"],
  fallback: "system",
} as const;

export const PREF_THEME_EFFECTS = {
  key: "theme-effects",
  scope: "device",
  type: "enum",
  values: ["system", "reduce", "full"],
  fallback: "system",
} as const;

export const PREF_THEME = {
  key: "theme",
  scope: "device",
  type: "text",
  fallback: "lucidos",
} as const;

export const PREF_AUTOCORRECT = {
  key: "autocorrect",
  scope: "device",
  type: "flag",
  fallback: "true",
} as const;

export const PREF_PUSH_NOTIFICATIONS = {
  key: "push_notifications",
  scope: "device",
  type: "enum",
  values: ["enabled", "declined"],
  fallback: null,
} as const;

export const PREF_BACKUP_SCHEDULE = {
  key: "backup_schedule",
  scope: "global",
  type: "text",
  fallback: "off",
} as const;

export const PREF_BACKUP_PROVIDER = {
  key: "backup_provider",
  scope: "global",
  type: "enum",
  values: ["google_drive", "dropbox"],
  fallback: null,
} as const;

export const PREF_BACKUP_RETENTION = {
  key: "backup_retention",
  scope: "global",
  type: "number",
  min: 1,
  max: 1000,
  fallback: "5",
} as const;

export const PREF_BACKUP_REMINDER_DISMISSED = {
  key: "backup_reminder_dismissed",
  scope: "global",
  type: "text",
  fallback: null,
} as const;

export const PREF_COMMAND_GUARD = {
  key: "command_guard",
  scope: "global",
  type: "flag",
  fallback: "false",
} as const;

export const PREF_COMMAND_GUARD_JUDGE = {
  key: "command_guard_judge",
  scope: "global",
  type: "flag",
  fallback: "true",
} as const;

export const PREF_COMMAND_GUARD_CODING_AGENTS = {
  key: "command_guard_coding_agents",
  scope: "global",
  type: "flag",
  fallback: "true",
} as const;

export const PREF_MODEL_COMMAND_JUDGE = {
  key: "model_command_judge",
  scope: "global",
  type: "text",
  fallback: "gemini-3.1-flash-lite",
} as const;

export const PREF_REASONING_COMMAND_JUDGE = {
  key: "reasoning_command_judge",
  scope: "global",
  type: "enum",
  values: ["none", "low", "medium", "high", "xhigh", "max"],
  fallback: "none",
} as const;

export const PREF_JUDGMENT_COMMAND_GUARD = {
  key: "judgment_command_guard",
  scope: "global",
  type: "text",
  fallback: null,
} as const;

export const PREF_JUDGMENT_QUERY_CLASSIFICATION = {
  key: "judgment_query_classification",
  scope: "global",
  type: "text",
  fallback: null,
} as const;

export const PREF_JUDGMENT_MEMORY_FIND = {
  key: "judgment_memory_find",
  scope: "global",
  type: "text",
  fallback: null,
} as const;

export const PREF_JUDGMENT_READ_DECISION = {
  key: "judgment_read_decision",
  scope: "global",
  type: "text",
  fallback: null,
} as const;

export const PREF_JUDGMENT_WAKE_DECISION = {
  key: "judgment_wake_decision",
  scope: "global",
  type: "text",
  fallback: null,
} as const;

export const PREF_MAX_TOOL_CALLS = {
  key: "max_tool_calls",
  scope: "global",
  type: "number",
  min: 1,
  max: 4294967295,
  fallback: "500",
} as const;

export const PREF_CAPTURE_CONTEXT = {
  key: "capture_context",
  scope: "global",
  type: "flag",
  fallback: "false",
} as const;

export const PREF_CACHE_CARDS = {
  key: "cache_cards",
  scope: "global",
  type: "flag",
  fallback: "false",
} as const;

export const PREF_KEYBINDINGS = {
  key: "keybindings",
  scope: "global",
  type: "text",
  fallback: null,
} as const;

export const PREF_VOICE_INPUT_DEVICE = {
  key: "voice_input_device",
  scope: "device",
  type: "text",
  fallback: null,
} as const;

export const PREF_NETWORK_BIND = {
  key: "network_bind",
  scope: "global",
  type: "text",
  fallback: null,
} as const;

export const PREF_ENGINE_SWITCH_DISMISSED_BUILD = {
  key: "engine_switch_dismissed_build",
  scope: "global",
  type: "text",
  fallback: null,
} as const;

export const PREF_CLIENT_REFRESH_DISMISSED_BUILD = {
  key: "client_refresh_dismissed_build",
  scope: "global",
  type: "text",
  fallback: null,
} as const;

export const PREF_PROVIDER_ENABLED_VERTEX = {
  key: "provider_enabled_vertex",
  scope: "global",
  type: "flag",
  fallback: "true",
} as const;

export const PREF_PROVIDER_ENABLED_ANTHROPIC = {
  key: "provider_enabled_anthropic",
  scope: "global",
  type: "flag",
  fallback: "true",
} as const;

export const PREF_PROVIDER_ENABLED_OPENAI = {
  key: "provider_enabled_openai",
  scope: "global",
  type: "flag",
  fallback: "true",
} as const;

export const PREF_PROVIDER_ENABLED_OPENROUTER = {
  key: "provider_enabled_openrouter",
  scope: "global",
  type: "flag",
  fallback: "true",
} as const;

export const PREF_PROVIDER_ENABLED_XAI = {
  key: "provider_enabled_xai",
  scope: "global",
  type: "flag",
  fallback: "true",
} as const;

export const PREF_PROVIDER_ENABLED_LOCAL = {
  key: "provider_enabled_local",
  scope: "global",
  type: "flag",
  fallback: "true",
} as const;

export const PREF_PROVIDER_ENABLED_CLAUDE_GATEWAY = {
  key: "provider_enabled_claude_gateway",
  scope: "global",
  type: "flag",
  fallback: "true",
} as const;

export const PREF_PROVIDER_ENABLED_TYPESAFE = {
  key: "provider_enabled_typesafe",
  scope: "global",
  type: "flag",
  fallback: "true",
} as const;

export const PREF_PROVIDER_ENABLED_CLOUDFLARE_WORKERS_AI = {
  key: "provider_enabled_cloudflare_workers_ai",
  scope: "global",
  type: "flag",
  fallback: "true",
} as const;

export const PREF_PROVIDER_ENABLED_SYSTEM_ONE_CUSTOM = {
  key: "provider_enabled_system_one_custom",
  scope: "global",
  type: "flag",
  fallback: "true",
} as const;

export const PREF_LOCAL_BASE_URL = {
  key: "local_base_url",
  scope: "global",
  type: "text",
  fallback: "http://localhost:11434/v1",
} as const;

export const PREF_CLAUDE_GATEWAY_URL = {
  key: "claude_gateway_url",
  scope: "global",
  type: "text",
  fallback: null,
} as const;

export const PREF_SYSTEM_ONE_CUSTOM_URL = {
  key: "system_one_custom_url",
  scope: "global",
  type: "text",
  fallback: null,
} as const;

export const PREF_SYSTEM_ONE_CUSTOM_MODEL = {
  key: "system_one_custom_model",
  scope: "global",
  type: "text",
  fallback: null,
} as const;

export const PREF_SYSTEM_ONE_OPENROUTER_MODEL = {
  key: "system_one_openrouter_model",
  scope: "global",
  type: "text",
  fallback: null,
} as const;

export const PREFERENCE_CATALOG = {
  "language": PREF_LANGUAGE,
  "timezone": PREF_TIMEZONE,
  "chat_model": PREF_CHAT_MODEL,
  "trigger_model": PREF_TRIGGER_MODEL,
  "chat_reasoning_efforts": PREF_CHAT_REASONING_EFFORTS,
  "response_style": PREF_RESPONSE_STYLE,
  "response_styles": PREF_RESPONSE_STYLES,
  "title_style": PREF_TITLE_STYLE,
  "technical_literacy": PREF_TECHNICAL_LITERACY,
  "image_model": PREF_IMAGE_MODEL,
  "model_title": PREF_MODEL_TITLE,
  "reasoning_title": PREF_REASONING_TITLE,
  "model_image_description": PREF_MODEL_IMAGE_DESCRIPTION,
  "reasoning_image_description": PREF_REASONING_IMAGE_DESCRIPTION,
  "model_change_summary": PREF_MODEL_CHANGE_SUMMARY,
  "reasoning_change_summary": PREF_REASONING_CHANGE_SUMMARY,
  "model_summary_compaction": PREF_MODEL_SUMMARY_COMPACTION,
  "reasoning_summary_compaction": PREF_REASONING_SUMMARY_COMPACTION,
  "model_memory_find": PREF_MODEL_MEMORY_FIND,
  "reasoning_memory_find": PREF_REASONING_MEMORY_FIND,
  "model_read_decision": PREF_MODEL_READ_DECISION,
  "reasoning_read_decision": PREF_REASONING_READ_DECISION,
  "model_wake_decision": PREF_MODEL_WAKE_DECISION,
  "reasoning_wake_decision": PREF_REASONING_WAKE_DECISION,
  "model_memory": PREF_MODEL_MEMORY,
  "reasoning_memory": PREF_REASONING_MEMORY,
  "model_query_classification": PREF_MODEL_QUERY_CLASSIFICATION,
  "reasoning_query_classification": PREF_REASONING_QUERY_CLASSIFICATION,
  "model_conversation_summary": PREF_MODEL_CONVERSATION_SUMMARY,
  "voice_enabled": PREF_VOICE_ENABLED,
  "model_voice_talker": PREF_MODEL_VOICE_TALKER,
  "model_voice_transcriber": PREF_MODEL_VOICE_TRANSCRIBER,
  "voice_talker_voice": PREF_VOICE_TALKER_VOICE,
  "voice_resident_sections": PREF_VOICE_RESIDENT_SECTIONS,
  "reasoning_conversation_summary": PREF_REASONING_CONVERSATION_SUMMARY,
  "vertex_region": PREF_VERTEX_REGION,
  "opencode_free_enabled": PREF_OPENCODE_FREE_ENABLED,
  "proxy_timeout_secs": PREF_PROXY_TIMEOUT_SECS,
  "notifications_filter": PREF_NOTIFICATIONS_FILTER,
  "notification_toasts": PREF_NOTIFICATION_TOASTS,
  "composer_enter_key": PREF_COMPOSER_ENTER_KEY,
  "mobile_dynamic_bars": PREF_MOBILE_DYNAMIC_BARS,
  "automatic_widgets": PREF_AUTOMATIC_WIDGETS,
  "external_link_target": PREF_EXTERNAL_LINK_TARGET,
  "welcome_suggestions_dismissed": PREF_WELCOME_SUGGESTIONS_DISMISSED,
  "self_curated_context_mode": PREF_SELF_CURATED_CONTEXT_MODE,
  "self_curated_context_expire_after_rounds": PREF_SELF_CURATED_CONTEXT_EXPIRE_AFTER_ROUNDS,
  "self_curated_context_sweep_every_rounds": PREF_SELF_CURATED_CONTEXT_SWEEP_EVERY_ROUNDS,
  "workspace_prompt_footprint_section_ceiling": PREF_WORKSPACE_PROMPT_FOOTPRINT_SECTION_CEILING,
  "workspace_prompt_footprint_total_ceiling": PREF_WORKSPACE_PROMPT_FOOTPRINT_TOTAL_CEILING,
  "workspace_prompt_footprint_unused_days": PREF_WORKSPACE_PROMPT_FOOTPRINT_UNUSED_DAYS,
  "memory_module": PREF_MEMORY_MODULE,
  "workspace_view_bytes_home": PREF_WORKSPACE_VIEW_BYTES_HOME,
  "workspace_view_bytes_chat": PREF_WORKSPACE_VIEW_BYTES_CHAT,
  "workspace_view_bytes_trigger": PREF_WORKSPACE_VIEW_BYTES_TRIGGER,
  "workspace_view_bytes_coding_agent": PREF_WORKSPACE_VIEW_BYTES_CODING_AGENT,
  "thread_view_bytes": PREF_THREAD_VIEW_BYTES,
  "memory_view_model_caps": PREF_MEMORY_VIEW_MODEL_CAPS,
  "coding_agent_default": PREF_CODING_AGENT_DEFAULT,
  "coding_agent_claude_path": PREF_CODING_AGENT_CLAUDE_PATH,
  "coding_agent_codex_path": PREF_CODING_AGENT_CODEX_PATH,
  "coding_agent_claude_permission_mode": PREF_CODING_AGENT_CLAUDE_PERMISSION_MODE,
  "coding_agent_auto_compact_trial": PREF_CODING_AGENT_AUTO_COMPACT_TRIAL,
  "cache_keep_alive_mode": PREF_CACHE_KEEP_ALIVE_MODE,
  "model_claude_code": PREF_MODEL_CLAUDE_CODE,
  "reasoning_claude_code": PREF_REASONING_CLAUDE_CODE,
  "model_codex": PREF_MODEL_CODEX,
  "reasoning_codex": PREF_REASONING_CODEX,
  "intelligence_ladder_chat": PREF_INTELLIGENCE_LADDER_CHAT,
  "intelligence_ladder_claude_code": PREF_INTELLIGENCE_LADDER_CLAUDE_CODE,
  "intelligence_ladder_codex": PREF_INTELLIGENCE_LADDER_CODEX,
  "long_prompt_share": PREF_LONG_PROMPT_SHARE,
  "theme-mode": PREF_THEME_MODE,
  "font-family": PREF_FONT_FAMILY,
  "ui-scale": PREF_UI_SCALE,
  "motion": PREF_MOTION,
  "theme-effects": PREF_THEME_EFFECTS,
  "theme": PREF_THEME,
  "autocorrect": PREF_AUTOCORRECT,
  "push_notifications": PREF_PUSH_NOTIFICATIONS,
  "backup_schedule": PREF_BACKUP_SCHEDULE,
  "backup_provider": PREF_BACKUP_PROVIDER,
  "backup_retention": PREF_BACKUP_RETENTION,
  "backup_reminder_dismissed": PREF_BACKUP_REMINDER_DISMISSED,
  "command_guard": PREF_COMMAND_GUARD,
  "command_guard_judge": PREF_COMMAND_GUARD_JUDGE,
  "command_guard_coding_agents": PREF_COMMAND_GUARD_CODING_AGENTS,
  "model_command_judge": PREF_MODEL_COMMAND_JUDGE,
  "reasoning_command_judge": PREF_REASONING_COMMAND_JUDGE,
  "judgment_command_guard": PREF_JUDGMENT_COMMAND_GUARD,
  "judgment_query_classification": PREF_JUDGMENT_QUERY_CLASSIFICATION,
  "judgment_memory_find": PREF_JUDGMENT_MEMORY_FIND,
  "judgment_read_decision": PREF_JUDGMENT_READ_DECISION,
  "judgment_wake_decision": PREF_JUDGMENT_WAKE_DECISION,
  "max_tool_calls": PREF_MAX_TOOL_CALLS,
  "capture_context": PREF_CAPTURE_CONTEXT,
  "cache_cards": PREF_CACHE_CARDS,
  "keybindings": PREF_KEYBINDINGS,
  "voice_input_device": PREF_VOICE_INPUT_DEVICE,
  "network_bind": PREF_NETWORK_BIND,
  "engine_switch_dismissed_build": PREF_ENGINE_SWITCH_DISMISSED_BUILD,
  "client_refresh_dismissed_build": PREF_CLIENT_REFRESH_DISMISSED_BUILD,
  "provider_enabled_vertex": PREF_PROVIDER_ENABLED_VERTEX,
  "provider_enabled_anthropic": PREF_PROVIDER_ENABLED_ANTHROPIC,
  "provider_enabled_openai": PREF_PROVIDER_ENABLED_OPENAI,
  "provider_enabled_openrouter": PREF_PROVIDER_ENABLED_OPENROUTER,
  "provider_enabled_xai": PREF_PROVIDER_ENABLED_XAI,
  "provider_enabled_local": PREF_PROVIDER_ENABLED_LOCAL,
  "provider_enabled_claude_gateway": PREF_PROVIDER_ENABLED_CLAUDE_GATEWAY,
  "provider_enabled_typesafe": PREF_PROVIDER_ENABLED_TYPESAFE,
  "provider_enabled_cloudflare_workers_ai": PREF_PROVIDER_ENABLED_CLOUDFLARE_WORKERS_AI,
  "provider_enabled_system_one_custom": PREF_PROVIDER_ENABLED_SYSTEM_ONE_CUSTOM,
  "local_base_url": PREF_LOCAL_BASE_URL,
  "claude_gateway_url": PREF_CLAUDE_GATEWAY_URL,
  "system_one_custom_url": PREF_SYSTEM_ONE_CUSTOM_URL,
  "system_one_custom_model": PREF_SYSTEM_ONE_CUSTOM_MODEL,
  "system_one_openrouter_model": PREF_SYSTEM_ONE_OPENROUTER_MODEL,
} as const;

export type PreferenceKey = keyof typeof PREFERENCE_CATALOG;

/** The allowed values of an enum preference, as a union. */
export type PreferenceValues<K extends PreferenceKey> =
  (typeof PREFERENCE_CATALOG)[K] extends { values: readonly (infer V)[] } ? V : never;
