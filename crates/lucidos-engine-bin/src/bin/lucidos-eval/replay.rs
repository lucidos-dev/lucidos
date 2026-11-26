use lucidos_engine::core::prefs;
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};
use uuid::Uuid;

type Fallible<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

const PREVIEW_CHARS: usize = 200;

const AFTER_THE_REQUEST: [&str; 3] = ["ResponseGenerated", "ToolCalled", "ToolResult"];

#[derive(Serialize, Deserialize)]
pub struct Round {
    pub round: usize,
    pub capture: serde_json::Value,
    pub after: Vec<Happened>,
}

#[derive(Serialize, Deserialize)]
pub struct Happened {
    pub event_type: String,
    pub payload: serde_json::Value,
}

#[derive(Deserialize)]
struct Section {
    name: String,
    #[serde(default)]
    content: Option<String>,
    #[serde(default, alias = "char_count")]
    budget_delta_chars: usize,
    #[serde(default)]
    content_chars: Option<usize>,
    #[serde(default)]
    role: Option<String>,
}

impl Section {
    fn size(&self) -> (usize, bool) {
        match self.content_chars {
            Some(chars) => (chars, true),
            None => (self.budget_delta_chars, false),
        }
    }
}

pub async fn rounds(pool: &PgPool, thread_id: Uuid) -> Fallible<Vec<Round>> {
    let captures = sqlx::query(
        "SELECT sequence, payload::text AS payload FROM events \
          WHERE event_type = 'ContextCaptured' AND thread_id = $1 \
            AND payload->>'producer' = 'main_llm' \
          ORDER BY sequence",
    )
    .bind(thread_id)
    .fetch_all(pool)
    .await?;

    let mut rounds = Vec::new();
    for (index, row) in captures.iter().enumerate() {
        let sequence: i64 = row.try_get("sequence")?;
        let until: Option<i64> = captures
            .get(index + 1)
            .map(|next| next.try_get("sequence"))
            .transpose()?;
        rounds.push(Round {
            round: index + 1,
            capture: serde_json::from_str(row.try_get::<String, _>("payload")?.as_str())?,
            after: happened_between(pool, thread_id, sequence, until).await?,
        });
    }
    Ok(rounds)
}

async fn happened_between(
    pool: &PgPool,
    thread_id: Uuid,
    after: i64,
    until: Option<i64>,
) -> Fallible<Vec<Happened>> {
    let wanted: Vec<String> = AFTER_THE_REQUEST.iter().map(|t| t.to_string()).collect();
    let rows = sqlx::query(
        "SELECT event_type, payload::text AS payload FROM events \
          WHERE thread_id = $1 AND sequence > $2 \
            AND ($3::bigint IS NULL OR sequence < $3) \
            AND event_type = ANY($4) \
          ORDER BY sequence",
    )
    .bind(thread_id)
    .bind(after)
    .bind(until)
    .bind(&wanted)
    .fetch_all(pool)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(Happened {
                event_type: row.try_get("event_type")?,
                payload: serde_json::from_str(row.try_get::<String, _>("payload")?.as_str())?,
            })
        })
        .collect()
}

pub struct Options<'a> {
    pub round: Option<usize>,
    pub section: Option<&'a str>,
    pub raw: bool,
}

pub fn print_rounds(label: &str, rounds: &[Round], options: &Options) {
    if rounds.is_empty() {
        println!("{}", nothing_captured());
        return;
    }
    for round in rounds
        .iter()
        .filter(|round| options.round.is_none_or(|want| round.round == want))
    {
        print_round(label, round, options);
    }
}

fn nothing_captured() -> String {
    format!(
        "no captured rounds. Either the thread never reached the model, or the arm ran without \
         the `{}` preference, which leaves every section body empty.",
        prefs::CAPTURE_CONTEXT.key()
    )
}

