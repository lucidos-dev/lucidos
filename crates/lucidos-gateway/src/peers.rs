//! Where a workspace lives when another install on this machine owns it.
//!
//! A thread link is workspace-qualified (`thread:<workspace>/<uuid>`), and the
//! client resolves the name against the gateway serving the page. A machine can
//! carry several *install vehicles*, each with its own gateway, port and
//! registry (ADR 0189). So a link into the OTHER install resolved to nothing,
//! and said the workspace was unavailable while it ran one port away.
//!
//! **A location crosses, and nothing else.** No workspace data is read, no
//! credential is forwarded, and nothing is proxied. The client navigates to the
//! peer gateway's own origin, which authenticates the browser itself. ADR 0132
//! refused to widen a pairing minted here into authority over another install's
//! workspaces, and a proxy hop would do that for every link.
//!
//! **The scheme is measured.** The packaged gateway serves plain http and a dev
//! checkout serves https, and TLS comes from launch environment recorded nowhere
//! on disk. Asking the peer also means no peer-side code is needed, so this
//! reaches a gateway many releases old (ADR 0105).
//!
//! Plan: `docs/plans/2026-09-16-cross-gateway-thread-links.md`.

use crate::net_config::{SCHEME_HTTP, SCHEME_HTTPS};
use crate::registry::{self, Registry};
use lucidos_installs::{Install, Inventory};
use std::path::Path;
use std::time::Duration;

/// Where every gateway keeps its registry, relative to its data dir.
const REGISTRY_REL: &str = "config/workspaces.json";

/// The only path this module ever asks a peer for. Public
/// (`auth_api::is_public_path`), so the probe presents no credential.
const PEER_HEALTH_PATH: &str = "/~/api/v1/health";

/// What `/~/api/v1/health` calls a gateway, as opposed to an engine.
const GATEWAY_ROLE: &str = "gateway";

/// How long one probe may take. Two schemes are tried, so this is half the
/// worst case a user waits on their click.
const PROBE_TIMEOUT: Duration = Duration::from_millis(1500);

/// A workspace found on an install that is not ours.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerWorkspace {
    /// The install carrying it, named as the install inventory names it.
    pub install: String,
    /// The gateway port that install is configured for.
    pub gateway_port: u16,
    /// The slug that gateway routes it under (`/<slug>/`).
    pub slug: String,
}

/// What a lookup by name found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Located {
    /// No other install on this machine carries it.
    Nowhere,
    /// Exactly one does.
    One(PeerWorkspace),
    /// Two or more do. Naming one would be a guess.
    Ambiguous(Vec<PeerWorkspace>),
}

/// Find the workspace called `name` on an install that is not this one.
///
/// Answers one question and never enumerates: a caller learns where a name they
/// already hold lives, and nothing about the peer's other workspaces.
///
/// `name` is matched against parsed registry contents. It is never joined into
/// a path, so a name shaped like a traversal reads as an ordinary miss.
///
/// `own_gateway_port` is this gateway's own. An install configured for it is
/// skipped even when it belongs to somebody else. Under *port contention* only
/// one of them binds it (ADR 0189). If that one is us, the peer's URL would
/// land here, on a slug we do not serve. If it is two other installs, the URL
/// is a guess either way, so [`port_is_contended`] drops the hit.
pub fn locate(inventory: &Inventory, own_gateway_port: u16, name: &str) -> Located {
    let mut hits: Vec<PeerWorkspace> = Vec::new();
    let mut seen: Vec<&Path> = Vec::new();
    for install in &inventory.installs {
        // Our own install is what the caller already searched, through the live
        // control listing. Offering it back would hand the page a URL to here.
        if install.running_here || install.port == Some(own_gateway_port) {
            continue;
        }
        let (Some(port), Some(data_dir)) = (install.port, install.data_dir.as_deref()) else {
            continue;
        };
        // Two bundles in two application directories share one data dir, so
        // they report the same workspace twice. That is one install found
        // twice rather than an ambiguity, and the two resolve identically. The
        // data dir is what makes them one, so it is what the dedupe keys on.
        if seen.contains(&data_dir) {
            continue;
        }
        if port_is_contended(inventory, port) {
            continue;
        }
        let Some(peer) = workspace_in(install, name) else {
            continue;
        };
        seen.push(data_dir);
        hits.push(peer);
    }
    match hits.len() {
        0 => Located::Nowhere,
        1 => Located::One(hits.remove(0)),
        _ => Located::Ambiguous(hits),
    }
}

