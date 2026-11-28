//! INVARIANT: Credentials for every `git2` clone the engine performs.
//!
//! Four sites share this: the marketplace scan, the plugin install fetch, and
//! the two `git_clone` tool routes. Without a credential callback libgit2 can
//! only clone public repos, and a private or internal one fails with
//! `remote authentication required but no callback set`.
//!
//! An HTTPS secret comes from the Lucidos credential store and nowhere else.
//! The store is `async` over a `PgPool` while a clone is synchronous, because
//! git2 types are not `Send`. So the caller resolves a [`GitCredentials`]
//! before the clone and hands it in. SSH keys still come from ssh-agent, and a
//! `git credential` helper is still consulted, because neither carries a
//! secret through Lucidos.

use crate::core::credentials::{
    credential_base_url_matches, AuthType, Credential, CredentialStore,
};
use git2::{Cred, CredentialType, FetchOptions, RemoteCallbacks};
use sqlx::PgPool;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Once};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialSource {
    Username,
    SshAgent,
    /// INVARIANT: A stored secret as the HTTPS password. GitHub documents this form for
    /// an installation token (`x-access-token:<token>`) and ignores the
    /// username, and a GitHub Enterprise install behaves the same.
    StoredAsPassword,
    StoredAsUsername,
    Helper,
    Anonymous,
}

impl CredentialSource {
    fn bit(self) -> u8 {
        match self {
            Self::Username => 1,
            Self::SshAgent => 1 << 1,
            Self::StoredAsPassword => 1 << 2,
            Self::StoredAsUsername => 1 << 3,
            Self::Helper => 1 << 4,
            Self::Anonymous => 1 << 5,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoredKind {
    None,
    Token,
    /// INVARIANT: A username and password pair. Only the password form applies, because
    /// sending the password as the username would drop the username.
    UserPass,
}

/// INVARIANT: A credential-store row, reduced to what a clone can present.
///
/// `service_name` and `base_url` are safe to print. `secret` never is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredGitCredential {
    pub service_name: String,
    pub base_url: String,
    pub username: Option<String>,
    pub secret: String,
}

fn presentable_pair(auth_type: AuthType, value: &str) -> Option<(Option<String>, String)> {
    let (username, secret) = match auth_type {
        AuthType::Bearer | AuthType::ApiKey => (None, value.to_string()),
        AuthType::Basic => match value.split_once(':') {
            Some((user, password)) => (Some(user.to_string()), password.to_string()),
            None => (None, value.to_string()),
        },
        AuthType::Password => {
            let parsed: serde_json::Value = serde_json::from_str(value).ok()?;
            let username = parsed["username"].as_str()?.to_string();
            let password = parsed["password"].as_str()?.to_string();
            (Some(username), password)
        }
        AuthType::OauthClient
        | AuthType::EmailPassword
        | AuthType::Secret
        | AuthType::OnepasswordServiceAccount
        | AuthType::Unknown => return None,
    };
    (!secret.trim().is_empty()).then_some((username, secret))
}

impl StoredGitCredential {
    /// INVARIANT: Reduce a stored credential to one entry per base URL it declares, or an
    /// empty vector if its type cannot clone.
    ///
    /// `bearer` and `api_key` hold a bare token. `basic` holds the
    /// `username:password` pair its own form asks for. `password` holds that
    /// pair as JSON. The rest carry no secret a git remote could use, `secret`
    /// included: it is signed with rather than sent.
    ///
    /// One entry per member, because a scope is a set and each member is an
    /// independent host this credential may be offered at. The caller matches
    /// per entry, so nothing here widens what a member covers.
    ///
    /// The stored value is presented byte for byte, because a password may
    /// legitimately start or end with a space. Trimming decides emptiness and
    /// nothing else, matching the credential form and the env-var injection.
    pub fn entries_from_credential(credential: Credential, value: &str) -> Vec<Self> {
        let Some((username, secret)) = presentable_pair(credential.auth_type, value) else {
            return Vec::new();
        };
        let username = username.filter(|u| !u.is_empty());
        credential
            .base_urls
            .iter()
            .map(|base_url| Self {
                service_name: credential.service_name.clone(),
                base_url: base_url.clone(),
                username: username.clone(),
                secret: secret.clone(),
            })
            .collect()
    }

    fn kind(&self) -> StoredKind {
        match self.username {
            Some(_) => StoredKind::UserPass,
            None => StoredKind::Token,
        }
    }
}

/// INVARIANT: The stored credentials a clone may present, resolved before it starts.
///
/// This is `Send`, which the store's own types are not required to be. A
/// caller resolves it in async code and passes it into the blocking clone.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GitCredentials {
    /// INVARIANT: Most specific `base_url` first, so the narrowest scope wins.
    entries: Vec<StoredGitCredential>,
}

impl GitCredentials {
    pub fn none() -> Self {
        Self::default()
    }

