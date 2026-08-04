//! LLM-facing schemas for cross-cutting tools that do not belong to a
//! single domain family: navigate_ui, git_clone, get_backup_status,
//! request_credential, connect_oauth_account, execute_intent,
//! ask_user_question, todo_write.
//!
//! (manage_repositories / manage_models moved to the capability parity manifest —
//! domains `repositories` / `models` — and are built by `capability_manifest`.)

use crate::llm::provider::ToolDefinition;
use crate::llm::tool_names as tn;
use serde_json::json;

/// Single source of truth for the `navigate_ui` `target` enum — the top-level
/// navigation destinations. The SDK `NavigateTarget` type is GENERATED from this
/// list (see `packages/lucidos-sdk/src/generated/navigate-targets.ts`), so the
/// LLM tool schema and the SDK can never drift. To change the set, edit here and
/// regenerate:
///   cargo test -p lucidos-engine --lib generate_navigate_targets_file -- --ignored
const NAVIGATE_TARGETS: &[&str] = &[
    "files",
    "apps",
    "app-store",
    "plugins",
    "triggers",
    "thread-queue",
    "changes",
    "notifications",
    "settings",
    "app",
    "file",
    "trigger",
    "thread",
    "new-app",
    "new-trigger",
    "new-chat",
    "url",
];

/// Single source of truth for the `navigate_ui` `settings_view` enum — the
/// Settings sub-sections the agent may deep-link to. The SDK `SettingsViewTarget`
/// type is GENERATED from this list (same generated file as `NAVIGATE_TARGETS`).
/// Deliberately excludes the platform-gated `mobile-access` / `experimental`
/// subviews (hidden from the user's own nav off-platform; the LLM has no reliable
/// platform signal). Every value here must be a renderable subview in
/// `SettingsView.renderSubview` — a frontend Vitest cross-check pins that.
const NAVIGABLE_SETTINGS_VIEWS: &[&str] = &[
    "models",
    "appearance",
    "devices",
    "accounts",
    "repositories",
    "marketplaces",
    "permissions",
    "keyboard-shortcuts",
    "system",
    "thread-queue",
    "backup",
    "memory",
    "disk-usage",
    "environment-variables",
];

/// Tool for navigating the Lucidos UI to a specific panel, app, or file.
pub fn get_navigate_ui_tool() -> ToolDefinition {
    ToolDefinition {
        name: tn::NAVIGATE_UI.to_string(),
        description: "Navigate the Lucidos UI to a specific panel, app, file, thread, or form. Use this when the user asks to open, show, or go to something in the interface — e.g. \"open settings\", \"show me the habit tracker\", \"go to triggers\", \"open my budget file\", \"go to that thread\".".to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "target": {
                    "type": "string",
                    "enum": NAVIGATE_TARGETS,
                    "description": "Navigation target. Use 'files', 'apps', 'app-store', 'triggers', 'thread-queue', 'changes', 'notifications', 'settings' for panels. 'app-store' opens the Plugins panel, where users browse and install apps and other plugins from marketplaces (they uncheck the panel's 'Installed only' filter to see installable ones; refer to it as the 'Plugins panel', never the retired 'App Store' or 'Store'). Use 'app' to open an app by ID. Use 'file' to preview a file. Use 'trigger' to focus a trigger by ID. Use 'thread' to focus a thread by ID. Use 'new-app', 'new-trigger', or 'new-chat' to open the creation form. Use 'url' to open a URL in the internal browser panel. For a Settings sub-section, use target 'settings' with the 'settings_view' arg."
                },
                "settings_view": {
                    "type": "string",
                    "enum": NAVIGABLE_SETTINGS_VIEWS,
                    "description": "Which Settings sub-section to open. Only used when target is 'settings'. 'models' = chat/image/background model settings (use this for questions about the current model), 'appearance' = theme/font/scale, 'permissions' = command guard + allowlists, 'keyboard-shortcuts', 'devices', 'accounts' = credentials + OAuth, 'repositories', 'marketplaces'. Under Settings → System: 'system', 'backup', 'memory', 'disk-usage', 'environment-variables', 'thread-queue'. Omit to land on the Settings home list."
                },
                "app_id": {
                    "type": "string",
                    "description": "App ID to open. Required when target is 'app'."
                },
                "file_path": {
                    "type": "string",
                    "description": "File path to preview, including the directory prefix (e.g. 'artifacts/research/notes.md', 'knowhow/domain/guide.md'). Required when target is 'file'."
                },
                "id": {
                    "type": "string",
                    "description": "ID of the entity to navigate to. Required when target is 'thread' or 'trigger'."
                },
                "url": {
                    "type": "string",
                    "description": "URL to open in the internal browser panel. Required when target is 'url'."
                },
                "event_id": {
                    "type": "string",
                    "description": "Specific event UUID inside thread to scroll-to-and-pulse on land. Only used when target is 'thread'."
                },
                "prompt": {
                    "type": "string",
                    "description": "Pre-populated draft text for the new chat compose box. Only used when target is 'new-chat'."
                }
            },
            "required": ["target"]
        }),
    }
}

