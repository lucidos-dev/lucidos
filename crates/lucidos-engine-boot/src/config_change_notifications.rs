//! Startup subscriber for config change notifications.

use lucidos_core::log_legacy;
use lucidos_engine::engine::config_change::{change_of, notice, CONFIG_CHANGE_EVENTS};
use lucidos_engine::engine::event_bus::{
    BusEvent, EmittedEvent, EventBus, LagReplay, ReplayScope, SystemEvent,
};
use lucidos_engine::gateway_users;
use sqlx::{types::Uuid, PgPool};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Post a notification for every config change somebody makes, for as long
/// as the bus runs.
pub fn spawn(bus: EventBus, pool: PgPool) -> tokio::task::JoinHandle<()> {
    let (mut rx, mut replay) = LagReplay::subscribe(
        &bus,
        ReplayScope::Workspace,
        Some(CONFIG_CHANGE_EVENTS.to_vec()),
    );
    tokio::spawn(async move {
        while let Some(frames) = replay.read(rx.recv().await, &pool).await {
            for frame in frames {
                if let Err(e) = announce(&bus, &frame).await {
                    log_legacy!(
                        "[ConfigChange] Cannot post a config change notification: {}",
                        e
                    );
                }
            }
        }
        log_legacy!("[ConfigChange] Config change stream ended");
    })
}

async fn announce(bus: &EventBus, frame: &EmittedEvent) -> Result<(), BoxError> {
    let (BusEvent::System(event), Some(_)) = (&frame.typed, frame.seq) else {
        return Ok(());
    };
    let Some(change) = change_of(event.event_type(), &event.to_payload()) else {
        return Ok(());
    };
    let Some(notice) = notice(&change, frame.event_id, &gateway_users::current()) else {
        return Ok(());
    };
    bus.emit(BusEvent::System(SystemEvent::NotificationCreated {
        id: Uuid::new_v4().to_string(),
        title: notice.title,
        message: notice.message,
        task_id: None,
        app_id: None,
        thread_id: None,
        event_id: None,
        tap: notice.tap,
        actor: None,
        audience: notice.audience,
        push_group: None,
    }))
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use lucidos_engine::engine::config_change::ConfigChange;
    use lucidos_engine::engine::thread_events::{agent_tool_actor, MessageOrigin};
    use lucidos_engine::gateway_users::{User, UserSnapshot};
    use lucidos_engine::scheduler::notifications::Tap;

    fn device(id: &str) -> MessageOrigin {
        MessageOrigin::Device {
            device_id: id.into(),
        }
    }

    fn person(id: &str, devices: &[&str]) -> User {
        User {
            id: id.into(),
            name: id.to_uppercase(),
            emails: vec![],
            device_ids: devices.iter().map(|d| d.to_string()).collect(),
        }
    }

    /// Sam holds two devices; Kim holds one.
    fn team() -> UserSnapshot {
        UserSnapshot {
            users: vec![
                person("sam", &["sam-phone", "sam-laptop"]),
                person("kim", &["kim-phone"]),
            ],
            relayed: vec![],
        }
    }

    #[test]
    fn a_change_notifies_everyone_but_its_maker_and_only_on_a_team() {
        let change = ConfigChange {
            title: "Repository added",
            what: "added the repository \"x\"".into(),
            actor: Some(device("sam-laptop")),
            undo: None,
        };
        let id = Uuid::new_v4();
        let notice = notice(&change, id, &team()).unwrap();
        assert_eq!(notice.message, "SAM added the repository \"x\".");
        assert_eq!(notice.tap, Tap::Modal, "no undo, no Undo tap");
        assert_eq!(notice.audience, Some(vec!["kim-phone".to_string()]));

        let agent = ConfigChange {
            actor: Some(agent_tool_actor(Uuid::new_v4())),
            ..change.clone()
        };
        let notice = super::notice(&agent, id, &team()).unwrap();
        assert_eq!(notice.message, "An agent added the repository \"x\".");
        assert_eq!(notice.audience, None, "nobody made it, so the team hears");

        let solo = UserSnapshot {
            users: vec![person("sam", &["sam-laptop"])],
            relayed: vec![],
        };
        assert_eq!(super::notice(&change, id, &solo), None);
        let nobody = ConfigChange {
            actor: None,
            ..change
        };
        assert_eq!(super::notice(&nobody, id, &team()), None);
    }
}
