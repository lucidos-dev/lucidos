//! `lucidos` — CLI for coding-agent subprocesses to talk back to the parent
//! Lucidos workspace.
//!
//! ## Workspace resolution
//!
//! Subcommands need to know which workspace they belong to. Order:
//!   1. `$LUCIDOS_WORKSPACE` env var (engine sets this on every spawned
//!      subprocess and it is authoritative).
//!   2. Walk up from `$PWD` looking for the first `.lucidos/ports` file —
//!      fallback for terminal users who run the CLI without the env var.
//!
//! See `workspace::resolve` for why the order matters.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand, ValueEnum};

mod ask_user_question_hook;
mod cc_bash_guard;
mod cc_plan_gate;
mod cc_read_coerce;
mod cc_stop_reminder;
mod changes;
mod coding_agent_diff_hook;
mod data;
mod data_store;
mod events;
/// Manifest-generated subcommands (one per `cli = true` capability domain).
/// AUTO-GENERATED content; wire each enum below. See
/// `crates/lucidos-engine/src/capability_manifest/`.
mod generated;
mod hardened;
mod http;
mod knowhow;
mod mcp_permission_server;
mod notify;
mod planned;
mod proxy;
mod spawn_thread;
mod threads;
mod workspace;

use data::WriteSource;
use workspace::resolve_from_env;

