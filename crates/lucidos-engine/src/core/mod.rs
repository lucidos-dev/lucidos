pub mod announced_surfaces;
pub mod apps;
pub mod artifacts;
pub mod backup;
pub mod blobs;
pub mod changes;
pub mod changes_projection;
pub mod credentials;
pub mod device_presence;
pub mod devices;
pub mod email;
pub mod environment_variables;
pub mod events;
pub mod home_path;
pub mod image_described_backfill;
pub mod image_migration;
pub mod intents;
pub mod knowhow;
pub mod mcp_servers;
pub mod models;
pub mod oauth;
pub mod pinned_apps;
pub mod plugin_marketplaces;
pub mod plugins;
pub mod preference_catalog;
pub mod preferences;
pub mod repositories;
pub mod shell;
pub mod store;
pub mod system_knowhow;
pub mod user_dir;
pub mod user_path;

/// Get the database URL from the environment, with a default for local dev.
pub fn database_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://lucidos:lucidos@localhost:5432/lucidos".to_string())
}

/// Process-lifetime cache of `pg_env_vars(&database_url())`. The DATABASE_URL
/// env var doesn't change once the engine starts, so we parse the URL once and
/// hand callers a slice they can either iterate (`runtime::claude_code` borrows
/// `&String` into `cmd.env`) or clone-extend (`build_script_env_vars`).
pub fn pg_env_vars_cached() -> &'static [(String, String)] {
    static CACHED: std::sync::LazyLock<Vec<(String, String)>> =
        std::sync::LazyLock::new(|| pg_env_vars(&database_url()));
    CACHED.as_slice()
}

/// Parse a `postgres(ql)://user:password@host[:port]/dbname[?...]` URL into the
/// libpq env-var bundle (`PGUSER`/`PGPASSWORD`/`PGHOST`/`PGPORT`/`PGDATABASE`).
///
/// The point is to inject these into every subprocess we spawn (engine bash
/// tool, Python tool, scheduled scripts, coding-agent sessions) so callers can run
/// `psql -c '…'` bare without ever putting the password in argv. argv is what
/// gets persisted into `CodingAgentToolCalled`/`ToolCalled` event payloads and
/// rendered in the steps UI; env vars are not, so the password never leaks
/// through the events table or the modal.
///
/// Returns an empty Vec if the URL doesn't match the expected shape — we'd
/// rather skip the injection than emit a half-broken bundle that confuses
/// libpq.
pub(crate) fn pg_env_vars(database_url: &str) -> Vec<(String, String)> {
    // The `:password` segment is OPTIONAL: the gateway's embedded (packaged)
    // Postgres backend uses trust auth on loopback and hands the engine a
    // passwordless URL (`postgres://lucidos@127.0.0.1:<port>/lucidos`). Without
    // the optional group the regex wouldn't match and `apply_pg_env` would reject
    // the URL, breaking `pg_restore`/`psql` for a picker restore on a packaged
    // build. An `@` is still required, so a malformed `postgres://no-at-sign/db`
    // remains empty.
    static RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(
            r"^postgres(?:ql)?://([^:@/?]+)(?::([^@/?]*))?@([^:/?]+)(?::(\d+))?/([^?]+)",
        )
        .expect("postgres URL regex must compile")
    });
    let Some(caps) = RE.captures(database_url) else {
        return Vec::new();
    };
    let user = urlencoding::decode(&caps[1])
        .unwrap_or_default()
        .into_owned();
    let host = caps[3].to_string();
    let port = caps
        .get(4)
        .map(|m| m.as_str().to_string())
        .unwrap_or_else(|| "5432".to_string());
    let dbname = urlencoding::decode(&caps[5])
        .unwrap_or_default()
        .into_owned();
    let mut vars = vec![("PGUSER".to_string(), user)];
    // Only emit PGPASSWORD when the URL carried a password segment. A
    // passwordless (trust-auth) URL omits it so libpq doesn't try to send one;
    // an explicit empty password (`user:@host`) still emits an empty PGPASSWORD,
    // preserving prior behavior.
    if let Some(pw) = caps.get(2) {
        let password = urlencoding::decode(pw.as_str())
            .unwrap_or_default()
            .into_owned();
        vars.push(("PGPASSWORD".to_string(), password));
    }
    vars.push(("PGHOST".to_string(), host));
    vars.push(("PGPORT".to_string(), port));
    vars.push(("PGDATABASE".to_string(), dbname));
    vars
}

/// Mask the password segment of any `postgres(ql)://user:password@host…` URL
/// occurring in `s`. Best-effort safety net for the path where the env-var
/// injection (`pg_env_vars`) didn't take — a stray `psql "$DATABASE_URL"` from
/// CC, a Python script that hardcoded the URI, an OAuth-style URL pasted into
/// a curl call. Applied at the spawn-side log line and at the boundary where
/// CC's bash `args` get persisted.
pub fn redact_postgres_secrets(s: &str) -> String {
    static RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"(postgres(?:ql)?://[^:@/?\s]+):[^@/?\s]+@")
            .expect("postgres redaction regex must compile")
    });
    RE.replace_all(s, "$1:***@").into_owned()
}

/// Recursively apply `redact_postgres_secrets` to every string value inside a
/// `serde_json::Value`. Used to scrub the `args` of `CodingAgentToolCalled`
/// events before they reach the event store / SSE stream — the Bash tool's
/// `command` field can carry a hardcoded `postgres://user:pass@…` URL.
///
/// Hot path: every ToolCalled event walks every string. The `contains` guard
/// short-circuits the regex + allocation for any string that doesn't even
/// mention "postgres" (the vast majority of bash commands, file paths, etc.).
pub fn redact_postgres_secrets_in_json(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::String(s) => {
            if !s.contains("postgres") {
                return;
            }
            let redacted = redact_postgres_secrets(s);
            if redacted != *s {
                *s = redacted;
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                redact_postgres_secrets_in_json(item);
            }
        }
        serde_json::Value::Object(map) => {
            for (_, v) in map.iter_mut() {
                redact_postgres_secrets_in_json(v);
            }
        }
        _ => {}
    }
}

/// Shortest secret value we bother scrubbing. Below this, a "secret" is too
/// generic (a 1-char password, "GET", an index) to redact without nuking
/// legitimate text everywhere it appears. Auth tokens and real credentials are
/// far longer, so this never weakens redaction of actual secrets.
pub const MIN_REDACTABLE_SECRET_LEN: usize = 4;

