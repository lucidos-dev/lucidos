//! What the provider config subscriber rebuilds on, and what it ignores.

use crate::core::{PREF_LOCAL_BASE_URL, PREF_OPENCODE_FREE_ENABLED};
use crate::engine::event_bus::{BusEvent, SystemEvent};
use crate::engine::provider_config_trigger;

fn pref(key: &str) -> BusEvent {
    BusEvent::System(SystemEvent::PreferencesChanged {
        key: key.to_string(),
        value: Some("true".to_string()),
        actor: None,
    })
}

fn credential(service: &str) -> BusEvent {
    BusEvent::System(SystemEvent::CredentialUpdated {
        service_name: service.to_string(),
        actor: None,
    })
}

/// A provider credential still triggers a rebuild, which is the behaviour that
/// predates preference-driven providers.
#[test]
fn a_provider_credential_triggers_a_rebuild() {
    for service in crate::llm::PROVIDER_CREDENTIAL_SERVICES {
        let trigger = provider_config_trigger(&credential(service));
        assert!(trigger.is_some(), "{service}");
        assert!(trigger.unwrap().contains(service));
    }
}

/// The keyless tier has no credential, so its toggle is the only thing that can
/// install it. Without this arm, turning it on would need a restart.
#[test]
fn a_provider_preference_triggers_a_rebuild() {
    for key in [PREF_OPENCODE_FREE_ENABLED, PREF_LOCAL_BASE_URL] {
        let trigger = provider_config_trigger(&pref(key));
        assert!(trigger.is_some(), "{key}");
        assert!(trigger.unwrap().contains(key));
    }
}

/// Every other preference leaves the provider alone. The bus carries one on
/// almost every settings change, and rebuilding on each would discard warm
/// Vertex tokens for nothing.
#[test]
fn an_unrelated_preference_does_not_trigger_a_rebuild() {
    for key in ["theme", "timezone", "chat_model", "ui_scale"] {
        assert!(provider_config_trigger(&pref(key)).is_none(), "{key}");
    }
}

/// A credential for something that is not a provider is ignored too.
#[test]
fn a_non_provider_credential_does_not_trigger_a_rebuild() {
    for service in ["github", "google", "slack"] {
        assert!(
            provider_config_trigger(&credential(service)).is_none(),
            "{service}"
        );
    }
}
