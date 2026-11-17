//! Workspace-client compatibility, independent of transport protocol and build ids.
//! Breaking API/client semantics bump VERSION. Additive changes keep it.
use serde::{Deserialize, Serialize};

pub const VERSION: u32 = 1;
const GATEWAY: &[&str] = &["trusted-remote-client", "opaque-remote-content"];
const ENGINE: &[&str] = &["workspace-api-v1"];

#[derive(Debug, Serialize, Deserialize)]
pub struct ClientContract {
    pub min: u32,
    pub max: u32,
    pub capabilities: Vec<String>,
}

impl ClientContract {
    fn new(capabilities: &[&str]) -> Self {
        Self {
            min: VERSION,
            max: VERSION,
            capabilities: capabilities.iter().map(|s| (*s).into()).collect(),
        }
    }

    pub fn gateway() -> Self {
        Self::new(GATEWAY)
    }
    pub fn engine() -> Self {
        Self::new(ENGINE)
    }

    fn supports(&self, required: &[&str]) -> bool {
        self.min <= VERSION
            && VERSION <= self.max
            && required
                .iter()
                .all(|r| self.capabilities.iter().any(|c| c == r))
    }

    pub fn supports_gateway(&self) -> bool {
        self.supports(GATEWAY)
    }
    pub fn supports_engine(&self) -> bool {
        self.supports(ENGINE)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gateway_support_never_certifies_engine_support() {
        assert!(ClientContract::gateway().supports_gateway());
        assert!(!ClientContract::gateway().supports_engine());
        assert!(ClientContract::engine().supports_engine());
        assert!(!ClientContract::engine().supports_gateway());
    }

    #[test]
    fn engine_requires_range_and_capabilities() {
        let mut c = ClientContract::engine();
        c.min = 0;
        c.max = 2;
        assert!(c.supports_engine());
        c.min = 2;
        assert!(!c.supports_engine());
        c.min = 1;
        c.max = 0;
        assert!(!c.supports_engine());
        c.max = 1;
        c.capabilities.clear();
        assert!(!c.supports_engine());
    }
}
