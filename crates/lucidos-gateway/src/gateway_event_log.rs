//! The gateway event log: devices and operators, event sourced (ADR 0623).
//!
//! Every pairing, revoke and operator change is one line of JSON appended to
//! `<data dir>/gateway-events.jsonl`, and synced before anyone acts on it. The
//! [`Directory`] is a projection of that file: replaying the log rebuilds it.
//! The log needs no Postgres, so a device still signs in while Postgres is down.
//!
//! **An operator is identity without authority.** Nothing in the gateway reads
//! an operator to allow or refuse a request. [`crate::auth::authorize_with_match`]
//! reads only [`Directory::devices`].
//!
//! The log replaces `paired-devices.json`, which [`open`] migrates exactly once
//! into seed events, every device under one operator.

use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::auth::{self, PairedDevice, PairedDevices};

/// The log's file name, under the gateway's data dir.
pub const LOG_FILE: &str = "gateway-events.jsonl";

/// What `paired-devices.json` is renamed to once its rows are in the log. No
/// gateway reads it again, so a downgrade cannot revive a revoked device.
const MIGRATED_SUFFIX: &str = "migrated";

/// The name of the operator a gateway creates for itself: the one that holds
/// every migrated device, or the first device paired with no operator named.
pub const FIRST_OPERATOR_NAME: &str = "Owner";

/// One change to the devices or operators, named in the past tense.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum GatewayEvent {
    DevicePaired {
        device_id: String,
        label: String,
        credential_digest: String,
        operator_id: String,
    },
    /// A device reached this gateway, at most once a day. A liveness hint for
    /// the devices list, never an auth input.
    DeviceSeen {
        device_id: String,
    },
    DeviceRevoked {
        device_id: String,
    },
    DeviceMoved {
        device_id: String,
        operator_id: String,
    },
    OperatorAdded {
        operator_id: String,
        name: String,
    },
    OperatorRenamed {
        operator_id: String,
        name: String,
    },
    /// Its devices were revoked first, each with its own `DeviceRevoked`.
    OperatorRemoved {
        operator_id: String,
    },
    OperatorEmailAdded {
        operator_id: String,
        email: String,
    },
    OperatorEmailRemoved {
        operator_id: String,
        email: String,
    },
    /// A device relayed through a gateway pairing arrived with a signed
    /// operator that differs from the last one recorded for it.
    RelayedOperatorSeen {
        device_id: String,
        operator_id: String,
        name: String,
        via: String,
        /// The emails A holds for the person. An engine merges the relayed
        /// person into the local one holding the same email.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        emails: Vec<String>,
    },
    /// A gateway pairing ended, so its relayed people are gone. Device ids
    /// relayed through it start with `<pairing_id>:`.
    RelayedPairingForgotten {
        pairing_id: String,
    },
}

impl GatewayEvent {
    /// Does this change who is who? `DeviceSeen` does not, so engines are not
    /// told about it.
    pub fn changes_identity(&self) -> bool {
        !matches!(self, Self::DeviceSeen { .. })
    }
}

/// One line of the log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoggedEvent {
    /// RFC 3339, when the change happened.
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

/// A person, as this gateway knows them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Operator {
    pub id: String,
    pub name: String,
    /// Lowercase. One email belongs to at most one operator.
    pub emails: Vec<String>,
    pub added_at: String,
}

/// A device that reaches this gateway through a gateway pairing, with the
/// operator the other gateway signed for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RelayedDevice {
    /// The namespaced id this gateway's engines see.
    pub device_id: String,
    /// Namespaced like the device id, so it never collides with a local one.
    pub operator_id: String,
    pub name: String,
    /// The other gateway's label.
    pub via: String,
    /// The emails A signed for the person, lowercase.
    #[serde(default)]
    pub emails: Vec<String>,
}

/// The projection of the log.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Directory {
    /// Every paired device. Authorization reads this and nothing else.
    pub devices: PairedDevices,
    /// Oldest first, which is what makes [`Directory::first_operator`] stable.
    pub operators: Vec<Operator>,
    /// Which operator holds each device.
    pub memberships: BTreeMap<String, String>,
    pub relayed: Vec<RelayedDevice>,
}

/// Why a change was refused. Always the caller's mistake, said plainly.
pub type Refusal = String;

