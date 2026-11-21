//! Coding-agent model and effort choices, including Claude Code discovery.

use std::sync::Arc;

use crate::core::coding_agent::CodingAgent;

use super::intelligence_rungs::strip_version_pin;

/// The Claude Code profile a thread is bound to: its *account pin*.
///
/// Claude Code treats an unset `CLAUDE_CONFIG_DIR` as a different profile from
/// one set to its default path. The two read different keychain entries and
/// different `.claude.json` files, so only one of them holds the user's login.
/// A pinned spawn therefore replays exactly what turn 1 ran with.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum AccountPin {
    /// `CLAUDE_CONFIG_DIR` was unset. `dir` is Claude Code's default,
    /// `$HOME/.claude`, where the transcripts live.
    DefaultConfigDir { dir: String },
    /// `CLAUDE_CONFIG_DIR` was set to `dir`.
    ExplicitConfigDir { dir: String },
}

impl AccountPin {
    /// The pin a thread's first session establishes: the live
    /// `CLAUDE_CONFIG_DIR` when set, else Claude Code's default dir. `None`
    /// only when there is neither, which means `$HOME` is unset.
    pub fn for_first_session(live: Option<String>, default_dir: Option<String>) -> Option<Self> {
        match live {
            Some(dir) => Some(Self::ExplicitConfigDir { dir }),
            None => default_dir.map(|dir| Self::DefaultConfigDir { dir }),
        }
    }

    /// Resolve a recorded pin.
    ///
    /// `explicit` is `None` on rows written before the engine recorded it.
    /// Those rows stored `default_dir` for an unset variable too, so that one
    /// path is ambiguous. It counts as explicit only while the live env still
    /// sets exactly that path. Any other path can only have come from the
    /// variable.
    pub fn from_recorded(
        dir: String,
        explicit: Option<bool>,
        live: Option<&str>,
        default_dir: Option<&str>,
    ) -> Self {
        let explicit = explicit
            .unwrap_or_else(|| Some(dir.as_str()) != default_dir || live == Some(dir.as_str()));
        if explicit {
            Self::ExplicitConfigDir { dir }
        } else {
            Self::DefaultConfigDir { dir }
        }
    }

    /// Where Claude Code keeps this profile's transcripts.
    pub fn dir(&self) -> &str {
        match self {
            Self::DefaultConfigDir { dir } | Self::ExplicitConfigDir { dir } => dir,
        }
    }

    pub fn is_explicit(&self) -> bool {
        matches!(self, Self::ExplicitConfigDir { .. })
    }
}

/// One `/model` or `/effort` picker entry. Claude Code's model rows come from
/// discovery (`cc_model_discovery`) and `cc_menu_options.json`, merged by
/// [`offered_cc_model_options`]. Codex's come from `codex_menu_options.json`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct CcMenuOption {
    pub value: String,
    /// The concrete model this row runs, when the backend said so. A session
    /// reports this id at Init, and the thread records it, so a spawn must
    /// accept it too. Only discovered Claude Code rows carry it.
    #[serde(default)]
    pub resolved_model: Option<String>,
    pub label: String,
    pub description: String,
    /// Optional model compatibility metadata used by Codex reasoning efforts.
    /// Claude Code and universally supported Codex options leave it absent.
    #[serde(default)]
    pub supported_models: Option<Vec<String>>,
    /// The window a session on this model actually runs under, in tokens.
    ///
    /// Absent on almost every row, because it is an OVERRIDE of what
    /// `llm::model_registry::context_window_for` infers from the id. Declare it
    /// only where the backend's window differs, and read it through
    /// [`coding_agent_context_window`].
    ///
    /// The two answers differ because they describe different requests. The
    /// registry describes the one LUCIDOS makes, where 1M mode is gated on our
    /// own `[1m]` suffix. A coding agent makes its own request and picks its
    /// own context mode, so nothing here bounds a prompt the engine packs.
    #[serde(default)]
    pub context_window: Option<usize>,
    /// The tiers this model accepts, when the backend said so per model.
    /// `None` derives them from the effort rows (`efforts_for_model`).
    #[serde(default)]
    pub reasoning_efforts: Option<Vec<String>>,
    /// The effort a pick naming only this model runs at, when the backend
    /// said so per model. Only the Codex menu file carries it: Claude Code's
    /// answer comes from the model registry.
    #[serde(default)]
    pub default_effort: Option<String>,
    /// Whether a new spawn may pick this row. A row with `offered: false` is
    /// overlay only: it keeps its `context_window` for threads already on it,
    /// and is neither listed nor accepted.
    #[serde(default = "offered_by_default")]
    pub offered: bool,
}

fn offered_by_default() -> bool {
    true
}

