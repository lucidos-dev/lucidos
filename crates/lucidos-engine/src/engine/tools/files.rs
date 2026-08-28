use super::super::context::sanitize_file_content_for_llm;
use super::super::LucidosEngine;
use crate::engine::event_bus::{BusEvent, SystemEvent};
use base64::Engine as _;

/// Outer safety bound on image files `read_file` will load. This is *not* the
/// reject point for normal photos — anything under this cap is read, and if its
/// base64 payload would exceed `api::MAX_IMAGE_BASE64_BYTES` it is downsampled
/// via `ChatImage::compress` rather than rejected. Files above this absurd-size
/// bound are still refused.
const IMAGE_MAX_BYTES: u64 = 25 * 1024 * 1024;

/// Why a write to `data_path` should be rejected, or `None` if it's allowed.
/// Centralized so all write tools share the same policy and message: every
/// mutating file tool (`write_file`, `edit_file`, `copy_file`'s destination,
/// `delete_file`) consults this before touching disk.
///
/// `data_path` is already normalized, so the `.lucidos/` arm only ever sees the
/// one readable subtree ([`crate::core::TMP_DIR`]); the rest was refused
/// outright during normalization. Scratch is readable but not writable because
/// these tools git-commit everything they write, and that tree is gitignored:
/// letting a write through would either commit nothing (a silent lie to the
/// caller) or need a second, commitless result shape. `run_python` already
/// covers the case.
fn read_only_reason(data_path: &str) -> Option<&'static str> {
    if crate::core::is_system_knowhow_path(data_path) {
        return Some("system knowhow is read-only (shipped with the engine)");
    }
    if crate::core::is_tmp_path(data_path) {
        return Some(
            "the file tools commit everything they write, and .lucidos/tmp/ is gitignored \
             ephemeral scratch. Read it here, but create and edit it with run_python, whose \
             cwd is the workspace root: open('.lucidos/tmp/notes.json', 'w'). For a file \
             that should persist, target a typed data/ subdirectory such as artifacts/",
        );
    }
    None
}

/// Convert a flexible "dot-bracket" path expression to an RFC 6901 JSON Pointer.
///
/// Accepted syntaxes (mix freely in one path):
///
/// | Input                            | Output                  | Notes                          |
/// |----------------------------------|-------------------------|--------------------------------|
/// | `metadata.author.name`           | `/metadata/author/name` | Dot-separated identifiers      |
/// | `sections[0]`                    | `/sections/0`           | Bracket array index            |
/// | `matrix[1][2]`                   | `/matrix/1/2`           | Consecutive brackets           |
/// | `dailyLog["2026-05-04"]`         | `/dailyLog/2026-05-04`  | Double-quoted key in brackets  |
/// | `dailyLog['2026-05-04']`         | `/dailyLog/2026-05-04`  | Single-quoted key in brackets  |
/// | `data["foo.bar"]`                | `/data/foo.bar`         | Quoted keys may contain `.`    |
/// | `$.streak` / `$`                 | `/streak` / ``          | JSONPath-style root marker     |
/// | `[0].name`                       | `/0/name`               | Path may start with brackets   |
/// | `/sections/1/title`              | `/sections/1/title`     | Already-pointer is returned as-is |
/// | `` (empty)                       | `` (empty)              | Targets the document root      |
///
/// Quoted keys honor RFC 6901 escaping (`~` → `~0`, `/` → `~1`), so a key like
/// `"a/b"` in `data["a/b"]` becomes `/data/a~1b`. Inside a quoted key, `\` escapes
/// the next character (use `\"` to embed a double-quote in a double-quoted key).
pub(crate) fn dot_path_to_pointer(path: &str) -> String {
    if path.is_empty() || path.starts_with('/') {
        return path.to_string();
    }
    // Tolerate the JSONPath root marker — `$.foo` and `$['foo']` are common shapes.
    let stripped = path
        .strip_prefix("$.")
        .or_else(|| path.strip_prefix('$'))
        .unwrap_or(path);
    if stripped.is_empty() {
        return String::new();
    }

    let mut pointer = String::new();
    let mut buf = String::new();
    let mut chars = stripped.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            '.' => flush_segment(&mut pointer, &mut buf),
            '[' => {
                flush_segment(&mut pointer, &mut buf);
                let key = match chars.peek() {
                    Some(&q @ ('\'' | '"')) => {
                        chars.next();
                        read_quoted_key(&mut chars, q)
                    }
                    _ => read_unquoted_bracket(&mut chars),
                };
                pointer.push('/');
                pointer.push_str(&pointer_escape(&key));
            }
            _ => buf.push(c),
        }
    }
    flush_segment(&mut pointer, &mut buf);
    pointer
}

fn flush_segment(pointer: &mut String, buf: &mut String) {
    if !buf.is_empty() {
        pointer.push('/');
        pointer.push_str(&pointer_escape(buf));
        buf.clear();
    }
}

fn read_quoted_key<I: Iterator<Item = char>>(
    chars: &mut std::iter::Peekable<I>,
    quote: char,
) -> String {
    let mut key = String::new();
    while let Some(c) = chars.next() {
        if c == quote {
            break;
        }
        if c == '\\' {
            if let Some(esc) = chars.next() {
                key.push(esc);
            }
        } else {
            key.push(c);
        }
    }
    // Optional — caller's input may be malformed and lack the closing `]`.
    if let Some(&']') = chars.peek() {
        chars.next();
    }
    key
}

fn read_unquoted_bracket<I: Iterator<Item = char>>(chars: &mut std::iter::Peekable<I>) -> String {
    let mut key = String::new();
    for c in chars.by_ref() {
        if c == ']' {
            break;
        }
        key.push(c);
    }
    key
}

fn pointer_escape(s: &str) -> String {
    // RFC 6901: '~' must be escaped first, then '/'.
    s.replace('~', "~0").replace('/', "~1")
}

/// Set a value at the given JSON Pointer path in a parsed JSON document.
/// Empty pointer replaces the root. Returns Err if the path doesn't exist.
pub(crate) fn json_set_value(
    doc: &mut serde_json::Value,
    pointer: &str,
    new_value: serde_json::Value,
) -> Result<(), String> {
    if pointer.is_empty() {
        *doc = new_value;
        return Ok(());
    }
    match doc.pointer_mut(pointer) {
        Some(target) => {
            *target = new_value;
            Ok(())
        }
        None => Err(format!("JSON path '{}' not found in document", pointer)),
    }
}

/// The app id a `data/`-relative path belongs to, if it lives under `apps/<id>/`.
/// `None` for non-app paths or a bare `apps/` with no id segment.
fn data_path_app_id(data_path: &str) -> Option<&str> {
    data_path
        .strip_prefix("apps/")?
        .split('/')
        .next()
        .filter(|s| !s.is_empty())
}

/// The app id iff `data_path` is *exactly* an app's manifest — `apps/<id>/manifest.json`.
/// We only fire `App*` lifecycle events on the manifest, because the apps list
/// renders solely manifest-derived fields (`id`/`name`/`description`/`icon`):
/// an app becomes real when its manifest appears (`AppCreated`), its list row
/// changes only when the manifest is edited (`AppUpdated`), and it leaves the
/// list when the manifest is gone (`AppDeleted`). Edits to `index.html` /
/// scripts / other files change nothing the list shows, so they emit nothing —
/// which also avoids a burst of `AppUpdated` per file while an app is built.
fn data_path_app_manifest_id(data_path: &str) -> Option<&str> {
    let rest = data_path.strip_prefix("apps/")?;
    let (id, file) = rest.split_once('/')?;
    (!id.is_empty() && file == "manifest.json").then_some(id)
}