#[derive(Parser)]
#[command(
    name = "lucidos",
    version,
    about = "Talk back to the parent Lucidos workspace from a coding-agent subprocess.",
    long_about = "Resolves the parent workspace from $LUCIDOS_WORKSPACE if set, \
                  else walks up from $PWD for the first .lucidos/ports file."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Operations on the parent workspace's `data/` directory.
    Data {
        #[command(subcommand)]
        action: DataCmd,
    },
    /// Manage the cross-workspace persistent data store at `~/.lucidos/data/`.
    /// Use this for bulk reference corpora the user wants to keep but should
    /// not live inside any single workspace (system-knowhow/best-practices
    /// rule 8).
    #[command(name = "data-store")]
    DataStore {
        #[command(subcommand)]
        action: DataStoreCmd,
    },
    /// Emit and query domain events on the parent workspace's event store.
    Events {
        #[command(subcommand)]
        action: EventsCmd,
    },
    /// Record/query hardening state. Invoked by the `mark-harden.sh` hook.
    Hardened {
        #[command(subcommand)]
        action: HardenedCmd,
    },
    /// Record/query the durable Planned marker that enforces the
    /// `implementation-plan` skill. `mark --plan <docs/plans/file>` records a
    /// real plan (the skill calls this); `mark --simple "<reason>"`
    /// acknowledges a local fix that needs no plan; `state` prints
    /// `PRESENT`/`MISSING`. Both marked states satisfy the pre-edit gate and
    /// the Apply floor.
    Planned {
        #[command(subcommand)]
        action: PlannedCmd,
    },
    /// MCP stdio server invoked by Claude Code as `--permission-prompt-tool`.
    /// Forwards each permission request to the parent engine and blocks on the
    /// user's decision. Hidden from `--help`; not meant for direct invocation.
    #[command(name = "mcp-permission-server", hide = true)]
    McpPermissionServer,
    /// PreToolUse hook subcommand invoked by Claude Code via .lucidos/cc-settings.json
    /// when the AskUserQuestion tool fires. Reads the hook payload from stdin,
    /// long-polls the parent engine for the user's answer, prints the protocol-required
    /// updatedInput JSON to stdout. Hidden; not for direct invocation.
    #[command(name = "ask-user-question-hook", hide = true)]
    AskUserQuestionHook,
    /// Stop hook subcommand invoked by Claude Code via .lucidos/cc-settings.json
    /// when CC tries to idle. If the branch has commits but no harden marker,
    /// prints a permissive reminder JSON so CC nudges the model to run /harden.
    /// Hidden; not for direct invocation.
    #[command(name = "cc-stop-reminder", hide = true)]
    CcStopReminder,
    /// PreToolUse hook subcommand invoked by Claude Code via .lucidos/cc-settings.json
    /// for every Bash tool call. Refuses kill patterns that would catch sibling CC
    /// subprocesses by accident (e.g. `ps | grep cargo | xargs kill` matches every
    /// claude argv). Reads the hook payload from stdin; exits 2 + stderr to block.
    /// Hidden; not for direct invocation.
    #[command(name = "cc-bash-guard", hide = true)]
    CcBashGuard,
    /// PreToolUse hook subcommand invoked by Claude Code via .lucidos/cc-settings.json
    /// for every Read tool call. Coerces string-typed `offset` / `limit` fields to
    /// numbers via `updatedInput`, working around a model habit that otherwise
    /// trips CC's input validator. No-op (silent) when the input is already
    /// well-shaped. Hidden; not for direct invocation.
    #[command(name = "cc-read-coerce", hide = true)]
    CcReadCoerce,
    /// PreToolUse hook subcommand invoked by Claude Code via .lucidos/cc-settings.json
    /// for every Edit/Write tool call. Asks the engine whether this branch has a
    /// Planned marker; if not (and the worktree ships the implementation-plan
    /// skill), returns `permissionDecision: "deny"` instructing the model to run
    /// the skill or acknowledge a local fix. Exempts `docs/plans/` writes and is
    /// a silent no-op in repos without the skill. Hidden; not for direct invocation.
    #[command(name = "cc-plan-gate", hide = true)]
    CcPlanGate,
    /// Git post-commit hook subcommand installed in coding-agent worktrees.
    /// Refreshes the parent engine's `coding_agent_has_diff` projection so the
    /// Diff button appears as soon as committed work exists. Hidden; not for
    /// direct invocation.
    #[command(name = "coding-agent-diff-hook", hide = true)]
    CodingAgentDiffHook,
    /// POST a new chat or coding-agent thread to another (or this same) Lucidos
    /// workspace. Defaults caller_* fields from $LUCIDOS_WORKSPACE,
    /// $LUCIDOS_THREAD_ID, $LUCIDOS_EVENT_ID. With `--parent`, emits
    /// parent_thread_id/spawning_event_id instead (same-workspace callback).
    /// `--repo <name>` defaults from $LUCIDOS_REPO so a coding-agent subprocess
    /// inherits the calling thread's repo without callers passing it explicitly.
    /// `--folder <path>` instead targets an app folder (`data/apps/<id>`),
    /// spawning an app coding-agent thread (mutually exclusive with `--repo`;
    /// requires `--cc`, `--codex`, or `--coding-agent`).
    #[command(name = "spawn-thread")]
    SpawnThread(SpawnThreadArgs),
    /// Call a backend configured in `data/config/apis.json` through the
    /// engine's proxy (engine injects the configured auth header). Body to
    /// stdout; exit 0 by default even on 4xx/5xx. Use `--fail` to mirror
    /// `curl --fail`, `--include` to mirror `curl -i`.
    Proxy(ProxyCliArgs),
    /// Send a push notification via the parent workspace. Persists to the
    /// inbox AND pushes to subscribed devices, identical to the
    /// `send_notification` LLM tool. Use from scripts that need to nudge the
    /// user without going through an LLM thread.
    Notify(NotifyArgs),
    /// Query thread summaries on the parent workspace. The same shape returned
    /// by `GET /api/v1/threads/list` and the `list_threads` LLM tool — a flat
    /// newest-first list of every thread (with optional filters), distinct
    /// from the UI-shaped `/api/v1/threads`.
    Threads {
        #[command(subcommand)]
        action: ThreadsCmd,
    },
    /// Operations on pending / applied changes: `list` and `apply`. `apply`
    /// wraps `POST /api/v1/changes/<id>/apply`, forwarding the subprocess-origin
    /// headers via `client()` so the engine stamps the resulting
    /// `ChangeApplied` as `Api { mode: Agent }` instead of `Api { mode: Human }`
    /// (which the UI renders as "You"). Hand-rolled urllib / curl from inside
    /// a `run_python` / `run_bash` tool loses those headers — use this
    /// subcommand instead. `list` GETs `/api/v1/changes` so a script can find
    /// the pending change id without guessing a command or scanning events.
    Changes {
        #[command(subcommand)]
        action: ChangesCmd,
    },
    /// Read the engine-shipped system-knowhow corpus (and any user knowhow):
    /// `list` the catalog, `read <id>` one doc's full content. The authoritative
    /// guides for building Lucidos apps (`system-knowhow/building-an-app`,
    /// `system-knowhow/js-sdk`, `system-knowhow/best-practices`, …) live in the
    /// engine, NOT in an app coding-agent thread's sparse-checkout worktree — so
    /// this is how a session pulls them on demand. Mirrors the chat agent's
    /// `load_knowhow` tool over HTTP.
    Knowhow {
        #[command(subcommand)]
        action: KnowhowCmd,
    },
    /// Read and clear the notification inbox (`list` / `read --id <uuid>` /
    /// `read-all`). Generated from the capability parity manifest and routed
    /// through the gateway-safe HTTP client — use this instead of hand-rolled
    /// `curl` (which has to reverse-engineer the engine port + gateway prefix).
    /// Mirrors the chat agent's grouped `notifications` tool.
    Notifications {
        #[command(subcommand)]
        action: generated::NotificationsCmd,
    },
    /// Read and change user preferences (`get` / `set --key K --value V`).
    /// Generated from the capability parity manifest; routed through the
    /// gateway-safe HTTP client. Mirrors the chat agent's grouped `preferences`
    /// tool and the SDK `lucidos.preferences` namespace.
    Preferences {
        #[command(subcommand)]
        action: generated::PreferencesCmd,
    },
    /// Create and manage triggers — scheduled (cron) and/or event-driven
    /// automations (`create` / `list` / `update` / `delete`). Generated from the
    /// capability parity manifest; routed through the gateway-safe HTTP client.
    /// Mirrors the chat agent's grouped `triggers` tool and the SDK
    /// `lucidos.triggers` namespace. (Pause/resume a trigger via `update --id
    /// <uuid> --paused true|false`.)
    Triggers {
        #[command(subcommand)]
        action: generated::TriggersCmd,
    },
    /// Manage trigger groups — the user-visible folders that organize triggers
    /// in the panel (`list` / `create` / `rename` / `reorder` / `delete`).
    /// Generated from the capability parity manifest; routed through the
    /// gateway-safe HTTP client. Mirrors the chat agent's grouped
    /// `trigger_groups` tool.
    #[command(name = "trigger-groups")]
    TriggerGroups {
        #[command(subcommand)]
        action: generated::TriggerGroupsCmd,
    },
    /// Manage apps — `list` all apps, `get` one by id, `update` an app's
    /// name/description, or `delete` an app. Generated from the capability
    /// parity manifest; routed through the gateway-safe HTTP client. (Creating
    /// an app is the chat agent's `create_app` tool; editing app source is done
    /// in the app's coding-agent worktree.)
    Apps {
        #[command(subcommand)]
        action: generated::AppsCmd,
    },
    /// Inspect the Thread Queue (background admission control) — `list` the live
    /// queue + capacity policy, force-admit a queued entry with `run-now
    /// --entry-id <uuid>`, or `drop --entry-id <uuid>`. Generated from the
    /// capability parity manifest; routed through the gateway-safe HTTP client.
    /// (Mirrors the chat agent's grouped `thread_queue` tool. Changing the
    /// capacity policy is the LLM tool's `update_policy` action — kept off the
    /// CLI because the raw HTTP PUT would reset omitted caps to defaults.)
    #[command(name = "thread-queue")]
    ThreadQueue {
        #[command(subcommand)]
        action: generated::ThreadQueueCmd,
    },
    /// Read long-term memory — `stats` (index counts), `entries` (paginated
    /// long-term-memory entries), or `source` (the originating event/artifact for
    /// one memory). Generated from the capability parity manifest; routed through
    /// the gateway-safe HTTP client. (Correcting memory is the chat agent's
    /// grouped `memory` tool; reading is the agent's injected context.)
    Memory {
        #[command(subcommand)]
        action: generated::MemoryCmd,
    },
    /// Manage non-secret environment variables injected into every subprocess
    /// Lucidos spawns — `list`, `set --name N --value V`, or `delete --name N`.
    /// Generated from the capability parity manifest; routed through the
    /// gateway-safe HTTP client. (For secrets use a credential, not this.)
    #[command(name = "env-vars")]
    EnvVars {
        #[command(subcommand)]
        action: generated::EnvVarsCmd,
    },
    /// Manage the chat-model registry (Settings → Models) — `list`, `add --id
    /// <id> --provider <p> [--label L] [--sort-order N]`, `update --id <id> …`,
    /// or `delete --id <id>`. Generated from the capability parity manifest;
    /// routed through the gateway-safe HTTP client. Mirrors the chat agent's
    /// `manage_models` tool. (To switch the ACTIVE model, set the `chat_model`
    /// preference; builtins can be disabled via `update --enabled false`, not
    /// deleted.)
    Models {
        #[command(subcommand)]
        action: generated::ModelsCmd,
    },
}

