use serde::{Deserialize, Serialize};

use crate::{Claim, StampKey};

pub const HEADER_REMOTE_START: &str = "x-lucidos-remote-start";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RemoteStartSetting {
    Allow,
    Ask,
    #[default]
    Deny,
}

impl RemoteStartSetting {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Ask => "ask",
            Self::Deny => "deny",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemotePairing {
    pub id: String,
    pub label: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteStart {
    pub setting: RemoteStartSetting,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pairing: Option<RemotePairing>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Wire {
    start: RemoteStart,
    stamp: String,
}

impl RemoteStart {
    pub fn allowed() -> Self {
        Self {
            setting: RemoteStartSetting::Allow,
            pairing: None,
        }
    }

    fn is_admissible(&self) -> bool {
        match self.setting {
            RemoteStartSetting::Allow => true,
            RemoteStartSetting::Ask => self.pairing.is_some(),
            RemoteStartSetting::Deny => false,
        }
    }

    fn claim<'a>(&'a self, target_workspace: &'a str, nonce: &'a str) -> Claim<'a> {
        let (pairing_id, pairing_label) = self
            .pairing
            .as_ref()
            .map_or(("", ""), |p| (p.id.as_str(), p.label.as_str()));
        Claim::RemoteStart {
            target_workspace,
            nonce,
            setting: self.setting,
            pairing_id,
            pairing_label,
        }
    }

    pub fn header_value(&self, key: &StampKey, target_workspace: &str, nonce: &str) -> String {
        let wire = Wire {
            start: self.clone(),
            stamp: key.sign_now(self.claim(target_workspace, nonce)),
        };
        crate::hex(&serde_json::to_vec(&wire).expect("plain fields serialize"))
    }

    pub fn from_header(
        value: &str,
        key: &StampKey,
        target_workspace: &str,
        nonce: &str,
        now_unix: u64,
    ) -> Option<Self> {
        let wire: Wire = serde_json::from_slice(&crate::unhex(value)?).ok()?;
        (wire.start.is_admissible()
            && key.verify(
                &wire.stamp,
                wire.start.claim(target_workspace, nonce),
                now_unix,
            ))
        .then_some(wire.start)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> StampKey {
        StampKey::read_handover("ab".repeat(32).as_bytes()).unwrap()
    }

    fn ask() -> RemoteStart {
        RemoteStart {
            setting: RemoteStartSetting::Ask,
            pairing: Some(RemotePairing {
                id: "pairing-1".into(),
                label: "Büro Mac; \"laptop\"".into(),
            }),
        }
    }

    #[test]
    fn a_signed_start_reads_back_for_its_own_target_and_nonce_only() {
        let value = ask().header_value(&key(), "target", "nonce-1");
        assert!(value.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_eq!(
            RemoteStart::from_header(&value, &key(), "target", "nonce-1", crate::now_unix()),
            Some(ask())
        );
        assert_eq!(
            RemoteStart::from_header(&value, &key(), "other", "nonce-1", crate::now_unix()),
            None
        );
        assert_eq!(
            RemoteStart::from_header(&value, &key(), "target", "nonce-2", crate::now_unix()),
            None
        );
        let other_key = StampKey::read_handover("cd".repeat(32).as_bytes()).unwrap();
        assert_eq!(
            RemoteStart::from_header(&value, &other_key, "target", "nonce-1", crate::now_unix()),
            None
        );
    }

    #[test]
    fn an_altered_setting_or_label_breaks_the_stamp() {
        let value = ask().header_value(&key(), "target", "nonce");
        let mut wire: Wire = serde_json::from_slice(&crate::unhex(&value).unwrap()).unwrap();
        wire.start.setting = RemoteStartSetting::Allow;
        let forged = crate::hex(&serde_json::to_vec(&wire).unwrap());
        assert_eq!(
            RemoteStart::from_header(&forged, &key(), "target", "nonce", crate::now_unix()),
            None
        );
        wire.start = ask();
        wire.start.pairing.as_mut().unwrap().label = "Someone else".into();
        let forged = crate::hex(&serde_json::to_vec(&wire).unwrap());
        assert_eq!(
            RemoteStart::from_header(&forged, &key(), "target", "nonce", crate::now_unix()),
            None
        );
    }

    #[test]
    fn deny_and_an_ask_with_no_pairing_never_read_back() {
        for start in [
            RemoteStart {
                setting: RemoteStartSetting::Deny,
                pairing: ask().pairing,
            },
            RemoteStart {
                setting: RemoteStartSetting::Ask,
                pairing: None,
            },
        ] {
            let value = start.header_value(&key(), "target", "nonce");
            assert_eq!(
                RemoteStart::from_header(&value, &key(), "target", "nonce", crate::now_unix()),
                None
            );
        }
        for junk in ["", "zz", "abc"] {
            assert_eq!(
                RemoteStart::from_header(junk, &key(), "target", "nonce", crate::now_unix()),
                None
            );
        }
    }

    #[test]
    fn the_setting_defaults_to_deny_and_is_kebab_case_on_the_wire() {
        assert_eq!(RemoteStartSetting::default(), RemoteStartSetting::Deny);
        for setting in [
            RemoteStartSetting::Allow,
            RemoteStartSetting::Ask,
            RemoteStartSetting::Deny,
        ] {
            assert_eq!(
                serde_json::to_value(setting).unwrap(),
                serde_json::Value::from(setting.as_str())
            );
        }
    }
}
