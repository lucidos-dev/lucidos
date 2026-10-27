//! `lucidos intelligence-ladder`: a *ladder surface*'s *intelligence
//! ladder*, as the engine serves it. Also how a plan card and a refusal show
//! it. The CLI keeps no copy of the ordering: every rung and band comes from
//! `GET /api/v1/intelligence-ladder` (ADR 0500, ADR 0525).

use crate::http::{client as http_client, ConnectRetry};
use crate::workspace::{BoxError, Workspace};

/// The reason slug the engine puts on a pick off the ladder.
pub(crate) const OFF_LADDER_REASON: &str = "off-intelligence-ladder";

/// One intelligence rung that is on, with its bands as the engine labels
/// them.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct IntelligenceRung {
    pub(crate) model: String,
    pub(crate) effort: String,
    pub(crate) label: String,
    pub(crate) intelligence: Option<String>,
    pub(crate) cost: Option<String>,
    pub(crate) time: Option<String>,
    pub(crate) is_default: bool,
}

/// A surface's effective ladder, least to most intelligent, and where the
/// calling thread stands on it. Rungs that are off stay out: no agent may
/// pick them.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Ladder {
    /// `chat`, `claude-code` or `codex`.
    pub(crate) surface: String,
    pub(crate) intelligence_rungs: Vec<IntelligenceRung>,
    /// The thread's rung, when it recorded one on the ladder.
    pub(crate) current: Option<usize>,
    /// The rung the bands compare against, as `model at effort`. Codex
    /// compares against Claude Code's default while it has none of its own.
    pub(crate) reference: Option<String>,
}

impl Ladder {
    pub(crate) fn parse(body: &serde_json::Value) -> Option<Self> {
        let served = body["rungs"].as_array()?;
        let current_served = body["current"]["position"]
            .as_u64()
            .and_then(|p| usize::try_from(p).ok());
        let mut intelligence_rungs = Vec::new();
        let mut current = None;
        for (i, r) in served.iter().enumerate() {
            if r["on"] != true {
                continue;
            }
            if current_served == Some(i) {
                current = Some(intelligence_rungs.len());
            }
            let text = |value: &serde_json::Value| value.as_str().map(str::to_string);
            intelligence_rungs.push(IntelligenceRung {
                model: text(&r["model"])?,
                effort: text(&r["effort"])?,
                label: text(&r["label"])?,
                intelligence: text(&r["intelligence"]["label"]),
                cost: text(&r["cost"]["label"]),
                time: text(&r["time"]["label"]),
                is_default: r["is_default"] == true,
            });
        }
        Some(Self {
            surface: body["surface"].as_str()?.to_string(),
            intelligence_rungs,
            current,
            reference: body["reference"]["model"]
                .as_str()
                .zip(body["reference"]["effort"].as_str())
                .map(|(model, effort)| format!("{model} at {effort}")),
        })
    }

    /// Whether a raise has somewhere to go: a rung above the thread's own
    /// that is not the top, which a builder never takes. Unknown when the
    /// thread is off the ladder or recorded nothing.
    pub(crate) fn has_higher_intelligence_rung(&self) -> bool {
        self.current
            .is_some_and(|p| p + 2 < self.intelligence_rungs.len())
    }

    /// The surface as the UI names it: the Lucidos Agent for `chat`.
    fn surface_name(&self) -> &str {
        if self.surface == CHAT_SURFACE {
            "Lucidos Agent"
        } else {
            &self.surface
        }
    }

