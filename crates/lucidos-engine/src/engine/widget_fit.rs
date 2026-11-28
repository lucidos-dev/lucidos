use crate::core::apps::WidgetSize;
use crate::core::prefs;
use crate::core::{AppManager, DeviceStore, PreferenceStore};
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::pending_answers::{await_answer, forget_answer, PendingAnswers};
use crate::engine::thread_events::device_actor;
use crate::engine::thread_events::{EventMeta, MessageOrigin, ThreadEvent};
use crate::engine::widget_fit_sessions::{FitScope, FitSessions};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::collections::{BTreeMap, HashMap};
use std::time::Duration;
use uuid::Uuid;

/// INVARIANT: How long the page may take over one box. It must cover the host's deadline
/// per size, `SETTLE_CAP_MS + FIT_ANSWER_SLACK_MS` (`WidgetFitCheck.tsx`).
/// `widget-fit-check.test.tsx` pins that.
pub const WIDGET_FIT_BOX_BUDGET: Duration = Duration::from_secs(6);

fn fit_wait(box_budget: Duration, sizes: usize) -> Duration {
    box_budget.saturating_mul(u32::try_from(sizes).unwrap_or(u32::MAX).saturating_add(1))
}

const MAX_NOTE_LINES: usize = 12;

const QUOTE_CHARS: usize = 60;

const RENDERER_CANDIDATES: i64 = 20;

const BOX_RULES: &str = "Box rules: each size is a fixed box that never scrolls. Fit it by \
     showing less, let data text wrap, and never cut it with an ellipsis \
     (system-knowhow/building-an-app/when-an-app-is-the-right-answer.md § The widget box).";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FitSurface {
    DeskTile,
    PhoneColumn,
}

impl FitSurface {
    pub const ALL: [Self; 2] = [Self::DeskTile, Self::PhoneColumn];