#[derive(Subcommand)]
enum KnowhowCmd {
    /// List the merged user + system knowhow catalog as JSON
    /// (`{ knowhow: [{ id, name, description }] }`). Read `.knowhow[].id` to
    /// find the id to pass to `read`.
    List,
    /// Read one knowhow doc's full content by id. Pass an id from `list` —
    /// e.g. `lucidos knowhow read system-knowhow/building-an-app`. Exit
    /// non-zero if the id resolves to nothing.
    Read {
        /// Knowhow id, exactly as it appears in `list` (e.g.
        /// `system-knowhow/building-an-app` or a user knowhow id).
        id: String,
    },
}

#[derive(Subcommand)]
enum ChangesCmd {
    /// List pending + applied changes as JSON (the `GET /api/v1/changes`
    /// payload verbatim: `{ pending, applied, total_pending, … }`). The
    /// canonical way to find a pending change's id before `apply` — read
    /// `.pending[].id` from the output. Exit non-zero on transport / HTTP
    /// error.
    List,
    /// Apply a pending change by id. Echoes the engine's typed
    /// `ApplyChangeResult` JSON to stdout (see `docs/apply-change-api.md`).
    /// Exit non-zero on transport / HTTP error; the engine's error body is
    /// surfaced verbatim on stderr.
    Apply {
        /// UUID of the change to apply. Get it from `lucidos changes list`
        /// (`.pending[].id`), `ChangeProposed` event payloads, or the
        /// `changes` table.
        change_id: String,
    },
}

