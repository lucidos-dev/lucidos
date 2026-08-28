pub mod announced_surfaces;
pub mod apps;
pub mod artifacts;
pub mod aux_context_backfill;
pub mod backup;
pub mod blobs;
pub mod changes;
pub mod changes_projection;
pub mod credentials;
pub mod device_presence;
pub mod devices;
pub mod email;
pub mod environment_variables;
pub mod event_subscription;
pub mod events;
pub mod git_auth;
pub mod grants;
pub mod handshake_approvals;
pub mod home_path;
pub mod image_described_backfill;
pub mod image_migration;
pub mod intents;
pub mod knowhow;
pub mod mcp_servers;
pub mod models;
pub mod oauth;
pub mod oauth_registry;
pub mod pinned_apps;
pub mod plugin_marketplaces;
pub mod plugins;
pub mod preference_catalog;
pub mod preferences;
pub mod repositories;
pub mod shell;
pub mod slug;
pub mod store;
pub mod system_knowhow;
pub mod user_dir;
pub mod user_path;
pub mod webhook_deliveries;
pub mod webhook_ingress;
pub mod webhook_probe_token;
pub mod webhooks;

use std::borrow::Cow;

/// Get the database URL from the environment, with a default for local dev.
pub fn database_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://lucidos:lucidos@localhost:5432/lucidos".to_string())
}

/// Process-lifetime cache of `pg_env_vars(&database_url())`. DATABASE_URL does
/// not change once the engine starts, so the URL is parsed once and callers get
/// a slice they can iterate or clone-extend.
pub fn pg_env_vars_cached() -> &'static [(String, String)] {
    static CACHED: std::sync::LazyLock<Vec<(String, String)>> =
        std::sync::LazyLock::new(|| pg_env_vars(&database_url()));
    CACHED.as_slice()
}

/// Parse a `postgres(ql)://user:password@host[:port]/dbname[?...]` URL into the
/// libpq env-var bundle (`PGUSER`/`PGPASSWORD`/`PGHOST`/`PGPORT`/`PGDATABASE`).
///
/// These are injected into every subprocess the engine spawns, so a caller can
/// run `psql -c '…'` bare and never put the password in argv. argv is
/// persisted into tool-call event payloads and rendered in the steps UI; env
/// vars are not.
///
/// An unrecognized URL yields an empty Vec. Skipping the injection beats
/// emitting a half-broken bundle that confuses libpq.
pub(crate) fn pg_env_vars(database_url: &str) -> Vec<(String, String)> {
    // The `:password` segment is OPTIONAL. The packaged Postgres backend uses
    // trust auth on loopback and hands the engine a passwordless URL. A
    // required group would reject it, breaking a picker restore. An `@` is
    // still required, so `postgres://no-at-sign/db` stays empty.
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
    // Only when the URL carried a password segment, so libpq does not try to
    // send one for a trust-auth URL. An explicit empty password (`user:@host`)
    // still emits an empty PGPASSWORD.
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

/// Convert a credential `service_name` or an OAuth provider name into the
/// `{NAME}` segment of an injected env var (`CRED_{NAME}`, `OAUTH_{NAME}_…`):
/// uppercased, with every character outside `[A-Z0-9_]` replaced by `_`.
///
/// Sanitizes by character CLASS, never a fixed list. A name like
/// `CRED_OAUTH:GOOGLE` is a legal `environ` entry and an illegal shell
/// identifier. Bash reads it as `$CRED_OAUTH` then a literal `:GOOGLE`, while
/// Python still reaches it through `os.environ`. That silent asymmetry between
/// two script runtimes is worse than either outcome alone.
///
/// No leading-digit guard: every caller prefixes `CRED_` or `OAUTH_`, so the
/// full variable name always starts with a letter.
///
/// Non-ASCII collapses to `_` per `char`, so the result is always ASCII and
/// the function never slices a multi-byte boundary.
pub fn env_var_segment(name: &str) -> String {
    name.chars()
        .map(|c| {
            let upper = c.to_ascii_uppercase();
            if upper.is_ascii_alphanumeric() || upper == '_' {
                upper
            } else {
                '_'
            }
        })
        .collect()
}

/// Mask the password segment of any `postgres(ql)://user:password@host…` URL
/// in `s`. A best-effort safety net for wherever the env-var injection did not
/// take: a hardcoded URI in a script, a URL pasted into a curl call. Applied at
/// the spawn-side log line and where a tool call's `args` are persisted.
pub fn redact_postgres_secrets(s: &str) -> String {
    static RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"(postgres(?:ql)?://[^:@/?\s]+):[^@/?\s]+@")
            .expect("postgres redaction regex must compile")
    });
    RE.replace_all(s, "$1:***@").into_owned()
}

/// Recursively apply `redact_postgres_secrets` to every string inside a
/// `serde_json::Value`. Scrubs a tool call's `args` before they reach the event
/// store or the SSE stream, since a Bash `command` can carry a hardcoded URL.
///
/// Hot path: every tool-call event walks every string. The `contains` guard
/// short-circuits the regex and the allocation for any string that never
/// mentions "postgres".
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
/// `[REDACTED]`. Keeps credential material out of logs and error messages that
/// untrusted code could otherwise echo. Longest secrets are scrubbed first, so
/// a secret that is a substring of another leaves no partial match behind.
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

