//! Users, as the gateway that spawned this engine knows them (ADR 0623).
//!
//! A *user* is a user the gateway knows: a name, emails, and the paired
//! devices they hold.
//! The gateway keeps them in its event log. This engine reads the list at boot
//! and again whenever the gateway signals an identity change, and keeps
//! the last answer in memory. Events store device ids or an actor email only,
//! so every attribution resolves through [`current`] at read time.
//!
//! **A user is identity without authority.** No handler may branch on it
//! to allow or refuse an action.
//!
//! A test must never read a real gateway, so the fetch runs only once
//! [`enable`] has, which `main` calls for a gateway-spawned engine.

use serde::{Deserialize, Serialize};
use std::sync::{Arc, OnceLock, RwLock};
use std::time::Duration;

/// The gateway route every engine reads, under its auth mount.
const USERS_PATH: &str = "/~/api/v1/auth/users";
// An older gateway serves this name. See "User rename wire aliases" in
// docs/temporary-measures.md.
const LEGACY_USERS_PATH: &str = "/~/api/v1/auth/operators";

const GATEWAY_TIMEOUT: Duration = Duration::from_secs(5);

static GATEWAY_PORT: OnceLock<String> = OnceLock::new();

/// Set once the gateway has answered, so boot can wait for the first answer.
static ANSWERED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static ANSWER_ARRIVED: tokio::sync::Notify = tokio::sync::Notify::const_new();

/// One user, with the devices they hold.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct User {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub emails: Vec<String>,
    #[serde(default)]
    pub device_ids: Vec<String>,
}

/// A device relayed through a gateway pairing, with the other gateway's
/// user for it. `via` is that gateway's label, not a device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelayedUser {
    pub device_id: String,
    #[serde(alias = "operator_id")]
    pub user_id: String,
    pub name: String,
    pub via: String,
    /// The emails the other gateway holds for the user. A relayed user
    /// with an email a local user holds is that local user.
    #[serde(default)]
    pub emails: Vec<String>,
}

/// Who an actor turned out to be.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UserRef {
    pub id: String,
    /// "Sam", or "Sam (A)" for a relayed user another user shares a name with.
    pub name: String,
}

/// One user this gateway knows, for a picker such as the composer's mention
/// list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UserListing {
    pub id: String,
    /// "Sam", or "Sam (A)" for a relayed user another user shares a name with.
    pub name: String,
    /// False for a user known only by email. No push reaches them.
    pub has_device: bool,
}

/// The gateway's answer to `GET /~/api/v1/auth/users`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserSnapshot {
    #[serde(default, alias = "operators")]
    pub users: Vec<User>,
    #[serde(default)]
    pub relayed: Vec<RelayedUser>,
}

impl UserSnapshot {
    /// The user holding a device, local or relayed.
    pub fn user_of_device(&self, device_id: &str) -> Option<UserRef> {
        if let Some(user) = self
            .users
            .iter()
            .find(|o| o.device_ids.iter().any(|d| d == device_id))
        {
            return Some(UserRef {
                id: user.id.clone(),
                name: user.name.clone(),
            });
        }
        let relayed = self.relayed.iter().find(|r| r.device_id == device_id)?;
        Some(UserRef {
            id: self.relayed_user_id(relayed).to_string(),
            name: self.relayed_display_name(relayed),
        })
    }

    /// How a relayed user reads: the local name once merged by email, else
    /// their own name. The gateway label joins it, "Sam (A)", only when
    /// another distinct user holds the same name (trimmed, any case).
    /// `gateway_user_names.json` pins this rule for the frontend copy.
    fn relayed_display_name(&self, relayed: &RelayedUser) -> String {
        if let Some(local) = self.local_of_relayed(relayed) {
            return local.name.clone();
        }
        let key = |name: &str| name.trim().to_lowercase();
        let own = key(&relayed.name);
        let name = relayed.name.trim();
        let shared = self.users.iter().any(|o| key(&o.name) == own)
            || self.relayed.iter().any(|r| {
                r.user_id != relayed.user_id
                    && self.local_of_relayed(r).is_none()
                    && key(&r.name) == own
            });
        if shared {
            format!("{name} ({})", relayed.via)
        } else {
            name.to_string()
        }
    }