#[derive(Subcommand)]
enum ThreadsCmd {
    /// List thread summaries as JSON. Newest-first.
    ///
    /// `--active` restricts to threads where the agentic loop is mid-flow
    /// (`status` of `running` or `waiting_for_user_answer`). Status
    /// `waiting` is *not* active — it means the coding agent has stopped and proposed
    /// changes the user must act on; the loop has paused.
    List {
        /// Restrict to active threads only.
        #[arg(long)]
        active: bool,
        /// Comma-separated source filter (`chat`, `trigger`, `coding-agent`; legacy `claude_code` also accepted).
        #[arg(long)]
        source: Option<String>,
        /// Max rows to return. Server clamps to 1..=1000 (default 100).
        #[arg(long)]
        limit: Option<u32>,
    },
    /// Count thread summaries matching the same filters as `list`.
    /// Outputs `{ "count": N }`.
    Count {
        /// Restrict to active threads only.
        #[arg(long)]
        active: bool,
        /// Comma-separated source filter (`chat`, `trigger`, `coding-agent`; legacy `claude_code` also accepted).
        #[arg(long)]
        source: Option<String>,
    },
}

#[derive(Args)]
pub(crate) struct NotifyArgs {
    /// Notification title (required, non-empty).
    #[arg(long)]
    pub(crate) title: String,
    /// Notification message body (required, non-empty).
    #[arg(long)]
    pub(crate) message: String,
    /// Optional deep-link target. Set only when tapping the notification
    /// should open that app to act on it — same rule the LLM tool follows.
    #[arg(long = "app-id")]
    pub(crate) app_id: Option<String>,
    /// Where a tap should land. `modal` (default) opens the inbox modal;
    /// `none` is passive (no destination, marks read on display); `navigate`
    /// deep-links via the same router `navigate_ui` uses — the CLI infers
    /// the target from the other flags (presence of `--thread-id` →
    /// navigate-to-thread carrying `--event-id` if set; otherwise presence
    /// of `--app-id` → navigate-to-app). Scripts needing fuller control over
    /// the navigate target should POST `/api/v1/notifications` directly with
    /// a structured `tap` body.
    #[arg(long, value_enum)]
    pub(crate) tap: Option<notify::CliTap>,
    /// Thread the notification originated from. With `--tap navigate` and no
    /// `--app-id`, drives a thread deep-link. Also drives the inbox modal's
    /// "Open thread" button regardless of tap.
    #[arg(long = "thread-id")]
    pub(crate) thread_id: Option<String>,
    /// Specific event id inside `--thread-id` to scroll to and briefly pulse
    /// when the tap lands. Ignored without `--thread-id`.
    #[arg(long = "event-id")]
    pub(crate) event_id: Option<String>,
}

#[derive(Args)]
pub(crate) struct ProxyCliArgs {
    /// Name of the proxy entry in `data/config/apis.json`.
    pub(crate) name: String,
    /// Request path (e.g. `/Spisestua/play`). Empty / missing = root.
    #[arg(default_value = "")]
    pub(crate) path: String,
    /// HTTP method (GET, POST, PUT, DELETE, PATCH, …).
    #[arg(short = 'X', long = "request", default_value = "GET")]
    pub(crate) method: String,
    /// Repeated header (e.g. `-H "Content-Type: application/json"`).
    #[arg(short = 'H', long = "header", value_name = "HEADER")]
    pub(crate) headers: Vec<String>,
    /// Request body (inline string).
    #[arg(short = 'd', long = "data", value_name = "BODY")]
    pub(crate) data: Option<String>,
    /// Read request body from stdin.
    #[arg(long = "data-stdin", conflicts_with = "data")]
    pub(crate) data_stdin: bool,
    /// Prepend status line + response headers to stdout (`curl -i`).
    #[arg(short = 'i', long = "include")]
    pub(crate) include: bool,
    /// Exit non-zero on HTTP 4xx/5xx and suppress the body (`curl --fail`).
    #[arg(long = "fail")]
    pub(crate) fail: bool,
}

