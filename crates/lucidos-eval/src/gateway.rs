//! Register each arm with the workspace gateway, and read back its port.
//!
//! Registration and port discovery are best-effort. Before seeding, a
//! configured gateway must prove the slug is not already registered.
//! Without gateway configuration, evaluation can run unregistered. Registration
//! and port-discovery failures are reported; port discovery falls back to a free
//! port. A failed pre-seed observation stops before creating any arm resources.
//!
//! Registration is what makes an arm browsable. The gateway lists it in the
//! picker, routes `/eval-<label>-<arm>-<repeat>/` to it, and derives its
//! `lucidos_`-prefixed database from the same slug the harness seeded.
//! Autostart stays off, so the gateway never spawns an arm engine of its own
//! accord.

use std::path::Path;
use std::time::Duration;

use lucidos_local_token as local_token;

type Fallible<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// How long a control call may take before the run stops waiting on it.
///
/// Short on purpose. The gateway is loopback and answers in milliseconds, and
/// nothing here is worth delaying a seed for.
const TIMEOUT: Duration = Duration::from_secs(5);

/// Where the gateway is, and the credential that reaches its control plane.
pub struct Gateway {
    base_url: String,
    client: reqwest::Client,
    /// The machine-local token, absent when there is no gateway on this
    /// machine. Sent as `x-lucidos-local-token`, which is what proves a caller
    /// is local: a loopback address does not, because `tailscale serve` proxies
    /// remote requests from `127.0.0.1` too.
    token: Option<String>,
}

impl Gateway {
    /// Read the configuration. `None` when nothing usable is set.
    pub fn from_env() -> Option<Gateway> {
        let base_url = std::env::var("LUCIDOS_EVAL_GATEWAY_URL")
            .ok()
            .map(|u| u.trim().trim_end_matches('/').to_string())
            .filter(|u| !u.is_empty())?;
        let client = reqwest::Client::builder()
            // The loopback pair `.claude/rules/rust.md` prescribes. The dev
            // gateway's certificate is self-signed, and `no_proxy` keeps an
            // `HTTPS_PROXY` out of a hop that never leaves this machine.
            .no_proxy()
            .danger_accept_invalid_certs(true)
            .timeout(TIMEOUT)
            .build()
            .ok()?;
        Some(Gateway {
            base_url,
            client,
            token: local_token::read(),
        })
    }

    fn control(&self, path: &str) -> String {
        format!("{}/~/api/v1/control/{path}", self.base_url)
    }

    fn request(&self, builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.token {
            Some(token) => builder.header(local_token::HEADER_LOCAL_TOKEN, token),
            None => builder,
        }
    }

