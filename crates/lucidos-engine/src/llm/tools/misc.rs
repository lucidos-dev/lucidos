//! LLM-facing schemas for cross-cutting tools that do not belong to a
//! single domain family: navigate_ui, git_clone, get_backup_status,
//! request_credential, connect_oauth_account, execute_intent,
//! ask_user_question, await_event, todo_write.
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

/// Single source of truth for the `navigate_ui` `settings_view` enum: the
/// Settings sub-sections the agent may deep-link to. The SDK `SettingsViewTarget`
/// type is GENERATED from this list (same generated file as `NAVIGATE_TARGETS`).
/// Every value here must be a renderable subview in `SettingsView.renderSubview`,
/// and a frontend Vitest cross-check pins that.
///
/// Nearly every top-level Settings category and System sub-page. It used to omit
/// four categories for being platform-gated, which the LLM had no signal for.
/// None is gated now, only rows inside one, so nothing has to be withheld. See
/// `docs/plans/2026-08-05-settings-information-architecture.md`.
///
/// Two are still absent, because every value costs the always-loaded budget and
/// nothing has asked to link these: `webhooks` and `communication-surfaces`.
pub(crate) const NAVIGABLE_SETTINGS_VIEWS: &[&str] = &[
    "models",
    "permissions",
    "mcp",
    "coding-agents",
    "accounts",
    "locale",
    "marketplaces",
    "access",
    "devices",
    "system",
    // The Overview page: connection state, versions and maintenance. `system`
    // above is the submenu listing the sub-pages. So that one answers "open
    // System", and this one answers "what version is this".
    "system-overview",
    "appearance",
    "keyboard-shortcuts",
    "release-notices",
    "whats-new",
    "thread-queue",
    "backup",
    "memory",
    "disk-usage",
    "environment-variables",
    "debugging",
];

/// Tool for navigating the Lucidos UI to a specific panel, app, or file.
pub fn get_navigate_ui_tool() -> ToolDefinition {
    ToolDefinition {
        name: tn::NAVIGATE_UI.to_string(),
        description: "Navigate the Lucidos UI to a panel, app, file, thread, or creation form, when the user asks to open, show, or go to something.".to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "target": {
                    "type": "string",
                    "enum": NAVIGATE_TARGETS,
                    "description": "Most values need nothing else. Companion args: 'app' takes app_id, 'file' file_path, 'trigger' and 'thread' id, 'settings' settings_view, 'url' url. 'app-store' is the Plugins panel."
                },
                "settings_view": {
                    "type": "string",
                    "enum": NAVIGABLE_SETTINGS_VIEWS,
                    "description": "Only with target 'settings'; omitting it lands on the Settings home list. Non-obvious: 'models' also holds the current chat model, 'permissions' the command guard, 'coding-agents' binary paths and repositories, 'accounts' credentials and OAuth, 'access' remote reach, 'appearance' theme, font and scale, 'system-overview' versions and restart."
                },
                "app_id": {
                    "type": "string",
                    "description": "Required when target is 'app'."
                },
                "fragment": {
                    "type": "string",
                    "description": "A place inside the app; 'app' only. Arrives as its location.hash, so the app routes to that item. Pass it whenever you name one."
                },
                "file_path": {
                    "type": "string",
                    "description": "Required for 'file'. Path with its directory prefix (e.g. 'artifacts/notes.md'). In a registered repository clone: 'repo:<repoId>:file:<path>' at HEAD, or 'repo:<repoId>:file#<ref>:<path>' for a branch, tag or sha."
                },
                "line": {
                    "type": "integer",
                    "description": "1-based line; 'file' only. The preview scrolls to it and switches a rendered file to source view. Pass it whenever you cite a line."
                },
                "line_end": {
                    "type": "integer",
                    "description": "Last line of the range, inclusive. Omit to highlight only 'line'. 'file' only."
                },
                "id": {
                    "type": "string",
                    "description": "Required when target is 'thread' or 'trigger'."
                },
                "url": {
                    "type": "string",
                    "description": "Required when target is 'url'."
                },
                "event_id": {
                    "type": "string",
                    "description": "Event uuid inside the thread to scroll to and pulse on land. 'thread' only."
                },
                "prompt": {
                    "type": "string",
                    "description": "Draft text for the compose box. 'new-chat' only."
                }
            },
            "required": ["target"]
        }),
    }
}

/// The registry row wrapping [`get_navigate_ui_tool`], so the chat tail is a
/// gated table rather than a push.
pub(super) fn navigate_ui_tools() -> Vec<ToolDefinition> {
    vec![get_navigate_ui_tool()]
}

// `manage_repositories` and `manage_models` are now manifest-built grouped tools
// (see `crate::capability_manifest`, domains `repositories` / `models`). Their
// schemas come from the manifest (SSOT); `execute_tool` keeps routing the
// unchanged tool names to `execute_manage_repositories` / `execute_manage_models`.

