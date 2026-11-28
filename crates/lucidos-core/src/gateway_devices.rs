//! Device identity, read from the workspace gateway (ADR 0825).
//!
//! The gateway is the one source of a device's typed name and pairing label.
//! The engine caches the gateway's list in memory. It reads the list again
//! when the gateway signals an identity change, and never stores a name.

use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock, RwLock};
use std::time::Duration;

/// INVARIANT: The gateway routes every engine reads, under its auth mount.
const DEVICE_NAMES_PATH: &str = "/~/api/v1/auth/device-names";
const IMPORT_PATH: &str = "/~/api/v1/auth/device-names/import";

const GATEWAY_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_RETRY_WAIT: Duration = Duration::from_secs(60);

static GATEWAY_PORT: OnceLock<String> = OnceLock::new();
static ANSWERED: AtomicBool = AtomicBool::new(false);
static FIRST_ANSWER: tokio::sync::Notify = tokio::sync::Notify::const_new();

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceIdentity {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceIdentities {
    #[serde(default)]
    pub devices: Vec<DeviceIdentity>,
}

impl DeviceIdentities {
    pub fn of(&self, device_id: &str) -> Option<&DeviceIdentity> {
        self.devices.iter().find(|d| d.id == device_id)
    }

    pub fn name_of(&self, device_id: &str) -> Option<&str> {
        self.of(device_id).and_then(|d| d.name.as_deref())
    }

    pub fn label_of(&self, device_id: &str) -> Option<&str> {
        self.of(device_id).and_then(|d| d.label.as_deref())
    }

    /// INVARIANT: Devices the gateway names or labels. A one-off device is never one.
    pub fn identified_ids(&self) -> Vec<String> {
        self.devices
            .iter()
            .filter(|d| d.name.is_some() || d.label.is_some())
            .map(|d| d.id.clone())
            .collect()
    }

    /// INVARIANT: The devices whose typed name differs between the two lists. A label
    /// change, or a new device with no name, is no rename. A named device that
    /// left the list reads as a cleared name.
    pub fn renamed_since(&self, earlier: &Self) -> Vec<DeviceIdentity> {
        let ids: BTreeSet<&String> = earlier
            .devices
            .iter()
            .chain(&self.devices)
            .map(|d| &d.id)
            .collect();
        ids.into_iter()
            .filter(|id| earlier.name_of(id) != self.name_of(id))
            .map(|id| {
                self.of(id).cloned().unwrap_or_else(|| DeviceIdentity {
                    id: id.clone(),
                    ..DeviceIdentity::default()
                })
            })
            .collect()
    }
}

#[cfg(not(test))]
fn slot() -> &'static RwLock<Arc<DeviceIdentities>> {
    static SLOT: OnceLock<RwLock<Arc<DeviceIdentities>>> = OnceLock::new();
    SLOT.get_or_init(RwLock::default)
}

/// INVARIANT: This crate's own tests reuse device ids, so each test thread holds
/// its own list. Tokio tests run on one thread unless they ask for more.
#[cfg(test)]
fn slot() -> &'static RwLock<Arc<DeviceIdentities>> {
    thread_local! {
        static SLOT: &'static RwLock<Arc<DeviceIdentities>> = Box::leak(Box::default());
    }
    SLOT.with(|slot| *slot)
}

pub fn current() -> Arc<DeviceIdentities> {
    slot().read().map(|s| Arc::clone(&s)).unwrap_or_default()
}

/// INVARIANT: The list, or `None` while the gateway has not answered yet. A
/// decision that must not mistake "not loaded" for "unnamed" waits for it.
/// An engine with no gateway knows its list: it is empty.
pub fn known() -> Option<Arc<DeviceIdentities>> {
    let waiting = GATEWAY_PORT.get().is_some() && !ANSWERED.load(Ordering::SeqCst);
    (!waiting).then(current)
}

fn install(identities: DeviceIdentities) -> Arc<DeviceIdentities> {
    let identities = Arc::new(identities);
    if let Ok(mut held) = slot().write() {
        *held = Arc::clone(&identities);
    }
    if !ANSWERED.swap(true, Ordering::SeqCst) {
        FIRST_ANSWER.notify_waiters();
    }
    identities
}

/// INVARIANT: The named devices, once a gateway that was late with its first
/// list answers. Readers showed fallback names meanwhile, so each name is one
/// they must hear. Call it only when [`known`] was `None` as serving began.
pub async fn late_first_answer() -> Vec<DeviceIdentity> {
    loop {
        let answered = FIRST_ANSWER.notified();
        if let Some(identities) = known() {
            return identities.renamed_since(&DeviceIdentities::default());
        }
        answered.await;
    }
}

/// INVARIANT: Hand the held names to the gateway, then read its list, retrying
/// both until the gateway answers. Without a gateway it does nothing, and the
/// held names wait for a boot that has one.
pub fn enable(pool: PgPool) {
    let Ok(port) = std::env::var("LUCIDOS_GATEWAY_PORT") else {
        return;
    };
    let _ = GATEWAY_PORT.set(port);
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        return;
    };
    handle.spawn(async move {
        retry("hand over the held device names", || import_held(&pool)).await;
        retry("read device names", refresh_once).await;
        crate::core::devices::DeviceStore::carry_names_over(&pool).await;
    });
}

