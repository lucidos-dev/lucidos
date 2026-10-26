//! LLM-facing schemas for cross-cutting tools that do not belong to a
//! single domain family: navigate_ui, git_clone, get_backup_status,
//! request_credential, connect_oauth_account,
//! ask_user_question, await_event, list_event_waits, cancel_event_wait,
//! request_read, todo_write.
//!
//! (manage_repositories / manage_models moved to the capability parity manifest —
//! domains `repositories` / `models` — and are built by `capability_manifest`.)

use crate::llm::provider::ToolDefinition;
use crate::llm::tool_names as tn;
use crate::runtime::agent_runtime::CodingAgent;
use serde_json::json;

/// Single source of truth for the `navigate_ui` `target` enum — the top-level
/// navigation destinations. The SDK `NavigateTarget` type is GENERATED from this
/// list into `packages/lucidos-sdk/src/generated/navigate-targets.ts`, so the
/// LLM tool schema and the SDK can never drift. To change the set, edit here.
const NAVIGATE_TARGETS: &[&str] = &[
    "dashboard",
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
    "intelligence",
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

/// The `device` argument of `navigate_ui` and `capture_app`, which one
/// resolver settles (`engine/tools/navigate.rs`).
pub(super) const DEVICE_ARG_DESCRIPTION: &str =
    "A Known devices id. Omit for the user's last used device.";

/// Tool for navigating the Lucidos UI to a specific panel, app, or file.
pub(super) fn get_navigate_ui_tool() -> ToolDefinition {
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
                    "description": "Required for 'file'. Path with its directory prefix (e.g. 'artifacts/notes.md'). In a registered repository clone: 'repo:<name or id>:file:<path>' at HEAD; 'file#<ref>' in place of 'file' for a branch, tag or sha."
                },
                "line": {
                    "type": "integer",
                    "description": "1-based line; 'file' only. The preview scrolls to it and switches a rendered file to source view. Pass it whenever you cite a line."
                },
                "line_end": {
                    "type": "integer",
                    "description": "Last line of the range, inclusive. Omit to highlight only 'line'. 'file' only."
                },
                // A thread's id takes the `current` alias or a uuid, and the
                // handler refuses anything else. The refusal is what says so,
                // not this description, which the always-loaded budget bills on
                // every request. See
                // docs/plans/2026-09-18-notification-tap-thread-id-is-a-uuid.md
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
                },
                "coding_agent": {
                    "type": "string",
                    "enum": CodingAgent::ALL.map(CodingAgent::as_str),
                    "description": "'new-chat' only."
                },
                "folder": {
                    "type": "string",
                    "description": "Repository name or id, or `data/apps/<id>`. 'new-chat' only."
                },
                "device": {
                    "type": "string",
                    "description": DEVICE_ARG_DESCRIPTION
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

/// The six OAuth endpoint properties `request_credential` and
/// `connect_oauth_account` both take, filled from the same
/// `system-knowhow/oauth-providers` row.
///
/// One copy, because the two had already drifted: only one of them named the
/// `authorize_params` separator, so a model on the other path had to guess it.
/// `prefix` leads every description on a tool that also accepts other auth
/// types, which is how `request_credential` still says where they apply.
fn oauth_endpoint_props(prefix: &str) -> serde_json::Map<String, serde_json::Value> {
    let text = |body: &str| json!({ "type": "string", "description": format!("{prefix}{body}") });
    [
        ("auth_url", text("From the knowhow; pre-fills the modal.")),
        ("token_url", text("From that knowhow.")),
        (
            "userinfo_url",
            text("Without one the account reports no email."),
        ),
        (
            "userinfo_method",
            json!({
                "type": "string",
                "enum": ["GET", "POST"],
                "description": format!("{prefix}GET unless the knowhow's row says POST."),
            }),
        ),
        (
            "authorize_params",
            text("Extra authorize-URL params, key=value&key=value, from the knowhow."),
        ),
        (
            "redirect_uri",
            text("Omit for the default loopback URI; the knowhow lists the rest."),
        ),
    ]
    .into_iter()
    .map(|(name, schema)| (name.to_string(), schema))
    .collect()
}

/// A tool's own properties with the shared OAuth endpoint block merged in.
/// The tool's own names win, so a tool can still describe one of the six
/// differently without losing the other five.
fn with_oauth_endpoints(properties: serde_json::Value, prefix: &str) -> serde_json::Value {
    let mut merged = oauth_endpoint_props(prefix);
    if let serde_json::Value::Object(own) = properties {
        merged.extend(own);
    }
    serde_json::Value::Object(merged)
}

pub(super) fn request_credential_tools() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: tn::REQUEST_CREDENTIAL.to_string(),
            description: "Request an API credential through a secure modal that keeps the secret out of the conversation and event log. ONE at a time: wait for each to resolve. Refused when a configured builtin provider proxy holds its key (e.g. 'openai', 'gpt'): call proxy_request instead. A non-token auth_type, or a host outside the proxy's base, proceeds.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": with_oauth_endpoints(json!({
                    "service_name": {
                        "type": "string",
                        "description": "Service name (e.g. 'oura'). For 'oauth_client' pass the BARE provider name: the auth type marks the row as an app registration."
                    },
                    "prompt": {
                        "type": "string",
                        "description": "Shown in the modal, including where to find the credential."
                    },
                    "base_urls": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Every host it may be sent to; an API host and a git host differ. Required except for 'secret'. A host an existing credential misses reopens that one to widen, never a second credential."
                    },
                    "auth_type": {
                        "type": "string",
                        "enum": crate::core::AuthType::agent_requestable_values(),
                        "description": "Default api_key. 'password' is username plus password, injected as Basic auth. 'secret' is signed with rather than sent: no base_url, no host, read by scripts as CRED_<NAME>. PREFER connect_oauth_account over 'oauth_client'. It opens the client modal; call it again after completion to authorize. For 'oauth_client', load_knowhow('system-knowhow/oauth-providers') first and pass its endpoints below."
                    },
                    "env_var_name": {
                        "type": "string",
                        "description": "Extra env var besides CRED_<NAME>, given only to a command naming it (`gh pr list  # GITHUB_TOKEN`). Must match [A-Z_][A-Z0-9_]*, not engine-owned. Single-value auth types only."
                    }
                }), "oauth_client: "),
                // `base_urls` is required for every type but `secret`, which is
                // sent nowhere. A JSON Schema cannot say that, so the handler
                // enforces it and names the type in its refusal.
                "required": ["service_name", "prompt", "auth_type"]
            }),
        },
    ]
}

