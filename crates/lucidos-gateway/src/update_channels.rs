//! Persistent channel selection. Raw values survive hand edits so refusals stay visible.
use lucidos_update_channel::{Channel, CustomName, UpdateAddress, BETA_FEED_URL};
use serde::{Deserialize, Serialize};

pub const LEGACY_SLOT_NAME: &str = "Custom address";

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct CustomSlot {
    pub name: String,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdatesToml {
    pub enabled: bool,
    pub channel: String,
    pub custom: Vec<CustomSlot>,
}

impl Default for UpdatesToml {
    fn default() -> Self {
        Self {
            enabled: true,
            channel: "stable".into(),
            custom: Vec::new(),
        }
    }
}

impl UpdatesToml {
    pub fn address(&self) -> Result<Option<UpdateAddress>, String> {
        match Channel::parse(&self.channel)? {
            Channel::Stable => Ok(None),
            Channel::Beta => UpdateAddress::parse(BETA_FEED_URL),
            Channel::Custom(name) => {
                let slot = self
                    .custom
                    .iter()
                    .find(|s| s.name == name.as_str())
                    .ok_or_else(|| format!("unknown custom slot: {name}", name = name.as_str()))?;
                if self
                    .custom
                    .iter()
                    .filter(|s| s.name == name.as_str())
                    .count()
                    != 1
                {
                    return Err(format!("duplicate custom slot: {}", name.as_str()));
                }
                UpdateAddress::parse(&slot.url)?
                    .map(Some)
                    .ok_or_else(|| "a custom slot URL cannot be empty".into())
            }
            Channel::Nightly => Err("Nightly is not published yet".into()),
            Channel::Local => Err("Local needs this gateway's origin".into()),
        }
    }

    pub fn key(&self) -> String {
        // Include slot contents so replacing a selected URL invalidates its answer.
        format!("{}:{:?}", self.channel, self.custom)
    }
}

#[derive(Deserialize)]
struct File {
    release_check: Option<Raw>,
}
#[derive(Deserialize)]
struct Raw {
    #[serde(default = "default_true")]
    enabled: bool,
    channel: Option<toml::Value>,
    update_address: Option<toml::Value>,
    #[serde(default)]
    custom: Vec<CustomSlot>,
}
fn default_true() -> bool {
    true
}
fn raw(value: toml::Value) -> String {
    match value {
        toml::Value::String(s) => s,
        other => other.to_string(),
    }
}

pub fn parse_updates_toml(contents: &str) -> UpdatesToml {
    match toml::from_str::<File>(contents) {
        Ok(File {
            release_check: Some(rc),
        }) => {
            let mut cfg = UpdatesToml {
                enabled: rc.enabled,
                custom: rc.custom,
                ..UpdatesToml::default()
            };
            if let Some(channel) = rc.channel {
                cfg.channel = raw(channel);
            } else if let Some(address) =
                rc.update_address.map(raw).filter(|a| !a.trim().is_empty())
            {
                cfg.custom.push(CustomSlot {
                    name: LEGACY_SLOT_NAME.into(),
                    url: address,
                });
                cfg.channel = format!("custom:{LEGACY_SLOT_NAME}");
            }
            cfg
        }
        Ok(_) => UpdatesToml::default(),
        Err(e) => {
            // A broken file must not silently turn a non-Stable pick into Stable.
            crate::log_legacy!("[ReleaseCheck] unreadable updates.toml: {e}");
            UpdatesToml {
                channel: "unreadable configuration".into(),
                ..UpdatesToml::default()
            }
        }
    }
}

pub fn render_updates_toml(cfg: &UpdatesToml) -> String {
    let mut out = format!(
        "# Lucidos update channels (ADR 0715).\n\
         # enabled controls automatic checks; manual checks remain available.\n\
         # Stable sends platform, architecture, version and IP (ADR 0108).\n\
         # channel: stable | beta | local | custom:<name>. Nightly is reserved.\n\
         # Custom slots survive switches. URLs require HTTPS or loopback HTTP.\n\
         [release_check]\nenabled = {}\nchannel = {}\n",
        cfg.enabled,
        toml::Value::String(cfg.channel.clone())
    );
    for slot in &cfg.custom {
        out.push_str(&format!(
            "\n[[release_check.custom]]\nname = {}\nurl = {}\n",
            toml::Value::String(slot.name.clone()),
            toml::Value::String(slot.url.clone())
        ));
    }
    out
}

pub fn validate_slot(slot: &CustomSlot) -> Result<CustomSlot, String> {
    CustomName::parse(&slot.name)?;
    let address = UpdateAddress::parse(&slot.url)?.ok_or("a custom slot URL cannot be empty")?;
    Ok(CustomSlot {
        name: slot.name.clone(),
        url: address.to_string(),
    })
}
