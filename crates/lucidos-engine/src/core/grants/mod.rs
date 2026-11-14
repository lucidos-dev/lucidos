//! The three permission-grant files: what they are called, and where they live.
//!
//! A grant is a decision the user made while looking at one workspace, so it is
//! kept per workspace rather than per machine (ADR 0095). Once a lane is
//! imported, its *tool grants* are `permissions` rows, and its file is a mirror
//! the engine rewrites ([`crate::engine::permission::tool_grants`]). The files sit in
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
//! how a pattern is derived. Derivation stays with each lane. The file format
//! is owned here, so the migration, the import and the mirror cannot drift.

pub mod migration;

use std::path::{Path, PathBuf};
use std::sync::Mutex;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Serializes every [`write_raw`] in this process, so two writers never share
/// a scratch file. `core::handshake_approvals` guards its own record the same
/// way.
static WRITE_LOCK: Mutex<()> = Mutex::new(());

/// Take [`WRITE_LOCK`], recovering a poisoned one. A panic under the lock
/// leaves the file consistent, because every write lands through a rename.
fn write_guard() -> std::sync::MutexGuard<'static, ()> {
    WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// Engine-owned runtime directory at the workspace root, holding the grant
/// files alongside the pidfile, the ports file and the worktrees. Gitignored,
/// excluded from backup, and unreachable by the file tools.
const GRANTS_DIR: &str = ".lucidos";

/// Every mirror file says what it is. The `permissions` table holds the grants
/// once a lane is imported, so a hand edit is reviewed rather than obeyed.
const MIRROR_NOTE: &str = "# Lucidos keeps these grants itself and rewrites this file to match. \
     Delete a line to revoke it.\n\
     # A line added here by hand grants nothing until you import it in Lucidos.\n";

/// Where `workspace_path` keeps its permission grants.
pub fn grants_dir(workspace_path: &Path) -> PathBuf {
    workspace_path.join(GRANTS_DIR)
}

/// One of the three persisted allowlists. Each gates a different caller, so the
/// pattern languages are unrelated and a single merged file would be ambiguous:
/// `Bash(git:*)` grants a command head, bare `Bash` grants every command
/// after the engine's unconditional floor, and `Mcp(slack:*)` grants MCP tools.
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

    /// The whole file the engine writes for `patterns`: the header, the note
    /// that the file is a mirror, then one pattern per line.
    pub fn mirror_body(self, patterns: &[String]) -> String {
        let mut body = format!("{}{MIRROR_NOTE}", self.header());
        for pattern in patterns {
            body.push_str(pattern);
            body.push('\n');
        }
        body
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

    /// Whether `pattern` has a shape this lane's matcher reads. A line it does
    /// not recognize still grants exactly what it granted in the file, which
    /// is an exact match at most. It is shown as *legacy unmapped*.
    pub fn recognizes(self, pattern: &str) -> bool {
        match self {
            Self::AgentCommands => {
                pattern == "Bash"
                    || pattern == "Python"
                    || call_argument(pattern, "Bash")
                        .and_then(|arg| arg.strip_suffix(":*"))
                        // This lane trusts no repo script, so a path head
                        // covers nothing.
                        .is_some_and(|head| is_bare_word(head) && !head.contains('/'))
            }
            Self::McpTools => call_argument(pattern, "Mcp")
                .and_then(|arg| arg.split_once(':'))
                .is_some_and(|(server, tool)| is_bare_word(server) && is_bare_word(tool)),
            Self::CodingAgentTools => {
                let name = pattern.split_once('(').map_or(pattern, |(name, _)| name);
                let well_formed = !pattern.contains('(') || pattern.ends_with(')');
                well_formed
                    && name.starts_with(|c: char| c.is_ascii_alphabetic())
                    && name
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
            }
        }
    }
}

/// The argument of `<name>(<argument>)`, when `pattern` has that shape.
fn call_argument<'p>(pattern: &'p str, name: &str) -> Option<&'p str> {
    pattern
        .strip_prefix(name)?
        .strip_prefix('(')?
        .strip_suffix(')')
}

/// A non-empty word with no space, bracket or colon in it.
fn is_bare_word(word: &str) -> bool {
    !word.is_empty()
        && !word
            .chars()
            .any(|c| c.is_whitespace() || matches!(c, '(' | ')' | ':'))
}

/// What changed a lane's standing grants. Each new row records it as its
/// `source`, and the `PermissionGrantsChanged` audit row names it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GrantSource {
    /// The owner's Always allow tap on a tool card.
    Card,
    /// The owner's edit in a Settings allowlist editor. Every event written
    /// before the source existed came from there.
    #[default]
    Settings,
    /// The one-time import of the grant file the lane read before.
    LegacyImport,
    /// The owner's Import on the card listing lines added to the file by hand.
    FileReview,
    /// Lines deleted from the file by hand. Deleting only takes power away.
    FileEdit,
    /// A revoke through `DELETE /api/v1/permissions/:permission_id` or a review card.
    Revoke,
}

impl GrantSource {
    /// The stored spelling, as the `permissions.source` column holds it.
    pub fn wire(self) -> &'static str {
        match self {
            Self::Card => "card",
            Self::Settings => "settings",
            Self::LegacyImport => "legacy-import",
            Self::FileReview => "file-review",
            Self::FileEdit => "file-edit",
            Self::Revoke => "revoke",
        }
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

/// Overwrite the file atomically, through a sibling temp file and a rename.
pub fn write_raw(dir: &Path, file: GrantFile, contents: &str) -> Result<(), BoxError> {
    let _guard = write_guard();
    std::fs::create_dir_all(dir)?;
    let path = file.path_in(dir);
    let tmp = scratch_path(&path);
    std::fs::write(&tmp, contents)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// The sibling file a write is staged in before its rename.
///
/// The name carries the pid. A second PROCESS on this workspace then cannot
/// interleave into one shared scratch file, whose rename would publish a torn
/// allowlist as the authoritative one. [`WRITE_LOCK`] covers the threads
/// inside this process. Separated from the write so the name is testable: the
/// file is renamed away, so nothing observes it afterwards.
fn scratch_path(path: &Path) -> PathBuf {
    path.with_extension(format!("tmp{}", std::process::id()))
}

#[cfg(test)]
mod tests;