pub(super) fn git_clone_tools() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: tn::GIT_CLONE.to_string(),
            description: "Clone a git repository. You must choose where it lands, there is no default: '.lucidos/tmp/<name>/' for inspect-then-discard work (ephemeral, gitignored), 'data/artifacts/imported/<name>/' for a persistent git-tracked dependency, which needs the user ASKED first. Over 500 files or 100 MB is refused for artifacts/imported/: extract only what the app needs, or move the dataset to ~/.lucidos/data/<name>/ (system-knowhow/best-practices rule 8).".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "Repository URL, https or ssh form."
                    },
                    "branch": {
                        "type": "string",
                        "description": "Branch to clone (defaults to the default branch)."
                    },
                    "destination": {
                        "type": "string",
                        "description": "REQUIRED: one of the two prefixes above. A bare name is rejected, so data/artifacts/ cannot bloat silently."
                    },
                    "include_patterns": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Globs to include, e.g. ['*.py', 'src/**/*.rs']."
                    },
                    "exclude_patterns": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Globs to exclude, e.g. ['node_modules/**']."
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
        description: "Read the workspace's backup status: schedule (user's timezone) and next run, provider and retention, the last run and its duration, recent history, and whether backups are stale. Read-only: change the schedule, provider and retention with set_preference (backup_schedule, backup_provider, backup_retention), and restore from the workspace picker.".to_string(),
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
            description: "Request an API credential through a secure modal, keeping the secret out of the conversation and the event log. ONE at a time: wait for each to resolve before requesting the next.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "service_name": {
                        "type": "string",
                        "description": "Service name (e.g. 'oura'). For 'oauth_client' pass the BARE provider name: the auth type marks the row as an app registration."
                    },
                    "prompt": {
                        "type": "string",
                        "description": "Shown in the modal, including where to find the credential."
                    },
                    "base_url": {
                        "type": "string",
                        "description": "Base URL for the API."
                    },
                    "auth_type": {
                        "type": "string",
                        "enum": ["api_key", "bearer", "basic", "password", "oauth_client"],
                        "description": "Default api_key. 'password' is username plus password, injected as Basic auth. PREFER connect_oauth_account over 'oauth_client', which does the same modal plus the authorize in one call. For 'oauth_client', load_knowhow('system-knowhow/oauth-providers') first and pass its endpoints below."
                    },
                    "auth_url": {
                        "type": "string",
                        "description": "oauth_client only, from the knowhow."
                    },
                    "token_url": {
                        "type": "string",
                        "description": "oauth_client only, from that knowhow."
                    },
                    "userinfo_url": {
                        "type": "string",
                        "description": "oauth_client only; without one the account reports no email."
                    },
                    "userinfo_method": {
                        "type": "string",
                        "enum": ["GET", "POST"],
                        "description": "oauth_client only. GET unless the knowhow's row says POST."
                    },
                    "authorize_params": {
                        "type": "string",
                        "description": "oauth_client only. Extra authorization-URL parameters from the knowhow row."
                    },
                    "scopes": {
                        "type": "string",
                        "description": "oauth_client only. Space-separated, pre-fills the modal."
                    },
                    "redirect_uri": {
                        "type": "string",
                        "description": "oauth_client only. Omit for the default loopback URI; the knowhow lists the other forms."
                    },
                    "env_var_name": {
                        "type": "string",
                        "description": "Extra env var name for the secret, alongside the default CRED_<NAME>. Must match [A-Z_][A-Z0-9_]* and not clobber an engine-owned name. Single-value auth types only."
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
            description: "Connect an OAuth account so Lucidos can call an API on the user's behalf. ONE call for the whole flow: with no client credentials yet it opens the credential modal itself, then authorizes. The page opens on the USER'S DEVICE in the browser they configured, so tell them to complete it there. Provider and scopes alone suffice for a provider the registry knows, otherwise load_knowhow('system-knowhow/oauth-providers').".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "provider": {
                        "type": "string",
                        "description": "Provider name (e.g. 'google'). Use a distinct name for a dedicated connection that must not carry other scopes, e.g. 'ghealth' on Google's endpoints."
                    },
                    "scopes": {
                        "type": "string",
                        "description": "Space-separated scopes."
                    },
                    "auth_url": {
                        "type": "string",
                        "description": "From the knowhow; pre-fills the modal."
                    },
                    "token_url": {
                        "type": "string",
                        "description": "From that knowhow."
                    },
                    "userinfo_url": {
                        "type": "string",
                        "description": "Without one the account reports no email."
                    },
                    "userinfo_method": {
                        "type": "string",
                        "enum": ["GET", "POST"],
                        "description": "GET unless the row says POST."
                    },
                    "authorize_params": {
                        "type": "string",
                        "description": "Extra authorization-URL parameters, key=value&key=value, from the knowhow row."
                    },
                    "base_url": {
                        "type": "string",
                        "description": "API base URL, pre-fills the modal."
                    },
                    "redirect_uri": {
                        "type": "string",
                        "description": "Omit for the default loopback URI; the knowhow names the other forms."
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
            description: "Execute a stored intent: what the user wants, paired with the knowhow for achieving it, both loaded automatically. It runs its own tool calls internally and returns only the final result.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "intent_id": {
                        "type": "string",
                        "description": "Intent id, e.g. 'job-search/find-jobs'."
                    },
                    "task": {
                        "type": "string",
                        "description": "What to do, e.g. 'Log today's sleep data'."
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
            description: "Ask a multiple-choice question and wait for the pick. Each renders as a card of buttons, one at a time; pass up to 4 only for a tight batch answered in sequence. Returns a JSON object mapping each question text to the chosen label, or to the user's typed text when they answer freeform.\n\nNEVER add an \"Other\" / \"Something else\" / \"Let me type it\" option: Lucidos has no text-entry option, so tapping one hands that label back as the answer, a dead end. Both escapes are already there, without you: the user can type in the prompt textarea and it arrives as their answer, and Cancel dismisses the question. An option carrying a decision you can act on is different and still welcome (\"None of these\").\n\nNever ask purely to get resumed while something you could subscribe to is pending.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "questions": {
                        "type": "array",
                        "minItems": 1,
                        "maxItems": 4,
                        "description": "One card at a time, answered in order.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "question": {
                                    "type": "string",
                                    "description": "REQUIRED, in the user's language. Never empty and never only in `header`: the engine rejects that and makes you re-ask."
                                },
                                "header": {
                                    "type": "string",
                                    "description": "Optional chip-label, 12 characters or fewer. Never a replacement for `question`."
                                },
                                "options": {
                                    "type": "array",
                                    "minItems": 2,
                                    "maxItems": 4,
                                    "description": "Mutually exclusive unless `multiSelect`.",
                                    "items": {
                                        "type": "object",
                                        "properties": {
                                            "label": {
                                                "type": "string",
                                                "description": "Button text, 1 to 5 words, in the user's language."
                                            },
                                            "description": {
                                                "type": "string",
                                                "description": "Optional one-line explanation under the label."
                                            }
                                        },
                                        "required": ["label"]
                                    }
                                },
                                "multiSelect": {
                                    "type": "boolean",
                                    "description": "When true the user can pick several before submitting. Default false."
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

/// One clause here is a **temporary measure**: "Saying you will re-arm is not
/// re-arming; a turn that ends with no new call leaves nothing watching for it"
/// carries no system fact and exists only to pre-empt a recurring model mistake
/// on this terminal tool. Registered in `docs/temporary-measures.md` § "\"Narrating it
/// does not do it\" on an event-wait re-arm", alongside its twin in
/// `engine::event_wait::WAIT_SPENT_NOTICE`. Everything else in the description
/// states real properties of the tool.
pub(super) fn await_event_tools() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: tn::AWAIT_EVENT.to_string(),
            description: format!("Subscribe to Lucidos state instead of checking over and over: a thread you did not spawn finishing, a change proposed, a trigger firing, a backup finishing, a domain event. The engine re-opens this thread with a NEW turn when a match arrives, or on `timeout_secs`.\n\nIT WATCHES FORWARD ONLY, so if the thing might already be in the past, still check state before subscribing. What you do NOT have to worry about is the race between that check and this call: a match from the few minutes just before it is named in the result with its age. READ THAT PART and act on it in THIS turn, because it is a report, not a delivery.\n\nMATCHING IS WORKSPACE-WIDE, so any thread's `ChildThreadCompleted` is a real wait whoever spawned it: name it with a `child_thread_id` condition. NOT YOUR OWN CHILD'S: that already re-opens this thread, so a wait duplicates it.\n\nTHE SUBSCRIPTION IS SPENT once it delivers, so if you want the next one too, call this again before that turn ends. Saying you will re-subscribe is not re-subscribing. A user message is different: every subscription survives it untouched, so do not register those again.\n\nAfter {} subscriptions in a row with no message from the user the next call is refused, so never promise to watch \"forever\".", crate::engine::event_wait::MAX_CONSECUTIVE_SUBSCRIPTIONS),
            parameters: json!({
                "type": "object",
                "properties": {
                    "on": {
                        "type": "array",
                        "minItems": 1,
                        "description": "Any match delivers, and the result names which fired. Same shape as a trigger's `on_event`.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "event_type": {
                                    "type": "string",
                                    "description": "PascalCase past tense: a thread event (`ChangeProposed`, `CodingAgentIdled`, `ChildThreadCompleted`, …), a persisted system event (`BackupCompleted`, `TriggerCompleted`, …), or a domain event this workspace emits. Refused: streaming events, the `EventWait*` family, and transient frames that write no event row (`BackupProgress`)."
                                },
                                "condition": {
                                    "type": "object",
                                    "description": "Optional filter on THIS entry. A key is a FIELD PATH, so dots read one level down (`workflow_run.event`) and a missing path is null. A value for equality, or an operator object (`$eq`, `$ne`, `$lt`, `$lte`, `$gt`, `$gte`, `$in`, `$nin`, `$regex`). `$or` takes a list of conditions. The event's OWN payload fields, plus `thread_id`, which scopes ANY thread event to one thread: `CodingAgentIdled` with `{\"thread_id\": \"<uuid>\"}` is one session finishing."
                                }
                            },
                            "required": ["event_type"]
                        }
                    },
                    "timeout_secs": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 86400,
                        "description": "REQUIRED. Seconds before giving up; there is no unbounded wait. Add margin: expiring early costs one turn, expiring late costs the whole wait."
                    },
                    "reason": {
                        "type": "string",
                        "description": "REQUIRED. One short line in the user's language saying what you await; it shows in the waiting indicator."
                    }
                },
                "required": ["on", "timeout_secs", "reason"]
            }),
        },
    ]
}