impl Directory {
    /// Rebuild the projection from logged events, oldest first.
    pub fn replay<'a>(events: impl IntoIterator<Item = &'a LoggedEvent>) -> Self {
        let mut directory = Self::default();
        for event in events {
            directory.apply(event);
        }
        directory
    }

    /// Fold one event in. Total: an event naming something already gone
    /// changes nothing, so a log always replays.
    pub fn apply(&mut self, logged: &LoggedEvent) {
        let at = &logged.at;
        match &logged.event {
            GatewayEvent::DevicePaired {
                device_id,
                label,
                credential_digest,
                operator_id,
            } => {
                self.devices.devices.retain(|d| &d.id != device_id);
                self.devices.devices.push(PairedDevice {
                    id: device_id.clone(),
                    label: label.clone(),
                    credential_digest: credential_digest.clone(),
                    paired_at: at.clone(),
                    last_seen_at: None,
                });
                self.memberships
                    .insert(device_id.clone(), operator_id.clone());
            }
            GatewayEvent::DeviceSeen { device_id } => {
                if let Some(device) = self.device_mut(device_id) {
                    device.last_seen_at = Some(at.clone());
                }
            }
            GatewayEvent::DeviceRevoked { device_id } => {
                self.devices.devices.retain(|d| &d.id != device_id);
                self.memberships.remove(device_id);
            }
            GatewayEvent::DeviceMoved {
                device_id,
                operator_id,
            } => {
                if let Some(held) = self.memberships.get_mut(device_id) {
                    held.clone_from(operator_id);
                }
            }
            GatewayEvent::OperatorAdded { operator_id, name } => {
                if self.operator(operator_id).is_none() {
                    self.operators.push(Operator {
                        id: operator_id.clone(),
                        name: name.clone(),
                        emails: Vec::new(),
                        added_at: at.clone(),
                    });
                }
            }
            GatewayEvent::OperatorRenamed { operator_id, name } => {
                if let Some(operator) = self.operator_mut(operator_id) {
                    operator.name.clone_from(name);
                }
            }
            GatewayEvent::OperatorRemoved { operator_id } => {
                self.operators.retain(|o| &o.id != operator_id);
            }
            GatewayEvent::OperatorEmailAdded { operator_id, email } => {
                if let Some(operator) = self.operator_mut(operator_id) {
                    if !operator.emails.contains(email) {
                        operator.emails.push(email.clone());
                    }
                }
            }
            GatewayEvent::OperatorEmailRemoved { operator_id, email } => {
                if let Some(operator) = self.operator_mut(operator_id) {
                    operator.emails.retain(|e| e != email);
                }
            }
            GatewayEvent::RelayedOperatorSeen {
                device_id,
                operator_id,
                name,
                via,
                emails,
            } => {
                self.relayed.retain(|r| &r.device_id != device_id);
                self.relayed.push(RelayedDevice {
                    device_id: device_id.clone(),
                    operator_id: operator_id.clone(),
                    name: name.clone(),
                    via: via.clone(),
                    emails: emails.clone(),
                });
            }
            GatewayEvent::RelayedPairingForgotten { pairing_id } => {
                let prefix = format!("{pairing_id}:");
                self.relayed.retain(|r| !r.device_id.starts_with(&prefix));
            }
        }
    }

    pub fn operator(&self, id: &str) -> Option<&Operator> {
        self.operators.iter().find(|o| o.id == id)
    }

    fn operator_mut(&mut self, id: &str) -> Option<&mut Operator> {
        self.operators.iter_mut().find(|o| o.id == id)
    }

    fn device_mut(&mut self, id: &str) -> Option<&mut PairedDevice> {
        self.devices.devices.iter_mut().find(|d| d.id == id)
    }

    fn has_device(&self, id: &str) -> bool {
        self.devices.devices.iter().any(|d| d.id == id)
    }

    /// The operator a device belongs to.
    pub fn operator_of_device(&self, device_id: &str) -> Option<&str> {
        self.memberships.get(device_id).map(String::as_str)
    }

    /// The devices an operator holds, in pairing order.
    pub fn devices_of(&self, operator_id: &str) -> Vec<&str> {
        self.devices
            .devices
            .iter()
            .filter(|d| self.operator_of_device(&d.id) == Some(operator_id))
            .map(|d| d.id.as_str())
            .collect()
    }

    /// The oldest operator still here: the one a pairing with no operator
    /// named joins.
    pub fn first_operator(&self) -> Option<&Operator> {
        self.operators.first()
    }

    /// The events that pair a device. With no operator named it joins the
    /// first operator, which a gateway with none creates first.
    pub fn pair(
        &self,
        device_id: &str,
        label: &str,
        credential_digest: &str,
        operator_id: Option<&str>,
    ) -> Result<Vec<GatewayEvent>, Refusal> {
        let mut events = Vec::new();
        let operator_id = match operator_id {
            Some(id) if self.operator(id).is_some() => id.to_string(),
            Some(_) => return Err("the person this pairing was for was removed".to_string()),
            None => match self.first_operator() {
                Some(first) => first.id.clone(),
                None => {
                    let id = mint_operator_id()?;
                    events.push(GatewayEvent::OperatorAdded {
                        operator_id: id.clone(),
                        name: FIRST_OPERATOR_NAME.to_string(),
                    });
                    id
                }
            },
        };
        events.push(GatewayEvent::DevicePaired {
            device_id: device_id.to_string(),
            label: label.to_string(),
            credential_digest: credential_digest.to_string(),
            operator_id,
        });
        Ok(events)
    }

    /// The event that revokes a device, or none when no device has that id.
    pub fn revoke(&self, device_id: &str) -> Vec<GatewayEvent> {
        if !self.has_device(device_id) {
            return Vec::new();
        }
        vec![GatewayEvent::DeviceRevoked {
            device_id: device_id.to_string(),
        }]
    }

    pub fn add_operator(&self, name: &str) -> Result<Vec<GatewayEvent>, Refusal> {
        Ok(vec![GatewayEvent::OperatorAdded {
            operator_id: mint_operator_id()?,
            name: valid_name(name)?,
        }])
    }

    pub fn rename_operator(&self, id: &str, name: &str) -> Result<Vec<GatewayEvent>, Refusal> {
        self.require_operator(id)?;
        Ok(vec![GatewayEvent::OperatorRenamed {
            operator_id: id.to_string(),
            name: valid_name(name)?,
        }])
    }

    /// Revoke every device the operator holds, then remove the operator. That
    /// is how a team offboards someone.
    pub fn remove_operator(&self, id: &str) -> Result<Vec<GatewayEvent>, Refusal> {
        self.require_operator(id)?;
        let mut events: Vec<GatewayEvent> = self
            .devices_of(id)
            .into_iter()
            .map(|device_id| GatewayEvent::DeviceRevoked {
                device_id: device_id.to_string(),
            })
            .collect();
        events.push(GatewayEvent::OperatorRemoved {
            operator_id: id.to_string(),
        });
        Ok(events)
    }

    pub fn add_email(&self, id: &str, email: &str) -> Result<Vec<GatewayEvent>, Refusal> {
        self.require_operator(id)?;
        let email = normalize_email(email)?;
        if let Some(holder) = self.operator_of_email(&email) {
            if holder.id == id {
                return Ok(Vec::new());
            }
            return Err(format!("{email} already belongs to {}", holder.name));
        }
        Ok(vec![GatewayEvent::OperatorEmailAdded {
            operator_id: id.to_string(),
            email,
        }])
    }

    pub fn remove_email(&self, id: &str, email: &str) -> Result<Vec<GatewayEvent>, Refusal> {
        let operator = self.require_operator(id)?;
        let email = email.trim().to_lowercase();
        if !operator.emails.contains(&email) {
            return Err(format!("{} has no email {email}", operator.name));
        }
        Ok(vec![GatewayEvent::OperatorEmailRemoved {
            operator_id: id.to_string(),
            email,
        }])
    }

    pub fn move_device(
        &self,
        device_id: &str,
        operator_id: &str,
    ) -> Result<Vec<GatewayEvent>, Refusal> {
        if !self.has_device(device_id) {
            return Err("no paired device with that id".to_string());
        }
        self.require_operator(operator_id)?;
        if self.operator_of_device(device_id) == Some(operator_id) {
            return Ok(Vec::new());
        }
        Ok(vec![GatewayEvent::DeviceMoved {
            device_id: device_id.to_string(),
            operator_id: operator_id.to_string(),
        }])
    }

    /// The event recording a relayed device's signed operator, or none when
    /// it is what the log already says.
    pub fn relayed_seen(&self, seen: RelayedDevice) -> Option<GatewayEvent> {
        if self.relayed.contains(&seen) {
            return None;
        }
        Some(GatewayEvent::RelayedOperatorSeen {
            device_id: seen.device_id,
            operator_id: seen.operator_id,
            name: seen.name,
            via: seen.via,
            emails: seen.emails,
        })
    }

    /// The event dropping the people relayed through an ended pairing, or
    /// none when it relayed nobody.
    pub fn pairing_forgotten(&self, pairing_id: &str) -> Option<GatewayEvent> {
        let prefix = format!("{pairing_id}:");
        self.relayed
            .iter()
            .any(|r| r.device_id.starts_with(&prefix))
            .then(|| GatewayEvent::RelayedPairingForgotten {
                pairing_id: pairing_id.to_string(),
            })
    }

    /// The operator holding an email, matched without regard to case.
    pub fn operator_of_email(&self, email: &str) -> Option<&Operator> {
        let email = email.trim().to_lowercase();
        self.operators.iter().find(|o| o.emails.contains(&email))
    }

    fn require_operator(&self, id: &str) -> Result<&Operator, Refusal> {
        self.operator(id)
            .ok_or_else(|| "no person with that id".to_string())
    }
}