/// Replace every occurrence of each known secret value in `text` with
/// `[REDACTED]`. Used to keep credential material out of logs and surfaced
/// error messages where untrusted code (a WASM signer's `log()` host import, a
/// `script_handshake` script's stderr) could otherwise echo it. Secrets shorter
/// than [`MIN_REDACTABLE_SECRET_LEN`] are skipped (see the const). Longest
/// secrets are scrubbed first so a secret that is a substring of another doesn't
/// leave a partial match behind.
pub fn redact_secret_values(text: &str, secrets: &[String]) -> String {
    let mut ordered: Vec<&String> = secrets
        .iter()
        .filter(|s| s.len() >= MIN_REDACTABLE_SECRET_LEN)
        .collect();
    if ordered.is_empty() {
        return text.to_string();
    }
    ordered.sort_by_key(|s| std::cmp::Reverse(s.len()));
    let mut out = text.to_string();
    for secret in ordered {
        if out.contains(secret.as_str()) {
            out = out.replace(secret.as_str(), "[REDACTED]");
        }
    }
    out
}

/// Directory structure within workspace/data/
pub const DATA_DIR: &str = "data";
pub const ARTIFACTS_DIR: &str = "data/artifacts";
pub const APPS_DIR: &str = "data/apps";
pub const KNOWHOW_DIR: &str = "data/knowhow";
pub const TRIGGERS_DIR: &str = "data/triggers";

pub use apps::{App, AppManager, AppManifest};
pub use artifacts::{
    list_searchable_data_files, ArtifactChange, ArtifactManager, WriteAnnouncement,
};
pub use credentials::{AuthType, Credential, CredentialInfo, CredentialStore};
pub use devices::DeviceStore;
pub use email::{EmailAccount, EmailAccountInfo, EmailStore};
pub use environment_variables::{EnvironmentVariable, EnvironmentVariableStore};
pub use intents::{Intent, IntentStore};
pub use knowhow::{Knowhow, KnowhowDirs, KnowhowStore, KnowhowSummary};
pub use oauth::{OAuthAccount, OAuthAccountInfo, OAuthStore};
pub use pinned_apps::{PinnedAppStore, PinnedAppUi};
pub use shell::{command_shell, TaskOutcome};
pub use system_knowhow::{is_system_knowhow_path, resolve_system_knowhow_dir, SystemKnowhowStore};

/// Migrate legacy `prompts/` directories to `intents/` across the workspace.
///
/// Handles three levels:
/// - `data/prompts/` → individual files become standalone triggers in `data/triggers/`
/// - `data/apps/*/prompts/` → renamed to `data/apps/*/intents/`
/// - `data/apps/*/triggers/` left as-is (already correct)
pub fn migrate_prompts_to_intents(workspace: &std::path::Path) {
    log!("[Migration] Checking for legacy prompts/ directories...");
    let data_dir = workspace.join(DATA_DIR);

    // Top-level prompts become standalone triggers (each .md gets its own dir)
    let top_prompts = data_dir.join("prompts");
    if top_prompts.is_dir() {
        let triggers_dir = data_dir.join("triggers");
        match std::fs::read_dir(&top_prompts) {
            Ok(entries) => {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().and_then(|e| e.to_str()) != Some("md") {
                        continue;
                    }
                    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                        let dest_dir = triggers_dir.join(stem);
                        if dest_dir.exists() {
                            continue;
                        }
                        if let Err(e) = std::fs::create_dir_all(&dest_dir) {
                            log!("[Migration] Failed to create {}: {}", dest_dir.display(), e);
                            continue;
                        }
                        let dest = dest_dir.join(entry.file_name());
                        if let Err(e) = std::fs::rename(&path, &dest) {
                            log!(
                                "[Migration] Failed to move {} → {}: {}",
                                path.display(),
                                dest.display(),
                                e
                            );
                        } else {
                            log!(
                                "[Migration] Moved prompt {} → {}",
                                path.display(),
                                dest.display()
                            );
                        }
                    }
                }
            }
            Err(e) => log!(
                "[Migration] Failed to read {}: {}",
                top_prompts.display(),
                e
            ),
        }
        if std::fs::read_dir(&top_prompts)
            .map(|mut d| d.next().is_none())
            .unwrap_or(false)
        {
            if let Err(e) = std::fs::remove_dir(&top_prompts) {
                log!("[Migration] Failed to remove empty data/prompts/: {}", e);
            } else {
                log!("[Migration] Removed empty data/prompts/");
            }
        }
    }

    // App-level prompts/ → intents/ (simple rename)
    let apps_dir = data_dir.join("apps");
    if apps_dir.is_dir() {
        match std::fs::read_dir(&apps_dir) {
            Ok(entries) => {
                for entry in entries.flatten() {
                    let app_prompts = entry.path().join("prompts");
                    let app_intents = entry.path().join("intents");
                    if app_prompts.is_dir() && !app_intents.exists() {
                        if let Err(e) = std::fs::rename(&app_prompts, &app_intents) {
                            log!(
                                "[Migration] Failed to rename {}/prompts → intents: {}",
                                entry.file_name().to_string_lossy(),
                                e
                            );
                        } else {
                            log!(
                                "[Migration] Renamed {}/prompts → intents",
                                entry.file_name().to_string_lossy()
                            );
                        }
                    }
                }
            }
            Err(e) => log!("[Migration] Failed to read {}: {}", apps_dir.display(), e),
        }
    }
}

/// Engine-managed entries in every workspace's `.gitignore`. Workspaces
/// auto-track their `data/` tree under git (artifacts), so anything the
/// engine writes there that should NOT be versioned has to be listed here.
///
/// Order matters for diff stability: kept in the same order as historical
/// values so existing files don't get rewritten on startup — new entries are
/// appended at the end. `data/.env` is a legacy per-workspace env file: its
/// contents are migrated into the `environment_variables` table at startup and
/// the file is removed (see `environment_variables::migrate_env_file_to_db`).
/// The gitignore entry stays as a safety backstop so any stray `.env` a user
/// drops can never land in the workspace's git-tracked artifacts repo.
const WORKSPACE_GITIGNORE_ENTRIES: &[&str] =
    &[".lucidos/", "data/postgres/", "data/blobs/", "data/.env"];