    pub async fn resolve_one(pool: &PgPool, url: &str) -> Self {
        Self::resolve_many(pool, &[url]).await
    }

    pub async fn resolve_many<S: AsRef<str>>(pool: &PgPool, urls: &[S]) -> Self {
        let mut entries: Vec<StoredGitCredential> = Vec::new();
        for url in urls {
            let url = url.as_ref();
            let found = match CredentialStore::find_by_url(pool, url).await {
                Ok(found) => found,
                Err(e) => {
                    log_legacy!(
                        "[GitAuth] credential lookup failed for {}: {}",
                        redacted(url),
                        e
                    );
                    continue;
                }
            };
            let Some(credential) = found else { continue };
            let service_name = credential.service_name.clone();
            if credential.base_urls.is_empty() {
                log_legacy!(
                    "[GitAuth] credential {} declares no base URL, so it is offered nowhere",
                    service_name
                );
                continue;
            }
            let Some(value) = credential.secret_or_log(pool, "[GitAuth]").await else {
                continue;
            };
            let found_entries = StoredGitCredential::entries_from_credential(credential, &value);
            if found_entries.is_empty() {
                log_legacy!(
                    "[GitAuth] credential {} holds nothing a git clone can present",
                    service_name
                );
                continue;
            }
            for entry in found_entries {
                if !entries.iter().any(|e| e.base_url == entry.base_url) {
                    entries.push(entry);
                }
            }
        }
        entries.sort_by_key(|e| std::cmp::Reverse(e.base_url.len()));
        Self { entries }
    }

    /// INVARIANT: The credential whose own `base_url` scopes `url`.
    ///
    /// libgit2 passes the URL it is authenticating against, which a redirect
    /// can move to another host mid-clone. Re-matching on every callback is
    /// what stops a secret following it there.
    pub fn for_url(&self, url: &str) -> Option<&StoredGitCredential> {
        self.entries
            .iter()
            .find(|e| credential_base_url_matches(&e.base_url, url))
    }
}

pub fn credential_plan(allowed: CredentialType, stored: StoredKind) -> Vec<CredentialSource> {
    let mut plan = Vec::new();
    if allowed.contains(CredentialType::USERNAME) {
        plan.push(CredentialSource::Username);
    }
    if allowed.contains(CredentialType::SSH_KEY) {
        plan.push(CredentialSource::SshAgent);
    }
    if allowed.contains(CredentialType::USER_PASS_PLAINTEXT) {
        match stored {
            StoredKind::Token => {
                plan.push(CredentialSource::StoredAsPassword);
                plan.push(CredentialSource::StoredAsUsername);
            }
            StoredKind::UserPass => plan.push(CredentialSource::StoredAsPassword),
            StoredKind::None => {}
        }
        plan.push(CredentialSource::Helper);
    }
    if allowed.contains(CredentialType::DEFAULT) {
        plan.push(CredentialSource::Anonymous);
    }
    plan
}

/// INVARIANT: Take the first source `tried` has not recorded yet, and record it.
///
/// This is the whole retry guard, and it is why a rejected credential cannot
/// hang a scan. libgit2 calls the credential callback again every time the
/// remote refuses what it got, so offering any source twice loops forever.
/// Returning `None` once the plan is spent turns that loop into a clean error.
pub fn next_untried(plan: &[CredentialSource], tried: &mut u8) -> Option<CredentialSource> {
    let next = plan.iter().copied().find(|s| *tried & s.bit() == 0)?;
    *tried |= next.bit();
    Some(next)
}

fn has_scheme(url: &str, scheme: &str) -> bool {
    url.split_once("://")
        .is_some_and(|(s, _)| s.eq_ignore_ascii_case(scheme))
}

pub fn is_local_url(url: &str) -> bool {
    has_scheme(url, "file")
}

/// INVARIANT: `url` with any inline HTTP credential replaced by `***@`.
///
/// An HTTPS URL may carry a token in its authority. The URL reaches the log, a
/// failure the Plugins panel shows, and `git_clone`'s commit message and event,
/// and none is a place for a secret. An SSH URL keeps its username, which names
/// an account rather than proving one.
pub fn redacted(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://").filter(|_| is_http_url(url)) else {
        return url.to_string();
    };
    let (authority, path) = rest.split_at(rest.find(['/', '?', '#']).unwrap_or(rest.len()));
    match authority.rsplit_once('@') {
        Some((_, host)) => format!("{scheme}://***@{host}{path}"),
        None => url.to_string(),
    }
}

