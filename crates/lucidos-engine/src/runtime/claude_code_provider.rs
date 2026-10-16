//! Whether a Claude Code session's progress notes reach the user.
//!
//! An always-thinking model writes its prose before a tool call into a
//! `thinking` block. The block carries text only when the request sets
//! `thinking.display: "updates"`. Claude Code sets it on Anthropic's own API,
//! and the Vertex relay sets it on Vertex (`runtime::vertex_relay`). On every
//! other provider nobody does. The block then comes back empty and the user
//! reads nothing, while the agent's own context still holds the text.
//!
//! The engine asks `claude auth status` which provider a session will use, and
//! allows only the two proven routes. Anything else counts as hidden.
//!
//! A temporary measure: `docs/temporary-measures.md` § "Claude Code's progress
//! notes are hidden on other providers". Plan:
//! `docs/plans/2026-10-16-tell-the-agent-when-its-notes-are-hidden.md`.

use std::collections::HashMap;
use std::ffi::OsString;
use std::future::Future;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use tokio::process::Command;
use uuid::Uuid;

use super::agent_runtime::AccountPin;

/// How long the check may run. A spawn never waits longer for it.
const CHECK_TIMEOUT: Duration = Duration::from_secs(5);

/// How long a verdict stays valid. A re-login is picked up within this window
/// without an engine restart.
const VERDICT_TTL: Duration = Duration::from_secs(10 * 60);

/// How long a failed check holds off the next one. Short, since a timeout or a
/// binary locked by an update says nothing about the provider. Long enough that
/// a hung binary does not stall every spawn for the full timeout.
const FAILURE_TTL: Duration = Duration::from_secs(30);

/// `apiProvider` on Anthropic's own API.
const PROVIDER_FIRST_PARTY: &str = "firstParty";

/// `apiProvider` on Google Vertex, where the relay asks for the notes.
const PROVIDER_VERTEX: &str = "vertex";

/// Whether the text a model writes before a tool call reaches the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotesVisibility {
    Shown,
    Hidden,
}

/// The notes reach the user on an allowlist only. An unknown provider, a failed
/// check and every other route count as hidden.
pub(crate) fn notes_visibility_for(provider: Option<&str>, relay_stamped: bool) -> NotesVisibility {
    match provider {
        Some(PROVIDER_FIRST_PARTY) => NotesVisibility::Shown,
        Some(PROVIDER_VERTEX) if relay_stamped => NotesVisibility::Shown,
        _ => NotesVisibility::Hidden,
    }
}

/// What a spawn runs `claude` with, so the check reads the provider the
/// session will use. The same fields a session's `SpawnArgs` carry.
pub(crate) struct ProviderInputs<'a> {
    pub(crate) binary_override: Option<&'a str>,
    pub(crate) user_env_vars: &'a [(String, String)],
    pub(crate) account_pin: Option<&'a AccountPin>,
    /// The session's working directory. Claude Code reads the project's
    /// settings from it, and they can switch the provider.
    pub(crate) worktree_path: &'a std::path::Path,
    pub(crate) thread_id: Uuid,
}

/// The verdict for a session about to spawn. Never fails and never waits past
/// [`CHECK_TIMEOUT`]: a check that could not run reads as hidden.
pub(crate) async fn notes_visibility(inputs: &ProviderInputs<'_>) -> NotesVisibility {
    let relay_stamped = super::claude_code::vertex_relay_url(
        inputs.user_env_vars,
        inputs.thread_id,
        super::vertex_relay::port(),
    )
    .is_some();
    let (binary, key) = check_target(inputs);
    let provider = cached_provider(&CACHE, key, Instant::now(), || {
        run_check(provider_check_command(inputs, &binary), CHECK_TIMEOUT)
    })
    .await;
    notes_visibility_for(provider.as_deref(), relay_stamped)
}

fn check_target(inputs: &ProviderInputs<'_>) -> (OsString, CheckKey) {
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    let binary = super::claude_code::resolve_claude_binary(
        home.as_deref(),
        inputs.binary_override.map(std::path::Path::new),
    );
    let key = CheckKey {
        binary: binary.clone(),
        env_digest: digest_of_env(inputs.user_env_vars),
        pin: inputs.account_pin.cloned(),
        worktree_path: inputs.worktree_path.to_path_buf(),
    };
    (binary, key)
}

