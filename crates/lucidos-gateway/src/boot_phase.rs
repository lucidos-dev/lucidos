#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BootPhase {
    ProvisioningDatabase,
    StartingEngine,
    Migrating,
    Recovering,
}

impl BootPhase {
    pub fn from_wire(s: &str) -> Option<Self> {
        match s {
            "provisioning-database" => Some(Self::ProvisioningDatabase),
            "starting-engine" => Some(Self::StartingEngine),
            "migrating" => Some(Self::Migrating),
            "recovering" => Some(Self::Recovering),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::ProvisioningDatabase => "Provisioning database…",
            Self::StartingEngine => "Starting engine…",
            Self::Migrating => "Running migrations…",
            Self::Recovering => "Recovering sessions…",
        }
    }
}

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
        assert_eq!(BootPhase::from_wire("downloading-memory-model"), None);
    }
}