// `manage_repositories` and `manage_models` are now manifest-built grouped tools
// (see `crate::capability_manifest`, domains `repositories` / `models`). Their
// schemas come from the manifest (SSOT); `execute_tool` keeps routing the
// unchanged tool names to `execute_manage_repositories` / `execute_manage_models`.

pub(super) fn git_clone_tools() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: tn::GIT_CLONE.to_string(),
            description: "Clone a git repository. Per system-knowhow/best-practices rule 8, the agent must explicitly choose where the clone lands — there is no default. Two valid destination roots: '.lucidos/tmp/<name>/' for research / inspect / extract-then-discard work (ephemeral, gitignored, won't bloat artifact count); 'data/artifacts/imported/<name>/' for persistent, git-tracked dependencies the user wants to keep. Refuses to write to artifacts/imported/ if the resulting clone exceeds 500 files or 100 MB — extract only what the app needs, or move the dataset to ~/.lucidos/data/<name>/.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "Git repository URL (https://github.com/owner/repo or git@github.com:owner/repo)"
                    },
                    "branch": {
                        "type": "string",
                        "description": "Branch to clone (optional, defaults to default branch)"
                    },
                    "destination": {
                        "type": "string",
                        "description": "REQUIRED. Must start with one of two prefixes: '.lucidos/tmp/<name>/' (ephemeral, gitignored — the default per rule 8 for research / inspect / extract-then-discard work) or 'data/artifacts/imported/<name>/' (persistent, git-tracked — only when the user has confirmed they want the full repo in the workspace). Bare names without a prefix are rejected to prevent silently bloating data/artifacts/."
                    },
                    "include_patterns": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Glob patterns for files to include (optional, e.g., ['*.py', 'src/**/*.rs'])"
                    },
                    "exclude_patterns": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Glob patterns for files to exclude (optional, e.g., ['*.lock', 'node_modules/**'])"
                    }
                },
                "required": ["url", "destination"]
            }),
        },
    ]
}

/// Standalone `get_backup_status` (read-only status, distinct surface — NOT part
/// of the grouped `preferences` tool). Environment-variable management moved to
/// the grouped `env_vars` manifest tool (list/set/delete); the former
/// `set_environment_variable` name stays wired as a back-compat alias to its
/// `set` action in `execute_tool`.
pub(super) fn backup_status_tools() -> Vec<ToolDefinition> {
    vec![ToolDefinition {
        name: tn::GET_BACKUP_STATUS.to_string(),
        description: "Read the workspace's backup status: the schedule (in the user's timezone) and next scheduled run, the provider and retention, the last run (with duration), recent run history (start/finish/size for each), and whether backups are stale. Read-only — makes no changes. Use it to answer 'when's my next/last backup?', 'how big/long are my backups?', or to check before changing the schedule. The schedule/provider/retention are CHANGED with set_preference (keys backup_schedule, backup_provider, backup_retention); restore is done from the workspace picker.".to_string(),
        parameters: json!({
            "type": "object",
            "properties": {}
        }),
    }]
}