/// Wire-format actor mode accepted by `--mode`. Typed enum so clap rejects
/// typos at parse time instead of letting the engine return 400 at runtime.
#[derive(Clone, Copy, Debug, ValueEnum)]
pub(crate) enum CliMode {
    Human,
    Agent,
    Engine,
}

impl CliMode {
    pub(crate) fn as_wire(&self) -> &'static str {
        match self {
            CliMode::Human => "human",
            CliMode::Agent => "agent",
            CliMode::Engine => "engine",
        }
    }
}

/// Wire-format relation accepted by `--relation`. `child` = same-workspace
/// parent-with-callback (the spawned thread reports back when it
/// finishes); `top` = independent top-level thread, no callback. `sub` is
/// accepted as a back-compat alias for `child` (the pre-glossary wire
/// name; *child thread* is the direct descendant the spawn produces, while
/// *sub-thread* is the transitive descendant concept).
#[derive(Clone, Copy, Debug, ValueEnum)]
pub(crate) enum CliRelation {
    #[value(alias = "sub")]
    Child,
    Top,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(crate) enum CliCodingAgent {
    #[value(alias = "claude_code")]
    ClaudeCode,
    Codex,
}

impl CliCodingAgent {
    pub(crate) fn as_wire(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude-code",
            Self::Codex => "codex",
        }
    }
}

#[derive(Args)]
pub(crate) struct SpawnThreadArgs {
    /// Target workspace name (e.g. "dev", "myws"). Resolved relative to
    /// $LUCIDOS_WORKSPACES_ROOT (or `~/workspaces` if unset). Pass an absolute
    /// path to bypass the root lookup.
    #[arg(long)]
    pub(crate) to: String,
    /// Task prompt for the new thread. Must be self-contained.
    #[arg(long)]
    pub(crate) message: String,
    /// Optional thread title (shown in the target workspace's UI).
    #[arg(long)]
    pub(crate) title: Option<String>,
    /// Spawn a Claude Code coding-agent session instead of a chat thread.
    #[arg(long)]
    pub(crate) cc: bool,
    /// Spawn a Codex coding-agent session. Shortcut for
    /// `--coding-agent codex`; implies coding-agent mode.
    #[arg(long, conflicts_with = "coding_agent")]
    pub(crate) codex: bool,
    /// Coding-agent backend to launch. Implies coding-agent mode.
    #[arg(long, value_enum)]
    pub(crate) coding_agent: Option<CliCodingAgent>,
    /// CC model override (e.g. "sonnet", "opus", "haiku").
    #[arg(long)]
    pub(crate) cc_model: Option<String>,
    /// Chat model override.
    #[arg(long)]
    pub(crate) model: Option<String>,
    /// Repo name (or UUID) the spawned worktree should be created from.
    /// Defaults to `$LUCIDOS_REPO` (the engine sets it on every coding-agent
    /// subprocess to the calling thread's repo name) so a coding-agent sidequest
    /// stays in the same repo as its caller. Omit (and unset the env var) to
    /// fall back to the target workspace's default repo. Pass an empty string
    /// to force the workspace default even when the env var is set.
    #[arg(long)]
    pub(crate) repo: Option<String>,
    /// Target a folder instead of a repo — creates an *app coding-agent
    /// thread*. Accepts a workspace-relative path (`data/apps/habit-tracker`), an
    /// absolute path, or a registered repo name; resolved on the TARGET
    /// workspace (`--to`). With `--cc`, `--codex`, or `--coding-agent`, a
    /// `data/apps/<id>` value spawns a sparse-checkout worktree narrowed to
    /// that app folder whose Apply ff-merges into the workspace's main (no
    /// `/harden`, no engine restart) —
    /// exactly what the `run_coding_agent` tool's `folder` argument produces. Only
    /// whole app folders are valid; the engine rejects other `data/` paths,
    /// app subpaths, and non-existent folders. Mutually exclusive with
    /// `--repo`; requires a coding-agent flag. When set, the `$LUCIDOS_REPO`
    /// default is suppressed so the request never carries both a repo and a folder.
    #[arg(long, conflicts_with = "repo")]
    pub(crate) folder: Option<String>,
    /// Override the upstream actor mode. Defaults to "agent".
    #[arg(long, value_enum, default_value_t = CliMode::Agent)]
    pub(crate) mode: CliMode,
    /// Relationship of the spawned thread to the calling thread.
    /// `child` = same-workspace parent-with-callback (the spawned thread
    /// reports back when it finishes — emits `parent_thread_id` /
    /// `spawning_event_id`). `top` = independent top-level thread, no
    /// callback (emits `caller_*` fields). `child` requires `--to` to
    /// resolve to the same workspace as `$LUCIDOS_WORKSPACE`. `sub` is
    /// accepted as a back-compat alias for `child`. When omitted,
    /// defaults to `top` so existing cross-workspace recipes keep their
    /// fire-and-forget behavior.
    #[arg(long, value_enum, conflicts_with = "parent")]
    pub(crate) relation: Option<CliRelation>,
    /// DEPRECATED — alias for `--relation child`. Same-workspace
    /// parent-with-callback spawn. Will be removed in a future release.
    #[arg(long)]
    pub(crate) parent: bool,
    /// Use `http://` instead of `https://`. Test-only. Hidden from --help.
    #[arg(long, hide = true)]
    pub(crate) insecure_http: bool,
}

