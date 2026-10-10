//! Tests for the shared marketplace registry write path.
//!
//! The bug these pin: registering a marketplace persisted and committed, but
//! announced only `DataFileWritten` for a `config/` path, which no frontend arm
//! consumes. So an open Plugins panel never learned the list had changed. The
//! rename case (re-registering the same source under a new name) is the one
//! that surfaced it, and it is the one an `if created` guard would drop.

use super::*;

use crate::core::plugin_marketplaces::load_registry;
use crate::core::ArtifactManager;
use crate::engine::event_bus::{BusEvent, MockEventBus, SystemEvent};
use crate::engine::thread_events::{ActorMode, MessageOrigin, ThreadDirection};

const SOURCE: &str = "https://github.com/example-org/example-repo";

/// A workspace with a git repo, which `commit_data_path` needs.
fn workspace() -> (tempfile::TempDir, ArtifactManager) {
    let dir = tempfile::TempDir::new().unwrap();
    let am = ArtifactManager::new(dir.path().to_path_buf()).unwrap();
    (dir, am)
}

fn agent_actor() -> MessageOrigin {
    MessageOrigin::ThreadLink {
        thread_id: uuid::Uuid::new_v4(),
        title: None,
        spawning_event_id: None,
        mode: ActorMode::Agent,
        direction: ThreadDirection::Parent,
    }
}

/// Every `SystemEvent` the bus recorded, in emit order.
fn system_events(bus: &MockEventBus) -> Vec<SystemEvent> {
    bus.emitted_events()
        .into_iter()
        .filter_map(|e| match e {
            BusEvent::System(sys) => Some(sys),
            _ => None,
        })
        .collect()
}

fn registered(bus: &MockEventBus) -> Vec<(String, String, String, Option<MessageOrigin>)> {
    system_events(bus)
        .into_iter()
        .filter_map(|e| match e {
            SystemEvent::PluginMarketplaceRegistered {
                marketplace_id,
                name,
                source,
                actor,
            } => Some((marketplace_id, name, source, actor)),
            _ => None,
        })
        .collect()
}

fn removed_ids(bus: &MockEventBus) -> Vec<String> {
    system_events(bus)
        .into_iter()
        .filter_map(|e| match e {
            SystemEvent::PluginMarketplaceRemoved { marketplace_id, .. } => Some(marketplace_id),
            _ => None,
        })
        .collect()
}

fn data_file_written_paths(bus: &MockEventBus) -> Vec<String> {
    system_events(bus)
        .into_iter()
        .filter_map(|e| match e {
            SystemEvent::DataFileWritten { path, .. } => Some(path),
            _ => None,
        })
        .collect()
}

/// A first registration announces the entity event (so open panels refresh)
/// AND keeps the file-level `DataFileWritten` audit with its commit sha.
#[tokio::test]
async fn registering_announces_the_entity_event_and_the_file_write() {
    let (dir, am) = workspace();
    let bus = MockEventBus::new();

    let out = register_with_bus(dir.path(), &am, &bus, SOURCE, Some("Example plugins"), None)
        .await
        .expect("register");

    assert!(out.created, "a first registration must report created");
    assert_eq!(out.marketplace.name, "Example plugins");
    assert!(
        !out.commit.is_empty(),
        "the registry commit must be recorded"
    );

    let events = registered(&bus);
    assert_eq!(events.len(), 1, "exactly one PluginMarketplaceRegistered");
    assert_eq!(events[0].0, out.marketplace.id);
    assert_eq!(events[0].1, "Example plugins");
    assert_eq!(events[0].2, SOURCE);

    assert_eq!(
        data_file_written_paths(&bus),
        vec![crate::core::plugin_marketplaces::MARKETPLACES_DATA_PATH.to_string()],
        "the file-level audit event must survive alongside the entity event"
    );
}