/// The other two verbs on a thread's own subscriptions: read them, and stand
/// them down. Siblings of `await_event`, which arms them.
///
/// Both are scoped to the calling thread and take no thread id, so an agent
/// cannot read or end another thread's subscriptions. See
/// `engine::event_wait::agent_surface` for why each exists.
pub(super) fn event_wait_agent_tools() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: tn::LIST_EVENT_WAITS.to_string(),
            description: "What THIS thread is subscribed to right now: each subscription's id, what it watches, your reason, when you armed it, and when it times out. CALL IT BEFORE TELLING THE USER WHETHER YOU ARE STILL WATCHING: a spend, a timeout and a user pressing Stop waiting all land while you are not running. It is also where the `wait_id` comes from.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
        },
        ToolDefinition {
            name: tn::CANCEL_EVENT_WAIT.to_string(),
            description: "Stand down a subscription this thread armed with `await_event`: one by `wait_id`, the ones watching an event type with `on`, or all with `all: true`. THIS IS HOW YOU STOP WATCHING, and one you leave live re-opens this thread later whatever you told the user. Stopping is silent, so carry straight on in this turn. Pass exactly one argument.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "wait_id": {
                        "type": "string",
                        "description": "The subscription to stop, from `list_event_waits`. Omit when passing `on` or `all`."
                    },
                    "on": {
                        "type": "string",
                        "description": "Stop the ones watching this event type, no id needed. Use it over `all` when you got the answer about one thing and are still waiting on others."
                    },
                    "all": {
                        "type": "boolean",
                        "description": "Stop every live subscription on this thread. Prefer `wait_id` or `on` when only some of them are meant."
                    }
                },
                "required": []
            }),
        },
    ]
}