#[derive(Subcommand)]
enum HardenedCmd {
    /// Mark the current branch (in $PWD) as hardened at its current HEAD.
    /// Resolves repo_root, branch, and HEAD SHA from git, then POSTs to the
    /// parent engine's `/api/v1/internal/mark-hardened`.
    Mark,
    /// Print the hardening state of the current branch (in $PWD): `FRESH`,
    /// `STALE`, or `MISSING`. GETs `/api/v1/internal/hardened-state`. Used by
    /// the `harden.md` skill (skip `/harden` iff `FRESH`) and the
    /// `pre-push.sh` hook (allow push iff `FRESH`).
    Query,
}

#[derive(Subcommand)]
enum PlannedCmd {
    /// Record a Planned marker for the current branch (in $PWD). Pass exactly
    /// one of `--plan <docs/plans/file>` (a real implementation plan was
    /// written — the `implementation-plan` skill calls this; records the
    /// awaiting-approval `proposed` state) or `--simple "<reason>"` (this
    /// change is a local fix needing no plan; records `acknowledged_simple`).
    /// POSTs to the parent engine's `/api/v1/internal/mark-planned`.
    Mark(PlannedMarkArgs),
    /// Approve the proposed plan on the current branch (in $PWD), flipping the
    /// marker to gate-satisfying `planned` so edits and Apply unblock. Run by
    /// the coding agent AFTER the user approves the plan in chat. POSTs to the
    /// parent engine's `/api/v1/internal/approve-plan`.
    Approve,
    /// Print the Planned-marker state of the current branch (in $PWD):
    /// `SATISFIED`, `PROPOSED`, or `MISSING`. GETs
    /// `/api/v1/internal/planned-state`. Used by the `cc-plan-gate` hook (allow
    /// edits iff `SATISFIED`).
    State,
}

#[derive(Args)]
pub(crate) struct PlannedMarkArgs {
    /// Relative path of the implementation plan that was written
    /// (e.g. `docs/plans/2026-06-18-my-change.md`). Records state `proposed`
    /// (awaiting the user's approval).
    #[arg(long, conflicts_with = "simple")]
    pub(crate) plan: Option<String>,
    /// One-line reason this change is a local fix needing no plan. Records
    /// state `acknowledged_simple`.
    #[arg(long, conflicts_with = "plan")]
    pub(crate) simple: Option<String>,
}

#[derive(Subcommand)]
enum DataStoreCmd {
    /// Move a directory to `~/.lucidos/data/<name>/` and print the absolute
    /// path. Errors if the destination already exists.
    Add {
        /// Single path segment naming the target directory under
        /// `~/.lucidos/data/`.
        name: String,
        /// Source directory to move. `~` is expanded.
        source_dir: String,
    },
}

#[derive(Subcommand)]
enum DataCmd {
    /// Print the absolute filesystem path for a `data/`-rooted relative path.
    ///
    /// Paths starting with `artifacts/`, `knowhow/`, `apps/`, or `triggers/`
    /// are kept as-is; anything else is prepended with `artifacts/`.
    Path {
        /// Relative path inside the workspace's `data/` directory.
        relative: String,
        /// Create parent directories of the resolved path.
        #[arg(long)]
        mkdir: bool,
    },
    /// Write content to the resolved absolute path. Creates parent dirs.
    /// Prints a clickable chat link (bare store path, no scheme) on stdout and
    /// the absolute path on stderr.
    Write(WriteArgs),
}

#[derive(Args)]
struct WriteArgs {
    /// Relative path inside the workspace's `data/` directory.
    relative: String,
    /// Read content from a local file. Use `-` to read from stdin (default).
    #[arg(long, value_name = "PATH")]
    from: Option<String>,
}