    /// The local user a relayed user is: the one holding an email the
    /// other gateway signed for any of that user's devices. So one human
    /// paired to two gateways counts once.
    fn local_of_relayed(&self, relayed: &RelayedUser) -> Option<&User> {
        let emails: std::collections::BTreeSet<String> = self
            .relayed
            .iter()
            .filter(|r| r.user_id == relayed.user_id)
            .flat_map(|r| r.emails.iter().map(|e| e.trim().to_lowercase()))
            .collect();
        self.users
            .iter()
            .find(|o| o.emails.iter().any(|e| emails.contains(&e.to_lowercase())))
    }

    /// The user id a relayed device counts under.
    fn relayed_user_id<'a>(&'a self, relayed: &'a RelayedUser) -> &'a str {
        self.local_of_relayed(relayed)
            .map_or(relayed.user_id.as_str(), |o| o.id.as_str())
    }

    /// The id of every user this gateway knows, local and relayed.
    pub fn user_ids(&self) -> std::collections::BTreeSet<&str> {
        self.users
            .iter()
            .map(|o| o.id.as_str())
            .chain(self.relayed.iter().map(|r| self.relayed_user_id(r)))
            .collect()
    }

    /// The user holding an email, matched without regard to case. An email
    /// no user holds is a stranger's.
    pub fn user_of_email(&self, email: &str) -> Option<UserRef> {
        let email = email.trim().to_lowercase();
        self.users
            .iter()
            .find(|o| o.emails.iter().any(|e| e.to_lowercase() == email))
            .map(|o| UserRef {
                id: o.id.clone(),
                name: o.name.clone(),
            })
    }

    /// The user a user id names, local or relayed, as a device of
    /// theirs would name them. A removed user's id names nobody.
    pub fn user_of_id(&self, user_id: &str) -> Option<UserRef> {
        if let Some(user) = self.users.iter().find(|o| o.id == user_id) {
            return Some(UserRef {
                id: user.id.clone(),
                name: user.name.clone(),
            });
        }
        let relayed = self
            .relayed
            .iter()
            .find(|r| self.relayed_user_id(r) == user_id)?;
        self.user_of_device(&relayed.device_id)
    }

    /// The user behind an event origin: a device's holder, or the user
    /// holding a trigger fire's actor email. Every other origin is nobody.
    pub fn user_of_origin(
        &self,
        origin: &crate::engine::thread_events::MessageOrigin,
    ) -> Option<UserRef> {
        use crate::engine::thread_events::{EngineReason, MessageOrigin};
        match origin {
            MessageOrigin::Device { device_id } | MessageOrigin::SessionDevice { device_id } => {
                self.user_of_device(device_id)
            }
            MessageOrigin::Engine {
                reason:
                    EngineReason::Scheduler {
                        actor_email: Some(email),
                        ..
                    },
            } => self.user_of_email(email),
            _ => None,
        }
    }

    /// How many users this gateway knows, local and relayed. Routing among
    /// users means something only from two.
    pub fn user_count(&self) -> usize {
        self.user_ids().len()
    }

    /// The name an agent reads a user's typed message under: the device's
    /// holder on a team gateway. A personal gateway, and a sender no user
    /// holds, keep "User".
    pub fn speaker_of_device(&self, device_id: Option<&str>) -> String {
        device_id
            .filter(|_| self.user_count() >= 2)
            .and_then(|id| self.user_of_device(id))
            .map_or_else(|| "User".to_string(), |o| o.name)
    }

    /// Every user this gateway knows, local and relayed, each once.
    pub fn listings(&self) -> Vec<UserListing> {
        self.user_ids()
            .into_iter()
            .filter_map(|id| {
                let who = self.user_of_id(id)?;
                Some(UserListing {
                    has_device: !self.devices_of(id).is_empty(),
                    id: who.id,
                    name: who.name,
                })
            })
            .collect()
    }

    /// The devices a user holds, local and relayed. Empty for an unknown
    /// user.
    pub fn devices_of(&self, user_id: &str) -> Vec<&str> {
        let local = self
            .users
            .iter()
            .filter(|o| o.id == user_id)
            .flat_map(|o| o.device_ids.iter().map(String::as_str));
        let relayed = self
            .relayed
            .iter()
            .filter(|r| self.relayed_user_id(r) == user_id)
            .map(|r| r.device_id.as_str());
        local.chain(relayed).collect()
    }
}