/// Pure decision: which `App*` *birth/death* event (if any) a manifest file op
/// implies, from the manifest's presence before/after the op. `name` is read
/// lazily (only when an event needs it). Kept separate from the emit so the
/// matrix is unit-testable without a live engine.
///
/// The file tools own only `AppCreated` (manifest newly appears) and
/// `AppDeleted` (manifest removed) — the precise, immediate birth/death
/// signals. `AppUpdated` is NOT emitted here: edits are coalesced into one
/// per-app `AppUpdated` at END OF TURN by the agentic loop's `modified_app_uis`
/// post-loop pass (`agentic_loop/run.rs`), so iterating on an app doesn't spray
/// an `AppUpdated` per keystroke. An edit to an existing manifest therefore
/// returns `None` here.
fn app_lifecycle_event(
    app_id: &str,
    name: impl FnOnce() -> Option<String>,
    exists_now: bool,
    existed_before: bool,
    is_delete: bool,
) -> Option<SystemEvent> {
    match (exists_now, is_delete, existed_before) {
        // Manifest just appeared → the app was born.
        (true, false, false) => Some(SystemEvent::AppCreated {
            app_id: app_id.to_string(),
            name: name(),
            actor: None,
        }),
        // Manifest removed (and it was there before) → the app is gone.
        (false, true, true) => Some(SystemEvent::AppDeleted {
            app_id: app_id.to_string(),
            actor: None,
        }),
        // Edit of an existing manifest, or anything else → handled at end of
        // turn (AppUpdated) or not list-relevant.
        _ => None,
    }
}

impl LucidosEngine {
    /// If `data_path` is an app's `manifest.json`, emit the matching `App*`
    /// lifecycle event so the disk-backed apps list refreshes live (mirrors the
    /// `artifacts/` emission the file tools already do). Only the manifest is
    /// instrumented — see `data_path_app_manifest_id`.
    ///
    /// Without this, an app *created* via the raw file tools
    /// (`write_file`/`copy_file`) instead of `create_app` is committed to disk
    /// but emits no `AppCreated` — so no open page's `appsList` (a disk-scan
    /// projection refreshed by these events) learns about it until a full
    /// reload. That was the "App no longer exists" toast for a live app: the
    /// creating thread scaffolded with `write_file` (no `create_app`), no
    /// `AppCreated` fired, and a viewing page's stale list reported it gone.
    /// `AppCreated` is the load-bearing case; `AppDeleted` covers removal.
    /// `AppUpdated` is deliberately NOT emitted here — manifest *edits* are
    /// coalesced into one per-app `AppUpdated` at end of turn by the agentic
    /// loop's `modified_app_uis` pass, so iterating doesn't spray an event per
    /// write. This is the chat-file-tool channel of the per-channel refresh-hint
    /// model — it does NOT cover apps written via `run_bash` / `run_python` (no
    /// chokepoint to instrument); those need a reload, but creating an app that
    /// way is a deep deviation from the documented `create_app` workflow
    /// (`system-knowhow/building-an-app.md`).
    ///
    /// `existed_before` = whether the manifest was present before this op,
    /// snapshotted by the caller BEFORE touching disk. `is_delete` flips the
    /// post-op reading: a delete that removed the manifest is `AppDeleted`.
    /// `actor: None` matches the artifact emits (agent-driven file ops).
    async fn emit_app_event_for_data_path(
        &self,
        data_path: &str,
        existed_before: bool,
        is_delete: bool,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let Some(app_id) = data_path_app_manifest_id(data_path) else {
            return Ok(());
        };
        let exists_now = self.app_manager.app_exists(app_id);
        let Some(event) = app_lifecycle_event(
            app_id,
            || self.app_manager.app_name(app_id),
            exists_now,
            existed_before,
            is_delete,
        ) else {
            return Ok(());
        };
        log!(
            "[Apps] file tool emitting {} for app '{}' (path {})",
            event.event_type(),
            app_id,
            data_path
        );
        self.event_bus.emit(BusEvent::System(event)).await?;
        Ok(())
    }

    /// Record a `data/scripts/` write as authored, so the handshake runner will
    /// run it (ADR 0144). A no-op for every other path, and for every caller
    /// that is not in process.
    ///
    /// It takes `authorship` rather than assuming it, so the whole decision is
    /// here and a call site cannot forget its half. `data/scripts/` stays
    /// writable over the API, and an app UI writing there looks exactly like
    /// the Files panel, so an HTTP write must never record.
    async fn record_script_authorship(
        &self,
        authorship: WriteAuthorship,
        full_path: &std::path::Path,
        bytes: &[u8],
    ) {
        let Some(rel) =
            crate::core::handshake_approvals::workspace_relative(self.workspace_path(), full_path)
        else {
            return;
        };
        if !records_script_authorship(authorship, &rel) {
            return;
        }
        match crate::core::handshake_approvals::record(self.workspace_path(), &rel, bytes) {
            // Unchanged content re-recorded is not news, so it announces nothing.
            Ok(false) => {}
            Ok(true) => {
                self.event_bus
                    .emit_or_log(
                        BusEvent::System(SystemEvent::HandshakeScriptApproved {
                            path: rel,
                            source: crate::core::handshake_approvals::ApprovalSource::Authored,
                            actor: None,
                        }),
                        "[Files] HandshakeScriptApproved",
                    )
                    .await;
            }
            // The write landed and only the record failed. Failing the write
            // now would be a lie, so the log is the signal here, and the
            // runner's own refusal is what the user meets.
            Err(e) => log!(
                "[Files] Failed to record handshake approval for {}: {}",
                rel,
                e
            ),
        }
    }

    /// Drop a `data/scripts/` file's approval, so a later file at the same path
    /// starts unapproved. A no-op for every other path.
    async fn forget_script_authorship(&self, full_path: &std::path::Path) {
        let Some(rel) =
            crate::core::handshake_approvals::workspace_relative(self.workspace_path(), full_path)
        else {
            return;
        };
        // `delete_file` is a tool call, so the caller is in process by
        // construction. Dropping an approval is safe from anywhere in any
        // case: it can only stop a script running.
        if !records_script_authorship(WriteAuthorship::InProcessTool, &rel) {
            return;
        }
        if let Err(e) = crate::core::handshake_approvals::forget(self.workspace_path(), &rel) {
            log!(
                "[Files] Failed to drop handshake approval for {}: {}",
                rel,
                e
            );
        }
    }

    /// Commit a file change via the appropriate manager: shared user dir, app, or workspace artifact.
    async fn commit_file_change(
        &self,
        data_path: &str,
        full_path: &std::path::Path,
        message: &str,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        if let Some(ud) = self.user_dir() {
            if full_path.starts_with(ud) {
                let rel = full_path.strip_prefix(ud).unwrap();
                crate::core::user_dir::auto_commit(ud, &rel.to_string_lossy(), message);
                return Ok("shared".to_string());
            }
        }
        if let Some(app_path) = data_path.strip_prefix("apps/") {
            Ok(self.app_manager.commit(app_path, message)?)
        } else {
            Ok(self
                .artifact_manager
                .commit_data_path(data_path, message)
                .await?)
        }
    }
}