    /// Register `dir` as a workspace, answering with the port it holds.
    async fn adopt_call(&self, dir: &Path, slug: &str, name: &str) -> Fallible<u16> {
        // Hold one generation lock from preflight through publication. A
        // cleanup retry must take the same lock before archiving this file.
        let store = OwnerStore::lock(dir, slug)?;
        match std::fs::symlink_metadata(store.path()) {
            Ok(_) => {
                return Err("unarchived owner credential exists; refusing a new adoption".into())
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        let body = serde_json::json!({
            "dir": dir,
            "name": name,
            // The arms are a measurement, not a service. Nothing about them
            // should come up on a gateway boot.
            "autostart": false,
            "origin": "eval",
            "ephemeral": true,
        });
        let response = self
            .request(self.client.post(self.control("workspaces/adopt")))
            .json(&body)
            .send()
            .await?;
        let status = response.status();
        let payload: serde_json::Value = response.json().await.unwrap_or_default();
        if !status.is_success() {
            let reason = payload["error"].as_str().unwrap_or("no reason given");
            return Err(format!("the gateway answered {status}: {reason}").into());
        }
        let (port, token) = adoption(&payload, slug)?;
        store.save(dir, slug, token)?;
        Ok(port)
    }

    /// Stop the gateway's own engine for `slug`, keeping the registry entry.
    ///
    /// 202 when it stopped one, and a 404 for a slug the gateway does not know.
    /// Both are fine here, so only a transport failure is reported.
    async fn stop_call(&self, slug: &str) -> Fallible<()> {
        self.request(
            self.client
                .post(self.control(&format!("workspaces/{slug}/stop"))),
        )
        .send()
        .await?;
        Ok(())
    }

    /// The port the registry holds for `slug`, if it is registered.
    async fn registered_port_call(&self, slug: &str) -> Fallible<Option<u16>> {
        let payload: serde_json::Value = self
            .request(self.client.get(self.control("workspaces")))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let workspaces = payload["workspaces"]
            .as_array()
            .ok_or("gateway registry returned no workspace list")?;
        match workspaces.iter().find(|ws| ws["id"].as_str() == Some(slug)) {
            Some(ws) => Ok(Some(
                ws["port"]
                    .as_u64()
                    .and_then(|port| u16::try_from(port).ok())
                    .filter(|port| *port != 0)
                    .ok_or("registered workspace has no valid port")?,
            )),
            None if workspaces.iter().all(|ws| ws["id"].as_str().is_some()) => Ok(None),
            None => Err("gateway registry contains an invalid workspace identity".into()),
        }
    }
}

/// Seeding cannot take over a registration, even if its directory is missing.
/// An unavailable configured gateway leaves ownership unknown and refuses seed.
pub async fn check_seed_registration(slug: &str) -> Fallible<()> {
    let Some(gateway) = Gateway::from_env() else {
        return Ok(());
    };
    gateway.check_seed_call(slug).await
}

impl Gateway {
    async fn check_seed_call(&self, slug: &str) -> Fallible<()> {
        if self.registered_port_call(slug).await?.is_some() {
            return Err(format!("refusing to seed registered workspace '{slug}'; resume its recorded run or choose a fresh run label").into());
        }
        Ok(())
    }
}

/// Register one arm with the gateway. Best-effort, and never a run's problem.
///
/// The port the gateway allocates is read back at run time by
/// [`registered_port`], not carried from here: `seed` and `run` are separate
/// invocations, so one path serves both.
pub async fn register_arm(dir: &Path, slug: &str, display_name: &str) {
    let Some(gateway) = Gateway::from_env() else {
        println!("[eval] no LUCIDOS_EVAL_GATEWAY_URL, so '{slug}' stays unregistered");
        return;
    };
    // An existing registration is not ours to retag, including a user who
    // adopted the same directory. A failed observation never authorizes adopt.
    match gateway.registered_port_call(slug).await {
        Ok(Some(port)) => {
            println!("[eval] '{slug}' already registered on port {port}; protection unchanged");
            return;
        }
        Ok(None) => {}
        Err(error) => {
            println!("{}", unregistered(slug, &error.to_string()));
            return;
        }
    }
    match gateway.adopt_call(dir, slug, display_name).await {
        Ok(port) => println!("[eval] registered '{slug}' with the gateway on port {port}"),
        Err(error) => println!("{}", unregistered(slug, &error.to_string())),
    }
}

/// Read only the expected identity and credential; never put the response
/// body in an error, since successful adoption carries a deletion secret.
fn adoption<'a>(payload: &'a serde_json::Value, slug: &str) -> Fallible<(u16, &'a str)> {
    if payload["workspace"]["id"].as_str() != Some(slug) {
        return Err("gateway adoption returned a different workspace identity".into());
    }
    let port = payload["workspace"]["port"]
        .as_u64()
        .and_then(|port| u16::try_from(port).ok())
        .filter(|port| *port != 0)
        .ok_or("gateway adoption returned no valid port")?;
    let token = payload["owner_token"]
        .as_str()
        .filter(|token| !token.is_empty())
        .ok_or("gateway adoption returned no owner token; automatic cleanup unavailable")?;
    Ok((port, token))
}

/// Serialize credential generations across seed, run and cleanup processes.
/// The lock file is stable: never unlink or rename it when rotating secrets.
struct OwnerStore {
    _lock: std::fs::File,
    folder: std::path::PathBuf,
    slug: String,
}

impl OwnerStore {
    fn lock(dir: &Path, slug: &str) -> Fallible<Self> {
        if slug.is_empty()
            || !slug
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            || dir.file_name().and_then(|name| name.to_str()) != Some(slug)
        {
            return Err("workspace directory does not match its owner credential identity".into());
        }
        let folder = dir
            .parent()
            .ok_or("workspace has no parent")?
            .join(".cleanup-owners");
        std::fs::create_dir_all(&folder)?;
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(folder.join(format!("{slug}.lock")))?;
        // Fail promptly rather than blocking an async worker behind a process
        // which could be waiting for this runtime's network response.
        lock.try_lock()
            .map_err(|_| "workspace ownership is busy; retry this operation")?;
        Ok(Self {
            _lock: lock,
            folder,
            slug: slug.into(),
        })
    }