/// Whose read state a caller reads and writes: Notifications and Review.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reader {
    /// The shared read state. A gateway with fewer than two users, and any
    /// caller no user holds, read as one team, as before users.
    Team,
    /// One user on a team gateway: the device reading, and every device
    /// that user holds. A read on one is a read on all.
    User {
        device_id: String,
        devices: Vec<String>,
    },
}

impl Reader {
    /// The reader behind a request's actor.
    pub fn of(
        actor: Option<&crate::engine::thread_events::MessageOrigin>,
        snapshot: &UserSnapshot,
    ) -> Self {
        use crate::engine::thread_events::MessageOrigin;
        let Some(MessageOrigin::Device { device_id }) = actor else {
            return Self::Team;
        };
        if snapshot.user_count() < 2 {
            return Self::Team;
        }
        match snapshot.user_of_device(device_id) {
            Some(user) => Self::User {
                device_id: device_id.clone(),
                devices: snapshot
                    .devices_of(&user.id)
                    .into_iter()
                    .map(str::to_string)
                    .collect(),
            },
            None => Self::Team,
        }
    }

    /// The user's devices, for a query parameter. `None` is the team.
    pub fn devices(&self) -> Option<&[String]> {
        match self {
            Self::Team => None,
            Self::User { devices, .. } => Some(devices),
        }
    }
}

fn snapshot_slot() -> &'static RwLock<Arc<UserSnapshot>> {
    static SLOT: OnceLock<RwLock<Arc<UserSnapshot>>> = OnceLock::new();
    SLOT.get_or_init(RwLock::default)
}

/// The last user list the gateway gave. Empty with no gateway.
pub fn current() -> Arc<UserSnapshot> {
    snapshot_slot()
        .read()
        .map(|s| Arc::clone(&s))
        .unwrap_or_default()
}

/// The user list, or `None` while the gateway has not answered yet. Until
/// then a team reads as one user, so a decision that differs between the
/// two must wait. An engine with no gateway knows its list: it is empty.
pub fn known() -> Option<Arc<UserSnapshot>> {
    let waiting =
        GATEWAY_PORT.get().is_some() && !ANSWERED.load(std::sync::atomic::Ordering::SeqCst);
    (!waiting).then(current)
}

fn install(snapshot: UserSnapshot) {
    if let Ok(mut slot) = snapshot_slot().write() {
        *slot = Arc::new(snapshot);
    }
}

/// Read users from the gateway that spawned this engine. No-op without
/// one. Reads in the background, retrying until the gateway answers: until
/// then a team gateway would read as one user.
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
                "[Users] could not read users from the gateway, retrying in {}s: {}",
                wait.as_secs(),
                e
            ),
        }
        tokio::time::sleep(wait).await;
        wait = (wait * 2).min(MAX_RETRY_WAIT);
    }
}

/// Wait for the gateway's first answer, at most one gateway timeout, so the
/// engine does not serve a team as one user while it boots. Returns at once
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
        log!("[Users] serving before the gateway named its users");
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