/// Ensure the workspace `.gitignore` exists and contains every
/// engine-managed entry from `WORKSPACE_GITIGNORE_ENTRIES`. Idempotent:
/// pre-existing entries are left untouched (line-equality), missing
/// entries are appended. Returns `true` when the file was created or
/// updated — callers may use that as a signal to commit the change.
pub fn ensure_workspace_gitignore_entries(workspace: &std::path::Path) -> std::io::Result<bool> {
    let path = workspace.join(".gitignore");
    let existing = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e),
    };
    let already: std::collections::HashSet<&str> = existing.lines().map(str::trim).collect();
    let missing: Vec<&str> = WORKSPACE_GITIGNORE_ENTRIES
        .iter()
        .copied()
        .filter(|entry| !already.contains(entry))
        .collect();
    if missing.is_empty() {
        return Ok(false);
    }
    let mut next = existing.clone();
    if !next.is_empty() && !next.ends_with('\n') {
        next.push('\n');
    }
    for entry in missing {
        next.push_str(entry);
        next.push('\n');
    }
    std::fs::write(&path, next)?;
    Ok(true)
}

pub use device_presence::DevicePresenceStore;
pub use events::EventRow;
pub use mcp_servers::{McpServer, McpServerStore};
pub use models::{Model, ModelStore};
pub use preferences::{
    PreferenceStore, DEFAULT_CHAT_MODEL, DEFAULT_COMMAND_JUDGE_MODEL, DEFAULT_LOCAL_BASE_URL,
    DEFAULT_MAX_TOOL_CALLS, MIN_MAX_TOOL_CALLS, PREF_CHAT_MODEL, PREF_CHAT_REASONING_EFFORT,
    PREF_CODING_AGENT_CLAUDE_PATH, PREF_CODING_AGENT_CODEX_PATH, PREF_IMAGE_MODEL,
    PREF_LOCAL_BASE_URL, PREF_MAX_TOOL_CALLS, PREF_MODEL_COMMAND_JUDGE,
    PREF_MODEL_IMAGE_DESCRIPTION, PREF_MODEL_MEMORY, PREF_MODEL_TITLE, PREF_VERTEX_REGION,
};
pub use store::{
    ConversationMessage, ConversationSnapshot, EventStore, ResponseEvent, SessionMessage, Step,
    ThreadEventRow, ThreadSummary,
};

/// Reset the index to match HEAD's tree before staging — drops any entries
/// left behind by a previous `commit_*` call that staged but didn't commit
/// (e.g. a crash partway through `commit_all_dirty`). No-op on a freshly
/// created repo with no HEAD yet.
pub fn reset_index_to_head(
    repo: &git2::Repository,
    index: &mut git2::Index,
) -> Result<(), git2::Error> {
    if let Ok(head) = repo.head() {
        if let Ok(tree) = head.peel_to_tree() {
            index.read_tree(&tree)?;
        }
    }
    Ok(())
}

/// Brand-new workspace: write `lucidos.toml` pinning the allocated vite
/// port and commit it via the engine's git identity (see `commit_index`),
/// so `git status` is clean from the first boot and the port survives any
/// future `port-registry` drift.
///
/// Returns `true` when a commit was made, `false` when `lucidos.toml`
/// already exists (in which case the file is left strictly untouched —
/// the user may have hand-edited a pin to a different port and we don't
/// clobber that). The caller (engine startup) gates this on "the
/// workspace was just git-init'd by us" so existing workspaces don't get
/// surprised by a freshly-pinned port on a later engine boot.
///
/// Crash-safety: if the commit phase fails after the file has been
/// written, the file is removed so the working tree stays clean. The
/// engine gate is one-shot — next boot won't retry the pin — but at
/// least the workspace isn't left permanently dirty with an untracked
/// lucidos.toml, which would defeat the whole point of the change.
pub fn pin_workspace_vite_port(
    workspace: &std::path::Path,
    vite_port: u16,
) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    let path = workspace.join("lucidos.toml");
    if path.exists() {
        return Ok(false);
    }
    let body = format!("[ports]\nvite = {}\n", vite_port);
    std::fs::write(&path, body)?;

    match commit_new_lucidos_toml(workspace) {
        Ok(()) => Ok(true),
        Err(e) => {
            let _ = std::fs::remove_file(&path);
            Err(e)
        }
    }
}

fn commit_new_lucidos_toml(
    workspace: &std::path::Path,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let repo = git2::Repository::open(workspace)?;
    let mut index = repo.index()?;
    index.add_path(std::path::Path::new("lucidos.toml"))?;
    index.write()?;
    commit_index(&repo, "chore: pin workspace vite port")?;
    Ok(())
}

/// Open the workspace repo at `workspace_path`, stage the given `data/`-relative
/// paths (additions/modifications), and commit them in one commit. Returns the
/// commit sha.
///
/// This is the workspace-repo, fresh-handle sibling of
/// [`crate::core::ArtifactManager::commit_data_paths`] (which commits through the
/// engine's shared `Arc<Mutex<Repository>>`). Plugin install uses this so the
/// files it writes into `data/` are version-controlled exactly like
/// write_file/edit_file commits. Callers that may run concurrently with other
/// workspace-repo mutations MUST hold `lock_workspace_repo` (the install/uninstall
/// confirm flows do).
pub fn commit_data_paths_added(
    workspace_path: &std::path::Path,
    data_relative_paths: &[String],
    message: &str,
) -> Result<String, git2::Error> {
    let repo = git2::Repository::open(workspace_path)?;
    let mut index = repo.index()?;
    reset_index_to_head(&repo, &mut index)?;
    for p in data_relative_paths {
        if is_path_traversal(p) {
            return Err(git2::Error::from_str(&format!(
                "Path traversal not allowed: {}",
                p
            )));
        }
        index.add_path(std::path::Path::new(&format!("data/{}", p)))?;
    }
    index.write()?;
    commit_index(&repo, message)
}

/// Open the workspace repo, stage the deletion of the given `data/`-relative
/// paths (already removed from disk), and commit. Returns `Ok(None)` when none
/// of the paths were tracked — nothing to commit (e.g. a legacy plugin whose
/// files were never committed because of the install git-tracking bug this is
/// the uninstall counterpart of). Paths not present in the index are skipped so
/// a mix of tracked and never-committed files commits the tracked deletions
/// without erroring on the rest.
pub fn commit_data_paths_removed(
    workspace_path: &std::path::Path,
    data_relative_paths: &[String],
    message: &str,
) -> Result<Option<String>, git2::Error> {
    let repo = git2::Repository::open(workspace_path)?;
    let mut index = repo.index()?;
    reset_index_to_head(&repo, &mut index)?;
    let mut staged_any = false;
    for p in data_relative_paths {
        if is_path_traversal(p) {
            continue;
        }
        // `remove_path` errors when the entry isn't in the index (untracked /
        // never-committed file) — tolerate that and keep going.
        if index
            .remove_path(std::path::Path::new(&format!("data/{}", p)))
            .is_ok()
        {
            staged_any = true;
        }
    }
    if !staged_any {
        return Ok(None);
    }
    index.write()?;
    commit_index(&repo, message).map(Some)
}

