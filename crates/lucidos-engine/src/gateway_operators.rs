//! Operators, as the gateway that spawned this engine knows them (ADR 0623).
//!
//! An *operator* is a person: a name, emails, and the paired devices they hold.
//! The gateway keeps them in its event log. This engine reads the list at boot
//! and again whenever the gateway posts `/api/v1/operators/changed`, and keeps
//! the last answer in memory. Events store device ids or an actor email only,
//! so every attribution resolves through [`current`] at read time.
//!
//! **An operator is identity without authority.** No handler may branch on it
//! to allow or refuse an action.
//!
//! A test must never read a real gateway, so the fetch runs only once
//! [`enable`] has, which `main` calls for a gateway-spawned engine.

use serde::{Deserialize, Serialize};
use std::sync::{Arc, OnceLock, RwLock};
use std::time::Duration;

/// The gateway route every engine reads, under its auth mount.
const OPERATORS_PATH: &str = "/~/api/v1/auth/operators";

const GATEWAY_TIMEOUT: Duration = Duration::from_secs(5);

static GATEWAY_PORT: OnceLock<String> = OnceLock::new();

/// Set once the gateway has answered, so boot can wait for the first answer.
static ANSWERED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static ANSWER_ARRIVED: tokio::sync::Notify = tokio::sync::Notify::const_new();

/// One person, with the devices they hold.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Operator {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub emails: Vec<String>,
    #[serde(default)]
    pub device_ids: Vec<String>,
}

/// A device relayed through a gateway pairing, with the other gateway's
/// operator for it: shown as "Sam via A".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelayedOperator {
    pub device_id: String,
    pub operator_id: String,
    pub name: String,
    pub via: String,
    /// The emails the other gateway holds for the person. A relayed person
    /// with an email a local person holds is that local person.
    #[serde(default)]
    pub emails: Vec<String>,
}

/// Who an actor turned out to be.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OperatorRef {
    pub id: String,
    /// "Sam", or "Sam via A" for a relayed device.
    pub name: String,
}

/// One person this gateway knows, for a picker such as the composer's mention
/// list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Person {
    pub id: String,
    /// "Sam", or "Sam via A" for a relayed person.
    pub name: String,
    /// False for a person known only by email. No push reaches them.
    pub has_device: bool,
}

/// The gateway's answer to `GET /~/api/v1/auth/operators`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperatorSnapshot {
    #[serde(default)]
    pub operators: Vec<Operator>,
    #[serde(default)]
    pub relayed: Vec<RelayedOperator>,
}

impl OperatorSnapshot {
    /// The person holding a device, local or relayed.
    pub fn operator_of_device(&self, device_id: &str) -> Option<OperatorRef> {
        if let Some(operator) = self
            .operators
            .iter()
            .find(|o| o.device_ids.iter().any(|d| d == device_id))
        {
            return Some(OperatorRef {
                id: operator.id.clone(),
                name: operator.name.clone(),
            });
        }
        let relayed = self.relayed.iter().find(|r| r.device_id == device_id)?;
        let name = self
            .local_of_relayed(relayed)
            .map_or(&relayed.name, |o| &o.name);
        Some(OperatorRef {
            id: self.relayed_person_id(relayed).to_string(),
            name: format!("{name} via {}", relayed.via),
        })
    }

    /// The local person a relayed person is: the one holding an email the
    /// other gateway signed for any of that person's devices. So one human
    /// paired to two gateways counts once.
    fn local_of_relayed(&self, relayed: &RelayedOperator) -> Option<&Operator> {
        let emails: std::collections::BTreeSet<String> = self
            .relayed
            .iter()
            .filter(|r| r.operator_id == relayed.operator_id)
            .flat_map(|r| r.emails.iter().map(|e| e.trim().to_lowercase()))
            .collect();
        self.operators
            .iter()
            .find(|o| o.emails.iter().any(|e| emails.contains(&e.to_lowercase())))
    }