/// What an edit changed, so the result can state a count.
///
/// Text mode carries both numbers because they can differ, and the difference
/// is the whole point: without `replace_all` the edit rewrites the first match
/// and leaves the rest, which the old single-line result reported exactly as it
/// reported a complete rewrite.
#[derive(Debug)]
pub(crate) enum EditOutcome {
    Text {
        replaced: usize,
        occurrences: usize,
    },
    /// JSON mode sets one value at one pointer. No occurrence count applies.
    JsonValueSet,
}

impl EditOutcome {
    /// The parenthetical the tool result carries, or empty for JSON mode.
    fn summary(&self) -> String {
        match self {
            EditOutcome::JsonValueSet => String::new(),
            EditOutcome::Text {
                replaced,
                occurrences,
            } if replaced == occurrences => {
                format!(" ({replaced} of {occurrences} occurrences replaced)")
            }
            EditOutcome::Text {
                replaced,
                occurrences,
            } => format!(
                " ({replaced} of {occurrences} occurrences replaced, pass replace_all for the rest)"
            ),
        }
    }
}

/// Where an edit landed, and what that implies about durability.
pub(crate) enum EditTarget {
    /// A `data/` file, committed to the workspace repo.
    Workspace { commit: String },
    /// A registered repository's working tree. Deliberately uncommitted: see
    /// `docs/adr/0093-file-tools-reach-registered-repos.md`.
    RepoWorkingTree,
}

pub(crate) struct FileEditResult {
    pub path: String,
    pub outcome: EditOutcome,
    pub target: EditTarget,
}

impl FileEditResult {
    /// The `[ACTION COMPLETED]` line the LLM sees. A repo edit says the tree is
    /// dirty in place of a commit sha, because that is the fact the next actor
    /// needs: nothing has been recorded, and `git diff` is where the work is.
    pub(crate) fn tool_message(&self) -> String {
        match &self.target {
            EditTarget::Workspace { commit } => format!(
                "[ACTION COMPLETED] UPDATED: {}{} (commit: {})",
                self.path,
                self.outcome.summary(),
                commit
            ),
            EditTarget::RepoWorkingTree => format!(
                "[ACTION COMPLETED] UPDATED: {}{} (repo working tree, NOT committed: review with \
                 git diff, or hand it to run_coding_agent to land as a reviewable change)",
                self.path,
                self.outcome.summary()
            ),
        }
    }
}

/// Whether `commit` agrees with the target it was passed alongside.
///
/// `commit: false` is *required* with `repo` rather than merely defaulted, so
/// the recorded tool call states the consequence. A user reading the step sees
/// an uncommitted write as such, instead of inferring it from a missing
/// argument.
fn check_commit_pairing(has_repo: bool, commit: Option<bool>) -> Result<(), String> {
    match (has_repo, commit) {
        (true, Some(true)) => Err(
            "A repo edit is never committed. Pass commit: false, or use run_coding_agent to land \
             the work as a reviewable change."
                .to_string(),
        ),
        (true, None) => Err(
            "A repo edit must state commit: false. It writes the working tree and commits \
             nothing, and the call has to say so."
                .to_string(),
        ),
        (false, Some(false)) => Err(
            "commit: false needs `repo`. Everything the file tools write under data/ is \
             committed; use run_python for an uncommitted workspace write."
                .to_string(),
        ),
        _ => Ok(()),
    }
}

/// Apply a text-mode edit, returning the new content and what it did.
///
/// Counting before replacing is the whole feature: `replacen(.., 1)` rewrites
/// the first match and leaves the rest, and the caller previously had no way to
/// tell that from a complete rewrite.
///
/// An empty `old` is refused outright rather than only when `new` is empty too.
/// Rust matches the empty pattern at every character boundary, so
/// `replace("", "x")` turns `ab` into `xaxbx`. That is never an edit anyone
/// asked for, and against a repo it is a corrupted source file with no commit
/// to recover from.
fn apply_text_edit(
    content: &str,
    old: &str,
    new: &str,
    replace_all: bool,
    path: &str,
) -> Result<(String, EditOutcome), String> {
    if old.is_empty() {
        return Err(
            "old_string is empty. Text mode needs the exact text to find; to create a file or \
             replace it whole, use write_file."
                .into(),
        );
    }
    if old == new {
        return Err("old_string and new_string are identical".into());
    }
    let occurrences = content.matches(old).count();
    if occurrences == 0 {
        return Err(format!("old_string not found in '{}'", path));
    }
    let (replaced, out) = if replace_all {
        (occurrences, content.replace(old, new))
    } else {
        (1, content.replacen(old, new, 1))
    };
    Ok((
        out,
        EditOutcome::Text {
            replaced,
            occurrences,
        },
    ))
}

/// Who asked for a write, for the one thing that turns on it: whether a
/// `data/scripts/` file becomes runnable (ADR 0144).
///
/// It exists because [`edit_file_at_path`](LucidosEngine::edit_file_at_path)
/// has two callers of very different standing. The `edit_file` tool runs in
/// process for the Lucidos Agent. `POST /api/v1/data/edit` is an HTTP caller,
/// and an app UI reaching it is indistinguishable from the Files panel. There
/// is deliberately no `Default`: a new call site has to say which it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WriteAuthorship {
    /// An in-process tool. A `data/scripts/` write records as authored.
    InProcessTool,
    /// The HTTP data surface. Never records, whatever it writes.
    ApiCaller,
}

/// Whether a write at `workspace_rel` may make a handshake script runnable.
///
/// Two conditions, and both are the decision rather than a detail. The caller
/// has to be in process, because an HTTP one cannot be told apart from an app
/// UI. And the file has to be under `data/scripts/`, the only tree the
/// handshake runner reads.
pub(crate) fn records_script_authorship(authorship: WriteAuthorship, workspace_rel: &str) -> bool {
    authorship == WriteAuthorship::InProcessTool && workspace_rel.starts_with("data/scripts/")
}

/// Every input `edit_file_at_path` takes. A struct rather than nine positional
/// arguments, so a call site reads as the tool call it mirrors.
pub(crate) struct EditFileArgs<'a> {
    pub raw_path: &'a str,
    /// Whether this edit may make a handshake script runnable. See
    /// [`WriteAuthorship`].
    pub authorship: WriteAuthorship,
    /// A registered repository's name or id. `raw_path` is relative to its root
    /// instead of to `data/`.
    pub repo: Option<&'a str>,
    pub json_path: Option<&'a str>,
    pub new_value: Option<serde_json::Value>,
    pub old_string: Option<&'a str>,
    pub new_string: Option<&'a str>,
    pub replace_all: bool,
    /// `Some(false)` is required with `repo`, so the recorded call states that
    /// nothing was committed. Without `repo` the edit always commits, so `None`
    /// and `Some(true)` both say something true and are both accepted. The two
    /// pairings that would lie are refused.
    pub commit: Option<bool>,
    pub message: Option<&'a str>,
}

impl LucidosEngine {
    /// Resolve `(repo, repo-relative path)` to a display path and an absolute
    /// one. The display path is `<repo name>/<path>`, so a tool result never
    /// leaks the checkout's absolute location back to the model.
    pub(crate) async fn resolve_repo_path(
        &self,
        repo_arg: &str,
        relative: &str,
    ) -> Result<(String, std::path::PathBuf), String> {
        let repo = super::repo_files::resolve_repo(self.pool(), repo_arg).await?;
        let full = super::repo_files::resolve_in_repo(std::path::Path::new(&repo.path), relative)?;
        let display = format!("{}/{}", repo.name, relative.trim_start_matches("./"));
        Ok((display, full))
    }

