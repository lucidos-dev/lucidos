//! The *widget embed* grammar (ADR 0415): `![label](app:<id>?params={...})`
//! in markdown draws a widget wherever a picture draws.
//!
//! This is the Rust definition; the frontend's `utils/widgetEmbed.ts` is the
//! TypeScript one. Both read `widget_embeds.fixture.json`, so an embed the
//! engine accepts is one the frontend draws, and the reverse.
//!
//! The params are a JSON object, written raw or percent-encoded. Raw JSON may
//! hold spaces and `)`, which a markdown link cannot, so this scans the text
//! itself rather than trusting a markdown parser.

use crate::engine::widgets::WidgetParams;

pub use crate::engine::thread_events::WidgetEmbed;

/// The text that opens an embed's link: the image form with an `app:` URL.
const OPEN: &str = "](app:";

/// One embed found in markdown: where it sits, and what it says.
#[derive(Debug, Clone, PartialEq)]
pub struct EmbedMatch {
    /// Byte range of the whole `![…](app:…)` in the scanned text.
    pub range: std::ops::Range<usize>,
    /// The embed, or why its syntax is broken.
    pub embed: Result<WidgetEmbed, String>,
}

/// Every embed in `text`, in order. An `app:` image whose id or params do not
/// parse is still a match, carrying the reason, so a caller can refuse it.
pub fn scan(text: &str) -> Vec<EmbedMatch> {
    let code = code_ranges(text);
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(rel) = text[from..].find(OPEN) {
        let open = from + rel;
        if let Some(range) = code.iter().find(|r| r.contains(&open)) {
            from = range.end;
            continue;
        }
        let Some(start) = label_start(text, open) else {
            from = open + OPEN.len();
            continue;
        };
        let label = &text[start + 2..open];
        let rest_at = open + OPEN.len();
        let (end, embed) = match parse_target(&text[rest_at..]) {
            Ok((len, app_id, params)) => (
                rest_at + len,
                Ok(WidgetEmbed {
                    app_id,
                    params,
                    label: Some(label.trim().to_string()).filter(|l| !l.is_empty()),
                }),
            ),
            Err((len, reason)) => (rest_at + len, Err(reason)),
        };
        let end = end.min(text.len());
        found.push(EmbedMatch {
            range: start..end,
            embed,
        });
        from = end.max(open + OPEN.len());
    }
    found
}

/// The byte ranges of code in `text`: a run of backticks up to the next run
/// of the same length, so an inline span and a fenced block alike. An embed
/// inside code is an example, not an embed, as markdown draws no picture
/// there. An unclosed run is not code.
fn code_ranges(text: &str) -> Vec<std::ops::Range<usize>> {
    let bytes = text.as_bytes();
    let run_at = |i: usize| bytes[i..].iter().take_while(|&&b| b == b'`').count();
    let mut ranges = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'`' {
            i += 1;
            continue;
        }
        let len = run_at(i);
        let mut j = i + len;
        let close = loop {
            match bytes[j..].iter().position(|&b| b == b'`') {
                None => break None,
                Some(rel) => {
                    let at = j + rel;
                    let n = run_at(at);
                    if n == len {
                        break Some(at + n);
                    }
                    j = at + n;
                }
            }
        };
        match close {
            Some(end) => {
                ranges.push(i..end);
                i = end;
            }
            None => i += len,
        }
    }
    ranges
}

/// Where the `![` before an `](app:` begins, if the label between them is a
/// single line with no `]`.
fn label_start(text: &str, open: usize) -> Option<usize> {
    let start = text[..open].rfind("![")?;
    let label = &text[start + 2..open];
    (!label.contains(']') && !label.contains('\n')).then_some(start)
}