/// Ephemeral scratch, workspace-root-relative and OUTSIDE `data/`: gitignored,
/// not indexed, safe to delete. Several tools land files here and print this
/// prefix back to the LLM. What the file tools resolve and what those tools
/// advertise must be one string.
///
/// A `/`-joined relative path rather than a `PathBuf`, because it is matched
/// against LLM-supplied path strings as often as it is joined onto a root.
pub const TMP_DIR: &str = ".lucidos/tmp";

/// Whether a normalized data path names a file inside [`TMP_DIR`]. Sibling of
/// [`is_system_knowhow_path`]: the two together are the whole set of prefixes
/// the file tools resolve outside `data/`, and both gate read-only enforcement.
///
/// Anchored on the path separator, so it answers `false` for the bare directory
/// (`.lucidos/tmp`) and for a sibling that merely shares the prefix
/// (`.lucidos/tmpfoo`). That matters because this predicate guards a security
/// boundary: a plain `starts_with(TMP_DIR)` would be correct only as long as
/// every caller normalized first, which is an invariant a future caller can
/// silently break.
pub fn is_tmp_path(data_path: &str) -> bool {
    data_path
        .strip_prefix(TMP_DIR)
        .is_some_and(|rest| rest.starts_with('/'))
}

pub use apps::{App, AppManager, AppManifest};
pub use artifacts::{
    is_vendored_path, list_searchable_data_files, ArtifactChange, ArtifactManager,
    WriteAnnouncement,
};
pub use credentials::{AuthType, Credential, CredentialInfo, CredentialStore};
pub use devices::DeviceStore;
pub use email::{EmailAccount, EmailAccountInfo, EmailStore};
pub use environment_variables::{EnvironmentVariable, EnvironmentVariableStore};
pub use grants::{grants_dir, GrantFile};
pub use intents::{Intent, IntentStore};
pub use knowhow::{Knowhow, KnowhowDirs, KnowhowListDepth, KnowhowStore, KnowhowSummary};
pub use oauth::{OAuthAccount, OAuthAccountInfo, OAuthStore};
pub use oauth_registry::OAuthProviderRow;
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
    DEFAULT_MAX_TOOL_CALLS, DEFAULT_VERTEX_REGION, MIN_MAX_TOOL_CALLS, PREF_CHAT_MODEL,
    PREF_CHAT_REASONING_EFFORT, PREF_CODING_AGENT_CLAUDE_PATH,
    PREF_CODING_AGENT_CLAUDE_PERMISSION_MODE, PREF_CODING_AGENT_CODEX_PATH, PREF_IMAGE_MODEL,
    PREF_LOCAL_BASE_URL, PREF_MAX_TOOL_CALLS, PREF_MODEL_COMMAND_JUDGE,
    PREF_MODEL_CONVERSATION_SUMMARY, PREF_MODEL_IMAGE_DESCRIPTION, PREF_MODEL_MEMORY,
    PREF_MODEL_TITLE, PREF_OPENCODE_FREE_ENABLED, PREF_PROVIDER_ENABLED_ANTHROPIC,
    PREF_PROVIDER_ENABLED_LOCAL, PREF_PROVIDER_ENABLED_OPENAI, PREF_PROVIDER_ENABLED_OPENROUTER,
    PREF_PROVIDER_ENABLED_VERTEX, PREF_PROVIDER_ENABLED_XAI, PREF_REASONING_COMMAND_JUDGE,
    PREF_REASONING_CONVERSATION_SUMMARY, PREF_REASONING_IMAGE_DESCRIPTION, PREF_REASONING_MEMORY,
    PREF_REASONING_TITLE, PREF_SELF_CURATED_CONTEXT_EXPIRE_AFTER_ROUNDS,
    PREF_SELF_CURATED_CONTEXT_MODE, PREF_SELF_CURATED_CONTEXT_SWEEP_EVERY_ROUNDS,
    PREF_VERTEX_REGION,
};
pub use store::{
    ConversationMessage, ConversationSnapshot, EventStore, ResponseEvent, SessionMessage, Step,
    ThreadEventRow, ThreadSummary,
};
pub use webhook_deliveries::{Claim, DeliveryLedger};
pub use webhooks::{Webhook, WebhookStore};

/// Reset the index to match HEAD's tree before staging. Drops entries a
/// previous `commit_*` staged without committing. No-op on a fresh repo with
/// no HEAD yet.
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