/// The rename. Re-registering the SAME source under a new name is an upsert: it
/// mutates the existing entry rather than adding a second one, reports
/// `created: false`, and must announce again carrying the NEW name. This is the
/// exact sequence from the bug report (register, then rename), and the case a
/// `if created { announce }` guard would silently drop.
#[tokio::test]
async fn renaming_announces_again_with_the_new_name() {
    let (dir, am) = workspace();
    let bus = MockEventBus::new();

    let first = register_with_bus(dir.path(), &am, &bus, SOURCE, Some("example repo"), None)
        .await
        .expect("register");

    let renamed = register_with_bus(
        dir.path(),
        &am,
        &bus,
        SOURCE,
        Some("Example's plugins"),
        None,
    )
    .await
    .expect("rename");

    assert!(
        !renamed.created,
        "a re-registration is an upsert, not a create"
    );
    assert_eq!(
        renamed.marketplace.id, first.marketplace.id,
        "the same source must resolve to the same marketplace"
    );
    assert_eq!(
        renamed.marketplaces.len(),
        1,
        "a rename must not add a second marketplace"
    );

    let events = registered(&bus);
    assert_eq!(
        events.len(),
        2,
        "the rename must announce too, not just the create"
    );
    assert_eq!(events[1].1, "Example's plugins", "carrying the NEW name");

    // And the rename is what a reloading client would read back.
    let persisted = load_registry(dir.path()).expect("reload");
    assert_eq!(persisted.marketplaces[0].name, "Example's plugins");
}

/// Removal announces its own event, so the list drops the row live.
#[tokio::test]
async fn removing_announces_the_removal() {
    let (dir, am) = workspace();
    let bus = MockEventBus::new();

    let registration = register_with_bus(dir.path(), &am, &bus, SOURCE, None, None)
        .await
        .expect("register");

    let removal = unregister_with_bus(dir.path(), &am, &bus, &registration.marketplace.id, None)
        .await
        .expect("remove")
        .expect("a registered marketplace must be removable");

    assert!(removal.marketplaces.is_empty());
    assert_eq!(removed_ids(&bus), vec![registration.marketplace.id]);
}

/// Removing an id that was never registered is `Ok(None)`, not an error and not
/// an announcement. That is what lets the HTTP handler return 404 without
/// sniffing an error string, and it keeps a bogus id from broadcasting a
/// removal that never happened.
#[tokio::test]
async fn removing_an_unknown_marketplace_is_a_silent_no_op() {
    let (dir, am) = workspace();
    let bus = MockEventBus::new();

    let outcome = unregister_with_bus(dir.path(), &am, &bus, "no-such-id", None)
        .await
        .expect("an unknown id is not an error");

    assert!(outcome.is_none(), "an unknown id must report not-found");
    assert!(
        system_events(&bus).is_empty(),
        "a no-op must announce nothing at all"
    );
}

/// The actor threads through to the event, so the timeline distinguishes a
/// marketplace the user added in Settings from one an agent registered mid-chat.
#[tokio::test]
async fn the_actor_reaches_the_announced_event() {
    let (dir, am) = workspace();
    let bus = MockEventBus::new();
    let actor = agent_actor();

    register_with_bus(dir.path(), &am, &bus, SOURCE, None, Some(actor.clone()))
        .await
        .expect("register");

    let events = registered(&bus);
    assert_eq!(
        events[0].3,
        Some(actor),
        "the acting thread must be stamped on the entity event"
    );
}

/// An unusable source fails as `InvalidSource` (the handler's 400) and writes
/// nothing: no registry file change, no commit, no announcement.
#[tokio::test]
async fn an_invalid_source_is_rejected_without_writing_or_announcing() {
    let (dir, am) = workspace();
    let bus = MockEventBus::new();

    let outcome = register_with_bus(dir.path(), &am, &bus, "not a url", None, None).await;

    assert!(
        matches!(outcome, Err(MarketplaceWriteError::InvalidSource(_))),
        "a bad source must be reported as invalid input, not an engine failure"
    );
    assert!(system_events(&bus).is_empty());
    assert!(load_registry(dir.path())
        .expect("reload")
        .marketplaces
        .is_empty());
}
