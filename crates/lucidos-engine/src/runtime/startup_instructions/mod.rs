//! A Rust copy of what Claude Code loads at session start: the *startup
//! instructions*.
//!
//! Claude Code puts CLAUDE.md and the unscoped rules into its first user
//! message, and that message names the worktree. So every fresh session rewrites
//! it to the prompt cache. This module reproduces the loading, so Lucidos can
//! move the files into the shared system prompt, where the cache holds them
//! across worktrees.
//!
//! The copy is never trusted alone. A session moves its files only after an
//! earlier session recorded exactly this output. That session ran the same
//! Claude Code version on the same files (see [`ledger`]).

mod ledger;
mod text;

#[cfg(test)]
pub(crate) use ledger::Verdict;
pub(crate) use ledger::{claude_version, KeyState, StartupCheck, StartupFollowUp, StartupLedger};

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

use sha2::{Digest, Sha256};

/// Claude Code stops following imports at this nesting. The file that holds the
/// first `@path` is level 0, so an import chain loads levels 0 to 4.
const MAX_IMPORT_DEPTH: usize = 5;

/// The file types Claude Code imports. A path with another extension stays in
/// the prose.
const IMPORTABLE_EXTENSIONS: &[&str] = &[
    "md", "txt", "json", "yaml", "yml", "toml", "py", "js", "ts", "tsx", "jsx", "rs", "go", "sh",
];

const PREAMBLE: &str = "Codebase and user instructions are shown below. Be sure to adhere to \
these instructions. IMPORTANT: These instructions OVERRIDE any default behavior and you MUST \
follow them exactly as written.";

/// Where Claude Code found a file, which decides how it describes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InstructionKind {
    User,
    Project,
    Local,
}

impl InstructionKind {
    /// The name Claude Code's transcript gives the type.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::User => "User",
            Self::Project => "Project",
            Self::Local => "Local",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::User => "user's private global instructions for all projects",
            Self::Project => "project instructions, checked into the codebase",
            Self::Local => "user's private project instructions, not checked in",
        }
    }
}

/// One entry of the startup instructions, in the shape of Claude Code's
/// transcript record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InstructionFile {
    pub kind: InstructionKind,
    /// The resolved path, as Claude Code records it.
    pub path: PathBuf,
    /// The path a symlink was reached through, when it differs from `path`.
    pub reached_as: Option<PathBuf>,
    pub content: String,
}

/// What Claude Code loads at start for one worktree and config directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StartupInstructions {
    cwd: PathBuf,
    config_dir: PathBuf,
    files: Vec<InstructionFile>,
}