pub(super) fn request_credential_tools() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: tn::REQUEST_CREDENTIAL.to_string(),
            description: "Request API credentials from the user via a secure modal dialog. NEVER accept credentials pasted in chat — always use this tool. The user enters the credential in a popup (not in chat), keeping it secure and out of the event log. Call this for ONE credential at a time and wait for it to resolve before requesting the next — issuing multiple credential requests in parallel stacks modals and forces the user to history-navigate between them.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "service_name": {
                        "type": "string",
                        "description": "Name of the service (e.g., 'oura', 'github', 'notion')"
                    },
                    "prompt": {
                        "type": "string",
                        "description": "Instructions shown in the modal. Include where to find the credential (e.g., 'Go to cloud.ouraring.com → Personal Access Tokens → Create. Paste the token below.')"
                    },
                    "base_url": {
                        "type": "string",
                        "description": "Base URL for the API (e.g., 'https://api.ouraring.com')"
                    },
                    "auth_type": {
                        "type": "string",
                        "enum": ["api_key", "bearer", "basic", "password", "oauth_client"],
                        "description": "Type of authentication. Use 'password' for username+password (injected as Basic auth), 'oauth_client' for OAuth client_id (+ client_secret for a confidential client). For oauth_client, first load_knowhow('system-knowhow/oauth-providers') and pass auth_url/token_url/userinfo_url (+ optional scopes/redirect_uri) so the modal pre-fills them and the user only enters client_id+client_secret. The client secret is OPTIONAL: leaving it blank makes Lucidos a public client that authenticates with PKCE instead — the right shape when the provider's app registration is a desktop/native app. Omit the endpoint args only for a provider not yet in that knowhow (then the user types the URLs by hand once). Default: api_key"
                    },
                    "auth_url": {
                        "type": "string",
                        "description": "oauth_client only. OAuth authorization endpoint looked up in the oauth-providers knowhow. Pre-fills the modal so the user doesn't type it."
                    },
                    "token_url": {
                        "type": "string",
                        "description": "oauth_client only. OAuth token endpoint from the oauth-providers knowhow. Pre-fills the modal."
                    },
                    "userinfo_url": {
                        "type": "string",
                        "description": "oauth_client only, optional. OAuth userinfo endpoint from the oauth-providers knowhow."
                    },
                    "scopes": {
                        "type": "string",
                        "description": "oauth_client only, optional. Default OAuth scopes (space-separated) to pre-fill the modal's scopes field."
                    },
                    "redirect_uri": {
                        "type": "string",
                        "description": "oauth_client only, optional. Loopback callback URI the user must register with the provider. Omit for the default 'http://127.0.0.1:14981/oauth/callback'. Only pass one when the oauth-providers knowhow says this provider needs a different host form (e.g. 'http://localhost:14981/oauth/callback'). Must be one of: http://127.0.0.1:14981/oauth/callback, http://localhost:14981/oauth/callback, http://[::1]:14981/oauth/callback."
                    },
                    "env_var_name": {
                        "type": "string",
                        "description": "Optional. Custom environment variable name to ALSO inject the secret under, in addition to the default CRED_<NAME> (e.g. 'APPLE_PASSWORD' when a script or tool expects that exact name). Pre-fills the modal's 'Env var name' field; the user can edit or clear it. Must match [A-Z_][A-Z0-9_]* and must not be an engine-owned name (CRED_*, OAUTH_*, PG*, PATH, LUCIDOS_*). Single-value auth types only — ignored for 'password' (which splits into _USERNAME/_PASSWORD)."
                    }
                },
                "required": ["service_name", "prompt", "base_url", "auth_type"]
            }),
        },
    ]
}