    /// The file list `glob_files` and `grep_files` walk for a `repo` argument.
    /// Resolved here rather than inside the tools' `spawn_blocking` because
    /// enumeration shells out to git, which belongs on the async runtime.
    pub(crate) async fn repo_search_entries(
        &self,
        repo_arg: &str,
    ) -> Result<Vec<(String, std::path::PathBuf)>, String> {
        let repo = super::repo_files::resolve_repo(self.pool(), repo_arg).await?;
        super::repo_files::repo_entries(std::path::Path::new(&repo.path)).await
    }
}

impl LucidosEngine {
    /// Edit a file in one of two modes, JSON (`json_path` + `new_value`) or
    /// text (`old_string` + `new_string`), against one of two targets.
    ///
    /// A `data/` target is committed to the workspace repo and announced, as it
    /// always has been. A `repo` target writes a registered repository's
    /// working tree and stops there: no commit, no index write, no event. The
    /// `commit` argument may state the target's own default but never
    /// contradict it, so no call can commit into someone's checkout.
    pub(crate) async fn edit_file_at_path(
        &self,
        args: EditFileArgs<'_>,
    ) -> Result<FileEditResult, String> {
        let EditFileArgs {
            raw_path,
            authorship,
            repo,
            json_path,
            new_value,
            old_string,
            new_string,
            replace_all,
            commit,
            message,
        } = args;

        // Checked before the read, so a rejected call never touches disk.
        check_commit_pairing(repo.is_some(), commit)?;

        let (path, full_path) = match repo {
            Some(repo_arg) => self.resolve_repo_path(repo_arg, raw_path).await?,
            None => {
                let (data_path, full_path) = self.resolve_data_path(raw_path)?;
                if let Some(reason) = read_only_reason(&data_path) {
                    return Err(format!("Cannot edit '{}': {}", data_path, reason));
                }
                (data_path, full_path)
            }
        };
        let path = path.as_str();

        let content = std::fs::read_to_string(&full_path)
            .map_err(|_| format!("File '{}' not found", path))?;

        let mut outcome = EditOutcome::JsonValueSet;
        let new_content = if let Some(jp) = json_path {
            let nv = new_value.ok_or("json_path requires new_value parameter")?;
            let mut doc: serde_json::Value = serde_json::from_str(&content)
                .map_err(|e| format!("File '{}' is not valid JSON: {}", path, e))?;
            let pointer = dot_path_to_pointer(jp);
            if let Err(e) = json_set_value(&mut doc, &pointer, nv) {
                let parent = pointer
                    .rfind('/')
                    .map(|i| &pointer[..pointer.floor_char_boundary(i)])
                    .unwrap_or("");
                let available = if parent.is_empty() {
                    doc.as_object()
                        .map(|o| o.keys().cloned().collect::<Vec<_>>().join(", "))
                } else {
                    doc.pointer(parent)
                        .and_then(|v| v.as_object())
                        .map(|o| o.keys().cloned().collect::<Vec<_>>().join(", "))
                };
                let hint = available
                    .map(|k| format!("\nAvailable keys at parent: {}", k))
                    .unwrap_or_default();
                return Err(format!("{}{}", e, hint));
            }
            serde_json::to_string_pretty(&doc).unwrap() + "\n"
        } else if let Some(os) = old_string {
            let (edited, text_outcome) =
                apply_text_edit(&content, os, new_string.unwrap_or(""), replace_all, path)?;
            outcome = text_outcome;
            edited
        } else {
            return Err("Provide either json_path + new_value or old_string + new_string".into());
        };

        // A repo edit stops at the write. Nothing below applies to it. The lock
        // guards the workspace tree, the commit and the announcements are about
        // `data/`, and an app id cannot come from a repo path.
        if repo.is_some() {
            std::fs::write(&full_path, &new_content)
                .map_err(|e| format!("Failed to write file: {}", e))?;
            return Ok(FileEditResult {
                path: path.to_string(),
                outcome,
                target: EditTarget::RepoWorkingTree,
            });
        }

        let app_existed_before = data_path_app_id(path)
            .map(|id| self.app_manager.app_exists(id))
            .unwrap_or(false);

        let _repo_guard = self.lock_workspace_repo().await;

        std::fs::write(&full_path, &new_content)
            .map_err(|e| format!("Failed to write file: {}", e))?;

        let commit_msg = message
            .filter(|m| !m.is_empty())
            .map(|m| m.to_string())
            .unwrap_or_else(|| format!("Edit {}", path));
        let commit_sha = self
            .commit_file_change(path, &full_path, &commit_msg)
            .await
            .map_err(|e| format!("Failed to commit: {}", e))?;

        if let Some(artifact_path) = path.strip_prefix("artifacts/") {
            // Before the announcement, not after: the cache tracks the file,
            // and the emit below can fail on a transient pool timeout while the
            // edit is already committed. Refreshing after the `?` would leave
            // exactly the stale profile this cache exists to prevent.
            self.user_profile
                .artifact_written(artifact_path, new_content.as_bytes())
                .await;
            self.event_bus
                .emit(BusEvent::System(SystemEvent::ArtifactUpdated {
                    artifact_path: artifact_path.to_string(),
                    commit: commit_sha.clone(),
                    source: None,
                }))
                .await
                .map_err(|e| format!("Failed to emit event: {}", e))?;
        }

        // Ahead of the emit below, which can fail on a transient pool timeout
        // with the edit already committed. Recording after the `?` would leave
        // an authored script unapproved, and the user meeting a refusal for a
        // change they asked for. Same ordering, and same reason, as the
        // profile-cache refresh above.
        self.record_script_authorship(authorship, &full_path, new_content.as_bytes())
            .await;

        self.emit_app_event_for_data_path(path, app_existed_before, false)
            .await
            .map_err(|e| format!("Failed to emit app event: {}", e))?;

        let sha_short = &commit_sha[..commit_sha.floor_char_boundary(7)];
        Ok(FileEditResult {
            path: path.to_string(),
            outcome,
            target: EditTarget::Workspace {
                commit: sha_short.to_string(),
            },
        })
    }
}

