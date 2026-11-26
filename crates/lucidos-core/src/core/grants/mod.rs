pub mod migration;

use std::path::{Path, PathBuf};
use std::sync::Mutex;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// INVARIANT: Serializes every [`write_raw`] in this process, so two writers never share
/// a scratch file. `core::handshake_approvals` guards its own record the same
/// way.
static WRITE_LOCK: Mutex<()> = Mutex::new(());

/// INVARIANT: Take [`WRITE_LOCK`], recovering a poisoned one. A panic under the lock
/// leaves the file consistent, because every write lands through a rename.
fn write_guard() -> std::sync::MutexGuard<'static, ()> {
    WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// INVARIANT: Engine-owned runtime directory at the workspace root, holding the grant
/// files alongside the pidfile, the ports file and the worktrees. Gitignored,
/// excluded from backup, and unreachable by the file tools.
const GRANTS_DIR: &str = ".lucidos";

pub fn grants_dir(workspace_path: &Path) -> PathBuf {
    workspace_path.join(GRANTS_DIR)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum GrantFile {
    #[serde(rename = "agent-allowed-commands")]
    AgentCommands,
    #[serde(rename = "cc-allowed-tools")]
    CodingAgentTools,
    #[serde(rename = "mcp-allowed-tools")]
    McpTools,
    #[serde(rename = "agent-denied-commands")]
    AgentDeniedCommands,
}

impl GrantFile {
    pub const ALL: [GrantFile; 4] = [
        Self::AgentCommands,
        Self::CodingAgentTools,
        Self::McpTools,
        Self::AgentDeniedCommands,
    ];

    pub fn denies(self) -> bool {
        matches!(self, Self::AgentDeniedCommands)
    }

    pub fn file_name(self) -> &'static str {
        match self {
            Self::AgentCommands => "agent-allowed-commands",
            Self::CodingAgentTools => "cc-allowed-tools",
            Self::McpTools => "mcp-allowed-tools",
            Self::AgentDeniedCommands => "agent-denied-commands",
        }
    }

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
            Self::AgentDeniedCommands => {
                "# Standing denies: one command rule per line. Lines starting with '#' are ignored.\n\
                 # Patterns: Bash(<head>:*) e.g. Bash(curl:*) · Bash (any bash) · Python (any python).\n\
                 # A rule refuses any command that runs <head>, for every agent, before any allow.\n\
                 # It matches the command's name. It protects no file: use a private path for that.\n"
            }
            Self::McpTools => {
                "# Lucidos Agent MCP allowlist: one pattern per line. Lines starting with '#' are ignored.\n\
                 # Patterns: Mcp(<server>:<tool>) e.g. Mcp(slack:channels_list) · Mcp(<server>:*) (any tool on the server).\n\
                 # <server> is the MCP server's registry id. Delete a line to revoke that grant.\n"
            }
        }
    }

    pub fn editor_body(self, patterns: &[String]) -> String {
        let mut body = self.header().to_string();
        for pattern in patterns {
            body.push_str(pattern);
            body.push('\n');
        }
        body
    }

    fn path_in(self, dir: &Path) -> PathBuf {
        dir.join(self.file_name())
    }

    pub fn recognizes(self, pattern: &str) -> bool {
        match self {
            Self::AgentCommands | Self::AgentDeniedCommands => {
                pattern == "Bash"
                    || pattern == "Python"
                    || call_argument(pattern, "Bash")
                        .and_then(|arg| arg.strip_suffix(":*"))
                        // INVARIANT: This lane trusts no repo script, so a path head
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

fn call_argument<'p>(pattern: &'p str, name: &str) -> Option<&'p str> {
    pattern
        .strip_prefix(name)?
        .strip_prefix('(')?
        .strip_suffix(')')
}

fn is_bare_word(word: &str) -> bool {
    !word.is_empty()
        && !word
            .chars()
            .any(|c| c.is_whitespace() || matches!(c, '(' | ')' | ':'))
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GrantSource {
    Card,
    #[default]
    Settings,
    LegacyImport,
    FileReview,
    FileEdit,
    Revoke,
}

impl GrantSource {
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

#[cfg(test)]
fn patterns(dir: &Path, file: GrantFile) -> Vec<String> {
    match std::fs::read_to_string(file.path_in(dir)) {
        Ok(contents) => parse_patterns(&contents),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => panic!("could not read {}: {e}", file.file_name()),
    }
}

pub fn parse_patterns(contents: &str) -> Vec<String> {
    contents
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| l.to_string())
        .collect()
}

pub fn write_raw(dir: &Path, file: GrantFile, contents: &str) -> Result<(), BoxError> {
    let _guard = write_guard();
    std::fs::create_dir_all(dir)?;
    let path = file.path_in(dir);
    let tmp = scratch_path(&path);
    std::fs::write(&tmp, contents)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// INVARIANT: The sibling file a write is staged in before its rename.
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

/// INVARIANT: Resolving symlinks is what the sandbox needs (see `sandbox_writable_roots`),
/// but it also means a `data` symlink decides what the hole covers. Relocating
/// the tree (`data -> /Volumes/ext/lucidos-data`) is legitimate and stays
/// allowed. Two shapes are not, and both grant `.lucidos/` runtime state or a
/// sibling thread's worktree:
///   * **up**, to the workspace root or an ancestor (`data -> .`, `data -> /`);
///   * **in**, to the engine's own [`GRANTS_DIR`] (`data -> .lucidos/…`).
///
/// Cheaper to make impossible than to document as a footgun. Named for what it
/// answers rather than for the first of the two, which is what it used to be.
///
/// Shared with Claude Code's `cc_settings::widened_directories`, which grants
/// the same tree through a different mechanism. Both back ends must agree on
/// which `data` layouts are supported, or a relocated tree works under one and
/// cards under the other.
pub fn grants_more_than_the_data_tree(resolved_data: &Path, workspace: &Path) -> bool {
    let workspace = std::fs::canonicalize(workspace).unwrap_or_else(|_| workspace.to_path_buf());
    if workspace.starts_with(resolved_data) {
        return true;
    }
    let runtime_dir = crate::core::grants::grants_dir(&workspace);
    match std::fs::canonicalize(&runtime_dir) {
        Ok(resolved_runtime) => resolved_data.starts_with(&resolved_runtime),
        // INVARIANT: No runtime dir yet means nothing to reach into. Fall back to the
        // unresolved path so a probe failure still refuses the obvious shape.
        Err(_) => resolved_data.starts_with(&runtime_dir),
    }
}