pub(super) fn connect_oauth_tools() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: tn::CONNECT_OAUTH_ACCOUNT.to_string(),
            description: "Connect an OAuth account so Lucidos can make authenticated API requests on the user's behalf. Works for any OAuth 2.0 provider. Opens the user's browser for authorization. If client credentials for the provider aren't configured yet, this first opens the credential modal — load_knowhow('system-knowhow/oauth-providers') and pass auth_url/token_url/userinfo_url so they pre-fill (e.g. a health-only Google connection named 'ghealth' uses Google's endpoints under a distinct name). For an already-configured provider, just pass provider + scopes.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "provider": {
                        "type": "string",
                        "description": "Provider name (e.g., 'google', 'microsoft', 'github'). Use a distinct name for a dedicated connection (e.g. 'ghealth' for a health-only Google connection that must not carry other scopes)."
                    },
                    "scopes": {
                        "type": "string",
                        "description": "Space-separated OAuth scopes to request (e.g., 'https://www.googleapis.com/auth/spreadsheets https://www.googleapis.com/auth/drive.readonly')"
                    },
                    "auth_url": {
                        "type": "string",
                        "description": "Optional. OAuth authorization endpoint from the oauth-providers knowhow — used only when the credential modal opens (no client credentials yet) to pre-fill the endpoints."
                    },
                    "token_url": {
                        "type": "string",
                        "description": "Optional. OAuth token endpoint from the oauth-providers knowhow — pre-fills the credential modal when it opens."
                    },
                    "userinfo_url": {
                        "type": "string",
                        "description": "Optional. OAuth userinfo endpoint from the oauth-providers knowhow — pre-fills the credential modal when it opens."
                    },
                    "base_url": {
                        "type": "string",
                        "description": "Optional. API base URL to pre-fill in the credential modal when it opens (e.g. 'https://healthcare.googleapis.com')."
                    },
                    "redirect_uri": {
                        "type": "string",
                        "description": "Optional. Loopback callback URI the user registers with the provider — pre-fills the credential modal when it opens. Omit for the default 'http://127.0.0.1:14981/oauth/callback'. Pass 'http://localhost:14981/oauth/callback' only when the oauth-providers knowhow says this provider needs the name form instead of the IP."
                    }
                },
                "required": ["provider", "scopes"]
            }),
        },
    ]
}

pub(super) fn execute_intent_tools() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: tn::EXECUTE_INTENT.to_string(),
            description: "Execute a stored intent — a description of what the user wants, paired with know-how for how to achieve it. The intent's instructions and relevant know-how are loaded automatically. Internal tool calls happen inside the execution — only the final result is returned. Use this when the user's request matches an available intent.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "intent_id": {
                        "type": "string",
                        "description": "Intent ID (e.g., 'job-search/find-jobs', 'home-control/run-control-loop')"
                    },
                    "task": {
                        "type": "string",
                        "description": "What to do (e.g., 'Make the title black', 'Log today\\'s sleep data')"
                    }
                },
                "required": ["intent_id"]
            }),
        },
    ]
}