fn mint_operator_id() -> Result<String, Refusal> {
    lucidos_local_token::mint_hex(8).map_err(|e| format!("could not mint an operator id: {e}"))
}

fn valid_name(name: &str) -> Result<String, Refusal> {
    let name = name.trim();
    if name.is_empty() {
        return Err("a person needs a name".to_string());
    }
    if name.chars().count() > 80 {
        return Err("a name is at most 80 characters".to_string());
    }
    Ok(name.to_string())
}

/// Lowercase and trimmed. Checks the shape only: the address is a key to
/// match against, never somewhere Lucidos sends mail.
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

/// The log file, append only.
#[derive(Debug)]
pub struct EventLog {
    path: PathBuf,
}

impl EventLog {
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Append events and sync them to disk before returning.
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

/// Run `write` on the log, and on failure cut the file back to its length
/// before. A partial line left in place would land mid-file at the next
/// append, and the next boot could not replay the log.
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

/// Open for appending, created owner-only: the log carries credential
/// digests. `truncate` starts the file over, for the one-time migration.
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

/// What [`open`] found, for the boot log.
#[derive(Debug, PartialEq, Eq)]
pub enum Opened {
    /// The log existed and replayed.
    Replayed,
    /// A torn last line, left by a crash mid-append, was cut off.
    RepairedTornTail,
    /// The device store moved into the log, with this many devices.
    Migrated(usize),
    /// Nothing to load: a fresh install. The file appears on the first event.
    Fresh,
}

/// Open this gateway's log, migrating `paired-devices.json` the first time.
///
/// `legacy` is the pre-isolation machine-global store, read only when this
/// gateway has neither a log nor a store of its own.
pub fn open(
    app_data: &Path,
    legacy: Option<&Path>,
) -> Result<(EventLog, Directory, Opened), crate::BoxError> {
    let log = EventLog {
        path: app_data.join(LOG_FILE),
    };
    if log.path.exists() {
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

/// The events that reproduce a device store: one operator holding every
/// device, each paired and last seen when the store says.
pub fn seed_events(store: &PairedDevices) -> Result<Vec<LoggedEvent>, crate::BoxError> {
    let Some(first) = store.devices.first() else {
        return Ok(Vec::new());
    };
    let operator_id = mint_operator_id()?;
    let mut events = vec![LoggedEvent {
        at: first.paired_at.clone(),
        event: GatewayEvent::OperatorAdded {
            operator_id: operator_id.clone(),
            name: FIRST_OPERATOR_NAME.to_string(),
        },
    }];
    for device in &store.devices {
        events.push(LoggedEvent {
            at: device.paired_at.clone(),
            event: GatewayEvent::DevicePaired {
                device_id: device.id.clone(),
                label: device.label.clone(),
                credential_digest: device.credential_digest.clone(),
                operator_id: operator_id.clone(),
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

/// Write the seed whole, then rename it into place, so a crash leaves either
/// no log (and the store still there) or the complete one.
fn write_seed(path: &Path, events: &[LoggedEvent]) -> Result<(), crate::BoxError> {
    let tmp = path.with_extension("jsonl.tmp");
    let mut lines = String::new();
    for event in events {
        lines.push_str(&serde_json::to_string(event)?);
        lines.push('\n');
    }
    let mut file = open_owner_only(&tmp, true)?;
    file.write_all(lines.as_bytes())?;
    file.sync_data()?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Read every event. A last line with no newline that does not parse is a
/// torn append: it is cut off the file, and `true` says so. Any other bad line
/// is an error, since skipping it could forget a revoke.
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
        // Whole but unterminated: end the line, or the next append joins it.
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
