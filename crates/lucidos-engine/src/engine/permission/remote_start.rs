use lucidos_gateway_stamp::remote_start::RemoteStartSetting;
use uuid::Uuid;

use crate::engine::permission::{Lifetime, PermissionChoice};
use crate::engine::thread_events::{MessageOrigin, RemoteStartAsk};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// INVARIANT: Held from a start card's open check through its saved setting and its
/// recorded answer. Two devices answering one card would otherwise save
/// one setting and record the other device's answer.
pub static ANSWER_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

pub fn remembered_setting(choice: &PermissionChoice) -> Option<RemoteStartSetting> {
    match choice {
        PermissionChoice::Grant {
            lifetime: Lifetime::Standing,
            ..
        } => Some(RemoteStartSetting::Allow),
        PermissionChoice::RefuseStanding => Some(RemoteStartSetting::Deny),
        _ => None,
    }
}

pub struct ThreadStart {
    pub origin_id: Uuid,
    pub payload: serde_json::Value,
}

impl ThreadStart {
    pub fn origin(&self) -> Option<MessageOrigin> {
        serde_json::from_value(self.payload.get("origin")?.clone()).ok()
    }

    pub fn pairing(&self) -> Option<RemoteStartAsk> {
        match self.origin()? {
            MessageOrigin::Workspace { remote_start, .. } => remote_start.map(|ask| *ask),
            _ => None,
        }
    }

    pub fn text_field(&self, key: &str) -> Option<String> {
        self.payload.get(key)?.as_str().map(str::to_string)
    }
}

pub async fn thread_start(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
) -> Result<Option<ThreadStart>, BoxError> {
    let row: Option<(Uuid, serde_json::Value)> = sqlx::query_as(
        "SELECT id, payload FROM events \
         WHERE thread_id = $1 AND event_type = 'MessageReceived' \
         ORDER BY sequence ASC LIMIT 1",
    )
    .bind(thread_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(origin_id, payload)| ThreadStart { origin_id, payload }))
}

pub async fn remember(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    setting: RemoteStartSetting,
) -> Result<(), BoxError> {
    let pairing = thread_start(pool, thread_id)
        .await?
        .and_then(|start| start.pairing())
        .ok_or("the thread's start names no pairing")?;
    let port = crate::core::base_path::gateway_port().ok_or("this workspace has no gateway")?;
    let url = format!(
        "{}/~/api/v1/auth/gateways/{}/remote-starts",
        lucidos_local_token::client::origin(&port),
        pairing.pairing_id
    );
    crate::gateway_auth::client_builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()?
        .put(url)
        .json(&serde_json::json!({ "remote_starts": setting }))
        .send()
        .await?
        .error_for_status()?;
    crate::log_legacy!(
        "[PermissionRequest] Set remote starts from '{}' to {}",
        pairing.pairing_label,
        setting.as_str()
    );
    Ok(())
}