#[derive(Debug, serde::Deserialize)]
struct CcMenuOptionsFile {
    models: Vec<CcMenuOption>,
    reasoning_efforts: Vec<CcMenuOption>,
}

const CC_MENU_OPTIONS_JSON: &str = include_str!("cc_menu_options.json");

static CC_MENU_OPTIONS: std::sync::LazyLock<CcMenuOptionsFile> = std::sync::LazyLock::new(|| {
    serde_json::from_str(CC_MENU_OPTIONS_JSON)
        .expect("cc_menu_options.json is malformed — see llm/cc_menu_options.json")
});

/// Every curated row in `cc_menu_options.json`, offered or not: the fallback
/// list until discovery succeeds, and the overlay that declares context windows.
pub fn curated_cc_model_options() -> &'static [CcMenuOption] {
    &CC_MENU_OPTIONS.models
}

static OFFERED_CURATED_CC_MODEL_OPTIONS: std::sync::LazyLock<std::sync::Arc<[CcMenuOption]>> =
    std::sync::LazyLock::new(|| offered_cc_model_options(None));

/// The models a new Claude Code spawn may pick, and the picker's source: what
/// discovery offers once Claude Code has answered, else the offered curated rows.
pub fn cc_model_options() -> std::sync::Arc<[CcMenuOption]> {
    super::cc_model_discovery::offered_options()
        .unwrap_or_else(|| OFFERED_CURATED_CC_MODEL_OPTIONS.clone())
}

/// Claude Code's own rows first, then the offered curated rows it did not list.
///
/// A model Claude Code accepts through `--model` can be absent from its list, so
/// the curated rows fill that gap. A discovered row wins on its value, which
/// keeps Claude Code's label and the `resolvedModel` it reported. A discovered
/// row whose value a curated `offered: false` row withdraws is dropped, because
/// a withdrawn model stays withdrawn whatever Claude Code lists.
///
/// A model Claude Code substituted stays offered: the owner decides whether
/// it leaves the ladder (`cc_model_discovery::is_refused`).
pub fn offered_cc_model_options(
    discovered: Option<&[CcMenuOption]>,
) -> std::sync::Arc<[CcMenuOption]> {
    let discovered = discovered.unwrap_or_default();
    let is_withdrawn = |value: &str| {
        curated_cc_model_options()
            .iter()
            .any(|c| !c.offered && c.value == value)
    };
    let kept = discovered.iter().filter(|d| !is_withdrawn(&d.value));
    let unlisted = curated_cc_model_options()
        .iter()
        .filter(|c| c.offered && !discovered.iter().any(|d| d.value == c.value));
    kept.chain(unlisted).cloned().collect()
}

/// A model id without its `[1m]` window or `@` pin suffix.
pub fn cc_model_base(id: &str) -> &str {
    id.split(['[', '@']).next().unwrap_or(id)
}

pub fn cc_reasoning_effort_options() -> &'static [CcMenuOption] {
    &CC_MENU_OPTIONS.reasoning_efforts
}

#[derive(Debug, serde::Deserialize)]
struct CodexMenuOptionsFile {
    models: Vec<CcMenuOption>,
    reasoning_efforts: Vec<CcMenuOption>,
}

const CODEX_MENU_OPTIONS_JSON: &str = include_str!("codex_menu_options.json");

static CODEX_MENU_OPTIONS: std::sync::LazyLock<CodexMenuOptionsFile> =
    std::sync::LazyLock::new(|| {
        serde_json::from_str(CODEX_MENU_OPTIONS_JSON)
            .expect("codex_menu_options.json is malformed — see llm/codex_menu_options.json")
    });

pub fn codex_model_options() -> &'static [CcMenuOption] {
    &CODEX_MENU_OPTIONS.models
}

pub fn codex_reasoning_effort_options() -> &'static [CcMenuOption] {
    &CODEX_MENU_OPTIONS.reasoning_efforts
}

/// The reasoning tiers `model` is offered, read out of the effort rows.
///
/// An effort with no `supported_models` is universal; one that names models is
/// offered only to those. The `default` row matches nothing by name, so it
/// takes the universal set. That is the right answer for a model the backend
/// has not resolved yet.
pub fn efforts_for_model(model: &str, efforts: &[CcMenuOption]) -> Vec<String> {
    efforts
        .iter()
        .filter(|e| {
            e.supported_models
                .as_ref()
                .is_none_or(|allowed| allowed.iter().any(|m| m == model))
        })
        .map(|e| e.value.clone())
        .collect()
}

/// The `/model` picker entries for one backend. Same data the frontend picker
/// renders, reached without the caller having to know which module owns it.
/// Claude Code's are its discovered rows once it has answered, then the offered
/// curated rows it did not list.
pub fn coding_agent_model_options(agent: CodingAgent) -> Arc<[CcMenuOption]> {
    match agent {
        CodingAgent::ClaudeCode => cc_model_options(),
        CodingAgent::Codex => CODEX_MODEL_OPTIONS.clone(),
    }
}