/// Everything `read_file` does once a path has resolved: binary detection, the
/// PDF and image arms, line slicing and chunked text.
///
/// `display_path` is what the result names, and is the caller's vocabulary: a
/// `data/`-relative path for a workspace file, `<repo>/<path>` for a repo one.
/// Splitting it from `full_path` is what lets both callers share this: the
/// workspace read had the two equal only by coincidence.
fn read_resolved_file(
    display_path: &str,
    full_path: &std::path::Path,
    args: &serde_json::Value,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let extension = lowercase_extension(display_path);

    if crate::core::is_binary_extension(&extension) {
        if !full_path.exists() {
            return Ok(format!(
                "[FILE NOT FOUND] '{}' does not exist",
                display_path
            ));
        }
        if extension == "pdf" {
            // A legacy sidecar from before PDF extraction was removed still
            // surfaces. The engine cannot regenerate one, but older workspaces
            // still carry useful ones.
            let text_sidecar = format!("{}.txt", full_path.display());
            let text_sidecar_path = std::path::Path::new(&text_sidecar);
            if text_sidecar_path.exists() {
                if let Ok(text) = std::fs::read_to_string(text_sidecar_path) {
                    return Ok(format!("[PDF Text Content]\n\n{}", text));
                }
            }

            return Ok(format!(
                "[Binary file: {} - PDF text extraction has been removed from \
                 Lucidos. The PDF is stored as a binary artifact; text content \
                 is not available.]",
                display_path
            ));
        }
        if let Some(media_type) = image_media_type(&extension) {
            let size = match std::fs::metadata(full_path) {
                Ok(m) => m.len(),
                Err(e) => return Err(format!("reading image file: {}", e).into()),
            };
            if size > IMAGE_MAX_BYTES {
                let mb = size as f64 / (1024.0 * 1024.0);
                return Ok(format!(
                    "Image too large to read directly ({:.1} MB). Max 25MB; \
                     smaller images are automatically resized to fit.",
                    mb
                ));
            }
            return match std::fs::read(full_path) {
                Ok(bytes) => Ok(encode_image_for_read(bytes, media_type)),
                Err(e) => Err(format!("reading image file: {}", e).into()),
            };
        }
        return Ok(format!(
            "[Binary file: {} - Cannot display binary content. File exists and is {} bytes.]",
            display_path,
            std::fs::metadata(full_path)
                .map(|m| m.len().to_string())
                .unwrap_or_else(|_| "unknown".to_string())
        ));
    }

    let offset = args.get("offset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
    let line_window = line_window_from_args(args);
    match std::fs::read_to_string(full_path) {
        Ok(content) => {
            // The slice is its own chunk, so sanitize from offset 0.
            let (text, sanitize_offset) = match line_window {
                Some((start, count)) => (slice_lines(&content, start, count), 0),
                None => (content, offset),
            };
            Ok(sanitize_file_content_for_llm(
                text,
                display_path,
                sanitize_offset,
            ))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Err(format!("reading file: file not found: {}", display_path).into())
        }
        Err(e) => Err(format!("reading file: {}", e).into()),
    }
}

fn image_media_type(ext: &str) -> Option<&'static str> {
    match ext {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        _ => None,
    }
}

/// Encode image `bytes` (with source `media_type`) into the `[IMAGE_CONTENT:<type>]\n<base64>`
/// sentinel that `read_file` returns for images. The image is run through
/// `ChatImage::fit_for_llm` — the same fit-to-target step every LLM-bound image path uses —
/// so an oversized iPhone photo is downsampled to fit rather than rejected. Fitting compresses
/// to JPEG when it shrinks the image, so the emitted media type becomes `image/jpeg` for those;
/// the sentinel always names the media type of the bytes actually returned.
///
/// `pub(crate)` so `view_image` (`engine::tools::image`) can emit the same sentinel —
/// the agentic loop's `parse_image_content_marker` lifts it into a vision block.
pub(crate) fn encode_image_for_read(bytes: Vec<u8>, media_type: &str) -> String {
    let fitted = crate::api::ChatImage {
        base64: base64::engine::general_purpose::STANDARD.encode(&bytes),
        mime_type: media_type.to_string(),
    }
    .fit_for_llm();

    // `fit_for_llm` compresses only when over target, and `compress` can't shrink bytes it
    // can't decode (a corrupt file with an image extension), so the payload may still be over
    // target. Don't emit an oversized IMAGE_CONTENT block the model API would reject with an
    // opaque error — fail fast with a clear message instead.
    if fitted.base64.len() > crate::api::MAX_IMAGE_BASE64_BYTES {
        let mb = fitted.base64.len() as f64 / (1024.0 * 1024.0);
        return format!(
            "Image could not be read: it could not be resized to fit ({:.1} MB after \
             compression) — the file may be corrupt or not a valid image.",
            mb
        );
    }

    format!("[IMAGE_CONTENT:{}]\n{}", fitted.mime_type, fitted.base64)
}

/// Lowercased file extension via `Path::extension`, or `""` if none.
fn lowercase_extension(path: &str) -> String {
    std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase()
}

/// File-name suffixes that `read_file` treats as a virtual directory: a path that traverses
/// past such a segment is unzipped on the fly so the LLM can read entries without dropping
/// to `run_python` + `zipfile`. `.lucidos-plugin` reuses the canonical constant in
/// `core::plugins` so the literal lives in one place.
const ARCHIVE_SUFFIXES: &[&str] = &[crate::core::plugins::PLUGIN_ARCHIVE_EXT, ".zip"];

/// Hard ceiling on the uncompressed size of a single zip entry the archive branch will
/// decompress into memory. Defends against zip-bombs (small `.lucidos-plugin` carrying a
/// huge entry that decompresses to multi-GB). Generous vs. real plugin file sizes.
pub(crate) const READ_FILE_FROM_ARCHIVE_MAX_BYTES: u64 = 10 * 1024 * 1024;

/// Slice text content by 1-based line range. `start_line == 0` is treated as `1`.
/// `line_count == None` reads to end. If `start_line` is past the file, returns `""`.
/// Lines are split on `\n`; the joined result uses `\n` between lines (no trailing
/// newline added). The function does not append slice metadata — callers compose that.
pub(crate) fn slice_lines(content: &str, start_line: usize, line_count: Option<usize>) -> String {
    let start_line = start_line.max(1);
    let take = line_count.unwrap_or(usize::MAX);
    let mut iter = content.split('\n').skip(start_line - 1).take(take);
    let Some(first) = iter.next() else {
        return String::new();
    };
    let mut out = String::from(first);
    for line in iter {
        out.push('\n');
        out.push_str(line);
    }
    out
}

/// If `data_path` traverses *into* a `.lucidos-plugin` or `.zip` segment (i.e. there is at
/// least one path component after the archive), split into `(archive_data_path, inner_path)`.
/// Returns `None` for plain paths and for archives with no inner path (so the caller can still
/// read the archive bytes themselves). The first matching segment wins — nested archives are
/// not transparently traversed.
pub(crate) fn split_archive_path(data_path: &str) -> Option<(String, String)> {
    // Fast path: 99% of read_file calls hit a non-archive path. Skip the segment walk
    // and the Vec allocation if no archive suffix appears anywhere.
    if !ARCHIVE_SUFFIXES.iter().any(|s| data_path.contains(s)) {
        return None;
    }
    let segments: Vec<&str> = data_path.split('/').collect();
    for (i, seg) in segments.iter().enumerate() {
        if i + 1 >= segments.len() {
            break;
        }
        if ARCHIVE_SUFFIXES.iter().any(|s| seg.ends_with(s)) {
            let archive = segments[..=i].join("/");
            let inner = segments[i + 1..].join("/");
            // Trailing slash like `foo.zip/` splits to ["foo.zip", ""] — treat as no inner.
            if inner.is_empty() {
                return None;
            }
            return Some((archive, inner));
        }
    }
    None
}

/// Pull `start_line` / `line_count` from the LLM's `read_file` args. Returns `None` when the
/// caller didn't ask for line slicing; otherwise `Some((start_line, line_count))` where a
/// missing `start_line` defaults to `1` and a missing `line_count` means "to end of file".
pub(crate) fn line_window_from_args(args: &serde_json::Value) -> Option<(usize, Option<usize>)> {
    let start = args
        .get("start_line")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize);
    let count = args
        .get("line_count")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize);
    match (start, count) {
        (None, None) => None,
        (s, c) => Some((s.unwrap_or(1), c)),
    }
}

