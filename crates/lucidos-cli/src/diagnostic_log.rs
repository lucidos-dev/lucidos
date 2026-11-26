use serde_json::Value;

use crate::http::{client as http_client, send_expect_json};
use crate::workspace::{BoxError, Workspace};

pub(crate) struct QueryFilters<'a> {
    pub source: Option<&'a str>,
    pub since: Option<&'a str>,
    pub until: Option<&'a str>,
    pub contains: &'a [String],
    pub regex: Option<&'a str>,
    pub label: Option<&'a str>,
    pub pid: Option<u32>,
    pub limit: Option<u32>,
    pub byte_limit: Option<u32>,
}

pub(crate) fn cmd_query(
    ws: &Workspace,
    filters: QueryFilters<'_>,
    json: bool,
) -> Result<(), BoxError> {
    let url = format!("{}/api/v1/engine/diagnostic-log", ws.base_url());
    let req = http_client()?
        .get(&url)
        .query(&build_query_params(&filters));
    let body = send_expect_json("GET", &url, req)?;
    if json {
        println!("{body}");
    } else {
        print!("{}", render(&body));
    }
    Ok(())
}

fn build_query_params(filters: &QueryFilters<'_>) -> Vec<(&'static str, String)> {
    let mut params: Vec<(&'static str, String)> = Vec::new();
    let mut push = |key, value: Option<String>| {
        if let Some(value) = value {
            params.push((key, value));
        }
    };
    push("source", filters.source.map(str::to_string));
    push("since", filters.since.map(str::to_string));
    push("until", filters.until.map(str::to_string));
    push("regex", filters.regex.map(str::to_string));
    push("label", filters.label.map(str::to_string));
    push("pid", filters.pid.map(|pid| pid.to_string()));
    push("limit", filters.limit.map(|n| n.to_string()));
    push("byte_limit", filters.byte_limit.map(|n| n.to_string()));
    for text in filters.contains {
        params.push(("contains", text.clone()));
    }
    params
}

fn render(body: &Value) -> String {
    let text = |value: &Value| value.as_str().unwrap_or_default().to_string();
    let mut out = String::new();
    for line in body["lines"].as_array().into_iter().flatten() {
        let source = line["source"]
            .as_str()
            .map_or(String::new(), |source| format!("[{source}] "));
        out.push_str(&format!(
            "{} {} {source}[{}] {}\n",
            text(&line["ts"]),
            line["pid"],
            text(&line["label"]),
            text(&line["msg"])
        ));
    }
    out.push_str(&format!(
        "-- {} lines, {} earlier matches left out{}; coverage starts {}\n",
        body["returned"].as_u64().unwrap_or_default(),
        body["earlier_matches"].as_u64().unwrap_or_default(),
        if body["truncated"].as_bool().unwrap_or_default() {
            ", cut by byte_limit"
        } else {
            ""
        },
        body["coverage_starts_at"]
            .as_str()
            .unwrap_or("nowhere: the log is empty"),
    ));
    for (source, status) in body["sources"].as_object().into_iter().flatten() {
        if let Some(status) = status.as_str().filter(|status| *status != "ok") {
            out.push_str(&format!("-- {source}: {status}\n"));
        }
    }
    if let Some(hint) = body["hint"].as_str() {
        out.push_str(&format!("-- {hint}\n"));
    }
    out
}

pub(crate) fn cmd_host_transfer(ws: &Workspace, path: &str, body: Value) -> Result<(), BoxError> {
    if path != "prepare" {
        let (id, suffix) = path.split_once('/').ok_or("Invalid host-log operation")?;
        if !id.starts_with("transfer-")
            || id.len() > 80
            || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
            || !matches!(suffix, "execute" | "status")
        {
            return Err("Invalid host-log operation".into());
        }
    }
    let url = format!("{}/api/v1/engine/host-log-transfer/{path}", ws.base_url());
    let client = crate::http::client_with_timeout(std::time::Duration::from_secs(230))?;
    let result = send_expect_json("POST", &url, client.post(&url).json(&body))?;
    println!("{result}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_filter_reaches_the_route_under_its_snake_case_key() {
        let contains = vec!["[Voice]".to_string(), "pid".to_string()];
        let params = build_query_params(&QueryFilters {
            source: Some("client"),
            since: Some("30m"),
            until: None,
            contains: &contains,
            regex: Some("a.b"),
            label: Some("Voice"),
            pid: Some(77210),
            limit: Some(50),
            byte_limit: Some(4096),
        });
        assert_eq!(
            params,
            vec![
                ("source", "client".to_string()),
                ("since", "30m".to_string()),
                ("regex", "a.b".to_string()),
                ("label", "Voice".to_string()),
                ("pid", "77210".to_string()),
                ("limit", "50".to_string()),
                ("byte_limit", "4096".to_string()),
                ("contains", "[Voice]".to_string()),
                ("contains", "pid".to_string()),
            ]
        );
    }

    #[test]
    fn records_print_one_per_line_with_a_footer() {
        let body = serde_json::json!({
            "lines": [{ "ts": "2026-11-12T10:00:00.000Z", "pid": 7, "label": "Engine", "msg": "[Voice] up" }],
            "returned": 1,
            "earlier_matches": 3,
            "truncated": true,
            "byte_size": 80,
            "coverage_starts_at": "2026-11-11T08:00:00+00:00",
            "hint": "narrow the filters",
        });
        assert_eq!(
            render(&body),
            "2026-11-12T10:00:00.000Z 7 [Engine] [Voice] up\n\
             -- 1 lines, 3 earlier matches left out, cut by byte_limit; \
             coverage starts 2026-11-11T08:00:00+00:00\n\
             -- narrow the filters\n"
        );
    }

    #[test]
    fn the_timeline_names_sources_and_the_footer_reports_only_unavailable_ones() {
        let body = serde_json::json!({
            "lines": [{
                "source": "engine", "ts": "2026-11-12T10:00:00Z",
                "pid": 7, "label": "Test", "msg": "engine evidence"
            }],
            "returned": 1,
            "sources": { "engine": "ok", "client": "unsupported", "gateway": "gateway-too-old" },
        });
        let text = render(&body);
        assert!(text.contains("7 [engine] [Test] engine evidence"));
        assert!(text.contains("-- client: unsupported\n"));
        assert!(text.contains("-- gateway: gateway-too-old\n"));
        assert!(!text.contains("-- engine:"));
    }
}

#[cfg(test)]
mod host_transfer_tests {
    use super::*;
    use std::io::{Read, Write};
    #[test]
    fn cli_posts_typed_operations_and_keeps_the_engine_permission_request() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let worker = std::thread::spawn(move || {
            let mut requests = vec![];
            for _ in 0..3 {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                    .unwrap();
                let mut buffer = [0; 8192];
                let mut data = String::new();
                loop {
                    let count = stream.read(&mut buffer).unwrap();
                    assert!(count > 0);
                    data.push_str(std::str::from_utf8(&buffer[..count]).unwrap());
                    if let Some((headers, body)) = data.split_once("\r\n\r\n") {
                        let len: usize = headers
                            .lines()
                            .find_map(|line| {
                                line.to_ascii_lowercase()
                                    .strip_prefix("content-length: ")
                                    .map(str::to_owned)
                            })
                            .unwrap()
                            .parse()
                            .unwrap();
                        if body.len() >= len {
                            break;
                        }
                    }
                }
                requests.push(data);
                let body = r#"{"request_id":"permission-request:fixture","operation":{"state":"prepared"}}"#;
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body).unwrap();
            }
            requests
        });
        let ws = Workspace {
            root: "/tmp/demo".into(),
            api_port: port,
            proto: "http".into(),
            api_base_override: Some(format!("http://127.0.0.1:{port}")),
        };
        cmd_host_transfer(&ws,"prepare",serde_json::json!({"user":"evidence","host":"logs.example.org","directory":"evidence/phone","reason":"diagnose crash"})).unwrap();
        cmd_host_transfer(
            &ws,
            "transfer-fixture/execute",
            serde_json::json!({"reason":"diagnose crash"}),
        )
        .unwrap();
        cmd_host_transfer(&ws, "transfer-fixture/status", serde_json::json!({})).unwrap();
        let requests = worker.join().unwrap();
        assert!(requests[0].starts_with("POST /api/v1/engine/host-log-transfer/prepare "));
        assert!(requests[0].contains("logs.example.org"));
        assert!(requests[1]
            .starts_with("POST /api/v1/engine/host-log-transfer/transfer-fixture/execute "));
        assert!(requests[2]
            .starts_with("POST /api/v1/engine/host-log-transfer/transfer-fixture/status "));
        for invalid in [
            "../transfer-x/execute",
            "transfer-x/execute?elsewhere",
            "transfer-x/status/extra",
        ] {
            assert!(cmd_host_transfer(&ws, invalid, serde_json::json!({})).is_err());
        }
    }
}