pub(super) fn ask_user_question_tools() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: tn::ASK_USER_QUESTION.to_string(),
            description: "Ask the user a multiple-choice question and wait for their pick. The Lucidos UI renders each question as a card with the options as clickable buttons, so the user can answer with a tap instead of typing. Use whenever the answer is a yes/no decision, a pick from 2–4 named choices, or a confirmation step before doing something significant, anywhere a button would beat a typed reply. Single-question is the common case; pass up to 4 questions in one call only when they are a tight batch the user should answer in sequence (each question renders one card at a time). Every question object MUST carry a non-empty `question` (the full question text the user reads on the card); the short `header` chip-label is NEVER a substitute, and the engine rejects (and forces you to re-ask) any call that leaves `question` empty, so fill it the first time. NEVER add an \"Other\" / \"Something else\" / \"Let me type it\" option: Lucidos has no text-entry option, so tapping one just sends you that label back as the user's answer, which is a dead end. Both escapes are on every card without you: the user can type any reply in the prompt textarea (it arrives as their answer to this question), and Cancel dismisses the question so they can steer you elsewhere. An option carrying a decision you can act on is different and still welcome (\"None of these\", \"Neither, ask me later\", \"Cancel the deploy\"); what is banned is an option whose only meaning is \"I will type it instead\". Returns a JSON object mapping each question text to the chosen option label (or the user's typed text when they answer freeform).".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "questions": {
                        "type": "array",
                        "minItems": 1,
                        "maxItems": 4,
                        "description": "1–4 questions to ask. Rendered one card at a time; the user answers them in order. The combined answers map comes back as the tool result.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "question": {
                                    "type": "string",
                                    "description": "REQUIRED. The full question shown to the user (in the user's language). Always provide the complete question here — never leave it empty or put the question only in `header`. The engine rejects calls whose `question` is missing and makes you re-ask."
                                },
                                "header": {
                                    "type": "string",
                                    "description": "Optional short chip-label (≤12 chars) summarising the question (e.g. \"Approach\", \"Confirm\"). Supplementary only — NOT a replacement for `question`."
                                },
                                "options": {
                                    "type": "array",
                                    "minItems": 2,
                                    "maxItems": 4,
                                    "description": "2–4 choices rendered as buttons. Pick distinct, mutually-exclusive labels unless `multiSelect` is true.",
                                    "items": {
                                        "type": "object",
                                        "properties": {
                                            "label": {
                                                "type": "string",
                                                "description": "Button text the user clicks. 1–5 words; in the user's language."
                                            },
                                            "description": {
                                                "type": "string",
                                                "description": "Optional one-line explanation rendered under the button label — useful when the trade-offs aren't obvious from the label alone."
                                            }
                                        },
                                        "required": ["label"]
                                    }
                                },
                                "multiSelect": {
                                    "type": "boolean",
                                    "description": "When true, the user can pick multiple options before submitting. Default false."
                                }
                            },
                            "required": ["question", "options"]
                        }
                    }
                },
                "required": ["questions"]
            }),
        },
    ]
}

pub(super) fn todo_write_tools() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: tn::TODO_WRITE.to_string(),
            description: "Maintain your current todo list — a per-thread, user-visible list of items you're working through during a response. Use proactively on multi-step tasks (≥3 steps) so the user can watch your plan in real time. Replace-whole-list: every call carries the ENTIRE new list and discards the previous one. The empty list clears the panel.\n\nWorkflow: (1) Call once at the start with all items `pending`. (2) Before starting an item, re-call with that item flipped to `in_progress` — at most ONE item may be `in_progress` at a time. (3) After finishing, re-call with it `completed`. Always pass the whole list, including unchanged items.\n\nField guidance: `content` is the imperative form (\"Run tests\"). `active_form` is the present-continuous form (\"Running tests\") shown to the user only while the item is `in_progress`. Skip the tool entirely for trivial single-step requests.\n\nMax 50 items per call.\n\nIMPORTANT — abandonment: if you end your response with any item still `pending` or `in_progress`, the engine flips those items to `abandoned` so the user sees you walked away from them. To avoid that, either finish each item (mark `completed`) or call this tool with `[]` to drop the list explicitly. You cannot set `abandoned` yourself — it is engine-only.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "todos": {
                        "type": "array",
                        "description": "The whole new todo list. Pass `[]` to clear.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "content": {
                                    "type": "string",
                                    "description": "Imperative form, what the item is (\"Run tests\")."
                                },
                                "active_form": {
                                    "type": "string",
                                    "description": "Present-continuous form, shown while in_progress (\"Running tests\")."
                                },
                                "status": {
                                    "type": "string",
                                    "enum": ["pending", "in_progress", "completed"],
                                    "description": "At most ONE item may be `in_progress` at a time. (`abandoned` is engine-only — set automatically when you end a response with non-completed items.)"
                                }
                            },
                            "required": ["content", "active_form", "status"]
                        }
                    }
                },
                "required": ["todos"]
            }),
        },
    ]
}