static CODEX_MODEL_OPTIONS: std::sync::LazyLock<Arc<[CcMenuOption]>> =
    std::sync::LazyLock::new(|| codex_model_options().into());

/// Every hand-kept row for one backend, offered or not: the source of the
/// offered rows Claude Code did not list, and the overlay that declares
/// context windows.
pub fn curated_coding_agent_model_options(agent: CodingAgent) -> &'static [CcMenuOption] {
    match agent {
        CodingAgent::ClaudeCode => curated_cc_model_options(),
        CodingAgent::Codex => codex_model_options(),
    }
}

/// The context window a coding-agent session runs `model` under, when the
/// backend's own window differs from what the model registry infers.
///
/// `None` means nothing is declared, and the caller falls back to
/// `context_window_for`. That fallback is right for the engine's own calls and
/// wrong for these: it answers 200k for most bare `claude-` ids, because LUCIDOS
/// gates 1M mode on its own `[1m]` suffix. Claude Code does not. A Sonnet 5
/// session runs 1M whatever we spell, so a capture rendered a real 240k prompt
/// as "203k / 200k (100%)" before this existed.
///
/// The windows are declared on the curated rows, which stay the overlay after
/// discovery. An alias is never followed to the model it runs: a legacy id
/// folds onto its alias (`normalize_cc_model_id`), so the alias's current model
/// says nothing about the session that reported it.
///
/// The lookup ignores a `@default` version pin. The curated file writes it on two
/// rows (`claude-opus-5@default`), and the agent strips it when it echoes the
/// model back. It deliberately does NOT ignore `[1m]`, which keeps a 1M row
/// distinct from its bare sibling. [`likely_intended_model`] documents the same
/// rule.
pub fn coding_agent_context_window(agent: CodingAgent, model: &str) -> Option<usize> {
    let target = strip_version_pin(model);
    curated_coding_agent_model_options(agent)
        .iter()
        .find(|o| strip_version_pin(&o.value) == target)
        .and_then(|o| o.context_window)
}

/// The `/effort` picker entries for one backend.
pub fn coding_agent_reasoning_effort_options(agent: CodingAgent) -> &'static [CcMenuOption] {
    match agent {
        CodingAgent::ClaudeCode => cc_reasoning_effort_options(),
        CodingAgent::Codex => codex_reasoning_effort_options(),
    }
}

/// Validate a caller-supplied model id against the backend that will run it.
///
/// Rejects an unknown id so callers can trust the reported model choice.
/// Previously `run_coding_agent` accepted the argument but used the settings default.
pub fn validate_coding_agent_model(
    agent: CodingAgent,
    model: Option<&str>,
) -> Result<Option<String>, String> {
    validate_model_against(agent, model, &coding_agent_model_options(agent))
}

pub fn validate_model_against(
    agent: CodingAgent,
    model: Option<&str>,
    options: &[CcMenuOption],
) -> Result<Option<String>, String> {
    let Some(model) = model.map(str::trim).filter(|m| !m.is_empty()) else {
        return Ok(None);
    };
    if offered_row(options, model).is_some() {
        return Ok(Some(model.to_string()));
    }
    let offered = options
        .iter()
        .map(describe_row)
        .collect::<Vec<_>>()
        .join(", ");
    let hint = likely_intended_model(model, options)
        .map(|likely| format!(", which spells that model '{likely}'"))
        .unwrap_or_default();
    Err(format!(
        "model '{model}' is not offered by {}{hint}. Choose one of: {offered}",
        agent.as_str(),
    ))
}

/// The picker row a caller's `model` names: the row with that value, else the
/// first row that runs that concrete model.
///
/// The second arm is what a caller copying a thread's model needs. The picker
/// sends `sonnet`, Claude Code reports `claude-sonnet-5[1m]` at Init, and the
/// thread records that. The recorded id names a model the picker offers, so it
/// must pass. Several rows can run one model (`default` and `opus` often do), and
/// then the first one answers.
pub fn offered_row<'a>(options: &'a [CcMenuOption], model: &str) -> Option<&'a CcMenuOption> {
    options.iter().find(|o| o.value == model).or_else(|| {
        options
            .iter()
            .find(|o| o.resolved_model.as_deref() == Some(model))
    })
}

/// One row as a refusal lists it: the value, and the model it runs when that
/// differs, so the caller sees the same pairing the picker shows.
fn describe_row(option: &CcMenuOption) -> String {
    match option.resolved_model.as_deref() {
        Some(runs) if runs != option.value => format!("{} (runs {runs})", option.value),
        _ => option.value.clone(),
    }
}