fn is_http_url(url: &str) -> bool {
    has_scheme(url, "https") || has_scheme(url, "http")
}

fn credential_base_url_hint(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let authority = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let authority = authority.trim();
    (!authority.is_empty()).then(|| {
        format!(
            "{}://{}",
            scheme.to_ascii_lowercase(),
            authority.to_ascii_lowercase()
        )
    })
}

/// INVARIANT: The error the callback returns once every source has been offered.
///
/// Class and code match what libgit2 raises for a refused HTTP credential, so
/// [`describe_clone_failure`] recognises it like any other auth failure.
fn all_credentials_spent() -> git2::Error {
    git2::Error::new(
        git2::ErrorCode::Auth,
        git2::ErrorClass::Http,
        "every available credential was rejected",
    )
}

fn stored_credential_missing() -> git2::Error {
    git2::Error::from_str("a stored credential source was offered without a credential")
}

const TOKEN_USERNAME: &str = "x-access-token";

/// INVARIANT: Offer untried sources until one produces a credential.
///
/// A source that cannot build one is skipped rather than fatal. Two cases need
/// that: an absent ssh-agent, and a credential helper missing from the PATH of
/// an app launched from the Dock. Neither must abort a clone the next source
/// could finish. When the plan runs out the caller gets one auth error, which
/// is also what stops libgit2 re-invoking the callback forever.
fn first_working<T, E>(
    plan: &[CredentialSource],
    tried: &mut u8,
    mut build: impl FnMut(CredentialSource) -> Result<T, E>,
) -> Result<T, git2::Error> {
    while let Some(source) = next_untried(plan, tried) {
        if let Ok(credential) = build(source) {
            return Ok(credential);
        }
    }
    Err(all_credentials_spent())
}

fn build_credential(
    source: CredentialSource,
    url: &str,
    username_from_url: Option<&str>,
    stored: Option<&StoredGitCredential>,
) -> Result<Cred, git2::Error> {
    match source {
        CredentialSource::Username => Cred::username(username_from_url.unwrap_or("git")),
        CredentialSource::SshAgent => Cred::ssh_key_from_agent(username_from_url.unwrap_or("git")),
        CredentialSource::StoredAsPassword => {
            let stored = stored.ok_or_else(stored_credential_missing)?;
            let username = stored
                .username
                .as_deref()
                .or(username_from_url)
                .unwrap_or(TOKEN_USERNAME);
            Cred::userpass_plaintext(username, &stored.secret)
        }
        CredentialSource::StoredAsUsername => {
            let stored = stored.ok_or_else(stored_credential_missing)?;
            Cred::userpass_plaintext(&stored.secret, "")
        }
        CredentialSource::Helper => {
            Cred::credential_helper(&git2::Config::open_default()?, url, username_from_url)
        }
        CredentialSource::Anonymous => Cred::default(),
    }
}

fn plan_for<'a>(
    url: &str,
    allowed: CredentialType,
    credentials: &'a GitCredentials,
) -> (Vec<CredentialSource>, Option<&'a StoredGitCredential>) {
    let stored = credentials.for_url(url);
    let kind = stored.map_or(StoredKind::None, StoredGitCredential::kind);
    (credential_plan(allowed, kind), stored)
}