#[derive(Subcommand)]
enum EventsCmd {
    /// POST a domain event to the parent workspace's event store.
    ///
    /// The payload must include a `summary` string field, or `--summary` must
    /// be passed (which is injected into the payload before sending).
    Emit {
        /// PascalCase past-tense event type (e.g. `AnalysisCompleted`).
        event_type: String,
        /// JSON object payload. Must include `summary` or pass --summary.
        #[arg(long)]
        payload: String,
        /// Optional summary string. Replaces or injects `payload.summary`.
        #[arg(long)]
        summary: Option<String>,
    },
    /// GET events from the parent workspace's event store. Outputs JSON.
    Query {
        /// Filter by event type.
        #[arg(long = "type", value_name = "TYPE")]
        event_type: Option<String>,
        /// ISO 8601 lower bound (inclusive).
        #[arg(long)]
        since: Option<String>,
        /// ISO 8601 upper bound (exclusive).
        #[arg(long)]
        until: Option<String>,
        /// Page backward: return only events strictly older than this event id
        /// under (created, id) lexicographic ordering. Mutually exclusive with
        /// --after-event-id.
        #[arg(long, value_name = "UUID", conflicts_with = "after_event_id")]
        before_event_id: Option<String>,
        /// Tail-follow: return only events strictly newer than this event id.
        /// Mutually exclusive with --before-event-id.
        #[arg(long, value_name = "UUID")]
        after_event_id: Option<String>,
        /// Max events to return (server clamps to 1..=1000).
        #[arg(long)]
        limit: Option<u32>,
    },
    /// Count events by type/time without materialising payloads. Outputs
    /// `{count, byte_total}` when `--type` is given; otherwise a per-type
    /// breakdown `{by_type:[...], total_count, total_byte_total}` sorted by
    /// count desc. Use before `query` to size a sweep.
    Count {
        /// Filter by event type. Omit for a per-type breakdown.
        #[arg(long = "type", value_name = "TYPE")]
        event_type: Option<String>,
        /// ISO 8601 lower bound (inclusive).
        #[arg(long)]
        since: Option<String>,
        /// ISO 8601 upper bound (exclusive).
        #[arg(long)]
        until: Option<String>,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(code) => ExitCode::from(code),
        Err(e) => {
            eprintln!("lucidos: {}", e);
            ExitCode::from(1)
        }
    }
}

