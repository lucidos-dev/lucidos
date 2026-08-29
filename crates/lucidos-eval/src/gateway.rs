//! Register each arm with the workspace gateway, and read back its port.
//!
//! Two calls into `/~/api/v1/control`, and neither is allowed to fail a run.
//! The eval seeds workspaces and drives fourteen threads against them; whether a
//! dev gateway happens to be up is nothing to do with the measurement. Every
//! outcome here is therefore a printed line and a `None`. The caller falls back
//! to the free port it used before any of this existed.
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
    async fn adopt_call(&self, dir: &Path, name: &str) -> Fallible<u16> {
        let body = serde_json::json!({
            "dir": dir,
            "name": name,
            // The arms are a measurement, not a service. Nothing about them
            // should come up on a gateway boot.
            "autostart": false,
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
        payload["workspace"]["port"]
            .as_u64()
            .and_then(|p| u16::try_from(p).ok())
            .ok_or_else(|| format!("the gateway's answer carried no port: {payload}").into())
    }

    /// Stop the gateway's own engine for `slug`, keeping the registry entry.
    ///
    /// 202 when it stopped one, and a 400 for a slug the gateway does not know.
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
        Ok(payload["workspaces"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|ws| ws["id"].as_str() == Some(slug))
            .and_then(|ws| ws["port"].as_u64())
            .and_then(|p| u16::try_from(p).ok()))
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
    match gateway.adopt_call(dir, display_name).await {
        Ok(port) => println!("[eval] registered '{slug}' with the gateway on port {port}"),
        Err(error) => println!("{}", unregistered(slug, &error.to_string())),
    }
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
/// One line, saying what went wrong and what it costs. It costs browsing, and
/// nothing else: the run is unaffected, which is the part a reader mid-seed
/// needs to know before deciding whether to stop it.
fn unregistered(slug: &str, reason: &str) -> String {
    format!(
        "[eval] '{slug}' is not registered with the gateway: {reason}. \
         The run continues on a free port, and the arm is not browsable."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_registration_says_the_run_is_unaffected() {
        // The reader is mid-seed and has to decide whether to stop. Naming the
        // cause without saying what it costs is what makes them stop.
        let line = unregistered("eval-lean-1", "error sending request");
        assert!(line.contains("eval-lean-1"), "{line}");
        assert!(line.contains("error sending request"), "{line}");
        assert!(line.contains("The run continues"), "{line}");
        assert!(line.contains("not browsable"), "{line}");
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