/// Contract codegen for the `navigate_ui` `target` + `settings_view` enums.
///
/// Rust is the single source of truth (`NAVIGATE_TARGETS` /
/// `NAVIGABLE_SETTINGS_VIEWS`); the SDK `NavigateUi` TS types are GENERATED from
/// it into `packages/lucidos-sdk/src/generated/navigate-targets.ts`. Mirrors the
/// thread-lifecycle contract pattern in `thread_lifecycle_tests/contract.rs`:
/// `generated_navigate_targets_is_up_to_date` fails `cargo test` if the on-disk
/// file is stale, and the `#[ignore]` `generate_navigate_targets_file` rewrites
/// it. Edit the consts above, then regenerate.
#[cfg(test)]
mod navigate_targets_codegen {
    use super::*;

    /// Path to the generated SDK file. CARGO_MANIFEST_DIR is
    /// `<repo>/crates/lucidos-engine`, so two `.parent()`s reach the repo root
    /// (the SDK lives under `packages/`, not `crates/`).
    fn generated_path() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("packages/lucidos-sdk/src/generated/navigate-targets.ts")
    }

    fn push_const(out: &mut String, const_name: &str, type_name: &str, values: &[&str]) {
        out.push_str(&format!("export const {} = [\n", const_name));
        for v in values {
            out.push_str(&format!("  '{}',\n", v));
        }
        out.push_str("] as const;\n");
        out.push_str(&format!(
            "export type {} = (typeof {})[number];\n",
            type_name, const_name
        ));
    }

    fn generate_navigate_targets_ts() -> String {
        let mut out = String::new();
        out.push_str(
            "// AUTO-GENERATED by crates/lucidos-engine/src/llm/tools/misc.rs — do not edit by hand.\n",
        );
        out.push_str(
            "// Regenerate: cargo test -p lucidos-engine --lib generate_navigate_targets_file -- --ignored\n",
        );
        out.push_str("//\n");
        out.push_str(
            "// Source of truth for the `navigate_ui` contract: the `NAVIGATE_TARGETS` and\n",
        );
        out.push_str(
            "// `NAVIGABLE_SETTINGS_VIEWS` consts in that Rust file. The SDK `NavigateUi` type\n",
        );
        out.push_str(
            "// derives from this file, so the LLM tool schema and the SDK can never drift.\n\n",
        );
        push_const(
            &mut out,
            "NAVIGATE_TARGETS",
            "NavigateTarget",
            NAVIGATE_TARGETS,
        );
        out.push('\n');
        push_const(
            &mut out,
            "SETTINGS_VIEW_TARGETS",
            "SettingsViewTarget",
            NAVIGABLE_SETTINGS_VIEWS,
        );
        out
    }

    /// The tool schema's enums are built FROM the consts, so this guards only the
    /// wiring (a refactor that hardcodes an enum again would trip it).
    #[test]
    fn tool_schema_enums_match_consts() {
        let tool = get_navigate_ui_tool();
        let props = &tool.parameters["properties"];
        assert_eq!(
            props["target"]["enum"],
            serde_json::json!(NAVIGATE_TARGETS),
            "navigate_ui target enum drifted from NAVIGATE_TARGETS"
        );
        assert_eq!(
            props["settings_view"]["enum"],
            serde_json::json!(NAVIGABLE_SETTINGS_VIEWS),
            "navigate_ui settings_view enum drifted from NAVIGABLE_SETTINGS_VIEWS"
        );
    }

    #[test]
    fn generated_navigate_targets_is_up_to_date() {
        let generated = generate_navigate_targets_ts();
        let path = generated_path();
        match std::fs::read_to_string(&path) {
            Ok(existing) => assert_eq!(
                existing, generated,
                "Generated navigate-targets.ts is stale. Run: \
                 cargo test -p lucidos-engine --lib generate_navigate_targets_file -- --ignored"
            ),
            Err(_) => panic!(
                "Generated file missing at {}. Run: \
                 cargo test -p lucidos-engine --lib generate_navigate_targets_file -- --ignored",
                path.display()
            ),
        }
    }

    #[test]
    #[ignore]
    fn generate_navigate_targets_file() {
        let generated = generate_navigate_targets_ts();
        let path = generated_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &generated).unwrap();
        crate::log!("[ContractTest] Generated: {}", path.display());
    }
}