/// Parse `<id>[?params=<json>])` after the `app:`. Returns the length up to
/// and including the closing `)`. A failure still reports a length, so the
/// scan moves past the broken embed.
fn parse_target(rest: &str) -> Result<(usize, String, Option<WidgetParams>), (usize, String)> {
    let id_len = rest
        .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')))
        .unwrap_or(rest.len());
    let app_id = &rest[..id_len];
    let skip_to_close = |at: usize| rest[at..].find(')').map_or(rest.len(), |i| at + i + 1);
    if app_id.is_empty() || app_id.starts_with('.') || app_id.contains("..") {
        return Err((
            skip_to_close(id_len),
            format!("'{app_id}' is not a widget id"),
        ));
    }
    let after_id = &rest[id_len..];
    if after_id.starts_with(')') {
        return Ok((id_len + 1, app_id.to_string(), None));
    }
    let Some(query) = after_id.strip_prefix("?params=") else {
        return Err((
            skip_to_close(id_len),
            format!("the embed for '{app_id}' must end after the id or carry ?params={{…}}"),
        ));
    };
    let query_at = id_len + "?params=".len();
    let (json, json_len) = match query.chars().next() {
        Some('{') => match balanced_object_len(query) {
            Some(len) => (query[..len].to_string(), len),
            None => {
                return Err((
                    rest.len(),
                    format!("the params for '{app_id}' are not a closed JSON object"),
                ))
            }
        },
        Some('%') => {
            let len = query.find(')').unwrap_or(query.len());
            match percent_decode(&query[..len]) {
                Some(decoded) => (decoded, len),
                None => {
                    return Err((
                        query_at + len + 1,
                        format!("the params for '{app_id}' are not valid percent-encoding"),
                    ))
                }
            }
        }
        _ => {
            return Err((
                skip_to_close(query_at),
                format!("the params for '{app_id}' must be a JSON object"),
            ))
        }
    };
    let close_at = query_at + json_len;
    if !rest[close_at..].starts_with(')') {
        return Err((
            skip_to_close(close_at),
            format!("the embed for '{app_id}' must close with ) right after its params"),
        ));
    }
    let params: WidgetParams = serde_json::from_str::<serde_json::Value>(&json)
        .ok()
        .and_then(|v| match v {
            serde_json::Value::Object(map) => Some(map.into_iter().collect()),
            _ => None,
        })
        .ok_or_else(|| {
            (
                close_at + 1,
                format!("the params for '{app_id}' are not a JSON object"),
            )
        })?;
    Ok((
        close_at + 1,
        app_id.to_string(),
        Some(params).filter(|p| !p.is_empty()),
    ))
}

/// The length of the JSON object at the start of `s`, string-aware, or `None`
/// when it never closes.
fn balanced_object_len(s: &str) -> Option<usize> {
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (i, c) in s.char_indices() {
        if in_string {
            match (escaped, c) {
                (true, _) => escaped = false,
                (false, '\\') => escaped = true,
                (false, '"') => in_string = false,
                _ => {}
            }
            continue;
        }
        match c {
            '"' => in_string = true,
            '{' | '[' => depth += 1,
            '}' | ']' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(i + 1);
                }
            }
            _ => {}
        }
    }
    None
}

fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = s.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// `text` with the embeds in `matches` cut out and the ends trimmed. `None`
/// when nothing else is left.
pub fn without_embeds(text: &str, matches: &[EmbedMatch]) -> Option<String> {
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for m in matches {
        out.push_str(&text[at..m.range.start]);
        at = m.range.end;
    }
    out.push_str(&text[at..]);
    let trimmed = out.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shared fixture: the TypeScript definition reads the same file.
    #[test]
    fn the_shared_fixture_parses_as_written() {
        let raw = include_str!("widget_embeds.fixture.json");
        let cases: Vec<serde_json::Value> = serde_json::from_str(raw).unwrap();
        assert!(!cases.is_empty());
        for case in cases {
            let markdown = case["markdown"].as_str().unwrap();
            let matches = scan(markdown);
            let expected = case["embeds"].as_array().unwrap();
            assert_eq!(matches.len(), expected.len(), "count for {markdown:?}");
            for (got, want) in matches.iter().zip(expected) {
                match (&got.embed, want.get("error")) {
                    (Err(_), Some(serde_json::Value::Bool(true))) => {}
                    (Ok(embed), None) => {
                        assert_eq!(
                            embed.app_id,
                            want["app_id"].as_str().unwrap(),
                            "{markdown:?}"
                        );
                        assert_eq!(
                            embed.label.as_deref(),
                            want["label"].as_str(),
                            "{markdown:?}"
                        );
                        let params = serde_json::to_value(&embed.params).unwrap();
                        assert_eq!(params, want["params"], "{markdown:?}");
                    }
                    (got, want) => panic!("{markdown:?}: got {got:?}, want error {want:?}"),
                }
            }
            assert_eq!(
                without_embeds(markdown, &matches).as_deref(),
                case["without_embeds"].as_str(),
                "{markdown:?}"
            );
        }
    }

    #[test]
    fn a_plain_picture_is_not_an_embed() {
        assert!(scan("![chart](artifacts/chart.png)").is_empty());
        assert!(scan("[link](app:player)").is_empty());
    }
}