pub(super) fn connect_oauth_tools() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: tn::CONNECT_OAUTH_ACCOUNT.to_string(),
            description: "Connect an OAuth account so Lucidos can call an API on the user's behalf. With no client credentials it opens a credential modal. Wait for completed, then call again to authorize. The page opens on the USER'S DEVICE in the browser they configured, so tell them to complete it there. Provider and scopes alone suffice for a provider the registry knows, otherwise load_knowhow('system-knowhow/oauth-providers').".to_string(),
            parameters: json!({
                "type": "object",
                "properties": with_oauth_endpoints(json!({
                    "provider": {
                        "type": "string",
                        "description": "Provider name (e.g. 'google'). Use a distinct name for a dedicated connection that must not carry other scopes, e.g. 'ghealth' on Google's endpoints."
                    },
                    "scopes": {
                        "type": "string",
                        "description": "Space-separated scopes."
                    },
                    "base_url": {
                        "type": "string",
                        "description": "API base URL, pre-fills the modal."
                    }
                }), ""),
                "required": ["provider", "scopes"]
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
                    "message": {
                        "type": "string",
                        "description": "Markdown shown in full as your reply, above the first card: the answer, steps, links, pictures or draft it follows."
                    },
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
                                    "description": "REQUIRED, in the user's language: the full text shown on the card. Never empty: the engine rejects that and makes you re-ask. Markdown: short paragraphs or a list, the question last."
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
                                                "description": "Optional inline markdown under the label: a one-line explanation, this option's own picture `![A](artifacts/a.png)`, or a widget embed `![A](app:<id>?params={…})`."
                                            },
                                            "quiet": {
                                                "type": "boolean",
                                                "description": "True when this pick needs nothing more from you (\"Not now\"): it ends your turn, no reply."
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

/// One clause here is a **temporary measure**: "Saying you will re-subscribe is
/// not re-subscribing" carries no system fact and exists only to pre-empt a
/// recurring model mistake on this tool. Registered in `docs/temporary-measures.md` § "\"Narrating it
/// does not do it\" on an event-wait re-arm", alongside its twin in
/// `engine::event_wait::WAIT_SPENT_NOTICE`. Everything else in the description
/// states real properties of the tool.
pub(super) fn await_event_tools() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: tn::AWAIT_EVENT.to_string(),
            description: format!("Subscribe to Lucidos state instead of polling it: a thread you did not spawn finishing, a change proposed, a trigger firing, a backup finishing, a domain event. The engine re-opens this thread with a NEW turn when a match arrives, or on `timeout_secs`.\n\nIT WATCHES FORWARD ONLY, so if the thing might already be in the past, still check state before subscribing. What you do NOT have to worry about is the race between that check and this call: a match from the few minutes just before it is named in the result with its age. READ THAT PART and act on it in THIS turn, because it is a report, not a delivery.\n\nMATCHING IS WORKSPACE-WIDE, so any thread's `ChildThreadCompleted` is a real wait whoever spawned it: name it with a `child_thread_id` condition, plus `ChildReportFiled`. NOT YOUR OWN CHILD'S: it reports here on its own, so a wait duplicates it.\n\nTHE SUBSCRIPTION IS SPENT once it delivers, so if you want the next one too, call this again before that turn ends. Saying you will re-subscribe is not re-subscribing. A user message is different: every subscription survives it untouched, so do not register those again.\n\nAfter {} subscriptions in {} minutes without a user reply the next call is refused, so never promise to watch \"forever\".", crate::engine::event_wait::MAX_RECENT_SUBSCRIPTIONS, crate::engine::event_wait::RECENT_SUBSCRIPTION_WINDOW_SECS / 60),
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
                        "maximum": crate::engine::event_wait::MAX_TIMEOUT_SECS,
                        "description": "REQUIRED. Seconds before giving up; there is no unbounded wait. Add margin: expiring early costs one turn, expiring late costs the whole wait."
                    },
                    "reason": {
                        "type": "string",
                        "description": "REQUIRED. What you await, as a short noun phrase in the user's language; shown after 'Waiting for'."
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

/// The argument `request_read` takes, and the read-request route's body field.
pub(crate) const READ_ARG: &str = "read";

/// The *read decision* (ADR 0409, ADR 0417). Scoped to the calling thread and
/// takes no thread id, so an agent can only decide for its own reply.
pub(super) fn request_read_tools() -> Vec<ToolDefinition> {
    vec![ToolDefinition {
        name: tn::REQUEST_READ.to_string(),
        description: "Your read decision on this turn's reply, when sure. read: true when it holds what the user asked for (findings) or they will read it later: Review lists it until seen. read: false for small talk, a confirmation, a ready or applied change, a kickoff, or progress while sub-threads or event waits run. Unsure or asking a question: skip it. It does not notify.".to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                (READ_ARG): {
                    "type": "boolean",
                    "description": "Whether the user should read this reply."
                }
            },
            "required": [READ_ARG]
        }),
    }]
}

