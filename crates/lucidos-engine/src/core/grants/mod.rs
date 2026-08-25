//! The three permission-grant files: what they are called, and where they live.
//!
//! A grant is a decision the user made while looking at one workspace, so it is
//! stored per workspace rather than per machine (ADR 0095). The files sit in
//! `<workspace>/.lucidos/`, which the engine reads and the agent's own file
//! tools refuse: `write_file` and `edit_file` resolve nothing under `.lucidos/`
//! except the `tmp/` subtree. `data/config/` was the obvious alternative and is
//! disqualified for exactly that reason, because a permission file the agent
//! can rewrite is not a permission file.
//!
//! [`migration`] seeds each workspace that existed when grants stopped being
//! machine-global.
//!
//! The three lanes differ only in the file name, the instructional header and
//! how a pattern is derived. Derivation stays with each lane. The file itself
//! is owned here, so the migration and the three readers cannot drift.

pub mod migration;

use std::path::{Path, PathBuf};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Engine-owned runtime directory at the workspace root, holding the grant
/// files alongside the pidfile, the ports file and the worktrees. Gitignored,
/// excluded from backup, and unreachable by the file tools.
const GRANTS_DIR: &str = ".lucidos";

/// Where `workspace_path` keeps its permission grants.
pub fn grants_dir(workspace_path: &Path) -> PathBuf {
    workspace_path.join(GRANTS_DIR)
}

/// One of the three persisted allowlists. Each gates a different caller, so the
/// pattern languages are unrelated and a single merged file would be ambiguous:
/// `Bash(git:*)` is a chat command-guard pattern, `Bash` on its own is also a
/// Claude Code `--allowedTools` entry, and `Mcp(slack:*)` is neither.
///
/// Serializes as its own [`GrantFile::file_name`], so the `grant_file` on a
/// `PermissionGrantsChanged` event reads as the name the Settings editor shows.
/// `the_wire_name_is_the_file_name` pins the two together.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum GrantFile {
    /// The Lucidos Agent command guard (ADR 0002).
    #[serde(rename = "agent-allowed-commands")]
    AgentCommands,
    /// Claude Code's `--allowedTools`, handed over on every spawn.
    #[serde(rename = "cc-allowed-tools")]
    CodingAgentTools,
    /// The Lucidos Agent's MCP tool gate.
    #[serde(rename = "mcp-allowed-tools")]
    McpTools,
}

impl GrantFile {
    /// Every lane, for the migration and for tests that must cover all three.
    pub const ALL: [GrantFile; 3] = [Self::AgentCommands, Self::CodingAgentTools, Self::McpTools];

    /// The file name inside [`grants_dir`].
    pub fn file_name(self) -> &'static str {
        match self {
            Self::AgentCommands => "agent-allowed-commands",
            Self::CodingAgentTools => "cc-allowed-tools",
            Self::McpTools => "mcp-allowed-tools",
        }
    }

    /// The instructional comment a fresh file opens with, so the editor shows
    /// the pattern language before the first grant exists.
    pub fn header(self) -> &'static str {
        match self {
            Self::AgentCommands => {
                "# Lucidos Agent command allowlist: one pattern per line. Lines starting with '#' are ignored.\n\
                 # Patterns: Bash(<head>:*) e.g. Bash(git:*) · Bash (any bash) · Python (any python).\n\
                 # A chained command (&&, |, ;) auto-allows only when EVERY segment's head is covered.\n\
                 # <head> matches only a BARE command word. Bash(ls:*) covers 'ls' and 'sudo ls',\n\
                 # never './ls' or 'data/bin/ls': a path-qualified command always asks.\n"
            }
            Self::CodingAgentTools => {
                "# One pattern per line. Lines starting with '#' are ignored.\n"
            }
            Self::McpTools => {
                "# Lucidos Agent MCP allowlist: one pattern per line. Lines starting with '#' are ignored.\n\
                 # Patterns: Mcp(<server>:<tool>) e.g. Mcp(slack:channels_list) · Mcp(<server>:*) (any tool on the server).\n\
                 # <server> is the MCP server's registry id. Delete a line to revoke that grant.\n"
            }
        }
    }

    /// Patterns granted before the user has decided anything. Empty for every
    /// lane, so each feature ships dark and the list is built by the per-prompt
    /// "Always allow" buttons.
    pub fn compiled_defaults(self) -> &'static [&'static str] {
        &[]
    }

    fn path_in(self, dir: &Path) -> PathBuf {
        dir.join(self.file_name())
    }
}

/// Granted patterns from `<dir>/<file>`, one per line, blanks and `#` comments
/// ignored.
///
/// A missing file yields the compiled defaults. An IO error yields nothing at
/// all: a read failure must never auto-allow, so it degrades to "no patterns"
/// and the user re-approves once, rather than the gate silently opening.
pub fn patterns(dir: &Path, file: GrantFile) -> Vec<String> {
    let path = file.path_in(dir);
    match std::fs::read_to_string(&path) {
        Ok(contents) => parse_patterns(&contents),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => file
            .compiled_defaults()
            .iter()
            .map(|s| s.to_string())
            .collect(),
        Err(e) => {
            crate::log!(
                "[Grants] Failed to read {}: {}. Treating as no patterns.",
                path.display(),
                e
            );
            Vec::new()
        }
    }
}

/// Whether the file is on disk. Only the Claude Code lane asks, because it is
/// the one that seeds a header for the user to discover.
pub fn exists(dir: &Path, file: GrantFile) -> bool {
    file.path_in(dir).exists()
}

/// The granted lines of an allowlist body.
///
/// Public so a caller that already holds the body can name what it granted
/// without a second read of the file it just wrote.
pub fn parse_patterns(contents: &str) -> Vec<String> {
    contents
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| l.to_string())
        .collect()
}

/// Append `pattern` unless it is already granted. Creates the file with its
/// header when absent.
pub fn append(dir: &Path, file: GrantFile, pattern: &str) -> Result<(), BoxError> {
    let path = file.path_in(dir);
    let existing = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => file.header().to_string(),
        Err(e) => return Err(e.into()),
    };
    if parse_patterns(&existing).iter().any(|l| l == pattern) {
        return Ok(());
    }
    let mut next = existing;
    if !next.is_empty() && !next.ends_with('\n') {
        next.push('\n');
    }
    next.push_str(pattern);
    next.push('\n');
    write_raw(dir, file, &next)
}

/// The whole file, for the settings editor. A missing file reads as its header,
/// so the editor always opens on the instructional comment.
pub fn read_raw(dir: &Path, file: GrantFile) -> Result<String, BoxError> {
    match std::fs::read_to_string(file.path_in(dir)) {
        Ok(s) => Ok(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(file.header().to_string()),
        Err(e) => Err(e.into()),
    }
}

/// Overwrite the file atomically, through a sibling temp file and a rename.
///
/// Every gate re-reads its file per prompt, so an edit takes effect on the next
/// gated call with no restart. Claude Code is the one exception: a running
/// subprocess keeps the `--allowedTools` flag it was spawned with.
pub fn write_raw(dir: &Path, file: GrantFile, contents: &str) -> Result<(), BoxError> {
    std::fs::create_dir_all(dir)?;
    let path = file.path_in(dir);
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, contents)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

#[cfg(test)]
mod tests;
