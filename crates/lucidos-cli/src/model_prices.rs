use crate::http::{client as http_client, send_and_print};
use crate::workspace::{BoxError, Workspace};

pub(crate) fn cmd(ws: &Workspace) -> Result<(), BoxError> {
    let url = format!("{}/api/v1/model-prices", ws.base_url());
    send_and_print("GET", &url, http_client()?.get(&url))
}