/// The argument `wake_parent` takes, and the wake-decision route's body field.
pub(crate) const WAKE_ARG: &str = "wake";

/// The *wake decision*, for a thread with a parent it reports to. Scoped to the
/// calling thread and takes no thread id, like `request_read`.
pub(crate) fn wake_parent_tool() -> ToolDefinition {
    ToolDefinition {
        name: tn::WAKE_PARENT.to_string(),
        description: "Your wake decision on this turn's report to your parent thread, when sure. wake: true when your parent must act on it now: it waits for this result to take a next step, combine results, retry, or decide something only it can. wake: false when the report only informs: the user reads your findings here, and your parent reads the report on its next turn. A failure always wakes it. Unsure, skip it: Lucidos decides, and a doubt wakes.".to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                (WAKE_ARG): {
                    "type": "boolean",
                    "description": "Whether your parent thread must act on this turn's report now."
                }
            },
            "required": [WAKE_ARG]
        }),
    }
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
            // `todos` is required to write and meaningless to read, which no
            // JSON Schema can say. So it is absent from `required` below and
            // the handler enforces it, naming `read` in the refusal. Same
            // shape as `request_credential`'s `base_urls`.
            "action": {
                "type": "string",
                "enum": ["write", "read"],
                "description": "Default `write`, which replaces the list and needs `todos`. `read` answers with the current list and changes nothing."
            },
            "todos": {
                "type": "array",
                "description": "The whole new list. `[]` clears. Required to write.",
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
        "required": []
    });

    vec![ToolDefinition {
        name: tn::TODO_WRITE.to_string(),
        description: "Maintain your todo list: a per-thread, user-visible list of items you are working through during a response, rendered in the prompt bar. Replace-whole-list, so every call carries the ENTIRE new list; `[]` clears it. Max 50 items, at most ONE `in_progress`. EVERY CALL ANSWERS WITH THE RESULTING LIST, which is the same list the user sees. AT RESPONSE END the engine settles every unfinished item: `waiting` if you still hold an event wait, else `abandoned` (you walked away). Work you finish after a settle still shows `abandoned` until you write the list again, so re-read it with `action: \"read\"` before telling the user where a plan stands.".to_string(),
        parameters,
    }]
}