fn run(cli: Cli) -> Result<u8, workspace::BoxError> {
    match cli.command {
        Command::Data { action } => {
            let ws = resolve_from_env()?;
            match action {
                DataCmd::Path { relative, mkdir } => data::cmd_path(&ws, &relative, mkdir)?,
                DataCmd::Write(args) => {
                    let source = match args.from.as_deref() {
                        None | Some("-") => WriteSource::Stdin,
                        Some(p) => WriteSource::File(PathBuf::from(p)),
                    };
                    data::cmd_write(&ws, &args.relative, source)?;
                }
            }
            Ok(0)
        }
        Command::DataStore { action } => match action {
            DataStoreCmd::Add { name, source_dir } => {
                data_store::cmd_add(&name, &source_dir)?;
                Ok(0)
            }
        },
        Command::Events { action } => {
            let ws = resolve_from_env()?;
            match action {
                EventsCmd::Emit {
                    event_type,
                    payload,
                    summary,
                } => events::cmd_emit(&ws, &event_type, &payload, summary.as_deref())?,
                EventsCmd::Query {
                    event_type,
                    since,
                    until,
                    before_event_id,
                    after_event_id,
                    limit,
                } => events::cmd_query(
                    &ws,
                    events::QueryFilters {
                        event_type: event_type.as_deref(),
                        since: since.as_deref(),
                        until: until.as_deref(),
                        before_event_id: before_event_id.as_deref(),
                        after_event_id: after_event_id.as_deref(),
                        limit,
                    },
                )?,
                EventsCmd::Count {
                    event_type,
                    since,
                    until,
                } => events::cmd_count(
                    &ws,
                    events::CountFilters {
                        event_type: event_type.as_deref(),
                        since: since.as_deref(),
                        until: until.as_deref(),
                    },
                )?,
            }
            Ok(0)
        }
        Command::Hardened { action } => {
            let ws = resolve_from_env()?;
            match action {
                HardenedCmd::Mark => hardened::cmd_mark(&ws)?,
                HardenedCmd::Query => hardened::cmd_query(&ws)?,
            }
            Ok(0)
        }
        Command::Planned { action } => {
            let ws = resolve_from_env()?;
            match action {
                PlannedCmd::Mark(args) => {
                    let kind = match (args.plan.as_deref(), args.simple.as_deref()) {
                        (Some(p), None) => planned::MarkKind::Plan(p),
                        (None, Some(r)) => planned::MarkKind::Simple(r),
                        (None, None) => {
                            return Err("`lucidos planned mark` requires either --plan <path> or --simple \"<reason>\"".into());
                        }
                        // clap's conflicts_with prevents both being set.
                        (Some(_), Some(_)) => unreachable!("clap conflicts_with(plan, simple)"),
                    };
                    planned::cmd_mark(&ws, kind)?;
                }
                PlannedCmd::Approve => planned::cmd_approve(&ws)?,
                PlannedCmd::State => planned::cmd_state(&ws)?,
            }
            Ok(0)
        }
        Command::McpPermissionServer => {
            mcp_permission_server::run()?;
            Ok(0)
        }
        Command::AskUserQuestionHook => {
            ask_user_question_hook::run()?;
            Ok(0)
        }
        Command::CcStopReminder => {
            cc_stop_reminder::run()?;
            Ok(0)
        }
        Command::CcBashGuard => cc_bash_guard::run(),
        Command::CcReadCoerce => {
            cc_read_coerce::run()?;
            Ok(0)
        }
        Command::CcPlanGate => {
            cc_plan_gate::run()?;
            Ok(0)
        }
        Command::CodingAgentDiffHook => {
            coding_agent_diff_hook::run()?;
            Ok(0)
        }
        Command::SpawnThread(args) => {
            spawn_thread::run(args)?;
            Ok(0)
        }
        Command::Proxy(args) => {
            let ws = resolve_from_env()?;
            let body = if args.data_stdin {
                proxy::BodySource::Stdin
            } else if let Some(d) = args.data {
                proxy::BodySource::Inline(d)
            } else {
                proxy::BodySource::None
            };
            proxy::run(
                &ws,
                proxy::ProxyArgs {
                    name: args.name,
                    path: args.path,
                    method: args.method,
                    headers: args.headers,
                    body,
                    include: args.include,
                    fail: args.fail,
                },
            )
        }
        Command::Notify(args) => {
            let ws = resolve_from_env()?;
            notify::cmd_notify(
                &ws,
                &args.title,
                &args.message,
                notify::NotifyExtras {
                    app_id: args.app_id.as_deref(),
                    tap: args.tap,
                    thread_id: args.thread_id.as_deref(),
                    event_id: args.event_id.as_deref(),
                },
            )?;
            Ok(0)
        }
        Command::Threads { action } => {
            let ws = resolve_from_env()?;
            match action {
                ThreadsCmd::List {
                    active,
                    source,
                    limit,
                } => threads::cmd_list(
                    &ws,
                    threads::ListFilters {
                        active: if active { Some(true) } else { None },
                        source: source.as_deref(),
                        limit,
                    },
                )?,
                ThreadsCmd::Count { active, source } => threads::cmd_count(
                    &ws,
                    if active { Some(true) } else { None },
                    source.as_deref(),
                )?,
            }
            Ok(0)
        }
        Command::Changes { action } => {
            let ws = resolve_from_env()?;
            match action {
                ChangesCmd::List => changes::cmd_list(&ws)?,
                ChangesCmd::Apply { change_id } => changes::cmd_apply(&ws, &change_id)?,
            }
            Ok(0)
        }
        Command::Knowhow { action } => {
            let ws = resolve_from_env()?;
            match action {
                KnowhowCmd::List => knowhow::cmd_list(&ws)?,
                KnowhowCmd::Read { id } => knowhow::cmd_read(&ws, &id)?,
            }
            Ok(0)
        }
        Command::Notifications { action } => {
            let ws = resolve_from_env()?;
            generated::dispatch_notifications(&ws, action)?;
            Ok(0)
        }
        Command::Preferences { action } => {
            let ws = resolve_from_env()?;
            generated::dispatch_preferences(&ws, action)?;
            Ok(0)
        }
        Command::Triggers { action } => {
            let ws = resolve_from_env()?;
            generated::dispatch_triggers(&ws, action)?;
            Ok(0)
        }
        Command::TriggerGroups { action } => {
            let ws = resolve_from_env()?;
            generated::dispatch_trigger_groups(&ws, action)?;
            Ok(0)
        }
        Command::Apps { action } => {
            let ws = resolve_from_env()?;
            generated::dispatch_apps(&ws, action)?;
            Ok(0)
        }
        Command::ThreadQueue { action } => {
            let ws = resolve_from_env()?;
            generated::dispatch_thread_queue(&ws, action)?;
            Ok(0)
        }
        Command::Memory { action } => {
            let ws = resolve_from_env()?;
            generated::dispatch_memory(&ws, action)?;
            Ok(0)
        }
        Command::EnvVars { action } => {
            let ws = resolve_from_env()?;
            generated::dispatch_env_vars(&ws, action)?;
            Ok(0)
        }
        Command::Models { action } => {
            let ws = resolve_from_env()?;
            generated::dispatch_models(&ws, action)?;
            Ok(0)
        }
    }
}
