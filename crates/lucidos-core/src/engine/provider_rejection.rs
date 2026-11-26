use serde::{Deserialize, Serialize};

pub const PROXY_CONFIG_REL_PATH: &str = "data/config/apis.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RejectedProvider {
    pub provider: Option<String>,
    pub reason: String,
}

impl RejectedProvider {
    /// INVARIANT: How this reads to a person: the provider name, or the config path
    /// when the file itself is what failed. Used by the startup log and the
    /// boot notification. `thread-sync.ts` mirrors the same fallback for the
    /// `provider: null` it receives on the wire.
    pub fn label(&self) -> &str {
        self.provider.as_deref().unwrap_or(PROXY_CONFIG_REL_PATH)
    }
}