#[cfg(test)]
mod oauth_endpoint_tests {
    use super::*;

    fn only_tool(mut tools: Vec<ToolDefinition>) -> ToolDefinition {
        assert_eq!(tools.len(), 1);
        tools.remove(0)
    }

    /// Both OAuth-capable tools describe the same six endpoint properties, from
    /// one source. The two hand-written copies had already drifted: only
    /// `connect_oauth_account` named the `authorize_params` separator, so a
    /// model taking the `request_credential` path had to guess it.
    #[test]
    fn both_oauth_tools_read_the_same_endpoint_block() {
        let request = only_tool(request_credential_tools());
        let connect = only_tool(connect_oauth_tools());
        for name in [
            "auth_url",
            "token_url",
            "userinfo_url",
            "userinfo_method",
            "authorize_params",
            "redirect_uri",
        ] {
            let requested = request.parameters["properties"][name]["description"]
                .as_str()
                .unwrap_or_else(|| panic!("request_credential dropped {name}"));
            let connected = connect.parameters["properties"][name]["description"]
                .as_str()
                .unwrap_or_else(|| panic!("connect_oauth_account dropped {name}"));
            assert_eq!(
                requested.strip_prefix("oauth_client: "),
                Some(connected),
                "{name} must read the same on both tools"
            );
        }
        assert!(
            request.parameters["properties"]["authorize_params"]["description"]
                .as_str()
                .is_some_and(|d| d.contains("key=value")),
            "the wire format the model needs must reach both paths"
        );
    }
}

/// The SDK's `NavigateUi` types, rendered from `NAVIGATE_TARGETS` and
/// `NAVIGABLE_SETTINGS_VIEWS` into
/// `packages/lucidos-sdk/src/generated/navigate-targets.ts`. Edit the consts
/// above, never the output.
pub(crate) mod navigate_targets_codegen {
    use super::*;
    use crate::codegen::REGENERATE;

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

    pub(crate) fn render() -> String {
        let mut out = String::new();
        out.push_str(
            "// AUTO-GENERATED by crates/lucidos-engine/src/llm/tools/misc.rs. Do not edit by hand.\n",
        );
        out.push_str(&format!("// Regenerate: {REGENERATE}\n"));
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
        out.push('\n');
        push_const(
            &mut out,
            "CODING_AGENT_TARGETS",
            "CodingAgentTarget",
            &CodingAgent::ALL.map(CodingAgent::as_str),
        );
        out
    }

    #[cfg(test)]
    mod tests {
        use super::*;

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

        /// A new-chat navigate preselects the destination with the same field
        /// names and backend values as `run_coding_agent`.
        #[test]
        fn new_chat_destination_args_match_run_coding_agent() {
            let navigate = get_navigate_ui_tool();
            let spawn = crate::llm::tools::threads::spawn_tools()
                .into_iter()
                .find(|t| t.name == tn::RUN_CODING_AGENT)
                .expect("run_coding_agent is registered");
            let (nav, run) = (
                &navigate.parameters["properties"],
                &spawn.parameters["properties"],
            );
            assert_eq!(nav["coding_agent"]["enum"], run["coding_agent"]["enum"]);
            assert_eq!(nav["folder"]["type"], run["folder"]["type"]);
        }
    }
}
