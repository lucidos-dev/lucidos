use serde_json::json;

use crate::http::{client as http_client, send_and_print};
use crate::workspace::{BoxError, Workspace};

fn calling_thread() -> Result<String, BoxError> {
    std::env::var("LUCIDOS_THREAD_ID").map_err(|_| {
        "LUCIDOS_THREAD_ID is not set, so there is no thread whose subscriptions these \
         act on. This subcommand only works from inside a Lucidos coding-agent session."
            .into()
    })
}

pub(crate) fn cmd_event_waits_list(ws: &Workspace) -> Result<(), BoxError> {
    let thread_id = calling_thread()?;
    let url = format!("{}/api/v1/threads/{}/event-waits", ws.base_url(), thread_id);
    send_and_print("GET", &url, http_client()?.get(&url))
}

pub(crate) fn cmd_event_waits_pending_cards(
    ws: &Workspace,
    wait_id: uuid::Uuid,
    after: i64,
) -> Result<(), BoxError> {
    let thread_id = calling_thread()?;
    let url = format!(
        "{}/api/v1/threads/{}/event-waits/{}/pending-cards",
        ws.base_url(),
        thread_id,
        wait_id,
    );
    send_and_print(
        "GET",
        &url,
        http_client()?.get(&url).query(&[("after", after)]),
    )
}

pub(crate) fn cmd_event_waits_cancel(
    ws: &Workspace,
    wait_id: Option<&str>,
    on: Option<&str>,
    all: bool,
) -> Result<(), BoxError> {
    let thread_id = calling_thread()?;
    let mut body = json!({});
    let fields = body.as_object_mut().expect("a fresh object");
    if let Some(id) = wait_id {
        fields.insert("wait_id".into(), json!(id));
    }
    if let Some(event_type) = on {
        fields.insert("on".into(), json!(event_type));
    }
    if all {
        fields.insert("all".into(), json!(true));
    }
    let url = format!(
        "{}/api/v1/threads/{}/event-waits/cancel",
        ws.base_url(),
        thread_id
    );
    send_and_print("POST", &url, http_client()?.post(&url).json(&body))
}