/// INVARIANT: Callbacks that authenticate a clone.
///
/// The plan is resolved from the URL libgit2 passes on each call, never from
/// the URL the clone started with. Those differ when a fetch follows a
/// redirect, and a credential picked once at the start would travel to the new
/// host.
///
/// A public clone is unaffected: libgit2 invokes the credential callback only
/// when the remote demands one.
fn authenticated_callbacks(credentials: GitCredentials) -> RemoteCallbacks<'static> {
    let mut tried = 0u8;
    let mut callbacks = RemoteCallbacks::new();
    callbacks.credentials(move |url, username_from_url, allowed| {
        let (plan, stored) = plan_for(url, allowed, &credentials);
        first_working(&plan, &mut tried, |source| {
            log_legacy!("[GitAuth] trying {:?} for {}", source, redacted(url));
            build_credential(source, url, username_from_url, stored)
                .inspect_err(|e| log_legacy!("[GitAuth] {:?} is unavailable: {}", source, e))
        })
    });
    callbacks
}

fn authenticated_fetch_options(
    credentials: &GitCredentials,
    deadline: Option<Instant>,
) -> FetchOptions<'static> {
    let mut callbacks = authenticated_callbacks(credentials.clone());
    if let Some(deadline) = deadline {
        callbacks.sideband_progress(move |_| Instant::now() < deadline);
        callbacks.transfer_progress(move |_| Instant::now() < deadline);
    }
    let mut opts = FetchOptions::new();
    opts.remote_callbacks(callbacks);
    opts
}

pub fn shallow_clone(
    url: &str,
    branch: Option<&str>,
    into: &Path,
    credentials: &GitCredentials,
) -> Result<git2::Repository, String> {
    clone_repo(url, branch, into, credentials, None)
}

/// INVARIANT: Clone like [`shallow_clone`], but on a blocking thread, giving up after `limit`.
///
/// A clone is a blocking network fetch, so an async caller must never run it on
/// a tokio worker. libgit2 clones in-process, so no child exists to kill at the
/// deadline. Three bounds stand in for that: the progress callbacks abort the
/// transfer, [`SERVER_IDLE_TIMEOUT_MS`] ends a silent socket, and the caller
/// stops waiting.
///
/// The clone lands in a staging sibling of `into` that only this call writes.
/// It moves into place only while the caller still waits for it. A clone that
/// outlives its caller removes its own staging directory and never touches `into`.
pub async fn shallow_clone_within(
    url: &str,
    branch: Option<&str>,
    into: &Path,
    credentials: &GitCredentials,
    limit: Duration,
) -> Result<(), String> {
    let deadline = Instant::now() + limit;
    // INVARIANT: Whoever sets this first decides the outcome: the clone thread by moving
    // the clone into place, or the caller by giving up on it.
    let settled = Arc::new(AtomicBool::new(false));
    let mut clone = {
        let (url, branch) = (url.to_string(), branch.map(str::to_string));
        let (into, credentials) = (into.to_path_buf(), credentials.clone());
        let settled = settled.clone();
        tokio::task::spawn_blocking(move || {
            let staging = staging_dir(&into);
            let cloned = clone_repo(
                &url,
                branch.as_deref(),
                &staging,
                &credentials,
                Some(deadline),
            );
            let result = match cloned.map(drop) {
                Ok(()) if !settled.swap(true, Ordering::SeqCst) => std::fs::rename(&staging, &into)
                    .map_err(|e| {
                        format!(
                            "git clone finished but could not move into {}: {e}",
                            crate::core::home_path::abbreviate(&into)
                        )
                    }),
                Err(e) if Instant::now() < deadline => Err(e),
                _ => Err(clone_timed_out(&url, limit)),
            };
            if result.is_err() {
                let _ = std::fs::remove_dir_all(&staging);
            }
            result
        })
    };
    let _give_up = GiveUpOnDrop(settled.clone());
    let joined = match tokio::time::timeout_at(deadline.into(), &mut clone).await {
        Ok(joined) => joined,
        Err(_) if settled.swap(true, Ordering::SeqCst) => clone.await,
        Err(_) => return Err(clone_timed_out(url, limit)),
    };
    joined.unwrap_or_else(|e| Err(format!("git clone failed: {e}")))
}

struct GiveUpOnDrop(Arc<AtomicBool>);