    fn path(&self) -> std::path::PathBuf {
        self.folder.join(format!("{}.json", self.slug))
    }

    /// Keep the secret outside the arm and never replace a generation.
    fn save(&self, dir: &Path, slug: &str, token: &str) -> Fallible<()> {
        use std::io::Write;
        let mut file = tempfile::NamedTempFile::new_in(&self.folder)?;
        serde_json::to_writer(
            &mut file,
            &serde_json::json!({
                "workspace_id": slug,
                "dir": dir,
                "owner_token": token,
            }),
        )?;
        file.flush()?;
        file.as_file().sync_all()?;
        file.persist_noclobber(self.path())?;
        std::fs::File::open(&self.folder)?.sync_all()?;
        Ok(())
    }
}

/// A run keeps its original cleanup authority even after a label is reused.
/// Do not derive this again from the current workspace on resume.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunOwner {
    schema: u32,
    run_id: String,
    workspace_id: String,
    dir: std::path::PathBuf,
    owner: Option<Owner>,
}

#[derive(PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Owner {
    workspace_id: String,
    dir: std::path::PathBuf,
    owner_token: String,
}

pub fn pin_run_owner(
    results: &Path,
    run: &str,
    dir: &Path,
    slug: &str,
    resuming: bool,
) -> Fallible<RunOwner> {
    use std::io::ErrorKind;
    if [run, slug].iter().any(|value| {
        value.is_empty()
            || !value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    }) || dir.file_name().and_then(|name| name.to_str()) != Some(slug)
    {
        return Err("invalid run ownership identity".into());
    }
    let folder = results.join(".cleanup-owners").join(run);
    let path = folder.join(format!("{slug}.json"));
    match std::fs::read(&path) {
        Ok(bytes) => {
            let pinned: RunOwner = serde_json::from_slice(&bytes)?;
            if pinned.schema != 1
                || pinned.run_id != run
                || pinned.workspace_id != slug
                || pinned.dir != dir
                || pinned.owner.as_ref().is_some_and(|owner| {
                    owner.workspace_id != slug || owner.dir != dir || owner.owner_token.is_empty()
                })
            {
                return Err("run ownership binding does not match this arm".into());
            }
            return Ok(pinned);
        }
        Err(error) if error.kind() == ErrorKind::NotFound && !resuming => {}
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return Err(
                "resumed run has no pinned ownership; refusing current workspace credentials"
                    .into(),
            );
        }
        Err(error) => return Err(error.into()),
    }
    let store = OwnerStore::lock(dir, slug)?;
    let current = store.path();
    let owner: Option<Owner> = match std::fs::read(current) {
        Ok(bytes) => Some(serde_json::from_slice(&bytes)?),
        Err(error) if error.kind() == ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    if owner.as_ref().is_some_and(|owner| {
        owner.workspace_id != slug || owner.dir != dir || owner.owner_token.is_empty()
    }) {
        return Err("workspace owner credential identity mismatch".into());
    }
    let pinned = RunOwner {
        schema: 1,
        run_id: run.into(),
        workspace_id: slug.into(),
        dir: dir.into(),
        owner,
    };
    std::fs::create_dir_all(&folder)?;
    let mut file = tempfile::NamedTempFile::new_in(&folder)?;
    serde_json::to_writer(file.as_file_mut(), &pinned)?;
    file.as_file().sync_all()?;
    file.persist_noclobber(path)?;
    std::fs::File::open(&folder)?.sync_all()?;
    std::fs::File::open(folder.parent().ok_or("binding has no parent")?)?.sync_all()?;
    std::fs::File::open(results)?.sync_all()?;
    Ok(pinned)
}

