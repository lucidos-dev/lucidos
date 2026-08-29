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
mod await_event;
mod build_slot;
mod cc_bash_guard;
mod cc_plan_gate;
mod cc_read_coerce;
mod cc_stop_reminder;
mod changes;
mod coding_agent_diff_hook;
mod credentials;
mod data;
mod data_store;
mod event_waits;
mod events;
mod frontend_preview;
/// Manifest-generated subcommands (one per `cli = true` capability domain).
/// AUTO-GENERATED content; wire each enum below. See
/// `crates/lucidos-engine/src/capability_manifest/`.
mod generated;
mod handshake;
mod hardened;
mod http;
mod knowhow;
mod mcp_permission_server;
mod notify;
mod pair;
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
    /// Subscribe this thread to an event instead of polling for it, then FINISH
    /// your session. Returns immediately: the engine re-opens this thread with a
    /// follow-up message when a matching event lands, or tells you the deadline
    /// passed. Nothing is blocked while you are subscribed, and you must not
    /// sit in a sleep-and-recheck loop waiting for it.
    ///
    /// Use it for anything the engine persists: a change appearing
    /// (`ChangeProposed`), a trigger firing, a backup finishing
    /// (`BackupCompleted`), a workspace domain event. NOT for
    /// external state with no Lucidos event, which has nothing to deliver, and
    /// NOT for a thread you spawned as your own child: that one already re-opens
    /// this thread with its result when it finishes, so a wait on its
    /// `ChildThreadCompleted` buys nothing. Await a completion only for a thread
    /// that is not your own child, named with a `child_thread_id` condition. A
    /// session nobody spawned emits no completion at all: watch
    /// `CodingAgentIdled` with a `thread_id` condition instead.
    ///
    /// A rendezvous, not a stream: the first match consumes it. For a standing
    /// rule that fires every time, create a trigger instead.
    #[command(name = "await-event")]
    AwaitEvent(AwaitEventArgs),
    /// Mint a one-time code that pairs a device with this machine's gateway.
    ///
    /// The gateway authenticates every network caller, and a browser cannot
    /// read the machine-local token file, so even a browser here has to pair.
    /// Run this in a terminal, then type the code into the device.
    ///
    /// The code works once and expires in five minutes.
    Pair {
        /// Gateway port, when it is not on 5252 (packaged) or 5251 (dev).
        #[arg(long)]
        port: Option<u16>,
        /// What to call the device in the paired list.
        #[arg(long)]
        label: Option<String>,
        /// Also draw the code as a QR, so a phone scans instead of typing.
        ///
        /// Needs an address the phone can reach. That is this machine's
        /// MagicDNS name, or its tailnet address, or whatever `--host` says.
        #[arg(long)]
        qr: bool,
        /// The hostname a phone should open, when the tailnet answer is wrong
        /// or absent. Implies `--qr`.
        #[arg(long)]
        host: Option<String>,
    },
    /// Run a heavy build under a *build slot*, so parallel worktrees cannot
    /// pile N full compiles onto one host and OOM it.
    ///
    /// `lucidos build-slot -- <command>` waits for a free slot, runs the
    /// command as its child, and frees the slot when it exits. The slot is an
    /// OS lock, so a killed build releases it too and nothing goes stale.
    ///
    /// Wrap anything heavy: `cargo build`, `cargo test`, a Gradle or Xcode
    /// build, a big webpack run. Do not wrap cheap work, which would sit in a
    /// slot for minutes to save seconds.
    #[command(name = "build-slot")]
    BuildSlot(BuildSlotArgs),
    /// Read or stop this thread's own event subscriptions, the ones
    /// `await-event` armed.
    ///
    /// `list` is how you answer "am I still watching for that?". You cannot
    /// know otherwise, because a subscription is spent the moment it fires, can
    /// time out, and can be stopped by the user, none of which is announced to
    /// a session that is not running. `cancel` is how you stand one down when
    /// the user says to stop; without it a subscription is unrevokable and
    /// re-opens this thread later whatever you told them.
    ///
    /// Both act on `$LUCIDOS_THREAD_ID` and take no thread argument, so neither
    /// can reach another thread's subscriptions.
    #[command(name = "event-waits")]
    EventWaits {
        #[command(subcommand)]
        action: EventWaitsCmd,
    },
    /// Record/query hardening state. Invoked by `/harden` Phase 5.
    Hardened {
        #[command(subcommand)]
        action: HardenedCmd,
    },
    /// Record/query the durable Planned marker that enforces the
    /// `implementation-plan` skill. `mark --plan <docs/plans/file>` records a
    /// real plan awaiting the user's approval (the skill calls this);
    /// `approve` flips that to the gate-satisfying state once the user has
    /// approved; `mark --simple "<reason>"` acknowledges a local fix that needs
    /// no plan; `state` prints `SATISFIED`, `PROPOSED`, or `MISSING`. Only an
    /// approved plan and a `--simple` ack satisfy the pre-edit gate and the
    /// Apply floor; a `proposed` plan still blocks.
    Planned {
        #[command(subcommand)]
        action: PlannedCmd,
    },
    /// Start, stop, or inspect the frontend preview: a Vite dev server the
    /// engine supervises inside a coding-agent worktree, on its own port, so a
    /// TypeScript or CSS change is visible in the real app BEFORE Apply. The
    /// engine owns the process because one a coding agent starts itself dies
    /// with its turn. Development only, and refused on a packaged install.
    #[command(name = "frontend-preview")]
    FrontendPreview {
        #[command(subcommand)]
        action: FrontendPreviewCmd,
    },
    /// MCP stdio server invoked by Claude Code as `--permission-prompt-tool`,
    /// and by Codex as its `ask_user_question` provider. Forwards each request
    /// to the parent engine and blocks on the user's decision. Hidden from
    /// `--help`; not meant for direct invocation.
    #[command(name = "mcp-permission-server", hide = true)]
    McpPermissionServer {
        /// Advertise only the `approve` permission tool, hiding
        /// `ask_user_question`. Passed by Claude Code, which has its own
        /// `AskUserQuestion` tool: see `mcp_permission_server::ToolSet`.
        #[arg(long)]
        permission_only: bool,
    },
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
    /// Approve an auth handshake script, or list which ones may run.
    ///
    /// The engine runs a `script_handshake` script only when it recorded who
    /// wrote it (ADR 0144). Its own file tools record as they write, so this is
    /// for a script edited outside Lucidos.
    Handshake(HandshakeCliArgs),
    /// Read and set the base URLs a stored credential covers, its *credential
    /// scope*.
    ///
    /// The proxy presents a credential only to a base URL it declares. A
    /// provider often needs several: one Binance key pair signs both
    /// `api.binance.com` and `fapi.binance.com`. Adding or removing a secret is
    /// Settings, not this.
    Credentials(CredentialsCliArgs),
    /// Send a push notification via the parent workspace. Persists to the
    /// inbox AND pushes to subscribed devices, identical to the
    /// `send_notification` LLM tool. Use from scripts that need to nudge the
    /// user without going through an LLM thread.
    Notify(NotifyArgs),
    /// Query thread summaries on the parent workspace, and message your own
    /// child threads. `list` / `count` return the same shape as
    /// `GET /api/v1/threads/list` and the `list_threads` LLM tool: a flat
    /// newest-first list of every thread (with optional filters), distinct
    /// from the UI-shaped `/api/v1/threads`. `follow-up` sends a message to one
    /// of THIS thread's own direct children.
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
    /// A subprocess sees a change on its next spawn. The engine loads its own
    /// process env from the store only at startup, so a variable the engine
    /// itself reads needs an engine restart.
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
    /// Manage MCP servers: `list` (status, tool manifest and per-request token
    /// cost), `start --id <id>`, `stop --id <id>`, or `remove --id <id>`.
    /// Generated from the capability parity manifest; routed through the
    /// gateway-safe HTTP client. (Registering one is the chat agent's `mcp`
    /// tool, which needs a command and args this surface does not take.)
    /// Nothing starts servers at boot, so `list` reports running state for the
    /// current engine process only.
    Mcp {
        #[command(subcommand)]
        action: generated::McpCmd,
    },
    /// Manage inbound webhooks: `list`, `create`, `update --id <id>`, or
    /// `delete --id <id>`. A webhook is an endpoint a third party posts to,
    /// emitting one PINNED domain event that a caller cannot change. `create`
    /// prints its bearer token exactly once, because only the digest is stored.
    /// Deliveries arrive on the gateway's hook socket, at
    /// `{host}:{hook_port}/<slug>/<webhook-id>`, never on the main surface.
    /// Generated from the capability parity manifest; routed through the
    /// gateway-safe HTTP client. Deliberately NOT an agent capability, so there
    /// is no LLM tool and no SDK namespace.
    Webhooks {
        #[command(subcommand)]
        action: generated::WebhooksCmd,
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
    /// `--active` is the UNION of `running` and `waiting_for_user_answer`. For
    /// "is the workspace busy?", use `--status running`: a thread awaiting a
    /// user answer is blocked on the human, not working. Status `waiting` is in
    /// neither, and means the coding agent has stopped and proposed changes the
    /// user must act on.
    List {
        /// Restrict to the active union: `running` OR `waiting_for_user_answer`. For "is the workspace busy?" use `--status running` instead, since a thread awaiting a user answer is blocked on the human, not working.
        #[arg(long)]
        active: bool,
        /// Restrict to exactly these statuses (repeatable, or comma-separated): idle, running, waiting, waiting_for_user_answer, paused, failed. The precise form of `--active`, and mutually exclusive with it.
        #[arg(long, conflicts_with = "active")]
        status: Vec<String>,
        /// Comma-separated source filter (`chat`, `trigger`, `coding-agent`; legacy `claude_code` also accepted).
        #[arg(long)]
        source: Option<String>,
        /// Max rows to return. Server clamps to 1..=1000 (default 100).
        #[arg(long)]
        limit: Option<u32>,
        /// Restrict to the direct children of this thread (not grandchildren).
        #[arg(long)]
        parent: Option<String>,
        /// Restrict to the direct children of THIS thread. Shorthand for
        /// `--parent` with the calling thread's own id, so it only works from
        /// inside a Lucidos thread.
        #[arg(long)]
        my_children: bool,
    },
    /// Count thread summaries matching the same filters as `list`.
    /// Outputs `{ "count": N }`.
    ///
    /// `--active` is the UNION of `running` and `waiting_for_user_answer`, so a
    /// nonzero count does NOT mean work is in flight. An idle detector asking
    /// "is the workspace quiet?" wants `--status running`.
    Count {
        /// Restrict to the active union: `running` OR `waiting_for_user_answer`. For "is the workspace busy?" use `--status running` instead, since a thread awaiting a user answer is blocked on the human, not working.
        #[arg(long)]
        active: bool,
        /// Restrict to exactly these statuses (repeatable, or comma-separated): idle, running, waiting, waiting_for_user_answer, paused, failed. The precise form of `--active`, and mutually exclusive with it.
        #[arg(long, conflicts_with = "active")]
        status: Vec<String>,
        /// Comma-separated source filter (`chat`, `trigger`, `coding-agent`; legacy `claude_code` also accepted).
        #[arg(long)]
        source: Option<String>,
        /// Restrict to the direct children of this thread (not grandchildren).
        #[arg(long)]
        parent: Option<String>,
        /// Restrict to the direct children of THIS thread. Shorthand for
        /// `--parent` with the calling thread's own id.
        #[arg(long)]
        my_children: bool,
    },
    /// Send a message to one of THIS thread's own child threads: redirect one
    /// going the wrong way, hand it something a sibling learned, or tell a
    /// stalled one to continue.
    ///
    /// Returns as soon as the message lands; it does not wait for the child.
    /// The child reports back the usual way, as a completion card on its
    /// parent.
    ///
    /// You can only address your own DIRECT children. There is no flag for
    /// saying who you are: the engine reads the calling thread from the
    /// origin token this subprocess was spawned with, and looks the
    /// relationship up itself.
    FollowUp {
        /// The child's uuid. Find it with `threads list --my-children`, or in
        /// the result of `spawn-thread`.
        #[arg(long)]
        thread: String,
        /// What to say. Lands in the child's conversation as a message from
        /// this thread.
        #[arg(long)]
        message: String,
        /// The originating event, for the child's message-route panel.
        /// Defaults to `$LUCIDOS_EVENT_ID`.
        #[arg(long)]
        event_id: Option<String>,
        /// Stop the child's current turn so it reads this immediately,
        /// instead of queueing behind its current work. Whatever that turn
        /// was mid-way through is lost, so use it for a cancellation, not
        /// for an ordinary steer. Without it a child inside a long tool call
        /// reads you only when that call returns.
        #[arg(long)]
        urgent: bool,
    },
}

