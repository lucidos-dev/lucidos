//! Boot-phase progress for the workspace boot splash (ADR 0014 §11).
//!
//! Opening a stopped/new workspace shows the gateway's "Workspace starting…"
//! 503 splash (see [`crate::proxy::starting_page`]) until the engine answers
//! `/api/v1/health`. The engine hasn't bound HTTP yet during its own startup, so
//! the gateway can only see `Health::Booting` for the whole wait. To make the
//! wait legible rather than opaque, the boot is split into named phases shown on
//! the splash:
//!
//!   * **gateway-observed** phases the gateway sets itself ([`ProvisioningDatabase`],
//!     [`StartingEngine`]) — see `server.rs`;
//!   * **engine-reported** phases the engine POSTs to
//!     `/~/api/v1/control/workspaces/:id/boot-phase` *during its own startup*,
//!     before its HTTP server is up ([`Migrating`], [`Recovering`]) —
//!     best-effort telemetry, see the engine's `report_boot_phase`.
//!
//! (The embedding model is NOT a boot phase — it loads in the background and
//! never blocks the engine's boot.)
//!
//! [`ProvisioningDatabase`]: BootPhase::ProvisioningDatabase
//! [`StartingEngine`]: BootPhase::StartingEngine
//! [`Migrating`]: BootPhase::Migrating
//! [`Recovering`]: BootPhase::Recovering

/// One named step of a workspace's cold boot, rendered as a human label on the
/// boot splash. Ordering of the variants follows the boot timeline.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BootPhase {
    /// Gateway-observed: provisioning the workspace's Postgres database.
    ProvisioningDatabase,
    /// Gateway-observed: engine process spawned, awaiting its first healthy probe.
    StartingEngine,
    /// Engine-reported: running database migrations.
    Migrating,
    /// Engine-reported: replaying recovery sweeps (sessions, queues, changes).
    Recovering,
}

impl BootPhase {
    /// Parse the kebab-case wire value the engine POSTs. Returns `None` for an
    /// unrecognized value so a newer engine reporting a phase this gateway
    /// doesn't know is tolerated (the splash just keeps the previous label)
    /// rather than rejected.
    pub fn from_wire(s: &str) -> Option<Self> {
        match s {
            "provisioning-database" => Some(Self::ProvisioningDatabase),
            "starting-engine" => Some(Self::StartingEngine),
            "migrating" => Some(Self::Migrating),
            "recovering" => Some(Self::Recovering),
            _ => None,
        }
    }

    /// The human label shown beneath the mark on the boot splash.
    pub fn label(self) -> &'static str {
        match self {
            Self::ProvisioningDatabase => "Provisioning database…",
            Self::StartingEngine => "Starting engine…",
            Self::Migrating => "Running migrations…",
            Self::Recovering => "Recovering sessions…",
        }
    }
}

/// The label shown when no phase is known yet (initial paint, or an unknown
/// phase string). Keeps the splash meaningful before the first phase lands.
pub const DEFAULT_LABEL: &str = "Workspace starting…";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_wire_values_round_trip_to_a_label() {
        for wire in [
            "provisioning-database",
            "starting-engine",
            "migrating",
            "recovering",
        ] {
            let phase = BootPhase::from_wire(wire).expect("known phase parses");
            assert!(!phase.label().is_empty());
        }
    }

    #[test]
    fn unknown_wire_value_is_none() {
        assert_eq!(BootPhase::from_wire("ready"), None);
        assert_eq!(BootPhase::from_wire(""), None);
        // The retired memory-model phase (the engine no longer reports it — the
        // model loads in the background) parses as unknown; the tolerant
        // `from_wire` keeps the splash on its previous label.
        assert_eq!(BootPhase::from_wire("downloading-memory-model"), None);
    }
}
