//! `cognos` — CLI for Claude Code subprocesses to talk back to the parent
//! CognOS workspace.
//!
//! ## Workspace resolution
//!
//! Subcommands need to know which workspace they belong to. Order:
//!   1. Walk up from `$PWD` looking for the first `.cognos/ports` file.
//!      That ancestor directory is the parent workspace.
//!   2. Fall back to `$COGNOS_WORKSPACE` env var (engine sets this on the
//!      spawned subprocess).
//!
//! The walk naturally skips `<parent>/.cognos/worktrees/<id>/` because that
//! directory does not itself contain a `.cognos/ports` file.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};

mod data;
mod events;
mod hardened;
mod http;
mod mcp_permission_server;
mod workspace;

use data::WriteSource;
use workspace::resolve_from_env;

#[derive(Parser)]
#[command(
    name = "cognos",
    version,
    about = "Talk back to the parent CognOS workspace from a Claude Code subprocess.",
    long_about = "Resolves the parent workspace by walking up from $PWD for the first \
                  .cognos/ports file, falling back to $COGNOS_WORKSPACE."
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
}

#[derive(Subcommand)]
enum HardenedCmd {
    /// Mark the current branch (in $PWD) as hardened at its current HEAD.
    /// Resolves repo_root, branch, and HEAD SHA from git, then POSTs to the
    /// parent engine's `/api/internal/mark-hardened`.
    Mark,
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
        /// Max events to return (server clamps to 1..=1000).
        #[arg(long)]
        limit: Option<u32>,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("cognos: {}", e);
            ExitCode::from(1)
        }
    }
}

fn run(cli: Cli) -> Result<(), workspace::BoxError> {
    match cli.command {
        Command::Data { action } => {
            let ws = resolve_from_env()?;
            match action {
                DataCmd::Path { relative, mkdir } => data::cmd_path(&ws, &relative, mkdir),
                DataCmd::Write(args) => {
                    let source = match args.from.as_deref() {
                        None | Some("-") => WriteSource::Stdin,
                        Some(p) => WriteSource::File(PathBuf::from(p)),
                    };
                    data::cmd_write(&ws, &args.relative, source)
                }
            }
        }
        Command::Events { action } => {
            let ws = resolve_from_env()?;
            match action {
                EventsCmd::Emit {
                    event_type,
                    payload,
                    summary,
                } => events::cmd_emit(&ws, &event_type, &payload, summary.as_deref()),
                EventsCmd::Query {
                    event_type,
                    since,
                    until,
                    limit,
                } => events::cmd_query(
                    &ws,
                    event_type.as_deref(),
                    since.as_deref(),
                    until.as_deref(),
                    limit,
                ),
            }
        }
        Command::Hardened { action } => {
            let ws = resolve_from_env()?;
            match action {
                HardenedCmd::Mark => hardened::cmd_mark(&ws),
            }
        }
        Command::McpPermissionServer => mcp_permission_server::run(),
    }
}