    /// The ladder as numbered lines, with the default and the thread's rung
    /// marked.
    pub(crate) fn render(&self) -> String {
        if self.intelligence_rungs.is_empty() {
            return format!(
                "The {} intelligence ladder has no rung.",
                self.surface_name()
            );
        }
        let compared = match &self.reference {
            Some(reference) => {
                format!("Intelligence, cost and time per task are rough multiples of {reference}.")
            }
            None => {
                "No reference rung has an estimate, so intelligence, cost and time are not compared.".to_string()
            }
        };
        let mut out = format!(
            "The {} intelligence ladder, least to most intelligent. {compared}",
            self.surface_name()
        );
        for (i, step) in self.intelligence_rungs.iter().enumerate() {
            let mut marks = Vec::new();
            if step.is_default {
                marks.push("default");
            }
            if self.current == Some(i) {
                marks.push("this thread");
            }
            let marks = if marks.is_empty() {
                String::new()
            } else {
                format!(" [{}]", marks.join(", "))
            };
            out.push_str(&format!(
                "\n{}. {}, {} (`{}`): intelligence {}, cost {}, time {}{marks}",
                i + 1,
                step.label,
                step.effort,
                self.pick_hint(step),
                band(step.intelligence.as_deref()),
                band(step.cost.as_deref()),
                band(step.time.as_deref()),
            ));
        }
        out
    }

    /// How a pick names `step`: spawn flags for a coding agent, the
    /// `model=effort` pair a chat pick takes.
    fn pick_hint(&self, step: &IntelligenceRung) -> String {
        if self.surface == CHAT_SURFACE {
            format!("{}={}", step.model, step.effort)
        } else {
            format!(
                "--coding-agent-model {} --reasoning-effort {}",
                step.model, step.effort
            )
        }
    }
}

/// The wire name of the Lucidos Agent's surface.
const CHAT_SURFACE: &str = "chat";

/// A band label, or why there is none.
fn band(label: Option<&str>) -> &str {
    label.unwrap_or("not estimated")
}

/// GET the ladder route on `base_url`. `target_workspace` asserts another
/// workspace than the caller's own, as `spawn-thread --to` does.
fn fetch(
    base_url: &str,
    query: &[(&str, &str)],
    target_workspace: Option<&str>,
) -> Result<serde_json::Value, BoxError> {
    let url = format!("{base_url}/api/v1/intelligence-ladder");
    let mut request = http_client()?.get(&url).query(query);
    if let Some(target) = target_workspace {
        request = request.header(crate::http::HEADER_TARGET_WORKSPACE, target);
    }
    let resp = request
        .send_with_connect_retry()
        .map_err(|e| format!("GET {url} failed: {e}"))?;
    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().unwrap_or_default();
        return Err(format!("GET {url} returned {status}: {text}").into());
    }
    resp.json()
        .map_err(|e| format!("GET {url} returned a non-JSON body: {e}").into())
}

/// The calling thread's ladder, with its rung marked. `None` when the engine
/// cannot be asked, which a card reports rather than guesses around.
pub(crate) fn read_own(ws: &Workspace) -> Option<Ladder> {
    let thread_id = std::env::var("LUCIDOS_THREAD_ID").ok()?;
    fetch(&ws.base_url(), &[("thread_id", &thread_id)], None)
        .ok()
        .as_ref()
        .and_then(Ladder::parse)
}

/// The rendered ladder of `surface` to print under an
/// `off-intelligence-ladder` refusal, read from the engine that refused.
pub(crate) fn render_for_refusal(
    base_url: &str,
    target_workspace: Option<&str>,
    surface: Option<&str>,
) -> Option<String> {
    let query: Vec<(&str, &str)> = surface
        .map(|surface| vec![("surface", surface)])
        .unwrap_or_default();
    fetch(base_url, &query, target_workspace)
        .ok()
        .as_ref()
        .and_then(Ladder::parse)
        .map(|ladder| ladder.render())
}