impl Drop for GiveUpOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

fn staging_dir(into: &Path) -> PathBuf {
    let name = into.file_name().unwrap_or_default().to_string_lossy();
    into.with_file_name(format!(".{name}.clone-{}", uuid::Uuid::new_v4()))
}

fn clone_timed_out(url: &str, limit: Duration) -> String {
    format!(
        "git clone of {} timed out after {}s. The remote stopped answering, or the \
         repository is too large to fetch in that time.",
        redacted(url),
        limit.as_secs()
    )
}

/// INVARIANT: How long a clone's socket may stay silent before libgit2 gives up on it.
///
/// No progress callback fires while a socket waits, so without this a remote
/// that stops answering blocks its cloning thread for good. A git server sends
/// progress or keepalive packets while it prepares a pack, so a healthy clone
/// never goes this long without a byte.
const SERVER_IDLE_TIMEOUT_MS: i32 = 120_000;

fn bound_idle_sockets() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        // SAFETY: libgit2 reads this global only when a clone opens a socket.
        // Every engine clone calls this first, and `call_once` holds them all
        // until the write is done, so no read races it.
        let set = unsafe { git2::opts::set_server_timeout_in_milliseconds(SERVER_IDLE_TIMEOUT_MS) };
        if let Err(e) = set {
            log_legacy!("[GitAuth] could not bound idle clone sockets: {}", e);
        }
    });
}

/// INVARIANT: Every engine clone goes through here, so no site can forget the callbacks,
/// the idle bound or the error mapping. The depth is 1 except for a local
/// `file://` URL: libgit2's local transport rejects a shallow fetch, and a
/// local clone costs no bandwidth to make deep.
fn clone_repo(
    url: &str,
    branch: Option<&str>,
    into: &Path,
    credentials: &GitCredentials,
    deadline: Option<Instant>,
) -> Result<git2::Repository, String> {
    bound_idle_sockets();
    let offered = credentials.for_url(url).map(|c| c.service_name.clone());
    let mut builder = git2::build::RepoBuilder::new();
    let mut fetch_opts = authenticated_fetch_options(credentials, deadline);
    if !is_local_url(url) {
        fetch_opts.depth(1);
    }
    if let Some(branch) = branch {
        builder.branch(branch);
    }
    builder.fetch_options(fetch_opts);
    builder
        .clone(url, into)
        .map_err(|e| describe_clone_failure(url, &e, offered.as_deref()))
}

fn is_auth_failure(err: &git2::Error) -> bool {
    if err.code() == git2::ErrorCode::Auth {
        return true;
    }
    let message = err.message().to_ascii_lowercase();
    message.contains("authentication required") || message.contains("authentication failed")
}

fn no_credential_remedy(url: &str) -> String {
    if !is_http_url(url) {
        return "Load a key that can read this repository into ssh-agent, with `ssh-add`, \
                and register it with the host."
            .to_string();
    }
    let base_url = credential_base_url_hint(url).unwrap_or_default();
    format!(
        "Add a credential in Settings, Credentials, with Base URL {base_url}, Auth Type \
         Bearer Token, and a token that can read the repo. You can instead log in once \
         through a git credential helper, or use an SSH URL (git@...) with your key loaded \
         in ssh-agent."
    )
}

/// INVARIANT: Turn a clone failure into something the user can act on.
///
/// A non-auth failure keeps its own text: "repository not found" must never be
/// reported as a credential problem. `offered` names the stored credential
/// that was presented, if any, and is never the secret itself.
fn describe_clone_failure(url: &str, err: &git2::Error, offered: Option<&str>) -> String {
    if !is_auth_failure(err) {
        return format!("git clone failed: {}", err);
    }
    let remedy = match offered {
        Some(service_name) => format!(
            "The credential '{service_name}' was rejected, or it cannot read this \
             repository. Store a token with read access to the repo (Settings, \
             Credentials), or use an SSH URL (git@...) with your key loaded in ssh-agent."
        ),
        None => no_credential_remedy(url),
    };
    format!(
        "Authentication required for {}. This repository looks private or internal. {remedy}",
        redacted(url)
    )
}

#[cfg(test)]
#[path = "git_auth_tests.rs"]
mod tests;