/// Create a commit from the current index state.
pub fn commit_index(repo: &git2::Repository, message: &str) -> Result<String, git2::Error> {
    let mut index = repo.index()?;
    let tree_id = index.write_tree()?;
    let tree = repo.find_tree(tree_id)?;

    let sig = git2::Signature::now("Lucidos", "lucidos@local")?;

    let parent = repo.head().ok().and_then(|h| h.peel_to_commit().ok());
    let parents: Vec<&git2::Commit> = parent.as_ref().map(|p| vec![p]).unwrap_or_default();

    let commit_id = repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &parents)?;

    Ok(commit_id.to_string())
}

/// Reject paths that would escape the directory they get joined onto: any `..`
/// component, or an absolute path (leading `/` or `\`). This is the single
/// canonical traversal guard for the whole engine — HTTP handlers (`data_api`,
/// `artifacts`, `apps`, `repositories`), LLM file tools (`search`, `import`,
/// `image`, `email`, `intents`), the proxy script runner, and script triggers
/// all funnel through it (directly, or via `crate::api::is_path_traversal`,
/// which re-exports this), so the rule can never drift between call sites.
///
/// Deliberately conservative: it matches `..` anywhere in the string (so even a
/// filename like `a..b` is rejected) and does not normalize or percent-decode
/// first. Both choices fail closed — a false positive costs a rejected request,
/// a false negative costs a path escape.
pub fn is_path_traversal(path: &str) -> bool {
    path.contains("..") || path.starts_with('/') || path.starts_with('\\')
}

/// Check if a file extension indicates a binary (non-text) file.
/// Used to decide whether to read as bytes vs text, skip indexing, etc.
pub fn is_binary_extension(ext: &str) -> bool {
    matches!(
        ext,
        "pdf"
            | "png"
            | "jpg"
            | "jpeg"
            | "gif"
            | "webp"
            | "ico"
            | "zip"
            | "tar"
            | "gz"
            | "rar"
            | "7z"
            | "doc"
            | "docx"
            | "xls"
            | "xlsx"
            | "ppt"
            | "pptx"
            | "mp3"
            | "mp4"
            | "wav"
            | "avi"
            | "mov"
            | "exe"
            | "dll"
            | "so"
            | "dylib"
            | "woff"
            | "woff2"
            | "ttf"
            | "eot"
    )
}

/// Lower-level split: pull the `---\n...\n---\n` YAML header off a markdown
/// file. Returns the trimmed frontmatter block and the body, or None if the
/// document doesn't start with a frontmatter delimiter or doesn't have a
/// closing `---`. The two field-specific parsers below (`parse_md_frontmatter`
/// for `name` + list field; `knowhow::parse_frontmatter` for `name` +
/// scalar `description`) share this prefix; each parses its own fields
/// because their downstream semantics diverge (list-vs-scalar field type;
/// description has a derive-from-body fallback that knowhow needs but
/// `parse_md_frontmatter`'s callers don't).
pub(crate) fn split_md_frontmatter(text: &str) -> Option<(&str, String)> {
    if !text.starts_with("---") {
        return None;
    }
    let parts: Vec<&str> = text.splitn(3, "---").collect();
    if parts.len() < 3 {
        return None;
    }
    Some((
        parts[1].trim(),
        parts[2].trim_start_matches('\n').to_string(),
    ))
}

/// Parse YAML frontmatter from a markdown file.
/// Extracts `name:` and a configurable list field (e.g., `knowhow:`).
/// Returns (name, list_values, body) or None if no valid frontmatter.
pub(crate) fn parse_md_frontmatter(
    text: &str,
    list_field: &str,
) -> Option<(String, Vec<String>, String)> {
    let (frontmatter, body) = split_md_frontmatter(text)?;

    let mut name = None;
    let mut list_values = Vec::new();
    let mut in_list = false;
    let list_prefix = format!("{}:", list_field);

    for line in frontmatter.lines() {
        if let Some(value) = line.strip_prefix("name:") {
            in_list = false;
            let v = value.trim().trim_matches('"');
            if !v.is_empty() {
                name = Some(v.to_string());
            }
        } else if let Some(value) = line.strip_prefix(&list_prefix) {
            let v = value.trim().trim_matches('"');
            if !v.is_empty() {
                list_values.push(v.to_string());
                in_list = false;
            } else {
                in_list = true;
            }
        } else if in_list {
            let trimmed = line.trim();
            if let Some(item) = trimmed.strip_prefix("- ") {
                list_values.push(item.trim().trim_matches('"').to_string());
            } else if !trimmed.is_empty() && !trimmed.starts_with('#') {
                in_list = false;
            }
        }
    }

    let name = name?;
    Some((name, list_values, body))
}

/// Strip null bytes from a string. PostgreSQL JSONB rejects \u0000.
pub fn sanitize_for_jsonb(s: &str) -> String {
    s.replace('\0', "")
}

/// Format a byte count as a human-readable size string (e.g. `1.5 KB`, `2.5 MB`).
pub fn format_byte_size(bytes: usize) -> String {
    if bytes >= 1_048_576 {
        format!("{:.1} MB", bytes as f64 / 1_048_576.0)
    } else if bytes >= 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{} bytes", bytes)
    }
}

#[cfg(test)]
mod format_byte_size_tests {
    use super::format_byte_size;

    #[test]
    fn formats_byte_sizes_across_thresholds() {
        assert_eq!(format_byte_size(0), "0 bytes");
        assert_eq!(format_byte_size(500), "500 bytes");
        assert_eq!(format_byte_size(1023), "1023 bytes");
        assert_eq!(format_byte_size(1024), "1.0 KB");
        assert_eq!(format_byte_size(1536), "1.5 KB");
        assert_eq!(format_byte_size(1_048_576), "1.0 MB");
        assert_eq!(format_byte_size(2_621_440), "2.5 MB");
    }
}