/// `lucidos intelligence-ladder`: print the ladder. Inside a thread, its own
/// backend and rung unless `--surface` names another surface.
pub(crate) fn cmd(
    ws: &Workspace,
    surface: Option<crate::CliLadderSurface>,
    json: bool,
) -> Result<(), BoxError> {
    let thread_id = std::env::var("LUCIDOS_THREAD_ID").ok();
    let query: Vec<(&str, &str)> = match (surface, thread_id.as_deref()) {
        (Some(surface), _) => vec![("surface", surface.as_wire())],
        (None, Some(thread)) => vec![("thread_id", thread)],
        (None, None) => Vec::new(),
    };
    let body = fetch(&ws.base_url(), &query, None)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&body)?);
        return Ok(());
    }
    let ladder = Ladder::parse(&body)
        .ok_or_else(|| format!("the engine served a ladder this CLI cannot read: {body}"))?;
    println!("{}", ladder.render());
    if let (Some(source), Some(estimated)) = (body["source"].as_str(), body["estimated"].as_str()) {
        let customised = if body["customised"] == true {
            "The owner's own list orders it. "
        } else {
            ""
        };
        println!("{customised}Bands: {source} Estimated {estimated}.");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> serde_json::Value {
        serde_json::json!({
            "surface": "claude-code",
            "coding_agent": "claude-code",
            "rungs": [
                {"model": "claude-haiku-5-5", "effort": "low", "label": "Haiku 5.5", "on": true,
                 "intelligence": {"multiple": 0.6, "label": "0.6×"},
                 "cost": {"multiple": 0.0197, "label": "0.02×"},
                 "time": {"multiple": 0.397, "label": "0.4×"}, "is_default": false},
                {"model": "claude-sonnet-5-5", "effort": "low", "label": "Sonnet 5.5", "on": false,
                 "intelligence": {"multiple": 0.8, "label": "0.8×"},
                 "cost": {"multiple": 0.315, "label": "0.31×"},
                 "time": {"multiple": 0.5, "label": "0.5×"}, "is_default": false},
                {"model": "claude-opus-5-5", "effort": "medium", "label": "Opus 5.5", "on": true,
                 "intelligence": {"multiple": 1.0, "label": "1.0×"},
                 "cost": {"multiple": 1.0, "label": "1.0×"}, "time": null, "is_default": true},
            ],
            "current": {"model": "claude-opus-5-5", "effort": "medium", "position": 2},
            "reference": {"model": "claude-opus-5-5", "effort": "medium"},
        })
    }

    #[test]
    fn the_render_marks_the_default_and_the_threads_intelligence_rung() {
        let ladder = Ladder::parse(&sample()).expect("parses");
        let text = ladder.render();
        assert!(text.contains("1. Haiku 5.5, low (`--coding-agent-model claude-haiku-5-5 --reasoning-effort low`): intelligence 0.6×, cost 0.02×, time 0.4×"), "{text}");
        assert!(
            !text.contains("Sonnet"),
            "a rung that is off stays out: {text}"
        );
        assert!(text.contains("2. Opus 5.5, medium"), "{text}");
        assert!(
            text.contains(
                "intelligence 1.0×, cost 1.0×, time not estimated [default, this thread]"
            ),
            "{text}"
        );
        assert!(
            text.contains("rough multiples of claude-opus-5-5 at medium."),
            "{text}"
        );
        assert!(
            !ladder.has_higher_intelligence_rung(),
            "the thread is on the top rung"
        );
    }

    #[test]
    fn a_thread_off_the_ladder_has_no_known_higher_intelligence_rung() {
        let mut body = sample();
        body["current"] = serde_json::json!({"model": "x", "effort": "low", "position": null});
        let ladder = Ladder::parse(&body).expect("parses");
        assert_eq!(ladder.current, None);
        assert!(!ladder.has_higher_intelligence_rung());
        body["current"]["position"] = 1.into();
        assert_eq!(
            Ladder::parse(&body).expect("parses").current,
            None,
            "a thread on a rung that is off is off the ladder"
        );
        body["current"]["position"] = 0.into();
        assert!(
            !Ladder::parse(&body)
                .expect("parses")
                .has_higher_intelligence_rung(),
            "the only rung above is the top, which a builder never takes"
        );
    }

    #[test]
    fn a_chat_intelligence_rung_reads_as_its_pair_on_the_lucidos_agent_ladder() {
        let mut body = sample();
        body["surface"] = "chat".into();
        let text = Ladder::parse(&body).expect("parses").render();
        assert!(
            text.starts_with("The Lucidos Agent intelligence ladder"),
            "{text}"
        );
        assert!(
            text.contains("1. Haiku 5.5, low (`claude-haiku-5-5=low`)"),
            "{text}"
        );
    }
}
