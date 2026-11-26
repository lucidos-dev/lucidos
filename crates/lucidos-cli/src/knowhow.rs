use crate::http::{client as http_client, send_and_print, send_expect_success_with_headers};
use crate::workspace::{BoxError, Workspace};

const HEADER_KNOWHOW_OVERSIZED: &str = "lucidos-knowhow-oversized";

pub(crate) fn cmd_list(ws: &Workspace) -> Result<(), BoxError> {
    let url = format!("{}/api/v1/knowhow", ws.base_url());
    send_and_print("GET", &url, http_client()?.get(&url))
}

pub(crate) fn cmd_read(ws: &Workspace, id: &str) -> Result<(), BoxError> {
    let url = format!("{}/api/v1/knowhow/read", ws.base_url());
    let (headers, body) = send_expect_success_with_headers(
        "GET",
        &url,
        http_client()?.get(&url).query(&[("id", id)]),
    )?;
    if let Some(note) = oversized_note(&headers, id) {
        println!("{note}\n");
    }
    println!("{body}");
    Ok(())
}

fn oversized_note(headers: &reqwest::header::HeaderMap, id: &str) -> Option<String> {
    let bytes: u64 = headers
        .get(HEADER_KNOWHOW_OVERSIZED)?
        .to_str()
        .ok()?
        .parse()
        .ok()?;
    Some(format!(
        "lucidos: `{id}` is {} KB, over the knowhow size cap, so your shell may cut it \
         before the end. Split it into an index plus parts (`lucidos knowhow read \
         system-knowhow/building-knowhow`, § Size).",
        bytes.div_ceil(1024)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_oversized_doc_is_noted_with_its_size_and_the_fix() {
        let mut headers = reqwest::header::HeaderMap::new();
        assert_eq!(oversized_note(&headers, "billing-api"), None);
        headers.insert(HEADER_KNOWHOW_OVERSIZED, "40000".parse().unwrap());
        let note = oversized_note(&headers, "billing-api").expect("an oversized doc is noted");
        assert!(note.contains("`billing-api` is 40 KB"), "{note}");
        assert!(note.contains("system-knowhow/building-knowhow"), "{note}");
        headers.insert(HEADER_KNOWHOW_OVERSIZED, "x".parse().unwrap());
        assert_eq!(oversized_note(&headers, "billing-api"), None);
    }
}