#[derive(Subcommand)]
enum EventWaitsCmd {
    /// List what this thread is subscribed to right now: each subscription's
    /// id, the events and conditions it watches, the reason it was armed with,
    /// how long ago that was, and how long is left.
    ///
    /// Call it before telling the user whether you are still watching for
    /// something, and to get the id `cancel` takes.
    List,
    /// Stop watching. Pass `--wait-id <id>` for one subscription, `--on
    /// <EventType>` for the ones watching an event type, or `--all` for every
    /// one on this thread; exactly one of the three.
    ///
    /// There is no re-entry, so nothing interrupts you: the subscription simply
    /// stops, the user sees it leave the waiting indicator, and the
    /// transcript records what was stopped.
    Cancel {
        /// The subscription to stop, from `event-waits list`.
        #[arg(long = "wait-id")]
        wait_id: Option<String>,
        /// Stop every subscription on this thread watching this event type,
        /// whatever condition each one carries. Needs no id, and leaves every
        /// other watch alone: the form to use when the answer about one thing
        /// arrived some other way.
        #[arg(long = "on")]
        on: Option<String>,
        /// Stop every live subscription on this thread.
        #[arg(long)]
        all: bool,
    },
}

#[derive(Args)]
pub(crate) struct AwaitEventArgs {
    /// Event name to watch for, PascalCase past tense. Repeat the flag to watch
    /// several: any one of them re-opens the thread.
    #[arg(long = "on", required = true)]
    pub(crate) on: Vec<String>,
    /// Optional payload filter as a JSON object, applied to every `--on` name.
    /// Field-to-value for equality, or an operator object (`{"$gt": 0}`,
    /// `{"$in": [...]}`). Filter on the event's OWN payload fields, the ones
    /// `lucidos events query` prints, plus `thread_id`: the engine supplies
    /// that one for every thread event, so `{"thread_id": "<uuid>"}` scopes the
    /// wait to one thread.
    #[arg(long)]
    pub(crate) condition: Option<String>,
    /// How long to wait before giving up, in seconds (1 to 86400). Required:
    /// there is no unbounded subscription. You get a timeout notice if nothing
    /// matches, so pick a real upper bound and add margin.
    #[arg(long = "timeout-secs")]
    pub(crate) timeout_secs: i64,
    /// One short line, in the user's language, saying what you are waiting for
    /// and why. The user reads it in the waiting indicator, and it is how
    /// they tell a sleeping thread from a stalled one.
    #[arg(long)]
    pub(crate) reason: String,
}