impl StartupInstructions {
    /// The files Claude Code loads when it starts in `cwd`: the user file, then
    /// each directory from the filesystem root down to `cwd`, with its
    /// `CLAUDE.md`, `.claude/CLAUDE.md`, unscoped rules and `CLAUDE.local.md`.
    pub(crate) fn mirror(cwd: &Path, config_dir: &Path, home: Option<&Path>) -> Self {
        let cwd = std::fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf());
        let mut loader = Loader {
            cwd: cwd.clone(),
            home: home.map(Path::to_path_buf),
            seen: HashSet::new(),
            files: Vec::new(),
        };
        loader.file(
            &config_dir.join("CLAUDE.md"),
            InstructionKind::User,
            0,
            false,
        );
        loader.rules(&config_dir.join("rules"), InstructionKind::User);
        let mut directories: Vec<&Path> = cwd.ancestors().collect();
        directories.reverse();
        for dir in directories {
            loader.file(&dir.join("CLAUDE.md"), InstructionKind::Project, 0, false);
            loader.file(
                &dir.join(".claude/CLAUDE.md"),
                InstructionKind::Project,
                0,
                false,
            );
            loader.rules(&dir.join(".claude/rules"), InstructionKind::Project);
            loader.file(
                &dir.join("CLAUDE.local.md"),
                InstructionKind::Local,
                0,
                false,
            );
        }
        Self {
            cwd,
            config_dir: config_dir.to_path_buf(),
            files: loader.files,
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// The directory Claude Code was started in, resolved.
    pub(crate) fn cwd(&self) -> &Path {
        &self.cwd
    }

    /// The block for the system prompt: Claude Code's preamble, then each file
    /// under its description. A path is relative to the worktree, so two
    /// worktrees with the same files render the same bytes.
    pub(crate) fn render(&self) -> String {
        let mut block = String::from(PREAMBLE);
        for file in &self.files {
            block.push_str(&format!(
                "\n\nContents of {} ({}):\n\n{}",
                self.label(file),
                file.kind.description(),
                file.content
            ));
        }
        block
    }

    /// Every path to give `claudeMdExcludes`, so Claude Code loads none of the
    /// files this block holds.
    pub(crate) fn excluded_paths(&self) -> Vec<String> {
        let mut paths = Vec::new();
        for file in &self.files {
            for path in std::iter::once(&file.path).chain(&file.reached_as) {
                let path = path.to_string_lossy().into_owned();
                if !paths.contains(&path) {
                    paths.push(path);
                }
            }
        }
        paths
    }

    /// Names the files, their text and the Claude Code version. Two sessions
    /// with one key load the same files, so a verdict on one holds for the
    /// other. Paths enter relative, so the key ignores where the worktree is.
    pub(crate) fn key(&self, claude_version: &str) -> String {
        let mut hash = Sha256::new();
        hash.update(claude_version.as_bytes());
        for file in &self.files {
            for part in [file.kind.name(), &self.label(file), file.content.as_str()] {
                hash.update([0]);
                hash.update(part.as_bytes());
            }
        }
        format!("{:x}", hash.finalize())
    }

    /// How a transcript's record differs from this copy, or `None` when it is
    /// the same entry for entry. The text names the first difference without
    /// quoting any file.
    pub(crate) fn first_difference(&self, recorded: &[RecordedFile]) -> Option<String> {
        for (index, (mine, theirs)) in self.files.iter().zip(recorded).enumerate() {
            let label = self.label(mine);
            if mine.path.to_string_lossy() != theirs.path {
                return Some(format!(
                    "entry {index}: expected {label}, Claude Code recorded {}",
                    self.relative_label(Path::new(&theirs.path))
                ));
            }
            if mine.kind.name() != theirs.kind {
                return Some(format!(
                    "entry {index} ({label}): expected type {}, recorded {}",
                    mine.kind.name(),
                    theirs.kind
                ));
            }
            if mine.content != theirs.content {
                let at = mine
                    .content
                    .bytes()
                    .zip(theirs.content.bytes())
                    .position(|(a, b)| a != b)
                    .unwrap_or(mine.content.len().min(theirs.content.len()));
                return Some(format!(
                    "entry {index} ({label}): content differs at byte {at} \
                     (expected {} bytes, recorded {})",
                    mine.content.len(),
                    theirs.content.len()
                ));
            }
        }
        (self.files.len() != recorded.len()).then(|| {
            format!(
                "expected {} entries, Claude Code recorded {}",
                self.files.len(),
                recorded.len()
            )
        })
    }

    /// A file as the block names it. A symlink keeps the name it was reached by,
    /// so a linked config directory or dotfile still reads `<config dir>/...`.
    fn label(&self, file: &InstructionFile) -> String {
        self.relative_label(file.reached_as.as_deref().unwrap_or(&file.path))
    }

    /// A path relative to the worktree, or to the config directory for a user
    /// file, never absolute.
    fn relative_label(&self, path: &Path) -> String {
        if let Ok(under_config) = path.strip_prefix(&self.config_dir) {
            if !path.starts_with(&self.cwd) {
                return Path::new("<config dir>")
                    .join(under_config)
                    .to_string_lossy()
                    .into_owned();
            }
        }
        relative_to(path, &self.cwd).to_string_lossy().into_owned()
    }
}

/// One file of a transcript's `instructions` attachment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecordedFile {
    pub kind: String,
    pub path: String,
    pub content: String,
}

/// `path` as a walk from `base`, with `..` for each step up.
fn relative_to(path: &Path, base: &Path) -> PathBuf {
    let path: Vec<_> = path.components().collect();
    let base: Vec<_> = base.components().collect();
    let shared = path.iter().zip(&base).take_while(|(a, b)| a == b).count();
    let mut relative = PathBuf::new();
    for _ in shared..base.len() {
        relative.push("..");
    }
    for part in &path[shared..] {
        relative.push(part);
    }
    relative
}

