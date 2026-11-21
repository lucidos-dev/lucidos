//! Workspace-client compatibility, independent of transport protocol and build ids.
//! Breaking API/client semantics bump VERSION. Additive changes keep it.
use serde::{Deserialize, Serialize};

pub const VERSION: u32 = 1;
const GATEWAY: &[&str] = &["trusted-remote-client", "opaque-remote-content"];
const ENGINE: &[&str] = &["workspace-api-v1"];
/// The engine calls its gateway at the https loopback origin. An older engine
/// calls plain http, which the gateway answers with a 301
/// (docs/temporary-measures.md, "Respawn engines older than the https loopback").
/// Optional, so a peer gateway still serves an older engine.
pub const LOOPBACK_TLS: &str = "loopback-tls";
/// The gateway checks a relayed request's signed contract binding itself, so
/// its peer may skip the per-request probes. Optional: older gateways lack it.
pub const CLAIM_BOUND_CONTRACT: &str = "claim-bound-contract";

#[derive(Debug, Clone, Serialize, Deserialize)]
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
        let mut contract = Self::new(GATEWAY);
        contract.capabilities.push(CLAIM_BOUND_CONTRACT.into());
        contract
    }
    pub fn engine() -> Self {
        let mut contract = Self::new(ENGINE);
        contract.capabilities.push(LOOPBACK_TLS.into());
        contract
    }

    pub fn has(&self, capability: &str) -> bool {
        self.capabilities.iter().any(|c| c == capability)
    }

    fn supports(&self, required: &[&str]) -> bool {
        self.min <= VERSION && VERSION <= self.max && required.iter().all(|r| self.has(r))
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
    fn an_engine_without_the_https_loopback_is_still_supported() {
        let mut older = ClientContract::engine();
        older.capabilities.retain(|c| c != LOOPBACK_TLS);
        assert!(older.supports_engine());
        assert!(ClientContract::engine().has(LOOPBACK_TLS));
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