#[derive(Args)]
pub(crate) struct BuildSlotArgs {
    /// Print who holds each slot on this host, and where the count came from.
    /// Takes no slot itself, so it is safe to run while the pool is full.
    #[arg(long)]
    pub(crate) status: bool,
    /// Set the machine-wide slot count, persisted next to the pool. One number
    /// for the host: setting it per workspace cannot work, because the pool
    /// spans them.
    #[arg(long = "set-capacity", value_name = "N")]
    pub(crate) set_capacity: Option<usize>,
    /// Give up after this many seconds instead of waiting indefinitely, exiting
    /// 75. Omit it to wait: a second build is wanted, just not concurrently.
    #[arg(long = "max-wait", value_name = "SECONDS")]
    pub(crate) max_wait: Option<u64>,
    /// What to call this build in the slot listing. Defaults to the command.
    #[arg(long)]
    pub(crate) label: Option<String>,
    /// The command to run, after `--`.
    #[arg(
        trailing_var_arg = true,
        allow_hyphen_values = true,
        value_name = "COMMAND"
    )]
    pub(crate) command: Vec<String>,
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
    /// Where a tap should land. `modal` (default) opens the inbox detail —
    /// use it for info-only notifications too, every notification is openable;
    /// `navigate` deep-links via the same router `navigate_ui` uses — the CLI
    /// infers the target from the other flags (presence of `--thread-id` →
    /// navigate-to-thread carrying `--event-id` if set; otherwise presence
    /// of `--app-id` → navigate-to-app). Scripts needing fuller control over
    /// the navigate target should POST `/api/v1/notifications` directly with
    /// a structured `tap` body. (The passive `none` kind was retired.)
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
    /// The place INSIDE the app the tap lands on, e.g. the one item the
    /// notification is about. It arrives as the app's `location.hash`, so
    /// only an app that routes on the hash moves. Needs `--tap navigate` on
    /// the `--app-id` branch; ignored on a thread deep-link.
    #[arg(long = "fragment")]
    pub(crate) fragment: Option<String>,
}

