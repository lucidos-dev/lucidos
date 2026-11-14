use serde_json::Value;

use crate::http::{client as http_client, send_expect_json};
use crate::workspace::{BoxError, Workspace};

pub(crate) struct QueryFilters<'a> {
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

/// Clap kebab-cases `--byte-limit`, and the route takes `byte_limit`.
fn build_query_params(filters: &QueryFilters<'_>) -> Vec<(&'static str, String)> {
    let mut params: Vec<(&'static str, String)> = Vec::new();
    let mut push = |key, value: Option<String>| {
        if let Some(value) = value {
            params.push((key, value));
        }
    };
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

/// One `ts pid [label] msg` line per record, then a footer naming what the
/// budgets left out and where retention starts.
fn render(body: &Value) -> String {
    let text = |value: &Value| value.as_str().unwrap_or_default().to_string();
    let mut out = String::new();
    for line in body["lines"].as_array().into_iter().flatten() {
        out.push_str(&format!(
            "{} {} [{}] {}\n",
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
    if let Some(hint) = body["hint"].as_str() {
        out.push_str(&format!("-- {hint}\n"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_filter_reaches_the_route_under_its_snake_case_key() {
        let contains = vec!["[Voice]".to_string(), "pid".to_string()];
        let params = build_query_params(&QueryFilters {
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
}