/// The todo list, everywhere the *self-curated context mode* is off.
///
/// Under the mode the checklist lives in the working understanding, under a
/// `[TODO]` heading in the model's own reply. Offering this schema as well
/// would be two write surfaces for one list, which is the cost bug twice over.
/// So the mode-on array is a strict subset of the mode-off one, and the mode
/// adds no tool at all.
pub(super) fn todo_write_tools(caps: &crate::llm::ToolCapabilities) -> Vec<ToolDefinition> {
    if caps.context_mode {
        return Vec::new();
    }
    let parameters = json!({
        "type": "object",
        "properties": {
            "todos": {
                "type": "array",
                "description": "The whole new list. `[]` clears.",
                "items": {
                    "type": "object",
                    "properties": {
                        "content": {
                            "type": "string",
                            "description": "Imperative form (\"Run tests\")."
                        },
                        "active_form": {
                            "type": "string",
                            "description": "Present-continuous form, shown while in_progress (\"Running tests\")."
                        },
                        "status": {
                            "type": "string",
                            "enum": ["pending", "in_progress", "completed"],
                            "description": "At most ONE item may be `in_progress`."
                        }
                    },
                    "required": ["content", "active_form", "status"]
                }
            }
        },
        "required": ["todos"]
    });

    vec![ToolDefinition {
        name: tn::TODO_WRITE.to_string(),
        description: "Maintain your todo list: a per-thread, user-visible list of items you are working through during a response, rendered in the prompt bar. Replace-whole-list, so every call carries the ENTIRE new list; `[]` clears it. Max 50 items, at most ONE `in_progress`. AT RESPONSE END the engine settles every unfinished item: `waiting` if you still hold an event wait, else `abandoned` (you walked away). Work you finish after a settle still shows `abandoned` until you call this again.".to_string(),
        parameters,
    }]
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
