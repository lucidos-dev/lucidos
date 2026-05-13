//! `lucidos` — CLI for Claude Code subprocesses to talk back to the parent
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
mod cc_edit_preread;
mod cc_read_coerce;
mod cc_stop_reminder;
mod data;
mod data_store;
mod events;
mod hardened;
mod http;
mod mcp_permission_server;
mod proxy;
mod spawn_thread;
mod workspace;

use data::WriteSource;
use workspace::resolve_from_env;

#[derive(Parser)]
#[command(
    name = "lucidos",
    version,
    about = "Talk back to the parent Lucidos workspace from a Claude Code subprocess.",
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
    /// for every Edit tool call. Asks the engine whether the target file has
    /// been Read in this thread; if not, returns `permissionDecision: "deny"`
    /// with an instruction to Read first. Prevents the "File has not been read
    /// yet" loop the model otherwise gets stuck in. Hidden; not for direct
    /// invocation.
    #[command(name = "cc-edit-preread", hide = true)]
    CcEditPreread,
    /// POST a new chat or Claude Code thread to another (or this same) Lucidos
    /// workspace. Defaults caller_* fields from $LUCIDOS_WORKSPACE,
    /// $LUCIDOS_THREAD_ID, $LUCIDOS_EVENT_ID. With `--parent`, emits
    /// parent_thread_id/spawning_event_id instead (same-workspace callback).
    /// `--repo <name>` defaults from $LUCIDOS_REPO so a CC subprocess inherits
    /// the calling thread's repo without callers passing it explicitly.
    #[command(name = "spawn-thread")]
    SpawnThread(SpawnThreadArgs),
    /// Call a backend configured in `data/config/apis.json` through the
    /// engine's proxy (engine injects the configured auth header). Body to
    /// stdout; exit 0 by default even on 4xx/5xx. Use `--fail` to mirror
    /// `curl --fail`, `--include` to mirror `curl -i`.
    Proxy(ProxyCliArgs),
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

/// Wire-format relation accepted by `--relation`. `sub` = same-workspace
/// parent-with-callback (the spawned thread reports back when it
/// finishes); `top` = independent top-level thread, no callback.
#[derive(Clone, Copy, Debug, ValueEnum)]
pub(crate) enum CliRelation {
    Sub,
    Top,
}

#[derive(Args)]
pub(crate) struct SpawnThreadArgs {
    /// Target workspace name (e.g. "dev", "personal"). Resolved relative to
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
    /// Spawn a Claude Code session instead of a chat thread.
    #[arg(long)]
    pub(crate) cc: bool,
    /// CC model override (e.g. "sonnet", "opus", "haiku").
    #[arg(long)]
    pub(crate) cc_model: Option<String>,
    /// Chat model override.
    #[arg(long)]
    pub(crate) model: Option<String>,
    /// Repo name (or UUID) the spawned worktree should be created from.
    /// Defaults to `$LUCIDOS_REPO` (the engine sets it on every CC subprocess
    /// to the calling thread's repo name) so a CC sidequest stays in the same
    /// repo as its caller. Omit (and unset the env var) to fall back to the
    /// target workspace's default repo. Pass an empty string to force the
    /// workspace default even when the env var is set.
    #[arg(long)]
    pub(crate) repo: Option<String>,
    /// Override the upstream actor mode. Defaults to "agent".
    #[arg(long, value_enum, default_value_t = CliMode::Agent)]
    pub(crate) mode: CliMode,
    /// Relationship of the spawned thread to the calling thread.
    /// `sub` = same-workspace parent-with-callback (the spawned thread
    /// reports back when it finishes — emits `parent_thread_id` /
    /// `spawning_event_id`). `top` = independent top-level thread, no
    /// callback (emits `caller_*` fields). `sub` requires `--to` to
    /// resolve to the same workspace as `$LUCIDOS_WORKSPACE`. When
    /// omitted, defaults to `top` so existing cross-workspace recipes
    /// keep their fire-and-forget behavior.
    #[arg(long, value_enum, conflicts_with = "parent")]
    pub(crate) relation: Option<CliRelation>,
    /// DEPRECATED — alias for `--relation sub`. Same-workspace
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
    /// parent engine's `/api/internal/mark-hardened`.
    Mark,
    /// Print the hardening state of the current branch (in $PWD): `FRESH`,
    /// `STALE`, or `MISSING`. GETs `/api/internal/hardened-state`. Used by
    /// the `harden.md` skill (skip `/harden` iff `FRESH`) and the
    /// `pre-push.sh` hook (allow push iff `FRESH`).
    Query,
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
        Command::CcEditPreread => {
            cc_edit_preread::run()?;
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
    }
}