/// Read a single entry from a zip archive at `archive_path` as UTF-8 text. Errors carry the
/// inner name so the LLM can correct typos without re-listing the archive. Inner names are
/// validated against the same zip-slip policy `extract_zip` uses, so `..`/leading-`/`/`\`/
/// empty names are rejected before the lookup. The uncompressed entry size is checked
/// against `max_uncompressed` before decompression — protects against zip-bombs.
pub(crate) fn read_text_from_zip(
    archive_path: &std::path::Path,
    inner_path: &str,
    max_uncompressed: u64,
) -> Result<String, String> {
    use std::io::Read as _;
    crate::core::plugins::validate_archive_entry_path(inner_path)
        .map_err(|e| format!("rejected zip entry '{}': {}", inner_path, e))?;
    let file = std::fs::File::open(archive_path).map_err(|e| {
        format!(
            "open archive {}: {}",
            crate::core::home_path::abbreviate(archive_path),
            e
        )
    })?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| format!("read archive: {}", e))?;
    let mut entry = zip
        .by_name(inner_path)
        .map_err(|e| format!("zip entry '{}' not found: {}", inner_path, e))?;
    let size = entry.size();
    if size > max_uncompressed {
        return Err(format!(
            "zip entry '{}' too large: uncompressed size {} bytes exceeds cap of {} bytes",
            inner_path, size, max_uncompressed
        ));
    }
    let mut buf = String::new();
    entry
        .read_to_string(&mut buf)
        .map_err(|e| format!("read entry '{}': {}", inner_path, e))?;
    Ok(buf)
}

/// If `inner_path` looks like a binary file by extension, return a clear short-circuit
/// message for the archive branch — `read_text_from_zip` would otherwise fail with an
/// opaque "stream did not contain valid UTF-8" error. Returns `None` for text-shaped
/// inner paths so the archive branch keeps reading them as text.
pub(crate) fn archive_entry_unsupported_message(inner_path: &str) -> Option<String> {
    if !crate::core::is_binary_extension(&lowercase_extension(inner_path)) {
        return None;
    }
    Some(format!(
        "[Binary entry '{}' inside archive — read_file inside zip/.lucidos-plugin \
         supports text only. Install the plugin or extract the archive first to \
         inspect binary contents.]",
        inner_path
    ))
}

/// Tool-result text the agentic loop substitutes for an `[IMAGE_CONTENT:…]`
/// sentinel once it has lifted the bytes into a real LLM image block. The
/// image itself is a sibling block in the same message, hence "below".
///
/// Only the tools that show the model an image it *explicitly asked for*
/// (`view_image`, `read_file` on an image file) produce this — ambient captures
/// (`capture_app`, `browser_screenshot`) go through a different branch. That
/// distinction is what the context trimmer keys off to decide which images stay
/// in vision for the whole turn; see `context::trim_context_if_needed`.
pub(crate) const EXPLICIT_IMAGE_RESULT_TEXT: &str = "[Image file displayed to you below]";

/// Parse a `[IMAGE_CONTENT:type]\n<base64>` sentinel produced by `read_file` for image files.
/// Returns `(media_type, base64_data)` if matched, `None` otherwise. Single source of truth
/// for the format; both the agentic loop (which lifts the bytes into an LLM image block) and
/// the persistence path (which strips them) call this.
pub(crate) fn parse_image_content_marker(s: &str) -> Option<(&str, &str)> {
    let rest = s.strip_prefix("[IMAGE_CONTENT:")?;
    let end_bracket = rest.find("]\n")?;
    Some((&rest[..end_bracket], rest[end_bracket + 2..].trim()))
}

/// If `s` is an `[IMAGE_CONTENT:...]\n<base64>` sentinel, return a small stub that names the
/// media type and approximate decoded size. Returns `None` for non-matching input.
///
/// Why: the LLM-facing path (agentic_loop) parses this sentinel and lifts the bytes into a proper
/// image content block before calling the model, so the base64 in the tool result string is dead
/// weight on disk and on the wire. A 2 MB PNG read by `read_file` becomes a ~50-byte stub.
pub(crate) fn strip_image_content_marker(s: &str) -> Option<String> {
    let (media_type, b64) = parse_image_content_marker(s)?;
    let approx_bytes = (b64.len() * 3) / 4;
    Some(format!(
        "[image {}, {} omitted, not embedded in event]",
        media_type,
        crate::core::format_byte_size(approx_bytes),
    ))
}