async fn retry<T, F, Fut>(what: &str, mut attempt: F) -> T
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, String>>,
{
    let mut wait = Duration::from_secs(1);
    loop {
        match attempt().await {
            Ok(value) => return value,
            Err(e) => log_legacy!(
                "[Devices] could not {what} through the gateway, retrying in {}s: {e}",
                wait.as_secs()
            ),
        }
        tokio::time::sleep(wait).await;
        wait = (wait * 2).min(MAX_RETRY_WAIT);
    }
}

/// INVARIANT: Read the gateway's list again. Returns the devices whose name
/// changed, so the caller can announce each one. Before the first answer
/// there is no earlier list to compare, so nothing reads as renamed.
pub async fn refresh() -> Vec<DeviceIdentity> {
    if GATEWAY_PORT.get().is_none() {
        return Vec::new();
    }
    let earlier = known();
    let now = retry("read device names", refresh_once).await;
    earlier.map_or_else(Vec::new, |earlier| now.renamed_since(&earlier))
}

/// INVARIANT: The list, asking the gateway once when it has not answered yet.
/// `Err` means the list is still unknown, never that a device is unnamed.
pub async fn load() -> Result<Arc<DeviceIdentities>, String> {
    match known() {
        Some(identities) => Ok(identities),
        None => refresh_once().await,
    }
}

async fn refresh_once() -> Result<Arc<DeviceIdentities>, String> {
    let response = client()?
        .get(format!("{}{DEVICE_NAMES_PATH}", origin()?))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let identities = match response.status() {
        reqwest::StatusCode::NOT_FOUND => DeviceIdentities::default(),
        status if status.is_success() => response.json().await.map_err(|e| e.to_string())?,
        status => return Err(format!("HTTP {status}")),
    };
    Ok(install(identities))
}

/// INVARIANT: Name a device on the gateway, so every workspace shows the same name.
/// A blank name clears it. The answer replaces the cached list at once, so
/// the gateway's later signal finds nothing new to announce here.
pub async fn rename(device_id: &str, name: Option<&str>) -> Result<(), String> {
    #[cfg(any(test, feature = "test-support"))]
    if GATEWAY_PORT.get().is_none() {
        name_for_test(device_id, name);
        return Ok(());
    }
    let response = client()?
        .put(format!(
            "{}/~/api/v1/auth/devices/{}/name",
            origin()?,
            urlencoding::encode(device_id)
        ))
        .json(&serde_json::json!({ "name": name }))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        let status = response.status();
        let detail = response.text().await.unwrap_or_default();
        return Err(format!(
            "the gateway refused the rename ({status}): {detail}"
        ));
    }
    install(response.json().await.map_err(|e| e.to_string())?);
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, sqlx::FromRow)]
pub struct HeldDeviceName {
    pub device_id: String,
    pub name: Option<String>,
    pub pairing_label: Option<String>,
}

pub async fn held_names(pool: &PgPool) -> Result<Vec<HeldDeviceName>, sqlx::Error> {
    sqlx::query_as(
        "SELECT device_id, name, pairing_label FROM device_name_imports ORDER BY device_id",
    )
    .fetch_all(pool)
    .await
}

/// INVARIANT: Send the names the migration held back to the gateway. Delete each
/// row only once the gateway has accepted the batch. Returns the count sent.
pub async fn import_held(pool: &PgPool) -> Result<usize, String> {
    let held = held_names(pool).await.map_err(|e| e.to_string())?;
    if held.is_empty() {
        return Ok(0);
    }
    let response = client()?
        .post(format!("{}{IMPORT_PATH}", origin()?))
        .json(&serde_json::json!({ "devices": held }))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!("HTTP {}", response.status()));
    }
    let ids: Vec<&str> = held.iter().map(|h| h.device_id.as_str()).collect();
    sqlx::query("DELETE FROM device_name_imports WHERE device_id = ANY($1)")
        .bind(&ids)
        .execute(pool)
        .await
        .map_err(|e| e.to_string())?;
    log_legacy!(
        "[Devices] handed {} device name(s) to the gateway",
        held.len()
    );
    Ok(held.len())
}

fn origin() -> Result<String, String> {
    GATEWAY_PORT
        .get()
        .map(lucidos_local_token::client::origin)
        .ok_or_else(|| "this engine runs without a gateway".to_string())
}

fn client() -> Result<reqwest::Client, String> {
    crate::gateway_auth::client_builder()
        .timeout(GATEWAY_TIMEOUT)
        .build()
        .map_err(|e| format!("could not build a gateway client: {e}"))
}

/// INVARIANT: Tests share one process-wide list. A seed adds or replaces one
/// device and leaves every other test's devices alone.
#[cfg(any(test, feature = "test-support"))]
pub fn seed_for_test(device_id: &str, name: Option<&str>, label: Option<&str>) {
    let mut held = slot().write().expect("the device list lock");
    let mut identities = (**held).clone();
    identities.devices.retain(|d| d.id != device_id);
    identities.devices.push(DeviceIdentity {
        id: device_id.to_string(),
        name: name.map(str::to_string),
        label: label.map(str::to_string),
    });
    *held = Arc::new(identities);
}

/// INVARIANT: A test engine has no gateway, so a rename lands in the shared
/// list, keeping the device's label.
#[cfg(any(test, feature = "test-support"))]
fn name_for_test(device_id: &str, name: Option<&str>) {
    let label = current().label_of(device_id).map(str::to_string);
    let name = name.map(str::trim).filter(|n| !n.is_empty());
    seed_for_test(device_id, name, label.as_deref());
}

#[cfg(test)]
#[path = "gateway_devices_tests.rs"]
mod tests;