/// Is more than one install configured for `port`?
///
/// A contended port addresses nobody. Only one install bound it, and nothing
/// on disk says which, so the URL may reach a stranger's picker under the
/// matched install's name. Dropping the hit says "cannot tell", which is the
/// answer `Inventory::serving_port` gives for the same question over rows.
///
/// Counted over distinct data dirs rather than over rows. Two bundles in two
/// application directories share one data dir and one port. That is one
/// install found twice, and a row count reads it as a contention.
fn port_is_contended(inventory: &Inventory, port: u16) -> bool {
    let mut dirs: Vec<Option<&Path>> = inventory
        .installs
        .iter()
        .filter(|install| install.port == Some(port))
        .map(|install| install.data_dir.as_deref())
        .collect();
    dirs.sort_unstable();
    dirs.dedup();
    dirs.len() > 1
}

/// The workspace called `name` in one install's registry, if it holds one.
///
/// An install with no recorded port or data dir is skipped rather than guessed
/// at: a location we cannot address is not a location.
fn workspace_in(install: &Install, name: &str) -> Option<PeerWorkspace> {
    let gateway_port = install.port?;
    let data_dir = install.data_dir.as_deref()?;
    let registry = Registry::load(&data_dir.join(REGISTRY_REL)).ok()?;
    // Display name first, then the slug that name would produce. The same two
    // steps, in the same order, as the client's own gateway lookup.
    let slug = registry
        .find_by_display_name(name, None)
        .or_else(|| registry.get(&registry::slugify(name)))?
        .id
        .clone();
    Some(PeerWorkspace {
        install: install.name.clone(),
        gateway_port,
        slug,
    })
}

/// Which scheme a gateway on `port` answered on, or `None` when none did.
///
/// Loopback is the right address to ask whatever the machine's bind says. Every
/// `BindChoice` reaches it: `Loopback` is it, `All` contains it, and an explicit
/// `Address` binds loopback beside itself (`net_config::bind_socket_addrs`).
///
/// `None` means the install is registered but its gateway is down, which the
/// caller reports rather than fixing. Another install's launch agent is not
/// ours to run.
pub async fn probe_scheme(client: &reqwest::Client, port: u16) -> Option<&'static str> {
    for scheme in [SCHEME_HTTPS, SCHEME_HTTP] {
        if answered_as_gateway(client, scheme, port).await {
            return Some(scheme);
        }
    }
    None
}

/// Did a Lucidos gateway answer here? The port alone proves nothing: it is a
/// well-known number and anything may hold it.
async fn answered_as_gateway(client: &reqwest::Client, scheme: &str, port: u16) -> bool {
    let url = format!("{scheme}://127.0.0.1:{port}{PEER_HEALTH_PATH}");
    let Ok(res) = client.get(&url).timeout(PROBE_TIMEOUT).send().await else {
        return false;
    };
    let Ok(bytes) = res.bytes().await else {
        return false;
    };
    let Ok(body) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return false;
    };
    body.get("role").and_then(|v| v.as_str()) == Some(GATEWAY_ROLE)
}

/// The client the probe uses.
///
/// Deliberately not the pooled engine client. That one carries SSE, so it holds
/// no overall timeout, and its pool is for a destination we own. This one speaks
/// to another install once per click and keeps nothing open afterwards.
///
/// Invalid certs are accepted because a dev gateway serves its own. The probe
/// reads one field off a public route, so a loopback impostor learns nothing
/// and gives nothing away.
pub fn probe_client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .danger_accept_invalid_certs(true)
        .pool_max_idle_per_host(0)
        .connect_timeout(PROBE_TIMEOUT)
        .build()
        .expect("failed to build the peer-gateway probe client")
}

#[cfg(test)]
#[path = "peers_tests.rs"]
mod tests;