impl LucidosEngine {
    pub(crate) async fn execute_file_tool(
        &self,
        name: &str,
        args: &serde_json::Value,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        match name {
            "read_file" => {
                let raw_path = args["path"].as_str().unwrap_or("");

                // A registered repository is addressed by name, never by path,
                // so `path` is repo-relative and skips `normalize_data_path`
                // entirely. Ahead of archive traversal, which would otherwise
                // claim any repo path with a `.zip` segment and then resolve it
                // against `data/`. Everything past resolution is shared.
                if let Some(repo_arg) = args.get("repo").and_then(|v| v.as_str()) {
                    let (display, full) = match self.resolve_repo_path(repo_arg, raw_path).await {
                        Ok(p) => p,
                        Err(e) => return Ok(format!("Error: {}", e)),
                    };
                    return read_resolved_file(&display, &full, args);
                }

                // Archive traversal: a path like
                // `artifacts/plugins/foo.lucidos-plugin/apps/x/index.html` reads the named
                // entry out of the zip on the fly, so the LLM doesn't need to drop to
                // `run_python` + `zipfile` just to peek inside.
                if let Some((archive_data_path, inner)) = split_archive_path(raw_path) {
                    // Binary inner entries (images, PDFs, nested archives) would crash
                    // read_text_from_zip's UTF-8 decode — short-circuit with an actionable
                    // message instead of a confusing stream error. Image/PDF support
                    // inside archives is intentionally out of scope.
                    if let Some(msg) = archive_entry_unsupported_message(&inner) {
                        return Ok(msg);
                    }
                    let (_archive_norm, archive_full) =
                        match self.resolve_data_path(&archive_data_path) {
                            Ok(p) => p,
                            Err(e) => return Ok(format!("Error: {}", e)),
                        };
                    if !archive_full.exists() {
                        return Ok(format!(
                            "[FILE NOT FOUND] archive '{}' does not exist",
                            archive_data_path
                        ));
                    }
                    let content = match read_text_from_zip(
                        &archive_full,
                        &inner,
                        READ_FILE_FROM_ARCHIVE_MAX_BYTES,
                    ) {
                        Ok(s) => s,
                        Err(e) => return Err(format!("reading from archive: {}", e).into()),
                    };
                    let display_path = format!("{}/{}", archive_data_path, inner);
                    let sliced = match line_window_from_args(args) {
                        Some((start, count)) => slice_lines(&content, start, count),
                        None => content,
                    };
                    return Ok(sanitize_file_content_for_llm(sliced, &display_path, 0));
                }

                let (data_path, full_path) = match self.resolve_data_path(raw_path) {
                    Ok(p) => p,
                    Err(e) => return Ok(format!("Error: {}", e)),
                };
                read_resolved_file(data_path.as_str(), &full_path, args)
            }
            "write_file" => {
                let content = args["content"].as_str().unwrap_or("");

                let (data_path, full_path) =
                    match self.resolve_data_path(args["path"].as_str().unwrap_or("")) {
                        Ok(p) => p,
                        Err(e) => return Ok(format!("Error: {}", e)),
                    };
                let path = data_path.as_str();

                if let Some(reason) = read_only_reason(path) {
                    return Ok(format!("Error: Cannot write '{}': {}", path, reason));
                }

                // SAFEGUARD: Reject empty content (prevents "deleting" via empty write)
                if content.trim().is_empty() {
                    return Ok(
                        "Error: Cannot write empty content. Provide actual file content."
                            .to_string(),
                    );
                }

                let file_exists = full_path.exists();
                // Snapshot before writing: did the app already exist? Decides
                // AppCreated vs nothing for a write to apps/<id>/manifest.json
                // (see `app_lifecycle_event`); a no-op for every other path.
                let app_existed_before = data_path_app_id(path)
                    .map(|id| self.app_manager.app_exists(id))
                    .unwrap_or(false);

                // SAFEGUARD: Only block overwrites of binary imports (PDFs, images, etc.)
                // Text files can always be edited since we have git versioning
                let extension = path.rsplit('.').next().unwrap_or("").to_lowercase();
                let is_binary_import = path.starts_with("artifacts/imported/")
                    && crate::core::is_binary_extension(&extension);

                if file_exists && is_binary_import {
                    return Ok(format!(
                        "Error: Cannot overwrite binary import '{}'. Delete it first or create a new file.",
                        path
                    ));
                }

                let _repo_guard = self.lock_workspace_repo().await;

                // Create parent directories and write
                if let Some(parent) = full_path.parent() {
                    std::fs::create_dir_all(parent)
                        .map_err(|e| format!("Failed to create directories: {}", e))?;
                }
                std::fs::write(&full_path, content)
                    .map_err(|e| format!("Failed to write file: {}", e))?;

                // Commit via appropriate manager
                let commit_msg = args
                    .get("message")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| {
                        let action = if file_exists { "Update" } else { "Add" };
                        format!("{} {}", action, path)
                    });
                let commit_sha = self
                    .commit_file_change(path, &full_path, &commit_msg)
                    .await?;

                if let Some(artifact_path) = path.strip_prefix("artifacts/") {
                    // Keep in-memory profile cache in sync. Not memory indexing:
                    // this is the user_profile read-cache backing extraction
                    // context. Ahead of the emit, which can fail once the write
                    // is already committed and would take the refresh with it.
                    self.user_profile
                        .artifact_written(artifact_path, content.as_bytes())
                        .await;
                    self.event_bus
                        .emit(BusEvent::System(SystemEvent::artifact_change(
                            file_exists,
                            artifact_path.to_string(),
                            commit_sha.clone(),
                            None,
                        )))
                        .await?;
                }

                // Before the emit, which can fail with the write already
                // committed. See the same ordering in `edit_file_at_path`.
                self.record_script_authorship(
                    WriteAuthorship::InProcessTool,
                    &full_path,
                    content.as_bytes(),
                )
                .await;

                self.emit_app_event_for_data_path(path, app_existed_before, false)
                    .await?;

                let result_action = if file_exists { "UPDATED" } else { "CREATED" };
                let sha_short = &commit_sha[..commit_sha.floor_char_boundary(7)];
                Ok(format!(
                    "[ACTION COMPLETED] {}: {} (commit: {})",
                    result_action, path, sha_short
                ))
            }
            "edit_file" => {
                let raw_path = args["path"].as_str().unwrap_or("");
                let repo = args.get("repo").and_then(|v| v.as_str());
                match self
                    .edit_file_at_path(EditFileArgs {
                        raw_path,
                        // The Lucidos Agent's own tool call, in process.
                        authorship: WriteAuthorship::InProcessTool,
                        repo,
                        json_path: args.get("json_path").and_then(|v| v.as_str()),
                        new_value: args.get("new_value").cloned(),
                        old_string: args.get("old_string").and_then(|v| v.as_str()),
                        new_string: args.get("new_string").and_then(|v| v.as_str()),
                        replace_all: args["replace_all"].as_bool().unwrap_or(false),
                        commit: args.get("commit").and_then(|v| v.as_bool()),
                        message: args.get("message").and_then(|v| v.as_str()),
                    })
                    .await
                {
                    Ok(r) => Ok(r.tool_message()),
                    Err(e) if e.contains("old_string not found") => {
                        // Show file content so the LLM can retry with the correct old_string
                        let resolved = match repo {
                            Some(repo_arg) => self
                                .resolve_repo_path(repo_arg, raw_path)
                                .await
                                .map(|(_, p)| p),
                            None => self.resolve_data_path(raw_path).map(|(_, p)| p),
                        };
                        let shown = match resolved
                            .and_then(|p| std::fs::read_to_string(p).map_err(|e| e.to_string()))
                        {
                            Ok(content) if content.len() > 15000 => {
                                format!("{}...\n[truncated, {} total chars — call read_file to see the rest]",
                                    &content[..content.floor_char_boundary(15000)], content.len())
                            }
                            Ok(content) => content,
                            Err(_) => "Call read_file to see the current content.".to_string(),
                        };
                        Ok(format!(
                            "Error: {}. The file was likely modified by a previous edit. Use the current file content below to construct the correct old_string:\n\n{}",
                            e, shown
                        ))
                    }
                    Err(e) => Ok(format!("Error: {}", e)),
                }
            }
            "list_files" => {
                // list_artifacts already walks all browsable data/ directories
                let all_files = match self.artifact_manager.list_artifacts() {
                    Ok(files) => files,
                    Err(e) => return Err(format!("listing files: {}", e).into()),
                };

                Ok(all_files.join("\n"))
            }
            "glob_files" => {
                use super::search;
                let pattern = args
                    .get("pattern")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                // Forward the raw arg; `search::glob_files` owns the default
                // (when None) and the clamp via `GLOB_LIMIT`.
                let limit = args
                    .get("limit")
                    .and_then(|v| v.as_u64())
                    .map(|n| n as usize);
                let repo_entries = match args.get("repo").and_then(|v| v.as_str()) {
                    Some(repo_arg) => match self.repo_search_entries(repo_arg).await {
                        Ok(entries) => Some(entries),
                        Err(e) => return Ok(format!("Error: {}", e)),
                    },
                    None => None,
                };
                let workspace = self.workspace_path().to_path_buf();
                let result = tokio::task::spawn_blocking(move || match repo_entries {
                    Some(entries) => search::glob_entries(entries, &pattern, limit),
                    None => search::glob_files(&workspace, &pattern, limit),
                })
                .await
                .map_err(|e| format!("glob_files task panicked: {}", e))?;
                match result {
                    // The `Error:` prefix is load-bearing: `lift_legacy_string`
                    // keys the ToolResult success bit off it, so a message that
                    // merely starts with "Error " would reach the model stamped
                    // as a SUCCESSFUL call.
                    Ok(r) => Ok(serde_json::to_string(&r)
                        .unwrap_or_else(|e| format!("Error: serializing glob result: {}", e))),
                    Err(e) => Ok(format!("Error: {}", e)),
                }
            }
            "grep_files" => {
                use super::search;
                let pattern = args
                    .get("pattern")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let path_glob = args
                    .get("path_glob")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let case_insensitive = args
                    .get("case_insensitive")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                // Forward the raw args; `search::grep_files` owns the defaults
                // (when None) and the clamps via `GREP_MAX_MATCHES` /
                // `GREP_CONTEXT_LINES`.
                let max_matches = args
                    .get("max_matches")
                    .and_then(|v| v.as_u64())
                    .map(|n| n as usize);
                let context_lines = args
                    .get("context_lines")
                    .and_then(|v| v.as_u64())
                    .map(|n| n as usize);
                let repo_entries = match args.get("repo").and_then(|v| v.as_str()) {
                    Some(repo_arg) => match self.repo_search_entries(repo_arg).await {
                        Ok(entries) => Some(entries),
                        Err(e) => return Ok(format!("Error: {}", e)),
                    },
                    None => None,
                };
                let workspace = self.workspace_path().to_path_buf();
                let result = tokio::task::spawn_blocking(move || match repo_entries {
                    Some(entries) => search::grep_entries(
                        entries,
                        &pattern,
                        path_glob.as_deref(),
                        case_insensitive,
                        max_matches,
                        context_lines,
                    ),
                    None => search::grep_files(
                        &workspace,
                        &pattern,
                        path_glob.as_deref(),
                        case_insensitive,
                        max_matches,
                        context_lines,
                    ),
                })
                .await
                .map_err(|e| format!("grep_files task panicked: {}", e))?;
                match result {
                    // Same `Error:` prefix contract as `glob_files` above.
                    Ok(r) => Ok(serde_json::to_string(&r)
                        .unwrap_or_else(|e| format!("Error: serializing grep result: {}", e))),
                    Err(e) => Ok(format!("Error: {}", e)),
                }
            }
            "copy_file" => {
                let (src_data_path, src_path) =
                    match self.resolve_data_path(args["source"].as_str().unwrap_or("")) {
                        Ok(p) => p,
                        Err(e) => return Ok(format!("Error: {}", e)),
                    };
                let source = src_data_path.as_str();
                let (dst_data_path, dst_path) =
                    match self.resolve_data_path(args["destination"].as_str().unwrap_or("")) {
                        Ok(p) => p,
                        Err(e) => return Ok(format!("Error: {}", e)),
                    };

                if let Some(reason) = read_only_reason(&dst_data_path) {
                    return Ok(format!(
                        "Error: Cannot copy to '{}': {}",
                        dst_data_path, reason
                    ));
                }

                if !src_path.exists() {
                    return Ok(format!("Error: Source file '{}' not found", source));
                }

                let file_exists = dst_path.exists();
                let app_existed_before = data_path_app_id(&dst_data_path)
                    .map(|id| self.app_manager.app_exists(id))
                    .unwrap_or(false);

                let _repo_guard = self.lock_workspace_repo().await;

                // Create parent directories
                if let Some(parent) = dst_path.parent() {
                    std::fs::create_dir_all(parent)
                        .map_err(|e| format!("Failed to create directories: {}", e))?;
                }
                std::fs::copy(&src_path, &dst_path)
                    .map_err(|e| format!("Failed to copy file: {}", e))?;

                // Commit
                let commit_msg = args
                    .get("message")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| format!("Copy {} to {}", source, &dst_data_path));
                let commit_sha = if let Some(app_path) = dst_data_path.strip_prefix("apps/") {
                    self.app_manager.commit(app_path, &commit_msg)?
                } else {
                    self.artifact_manager
                        .commit_data_path(&dst_data_path, &commit_msg)
                        .await?
                };

                // Emit event for artifacts
                if dst_data_path.starts_with("artifacts/") {
                    let artifact_path = dst_data_path.strip_prefix("artifacts/").unwrap();
                    // A copy can land ON the profile, and the new bytes exist
                    // only on disk here, so re-read rather than assume the
                    // source content is what arrived. Before the emit, which can
                    // fail with the copy already committed.
                    self.user_profile
                        .artifact_written_on_disk(self.workspace_path(), artifact_path)
                        .await;
                    self.event_bus
                        .emit(BusEvent::System(SystemEvent::artifact_change(
                            file_exists,
                            artifact_path.to_string(),
                            commit_sha.clone(),
                            None,
                        )))
                        .await?;
                }

                // The bytes landed by copy rather than by a write, so read
                // back what is actually there. Before the emit, which can fail
                // with the copy already committed.
                match std::fs::read(&dst_path) {
                    Ok(bytes) => {
                        self.record_script_authorship(
                            WriteAuthorship::InProcessTool,
                            &dst_path,
                            &bytes,
                        )
                        .await
                    }
                    // The copy landed and only the read-back failed, so the
                    // file stays unapproved. Fail-closed, and the runner's own
                    // refusal names the fix.
                    Err(e) => log!(
                        "[Files] Could not read {} back to record it: {}",
                        dst_path.display(),
                        e
                    ),
                }

                self.emit_app_event_for_data_path(&dst_data_path, app_existed_before, false)
                    .await?;

                let result_action = if file_exists { "OVERWRITTEN" } else { "COPIED" };
                let sha_short = &commit_sha[..commit_sha.floor_char_boundary(7)];
                Ok(format!(
                    "[ACTION COMPLETED] {}: {} → {} (commit: {})",
                    result_action, source, &dst_data_path, sha_short
                ))
            }
            "delete_file" => {
                let (data_path, full_path) =
                    match self.resolve_data_path(args["path"].as_str().unwrap_or("")) {
                        Ok(p) => p,
                        Err(e) => return Ok(format!("Error: {}", e)),
                    };
                let path = data_path.as_str();

                if let Some(reason) = read_only_reason(path) {
                    return Ok(format!("Error: Cannot delete '{}': {}", path, reason));
                }

                // Check if file exists
                if !full_path.exists() {
                    return Ok(format!("[NO ACTION NEEDED] File '{}' does not exist (may have been deleted already).", path));
                }

                // Plugin-owned files route through uninstall_plugin so the
                // user's confirm panel always fires and the registry stays
                // in sync with disk.
                match crate::engine::tools::plugins::find_plugin_owning_file(&self.pool, path).await
                {
                    Ok(Some(owner)) => {
                        return Ok(format!(
                            "Error: Cannot delete '{}': it belongs to installed plugin '{}' (\"{}\"). \
Use uninstall_plugin('{}') instead — the panel confirms with the user before any files are deleted, \
and emits the PluginUninstalled event so the registry stays in sync.",
                            path, owner.plugin_id, owner.plugin_name, owner.plugin_id
                        ));
                    }
                    Ok(None) => {}
                    Err(e) => {
                        return Ok(format!(
                            "Error: Cannot delete '{}': failed to check plugin ownership: {}",
                            path, e
                        ));
                    }
                }

                let app_existed_before = data_path_app_id(path)
                    .map(|id| self.app_manager.app_exists(id))
                    .unwrap_or(false);

                let _repo_guard = self.lock_workspace_repo().await;

                // Delete and commit via appropriate manager
                let commit_msg = args
                    .get("message")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| format!("Delete {}", path));
                let commit_sha = if let Some(app_path) = path.strip_prefix("apps/") {
                    self.app_manager
                        .delete_file_and_commit(app_path, &commit_msg)?
                } else {
                    self.artifact_manager
                        .delete_data_path_and_commit(&self.event_bus, path, &commit_msg)
                        .await?
                };

                // A deleted profile has to leave the cache too, or the engine
                // keeps rendering it into every chat turn forever.
                if let Some(artifact_path) = path.strip_prefix("artifacts/") {
                    self.user_profile.artifact_deleted(artifact_path).await;
                }

                self.emit_app_event_for_data_path(path, app_existed_before, true)
                    .await?;
                // A later file at this path starts unapproved, rather than
                // inheriting the deleted one's standing.
                self.forget_script_authorship(&full_path).await;

                let sha_short = &commit_sha[..commit_sha.floor_char_boundary(7)];
                Ok(format!(
                    "[ACTION COMPLETED] DELETED: {} (commit: {})",
                    path, sha_short
                ))
            }
            _ => Ok(format!("Unknown file tool: {}", name)),
        }
    }
}

#[cfg(test)]
#[path = "files_tests.rs"]
mod tests;