fn print_round(label: &str, round: &Round, options: &Options) {
    println!("\n=== {label} round {} ===", round.round);
    if options.raw {
        println!(
            "{}",
            serde_json::to_string_pretty(&round.capture).unwrap_or_default()
        );
        for happened in &round.after {
            println!(
                "{}",
                serde_json::to_string_pretty(&happened.payload).unwrap_or_default()
            );
        }
        return;
    }
    print_request(&round.capture, options.section);
    print_after(&round.after);
}

fn print_request(capture: &serde_json::Value, only: Option<&str>) {
    let data = capture.get("data").unwrap_or(capture);
    println!(
        "  model {}  window {}  estimated {} tokens",
        text_at(data, "model"),
        number_at(data, "context_window"),
        number_at(data, "estimated_total_tokens"),
    );
    match data.get("usage") {
        Some(usage) => println!(
            "  usage     total in {}  out {}  cache read {}  cache write {}",
            number_at(usage, "input_tokens"),
            number_at(usage, "output_tokens"),
            number_at(usage, "cache_read_tokens"),
            number_at(usage, "cache_creation_tokens"),
        ),
        None => println!("  usage     not reported"),
    }
    println!("  trims     {}", trims(data));

    let mut sections: Vec<Section> = data
        .get("sections")
        .cloned()
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default();
    sections.sort_by_key(|section| std::cmp::Reverse(section.size().0));
    println!("  sections  {} largest first", sections.len());
    for section in &sections {
        print_section(section, only);
    }
    if !sections.is_empty() && sections.iter().all(|s| s.content.is_none()) {
        println!("      no bodies: this run captured names and sizes only");
    }
    if sections.iter().any(|s| !s.size().1) {
        println!("      * budget delta, not a region size: row predates the split");
    }
}

fn print_section(section: &Section, only: Option<&str>) {
    let wanted = only.is_some_and(|name| section.name.eq_ignore_ascii_case(name));
    let (size, measured) = section.size();
    println!(
        "   {:<32} {:>9} chars{} [{}]",
        section.name,
        size,
        if measured { " " } else { "*" },
        section.role.as_deref().unwrap_or("-"),
    );
    match (&section.content, wanted) {
        (Some(body), true) => println!("{body}"),
        (Some(body), false) if only.is_none() => println!("      {}", preview(body)),
        (None, true) => println!(
            "      no body: this arm ran without the full capture, or without `{}`",
            prefs::CAPTURE_CONTEXT.key()
        ),
        _ => {}
    }
}

fn print_after(after: &[Happened]) {
    if after.is_empty() {
        println!("  the model produced nothing after this request");
        return;
    }
    println!("  then");
    for happened in after {
        let data = happened.payload.get("data").unwrap_or(&happened.payload);
        match happened.event_type.as_str() {
            "ToolCalled" => println!(
                "   ToolCalled        {} {}",
                text_at(data, "name"),
                preview(&compact(data.get("args")))
            ),
            "ToolResult" => println!(
                "   ToolResult        {} {}",
                text_at(data, "name"),
                preview(&text_at(data, "result"))
            ),
            "ResponseGenerated" => println!(
                "   ResponseGenerated {}",
                preview(&text_at(data, "content"))
            ),
            other => println!("   {other:<17} {}", preview(&compact(Some(data)))),
        }
    }
}

fn trims(data: &serde_json::Value) -> String {
    let passes: Vec<String> = data
        .get("trim_passes")
        .and_then(|value| value.as_array())
        .map(|passes| passes.iter().map(|pass| format!("pass {pass}")).collect())
        .unwrap_or_default();
    match (
        passes.is_empty(),
        data.get("trimmed").and_then(|v| v.as_bool()),
    ) {
        (false, _) => passes.join(", "),
        (true, Some(true)) => "trimmed, and the row predates the per-pass record".to_string(),
        (true, _) => "none".to_string(),
    }
}

pub fn trim_passes(rounds: &[Round]) -> std::collections::BTreeMap<u64, usize> {
    let mut counts = std::collections::BTreeMap::new();
    for round in rounds {
        let data = round.capture.get("data").unwrap_or(&round.capture);
        let Some(passes) = data.get("trim_passes").and_then(|v| v.as_array()) else {
            continue;
        };
        for pass in passes.iter().filter_map(|p| p.as_u64()) {
            *counts.entry(pass).or_insert(0) += 1;
        }
    }
    counts
}