    /// The person id a relayed device counts under.
    fn relayed_person_id<'a>(&'a self, relayed: &'a RelayedOperator) -> &'a str {
        self.local_of_relayed(relayed)
            .map_or(relayed.operator_id.as_str(), |o| o.id.as_str())
    }

    /// The id of every person this gateway knows, local and relayed.
    pub fn person_ids(&self) -> std::collections::BTreeSet<&str> {
        self.operators
            .iter()
            .map(|o| o.id.as_str())
            .chain(self.relayed.iter().map(|r| self.relayed_person_id(r)))
            .collect()
    }

    /// The person holding an email, matched without regard to case. An email
    /// no operator holds is a stranger's.
    pub fn operator_of_email(&self, email: &str) -> Option<OperatorRef> {
        let email = email.trim().to_lowercase();
        self.operators
            .iter()
            .find(|o| o.emails.iter().any(|e| e.to_lowercase() == email))
            .map(|o| OperatorRef {
                id: o.id.clone(),
                name: o.name.clone(),
            })
    }

    /// The person an operator id names, local or relayed, as a device of
    /// theirs would name them. A removed operator's id names nobody.
    pub fn operator_of_id(&self, operator_id: &str) -> Option<OperatorRef> {
        if let Some(operator) = self.operators.iter().find(|o| o.id == operator_id) {
            return Some(OperatorRef {
                id: operator.id.clone(),
                name: operator.name.clone(),
            });
        }
        let relayed = self
            .relayed
            .iter()
            .find(|r| self.relayed_person_id(r) == operator_id)?;
        self.operator_of_device(&relayed.device_id)
    }

    /// The person behind an event origin: a device's holder, or the operator
    /// holding a trigger fire's actor email. Every other origin is nobody.
    pub fn operator_of_origin(
        &self,
        origin: &crate::engine::thread_events::MessageOrigin,
    ) -> Option<OperatorRef> {
        use crate::engine::thread_events::{EngineReason, MessageOrigin};
        match origin {
            MessageOrigin::Device { device_id } => self.operator_of_device(device_id),
            MessageOrigin::Engine {
                reason:
                    EngineReason::Scheduler {
                        actor_email: Some(email),
                        ..
                    },
            } => self.operator_of_email(email),
            _ => None,
        }
    }

    /// How many people this gateway knows, local and relayed. Routing among
    /// people means something only from two.
    pub fn person_count(&self) -> usize {
        self.person_ids().len()
    }

    /// The name an agent reads a person's typed message under: the device's
    /// holder on a team gateway. A personal gateway, and a sender no operator
    /// holds, keep "User".
    pub fn speaker_of_device(&self, device_id: Option<&str>) -> String {
        device_id
            .filter(|_| self.person_count() >= 2)
            .and_then(|id| self.operator_of_device(id))
            .map_or_else(|| "User".to_string(), |o| o.name)
    }

    /// Every person this gateway knows, local and relayed, each once.
    pub fn people(&self) -> Vec<Person> {
        self.person_ids()
            .into_iter()
            .filter_map(|id| {
                let who = self.operator_of_id(id)?;
                Some(Person {
                    has_device: !self.devices_of(id).is_empty(),
                    id: who.id,
                    name: who.name,
                })
            })
            .collect()
    }

    /// The devices an operator holds, local and relayed. Empty for an unknown
    /// operator.
    pub fn devices_of(&self, operator_id: &str) -> Vec<&str> {
        let local = self
            .operators
            .iter()
            .filter(|o| o.id == operator_id)
            .flat_map(|o| o.device_ids.iter().map(String::as_str));
        let relayed = self
            .relayed
            .iter()
            .filter(|r| self.relayed_person_id(r) == operator_id)
            .map(|r| r.device_id.as_str());
        local.chain(relayed).collect()
    }
}

/// Whose read state a caller reads and writes: Notifications and Review.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reader {
    /// The shared read state. A gateway with fewer than two people, and any
    /// caller no operator holds, read as one team, as before operators.
    Team,
    /// One person on a team gateway: the device reading, and every device
    /// that person holds. A read on one is a read on all.
    Person {
        device_id: String,
        devices: Vec<String>,
    },
}