/// The single offered id that is a close misspelling of a rejected `model`,
/// or `None` when no exactly-one candidate is close enough to name.
///
/// Checked two ways, one at a time. First with a `@default` version pin
/// removed, the LUCIDOS CHAT picker's own decoration. Then with a trailing
/// `[1m]` context-window suffix removed.
///
/// Never both at once. Claude Code's own list pins `@default` on one row,
/// `claude-opus-5@default`. It pins `[1m]` on a different row for the same
/// model, `claude-opus-5[1m]`. Stripping both would collapse those two ids
/// into one stem and turn a clean match into a guess.
fn likely_intended_model<'a>(model: &str, options: &'a [CcMenuOption]) -> Option<&'a str> {
    unique_match_after(model, options, strip_version_pin).or_else(|| {
        unique_match_after(model, options, |id| {
            id.strip_suffix("[1m]").unwrap_or(id).to_string()
        })
    })
}

/// The one offered id whose value equals `strip(model)` once `strip` is
/// applied to it too. `None` when zero or more than one candidate ties.
fn unique_match_after<'a>(
    model: &str,
    options: &'a [CcMenuOption],
    strip: impl Fn(&str) -> String,
) -> Option<&'a str> {
    let target = strip(model);
    let mut matches = options.iter().filter(|o| strip(&o.value) == target);
    let first = matches.next()?;
    matches.next().is_none().then_some(first.value.as_str())
}

/// Validate a caller-supplied reasoning effort against the backend AND the
/// model that will run it. Refuses an out-of-vocabulary tier for the same
/// reason [`validate_coding_agent_model`] refuses an unknown model.
///
/// The model half exists because Codex declares some tiers per model, and its
/// driver DROPS a tier the model does not offer rather than failing: an
/// unsupported value kills the turn outright. The session still records the
/// tier that was asked for, so the picker would show one the turn never ran at.
/// Refusing the caller here is what keeps the two honest.
///
/// **An absent model is refused for such a tier, not waved through.** The
/// driver tests the restriction against the model in the request. So a
/// restricted tier with no model is dropped before the backend is consulted,
/// whatever its own config would have picked.
///
/// `validate_codex_effort` stays as the driver's last-resort guard. A
/// mid-session `set_reasoning_effort` and a resumed thread both reach it
/// without passing here.
pub fn validate_coding_agent_effort(
    agent: CodingAgent,
    model: Option<&str>,
    effort: Option<&str>,
) -> Result<Option<String>, String> {
    let Some(effort) = effort.map(str::trim).filter(|e| !e.is_empty()) else {
        return Ok(None);
    };
    let options = coding_agent_reasoning_effort_options(agent);
    // An absent model and the `default` sentinel both mean "the backend's own
    // config picks", and neither can match a name in `supported_models`.
    let model = model.map(str::trim).unwrap_or("");
    // A discovered row names its own tiers, and the picker offers exactly
    // those. Refusing anything else keeps the spawn and the picker in step.
    let row_tiers = offered_row(&coding_agent_model_options(agent), model)
        .and_then(|o| o.reasoning_efforts.clone());
    if let Some(tiers) = row_tiers {
        return effort_within_row_tiers(agent, model, effort, &tiers);
    }
    if efforts_for_model(model, options)
        .iter()
        .any(|o| o == effort)
    {
        return Ok(Some(effort.to_string()));
    }
    let Some(row) = options.iter().find(|o| o.value == effort) else {
        let offered = options
            .iter()
            .map(|o| o.value.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!(
            "reasoning_effort '{}' is not offered by {}. Choose one of: {}",
            effort,
            agent.as_str(),
            offered
        ));
    };
    // A row with no `supported_models` was already admitted above, so reaching
    // here means the tier names its models. Accept rather than render a refusal
    // with an empty list, which would be the one message a caller cannot act on.
    let Some(runs_on) = row.supported_models.as_ref() else {
        return Ok(Some(effort.to_string()));
    };
    Err(format!(
        "reasoning_effort '{}' runs only on these {} models: {}. Pass one as `model`; \
         on any other the backend drops the tier and runs at its own default.",
        effort,
        agent.as_str(),
        runs_on.join(", ")
    ))
}

/// Accept `effort` only if `tiers`, the model row's own list, names it.
pub fn effort_within_row_tiers(
    agent: CodingAgent,
    model: &str,
    effort: &str,
    tiers: &[String],
) -> Result<Option<String>, String> {
    if tiers.iter().any(|t| t == effort) {
        return Ok(Some(effort.to_string()));
    }
    let offered = if tiers.is_empty() {
        "none".to_string()
    } else {
        tiers.join(", ")
    };
    Err(format!(
        "reasoning_effort '{effort}' is not offered for model '{model}' by {}. \
         It offers: {offered}",
        agent.as_str()
    ))
}