/// A digest of the user's variables, order aside. The key holds the digest and
/// never the values, which can be credentials.
fn digest_of_env(env: &[(String, String)]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut sorted = env.to_vec();
    sorted.sort();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    sorted.hash(&mut hasher);
    hasher.finish()
}

/// `claude auth status`, under the environment a session gets: the user's
/// variables, then the thread's account pin, as `build_command` applies them.
/// It runs in the session's directory, where the project's settings apply.
fn provider_check_command(inputs: &ProviderInputs<'_>, binary: &OsString) -> Command {
    let mut cmd = Command::new(binary);
    cmd.args(["auth", "status"])
        .current_dir(inputs.worktree_path);
    crate::core::apply_to_subprocess_env(&mut cmd, inputs.user_env_vars);
    super::claude_code::apply_account_pin(&mut cmd, inputs.account_pin);
    cmd.env_remove("CLAUDECODE");
    cmd
}

/// Only the provider leaves this function: the reply also names the signed-in
/// account.
fn parse_api_provider(stdout: &[u8]) -> Option<String> {
    let reply: serde_json::Value = serde_json::from_slice(stdout).ok()?;
    reply.get("apiProvider")?.as_str().map(str::to_string)
}

/// The provider `cmd` reports, or `None` when the check could not run, ran
/// past `limit`, or answered with no provider. Logs the reason, never the
/// environment.
async fn run_check(mut cmd: Command, limit: Duration) -> Option<String> {
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    let child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => {
            crate::log!("[ClaudeCodeProvider] could not run `claude auth status`: {e}");
            return None;
        }
    };
    match tokio::time::timeout(limit, child.wait_with_output()).await {
        Ok(Ok(output)) => {
            let provider = parse_api_provider(&output.stdout);
            if provider.is_none() {
                crate::log!("[ClaudeCodeProvider] `claude auth status` named no provider");
            }
            provider
        }
        Ok(Err(e)) => {
            crate::log!("[ClaudeCodeProvider] `claude auth status` failed: {e}");
            None
        }
        Err(_) => {
            crate::log!(
                "[ClaudeCodeProvider] `claude auth status` took over {}s",
                limit.as_secs()
            );
            None
        }
    }
}

/// What picks a session's provider: the binary, the user's variables, the
/// account pin and the directory. The engine's own environment is the same for
/// every spawn.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CheckKey {
    binary: OsString,
    env_digest: u64,
    pin: Option<AccountPin>,
    worktree_path: std::path::PathBuf,
}

/// One answer per key, kept for [`VERDICT_TTL`], or [`FAILURE_TTL`] for a
/// failed check. Ephemeral by design, so an engine restart runs the check
/// again.
#[derive(Default)]
struct ProviderCache {
    entries: Mutex<HashMap<CheckKey, (Instant, Option<String>)>>,
}

static CACHE: LazyLock<ProviderCache> = LazyLock::new(ProviderCache::default);

impl ProviderCache {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<CheckKey, (Instant, Option<String>)>> {
        self.entries.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn get(&self, key: &CheckKey, now: Instant) -> Option<Option<String>> {
        let entries = self.lock();
        let (at, provider) = entries.get(key)?;
        is_fresh(*at, provider, now).then(|| provider.clone())
    }

    fn put(&self, key: CheckKey, now: Instant, provider: Option<String>) {
        let mut entries = self.lock();
        entries.retain(|_, (at, provider)| is_fresh(*at, provider, now));
        entries.insert(key, (now, provider));
    }
}

fn is_fresh(at: Instant, provider: &Option<String>, now: Instant) -> bool {
    let ttl = if provider.is_some() {
        VERDICT_TTL
    } else {
        FAILURE_TTL
    };
    now.saturating_duration_since(at) < ttl
}

/// The cached provider for `key`, running `check` on a miss.
async fn cached_provider<Fut: Future<Output = Option<String>>>(
    cache: &ProviderCache,
    key: CheckKey,
    now: Instant,
    check: impl FnOnce() -> Fut,
) -> Option<String> {
    if let Some(provider) = cache.get(&key, now) {
        return provider;
    }
    let provider = check().await;
    cache.put(key, now, provider.clone());
    provider
}

#[cfg(test)]
#[path = "claude_code_provider_tests.rs"]
mod tests;