#[derive(Args)]
pub(crate) struct CredentialsCliArgs {
    #[command(subcommand)]
    pub(crate) action: CredentialsAction,
}

#[derive(Subcommand)]
pub(crate) enum CredentialsAction {
    /// Every stored credential, its auth type, and the base URLs it covers.
    List {
        /// Only the credential with this service name.
        #[arg(long)]
        name: Option<String>,
        /// Print the engine's own JSON array instead of one line per row.
        #[arg(long)]
        json: bool,
    },
    /// Replace the whole set of base URLs one credential covers.
    ///
    /// Replaces rather than appends, so pass every host the credential should
    /// reach. Passing none leaves it sent nowhere, which is what an unscoped
    /// credential already is.
    #[command(name = "set-base-urls")]
    SetBaseUrls {
        /// The credential's service name, as Settings shows it.
        #[arg(long)]
        name: String,
        /// Which row, when an OAuth client registration shares the name with an
        /// API key. Omit unless the CLI asks for it.
        #[arg(long = "auth-type")]
        auth_type: Option<String>,
        /// One base URL, repeated per host. Each needs its scheme, e.g.
        /// `--url https://api.binance.com --url https://fapi.binance.com`.
        #[arg(long = "url")]
        urls: Vec<String>,
    },
}

#[derive(Args)]
pub(crate) struct HandshakeCliArgs {
    #[command(subcommand)]
    pub(crate) action: HandshakeAction,
}