async fn fetch(port: &str) -> Result<UserSnapshot, String> {
    let client = crate::gateway_auth::client_builder()
        .timeout(GATEWAY_TIMEOUT)
        .build()
        .map_err(|e| format!("could not build a gateway client: {e}"))?;
    let mut last_error = String::new();
    for scheme in crate::net_config::gateway_scheme_order() {
        for path in [USERS_PATH, LEGACY_USERS_PATH] {
            let url = format!("{scheme}://127.0.0.1:{port}{path}");
            match client.get(&url).send().await {
                Ok(response) if response.status() == reqwest::StatusCode::NOT_FOUND => {
                    if path == LEGACY_USERS_PATH {
                        return Ok(UserSnapshot::default());
                    }
                }
                Ok(response) if response.status().is_success() => {
                    return response.json().await.map_err(|e| e.to_string());
                }
                Ok(response) => {
                    last_error = format!("HTTP {}", response.status());
                    break;
                }
                Err(e) => {
                    last_error = e.to_string();
                    break;
                }
            }
        }
    }
    Err(last_error)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot() -> UserSnapshot {
        serde_json::from_value(serde_json::json!({
            "users": [
                { "id": "op-1", "name": "Sam", "emails": ["sam@example.com"], "device_ids": ["d1", "d2"] },
                { "id": "op-2", "name": "Alex", "emails": [], "device_ids": [] }
            ],
            "relayed": [
                { "device_id": "pair:d9", "user_id": "pair:op-7", "name": "Robin", "via": "laptop" }
            ]
        }))
        .unwrap()
    }

    #[test]
    fn a_device_resolves_to_its_user_local_or_relayed() {
        let snapshot = snapshot();
        assert_eq!(snapshot.user_of_device("d2").unwrap().name, "Sam");
        assert_eq!(snapshot.user_of_device("pair:d9").unwrap().name, "Robin");
        assert_eq!(snapshot.user_of_device("nobody"), None);
    }

    #[test]
    fn an_user_id_names_its_user_local_relayed_or_nobody() {
        let snapshot = snapshot();
        assert_eq!(snapshot.user_of_id("op-2").unwrap().name, "Alex");
        assert_eq!(snapshot.user_of_id("pair:op-7").unwrap().name, "Robin");
        assert_eq!(snapshot.user_of_id("op-removed"), None);
    }

    #[test]
    fn an_email_matches_its_user_whatever_the_case() {
        let snapshot = snapshot();
        assert_eq!(
            snapshot.user_of_email(" SAM@Example.com").unwrap().id,
            "op-1"
        );
        assert_eq!(snapshot.user_of_email("stranger@example.com"), None);
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
        assert_eq!(snapshot.user_of_origin(&device).unwrap().id, "op-1");
        assert_eq!(
            snapshot
                .user_of_origin(&fire(Some("sam@example.com")))
                .unwrap()
                .id,
            "op-1"
        );
        assert_eq!(
            snapshot.user_of_origin(&fire(Some("stranger@example.com"))),
            None
        );
        assert_eq!(snapshot.user_of_origin(&fire(None)), None);
        assert_eq!(snapshot.user_of_origin(&MessageOrigin::System), None);
    }

    #[test]
    fn a_user_with_no_devices_holds_none() {
        let snapshot = snapshot();
        assert!(snapshot.devices_of("op-2").is_empty());
        assert!(snapshot.devices_of("unknown").is_empty());
        assert_eq!(snapshot.devices_of("op-1").len(), 2);
    }

    #[test]
    fn the_users_list_names_everyone_once_and_marks_who_has_no_device() {
        let user = |id: &str, name: &str, has_device: bool| UserListing {
            id: id.into(),
            name: name.into(),
            has_device,
        };
        assert_eq!(
            snapshot().listings(),
            vec![
                user("op-1", "Sam", true),
                user("op-2", "Alex", false),
                user("pair:op-7", "Robin", true),
            ]
        );

        let mut solo = snapshot();
        solo.users.truncate(1);
        solo.relayed[0].emails = vec!["sam@example.com".into()];
        assert_eq!(solo.listings(), vec![user("op-1", "Sam", true)]);
    }

    #[test]
    fn a_reader_is_a_user_only_on_a_team_gateway() {
        use crate::engine::thread_events::MessageOrigin;
        let device = |id: &str| MessageOrigin::Device {
            device_id: id.into(),
        };
        let team = snapshot();
        assert_eq!(
            Reader::of(Some(&device("d2")), &team),
            Reader::User {
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
        alone.users.truncate(1);
        alone.relayed.clear();
        assert_eq!(Reader::of(Some(&device("d1")), &alone), Reader::Team);
    }

    /// One user who pairs their own two gateways: the laptop relays Sam and
    /// signs Sam's email, which the server's Sam holds.
    #[test]
    fn a_relayed_user_with_a_local_email_is_that_user() {
        use crate::engine::thread_events::MessageOrigin;
        let mut solo = snapshot();
        solo.users.truncate(1);
        solo.relayed[0].emails = vec!["SAM@example.com".into()];
        assert_eq!(solo.user_count(), 1, "one human counts once");
        let relayed = solo.user_of_device("pair:d9").unwrap();
        assert_eq!(
            (relayed.id.as_str(), relayed.name.as_str()),
            ("op-1", "Sam"),
            "a merged device shows the local name"
        );
        assert_eq!(solo.devices_of("op-1"), vec!["d1", "d2", "pair:d9"]);

        // A phone that relayed before the email was added merges too: the
        // emails of every device of that user count.
        let mut phone = solo.relayed[0].clone();
        phone.device_id = "pair:d8".into();
        phone.emails.clear();
        solo.relayed.push(phone);
        assert_eq!(solo.user_count(), 1);
        assert_eq!(solo.user_of_device("pair:d8").unwrap().id, "op-1");
        let laptop = MessageOrigin::Device {
            device_id: "pair:d9".into(),
        };
        assert_eq!(Reader::of(Some(&laptop), &solo), Reader::Team);

        solo.relayed.truncate(1);
        solo.relayed[0].emails = vec!["robin@example.com".into()];
        assert_eq!(solo.user_count(), 2, "someone else's email is someone else");
    }

    #[test]
    fn relayed_names_follow_the_shared_fixture() {
        let cases: Vec<serde_json::Value> =
            serde_json::from_str(include_str!("gateway_user_names.json")).unwrap();
        assert!(!cases.is_empty());
        for case in cases {
            let snapshot: UserSnapshot = serde_json::from_value(case["snapshot"].clone()).unwrap();
            for (device_id, expected) in case["names"].as_object().unwrap() {
                assert_eq!(
                    snapshot.user_of_device(device_id).unwrap().name,
                    expected.as_str().unwrap(),
                    "{}: {device_id}",
                    case["case"]
                );
            }
        }
    }

    #[tokio::test]
    async fn an_older_gateway_keeps_local_and_relayed_users() {
        let (port, server) = legacy_gateway(
            axum::http::StatusCode::NOT_FOUND,
            axum::http::StatusCode::OK,
        )
        .await;
        let result = fetch(&port).await;
        server.abort();
        assert!(server.await.unwrap_err().is_cancelled());
        let snapshot = result.unwrap();
        assert_eq!(snapshot.user_count(), 2);
        assert_eq!(snapshot.user_of_device("sam-phone").unwrap().name, "Sam");
        assert_eq!(
            snapshot.user_of_device("pair:kim-phone").unwrap().name,
            "Kim"
        );
    }

    #[tokio::test]
    async fn only_two_missing_user_routes_mean_a_gateway_has_no_users() {
        for status in [
            axum::http::StatusCode::NOT_FOUND,
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
        ] {
            let (port, server) = legacy_gateway(axum::http::StatusCode::NOT_FOUND, status).await;
            let result = fetch(&port).await;
            server.abort();
            assert!(server.await.unwrap_err().is_cancelled());
            if status == axum::http::StatusCode::NOT_FOUND {
                assert_eq!(result.unwrap(), UserSnapshot::default());
            } else {
                assert!(result.is_err(), "a failed legacy read must retry");
            }
        }
    }

    #[tokio::test]
    async fn a_failed_current_route_never_falls_back_to_legacy_users() {
        let (port, server) = legacy_gateway(
            axum::http::StatusCode::FORBIDDEN,
            axum::http::StatusCode::OK,
        )
        .await;
        let result = fetch(&port).await;
        server.abort();
        assert!(server.await.unwrap_err().is_cancelled());
        assert!(result.is_err());
    }

    async fn legacy_gateway(
        current_status: axum::http::StatusCode,
        status: axum::http::StatusCode,
    ) -> (String, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port().to_string();
        let router = axum::Router::new()
            .route(
                USERS_PATH,
                axum::routing::get(move || async move { current_status }),
            )
            .route(
                "/~/api/v1/auth/operators",
                axum::routing::get(move || async move {
                    (
                        status,
                        axum::Json(serde_json::json!({
                            "operators": [{
                                "id": "sam", "name": "Sam", "device_ids": ["sam-phone"]
                            }],
                            "relayed": [{
                                "device_id": "pair:kim-phone", "operator_id": "pair:kim",
                                "name": "Kim", "via": "Remote"
                            }]
                        })),
                    )
                }),
            );
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        (port, server)
    }

    #[tokio::test]
    async fn a_gateway_that_does_not_answer_is_an_error_not_an_empty_list() {
        let unused = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = unused.local_addr().unwrap().port().to_string();
        drop(unused);
        assert!(fetch(&port).await.is_err());
    }
}
