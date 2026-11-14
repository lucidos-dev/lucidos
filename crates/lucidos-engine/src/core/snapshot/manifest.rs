use super::{handle::Handle, BoxError};
use crate::{core::secret_store::SecretKey, engine::thread_events::MessageOrigin};
use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    Manual,
    Recovery,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ComponentName {
    #[serde(rename = "db.dump")]
    Database,
    #[serde(rename = "files.tar.zst")]
    Files,
    #[serde(rename = "files.index.jsonl.zst")]
    Index,
}

impl ComponentName {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Database => "db.dump",
            Self::Files => "files.tar.zst",
            Self::Index => "files.index.jsonl.zst",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Component {
    pub name: ComponentName,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Consistency {
    pub db_captured_at: DateTime<Utc>,
    pub files_started_at: DateTime<Utc>,
    pub files_finished_at: DateTime<Utc>,
    pub live: bool,
    pub running_threads: u64,
    pub files_changed_during_read: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Files {
    pub count: u64,
    pub bytes: u64,
    pub tree_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Verification {
    pub mac: bool,
    pub at: DateTime<Utc>,
}

/// Caller-safe metadata. Keys, database URLs and local paths have no fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub format_version: u32,
    pub handle: Handle,
    pub kind: Kind,
    pub label: Option<String>,
    pub keep: bool,
    pub workspace_id: String,
    pub workspace_name: String,
    pub created_at: DateTime<Utc>,
    pub captured_by: MessageOrigin,
    pub engine_version: String,
    pub schema_version: i64,
    pub secret_key_ids: BTreeSet<String>,
    pub consistency: Consistency,
    pub components: Vec<Component>,
    pub files: Files,
    pub table_rows: BTreeMap<String, i64>,
    pub verified: Option<Verification>,
    pub restore_id: Option<Uuid>,
}

#[derive(Debug, Serialize)]
pub struct AuthenticatedManifest {
    #[serde(flatten)]
    pub manifest: Manifest,
    pub mac: String,
}

impl<'de> Deserialize<'de> for AuthenticatedManifest {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let mut value = serde_json::Value::deserialize(deserializer)?;
        let mac = value
            .as_object_mut()
            .and_then(|object| object.remove("mac"))
            .and_then(|mac| mac.as_str().map(str::to_owned))
            .ok_or_else(|| D::Error::custom("snapshot manifest authentication missing"))?;
        let manifest: Manifest = serde_json::from_value(value.clone()).map_err(D::Error::custom)?;
        let normalized = serde_json::to_value(&manifest).map_err(D::Error::custom)?;
        if canonical_json(&value).map_err(D::Error::custom)?
            != canonical_json(&normalized).map_err(D::Error::custom)?
        {
            return Err(D::Error::custom(
                "snapshot manifest fields do not match format",
            ));
        }
        Ok(Self { manifest, mac })
    }
}

/// Object keys sort recursively, independent of serde_json's map backend.
pub fn canonical_json(value: &serde_json::Value) -> Result<Vec<u8>, BoxError> {
    fn sorted(value: &serde_json::Value) -> serde_json::Value {
        match value {
            serde_json::Value::Object(map) => {
                let entries: BTreeMap<_, _> =
                    map.iter().map(|(k, v)| (k.clone(), sorted(v))).collect();
                serde_json::Value::Object(entries.into_iter().collect())
            }
            serde_json::Value::Array(items) => {
                serde_json::Value::Array(items.iter().map(sorted).collect())
            }
            other => other.clone(),
        }
    }
    Ok(serde_json::to_vec(&sorted(value))?)
}

impl AuthenticatedManifest {
    pub fn seal(manifest: Manifest, key: &SecretKey) -> Result<Self, BoxError> {
        let mut mac = Hmac::<Sha256>::new_from_slice(key.as_bytes())?;
        mac.update(&canonical_json(&serde_json::to_value(&manifest)?)?);
        use base64::Engine;
        Ok(Self {
            manifest,
            mac: base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes()),
        })
    }

    pub fn verify(
        &self,
        key: &SecretKey,
        workspace: &str,
        handle: &Handle,
    ) -> Result<(), BoxError> {
        use base64::Engine;
        let mut mac = Hmac::<Sha256>::new_from_slice(key.as_bytes())?;
        mac.update(&canonical_json(&serde_json::to_value(&self.manifest)?)?);
        let expected = base64::engine::general_purpose::STANDARD
            .decode(&self.mac)
            .map_err(|_| "snapshot manifest authentication failed")?;
        mac.verify_slice(&expected)
            .map_err(|_| "snapshot manifest authentication failed")?;
        if self.manifest.workspace_id != workspace || &self.manifest.handle != handle {
            return Err("snapshot_wrong_workspace".into());
        }
        if self.manifest.format_version != 1 {
            return Err("unsupported snapshot format".into());
        }
        let names: BTreeSet<_> = self.manifest.components.iter().map(|c| c.name).collect();
        if names.len() != 3 || self.manifest.components.len() != 3 {
            return Err("snapshot components are incomplete or duplicated".into());
        }
        if (self.manifest.kind == Kind::Recovery) != self.manifest.restore_id.is_some() {
            return Err("snapshot recovery reference is invalid".into());
        }
        Ok(())
    }
}

#[cfg(test)]
pub(crate) fn fixture() -> Manifest {
    let now = Utc::now();
    Manifest {
        format_version: 1,
        handle: Handle::generate(),
        kind: Kind::Manual,
        label: None,
        keep: false,
        workspace_id: "demo-workspace".into(),
        workspace_name: "demo".into(),
        created_at: now,
        captured_by: MessageOrigin::Device {
            device_id: "test-device".into(),
        },
        engine_version: "test".into(),
        schema_version: 1,
        secret_key_ids: BTreeSet::new(),
        consistency: Consistency {
            db_captured_at: now,
            files_started_at: now,
            files_finished_at: now,
            live: true,
            running_threads: 0,
            files_changed_during_read: 0,
        },
        components: [
            ComponentName::Database,
            ComponentName::Files,
            ComponentName::Index,
        ]
        .into_iter()
        .map(|name| Component {
            name,
            bytes: 0,
            sha256: String::new(),
        })
        .collect(),
        files: Files {
            count: 0,
            bytes: 0,
            tree_sha256: String::new(),
        },
        table_rows: BTreeMap::new(),
        verified: None,
        restore_id: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn canonical_json_is_recursive_and_authentication_binds_every_field() {
        assert_eq!(
            canonical_json(&serde_json::json!({"b":{"z":1,"a":2},"a":0})).unwrap(),
            br#"{"a":0,"b":{"a":2,"z":1}}"#
        );
        let key = SecretKey::generate();
        let original = fixture();
        let id = original.workspace_id.clone();
        let handle = original.handle.clone();
        let signed = AuthenticatedManifest::seal(original, &key).unwrap();
        signed.verify(&key, &id, &handle).unwrap();
        assert!(signed.verify(&SecretKey::generate(), &id, &handle).is_err());
        assert!(signed.verify(&key, "another-workspace", &handle).is_err());
        let value = serde_json::to_value(&signed).unwrap();
        let mut missing = value.clone();
        missing.as_object_mut().unwrap().remove("label");
        assert!(serde_json::from_value::<AuthenticatedManifest>(missing).is_err());
        assert!(!serde_json::to_string(&signed)
            .unwrap()
            .contains(&key.to_base64()));
        for field in value.as_object().unwrap().keys().filter(|k| *k != "mac") {
            let mut edited = value.clone();
            edited[field] = serde_json::Value::Null;
            if let Ok(edited) = serde_json::from_value::<AuthenticatedManifest>(edited) {
                // Null only constitutes an edit for a non-null original field.
                if !value[field].is_null() {
                    assert!(edited.verify(&key, &id, &handle).is_err());
                }
            }
        }
        let mut edited = signed;
        edited.manifest.keep = true;
        assert!(edited.verify(&key, &id, &handle).is_err());
    }
}