/// Did this git2 error mean "another writer of the same repo got there first",
/// rather than "the operation itself is wrong"? The single place both contended
/// shapes are classified and documented, and what
/// [`retry_while_repo_contended`] retries on.
///
/// **`Locked`, lost a git lock file.** Every git lock is CROSS-PROCESS and
/// non-blocking, so a writer inside another writer's window fails outright
/// rather than waiting. `Locked` is libgit2's single answer for "a lock file
/// blocked this", whatever the lock was.
///
/// Match on the CODE alone: the class only says where libgit2 noticed. It writes
/// `Index` for `.git/index.lock`, and `Os` for every other lock. A held
/// `refs/heads/main.lock` takes the second route, and both are locks an ordinary
/// write meets:
///
/// ```text
/// {"error":"failed to lock file '<ws>/.git/refs/heads/main.lock' for writing: ; class=Os (2); code=Locked (-14)"}
/// ```
///
/// **`Reference` / `Modified`, lost the HEAD compare-and-swap.** `commit_index`
/// reads the parent with `repo.head()`, then asks libgit2 to move HEAD off
/// exactly that parent. A commit landing on HEAD in between fails the swap. The
/// competitor can be another git2 handle in THIS process or an out-of-process
/// `git` CLI writer. It shows up as an intermittent 500 on an ordinary write:
///
/// ```text
/// {"error":"old reference value does not match; class=Reference (4); code=Modified (-15)"}
/// ```
pub fn is_transient_repo_contention(e: &git2::Error) -> bool {
    e.code() == git2::ErrorCode::Locked
        || matches!(
            (e.class(), e.code()),
            (git2::ErrorClass::Reference, git2::ErrorCode::Modified)
        )
}

/// Run a git2 repository write, retrying briefly while another writer of the
/// same repo keeps winning the race. See [`is_transient_repo_contention`] for
/// the two shapes that counts as, and for why each one happens.
///
/// Both windows are milliseconds, so waiting them out is the only sane
/// response. Two things bind on callers:
///
/// - **`op` re-runs from the start every attempt, so it must be idempotent.**
///   Stage INSIDE it, never before: every `commit_*` routed through this opens
///   with `reset_index_to_head`, which makes a repeat safe. That reset also
///   makes the retry CORRECT for the HEAD race. It re-reads the tree from the
///   NEW head, so the attempt lands on top of the commit that beat it. A caller
///   that deletes first must remove OUTSIDE the closure, stage tolerantly, and
///   finish through [`commit_index_unless_unchanged`].
/// - **It sleeps the calling thread.** Prefer a blocking context. A caller
///   whose `Repository` guard is not `Send` runs inline, and only sleeps when
///   contended.
///
/// The budget is sized for the LOCK case. Any other error returns at once, and
/// the final contended error is returned once the budget is spent.
pub fn retry_while_repo_contended<T>(
    mut op: impl FnMut() -> Result<T, git2::Error>,
) -> Result<T, git2::Error> {
    const ATTEMPTS: u32 = 25;
    const BACKOFF: std::time::Duration = std::time::Duration::from_millis(40);
    let mut attempt = 1;
    loop {
        match op() {
            Err(e) if attempt < ATTEMPTS && is_transient_repo_contention(&e) => {
                std::thread::sleep(BACKOFF);
                attempt += 1;
            }
            settled => return settled,
        }
    }
}

/// Brand-new workspace: write `lucidos.toml` pinning the allocated vite port
/// and commit it. `git status` is then clean from the first boot, and the port
/// survives any later port-registry drift.
///
/// An existing `lucidos.toml` is left strictly untouched and reported as
/// `false`, since the user may have hand-edited its pin. The caller gates this
/// on having just git-init'd the workspace, so an existing one is never
/// surprised by a freshly-pinned port.
///
/// Crash-safety: a failure in the commit phase removes the file again, so the
/// working tree stays clean. The engine gate is one-shot and will not retry the
/// pin, but the workspace is at least not left dirty with an untracked file.
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

/// Deliberately NOT routed through `retry_while_repo_contended`: this runs once
/// during engine startup on a workspace we just git-init'd, before any manager,
/// timer or agent session exists to compete for HEAD. Its staging also happens
/// outside any closure, so wrapping it would be unsafe without restructuring it
/// for a re-run it cannot need.
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
    commit_data_paths_with_overrides(workspace_path, data_relative_paths, &[], message)
}

/// [`commit_data_paths_added`], but recording chosen paths from a buffer
/// instead of from the working tree.
///
/// A plugin update that merges the user's local edits needs the two to differ.
/// The working tree gets the merge, and the commit records upstream's bytes, so
/// the recorded commit stays a byte-exact copy of what the plugin shipped. That
/// is the baseline the NEXT update diffs against. Commit the merge into it and
/// the following update reads the patch as upstream's own content, sees no
/// local modification, and silently discards it.
///
/// Each override carries its git file mode, because `add_frombuffer` records
/// exactly what it is handed. Only the buffer path needs telling, since
/// `add_path` reads the mode off disk. An executable file must not be recorded
/// as `100644` just because its bytes arrived from memory.
///
/// An override for a path not listed in `data_relative_paths` is ignored: the
/// list decides what the commit covers.
pub fn commit_data_paths_with_overrides(
    workspace_path: &std::path::Path,
    data_relative_paths: &[String],
    overrides: &[(String, Vec<u8>, u32)],
    message: &str,
) -> Result<String, git2::Error> {
    // Validate before the retry loop: a rejected path is the caller's answer,
    // not contention, so re-checking it on every attempt buys nothing.
    for p in data_relative_paths {
        if is_path_traversal(p) {
            return Err(git2::Error::from_str(&format!(
                "Path traversal not allowed: {}",
                p
            )));
        }
    }
    let repo = git2::Repository::open(workspace_path)?;
    retry_while_repo_contended(|| {
        let mut index = repo.index()?;
        reset_index_to_head(&repo, &mut index)?;
        for p in data_relative_paths {
            let repo_relative = format!("data/{}", p);
            match overrides.iter().find(|(path, _, _)| path == p) {
                Some((_, bytes, mode)) => index.add_frombuffer(
                    &blob_index_entry(&repo_relative, git2::Oid::zero(), *mode),
                    bytes,
                )?,
                None => index.add_path(std::path::Path::new(&repo_relative))?,
            }
        }
        index.write()?;
        commit_index(&repo, message)
    })
}