/// Publish an immutable retry checkpoint, refusing corruption or another identity.
fn cleanup_checkpoint(path: &Path, pinned: &RunOwner, publish: bool) -> Fallible<bool> {
    let identity = serde_json::to_vec(&(1, &pinned.run_id, &pinned.workspace_id, &pinned.dir))?;
    match std::fs::read(path) {
        Ok(bytes) if bytes == identity => return Ok(true),
        Ok(_) => return Err("cleanup checkpoint identity mismatch".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    if !publish {
        return Ok(false);
    }
    let parent = path.parent().ok_or("checkpoint has no parent")?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    use std::io::Write;
    file.write_all(&identity)?;
    file.as_file().sync_all()?;
    file.persist_noclobber(path)?;
    std::fs::File::open(parent)?.sync_all()?;
    Ok(true)
}

impl Gateway {
    async fn cleanup_call(&self, results: &Path, pinned: &RunOwner) -> Fallible<()> {
        let owner = pinned
            .owner
            .as_ref()
            .ok_or("arm has no pinned cleanup authority; retained")?;
        let store = OwnerStore::lock(&pinned.dir, &pinned.workspace_id)?;
        let folder = results.join(".cleanup-owners").join(&pinned.run_id);
        let requested = folder.join(format!("{}.requested.json", pinned.workspace_id));
        let succeeded = folder.join(format!("{}.succeeded.json", pinned.workspace_id));
        let current: Option<Owner> = match std::fs::read(store.path()) {
            Ok(bytes) => Some(serde_json::from_slice(&bytes)?),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        if !cleanup_checkpoint(&succeeded, pinned, false)? {
            if current.as_ref() != Some(owner) {
                return Err("cleanup owner generation changed; refusing deletion".into());
            }
            // Names may change; the pinned token remains the only deletion authority.
            let payload: serde_json::Value = self
                .request(self.client.get(self.control("workspaces")))
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;
            let list = payload["workspaces"]
                .as_array()
                .ok_or("gateway registry returned no workspace list")?;
            if list.iter().any(|ws| ws["id"].as_str().is_none()) {
                return Err("invalid registry identity; cleanup retained".into());
            }
            let ws = list
                .iter()
                .find(|ws| ws["id"].as_str() == Some(&pinned.workspace_id))
                .ok_or("cleanup registration absent; outcome unconfirmed, credential retained")?;
            let name = ws["name"]
                .as_str()
                .filter(|s| !s.is_empty())
                .ok_or("cleanup workspace has no name")?;
            cleanup_checkpoint(&requested, pinned, true)?;
            let response = self
                .request(
                    self.client
                        .delete(self.control(&format!("workspaces/{}", pinned.workspace_id))),
                )
                .json(&serde_json::json!({"confirm": name, "owner_token": owner.owner_token}))
                .send()
                .await?;
            if response.status() != reqwest::StatusCode::NO_CONTENT {
                // Never include a response body: it may contain credentials.
                return Err(format!(
                    "cleanup refused ({}); original credential retained",
                    response.status()
                )
                .into());
            }
            cleanup_checkpoint(&succeeded, pinned, true)?;
        }
        // The immutable run pin archives this generation. A successful old retry
        // must leave a later adoption's credential alone, even after a crash.
        if current.as_ref() == Some(owner) {
            std::fs::remove_file(store.path())?;
            std::fs::File::open(&store.folder)?.sync_all()?;
        }
        Ok(())
    }
}

/// Cleanup only with the original binding; never discover replacement authority.
pub async fn cleanup_arm(results: &Path, run: &str, dir: &Path, slug: &str) -> Fallible<()> {
    let pinned = pin_run_owner(results, run, dir, slug, true)?;
    if pinned.owner.is_none() {
        println!("[eval] '{slug}' has no pinned owner; workspace retained");
        return Ok(());
    }
    let gateway = Gateway::from_env().ok_or("cleanup gateway unavailable; workspace retained")?;
    gateway.cleanup_call(results, &pinned).await
}

/// Ask the gateway to stop any engine of its own on this arm's port.
///
/// Browsing an arm lazy-starts a gateway-owned engine, and nothing stops it
/// again. The next run would find its port taken, so its own engine would fail
/// to bind. `boot_engine` catches that, but a clear failure is still a failure:
/// releasing first is what lets a run follow a browse.
///
/// Best-effort like the rest of this module. The engine the harness is about to
/// start replaces whatever this stopped, and the gateway re-adopts it.
pub async fn release_arm(slug: &str) {
    let Some(gateway) = Gateway::from_env() else {
        return;
    };
    if let Err(error) = gateway.stop_call(slug).await {
        println!("[eval] could not ask the gateway to release '{slug}': {error}");
    }
}

/// The port the gateway already holds for `slug`, if any.
///
/// `seed` and `run` are separate invocations, so the run cannot remember what
/// the seed's adoption returned. Best-effort in the same way: an unregistered
/// arm, or no gateway at all, falls back to a free port.
pub async fn registered_port(slug: &str) -> Option<u16> {
    let gateway = Gateway::from_env()?;
    match gateway.registered_port_call(slug).await {
        Ok(port) => port,
        Err(error) => {
            println!("[eval] cannot read the gateway's registry: {error}");
            None
        }
    }
}

/// What one arm's failed registration prints.
///
/// Registration may have reached the gateway before credential persistence
/// failed. Do not claim the registry is unchanged in that case.
fn unregistered(slug: &str, reason: &str) -> String {
    format!(
        "[eval] '{slug}' gateway registration incomplete: {reason}. \
         The run continues; browsing and automatic cleanup may be unavailable."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn save_owner(dir: &Path, slug: &str, token: &str) -> Fallible<()> {
        OwnerStore::lock(dir, slug)?.save(dir, slug, token)
    }

    #[tokio::test]
    async fn cleanup_retires_only_successful_matching_generation() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        for delete_status in [204, 403, 404, 202, 0] {
            let root = tempfile::tempdir().unwrap();
            let ws = root.path().join("eval-test");
            let results = root.path().join("results");
            save_owner(&ws, "eval-test", "original-secret").unwrap();
            let pinned = pin_run_owner(&results, "run-one", &ws, "eval-test", false).unwrap();
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                for (method, response) in [
                    (
                        "GET",
                        r#"{"workspaces":[{"id":"eval-test","name":"Eval test"}]}"#,
                    ),
                    ("DELETE", "{}"),
                ] {
                    let (mut socket, _) = listener.accept().await.unwrap();
                    let mut bytes = Vec::new();
                    loop {
                        let mut chunk = [0; 4096];
                        let n = socket.read(&mut chunk).await.unwrap();
                        assert!(n > 0);
                        bytes.extend_from_slice(&chunk[..n]);
                        let Some(split) = bytes.windows(4).position(|p| p == b"\r\n\r\n") else {
                            continue;
                        };
                        let headers = String::from_utf8_lossy(&bytes[..split]);
                        let length: usize = headers
                            .lines()
                            .find_map(|line| {
                                line.to_lowercase()
                                    .strip_prefix("content-length: ")
                                    .map(str::to_owned)
                            })
                            .map(|s| s.parse().unwrap())
                            .unwrap_or(0);
                        if bytes.len() < split + 4 + length {
                            continue;
                        }
                        assert!(headers.starts_with(method));
                        assert!(headers.contains("x-lucidos-local-token: local-secret"));
                        if method == "DELETE" {
                            let body: serde_json::Value =
                                serde_json::from_slice(&bytes[split + 4..]).unwrap();
                            assert_eq!(body["owner_token"], "original-secret");
                            assert_eq!(body["confirm"], "Eval test");
                        }
                        break;
                    }
                    if method == "DELETE" && delete_status == 0 {
                        break;
                    }
                    let status = if method == "DELETE" {
                        delete_status
                    } else {
                        200
                    };
                    socket
                    .write_all(
                        format!(
                            "HTTP/1.1 {status} Status\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            response.len(),
                            response
                        )
                        .as_bytes(),
                    )
                    .await
                    .unwrap();
                }
            });
            let gateway = Gateway {
                base_url: format!("http://{address}"),
                client: reqwest::Client::builder().no_proxy().build().unwrap(),
                token: Some("local-secret".into()),
            };
            let outcome = gateway.cleanup_call(&results, &pinned).await;
            server.await.unwrap();
            if delete_status != 204 {
                assert!(outcome.is_err());
                assert!(OwnerStore::lock(&ws, "eval-test").unwrap().path().exists());
                assert!(results
                    .join(".cleanup-owners/run-one/eval-test.requested.json")
                    .exists());
                assert!(!results
                    .join(".cleanup-owners/run-one/eval-test.succeeded.json")
                    .exists());
                continue;
            }
            outcome.unwrap();
            assert!(!OwnerStore::lock(&ws, "eval-test").unwrap().path().exists());
            // A retry after label reuse must neither contact the server nor retire its credential.
            save_owner(&ws, "eval-test", "new-secret").unwrap();
            gateway.cleanup_call(&results, &pinned).await.unwrap();
            let current: Owner = serde_json::from_slice(
                &std::fs::read(OwnerStore::lock(&ws, "eval-test").unwrap().path()).unwrap(),
            )
            .unwrap();
            assert_eq!(current.owner_token, "new-secret");
        }
    }

    #[tokio::test]
    async fn cleanup_refuses_changed_missing_or_unowned_generations_before_network() {
        let root = tempfile::tempdir().unwrap();
        let ws = root.path().join("eval-test");
        let results = root.path().join("results");
        let gateway = Gateway {
            base_url: "http://127.0.0.1:1".into(),
            client: reqwest::Client::new(),
            token: None,
        };
        let absent = pin_run_owner(&results, "unowned", &ws, "eval-test", false).unwrap();
        let error = gateway
            .cleanup_call(&results, &absent)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("no pinned cleanup authority"));
        save_owner(&ws, "eval-test", "old-secret").unwrap();
        let old = pin_run_owner(&results, "owned", &ws, "eval-test", false).unwrap();
        let path = OwnerStore::lock(&ws, "eval-test").unwrap().path();
        std::fs::remove_file(&path).unwrap();
        let error = gateway
            .cleanup_call(&results, &old)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("generation changed"));
        save_owner(&ws, "eval-test", "new-secret").unwrap();
        let error = gateway
            .cleanup_call(&results, &old)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("generation changed"));
        assert!(!error.contains("secret"));
        assert!(path.exists());
    }

    #[test]
    fn cleanup_checkpoints_are_immutable_and_identity_bound() {
        let root = tempfile::tempdir().unwrap();
        let ws = root.path().join("eval-test");
        let results = root.path().join("results");
        let mut pinned = pin_run_owner(&results, "run", &ws, "eval-test", false).unwrap();
        let path = results.join(".cleanup-owners/run/eval-test.requested.json");
        assert!(!cleanup_checkpoint(&path, &pinned, false).unwrap());
        assert!(cleanup_checkpoint(&path, &pinned, true).unwrap());
        assert!(cleanup_checkpoint(&path, &pinned, true).unwrap());
        pinned.run_id = "another".into();
        assert!(cleanup_checkpoint(&path, &pinned, true).is_err());
        std::fs::write(&path, b"broken").unwrap();
        assert!(cleanup_checkpoint(&path, &pinned, false).is_err());
    }

    #[test]
    fn owner_store_serializes_each_slug_without_blocking_other_arms() {
        let root = tempfile::tempdir().unwrap();
        let ws = root.path().join("eval-test");
        let first = OwnerStore::lock(&ws, "eval-test").unwrap();
        assert!(OwnerStore::lock(&ws, "eval-test").is_err());
        assert!(OwnerStore::lock(&root.path().join("eval-other"), "eval-other").is_ok());
        drop(first);
        assert!(OwnerStore::lock(&ws, "eval-test").is_ok());
        assert!(OwnerStore::lock(&ws, "../escape").is_err());
    }

    #[tokio::test]
    async fn adoption_refuses_unarchived_credential_before_contacting_gateway() {
        let root = tempfile::tempdir().unwrap();
        let ws = root.path().join("eval-test");
        save_owner(&ws, "eval-test", "original-secret").unwrap();
        let gateway = Gateway {
            base_url: "http://127.0.0.1:1".into(),
            client: reqwest::Client::new(),
            token: None,
        };
        let error = gateway
            .adopt_call(&ws, "eval-test", "test")
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("unarchived owner credential"), "{error}");
        assert!(!error.contains("original-secret"));
    }

    #[test]
    fn run_ownership_is_pinned_before_driving_and_never_refreshes() {
        let root = tempfile::tempdir().unwrap();
        let ws = root.path().join("eval-test");
        let results = root.path().join("results");
        save_owner(&ws, "eval-test", "original-secret").unwrap();
        pin_run_owner(&results, "run-one", &ws, "eval-test", false).unwrap();
        let owner = root.path().join(".cleanup-owners/eval-test.json");
        let newer = std::fs::read_to_string(&owner)
            .unwrap()
            .replace("original-secret", "new-secret");
        std::fs::write(&owner, newer).unwrap();
        let pinned = pin_run_owner(&results, "run-one", &ws, "eval-test", true).unwrap();
        assert_eq!(pinned.owner.unwrap().owner_token, "original-secret");
        assert!(pin_run_owner(&results, "legacy-run", &ws, "eval-test", true).is_err());
        assert!(pin_run_owner(&results, "../escape", &ws, "eval-test", false).is_err());
    }

    #[test]
    fn absent_ownership_and_corrupt_pins_never_gain_current_authority() {
        let root = tempfile::tempdir().unwrap();
        let ws = root.path().join("eval-test");
        let results = root.path().join("results");
        assert!(pin_run_owner(&results, "run-one", &ws, "eval-test", false)
            .unwrap()
            .owner
            .is_none());
        save_owner(&ws, "eval-test", "later-secret").unwrap();
        assert!(pin_run_owner(&results, "run-one", &ws, "eval-test", true)
            .unwrap()
            .owner
            .is_none());
        let path = results.join(".cleanup-owners/run-one/eval-test.json");
        std::fs::write(&path, b"{}").unwrap();
        assert!(pin_run_owner(&results, "run-one", &ws, "eval-test", true).is_err());
    }

    #[tokio::test]
    async fn seed_requires_an_authoritative_absent_registration() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        for (status, body, allowed) in [
            ("200 OK", r#"{"workspaces":[]}"#, true),
            (
                "200 OK",
                r#"{"workspaces":[{"id":"eval-test","port":5300,"origin":"user"}]}"#,
                false,
            ),
            ("200 OK", r#"{"workspaces":[{"id":"eval-test"}]}"#, false),
            ("200 OK", r#"{"workspaces":[{}]}"#, false),
            ("200 OK", r#"{}"#, false),
            ("503 Service Unavailable", r#"{}"#, false),
        ] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                while !request.windows(4).any(|part| part == b"\r\n\r\n") {
                    let mut chunk = [0; 4096];
                    let count = socket.read(&mut chunk).await.unwrap();
                    assert!(count > 0);
                    request.extend_from_slice(&chunk[..count]);
                }
                assert!(String::from_utf8_lossy(&request)
                    .starts_with("GET /~/api/v1/control/workspaces "));
                socket.write_all(format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
            });
            let gateway = Gateway {
                base_url: format!("http://{address}"),
                client: reqwest::Client::builder()
                    .no_proxy()
                    .timeout(TIMEOUT)
                    .build()
                    .unwrap(),
                token: None,
            };
            assert_eq!(gateway.check_seed_call("eval-test").await.is_ok(), allowed);
            server.await.unwrap();
        }
    }

    #[tokio::test]
    async fn new_adoption_sends_provenance_and_persists_returned_ownership() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            let body = loop {
                let mut chunk = [0; 4096];
                let count = socket.read(&mut chunk).await.unwrap();
                assert!(count > 0);
                bytes.extend_from_slice(&chunk[..count]);
                let Some(split) = bytes.windows(4).position(|part| part == b"\r\n\r\n") else {
                    continue;
                };
                let headers = String::from_utf8_lossy(&bytes[..split]).to_lowercase();
                assert!(headers.starts_with("post /~/api/v1/control/workspaces/adopt "));
                assert!(headers.contains("x-lucidos-local-token: test-local-token"));
                let length: usize = headers
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length: "))
                    .unwrap()
                    .parse()
                    .unwrap();
                if bytes.len() >= split + 4 + length {
                    break serde_json::from_slice::<serde_json::Value>(&bytes[split + 4..])
                        .unwrap();
                }
            };
            assert_eq!(body["origin"], "eval");
            assert_eq!(body["ephemeral"], true);
            assert_eq!(body["autostart"], false);
            let response = serde_json::json!({
                "workspace": {"id": "eval-test", "port": 5300},
                "owner_token": "test-only-secret"
            })
            .to_string();
            socket.write_all(format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                response.len(), response
            ).as_bytes()).await.unwrap();
        });
        let gateway = Gateway {
            base_url: format!("http://{address}"),
            client: reqwest::Client::builder()
                .no_proxy()
                .timeout(TIMEOUT)
                .build()
                .unwrap(),
            token: Some("test-local-token".into()),
        };
        let root = tempfile::tempdir().unwrap();
        let ws = root.path().join("eval-test");
        std::fs::create_dir(&ws).unwrap();
        assert_eq!(
            gateway
                .adopt_call(&ws, "eval-test", "Eval test")
                .await
                .unwrap(),
            5300
        );
        tokio::time::timeout(TIMEOUT, server)
            .await
            .unwrap()
            .unwrap();
        let saved: serde_json::Value = serde_json::from_slice(
            &std::fs::read(root.path().join(".cleanup-owners/eval-test.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(saved["workspace_id"], "eval-test");
        assert_eq!(saved["owner_token"], "test-only-secret");
    }

    #[test]
    fn adoption_requires_matching_identity_and_a_scoped_secret() {
        let valid = serde_json::json!({
            "workspace": {"id": "eval-test", "port": 5300},
            "owner_token": "test-only-secret"
        });
        assert!(adoption(&valid, "eval-test").is_ok());
        assert!(adoption(&valid, "another-workspace").is_err());
        let mut missing = valid.clone();
        missing["owner_token"] = serde_json::Value::Null;
        assert!(adoption(&missing, "eval-test").is_err());
        missing["owner_token"] = "".into();
        assert!(adoption(&missing, "eval-test").is_err());
        let error = adoption(&valid, "another-workspace")
            .err()
            .unwrap()
            .to_string();
        assert!(!error.contains("test-only-secret"));
    }

    #[test]
    fn ownership_survives_workspace_removal_and_cannot_be_overwritten() {
        let root = tempfile::tempdir().unwrap();
        let ws = root.path().join("eval-test");
        std::fs::create_dir(&ws).unwrap();
        save_owner(&ws, "eval-test", "test-only-secret").unwrap();
        let saved = root.path().join(".cleanup-owners/eval-test.json");
        let contents = std::fs::read(&saved).unwrap();
        assert!(save_owner(&ws, "eval-test", "replacement").is_err());
        std::fs::remove_dir(&ws).unwrap();
        assert_eq!(std::fs::read(&saved).unwrap(), contents);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(saved).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn a_failed_registration_says_the_run_is_unaffected() {
        // The reader is mid-seed and has to decide whether to stop. Naming the
        // cause without saying what it costs is what makes them stop.
        let line = unregistered("eval-lean-1", "error sending request");
        assert!(line.contains("eval-lean-1"), "{line}");
        assert!(line.contains("error sending request"), "{line}");
        assert!(line.contains("The run continues"), "{line}");
        assert!(
            line.contains("automatic cleanup may be unavailable"),
            "{line}"
        );
    }

    #[test]
    fn a_gateway_needs_a_url_and_tolerates_a_trailing_slash() {
        // Every call is built off `base_url`, so a pasted trailing slash would
        // otherwise produce `//~/api/v1/...`, which the gateway does not route.
        temporarily(Some("https://localhost:5251/"), || {
            let gateway = Gateway::from_env().expect("a url is all it needs");
            assert_eq!(
                gateway.control("workspaces/adopt"),
                "https://localhost:5251/~/api/v1/control/workspaces/adopt"
            );
        });
        for unusable in [None, Some(""), Some("   ")] {
            temporarily(unusable, || {
                assert!(Gateway::from_env().is_none(), "{unusable:?}");
            });
        }
    }

    /// Run `body` with the gateway URL set, then put it back.
    ///
    /// Tests share a process, so a variable left behind reaches the next one.
    /// Serialized on a mutex for the same reason.
    fn temporarily(value: Option<&str>, body: impl FnOnce()) {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        const KEY: &str = "LUCIDOS_EVAL_GATEWAY_URL";
        let guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let before = std::env::var(KEY).ok();
        let set = |v: Option<&str>| match v {
            Some(v) => std::env::set_var(KEY, v),
            None => std::env::remove_var(KEY),
        };
        set(value);
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(body));
        set(before.as_deref());
        drop(guard);
        if let Err(panic) = outcome {
            std::panic::resume_unwind(panic);
        }
    }
}
