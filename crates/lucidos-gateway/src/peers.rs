use crate::registry::{self, Registry};
use lucidos_installs::{Install, Inventory};
use std::path::Path;
use std::time::Duration;

const REGISTRY_REL: &str = "config/workspaces.json";

pub(crate) use lucidos_gateway_constants::GATEWAY_HEALTH_PATH;

pub(crate) const GATEWAY_ROLE: &str = "gateway";

const PROBE_TIMEOUT: Duration = Duration::from_millis(1500);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerWorkspace {
    pub install: String,
    pub gateway_port: u16,
    pub slug: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Located {
    Nowhere,
    One(PeerWorkspace),
    Ambiguous(Vec<PeerWorkspace>),
}

pub fn locate(inventory: &Inventory, own_gateway_port: u16, name: &str) -> Located {
    let mut hits: Vec<PeerWorkspace> = peer_installs(inventory, own_gateway_port)
        .filter_map(|install| workspace_in(install, name))
        .collect();
    match hits.len() {
        0 => Located::Nowhere,
        1 => Located::One(hits.remove(0)),
        _ => Located::Ambiguous(hits),
    }
}

pub fn peer_gateway_ports(inventory: &Inventory, own_gateway_port: u16) -> Vec<u16> {
    peer_installs(inventory, own_gateway_port)
        .filter_map(|install| install.port)
        .collect()
}

fn peer_installs(inventory: &Inventory, own_gateway_port: u16) -> impl Iterator<Item = &Install> {
    let mut seen: Vec<&Path> = Vec::new();
    inventory.installs.iter().filter(move |install| {
        if install.running_here || install.port == Some(own_gateway_port) {
            return false;
        }
        let (Some(port), Some(data_dir)) = (install.port, install.data_dir.as_deref()) else {
            return false;
        };
        if seen.contains(&data_dir) {
            return false;
        }
        seen.push(data_dir);
        !port_is_contended(inventory, port)
    })
}

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

fn workspace_in(install: &Install, name: &str) -> Option<PeerWorkspace> {
    let gateway_port = install.port?;
    let data_dir = install.data_dir.as_deref()?;
    let registry = Registry::load(&data_dir.join(REGISTRY_REL)).ok()?;
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

pub async fn answers_as_gateway(client: &reqwest::Client, port: u16) -> bool {
    let origin = lucidos_local_token::client::origin(port);
    let url = format!("{origin}{GATEWAY_HEALTH_PATH}");
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

pub async fn hand_off_oauth_callback(
    client: &reqwest::Client,
    port: u16,
    query: &str,
) -> Option<(u16, String)> {
    if !answers_as_gateway(client, port).await {
        return None;
    }
    let origin = lucidos_local_token::client::origin(port);
    let handoff = crate::oauth_callback::HANDOFF_PATH;
    let url = format!("{origin}{handoff}?{query}");
    let res = client
        .get(&url)
        .timeout(crate::oauth_callback::HANDOFF_TIMEOUT)
        .send()
        .await
        .ok()?;
    if !res
        .headers()
        .contains_key(crate::oauth_callback::HANDOFF_ANSWERED_HEADER)
    {
        return None;
    }
    let status = res.status().as_u16();
    let page = res.text().await.ok()?;
    Some((status, page))
}

pub fn probe_client() -> reqwest::Client {
    lucidos_local_token::client::client_builder()
        .pool_max_idle_per_host(0)
        .connect_timeout(PROBE_TIMEOUT)
        .build()
        .expect("failed to build the peer-gateway probe client")
}

#[cfg(test)]
#[path = "peers_tests.rs"]
mod tests;