/// A regular-file index entry for `repo_relative`, naming blob `id`.
///
/// Two callers with two needs. `Index::add_frombuffer` hashes the buffer and
/// fills the id in itself, so it takes `Oid::zero()`. `merge_file_from_index`
/// reads the id to find the blob, so it takes a real one. Neither consults the
/// stat fields, which is why they stay zeroed. `mode` is `0o100644` or
/// `0o100755`, the only two git records for a regular file.
pub fn blob_index_entry(repo_relative: &str, id: git2::Oid, mode: u32) -> git2::IndexEntry {
    git2::IndexEntry {
        ctime: git2::IndexTime::new(0, 0),
        mtime: git2::IndexTime::new(0, 0),
        dev: 0,
        ino: 0,
        mode,
        uid: 0,
        gid: 0,
        file_size: 0,
        id,
        flags: 0,
        flags_extended: 0,
        path: repo_relative.as_bytes().to_vec(),
    }
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
    retry_while_repo_contended(|| {
        let mut index = repo.index()?;
        reset_index_to_head(&repo, &mut index)?;
        let mut staged_any = false;
        for p in data_relative_paths {
            if is_path_traversal(p) {
                continue;
            }
            // `remove_path` errors when the entry isn't in the index (untracked
            // or never-committed file). Tolerate that and keep going.
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
    })
}

/// Create a commit from the current index state.
///
/// The parent is re-read from HEAD on every call, and the HEAD update libgit2
/// performs is compare-and-swap against exactly that parent. So this races any
/// other writer of the repo, and a loser fails with `Reference` / `Modified`.
/// Call it inside [`retry_while_repo_contended`], which turns that loss into a
/// re-run onto the winner's head; see [`is_transient_repo_contention`].
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

/// [`commit_index`], except that an index already identical to HEAD's tree
/// reports HEAD's own sha instead of recording an empty commit.
///
/// This is what makes the DELETE helpers safe to retry, and they are the only
/// callers. Each removes from the working tree BEFORE the retry closure and
/// stages the removal inside it. A competing writer can therefore commit the
/// same deletion in between. The retried attempt then resets onto that head,
/// finds the path already untracked, and has nothing left to stage. Reporting an
/// error there would deny a deletion that demonstrably happened, so this
/// reports the head recording it instead. A path that was never tracked takes
/// the same route.
///
/// Callers therefore stage the removal TOLERANTLY and let this decide, rather
/// than propagating the staging error. `git2`'s `remove_path` errors on an
/// entry that is not in the index.
pub fn commit_index_unless_unchanged(
    repo: &git2::Repository,
    message: &str,
) -> Result<String, git2::Error> {
    let mut index = repo.index()?;
    let tree_id = index.write_tree()?;
    // `if let Ok` rather than `?`, matching `commit_all_dirty`: an unborn HEAD
    // has no tree to compare against and must fall through to a root commit.
    if let Ok(head) = repo.head() {
        if let Ok(head_commit) = head.peel_to_commit() {
            if head_commit.tree()?.id() == tree_id {
                return Ok(head_commit.id().to_string());
            }
        }
    }
    commit_index(repo, message)
}

/// Reject paths that would escape the directory they get joined onto: any `..`
/// component, or an absolute path.
///
/// The single canonical traversal guard for the whole engine. HTTP handlers,
/// LLM file tools, the proxy script runner and script triggers all funnel
/// through it, so the rule cannot drift between call sites.
///
/// Deliberately conservative: it matches `..` anywhere in the string (so even a
/// filename like `a..b` is rejected) and does not normalize or percent-decode
/// first. Both choices fail closed — a false positive costs a rejected request,
/// a false negative costs a path escape.
pub fn is_path_traversal(path: &str) -> bool {
    path.contains("..") || path.starts_with('/') || path.starts_with('\\')
}

/// Whether a file extension indicates a binary file.
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

/// Pull the `---\n...\n---\n` YAML header off a markdown file, returning the
/// trimmed frontmatter block and the body.
///
/// The two field-specific parsers share this prefix, but each parses its own
/// fields. Their downstream semantics diverge on the field type and on whether
/// a missing value derives from the body.
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

/// Parse `name:` and a configurable list field out of markdown frontmatter.
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
mod is_tmp_path_tests {
    use super::is_tmp_path;

    #[test]
    fn matches_files_inside_the_scratch_tree() {
        assert!(is_tmp_path(".lucidos/tmp/notes.json"));
        assert!(is_tmp_path(".lucidos/tmp/some-repo/README.md"));
        assert!(is_tmp_path(".lucidos/tmp/"));
    }

    #[test]
    fn is_anchored_on_the_separator_not_the_string() {
        // The reason this is a named predicate rather than a bare
        // starts_with: it guards a security boundary, so a sibling that
        // merely shares the prefix must not slip through.
        assert!(!is_tmp_path(".lucidos/tmpfoo"));
        assert!(!is_tmp_path(".lucidos/tmp-old/x"));
        assert!(!is_tmp_path(".lucidos/tmp"));
    }

    #[test]
    fn rejects_paths_outside_the_scratch_tree() {
        assert!(!is_tmp_path("artifacts/notes.md"));
        assert!(!is_tmp_path(".lucidos/worktrees/thread-x/src/main.rs"));
        assert!(!is_tmp_path(".lucidos/exhaust/run.log"));
        // Not anchored at the start either: an artifact cannot spoof it.
        assert!(!is_tmp_path("artifacts/.lucidos/tmp/x.md"));
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

/// An *event wait* `reason` with a leading waiting phrase removed, for a label
/// that already said "wait".
///
/// The engine-side twin of `awaitedSubject`
/// (`store/thread-events/thread-event-types.ts`), which does this for the
/// transcript's two labels. Two implementations because two layers compose the
/// text: the pending step's here, the row's there. The three judgments behind
/// the rule live in
/// `docs/plans/2026-08-14-a-wait-label-does-not-say-waiting-twice.md`.
///
/// The TS twin also eats leading whitespace, which this does not need: the one
/// caller trims the reason before asking.
fn awaited_subject(reason: &str) -> &str {
    // Matched as verb, gap, preposition rather than as six literal phrases.
    // The TS twin's `\s+` accepts ANY run of whitespace, and the two must not
    // disagree: a single-space literal list left `waiting  for the lock`
    // stripped in the transcript and doubled here.
    for verb in ["waiting", "wait"] {
        let Some(rest) = strip_word(reason, verb) else {
            continue;
        };
        for preposition in ["for", "on", "until"] {
            let Some(subject) = strip_word(rest, preposition) else {
                continue;
            };
            // A strip that emptied the reason would leave a dangling colon.
            return if subject.is_empty() { reason } else { subject };
        }
    }
    reason
}

/// `s` past a leading `word` and the whitespace after it, or `None` when `s`
/// does not open on that whole word.
///
/// The trailing whitespace is what makes it a WORD match: without it
/// `waiting form the lock` would strip on `for`. A word ending the string is
/// still a match, so the caller can tell an empty subject from no match.
///
/// `split_at_checked` is the UTF-8 guard. It refuses an index that is not a
/// char boundary, so a reason opening with a multi-byte character falls
/// through instead of panicking.
fn strip_word<'a>(s: &'a str, word: &str) -> Option<&'a str> {
    let (head, tail) = s.split_at_checked(word.len())?;
    if !head.eq_ignore_ascii_case(word) {
        return None;
    }
    let rest = tail.trim_start();
    if !tail.is_empty() && rest.len() == tail.len() {
        return None;
    }
    Some(rest)
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

/// First non-empty line of a command, trimmed. A step label is one line of
/// HTML, and a newline collapses to a space there. A multi-line script must
/// therefore condense to its opening line, rather than render as a garbled
/// run-on. Shared by the engine and coding-agent description paths.
fn first_command_line(cmd: &str) -> &str {
    cmd.lines()
        .find(|l| !l.trim().is_empty())
        .map(|l| l.trim())
        .unwrap_or(cmd)
}

/// Shell basenames that mark a `<shell> -c <script>` wrapper, for the step-row
/// label ([`shell_script_body`]).
///
/// A SUPERSET of `command_guard::GUARD_SHELLS`, which unwraps the same shapes to
/// decide what the permission guard classifies. The containment is asserted, and
/// it holds in one direction on purpose:
///
/// * A shell the GUARD knows must be one the label knows, or a payload would be
///   scanned and then displayed still wrapped.
/// * The reverse is allowed, and `fish` is the whole difference.
///   Over-recognizing costs a label nothing. The guard, though, discards
///   operands after the script as positional parameters, and fish does not
///   follow that POSIX rule: it evaluates EVERY `-c` in turn, so
///   `fish -c 'ls' -c 'rm -rf /'` would hand the classifier `ls` alone.
///   Teaching the guard fish means handling its whole script-carrying flag
///   surface, which is a security change on its own terms.
pub(crate) const WRAPPER_SHELLS: [&str; 7] = ["sh", "bash", "zsh", "dash", "ksh", "ash", "fish"];

/// The script inside a login-shell wrapper, or `cmd` unchanged when there is no
/// wrapper to see through.
///
/// Codex reports a shell step as the whole invocation its harness built, where
/// Claude Code's `Bash` reports the script alone. Only Codex's
/// `command_execution` goes through here: a Claude Code session that literally
/// invokes `bash -lc "..."` chose to, and its row must show it.
///
/// Conservative by construction. Anything not recognizably
/// `<shell> -<letters including c> <one quoted-or-bare script>` comes back
/// untouched, and the result is always a suffix of `cmd`.
///
/// # Why this is not `command_guard::unwrap_shell_command`
///
/// Both unwrap the same shapes and share [`WRAPPER_SHELLS`], but their errors
/// have opposite costs. On an UNQUOTED operand (`zsh -lc git status`) the
/// guard returns `git status`, since reading too much only ever scans more.
/// POSIX `sh -c` really runs `git` with `$0=status`, so a step row must
/// decline instead of naming a command that never ran. Do not collapse them.
fn shell_script_body(cmd: &str) -> Cow<'_, str> {
    let Some((shell, rest)) = cmd.trim_start().split_once(char::is_whitespace) else {
        return Cow::Borrowed(cmd);
    };
    if !WRAPPER_SHELLS.contains(&shell.rsplit('/').next().unwrap_or(shell)) {
        return Cow::Borrowed(cmd);
    }
    let Some((flags, script)) = rest.trim_start().split_once(char::is_whitespace) else {
        return Cow::Borrowed(cmd);
    };
    // `-c`, `-lc`, `-ic`: one cluster of letters including the flag that says
    // "the next argument is the script". Anything else (`--norc`, a bare `-`, a
    // flag that takes its own value) means this is not the shape we can read.
    let Some(letters) = flags.strip_prefix('-') else {
        return Cow::Borrowed(cmd);
    };
    if letters.is_empty() || !letters.chars().all(|c| c.is_ascii_alphabetic()) {
        return Cow::Borrowed(cmd);
    }
    if !letters.contains('c') {
        return Cow::Borrowed(cmd);
    }
    let script = script.trim();
    match script.as_bytes().first() {
        // A quoted script has to be exactly ONE quoted word. In
        // `zsh -lc "a" && "b"` no part is "the command", so the label shows
        // the invocation verbatim rather than a confident half of it.
        Some(b'\'' | b'"') => unquote_shell_word(script).unwrap_or(Cow::Borrowed(cmd)),
        // An UNQUOTED suffix is the script only when it is a single word.
        // POSIX `sh -c` reads ONE operand as the script and assigns the rest to
        // the positional parameters. So `zsh -lc git status` runs `git` with
        // `$0=status`, and reading it as `git status` would put a command in
        // the row that never ran.
        Some(_) if !script.contains(char::is_whitespace) => Cow::Borrowed(script),
        _ => Cow::Borrowed(cmd),
    }
}

/// The contents of `s` when it is exactly ONE quoted shell word, unescaped for
/// that quoting style. `None` when `s` is unquoted, unterminated, or carries
/// anything after its closing quote. `"a" && "b"` is two words and a pipeline
/// rather than one script, and stripping its outer quotes would claim
/// otherwise.
fn unquote_shell_word(s: &str) -> Option<Cow<'_, str>> {
    let bytes = s.as_bytes();
    let quote = *bytes.first()?;
    if quote != b'\'' && quote != b'"' {
        return None;
    }
    // Lazily allocated: a script with no escapes borrows straight out of `s`.
    let mut unescaped: Option<String> = None;
    let mut chunk_start = 1;
    let mut i = 1;
    // Byte scanning is UTF-8-safe here: every byte compared against is ASCII
    // and a continuation byte is always >= 0x80, so a cut only ever lands on a
    // char boundary.
    while i < bytes.len() {
        if bytes[i] == quote {
            // A single-quoted script cannot contain a quote, so the wrapper
            // spells one as close, escape, reopen. That is not the terminator.
            if quote == b'\'' && s[i..].starts_with("'\\''") {
                let buf = unescaped.get_or_insert_with(String::new);
                buf.push_str(&s[chunk_start..i]);
                buf.push('\'');
                i += 4;
                chunk_start = i;
                continue;
            }
            if i + 1 != bytes.len() {
                return None;
            }
            return Some(match unescaped {
                Some(mut buf) => {
                    buf.push_str(&s[chunk_start..i]);
                    Cow::Owned(buf)
                }
                None => Cow::Borrowed(&s[1..i]),
            });
        }
        // Inside double quotes a backslash escapes exactly four characters plus
        // a line continuation. Before anything else it is a literal backslash.
        if quote == b'"' && bytes[i] == b'\\' && i + 1 < bytes.len() {
            let next = bytes[i + 1];
            if matches!(next, b'"' | b'\\' | b'$' | b'`' | b'\n') {
                let buf = unescaped.get_or_insert_with(String::new);
                buf.push_str(&s[chunk_start..i]);
                if next != b'\n' {
                    buf.push(next as char);
                }
                i += 2;
                chunk_start = i;
                continue;
            }
        }
        i += 1;
    }
    None // unterminated
}