    fn label(self) -> &'static str {
        match self {
            Self::DeskTile => "desk tile",
            Self::PhoneColumn => "phone column",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FitKind {
    PastEdge,
    Clipped,
    Ellipsis,
    Media,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FitEdge {
    Top,
    Right,
    Bottom,
    Left,
}

impl FitEdge {
    fn as_str(self) -> &'static str {
        match self {
            Self::Top => "top",
            Self::Right => "right",
            Self::Bottom => "bottom",
            Self::Left => "left",
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct FitFinding {
    pub kind: FitKind,
    pub edge: FitEdge,
    pub px: f64,
    pub path: String,
    pub text: String,
}

#[derive(Debug, Clone, Copy, Deserialize)]
pub struct FrameSize {
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum FitOutcome {
    Measured {
        #[serde(rename = "box")]
        frame: FrameSize,
        findings: Vec<FitFinding>,
        total: usize,
    },
    Failed {
        error: String,
    },
}

#[derive(Debug, Clone, Deserialize)]
pub struct FitBoxReport {
    pub surface: FitSurface,
    pub size: WidgetSize,
    #[serde(flatten)]
    pub outcome: FitOutcome,
}

pub type PendingWidgetFits = PendingAnswers<Vec<FitBoxReport>>;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct FitScale {
    pub percent: f64,
    pub device_id: Option<String>,
}

/// INVARIANT: The keys a scale is read from, in the order the shell (`currentUiScale`)
/// and the SDK (`ui.applyPreferences`) read them. The last two are the
/// pre-grid names, which no migration moved.
const SCALE_KEYS: [&str; 3] = [prefs::UI_SCALE.key(), "text-size", "font-size"];

fn scale_value(prefs: &HashMap<String, String>) -> Option<&str> {
    SCALE_KEYS.iter().find_map(|key| {
        prefs
            .get(*key)
            .map(String::as_str)
            .filter(|v| !v.is_empty())
    })
}

fn parse_scale(raw: &str) -> Option<f64> {
    let (min, max) = prefs::UI_SCALE.bounds();
    match raw.trim() {
        "small" => Some(100.0),
        "medium" => Some(112.5),
        "large" => Some(125.0),
        number => number
            .parse::<f64>()
            .ok()
            .filter(|n| (min..=max).contains(n)),
    }
}

pub(crate) fn largest_scale(global: f64, device_values: &[(String, String)]) -> FitScale {
    let mut largest = FitScale {
        percent: global,
        device_id: None,
    };
    for (device_id, value) in device_values {
        let Some(percent) = parse_scale(value) else {
            continue;
        };
        if percent > largest.percent {
            largest = FitScale {
                percent,
                device_id: Some(device_id.clone()),
            };
        }
    }
    largest
}

pub(crate) async fn resolve_fit_scale(pool: &PgPool) -> Result<FitScale, sqlx::Error> {
    let workspace = PreferenceStore::get_all(pool).await?;
    let global = scale_value(&workspace)
        .and_then(parse_scale)
        .unwrap_or_else(|| prefs::UI_SCALE.resolve(None));
    let mut own: BTreeMap<String, HashMap<String, String>> = BTreeMap::new();
    for key in SCALE_KEYS {
        for (device_id, value) in PreferenceStore::device_values(pool, key).await? {
            own.entry(device_id)
                .or_default()
                .insert(key.to_string(), value);
        }
    }
    let device_values: Vec<(String, String)> = own
        .into_iter()
        .filter_map(|(device_id, rows)| {
            let mut merged = workspace.clone();
            merged.extend(rows);
            scale_value(&merged).map(|value| (device_id, value.to_string()))
        })
        .collect();
    Ok(largest_scale(global, &device_values))
}

pub(crate) fn fit_sizes(declared: &[WidgetSize]) -> Vec<WidgetSize> {
    WidgetSize::ALL
        .into_iter()
        .filter(|size| declared.contains(size))
        .collect()
}

async fn renderer(pool: &PgPool, preferred: Option<&str>) -> Result<String, String> {
    if let Some(id) = preferred {
        match DeviceStore::seen(pool, id).await {
            Ok(Some(seen)) if seen.visible_now => return Ok(id.to_string()),
            Ok(_) => {}
            Err(e) => log_legacy!("[WidgetFit] presence lookup failed for device {id}: {e}"),
        }
    }
    let seen = DeviceStore::recently_seen(pool, RENDERER_CANDIDATES, 1)
        .await
        .map_err(|e| format!("could not look up an open Lucidos window: {e}"))?;
    seen.into_iter()
        .find(|seen| seen.visible_now)
        .map(|seen| seen.id)
        .ok_or_else(|| "no Lucidos window is open to render it".to_string())
}

pub struct WidgetFitContext<'a> {
    pub bus: &'a EventBus,
    pub pool: &'a PgPool,
    pub apps: &'a AppManager,
    pub pending: &'a PendingWidgetFits,
}

pub enum FitResponder<'a> {
    Device(Option<&'a str>),
    AgentRenderer {
        sessions: &'a FitSessions,
        handle: &'a str,
        device: &'a str,
        actor: MessageOrigin,
    },
}

pub async fn check_widget_fit(
    cx: &WidgetFitContext<'_>,
    app_id: &str,
    thread_id: Uuid,
    preferred: Option<&str>,
    box_budget: Duration,
) -> String {
    check_widget_fit_with_responder(
        cx,
        app_id,
        thread_id,
        FitResponder::Device(preferred),
        box_budget,
    )
    .await
}

pub async fn check_widget_fit_with_responder(
    cx: &WidgetFitContext<'_>,
    app_id: &str,
    thread_id: Uuid,
    responder: FitResponder<'_>,
    box_budget: Duration,
) -> String {
    let app = match cx.apps.get_app(app_id) {
        Ok(app) => app,
        Err(e) => return not_checked(app_id, &format!("could not read the widget: {e}")),
    };
    if !app.is_widget() {
        return not_checked(app_id, "it is not a widget");
    }
    let sizes = fit_sizes(&app.sizes);
    let scale = match resolve_fit_scale(cx.pool).await {
        Ok(scale) => scale,
        Err(e) => return not_checked(app_id, &format!("could not read the UI scale: {e}")),
    };
    let request_id = Uuid::new_v4().to_string();
    let (device, rx, actor, renderer_session_id) = match &responder {
        FitResponder::Device(preferred) => {
            let device = match renderer(cx.pool, *preferred).await {
                Ok(device) => device,
                Err(reason) => return not_checked(app_id, &reason),
            };
            let rx = await_answer(cx.pending, &request_id, Some(&device));
            let actor = device_actor(&device);
            (device, rx, actor, None)
        }
        FitResponder::AgentRenderer {
            sessions,
            handle,
            device,
            actor,
        } => {
            let scope = FitScope {
                thread: thread_id,
                app: app_id.to_string(),
                device: device.to_string(),
            };
            let rx = match sessions.select(handle, &scope, &request_id) {
                Ok(rx) => rx,
                Err(reason) => return not_checked(app_id, reason),
            };
            (
                device.to_string(),
                rx,
                actor.clone(),
                Some(handle.to_string()),
            )
        }
    };
    log_legacy!("[WidgetFit] check thread={thread_id} app={app_id} device={device}");
    let emitted = cx
        .bus
        .emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::WidgetFitRequested {
                app_id: app_id.to_string(),
                request_id: request_id.clone(),
                sizes: sizes.clone(),
                ui_scale: scale.percent,
                scale_device_id: scale.device_id.clone(),
                renderer_device_id: renderer_session_id.as_ref().map(|_| device.clone()),
                renderer_session_id,
            },
            meta: EventMeta::with_actor(Some(actor)),
        })
        .await;
    if let Err(e) = emitted {
        forget_answer(cx.pending, &request_id);
        if let FitResponder::AgentRenderer {
            sessions, handle, ..
        } = &responder
        {
            sessions.forget(handle);
        }
        return not_checked(app_id, &format!("could not send the request: {e}"));
    }

    let timeout = fit_wait(box_budget, sizes.len());
    match tokio::time::timeout(timeout, rx).await {
        Ok(Ok(boxes)) => {
            let required: Vec<&str> = app
                .params
                .iter()
                .filter(|(_, param)| param.required)
                .map(|(name, _)| name.as_str())
                .collect();
            fit_note(app_id, &sizes, scale.percent, &required, &boxes)
        }
        Ok(Err(_)) => not_checked(app_id, "the request was dropped"),
        Err(_) => {
            forget_answer(cx.pending, &request_id);
            if let FitResponder::AgentRenderer {
                sessions, handle, ..
            } = &responder
            {
                sessions.forget(handle);
                return not_checked(
                    app_id,
                    &format!(
                        "the agent renderer did not answer within {} s",
                        timeout.as_secs()
                    ),
                );
            }
            let label = DeviceStore::friendly_name(cx.pool, &device).await;
            not_checked(
                app_id,
                &format!(
                    "the Lucidos window on \"{label}\" did not answer within {} s",
                    timeout.as_secs()
                ),
            )
        }
    }
}

fn own_file_app_id(path: &str) -> Option<&str> {
    let (app_id, file) = path.strip_prefix("apps/")?.split_once('/')?;
    let own = file == "manifest.json"
        || [".html", ".css", ".js"]
            .iter()
            .any(|ext| file.ends_with(ext));
    own.then_some(app_id).filter(|id| !id.is_empty())
}

pub fn written_widgets<'p>(
    apps: &AppManager,
    written: impl IntoIterator<Item = &'p str>,
) -> Vec<String> {
    let mut widgets: Vec<String> = Vec::new();
    for app_id in written.into_iter().filter_map(own_file_app_id) {
        if widgets.iter().any(|seen| seen == app_id) {
            continue;
        }
        if apps.get_app(app_id).is_ok_and(|app| app.is_widget()) {
            widgets.push(app_id.to_string());
        }
    }
    widgets
}

pub async fn widget_fit_notes(
    cx: &WidgetFitContext<'_>,
    widgets: &[String],
    thread_id: Uuid,
    preferred: Option<&str>,
    box_budget: Duration,
) -> String {
    let mut notes = String::new();
    for app_id in widgets {
        notes.push_str("\n\n");
        notes.push_str(&check_widget_fit(cx, app_id, thread_id, preferred, box_budget).await);
    }
    notes
}

pub(crate) fn not_checked(app_id: &str, reason: &str) -> String {
    format!("[WIDGET FIT] {app_id}: not checked: {reason}.\n{BOX_RULES}")
}

fn percent(scale: f64) -> String {
    if scale.fract() == 0.0 {
        format!("{scale:.0}")
    } else {
        scale.to_string()
    }
}

fn spoken_list(items: &[&str]) -> String {
    match items {
        [] => String::new(),
        [only] => only.to_string(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

fn quote(text: &str) -> String {
    let text: String = text.chars().take(QUOTE_CHARS).collect();
    format!("\"{text}\"")
}

fn finding_line(finding: &FitFinding) -> String {
    let px = finding.px.round();
    let edge = finding.edge.as_str();
    let what = if finding.text.is_empty() {
        format!("({})", finding.path)
    } else {
        format!("{} ({})", quote(&finding.text), finding.path)
    };
    match finding.kind {
        FitKind::Ellipsis => format!("an ellipsis cuts {what} by {px} px"),
        FitKind::PastEdge => format!("{what} runs {px} px past the {edge} edge"),
        FitKind::Clipped => {
            format!("{what} is clipped {px} px at its {edge} edge by an overflow: hidden ancestor")
        }
        FitKind::Media => format!("media {what} runs {px} px past the {edge} edge"),
    }
}

/// INVARIANT: The note for a page's answer. Every size on every surface must be
/// measured with nothing cut off for a pass. `required` names the params the
/// widget requires: the check loads it without them, so it never passes.
pub(crate) fn fit_note(
    app_id: &str,
    sizes: &[WidgetSize],
    scale: f64,
    required: &[&str],
    boxes: &[FitBoxReport],
) -> String {
    let scale = percent(scale);
    let expected: Vec<(FitSurface, WidgetSize)> = FitSurface::ALL
        .into_iter()
        .flat_map(|surface| sizes.iter().map(move |&size| (surface, size)))
        .collect();
    let mut cut = Vec::new();
    let mut unmeasured = Vec::new();
    let mut cut_boxes = 0;
    for &(surface, size) in &expected {
        let place = format!("{}, {}", size.as_str(), surface.label());
        let report = boxes
            .iter()
            .find(|b| b.surface == surface && b.size == size);
        match report.map(|b| &b.outcome) {
            None => unmeasured.push(format!("- {place}: not checked: the page sent no report")),
            Some(FitOutcome::Failed { error }) => {
                unmeasured.push(format!("- {place}: not checked: {error}"))
            }
            Some(FitOutcome::Measured {
                frame,
                findings,
                total,
            }) => {
                if *total == 0 && findings.is_empty() {
                    continue;
                }
                cut_boxes += 1;
                let frame = format!("{}x{} px", frame.width.round(), frame.height.round());
                for finding in findings {
                    cut.push(format!("- {place} ({frame}): {}", finding_line(finding)));
                }
                let more = total.saturating_sub(findings.len());
                if more > 0 {
                    cut.push(format!("- {place}: {more} more cut off"));
                }
            }
        }
    }

    let size_names: Vec<&str> = sizes.iter().map(|s| s.as_str()).collect();
    let fits = format!(
        "fits {} on the desk tile and the phone column, at {scale}% UI scale.",
        spoken_list(&size_names)
    );
    let without_params = (!required.is_empty()).then(|| {
        format!(
            "requires {}, so the check drew its face without them.",
            spoken_list(required)
        )
    });
    if cut.is_empty() && unmeasured.is_empty() {
        return match without_params {
            None => format!("[WIDGET FIT] {app_id}: {fits}"),
            Some(caveat) => {
                format!(
                    "[WIDGET FIT] {app_id}: not checked with params: it {caveat} That face {fits}"
                )
            }
        };
    }
    let head = if cut.is_empty() {
        format!(
            "[WIDGET FIT] {app_id}: not checked: {} of {} boxes could not be measured, at \
             {scale}% UI scale.",
            unmeasured.len(),
            expected.len()
        )
    } else {
        format!(
            "[WIDGET FIT] {app_id}: content is cut off in {cut_boxes} of {} boxes, at {scale}% \
             UI scale. Fix each line, then check again with the widgets fit action.",
            expected.len()
        )
    };
    let head = match without_params {
        None => head,
        Some(caveat) => format!("{head}\nIt {caveat}"),
    };
    let mut lines: Vec<String> = cut.into_iter().chain(unmeasured).collect();
    if lines.len() > MAX_NOTE_LINES {
        let hidden = lines.len() - (MAX_NOTE_LINES - 1);
        lines.truncate(MAX_NOTE_LINES - 1);
        lines.push(format!("- {hidden} more lines not shown"));
    }
    format!("{head}\n{}\n{BOX_RULES}", lines.join("\n"))
}

#[cfg(test)]
#[path = "widget_fit_tests.rs"]
mod tests;