fn compact(value: Option<&serde_json::Value>) -> String {
    value.map_or_else(String::new, |v| {
        serde_json::to_string(v).unwrap_or_default()
    })
}

fn text_at(value: &serde_json::Value, key: &str) -> String {
    match value.get(key) {
        Some(serde_json::Value::String(text)) => text.clone(),
        Some(other) => compact(Some(other)),
        None => "-".to_string(),
    }
}

fn number_at(value: &serde_json::Value, key: &str) -> String {
    value
        .get(key)
        .and_then(|v| v.as_i64())
        .map_or_else(|| "-".to_string(), |n| n.to_string())
}

fn preview(text: &str) -> String {
    let one_line: String = text
        .chars()
        .map(|c| if c == '\n' { ' ' } else { c })
        .collect();
    let trimmed = one_line.trim();
    match trimmed.chars().count() > PREVIEW_CHARS {
        true => format!(
            "{}…",
            trimmed.chars().take(PREVIEW_CHARS).collect::<String>()
        ),
        false => trimmed.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn capture(trim_passes: Option<serde_json::Value>, trimmed: bool) -> serde_json::Value {
        let mut data = serde_json::json!({
            "model": "model-under-test",
            "context_window": 200_000,
            "estimated_total_tokens": 118_432,
            "trimmed": trimmed,
        });
        if let Some(passes) = trim_passes {
            data["trim_passes"] = passes;
        }
        data
    }

    #[test]
    fn a_round_names_the_passes_that_fired() {
        let data = capture(Some(serde_json::json!([1, 5])), true);
        assert_eq!(trims(&data), "pass 1, pass 5");
    }

    #[test]
    fn an_older_row_says_it_does_not_know_which_pass() {
        let data = capture(None, true);
        assert!(trims(&data).contains("predates"), "{}", trims(&data));
    }

    #[test]
    fn a_round_that_did_not_trim_says_none() {
        assert_eq!(trims(&capture(None, false)), "none");
        assert_eq!(trims(&capture(Some(serde_json::json!([])), false)), "none");
    }

    #[test]
    fn the_pass_tally_counts_rounds_rather_than_passes() {
        let rounds = vec![
            Round {
                round: 1,
                capture: capture(Some(serde_json::json!([1])), true),
                after: vec![],
            },
            Round {
                round: 2,
                capture: capture(Some(serde_json::json!([1, 5])), true),
                after: vec![],
            },
            Round {
                round: 3,
                capture: capture(None, false),
                after: vec![],
            },
        ];
        assert_eq!(trim_passes(&rounds), [(1, 2), (5, 1)].into_iter().collect());
    }

    #[test]
    fn a_wrapped_payload_reads_the_same_as_a_bare_one() {
        let bare = capture(Some(serde_json::json!([2])), true);
        let wrapped = serde_json::json!({"type": "ContextCaptured", "data": bare.clone()});
        let round = Round {
            round: 1,
            capture: wrapped,
            after: vec![],
        };
        assert_eq!(trim_passes(&[round]), [(2, 1)].into_iter().collect());
    }

    #[test]
    fn a_preview_is_one_line_and_bounded() {
        let long = "a\nb".repeat(500);
        let shown = preview(&long);
        assert!(!shown.contains('\n'));
        assert_eq!(shown.chars().count(), PREVIEW_CHARS + 1);
        assert_eq!(preview("  short  "), "short");
    }

    #[test]
    fn an_empty_thread_says_why_rather_than_printing_nothing() {
        assert!(nothing_captured().contains(prefs::CAPTURE_CONTEXT.key()));
    }

    #[test]
    fn the_event_filter_is_the_models_own_output() {
        assert!(AFTER_THE_REQUEST.contains(&"ToolCalled"));
        assert!(AFTER_THE_REQUEST.contains(&"ResponseGenerated"));
        assert!(!AFTER_THE_REQUEST.contains(&"ContextCaptured"));
    }
}