/// The bare tool name inside an `mcp__<server>__<tool>` identifier, or `None`
/// when `name` is not shaped like one. Every surface uses that naming: the
/// engine's own MCP client, Claude Code natively, and Codex, whose
/// `mcp_tool_call` item is deliberately rebuilt into the same shape
/// (`runtime/codex_parse.rs`). Both description paths split it here so the two
/// can't drift.
fn mcp_tool_suffix(name: &str) -> Option<&str> {
    let rest = name.strip_prefix("mcp__")?;
    rest.find("__").map(|sep| &rest[sep + 2..])
}

/// Summarize a `glob_files` or `grep_files` JSON result as
/// "N items[, truncated]".
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
///
/// The `Executing <name>...` fallback exists only for genuinely unknowable
/// names: a third-party MCP tool with no `mcp__` prefix, or a historical event
/// replaying a retired tool. Everything the engine ships is labelled in
/// [`tool_label`], and `every_known_tool_name_has_a_step_label` fails the build
/// otherwise, so the fallback can never render a registered tool's raw name.
pub fn describe_tool(name: &str, args: &serde_json::Value) -> String {
    tool_label(name, args).unwrap_or_else(|| format!("Executing {}...", name))
}

/// Step label for a search action: the verb plus the query it was given.
///
/// The query is quoted back because this label is the user's only view of what
/// the agent went looking for. A bare "Searching memory" reads as rummaging.
/// Bounded like every sibling that renders a model-supplied string: these
/// schemas invite a full sentence, and an unbounded one becomes a step row as
/// wide as the transcript.
fn search_label(verb: &str, args: &serde_json::Value) -> String {
    match args["q"].as_str().map(str::trim).filter(|q| !q.is_empty()) {
        Some(q) => format!("{verb} for \"{}\"...", middle_truncate(q, 50)),
        None => format!("{verb}..."),
    }
}