impl Reader {
    /// The reader behind a request's actor.
    pub fn of(
        actor: Option<&crate::engine::thread_events::MessageOrigin>,
        snapshot: &OperatorSnapshot,
    ) -> Self {
        use crate::engine::thread_events::MessageOrigin;
        let Some(MessageOrigin::Device { device_id }) = actor else {
            return Self::Team;
        };
        if snapshot.person_count() < 2 {
            return Self::Team;
        }
        match snapshot.operator_of_device(device_id) {
            Some(person) => Self::Person {
                device_id: device_id.clone(),
                devices: snapshot
                    .devices_of(&person.id)
                    .into_iter()
                    .map(str::to_string)
                    .collect(),
            },
            None => Self::Team,
        }
    }

    /// The person's devices, for a query parameter. `None` is the team.
    pub fn devices(&self) -> Option<&[String]> {
        match self {
            Self::Team => None,
            Self::Person { devices, .. } => Some(devices),
        }
    }
}

fn snapshot_slot() -> &'static RwLock<Arc<OperatorSnapshot>> {
    static SLOT: OnceLock<RwLock<Arc<OperatorSnapshot>>> = OnceLock::new();
    SLOT.get_or_init(RwLock::default)
}

/// The last operator list the gateway gave. Empty with no gateway.
pub fn current() -> Arc<OperatorSnapshot> {
    snapshot_slot()
        .read()
        .map(|s| Arc::clone(&s))
        .unwrap_or_default()
}

/// The operator list, or `None` while the gateway has not answered yet. Until
/// then a team reads as one person, so a decision that differs between the
/// two must wait. An engine with no gateway knows its list: it is empty.
pub fn known() -> Option<Arc<OperatorSnapshot>> {
    let waiting =
        GATEWAY_PORT.get().is_some() && !ANSWERED.load(std::sync::atomic::Ordering::SeqCst);
    (!waiting).then(current)
}

fn install(snapshot: OperatorSnapshot) {
    if let Ok(mut slot) = snapshot_slot().write() {
        *slot = Arc::new(snapshot);
    }
}

/// Read operators from the gateway that spawned this engine. No-op without
/// one. Reads in the background, retrying until the gateway answers: until
/// then a team gateway would read as one person.
pub fn enable() {
    let Ok(port) = std::env::var("LUCIDOS_GATEWAY_PORT") else {
        return;
    };
    let _ = GATEWAY_PORT.set(port.clone());
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        return;
    };
    handle.spawn(async move {
        read_until_answered(&port).await;
        ANSWERED.store(true, std::sync::atomic::Ordering::SeqCst);
        ANSWER_ARRIVED.notify_waiters();
    });
}

/// Read the list until the gateway answers, waiting longer after each
/// failure. Returns whether the answer differs from the last one.
async fn read_until_answered(port: &str) -> bool {
    let mut wait = Duration::from_secs(1);
    loop {
        match read_and_install(port).await {
            Ok(changed) => return changed,
            Err(e) => log!(
                "[Operators] could not read operators from the gateway, retrying in {}s: {}",
                wait.as_secs(),
                e
            ),
        }
        tokio::time::sleep(wait).await;
        wait = (wait * 2).min(MAX_RETRY_WAIT);
    }
}