/// `path` with `.` and `..` folded away, without touching the filesystem.
fn normalized(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

struct Loader {
    cwd: PathBuf,
    home: Option<PathBuf>,
    seen: HashSet<PathBuf>,
    files: Vec<InstructionFile>,
}

impl Loader {
    /// A rules directory, depth first, in the order the filesystem lists it.
    fn rules(&mut self, dir: &Path, kind: InstructionKind) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                self.rules(&path, kind);
            } else if path.extension().is_some_and(|ext| ext == "md") {
                self.file(&path, kind, 0, true);
            }
        }
    }

    /// One file, then the files it imports. `is_rule` marks a file Claude Code
    /// loads at start only when its frontmatter scopes it to no paths.
    fn file(&mut self, path: &Path, kind: InstructionKind, depth: usize, is_rule: bool) {
        if depth >= MAX_IMPORT_DEPTH {
            return;
        }
        let Ok(resolved) = std::fs::canonicalize(path) else {
            return;
        };
        if !resolved.is_file() || !self.seen.insert(resolved.clone()) {
            return;
        }
        let Ok(raw) = std::fs::read_to_string(&resolved) else {
            return;
        };
        let loaded = text::load(&raw);
        if is_rule && loaded.path_scoped {
            return;
        }
        if !loaded.content.is_empty() {
            let literal = normalized(path);
            self.files.push(InstructionFile {
                kind,
                reached_as: (literal != resolved).then_some(literal),
                path: resolved.clone(),
                content: loaded.content,
            });
        }
        let dir = resolved.parent().unwrap_or(&resolved).to_path_buf();
        for import in loaded.imports {
            if let Some(target) = self.import_target(&import, &dir, kind) {
                self.file(&target, kind, depth + 1, false);
            }
        }
    }

    /// Where an `@path` points, when Claude Code would follow it. Only a user
    /// file may reach outside the worktree.
    fn import_target(&self, import: &str, dir: &Path, kind: InstructionKind) -> Option<PathBuf> {
        let wanted = match import.strip_prefix("~/") {
            Some(under_home) => self.home.as_deref()?.join(under_home),
            None => dir.join(import),
        };
        let target = normalized(&wanted);
        let importable = target
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| IMPORTABLE_EXTENSIONS.contains(&ext));
        let reachable = kind == InstructionKind::User || target.starts_with(&self.cwd);
        (importable && reachable).then_some(target)
    }
}

/// How a session starts.
pub(crate) enum StartupDecision {
    /// Put the files in the system prompt and exclude them from Claude Code.
    Move(StartupMove),
    /// Claude Code loads the files itself. `Some` when this session can verify
    /// its key after its first turn.
    Keep(Option<StartupCheck>),
}

pub(crate) struct StartupMove {
    /// The text that follows the Lucidos system prompt.
    pub block: String,
    /// The paths for `claudeMdExcludes`.
    pub excluded: Vec<String>,
}

/// Decide how the session in `cwd` starts. `version` is Claude Code's, `None`
/// when it could not be read: nothing moves then, because nothing can be keyed.
///
/// Claude Code freezes a session's system prompt, so a resumed session starts
/// the way its first run did. One recorded as moved moves again, whatever the
/// ledger says of its key now. Any other resumed session keeps Claude Code's
/// loading, because a block appended to a frozen prompt would never be read.
pub(crate) fn decide(
    workspace: &Path,
    cwd: &Path,
    config_dir: &Path,
    home: Option<&Path>,
    version: Option<&str>,
    resume_session_id: Option<&str>,
) -> StartupDecision {
    let mirror = StartupInstructions::mirror(cwd, config_dir, home);
    let excluded = mirror.excluded_paths();
    if mirror.is_empty() || excluded.iter().any(|path| has_glob_syntax(path)) {
        return StartupDecision::Keep(None);
    }
    let resumed_moved = resume_session_id
        .is_some_and(|session_id| StartupLedger::session_moved(workspace, session_id));
    let moved = StartupDecision::Move(StartupMove {
        block: mirror.render(),
        excluded,
    });
    if resumed_moved {
        return moved;
    }
    let Some(version) = version else {
        return StartupDecision::Keep(None);
    };
    let key = mirror.key(version);
    match (StartupLedger::state_of(workspace, &key), resume_session_id) {
        (KeyState::Verified, None) => moved,
        (KeyState::Verified | KeyState::Failed, _) => StartupDecision::Keep(None),
        (KeyState::Unknown, _) => {
            StartupDecision::Keep(Some(StartupCheck::new(workspace, key, mirror, config_dir)))
        }
    }
}

/// Whether `claudeMdExcludes`, a list of glob patterns, would read `path` as
/// more than a literal. Such a pattern may miss its file, which would then
/// load twice.
fn has_glob_syntax(path: &str) -> bool {
    path.contains(['*', '?', '[', ']', '{', '}', '(', ')', '!', '\\'])
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