/// Head-truncate `s` to `max` bytes, appending `...` when it actually cuts.
/// Returns `s` unchanged when it already fits. UTF-8-safe.
///
/// **The appended `...` is part of the return value**, so a caller must not
/// also wrap the result in a `"{}..."` format string: a truncated value then
/// renders as six dots, which the frontend accents as two separate markers
/// (`highlightEllipsis` marks only the trailing three). A `describe_tool` arm
/// that wants the trailing in-progress `...` should elide with
/// `middle_truncate`, which marks its cut with `…` and appends nothing.
fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        // Find the last char boundary at or before `max` to avoid
        // panicking on multi-byte UTF-8 characters (e.g. æ, ø, å).
        let end = s.floor_char_boundary(max);
        format!("{}...", &s[..end])
    }
}

/// Truncate `s` to roughly `max` bytes, joining the head and tail with `…`
/// so that meaningful suffixes (filenames, file extensions, query tails)
/// survive. Returns `s` unchanged when it already fits. UTF-8-safe.
fn middle_truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    const ELLIPSIS: &str = "…"; // 3 bytes, 1 char
    if max <= ELLIPSIS.len() {
        return ELLIPSIS.to_string();
    }
    let budget = max - ELLIPSIS.len();
    let head_bytes = budget.div_ceil(2);
    let tail_bytes = budget / 2;
    let head_end = s.floor_char_boundary(head_bytes);
    let tail_start = s.ceil_char_boundary(s.len() - tail_bytes);
    if tail_start <= head_end {
        // Boundary snapping collapsed the cuts (multi-byte char straddling both
        // ends with a tiny budget). Return just the ellipsis rather than the
        // original — the caller asked for shortening.
        return ELLIPSIS.to_string();
    }
    format!("{}{}{}", &s[..head_end], ELLIPSIS, &s[tail_start..])
}

/// First non-empty line of a command, trimmed; the whole string when it has no
/// non-empty line. A step label is one line of HTML, where a newline collapses
/// to a space, so a multi-line script (a heredoc, a `&&` chain split across
/// lines) must condense to its opening line rather than elide across newlines
/// and render as a garbled run-on. Shared by the engine and coding-agent
/// description paths so both condense a command the same way.
fn first_command_line(cmd: &str) -> &str {
    cmd.lines()
        .find(|l| !l.trim().is_empty())
        .map(|l| l.trim())
        .unwrap_or(cmd)
}

/// Summarize a `glob_files` / `grep_files` JSON result as "N items[, truncated]".
/// Falls back to a char count if the JSON can't be parsed (e.g. the handler
/// returned an "Error: ..." string).
fn describe_search_result(result: &str, items_key: &str) -> Option<String> {
    let parsed: serde_json::Value = serde_json::from_str(result).ok()?;
    let count = parsed.get(items_key)?.as_array()?.len();
    let truncated = parsed
        .get("truncated")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    Some(if truncated {
        format!("{} {}, truncated", count, items_key)
    } else {
        format!("{} {}", count, items_key)
    })
}

pub fn describe_tool_result(tool_name: &str, result: &str, success: bool) -> Option<String> {
    if !success {
        let msg = result.lines().next().unwrap_or(result);
        return Some(truncate(msg, 120));
    }
    match tool_name {
        "read_file" => Some(format!("{} chars", result.len())),
        "list_files" => Some(format!("{} items", result.lines().count())),
        "glob_files" => describe_search_result(result, "paths"),
        "grep_files" => describe_search_result(result, "matches"),
        "search_artifacts" => Some(format!("{} results", result.lines().count())),
        "run_python" | "run_bash" => result.lines().next().map(|l| truncate(l, 100)),
        "write_file" | "edit_file" | "create_app" | "execute_intent" => Some("Done".to_string()),
        "git_commit" => result.lines().next().map(|l| truncate(l, 80)),
        "git_diff" | "git_log" => Some(format!("{} lines", result.lines().count())),
        "http_request" | "proxy_request" => result.lines().next().map(|l| truncate(l, 80)),
        _ => {
            if result.len() <= 80 {
                Some(result.to_string())
            } else {
                Some(format!("{} chars", result.len()))
            }
        }
    }
}