#[derive(Subcommand)]
pub(crate) enum HandshakeAction {
    /// Every handshake script `apis.json` names, and whether it may run.
    List,
    /// Record a script's current content as approved.
    Approve {
        /// `scripts/auth/<name>.py`, or the workspace-relative path.
        path: String,
    },
}

#[derive(Args)]
pub(crate) struct ProxyCliArgs {
    /// Name of the proxy entry in `data/config/apis.json`.
    pub(crate) name: String,
    /// Request path (e.g. `/living-room/play`). Empty / missing = root.
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
    /// Target workspace name (e.g. "dev", "myws"). Resolved against
    /// $LUCIDOS_WORKSPACES_ROOT when set, else the directory holding your own
    /// workspace, so a sibling is always reachable by name. Falls back to
    /// `~/workspaces`. Pass an absolute path to bypass the lookup.
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
    // TEMPORARY MEASURE — sunset deprecation; tracked in
    // docs/temporary-measures.md § "lucidos spawn-thread --parent deprecated alias".
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
enum FrontendPreviewCmd {
    /// Start (or move) the frontend preview onto a thread's worktree. Replaces
    /// any running preview: there is one slot per workspace. Prints the URL to
    /// open, built from the host this command reached the engine on.
    Start {
        /// Thread whose coding-agent worktree to serve. Defaults to
        /// `$LUCIDOS_THREAD_ID`, which every coding-agent subprocess carries.
        #[arg(long)]
        thread_id: Option<String>,
    },
    /// Stop the running frontend preview, if any.
    Stop,
    /// Print what the frontend preview is currently serving, if anything.
    Status,
}

#[derive(Subcommand)]
enum PlannedCmd {
    /// Record a Planned marker for the current branch (in $PWD). Pass exactly
    /// one of:
    ///
    /// `--plan <docs/plans/file>`, a real implementation plan the
    /// `implementation-plan` skill wrote. Records the awaiting-approval
    /// `proposed` state.
    ///
    /// `--simple "<reason>"`, a local fix needing no plan. Records
    /// `acknowledged_simple`.
    ///
    /// `--security-fix "<reason>" --files <csv>`, an UNATTENDED run's security
    /// fix confined to those files. Records `bounded_security_fix`.
    ///
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
    #[arg(long, conflicts_with_all = ["simple", "security_fix"])]
    pub(crate) plan: Option<String>,
    /// One-line reason this change is a local fix needing no plan. Records
    /// state `acknowledged_simple`.
    #[arg(long, conflicts_with_all = ["plan", "security_fix"])]
    pub(crate) simple: Option<String>,
    /// One-line reason an UNATTENDED run is committing this security fix with
    /// no prior plan decision. Name the finding and the regression test that
    /// proves the fix. Records state `bounded_security_fix` and requires
    /// `--files`. Never use it in a session that can ask the user, and never
    /// for non-security work.
    #[arg(long, requires = "files", conflicts_with_all = ["plan", "simple"])]
    pub(crate) security_fix: Option<String>,
    /// Repo-relative paths the bounded security fix is confined to, comma
    /// separated. Apply refuses the branch if it touched anything else, so
    /// name every file including the test. The engine caps the list.
    #[arg(long, value_delimiter = ',', requires = "security_fix")]
    pub(crate) files: Vec<String>,
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
        /// Restrict to one thread. This is how you read a past conversation:
        /// pair it with `--type MessageReceived`, or the query returns that
        /// thread's entire transcript including every streamed token.
        #[arg(long, value_name = "UUID")]
        thread_id: Option<String>,
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
                    thread_id,
                    limit,
                } => events::cmd_query(
                    &ws,
                    events::QueryFilters {
                        event_type: event_type.as_deref(),
                        since: since.as_deref(),
                        until: until.as_deref(),
                        before_event_id: before_event_id.as_deref(),
                        after_event_id: after_event_id.as_deref(),
                        thread_id: thread_id.as_deref(),
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
                    // clap's `conflicts_with` makes the three mutually
                    // exclusive, so at most one arm can match.
                    let kind = if let Some(p) = args.plan.as_deref() {
                        planned::MarkKind::Plan(p)
                    } else if let Some(r) = args.simple.as_deref() {
                        planned::MarkKind::Simple(r)
                    } else if let Some(r) = args.security_fix.as_deref() {
                        planned::MarkKind::SecurityFix {
                            reason: r,
                            files: args.files.clone(),
                        }
                    } else {
                        return Err("`lucidos planned mark` requires one of --plan <path>, --simple \"<reason>\", or --security-fix \"<reason>\" --files <csv>".into());
                    };
                    planned::cmd_mark(&ws, kind)?;
                }
                PlannedCmd::Approve => planned::cmd_approve(&ws)?,
                PlannedCmd::State => planned::cmd_state(&ws)?,
            }
            Ok(0)
        }
        Command::FrontendPreview { action } => {
            let ws = resolve_from_env()?;
            match action {
                FrontendPreviewCmd::Start { thread_id } => {
                    frontend_preview::cmd_start(&ws, thread_id.as_deref())?
                }
                FrontendPreviewCmd::Stop => frontend_preview::cmd_stop(&ws)?,
                FrontendPreviewCmd::Status => frontend_preview::cmd_status(&ws)?,
            }
            Ok(0)
        }
        Command::McpPermissionServer { permission_only } => {
            mcp_permission_server::run(if permission_only {
                mcp_permission_server::ToolSet::PermissionOnly
            } else {
                mcp_permission_server::ToolSet::All
            })?;
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
        Command::Handshake(args) => {
            let ws = resolve_from_env()?;
            match args.action {
                HandshakeAction::List => handshake::cmd_list(&ws)?,
                HandshakeAction::Approve { path } => handshake::cmd_approve(&ws, &path)?,
            }
            Ok(0)
        }
        Command::Credentials(args) => {
            let ws = resolve_from_env()?;
            match args.action {
                CredentialsAction::List { name, json } => {
                    credentials::cmd_list(&ws, name.as_deref(), json)?
                }
                CredentialsAction::SetBaseUrls {
                    name,
                    auth_type,
                    urls,
                } => credentials::cmd_set_base_urls(&ws, &name, auth_type.as_deref(), &urls)?,
            }
            Ok(0)
        }
        Command::Pair {
            port,
            label,
            qr,
            host,
        } => {
            pair::cmd_pair(pair::PairArgs {
                port,
                label: label.as_deref(),
                // `--host` is only useful for a QR, so asking for one is
                // implied rather than a second flag the user must remember.
                qr: qr || host.is_some(),
                host: host.as_deref(),
            })?;
            Ok(0)
        }
        Command::AwaitEvent(args) => {
            let ws = resolve_from_env()?;
            await_event::cmd_await_event(
                &ws,
                await_event::AwaitEventArgs {
                    on: &args.on,
                    condition: args.condition.as_deref(),
                    timeout_secs: args.timeout_secs,
                    reason: &args.reason,
                },
            )?;
            Ok(0)
        }
        // No workspace is resolved here. A build slot is host state, so this
        // must work in a plain checkout with no engine anywhere near it.
        Command::BuildSlot(args) => build_slot::run(build_slot::BuildSlotArgs {
            status: args.status,
            set_capacity: args.set_capacity,
            max_wait_secs: args.max_wait,
            label: args.label,
            command: args.command,
        }),
        Command::EventWaits { action } => {
            let ws = resolve_from_env()?;
            match action {
                EventWaitsCmd::List => event_waits::cmd_event_waits_list(&ws)?,
                EventWaitsCmd::Cancel { wait_id, on, all } => event_waits::cmd_event_waits_cancel(
                    &ws,
                    wait_id.as_deref(),
                    on.as_deref(),
                    all,
                )?,
            }
            Ok(0)
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
                    fragment: args.fragment.as_deref(),
                },
            )?;
            Ok(0)
        }
        Command::Threads { action } => {
            let ws = resolve_from_env()?;
            match action {
                ThreadsCmd::List {
                    active,
                    status,
                    source,
                    limit,
                    parent,
                    my_children,
                } => threads::cmd_list(
                    &ws,
                    threads::ListFilters {
                        active: if active { Some(true) } else { None },
                        status: &status,
                        source: source.as_deref(),
                        limit,
                        parent: threads::resolve_parent_filter(
                            parent,
                            my_children,
                            threads::source_thread_id_from_env(),
                        )?,
                    },
                )?,
                ThreadsCmd::Count {
                    active,
                    status,
                    source,
                    parent,
                    my_children,
                } => threads::cmd_count(
                    &ws,
                    threads::ListFilters {
                        active: if active { Some(true) } else { None },
                        status: &status,
                        source: source.as_deref(),
                        // `count` takes no --limit: a COUNT(*) has no page to
                        // size. Same reuse the store and HTTP layers make.
                        limit: None,
                        parent: threads::resolve_parent_filter(
                            parent,
                            my_children,
                            threads::source_thread_id_from_env(),
                        )?,
                    },
                )?,
                ThreadsCmd::FollowUp {
                    thread,
                    message,
                    event_id,
                    urgent,
                } => threads::cmd_follow_up(
                    &ws,
                    &thread,
                    &message,
                    event_id.or_else(threads::event_id_from_env).as_deref(),
                    urgent,
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
        Command::Mcp { action } => {
            let ws = resolve_from_env()?;
            generated::dispatch_mcp(&ws, action)?;
            Ok(0)
        }
        Command::Webhooks { action } => {
            let ws = resolve_from_env()?;
            generated::dispatch_webhooks(&ws, action)?;
            Ok(0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    /// The clap tree is built at startup and panics on a malformed
    /// definition, so asserting it builds is the cheapest coverage there is
    /// for a newly added subcommand.
    #[test]
    fn the_command_tree_is_valid() {
        Cli::command().debug_assert();
    }

    fn subcommand_flags(path: &[&str]) -> Vec<String> {
        let mut cmd = Cli::command();
        for name in path {
            cmd = cmd
                .find_subcommand(name)
                .unwrap_or_else(|| panic!("subcommand '{name}' exists"))
                .clone();
        }
        cmd.get_arguments()
            .filter_map(|a| a.get_long().map(|l| format!("--{l}")))
            .collect()
    }

    /// `threads follow-up` must offer no way to state who the caller is. The
    /// engine reads the calling thread off the thread-bound origin token, and
    /// a `--from` / `--caller-thread` flag would hand back exactly the
    /// capability that binding removes, one layer up.
    #[test]
    fn follow_up_exposes_no_caller_identity_flag() {
        let flags = subcommand_flags(&["threads", "follow-up"]);
        assert!(flags.contains(&"--thread".to_string()), "{flags:?}");
        assert!(flags.contains(&"--message".to_string()), "{flags:?}");
        assert!(flags.contains(&"--urgent".to_string()), "{flags:?}");
        for forbidden in ["--from", "--caller", "--caller-thread", "--parent"] {
            assert!(
                !flags.iter().any(|f| f == forbidden),
                "{forbidden} must not exist on follow-up: the caller is authenticated, not stated"
            );
        }
    }

    /// `--status` is the precise form of `--active`, and both `list` and
    /// `count` carry it: an idle detector calls `count`, which is the surface
    /// the union misled on 2026-08-07.
    #[test]
    fn threads_list_and_count_both_offer_the_status_filter() {
        for path in [["threads", "list"], ["threads", "count"]] {
            let flags = subcommand_flags(&path);
            assert!(
                flags.contains(&"--status".to_string()),
                "{path:?} must expose --status: {flags:?}"
            );
            assert!(
                flags.contains(&"--active".to_string()),
                "{path:?} keeps --active unchanged for existing callers: {flags:?}"
            );
        }
    }

    /// Two answers to one question. Left to compose, `--active --status idle`
    /// would return nothing and read as "the workspace is quiet".
    #[test]
    fn active_and_status_cannot_be_combined() {
        for path in [["threads", "list"], ["threads", "count"]] {
            let argv = [
                "lucidos", "threads", path[1], "--active", "--status", "running",
            ];
            let err = Cli::command()
                .try_get_matches_from(argv)
                .expect_err("--active with --status must be refused, not intersected");
            let rendered = err.to_string();
            assert!(
                rendered.contains("--active") && rendered.contains("--status"),
                "the refusal must name both flags: {rendered}"
            );
        }
    }

    /// Both spellings of one filter reach the engine, so a caller who follows
    /// the repo's kebab-case convention is not punished for it.
    #[test]
    fn status_accepts_repeats_and_comma_separated_values() {
        let matches = Cli::command()
            .try_get_matches_from([
                "lucidos",
                "threads",
                "count",
                "--status",
                "running",
                "--status",
                "failed,paused",
            ])
            .expect("--status is repeatable");
        let values: Vec<&String> = matches
            .subcommand_matches("threads")
            .and_then(|m| m.subcommand_matches("count"))
            .expect("threads count")
            .get_many::<String>("status")
            .expect("status values")
            .collect();
        assert_eq!(
            values,
            [&"running".to_string(), &"failed,paused".to_string()]
        );
    }

    /// The child-listing filter is spelled the same at every layer: `parent`
    /// is the HTTP query param, `my-children` mirrors the LLM tool's
    /// `my_children`. Neither is a third synonym.
    #[test]
    fn threads_list_and_count_share_the_child_filter_spelling() {
        for path in [["threads", "list"], ["threads", "count"]] {
            let flags = subcommand_flags(&path);
            assert!(
                flags.contains(&"--parent".to_string()),
                "{path:?} {flags:?}"
            );
            assert!(
                flags.contains(&"--my-children".to_string()),
                "{path:?} {flags:?}"
            );
            assert!(
                !flags.iter().any(|f| f == "--mine"),
                "{path:?}: --mine is a third name for a filter that already has one"
            );
        }
    }
}