/// The known-tool half of [`describe_tool`]: `Some(label)` for a name we ship,
/// `None` for anything else. Split out from the fallback so the exhaustiveness
/// guard test can ask "is this name labelled?" and get an honest answer, which
/// it cannot do through `describe_tool` (the fallback labels everything).
pub(crate) fn tool_label(name: &str, args: &serde_json::Value) -> Option<String> {
    Some(match name {
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
        // No `output_path` branch: that key belongs to `http_request`'s schema
        // (see the arm below), never `run_python`'s, which declares only `code`,
        // `packages` and `commit_message` (`llm/tools/exec.rs`). The lookup could
        // not match, so the arrow form was unreachable.
        "run_python" => "Running Python code...".to_string(),
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
        // No name in the label: the `run` action declares only `trigger_id`
        // (`capability_manifest`, TRIGGERS_OPS), which is a uuid the user has
        // never seen. There is no `name` key on the payload to prefer over it.
        "run_trigger" => "Running trigger now...".to_string(),
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
        "get_backup_status" => "Checking backup status...".to_string(),
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
                // `file_path`, not `path`: that is what `get_navigate_ui_tool`
                // declares (`llm/tools/misc.rs`) and what the frontend reads off
                // the `NavigationRequested` payload. Reading `path` matched no
                // key, so every file navigation rendered the bare "Opening
                // file..." instead of naming the file.
                "file" => format!(
                    "Opening {}...",
                    args["file_path"].as_str().unwrap_or("file")
                ),
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
        // The one step whose label outlives its turn: the thread parks here and
        // this row is what the user reads while it sleeps. So it leads with the
        // model's own `reason` rather than the event names, which are the
        // engine's vocabulary and answer a question nobody asked. No trailing
        // "..." for the same reason as `ask_user_question`: the reason is a
        // sentence, and `truncate` adds its own when it cuts.
        "await_event" => {
            let reason = args
                .get("reason")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty());
            match reason {
                // `awaited_subject`, because this label supplies the verb: a
                // reason opening "waiting for" would read "Waiting: waiting
                // for".
                Some(reason) => format!("Waiting: {}", truncate(awaited_subject(reason), 60)),
                None => "Waiting for an event...".to_string(),
            }
        }
        "list_event_waits" => "Checking what this thread is waiting for...".to_string(),
        // Names WHAT is being stopped rather than an id. The row sits beside
        // the step that armed the wait, and a uuid there is a string the user
        // cannot resolve to anything.
        "cancel_event_wait" => match args.get("all").and_then(|v| v.as_bool()) {
            Some(true) => "Stopping every subscription on this thread...".to_string(),
            _ => "Stopping a subscription...".to_string(),
        },
        "todo_write" => match args["todos"].as_array() {
            Some(todos) if todos.is_empty() => "Clearing todo list...".to_string(),
            _ => "Updating todo list...".to_string(),
        },
        "save_thread_image" => format!(
            "Saving image to {}...",
            args["path"].as_str().unwrap_or("artifacts")
        ),
        // `image` is a thread reference like "thread:2", exactly how the
        // conversation history labels it. So it reads as something the user can
        // find, rather than an internal id.
        "view_image" => format!("Viewing {}...", args["image"].as_str().unwrap_or("image")),
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
        // The message, never the `thread_id`. A raw uuid resolves to no child
        // the user recognises, and the message is the one part of the call that
        // says what is happening.
        "follow_up_child_thread" => match args["message"].as_str() {
            Some(message) => format!(
                "Following up with child thread: {}...",
                middle_truncate(message, 50)
            ),
            None => "Following up with child thread...".to_string(),
        },
        "list_threads" => "Listing threads...".to_string(),
        "count_threads" => "Counting threads...".to_string(),
        "search_threads" => search_label("Searching past conversations", args),
        "list_changes" => "Listing changes...".to_string(),
        "apply_change" => "Applying change...".to_string(),
        "correct_memory" | "correct_memory_by_id" => "Updating memory...".to_string(),
        "search_memory" => search_label("Searching memory", args),
        "memory_source" => "Tracing a memory to its conversation...".to_string(),
        "query_events" => format!(
            "Querying {} events...",
            args["event_type"].as_str().unwrap_or("all")
        ),
        "count_events" => match args["event_type"].as_str() {
            Some(event_type) => format!("Counting {} events...", event_type),
            None => "Counting events...".to_string(),
        },
        "list_event_types" => "Listing event types...".to_string(),
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
            Some("run") => "Running trigger now...".to_string(),
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
            Some("event_types") => "Listing event types...".to_string(),
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
        // Flat back-compat aliases for the two LLM-exposed `thread_queue`
        // actions; same wording as the grouped arm below.
        "list_thread_queue" => "Listing Thread Queue...".to_string(),
        "update_thread_queue_policy" => "Updating Thread Queue policy...".to_string(),
        "thread_queue" => match args["action"].as_str() {
            Some("update_policy") => "Updating Thread Queue policy...".to_string(),
            Some("run_now") => "Running queued entry now...".to_string(),
            Some("drop") => "Dropping queued entry...".to_string(),
            _ => "Listing Thread Queue...".to_string(),
        },
        // The GROUPED names are what the model actually emits, since the flat
        // ones are back-compat aliases that are never offered. So a grouped arm
        // ignoring `action` mislabels every action but one, and rendering a
        // read as "Updating memory..." claims a write that never happened.
        "memory" => match args["action"].as_str() {
            Some("search") => search_label("Searching memory", args),
            Some("source") => "Tracing a memory to its conversation...".to_string(),
            _ => "Updating memory...".to_string(),
        },
        "threads" => match args["action"].as_str() {
            Some("count") => "Counting threads...".to_string(),
            Some("search") => search_label("Searching past conversations", args),
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
        "register_plugin_marketplace" => format!(
            "Registering marketplace {}...",
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
            format!("MCP: {}...", mcp_tool_suffix(name).unwrap_or(name))
        }
        _ => return None,
    })
}