/// Wait for the gateway's first answer, at most one gateway timeout, so the
/// engine does not serve a team as one person while it boots. Returns at once
/// without a gateway. Past the timeout the retry goes on in the background.
pub async fn await_first_answer() {
    if GATEWAY_PORT.get().is_none() {
        return;
    }
    let arrived = ANSWER_ARRIVED.notified();
    if ANSWERED.load(std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    if tokio::time::timeout(GATEWAY_TIMEOUT, arrived)
        .await
        .is_err()
    {
        log!("[Operators] serving before the gateway named its operators");
    }
}

/// The longest pause between two reads of a gateway that does not answer.
const MAX_RETRY_WAIT: Duration = Duration::from_secs(60);

/// Read the list and keep it. Returns whether it differs from the last one.
async fn read_and_install(port: &str) -> Result<bool, String> {
    let snapshot = fetch(port).await?;
    let changed = *current() != snapshot;
    install(snapshot);
    Ok(changed)
}

/// Read the list again, retrying until the gateway answers, so a signal is
/// never lost to one failed read. Returns whether the answer differs from the
/// last one.
pub async fn refresh() -> bool {
    match GATEWAY_PORT.get() {
        Some(port) => read_until_answered(port).await,
        None => false,
    }
}

async fn fetch(port: &str) -> Result<OperatorSnapshot, String> {
    let client = crate::gateway_auth::client_builder()
        .timeout(GATEWAY_TIMEOUT)
        .build()
        .map_err(|e| format!("could not build a gateway client: {e}"))?;
    let mut last_error = String::new();
    for scheme in crate::net_config::peer_scheme_order() {
        let url = format!("{scheme}://127.0.0.1:{port}{OPERATORS_PATH}");
        match client.get(&url).send().await {
            // A gateway that predates operators has none to give.
            Ok(response) if response.status() == reqwest::StatusCode::NOT_FOUND => {
                return Ok(OperatorSnapshot::default())
            }
            Ok(response) if response.status().is_success() => {
                return response.json().await.map_err(|e| e.to_string());
            }
            Ok(response) => last_error = format!("HTTP {}", response.status()),
            Err(e) => last_error = e.to_string(),
        }
    }
    Err(last_error)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot() -> OperatorSnapshot {
        serde_json::from_value(serde_json::json!({
            "operators": [
                { "id": "op-1", "name": "Sam", "emails": ["sam@example.com"], "device_ids": ["d1", "d2"] },
                { "id": "op-2", "name": "Alex", "emails": [], "device_ids": [] }
            ],
            "relayed": [
                { "device_id": "pair:d9", "operator_id": "pair:op-7", "name": "Robin", "via": "laptop" }
            ]
        }))
        .unwrap()
    }

    #[test]
    fn a_device_resolves_to_its_person_local_or_relayed() {
        let snapshot = snapshot();
        assert_eq!(snapshot.operator_of_device("d2").unwrap().name, "Sam");
        assert_eq!(
            snapshot.operator_of_device("pair:d9").unwrap().name,
            "Robin via laptop"
        );
        assert_eq!(snapshot.operator_of_device("nobody"), None);
    }

    #[test]
    fn an_operator_id_names_its_person_local_relayed_or_nobody() {
        let snapshot = snapshot();
        assert_eq!(snapshot.operator_of_id("op-2").unwrap().name, "Alex");
        assert_eq!(
            snapshot.operator_of_id("pair:op-7").unwrap().name,
            "Robin via laptop"
        );
        assert_eq!(snapshot.operator_of_id("op-removed"), None);
    }

    #[test]
    fn an_email_matches_its_person_whatever_the_case() {
        let snapshot = snapshot();
        assert_eq!(
            snapshot.operator_of_email(" SAM@Example.com").unwrap().id,
            "op-1"
        );
        assert_eq!(snapshot.operator_of_email("stranger@example.com"), None);
    }

    #[test]
    fn an_origin_resolves_through_its_device_or_its_actor_email() {
        use crate::engine::thread_events::{EngineReason, MessageOrigin};
        let snapshot = snapshot();
        let fire = |actor_email: Option<&str>| {
            MessageOrigin::engine(EngineReason::Scheduler {
                trigger_id: "t-1".into(),
                trigger_name: None,
                actor_email: actor_email.map(str::to_string),
            })
        };
        let device = MessageOrigin::Device {
            device_id: "d1".into(),
        };
        assert_eq!(snapshot.operator_of_origin(&device).unwrap().id, "op-1");
        assert_eq!(
            snapshot
                .operator_of_origin(&fire(Some("sam@example.com")))
                .unwrap()
                .id,
            "op-1"
        );
        assert_eq!(
            snapshot.operator_of_origin(&fire(Some("stranger@example.com"))),
            None
        );
        assert_eq!(snapshot.operator_of_origin(&fire(None)), None);
        assert_eq!(snapshot.operator_of_origin(&MessageOrigin::System), None);
    }

    #[test]
    fn a_person_with_no_devices_holds_none() {
        let snapshot = snapshot();
        assert!(snapshot.devices_of("op-2").is_empty());
        assert!(snapshot.devices_of("unknown").is_empty());
        assert_eq!(snapshot.devices_of("op-1").len(), 2);
    }

    #[test]
    fn the_people_list_names_everyone_once_and_marks_who_has_no_device() {
        let person = |id: &str, name: &str, has_device: bool| Person {
            id: id.into(),
            name: name.into(),
            has_device,
        };
        assert_eq!(
            snapshot().people(),
            vec![
                person("op-1", "Sam", true),
                person("op-2", "Alex", false),
                person("pair:op-7", "Robin via laptop", true),
            ]
        );

        let mut solo = snapshot();
        solo.operators.truncate(1);
        solo.relayed[0].emails = vec!["sam@example.com".into()];
        assert_eq!(solo.people(), vec![person("op-1", "Sam", true)]);
    }

    #[test]
    fn a_reader_is_a_person_only_on_a_team_gateway() {
        use crate::engine::thread_events::MessageOrigin;
        let device = |id: &str| MessageOrigin::Device {
            device_id: id.into(),
        };
        let team = snapshot();
        assert_eq!(
            Reader::of(Some(&device("d2")), &team),
            Reader::Person {
                device_id: "d2".into(),
                devices: vec!["d1".into(), "d2".into()],
            }
        );
        assert_eq!(Reader::of(Some(&device("stray")), &team), Reader::Team);
        assert_eq!(
            Reader::of(Some(&MessageOrigin::System), &team),
            Reader::Team
        );
        assert_eq!(Reader::of(None, &team), Reader::Team);

        let mut alone = team.clone();
        alone.operators.truncate(1);
        alone.relayed.clear();
        assert_eq!(Reader::of(Some(&device("d1")), &alone), Reader::Team);
    }

    /// One person who pairs their own two gateways: the laptop relays as
    /// "Sam via laptop" and signs Sam's email, which the server's Sam holds.
    #[test]
    fn a_relayed_person_with_a_local_email_is_that_person() {
        use crate::engine::thread_events::MessageOrigin;
        let mut solo = snapshot();
        solo.operators.truncate(1);
        solo.relayed[0].emails = vec!["SAM@example.com".into()];
        assert_eq!(solo.person_count(), 1, "one human counts once");
        let relayed = solo.operator_of_device("pair:d9").unwrap();
        assert_eq!(
            (relayed.id.as_str(), relayed.name.as_str()),
            ("op-1", "Sam via laptop"),
            "a merged device still says where it came from"
        );
        assert_eq!(solo.devices_of("op-1"), vec!["d1", "d2", "pair:d9"]);

        // A phone that relayed before the email was added merges too: the
        // emails of every device of that person count.
        let mut phone = solo.relayed[0].clone();
        phone.device_id = "pair:d8".into();
        phone.emails.clear();
        solo.relayed.push(phone);
        assert_eq!(solo.person_count(), 1);
        assert_eq!(solo.operator_of_device("pair:d8").unwrap().id, "op-1");
        let laptop = MessageOrigin::Device {
            device_id: "pair:d9".into(),
        };
        assert_eq!(Reader::of(Some(&laptop), &solo), Reader::Team);

        solo.relayed.truncate(1);
        solo.relayed[0].emails = vec!["robin@example.com".into()];
        assert_eq!(
            solo.person_count(),
            2,
            "someone else's email is someone else"
        );
    }

    #[tokio::test]
    async fn a_gateway_that_does_not_answer_is_an_error_not_an_empty_list() {
        let unused = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = unused.local_addr().unwrap().port().to_string();
        drop(unused);
        assert!(fetch(&port).await.is_err());
    }
}