/// Human-friendly description of a tool call, used for progress steps in both
/// live streaming (engine.rs) and session replay (store.rs).
pub fn describe_tool(name: &str, args: &serde_json::Value) -> String {
    match name {
        "list_files" => "Listing files in workspace...".to_string(),
        "glob_files" => format!(
            "Globbing {}...",
            args["pattern"].as_str().unwrap_or("pattern")
        ),
        "grep_files" => {
            let pat = args["pattern"].as_str().unwrap_or("pattern");
            if let Some(path_glob) = args.get("path_glob").and_then(|v| v.as_str()) {
                format!("Grepping {} in {}...", pat, path_glob)
            } else {
                format!("Grepping {}...", pat)
            }
        }
        "read_file" => format!("Reading {}...", args["path"].as_str().unwrap_or("file")),
        "write_file" => format!("Writing {}...", args["path"].as_str().unwrap_or("file")),
        "edit_file" => format!("Editing {}...", args["path"].as_str().unwrap_or("file")),
        "copy_file" => format!(
            "Copying {} → {}...",
            args["source"].as_str().unwrap_or("file"),
            args["destination"].as_str().unwrap_or("file")
        ),
        "delete_file" => format!("Deleting {}...", args["path"].as_str().unwrap_or("file")),
        "run_python" => {
            if let Some(path) = args["output_path"].as_str() {
                format!("Running Python → {}...", path)
            } else {
                "Running Python code...".to_string()
            }
        }
        "run_bash" => {
            let cmd = args["command"].as_str().unwrap_or("command");
            format!(
                "Running: {}...",
                middle_truncate(first_command_line(cmd), 60)
            )
        }
        "run_python_background" => "Running Python in background...".to_string(),
        "run_bash_background" => {
            let cmd = args["command"].as_str().unwrap_or("command");
            format!(
                "Running in background: {}...",
                middle_truncate(first_command_line(cmd), 60)
            )
        }
        "bash_output" => "Checking background task output...".to_string(),
        "bash_kill" => "Stopping background task...".to_string(),
        "http_request" => {
            let url = args["url"].as_str().unwrap_or("URL");
            if let Some(path) = args["temp_path"].as_str() {
                format!("Fetching {} → .lucidos/tmp/{}...", url, path)
            } else if let Some(path) = args["output_path"].as_str() {
                format!("Fetching {} → artifacts/{}...", url, path)
            } else {
                format!("Fetching {}...", url)
            }
        }
        "proxy_request" => {
            let name = args["name"].as_str().unwrap_or("proxy");
            let method = args["method"].as_str().unwrap_or("GET");
            let path = args["path"].as_str().unwrap_or("");
            format!("{} via {} proxy: {}...", method, name, path)
        }
        "reload_proxy_modules" => "Reloading WASM auth modules...".to_string(),
        "import_file" => format!(
            "Importing {}...",
            args["source_path"].as_str().unwrap_or("file")
        ),
        "create_trigger" => format!(
            "Creating trigger '{}'...",
            args["name"].as_str().unwrap_or("trigger")
        ),
        "list_triggers" => "Listing triggers...".to_string(),
        "delete_trigger" => format!(
            "Deleting trigger {}...",
            args["trigger_id"].as_str().unwrap_or("trigger")
        ),
        "pause_trigger" => "Pausing trigger...".to_string(),
        "resume_trigger" => "Resuming trigger...".to_string(),
        "list_trigger_groups" => "Listing trigger groups...".to_string(),
        "create_trigger_group" => format!(
            "Creating trigger group '{}'...",
            args["name"].as_str().unwrap_or("group")
        ),
        "rename_trigger_group" => format!(
            "Renaming trigger group to '{}'...",
            args["name"].as_str().unwrap_or("group")
        ),
        "reorder_trigger_groups" => "Reordering trigger groups...".to_string(),
        "delete_trigger_group" => "Deleting trigger group...".to_string(),
        "set_language" => format!(
            "Setting language to {}...",
            args["language"].as_str().unwrap_or("language")
        ),
        "set_timezone" => format!(
            "Setting timezone to {}...",
            args["timezone"].as_str().unwrap_or("timezone")
        ),
        "set_environment_variable" => format!(
            "Setting environment variable {}...",
            args["name"].as_str().unwrap_or("variable")
        ),
        // The flat set_language / set_timezone / enable_push_notifications arms
        // are kept for replaying historical events; new writes go through
        // set_preference.
        "set_preference" => format!(
            "Updating {} setting...",
            args["key"].as_str().unwrap_or("preference")
        ),
        "get_preferences" => "Reading preferences...".to_string(),
        "fetch_news" => format!(
            "Fetching news about '{}'...",
            args["topic"].as_str().unwrap_or("topic")
        ),
        "browser_open" => format!("Opening {}...", args["url"].as_str().unwrap_or("URL")),
        "browser_extract" => format!(
            "Extracting {} from {}...",
            args["format"].as_str().unwrap_or("content"),
            args["selector"].as_str().unwrap_or("elements")
        ),
        "browser_click" => format!(
            "Clicking {}...",
            args["selector"].as_str().unwrap_or("element")
        ),
        "browser_type" => format!(
            "Typing into {}...",
            args["selector"].as_str().unwrap_or("input")
        ),
        "browser_eval" => "Executing JavaScript...".to_string(),
        "browser_screenshot" => {
            let path = args["path"].as_str().unwrap_or("screenshot.png");
            if let Some(url) = args.get("url").and_then(|v| v.as_str()) {
                format!("Taking screenshot of {}...", url)
            } else {
                format!("Taking screenshot {}...", path)
            }
        }
        "browser_close" => "Closing browser...".to_string(),
        "web_search" => format!(
            "Searching for {}...",
            args["query"].as_str().unwrap_or("web")
        ),
        "request_credential" => format!(
            "Requesting {} credentials...",
            args["service_name"].as_str().unwrap_or("API")
        ),
        "connect_oauth_account" => format!(
            "Connecting {} account...",
            args["provider"].as_str().unwrap_or("OAuth")
        ),
        "create_app" => format!(
            "Creating app '{}'...",
            args["name"].as_str().unwrap_or("app")
        ),
        "list_apps" => "Listing apps...".to_string(),
        "list_intents" => "Listing intents...".to_string(),
        "list_knowhow" => "Listing know-how...".to_string(),
        "load_knowhow" => format!(
            "Loading know-how '{}'...",
            args["id"].as_str().unwrap_or("knowhow")
        ),
        "execute_intent" => format!(
            "Executing intent {}...",
            args["intent_id"].as_str().unwrap_or("intent")
        ),
        "refresh_file" => format!("Refreshing {}...", args["path"].as_str().unwrap_or("file")),
        "refresh_app" => format!(
            "Refreshing {}...",
            args.get("app_name")
                .or(args.get("app_id"))
                .and_then(|v| v.as_str())
                .unwrap_or("app")
        ),
        "capture_app" => format!(
            "Capturing {}...",
            args.get("app_name")
                .or(args.get("app_id"))
                .and_then(|v| v.as_str())
                .unwrap_or("app")
        ),
        "run_coding_agent" | "run_claude" => {
            match args.get("coding_agent").and_then(|v| v.as_str()) {
                Some("codex") => "Executing Codex...".to_string(),
                _ => "Executing Claude Code...".to_string(),
            }
        }
        "configure_email" => format!(
            "Configuring email account '{}'...",
            args["name"].as_str().unwrap_or("email")
        ),
        "send_email" => format!(
            "Sending email to {}...",
            args["to"]
                .as_array()
                .map(|a| a
                    .iter()
                    .filter_map(|v| v.as_str())
                    .collect::<Vec<_>>()
                    .join(", "))
                .unwrap_or_default()
        ),
        "read_emails" => format!(
            "Reading emails from {}...",
            args.get("folder")
                .and_then(|v| v.as_str())
                .unwrap_or("INBOX")
        ),
        "read_email" => format!("Reading email #{}...", args["uid"].as_u64().unwrap_or(0)),
        "emit_event" => format!(
            "Emitting {} event...",
            args["event_type"].as_str().unwrap_or("event")
        ),
        "setup_mcp_server" => format!(
            "Setting up MCP server '{}'...",
            args["name"].as_str().unwrap_or("server")
        ),
        "list_mcp_servers" => "Listing MCP servers...".to_string(),
        "start_mcp_server" => format!(
            "Starting MCP server '{}'...",
            args["id"].as_str().unwrap_or("server")
        ),
        "stop_mcp_server" => format!(
            "Stopping MCP server '{}'...",
            args["id"].as_str().unwrap_or("server")
        ),
        "remove_mcp_server" => format!(
            "Removing MCP server '{}'...",
            args["id"].as_str().unwrap_or("server")
        ),
        "navigate_ui" => {
            let target = args["target"].as_str().unwrap_or("panel");
            match target {
                "app" | "app-ui" => format!(
                    "Opening {}...",
                    args.get("app_name")
                        .or(args.get("app_id"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("app")
                ),
                "file" => format!("Opening {}...", args["path"].as_str().unwrap_or("file")),
                "url" => format!("Opening {}...", args["url"].as_str().unwrap_or("URL")),
                _ => format!("Opening {}...", target),
            }
        }
        "update_trigger" => format!(
            "Updating trigger {}...",
            args["name"]
                .as_str()
                .or(args["trigger_id"].as_str())
                .unwrap_or("trigger")
        ),
        "send_notification" => format!(
            "Sending notification '{}'...",
            args["title"].as_str().unwrap_or("notification")
        ),
        "ask_user_question" => match args["questions"].as_array() {
            Some(questions) if questions.len() > 1 => {
                format!("Asking {} questions...", questions.len())
            }
            Some(questions) if questions.len() == 1 => {
                // Show the question itself (truncated) — far friendlier than the
                // raw tool name. Fall back to the short `header` chip, then a
                // generic label. No trailing "..." here: a short question ends
                // in "?" naturally, and `truncate` appends "..." when it cuts.
                let q = &questions[0];
                let text = q
                    .get("question")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .or_else(|| q.get("header").and_then(|v| v.as_str()))
                    .unwrap_or("a question");
                format!("Asking: {}", truncate(text, 60))
            }
            _ => "Asking a question...".to_string(),
        },
        "todo_write" => match args["todos"].as_array() {
            Some(todos) if todos.is_empty() => "Clearing todo list...".to_string(),
            _ => "Updating todo list...".to_string(),
        },
        "save_thread_image" => format!(
            "Saving image to {}...",
            args["path"].as_str().unwrap_or("artifacts")
        ),
        "generate_image" => format!(
            "Generating image: {}...",
            middle_truncate(args["prompt"].as_str().unwrap_or("image"), 50)
        ),
        "git_clone" => format!(
            "Cloning {}...",
            args["url"].as_str().unwrap_or("repository")
        ),
        "save_email_attachment" => format!(
            "Saving email attachment #{}...",
            args["attachment_index"].as_u64().unwrap_or(0)
        ),
        "run_thread" => format!(
            "Running thread: {}...",
            middle_truncate(args["prompt"].as_str().unwrap_or("task"), 50)
        ),
        "list_threads" => "Listing threads...".to_string(),
        "count_threads" => "Counting threads...".to_string(),
        "list_changes" => "Listing changes...".to_string(),
        "apply_change" => "Applying change...".to_string(),
        "correct_memory" => "Updating memory...".to_string(),
        "dismiss_from_context" => "Dismissing from context...".to_string(),
        "query_events" => format!(
            "Querying {} events...",
            args["event_type"].as_str().unwrap_or("all")
        ),
        "count_events" => match args["event_type"].as_str() {
            Some(event_type) => format!("Counting {} events...", event_type),
            None => "Counting events...".to_string(),
        },
        "notifications" | "read_notifications" => match args["action"].as_str() {
            Some("mark_read") => "Marking notification read...".to_string(),
            Some("mark_all_read") => "Marking all notifications read...".to_string(),
            // `list` and the legacy `read_notifications` alias (no action).
            _ => "Reading notifications...".to_string(),
        },
        // Grouped manifest tools — the flat per-verb arms above stay for the
        // back-compat aliases; these arms label the consolidated tool by action.
        "triggers" => match args["action"].as_str() {
            Some("create") => format!(
                "Creating trigger '{}'...",
                args["name"].as_str().unwrap_or("trigger")
            ),
            Some("update") => "Updating trigger...".to_string(),
            Some("delete") => "Deleting trigger...".to_string(),
            Some("pause") => "Pausing trigger...".to_string(),
            Some("resume") => "Resuming trigger...".to_string(),
            _ => "Listing triggers...".to_string(),
        },
        "trigger_groups" => match args["action"].as_str() {
            Some("create") => format!(
                "Creating trigger group '{}'...",
                args["name"].as_str().unwrap_or("group")
            ),
            Some("rename") => format!(
                "Renaming trigger group to '{}'...",
                args["name"].as_str().unwrap_or("group")
            ),
            Some("reorder") => "Reordering trigger groups...".to_string(),
            Some("delete") => "Deleting trigger group...".to_string(),
            _ => "Listing trigger groups...".to_string(),
        },
        "preferences" => match args["action"].as_str() {
            Some("set") => format!(
                "Updating {} setting...",
                args["key"].as_str().unwrap_or("preference")
            ),
            _ => "Reading preferences...".to_string(),
        },
        // The flat `set_environment_variable` alias is labelled by its own arm
        // above (no `action`); these arms label the grouped `env_vars` tool.
        "env_vars" => match args["action"].as_str() {
            Some("set") => format!(
                "Setting environment variable {}...",
                args["name"].as_str().unwrap_or("variable")
            ),
            Some("delete") => format!(
                "Deleting environment variable {}...",
                args["name"].as_str().unwrap_or("variable")
            ),
            _ => "Listing environment variables...".to_string(),
        },
        "manage_repositories" => match args["action"].as_str() {
            Some("add") => format!(
                "Adding repository '{}'...",
                args["name"].as_str().unwrap_or("repo")
            ),
            Some("remove") => format!(
                "Removing repository '{}'...",
                args["name"].as_str().unwrap_or("repo")
            ),
            Some("list") => "Listing repositories...".to_string(),
            _ => "Managing repositories...".to_string(),
        },
        "manage_models" => match args["action"].as_str() {
            Some("add") => format!(
                "Adding model '{}'...",
                args["id"].as_str().unwrap_or("model")
            ),
            Some("remove") => format!(
                "Removing model '{}'...",
                args["id"].as_str().unwrap_or("model")
            ),
            Some("enable") => format!(
                "Enabling model '{}'...",
                args["id"].as_str().unwrap_or("model")
            ),
            Some("disable") => format!(
                "Disabling model '{}'...",
                args["id"].as_str().unwrap_or("model")
            ),
            Some("list") => "Listing models...".to_string(),
            _ => "Managing models...".to_string(),
        },
        "events" => match args["action"].as_str() {
            Some("emit") => format!(
                "Emitting {} event...",
                args["event_type"].as_str().unwrap_or("event")
            ),
            Some("count") => match args["event_type"].as_str() {
                Some(event_type) => format!("Counting {} events...", event_type),
                None => "Counting events...".to_string(),
            },
            // `query` (and any unrecognised action) → query label.
            _ => format!(
                "Querying {} events...",
                args["event_type"].as_str().unwrap_or("all")
            ),
        },
        "changes" => match args["action"].as_str() {
            Some("apply") => "Applying change...".to_string(),
            _ => "Listing changes...".to_string(),
        },
        "thread_queue" => match args["action"].as_str() {
            Some("update_policy") => "Updating Thread Queue policy...".to_string(),
            Some("run_now") => "Running queued entry now...".to_string(),
            Some("drop") => "Dropping queued entry...".to_string(),
            _ => "Listing Thread Queue...".to_string(),
        },
        // Grouped `memory` tool exposes only the correction actions to the LLM
        // (correct / correct_by_id); both render the same label.
        "memory" => "Updating memory...".to_string(),
        "threads" => match args["action"].as_str() {
            Some("count") => "Counting threads...".to_string(),
            _ => "Listing threads...".to_string(),
        },
        "mcp" => match args["action"].as_str() {
            Some("setup") => format!(
                "Setting up MCP server '{}'...",
                args["name"].as_str().unwrap_or("server")
            ),
            Some("start") => format!(
                "Starting MCP server '{}'...",
                args["id"].as_str().unwrap_or("server")
            ),
            Some("stop") => format!(
                "Stopping MCP server '{}'...",
                args["id"].as_str().unwrap_or("server")
            ),
            Some("remove") => format!(
                "Removing MCP server '{}'...",
                args["id"].as_str().unwrap_or("server")
            ),
            _ => "Listing MCP servers...".to_string(),
        },
        "plugins" => match args["action"].as_str() {
            Some("install") => format!(
                "Installing plugin from {}...",
                args["source"].as_str().unwrap_or("source")
            ),
            Some("register_marketplace") => format!(
                "Registering marketplace {}...",
                args["source"].as_str().unwrap_or("source")
            ),
            Some("update") => format!(
                "Updating plugin '{}'...",
                args["id"].as_str().unwrap_or("plugin")
            ),
            Some("uninstall") => format!(
                "Uninstalling plugin '{}'...",
                args["id"].as_str().unwrap_or("plugin")
            ),
            _ => "Checking plugins for updates...".to_string(),
        },
        "browser_forget_login" => format!(
            "Forgetting login for {}...",
            args["domain"].as_str().unwrap_or("site")
        ),
        "browser_clear_data" => "Clearing browser data...".to_string(),
        "install_plugin" => format!(
            "Installing plugin from {}...",
            args["source"].as_str().unwrap_or("source")
        ),
        "check_plugin_updates" => match args.get("id").and_then(|v| v.as_str()) {
            Some(id) => format!("Checking plugin '{}' for updates...", id),
            None => "Checking installed plugins for updates...".to_string(),
        },
        "update_plugin" => format!(
            "Updating plugin '{}'...",
            args["id"].as_str().unwrap_or("plugin")
        ),
        "uninstall_plugin" => format!(
            "Uninstalling plugin '{}'...",
            args["id"].as_str().unwrap_or("plugin")
        ),
        "enable_push_notifications" => "Enabling push notifications...".to_string(),
        _ if name.starts_with("mcp__") => {
            let rest = &name[5..];
            if let Some(sep) = rest.find("__") {
                let tool_name = &rest[sep + 2..];
                format!("MCP: {}...", tool_name)
            } else {
                format!("MCP: {}...", name)
            }
        }
        _ => format!("Executing {}...", name),
    }
}

/// Human-friendly description of a Claude Code tool call.
pub fn describe_cc_tool(name: &str, args: &serde_json::Value) -> String {
    fn basename(p: &str) -> &str {
        p.rsplit('/').next().unwrap_or(p)
    }
    let str_arg = |key: &str| args.get(key).and_then(|v| v.as_str()).unwrap_or("");

    match name {
        "Read" => {
            let p = str_arg("file_path");
            if p.is_empty() {
                "Read file".into()
            } else {
                format!("Read {}", basename(p))
            }
        }
        "Edit" | "MultiEdit" => {
            let p = str_arg("file_path");
            if p.is_empty() {
                "Edit file".into()
            } else {
                format!("Edit {}", basename(p))
            }
        }
        "Write" => {
            let p = str_arg("file_path");
            if p.is_empty() {
                "Write file".into()
            } else {
                format!("Write {}", basename(p))
            }
        }
        "Glob" => {
            let pat = str_arg("pattern");
            if pat.is_empty() {
                "Find files".into()
            } else {
                format!("Find {}", middle_truncate(pat, 60))
            }
        }
        "Grep" => {
            let pat = str_arg("pattern");
            if pat.is_empty() {
                "Search code".into()
            } else {
                format!("Search '{}'", middle_truncate(pat, 60))
            }
        }
        "Bash" => {
            let cmd = str_arg("command");
            if cmd.is_empty() {
                "Run command".into()
            } else {
                format!("Run {}", middle_truncate(first_command_line(cmd), 60))
            }
        }
        "WebFetch" => {
            let url = str_arg("url");
            if url.is_empty() {
                "Fetch URL".into()
            } else {
                let origin: String = url.splitn(4, '/').take(3).collect::<Vec<_>>().join("/");
                format!("Fetch {}", origin)
            }
        }
        "WebSearch" => {
            let q = str_arg("query");
            if q.is_empty() {
                "Web search".into()
            } else {
                format!("Search '{}'", middle_truncate(q, 60))
            }
        }
        "Agent" => {
            let desc = str_arg("description");
            if desc.is_empty() {
                "Run agent".into()
            } else {
                desc.to_string()
            }
        }
        "Skill" => {
            let s = str_arg("skill");
            if s.is_empty() {
                "Run skill".into()
            } else {
                format!("Run skill: {}", s)
            }
        }
        "NotebookEdit" => {
            let p = str_arg("file_path");
            if p.is_empty() {
                "Edit notebook".into()
            } else {
                format!("Edit {}", basename(p))
            }
        }
        // Codex item types (see runtime/codex_parse.rs) — Codex reports
        // coarse-grained items, not named tools like CC.
        "command_execution" => {
            let cmd = str_arg("command");
            if cmd.is_empty() {
                "Run command".into()
            } else {
                format!("Run {}", middle_truncate(first_command_line(cmd), 60))
            }
        }
        "file_change" => {
            let n = args
                .get("changes")
                .and_then(|c| c.as_array())
                .map(|a| a.len())
                .unwrap_or(0);
            match n {
                0 => "Apply file changes".into(),
                1 => "Apply 1 file change".into(),
                n => format!("Apply {} file changes", n),
            }
        }
        "web_search" => {
            let q = str_arg("query");
            if q.is_empty() {
                "Web search".into()
            } else {
                format!("Search '{}'", middle_truncate(q, 60))
            }
        }
        "todo_list" => "Update plan".into(),
        _ => name.to_string(),
    }
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