/// The step-row verb a Codex `file_change` kind implies.
///
/// The kind arrives as `{"type": "add"}` over the app-server protocol and as a
/// bare `"add"` from older exec frames, so both resolve. An unrecognized one
/// reads "Change", which is true of every kind and claims nothing extra.
///
/// The vocabulary is deliberately Claude Code's, rather than the sentence verbs
/// `PermissionCard` uses. That card builds a sentence. This is a step label,
/// and it has to read like the label on a Claude Code row carrying the same
/// edit.
fn change_kind_verb(kind: Option<&serde_json::Value>) -> &'static str {
    let name = match kind {
        Some(serde_json::Value::String(s)) => s.as_str(),
        Some(v) => v.get("type").and_then(|t| t.as_str()).unwrap_or(""),
        None => "",
    };
    match name {
        "add" => "Write",
        "update" => "Edit",
        "delete" => "Delete",
        _ => "Change",
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
        // Same label as Codex's `todo_list` below. The two backends' plan steps
        // are one thing to the user and must read alike. The frontend already
        // assumes that, rendering both with one marker list.
        "TodoWrite" => "Update plan".into(),
        "ExitPlanMode" => "Present plan for approval".into(),
        // Codex item types (see runtime/codex_parse.rs). Codex reports
        // coarse-grained items, not named tools. Each arm lands on the SAME
        // sentence its Claude Code counterpart produces: the two backends share
        // every transcript component, so a row that reads differently is the
        // only thing left that can tell them apart.
        "command_execution" => {
            let script = shell_script_body(str_arg("command"));
            let line = first_command_line(&script);
            if line.is_empty() {
                "Run command".into()
            } else {
                format!("Run {}", middle_truncate(line, 60))
            }
        }
        "file_change" => match args.get("changes").and_then(|c| c.as_array()) {
            Some(changes) if !changes.is_empty() => {
                let verbs: Vec<&str> = changes
                    .iter()
                    .map(|c| change_kind_verb(c.get("kind")))
                    .collect();
                // One verb for the whole set only when they agree: a patch that
                // both creates and deletes is a "change", same honesty rule
                // `renderFileChangeQuestion` applies on the permission card.
                let verb = if verbs.windows(2).all(|w| w[0] == w[1]) {
                    verbs[0]
                } else {
                    "Change"
                };
                if changes.len() > 1 {
                    format!("{} {} files", verb, changes.len())
                } else {
                    // Indexing is safe: this arm is guarded on a non-empty array.
                    match changes[0]
                        .get("path")
                        .and_then(|p| p.as_str())
                        .filter(|p| !p.is_empty())
                    {
                        Some(p) => format!("{} {}", verb, basename(p)),
                        None => format!("{} 1 file", verb),
                    }
                }
            }
            _ => "Apply file changes".into(),
        },
        "web_search" => {
            let q = str_arg("query");
            if q.is_empty() {
                "Web search".into()
            } else {
                format!("Search '{}'", middle_truncate(q, 60))
            }
        }
        "todo_list" => "Update plan".into(),
        // An MCP tool reaches both backends under the same
        // `mcp__<server>__<tool>` name, and the server prefix is noise in a
        // step row.
        _ if name.starts_with("mcp__") => {
            format!("MCP: {}", mcp_tool_suffix(name).unwrap_or(name))
        }
        // Unlike `tool_label`, this fallback stays reachable. Coding-agent tool
        // names come from the model vendors, not a registry we own. A tool
        // added upstream tomorrow can have no arm here before it ships, and
        // showing its name beats showing nothing.
        _ => name.to_string(),
    }
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
