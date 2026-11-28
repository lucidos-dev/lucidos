use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::auth::{self, PairedDevice, PairedDevices};

pub const LOG_FILE: &str = "gateway-events.jsonl";

const MIGRATED_SUFFIX: &str = "migrated";

pub const FIRST_USER_NAME: &str = "Owner";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum GatewayEvent {
    DevicePaired {
        device_id: String,
        label: String,
        credential_digest: String,
        user_id: String,
    },
    DeviceSeen {
        device_id: String,
    },
    DeviceRevoked {
        device_id: String,
    },
    DeviceMoved {
        device_id: String,
        user_id: String,
    },
    UserAdded {
        user_id: String,
        name: String,
    },
    UserRenamed {
        user_id: String,
        name: String,
    },
    UserRemoved {
        user_id: String,
    },
    UserEmailAdded {
        user_id: String,
        email: String,
    },
    UserEmailRemoved {
        user_id: String,
        email: String,
    },
    RelayedUserSeen {
        device_id: String,
        user_id: String,
        name: String,
        via: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        emails: Vec<String>,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        label: String,
    },
    RelayedPairingForgotten {
        pairing_id: String,
    },
    DeviceNamed {
        device_id: String,
        name: Option<String>,
    },
}

impl GatewayEvent {
    pub fn changes_identity(&self) -> bool {
        !matches!(self, Self::DeviceSeen { .. })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoggedEvent {
    pub at: String,
    pub event: GatewayEvent,
}

impl LoggedEvent {
    pub fn now(event: GatewayEvent) -> Self {
        Self {
            at: chrono::Utc::now().to_rfc3339(),
            event,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct User {
    pub id: String,
    pub name: String,
    pub emails: Vec<String>,
    pub added_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RelayedDevice {
    pub device_id: String,
    pub user_id: String,
    pub name: String,
    pub via: String,
    #[serde(default)]
    pub emails: Vec<String>,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceIdentity {
    pub id: String,
    pub name: Option<String>,
    pub label: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ImportedDeviceName {
    pub device_id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub pairing_label: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Directory {
    pub devices: PairedDevices,
    pub users: Vec<User>,
    pub memberships: BTreeMap<String, String>,
    pub relayed: Vec<RelayedDevice>,
    pub device_names: BTreeMap<String, String>,
    /// INVARIANT: Devices whose name a user cleared. An import never names them
    /// again, so a workspace that boots late cannot undo the clear.
    pub cleared_device_names: std::collections::BTreeSet<String>,
}

pub type Refusal = String;

impl Directory {
    pub fn replay<'a>(events: impl IntoIterator<Item = &'a LoggedEvent>) -> Self {
        let mut directory = Self::default();
        for event in events {
            directory.apply(event);
        }
        directory
    }

    pub fn apply(&mut self, logged: &LoggedEvent) {
        let at = &logged.at;
        match &logged.event {
            GatewayEvent::DevicePaired {
                device_id,
                label,
                credential_digest,
                user_id,
            } => {
                self.devices.devices.retain(|d| &d.id != device_id);
                self.devices.devices.push(PairedDevice {
                    id: device_id.clone(),
                    label: label.clone(),
                    credential_digest: credential_digest.clone(),
                    paired_at: at.clone(),
                    last_seen_at: None,
                });
                self.memberships.insert(device_id.clone(), user_id.clone());
            }
            GatewayEvent::DeviceSeen { device_id } => {
                if let Some(device) = self.device_mut(device_id) {
                    device.last_seen_at = Some(at.clone());
                }
            }
            GatewayEvent::DeviceRevoked { device_id } => {
                self.devices.devices.retain(|d| &d.id != device_id);
                self.memberships.remove(device_id);
                self.device_names.remove(device_id);
                self.cleared_device_names.remove(device_id);
            }
            GatewayEvent::DeviceMoved { device_id, user_id } => {
                if let Some(held) = self.memberships.get_mut(device_id) {
                    held.clone_from(user_id);
                }
            }
            GatewayEvent::UserAdded { user_id, name } => {
                if self.user(user_id).is_none() {
                    self.users.push(User {
                        id: user_id.clone(),
                        name: name.clone(),
                        emails: Vec::new(),
                        added_at: at.clone(),
                    });
                }
            }
            GatewayEvent::UserRenamed { user_id, name } => {
                if let Some(user) = self.user_mut(user_id) {
                    user.name.clone_from(name);
                }
            }
            GatewayEvent::UserRemoved { user_id } => {
                self.users.retain(|o| &o.id != user_id);
            }
            GatewayEvent::UserEmailAdded { user_id, email } => {
                if let Some(user) = self.user_mut(user_id) {
                    if !user.emails.contains(email) {
                        user.emails.push(email.clone());
                    }
                }
            }
            GatewayEvent::UserEmailRemoved { user_id, email } => {
                if let Some(user) = self.user_mut(user_id) {
                    user.emails.retain(|e| e != email);
                }
            }
            GatewayEvent::RelayedUserSeen {
                device_id,
                user_id,
                name,
                via,
                emails,
                label,
            } => {
                self.relayed.retain(|r| &r.device_id != device_id);
                self.relayed.push(RelayedDevice {
                    device_id: device_id.clone(),
                    user_id: user_id.clone(),
                    name: name.clone(),
                    via: via.clone(),
                    emails: emails.clone(),
                    label: label.clone(),
                });
            }
            GatewayEvent::RelayedPairingForgotten { pairing_id } => {
                let prefix = format!("{pairing_id}:");
                self.relayed.retain(|r| !r.device_id.starts_with(&prefix));
                self.device_names.retain(|id, _| !id.starts_with(&prefix));
                self.cleared_device_names
                    .retain(|id| !id.starts_with(&prefix));
            }
            GatewayEvent::DeviceNamed { device_id, name } => match name {
                Some(name) => {
                    self.cleared_device_names.remove(device_id);
                    self.device_names.insert(device_id.clone(), name.clone());
                }
                None => {
                    self.device_names.remove(device_id);
                    self.cleared_device_names.insert(device_id.clone());
                }
            },
        }
    }

    pub fn user(&self, id: &str) -> Option<&User> {
        self.users.iter().find(|o| o.id == id)
    }

    fn user_mut(&mut self, id: &str) -> Option<&mut User> {
        self.users.iter_mut().find(|o| o.id == id)
    }

    fn device_mut(&mut self, id: &str) -> Option<&mut PairedDevice> {
        self.devices.devices.iter_mut().find(|d| d.id == id)
    }

    fn has_device(&self, id: &str) -> bool {
        self.devices.devices.iter().any(|d| d.id == id)
    }

    pub fn user_of_device(&self, device_id: &str) -> Option<&str> {
        self.memberships.get(device_id).map(String::as_str)
    }

    pub fn devices_of(&self, user_id: &str) -> Vec<&str> {
        self.devices
            .devices
            .iter()
            .filter(|d| self.user_of_device(&d.id) == Some(user_id))
            .map(|d| d.id.as_str())
            .collect()
    }

    pub fn first_user(&self) -> Option<&User> {
        self.users.first()
    }

    pub fn pair(
        &self,
        device_id: &str,
        label: &str,
        credential_digest: &str,
        user_id: Option<&str>,
    ) -> Result<Vec<GatewayEvent>, Refusal> {
        let mut events = Vec::new();
        let user_id = match user_id {
            Some(id) if self.user(id).is_some() => id.to_string(),
            Some(_) => return Err("the user this pairing was for was removed".to_string()),
            None => match self.first_user() {
                Some(first) => first.id.clone(),
                None => {
                    let id = mint_user_id()?;
                    events.push(GatewayEvent::UserAdded {
                        user_id: id.clone(),
                        name: FIRST_USER_NAME.to_string(),
                    });
                    id
                }
            },
        };
        events.push(GatewayEvent::DevicePaired {
            device_id: device_id.to_string(),
            label: label.to_string(),
            credential_digest: credential_digest.to_string(),
            user_id,
        });
        Ok(events)
    }

    pub fn revoke(&self, device_id: &str) -> Vec<GatewayEvent> {
        if !self.has_device(device_id) {
            return Vec::new();
        }
        vec![GatewayEvent::DeviceRevoked {
            device_id: device_id.to_string(),
        }]
    }

    pub fn add_user(&self, name: &str) -> Result<Vec<GatewayEvent>, Refusal> {
        Ok(vec![GatewayEvent::UserAdded {
            user_id: mint_user_id()?,
            name: valid_name(name)?,
        }])
    }

    pub fn rename_user(&self, id: &str, name: &str) -> Result<Vec<GatewayEvent>, Refusal> {
        self.require_user(id)?;
        Ok(vec![GatewayEvent::UserRenamed {
            user_id: id.to_string(),
            name: valid_name(name)?,
        }])
    }

    pub fn remove_user(&self, id: &str) -> Result<Vec<GatewayEvent>, Refusal> {
        self.require_user(id)?;
        let mut events: Vec<GatewayEvent> = self
            .devices_of(id)
            .into_iter()
            .map(|device_id| GatewayEvent::DeviceRevoked {
                device_id: device_id.to_string(),
            })
            .collect();
        events.push(GatewayEvent::UserRemoved {
            user_id: id.to_string(),
        });
        Ok(events)
    }

    pub fn add_email(&self, id: &str, email: &str) -> Result<Vec<GatewayEvent>, Refusal> {
        self.require_user(id)?;
        let email = normalize_email(email)?;
        if let Some(holder) = self.user_of_email(&email) {
            if holder.id == id {
                return Ok(Vec::new());
            }
            return Err(format!("{email} already belongs to {}", holder.name));
        }
        Ok(vec![GatewayEvent::UserEmailAdded {
            user_id: id.to_string(),
            email,
        }])
    }

    pub fn remove_email(&self, id: &str, email: &str) -> Result<Vec<GatewayEvent>, Refusal> {
        let user = self.require_user(id)?;
        let email = email.trim().to_lowercase();
        if !user.emails.contains(&email) {
            return Err(format!("{} has no email {email}", user.name));
        }
        Ok(vec![GatewayEvent::UserEmailRemoved {
            user_id: id.to_string(),
            email,
        }])
    }

    pub fn move_device(
        &self,
        device_id: &str,
        user_id: &str,
    ) -> Result<Vec<GatewayEvent>, Refusal> {
        if !self.has_device(device_id) {
            return Err("no paired device with that id".to_string());
        }
        self.require_user(user_id)?;
        if self.user_of_device(device_id) == Some(user_id) {
            return Ok(Vec::new());
        }
        Ok(vec![GatewayEvent::DeviceMoved {
            device_id: device_id.to_string(),
            user_id: user_id.to_string(),
        }])
    }

    pub fn relayed_seen(&self, seen: RelayedDevice) -> Option<GatewayEvent> {
        if self.relayed.contains(&seen) {
            return None;
        }
        Some(GatewayEvent::RelayedUserSeen {
            device_id: seen.device_id,
            user_id: seen.user_id,
            name: seen.name,
            via: seen.via,
            emails: seen.emails,
            label: seen.label,
        })
    }

    /// INVARIANT: The one place a device's typed name changes. A blank name clears it,
    /// so the device falls back to its pairing label.
    pub fn name_device(
        &self,
        device_id: &str,
        name: Option<&str>,
    ) -> Result<Vec<GatewayEvent>, Refusal> {
        let device_id = valid_device_id(device_id)?;
        let name = match name.map(str::trim).filter(|n| !n.is_empty()) {
            Some(n) => Some(valid_device_name(n)?),
            None => None,
        };
        if self.device_names.get(device_id) == name.as_ref() {
            return Ok(Vec::new());
        }
        Ok(vec![GatewayEvent::DeviceNamed {
            device_id: device_id.to_string(),
            name,
        }])
    }

    /// INVARIANT: Names a workspace held before the gateway owned them (ADR 0825).
    /// The first name for a device wins: a later import never overwrites one,
    /// nor names a device whose name a user cleared. So a workspace that boots
    /// late cannot undo a rename made since. A name past the length limit is
    /// cut to it, since a workspace once accepted any length.
    ///
    /// A pairing label becomes the name only where this gateway knows no label
    /// for the device, which keeps what the device was called before.
    pub fn import_device_names(&self, imported: &[ImportedDeviceName]) -> Vec<GatewayEvent> {
        let mut taken: std::collections::BTreeSet<&str> = self
            .device_names
            .keys()
            .chain(&self.cleared_device_names)
            .map(String::as_str)
            .collect();
        let mut events = Vec::new();
        for row in imported {
            let Ok(device_id) = valid_device_id(&row.device_id) else {
                continue;
            };
            if taken.contains(device_id) {
                continue;
            }
            let label = row
                .pairing_label
                .as_deref()
                .filter(|_| self.label_of(device_id).is_none());
            let Some(name) = [row.name.as_deref(), label]
                .into_iter()
                .flatten()
                .map(str::trim)
                .find(|n| !n.is_empty())
                .map(|n| n.chars().take(MAX_DEVICE_NAME_CHARS).collect::<String>())
            else {
                continue;
            };
            taken.insert(device_id);
            events.push(GatewayEvent::DeviceNamed {
                device_id: device_id.to_string(),
                name: Some(name),
            });
        }
        events
    }

    fn label_of(&self, device_id: &str) -> Option<&str> {
        self.devices
            .devices
            .iter()
            .find(|d| d.id == device_id)
            .map(|d| d.label.as_str())
            .or_else(|| {
                self.relayed
                    .iter()
                    .find(|r| r.device_id == device_id)
                    .map(|r| r.label.as_str())
            })
            .filter(|l| !l.trim().is_empty())
    }

    /// INVARIANT: Every device this gateway can name, with its typed name and its
    /// pairing label. Engines resolve every device name from this list.
    pub fn device_identities(&self) -> Vec<DeviceIdentity> {
        let ids: std::collections::BTreeSet<&str> = self
            .devices
            .devices
            .iter()
            .map(|d| d.id.as_str())
            .chain(self.relayed.iter().map(|r| r.device_id.as_str()))
            .chain(self.device_names.keys().map(String::as_str))
            .collect();
        ids.into_iter()
            .map(|id| DeviceIdentity {
                id: id.to_string(),
                name: self.device_names.get(id).cloned(),
                label: self.label_of(id).map(str::to_string),
            })
            .collect()
    }

    pub fn pairing_forgotten(&self, pairing_id: &str) -> Option<GatewayEvent> {
        let prefix = format!("{pairing_id}:");
        self.relayed
            .iter()
            .any(|r| r.device_id.starts_with(&prefix))
            .then(|| GatewayEvent::RelayedPairingForgotten {
                pairing_id: pairing_id.to_string(),
            })
    }

    pub fn user_of_email(&self, email: &str) -> Option<&User> {
        let email = email.trim().to_lowercase();
        self.users.iter().find(|o| o.emails.contains(&email))
    }

    fn require_user(&self, id: &str) -> Result<&User, Refusal> {
        self.user(id)
            .ok_or_else(|| "no user with that id".to_string())
    }
}

fn mint_user_id() -> Result<String, Refusal> {
    lucidos_local_token::mint_hex(8).map_err(|e| format!("could not mint a user id: {e}"))
}

fn valid_name(name: &str) -> Result<String, Refusal> {
    let name = name.trim();
    if name.is_empty() {
        return Err("a user needs a name".to_string());
    }
    if name.chars().count() > 80 {
        return Err("a name is at most 80 characters".to_string());
    }
    Ok(name.to_string())
}

fn valid_device_id(id: &str) -> Result<&str, Refusal> {
    let id = id.trim();
    if id.is_empty() || id.len() > 200 {
        return Err("a device id is 1 to 200 characters".to_string());
    }
    Ok(id)
}

const MAX_DEVICE_NAME_CHARS: usize = 80;

fn valid_device_name(name: &str) -> Result<String, Refusal> {
    if name.chars().count() > MAX_DEVICE_NAME_CHARS {
        return Err(format!(
            "a device name is at most {MAX_DEVICE_NAME_CHARS} characters"
        ));
    }
    Ok(name.to_string())
}

pub fn normalize_email(email: &str) -> Result<String, Refusal> {
    let email = email.trim().to_lowercase();
    let valid = email.len() <= 254
        && !email.chars().any(char::is_whitespace)
        && email
            .split_once('@')
            .is_some_and(|(local, domain)| !local.is_empty() && domain.contains('.'));
    if !valid {
        return Err(format!("'{email}' is not an email address"));
    }
    Ok(email)
}

#[derive(Debug)]
pub struct EventLog {
    path: PathBuf,
}

impl EventLog {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn append(&self, events: &[LoggedEvent]) -> Result<(), crate::BoxError> {
        if events.is_empty() {
            return Ok(());
        }
        let mut lines = String::new();
        for event in events {
            lines.push_str(&serde_json::to_string(event)?);
            lines.push('\n');
        }
        let mut file = open_owner_only(&self.path, false)?;
        append_or_cut(&mut file, |file| {
            file.write_all(lines.as_bytes())?;
            file.sync_data()
        })?;
        Ok(())
    }
}

fn append_or_cut(
    file: &mut std::fs::File,
    write: impl FnOnce(&mut std::fs::File) -> std::io::Result<()>,
) -> std::io::Result<()> {
    let before = file.metadata()?.len();
    let written = write(file);
    if written.is_err() {
        file.set_len(before)?;
    }
    written
}

fn open_owner_only(path: &Path, truncate: bool) -> std::io::Result<std::fs::File> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.create(true);
    if truncate {
        options.write(true).truncate(true);
    } else {
        options.append(true);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    options.open(path)
}

#[derive(Debug, PartialEq, Eq)]
pub enum Opened {
    Replayed,
    RepairedTornTail,
    Migrated(usize),
    Fresh,
}

pub fn open(
    app_data: &Path,
    legacy: Option<&Path>,
) -> Result<(EventLog, Directory, Opened), crate::BoxError> {
    let log = EventLog {
        path: app_data.join(LOG_FILE),
    };
    if log.path.exists() {
        rename_operator_to_user(&log.path)?;
        let (events, torn) = read_log(&log.path)?;
        let opened = if torn {
            Opened::RepairedTornTail
        } else {
            Opened::Replayed
        };
        return Ok((log, Directory::replay(&events), opened));
    }
    let store_path = auth::paired_devices_path(app_data);
    let source = if store_path.exists() {
        Some(store_path.as_path())
    } else {
        legacy.filter(|p| p.exists())
    };
    let Some(source) = source else {
        return Ok((log, Directory::default(), Opened::Fresh));
    };
    let store = PairedDevices::load(source)?;
    let events = seed_events(&store)?;
    write_seed(&log.path, &events)?;
    if source == store_path {
        let mut migrated = store_path.clone().into_os_string();
        migrated.push(".");
        migrated.push(MIGRATED_SUFFIX);
        std::fs::rename(&store_path, migrated)?;
    }
    Ok((
        log,
        Directory::replay(&events),
        Opened::Migrated(store.devices.len()),
    ))
}

pub const DEVICE_CUT_MARKER: &str = "device-cut-loopback-tls.done";

/// Revoke every paired device and delete the old device stores, once.
///
/// Loopback moved to https with a partitioned cookie (ADR 0745), so no
/// credential minted before it is accepted again. One `DeviceRevoked` per
/// device, never a new event type: an older gateway would refuse to replay
/// one, and a rollback would not boot. Users, emails, relayed devices and the
/// gateway pairings stay.
///
/// Temporary: `docs/temporary-measures.md` § Loopback TLS device cut.
///
/// `None` when the cut ran before, else how many devices it revoked.
pub fn cut_devices_once(
    log: &EventLog,
    directory: &mut Directory,
    app_data: &Path,
    legacy: Option<&Path>,
) -> Result<Option<usize>, crate::BoxError> {
    let marker = app_data.join(DEVICE_CUT_MARKER);
    if marker.exists() {
        return Ok(None);
    }
    let revoked: Vec<LoggedEvent> = directory
        .devices
        .devices
        .iter()
        .flat_map(|device| directory.revoke(&device.id))
        .map(LoggedEvent::now)
        .collect();
    log.append(&revoked)?;
    for event in &revoked {
        directory.apply(event);
    }
    let store = auth::paired_devices_path(app_data);
    let mut migrated = store.clone().into_os_string();
    migrated.push(".");
    migrated.push(MIGRATED_SUFFIX);
    for stale in [Some(store.as_path()), Some(Path::new(&migrated)), legacy]
        .into_iter()
        .flatten()
    {
        match std::fs::remove_file(stale) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
            _ => {}
        }
    }
    std::fs::create_dir_all(app_data)?;
    std::fs::write(&marker, b"")?;
    Ok(Some(revoked.len()))
}

pub fn seed_events(store: &PairedDevices) -> Result<Vec<LoggedEvent>, crate::BoxError> {
    let Some(first) = store.devices.first() else {
        return Ok(Vec::new());
    };
    let user_id = mint_user_id()?;
    let mut events = vec![LoggedEvent {
        at: first.paired_at.clone(),
        event: GatewayEvent::UserAdded {
            user_id: user_id.clone(),
            name: FIRST_USER_NAME.to_string(),
        },
    }];
    for device in &store.devices {
        events.push(LoggedEvent {
            at: device.paired_at.clone(),
            event: GatewayEvent::DevicePaired {
                device_id: device.id.clone(),
                label: device.label.clone(),
                credential_digest: device.credential_digest.clone(),
                user_id: user_id.clone(),
            },
        });
        if let Some(seen) = &device.last_seen_at {
            events.push(LoggedEvent {
                at: seen.clone(),
                event: GatewayEvent::DeviceSeen {
                    device_id: device.id.clone(),
                },
            });
        }
    }
    Ok(events)
}

fn write_seed(path: &Path, events: &[LoggedEvent]) -> Result<(), crate::BoxError> {
    let mut lines = String::new();
    for event in events {
        lines.push_str(&serde_json::to_string(event)?);
        lines.push('\n');
    }
    replace_whole(path, &lines)
}

fn replace_whole(path: &Path, contents: &str) -> Result<(), crate::BoxError> {
    let tmp = path.with_extension("jsonl.tmp");
    let mut file = open_owner_only(&tmp, true)?;
    file.write_all(contents.as_bytes())?;
    file.sync_data()?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

pub const PRE_USER_RENAME_SUFFIX: &str = "pre-user-rename";

fn rename_operator_to_user(path: &Path) -> Result<(), crate::BoxError> {
    let raw = std::fs::read_to_string(path)?;
    let mut changed = false;
    let mut rewritten = String::with_capacity(raw.len());
    for line in raw.split_inclusive('\n') {
        let body = line.trim_end_matches('\n');
        let renamed = serde_json::from_str::<serde_json::Value>(body)
            .ok()
            .and_then(|mut value| rename_logged_value(&mut value).then_some(value));
        match renamed {
            Some(value) => {
                changed = true;
                rewritten.push_str(&serde_json::to_string(&value)?);
                rewritten.push_str(&line[body.len()..]);
            }
            None => rewritten.push_str(line),
        }
    }
    if !changed {
        return Ok(());
    }
    let mut backup = path.as_os_str().to_owned();
    backup.push(".");
    backup.push(PRE_USER_RENAME_SUFFIX);
    let backup = PathBuf::from(backup);
    if !backup.exists() {
        std::fs::copy(path, &backup)?;
    }
    replace_whole(path, &rewritten)
}

fn rename_logged_value(logged: &mut serde_json::Value) -> bool {
    let Some(event) = logged.get_mut("event") else {
        return false;
    };
    let mut changed = false;
    if let Some(serde_json::Value::String(kind)) = event.get_mut("type") {
        if kind.contains("Operator") {
            *kind = kind.replace("Operator", "User");
            changed = true;
        }
    }
    if let Some(serde_json::Value::Object(data)) = event.get_mut("data") {
        if let Some(id) = data.remove("operator_id") {
            data.insert("user_id".to_string(), id);
            changed = true;
        }
    }
    changed
}

fn read_log(path: &Path) -> Result<(Vec<LoggedEvent>, bool), crate::BoxError> {
    let raw = std::fs::read_to_string(path)?;
    let (complete, tail) = match raw.rfind('\n') {
        Some(end) => raw.split_at(end + 1),
        None => ("", raw.as_str()),
    };
    let mut events = Vec::new();
    for (index, line) in complete.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let event = serde_json::from_str(line)
            .map_err(|e| format!("{} line {}: {e}", path.display(), index + 1))?;
        events.push(event);
    }
    if tail.trim().is_empty() {
        return Ok((events, false));
    }
    if let Ok(event) = serde_json::from_str(tail) {
        let mut file = open_owner_only(path, false)?;
        file.write_all(b"\n")?;
        file.sync_data()?;
        events.push(event);
        return Ok((events, false));
    }
    let file = std::fs::OpenOptions::new().write(true).open(path)?;
    file.set_len(complete.len() as u64)?;
    file.sync_data()?;
    Ok((events, true))
}

#[cfg(test)]
#[path = "gateway_event_log_tests.rs"]
mod tests;
