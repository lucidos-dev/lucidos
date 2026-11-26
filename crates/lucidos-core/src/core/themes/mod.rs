use std::collections::BTreeMap;
use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use super::fonts::{self, FontKind, FontSpec};
use super::workspace_fonts::{self, WorkspaceFont, WorkspaceFonts};
use color::{contrast, Rgba};
use parts::{PartsMap, PART_TOKEN_PREFIX};
use protected::{is_dark_page, reads_as, GREEN_HUES, PROTECTED_PREFIX, RED_HUES};

mod color;
mod grammar;
pub mod parts;
mod protected;

pub use parts::THEME_PARTS_CATALOG_JSON;

const PROTECTED_FONT_MONO_TOKEN: &str = "--protected-font-mono";
const TEXT_STRONG_TOKEN: &str = "--text-strong";

pub const THEMES_DIR: &str = "themes";

pub const THEME_MODE_VALUES: [&str; 3] = ["light", "dark", "system"];

pub const DEFAULT_THEME_ID: &str = "lucidos";

pub const THEME_TOKEN_CATALOG_JSON: &str = include_str!("theme-tokens.json");

const BUILT_IN_THEMES: &[(&str, &str)] = &[
    ("lucidos", include_str!("builtin/lucidos.json")),
    ("minimal", include_str!("builtin/minimal.json")),
    ("mono", include_str!("builtin/mono.json")),
    ("amethyst", include_str!("builtin/amethyst.json")),
    ("nord", include_str!("builtin/nord.json")),
    ("catppuccin", include_str!("builtin/catppuccin.json")),
    ("rose-pine", include_str!("builtin/rose-pine.json")),
    ("gruvbox", include_str!("builtin/gruvbox.json")),
    ("solarized", include_str!("builtin/solarized.json")),
    ("tokyo-night", include_str!("builtin/tokyo-night.json")),
    ("everforest", include_str!("builtin/everforest.json")),
    ("paper", include_str!("builtin/paper.json")),
    ("ember", include_str!("builtin/ember.json")),
    ("harbour", include_str!("builtin/harbour.json")),
    ("real-computer", include_str!("builtin/real-computer.json")),
];

/// INVARIANT: Caps the apply sites in `packages/lucidos-sdk/src/appearance.ts` check
/// again. They read these from the generated `theme-parts.ts`.
const MAX_VALUE_LENGTH: usize = 256;
pub const MAX_RESOLVED_TOKENS: usize = 200;
const MAX_ID_LENGTH: usize = 64;
const MAX_NAME_LENGTH: usize = 80;
const MAX_TEXT_LENGTH: usize = 400;

/// INVARIANT: WCAG's large-text floor. The engine refuses a theme whose text, or part text,
/// is fainter than this on its background. Above it, the protected palette
/// repairs protected surfaces.
const MIN_PAGE_TEXT_CONTRAST: f64 = 3.0;

/// INVARIANT: How far a shadow may reach from its box, so no theme can paint over a
/// neighbour such as an approval card.
const MAX_SHADOW_REM: f64 = 2.0;
const PX_PER_REM: f64 = 16.0;
const MAX_SHADOW_PX: f64 = MAX_SHADOW_REM * PX_PER_REM;
const SHADOW_COLOUR_FUNCTIONS: &[&str] = &[
    "rgb",
    "rgba",
    "hsl",
    "hsla",
    "hwb",
    "lab",
    "lch",
    "oklab",
    "oklch",
    "color",
    "color-mix",
];
const SHADOW_KIND: &str = "shadow";

static TOKEN_NAME_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^--[a-z][a-z0-9-]*$").expect("static regex"));
static VALUE_BANNED_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)[;{}<>@\\]|url\s*\(|image-set\s*\(|expression\s*\(|/\*").expect("static regex")
});
static THEME_ID_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-z0-9]+(-[a-z0-9]+)*$").expect("static regex"));

pub type TokenMap = BTreeMap<String, String>;

const FONT_MONO_TOKEN: &str = "--font-mono";

const UI_FONT_TOKENS: &[&str] = &[
    "--font-ui",
    "--font-family",
    "--font",
    "--font-features-text",
    "--font-features-code",
];

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThemeFonts {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ui: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mono: Option<String>,
}

impl ThemeFonts {
    pub fn is_empty(&self) -> bool {
        self.ui.is_none() && self.mono.is_none()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ThemeFamily {
    Blue,
    Violet,
    Warm,
    Neutral,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ThemeMode {
    Dark,
    Light,
}

impl ThemeMode {
    fn label(self) -> &'static str {
        match self {
            ThemeMode::Dark => "dark",
            ThemeMode::Light => "light",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ThemeModeMap {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub parts: PartsMap,
    #[serde(flatten)]
    pub tokens: TokenMap,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThemeDefinition {
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub author: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub credit: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub family: Option<ThemeFamily>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tokens: TokenMap,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dark: Option<ThemeModeMap>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub light: Option<ThemeModeMap>,
    #[serde(default, skip_serializing_if = "ThemeFonts::is_empty")]
    pub fonts: ThemeFonts,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub parts: PartsMap,
}

impl ThemeDefinition {
    fn mode_map(&self, mode: ThemeMode) -> Option<&ThemeModeMap> {
        match mode {
            ThemeMode::Dark => self.dark.as_ref(),
            ThemeMode::Light => self.light.as_ref(),
        }
    }

    /// INVARIANT: The modes this theme has a map for. A theme with neither applies only its
    /// shared `tokens`, in both modes. A map that holds only parts does not
    /// count, so an effect in one mode never makes the theme single-mode.
    pub fn modes(&self) -> Vec<ThemeMode> {
        [ThemeMode::Dark, ThemeMode::Light]
            .into_iter()
            .filter(|m| {
                self.mode_map(*m)
                    .is_some_and(|map| !map.tokens.is_empty() || map.parts.is_empty())
            })
            .collect()
    }

    fn maps(&self) -> impl Iterator<Item = &TokenMap> {
        [
            Some(&self.tokens),
            self.dark.as_ref().map(|m| &m.tokens),
            self.light.as_ref().map(|m| &m.tokens),
        ]
        .into_iter()
        .flatten()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ThemeSource {
    BuiltIn,
    Workspace,
}

/// INVARIANT: The token map each mode paints, derivation already applied, and the fonts
/// the theme suggests. Every client resolves the UI font from `fonts.ui`.
///
/// `fonts` names only fonts that exist and fit their slot. A workspace font
/// among them has its entry in `workspace_fonts`, so a surface can register
/// its faces without a second lookup.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ResolvedTheme {
    pub dark: TokenMap,
    pub light: TokenMap,
    #[serde(skip_serializing_if = "ThemeFonts::is_empty")]
    pub fonts: ThemeFonts,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub workspace_fonts: Vec<WorkspaceFont>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Theme {
    pub id: String,
    pub source: ThemeSource,
    #[serde(flatten)]
    pub definition: ThemeDefinition,
    pub modes: Vec<ThemeMode>,
    pub resolved: ResolvedTheme,
}

pub type ThemeResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Debug, Deserialize)]
struct Catalog {
    tokens: Vec<CatalogToken>,
}

#[derive(Debug, Deserialize)]
struct CatalogToken {
    name: String,
    kind: String,
    #[serde(default)]
    frames: bool,
    default: CatalogDefault,
    #[serde(default)]
    derive: Option<Derivation>,
}

#[derive(Debug, Deserialize)]
struct CatalogDefault {
    dark: String,
    light: String,
}

impl CatalogToken {
    fn default_in(&self, mode: ThemeMode) -> &str {
        match mode {
            ThemeMode::Dark => &self.default.dark,
            ThemeMode::Light => &self.default.light,
        }
    }
}

#[derive(Debug, Deserialize)]
struct Derivation {
    value: String,
    seeds: Vec<String>,
}

static CATALOG: LazyLock<Catalog> = LazyLock::new(|| {
    serde_json::from_str(THEME_TOKEN_CATALOG_JSON).expect("theme-tokens.json is valid")
});

fn catalog_token(name: &str) -> Option<&'static CatalogToken> {
    CATALOG.tokens.iter().find(|t| t.name == name)
}

const COLOUR_KIND: &str = "color";

pub fn is_colour_token(name: &str) -> bool {
    catalog_token(name).is_some_and(|t| t.kind == COLOUR_KIND)
}

pub fn is_frame_colour_token(name: &str) -> bool {
    catalog_token(name).is_some_and(|t| t.kind == COLOUR_KIND && t.frames)
}

fn colour_tokens() -> impl Iterator<Item = (&'static str, bool)> {
    CATALOG
        .tokens
        .iter()
        .filter(|t| t.kind == COLOUR_KIND)
        .map(|t| (t.name.as_str(), t.frames))
}

pub fn validate_id(id: &str) -> ThemeResult<()> {
    if THEME_MODE_VALUES.contains(&id) {
        return Err(format!(
            "'{id}' is a theme mode, not a theme id. Set the '{}' preference for light or dark",
            crate::core::prefs::THEME_MODE.key()
        )
        .into());
    }
    if !is_well_formed_id(id) {
        return Err(format!(
            "theme id '{id}' must be lowercase letters and digits joined by single hyphens"
        )
        .into());
    }
    Ok(())
}

pub fn is_well_formed_id(id: &str) -> bool {
    id.len() <= MAX_ID_LENGTH && THEME_ID_RE.is_match(id)
}

pub fn is_built_in(id: &str) -> bool {
    BUILT_IN_THEMES.iter().any(|(built_in, _)| *built_in == id)
}

pub fn is_valid_token_name(name: &str) -> bool {
    TOKEN_NAME_RE.is_match(name)
}

pub fn is_valid_token_value(value: &str) -> bool {
    let trimmed = value.trim();
    // INVARIANT: UTF-16 units, as JavaScript's `length` counts them at the apply site.
    !trimmed.is_empty()
        && trimmed.encode_utf16().count() <= MAX_VALUE_LENGTH
        && !VALUE_BANNED_RE.is_match(trimmed)
}

fn validate_token_name(label: &str, name: &str) -> ThemeResult<&'static CatalogToken> {
    if name.starts_with(PART_TOKEN_PREFIX) {
        return Err(format!("`{label}`: {name} is a part token. Set parts with `parts`.").into());
    }
    if UI_FONT_TOKENS.contains(&name) {
        return Err(format!(
            "`{label}`: {name} carries the user's UI font. A theme suggests one with `fonts.ui`."
        )
        .into());
    }
    if !is_valid_token_name(name) {
        let hint = if name.to_ascii_lowercase().starts_with("part") {
            " Did you mean parts?"
        } else {
            ""
        };
        return Err(format!(
            "`{label}`: '{name}' is not a custom property name (--lowercase-kebab).{hint}"
        )
        .into());
    }
    if name.starts_with(PROTECTED_PREFIX) {
        return Err(format!(
            "`{label}`: Lucidos computes {name} to keep protected surfaces readable, so a theme cannot set it"
        )
        .into());
    }
    catalog_token(name).ok_or_else(|| {
        format!(
            "`{label}`: {name} is not a theme token. A theme can set only the tokens GET /api/v1/themes/tokens lists."
        )
        .into()
    })
}

fn validate_map(label: &str, map: &TokenMap) -> ThemeResult<()> {
    for (name, value) in map {
        let token = validate_token_name(label, name)?;
        if !is_valid_token_value(value) {
            return Err(format!(
                "`{label}`: the value of {name} is empty, longer than {MAX_VALUE_LENGTH} characters, or uses a banned form (url(), ;, braces, @, backslash, a comment)"
            )
            .into());
        }
        if token.kind == SHADOW_KIND {
            validate_shadow_reach(value)
                .map_err(|why| format!("`{label}`: the shadow {name} {why}"))?;
        }
        if token.kind == COLOUR_KIND && !is_colour_value(value) {
            return Err(format!(
                "`{label}`: {name} is a colour token, and '{value}' is not a colour"
            )
            .into());
        }
        if let Some(alias) = parts::CATALOG.alias(name) {
            parts::check_alias(alias, &alias_value(value)).map_err(|why| {
                format!(
                    "`{label}`: {name} sets {} on {}: {why}",
                    alias.property,
                    alias.parts.join(", ")
                )
            })?;
        }
    }
    Ok(())
}

fn is_colour_value(value: &str) -> bool {
    grammar::is_token_colour(value, &is_colour_token)
        || evaluate_in(&TokenMap::new(), ThemeMode::Dark, value).is_some()
}

fn alias_value(value: &str) -> String {
    static LENGTH_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)(^|[\s(])([+-]?(?:\d+\.?\d*|\.\d+))(rem|px)\b").expect("static regex")
    });
    LENGTH_RE
        .replace_all(value, |c: &regex::Captures| {
            let n: f64 = c[2].parse().unwrap_or(0.0);
            let em = if c[3].eq_ignore_ascii_case("px") {
                n / 16.0
            } else {
                n
            };
            format!("{}{}em", &c[1], grammar::num(em))
        })
        .into_owned()
}

fn validate_shadow_reach(value: &str) -> Result<(), String> {
    for layer in color::split_top_level(value, |c| c == ',') {
        let mut lengths = Vec::new();
        for part in color::split_top_level(layer, char::is_whitespace) {
            let part = part.to_ascii_lowercase();
            if part.contains('(') {
                // INVARIANT: The call must be the whole word: `rgb(0 0 0)999px` hides a
                // length behind the colour.
                match color::function_call(&part) {
                    Some((name, _)) if SHADOW_COLOUR_FUNCTIONS.contains(&name) => continue,
                    Some((name, _)) => {
                        return Err(format!(
                            "uses {name}(), so how far it reaches cannot be checked"
                        ))
                    }
                    None => return Err(format!("has a word that cannot be read: `{part}`")),
                }
            }
            if let Some(px) = shadow_length_px(&part)? {
                lengths.push(px);
            }
        }
        let (offset, blur, spread) = match lengths.as_slice() {
            [] => continue,
            [x, y] => (x.abs().max(y.abs()), 0.0, 0.0),
            [x, y, blur] => (x.abs().max(y.abs()), *blur, 0.0),
            [x, y, blur, spread] => (x.abs().max(y.abs()), *blur, *spread),
            _ => return Err(format!("has a layer that is not a shadow: `{layer}`")),
        };
        let reach = offset + spread.max(0.0) + blur.max(0.0) / 2.0;
        if reach > MAX_SHADOW_PX {
            return Err(format!(
                "reaches {reach:.0}px past its box; a shadow may reach at most {MAX_SHADOW_REM}rem ({MAX_SHADOW_PX}px)"
            ));
        }
    }
    Ok(())
}

/// INVARIANT: One word of a shadow as a length in px: `None` for a keyword or a colour.
/// CSS ends a word's first token at a `+`, and at a sign after a number, so
/// `0+999px` is two lengths. A word that could hide one is refused.
fn shadow_length_px(part: &str) -> Result<Option<f64>, String> {
    let unreadable = || Err(format!("has a word that cannot be read: `{part}`"));
    let starts_numeric = part
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_digit() || matches!(c, '.' | '+' | '-'));
    if !starts_numeric {
        return if part.contains('+') {
            unreadable()
        } else {
            Ok(None)
        };
    }
    let unsigned = part.strip_prefix(['+', '-']).unwrap_or(part);
    let digits = unsigned
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(unsigned.len());
    let number_len = part.len() - unsigned.len() + digits;
    let Some(length) = part[..number_len]
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite())
    else {
        return unreadable();
    };
    match &part[number_len..] {
        "" if length == 0.0 => Ok(Some(0.0)),
        "px" => Ok(Some(length)),
        "rem" | "em" => Ok(Some(length * PX_PER_REM)),
        _ => Err(format!(
            "reaches {part}, in a unit whose size cannot be checked"
        )),
    }
}

fn validate_font(slot: &str, id: &str) -> ThemeResult<Option<&'static FontSpec>> {
    if let Some(font) = fonts::find(id) {
        return Ok(Some(font));
    }
    if workspace_fonts::is_workspace_id(id) {
        return Ok(None);
    }
    Err(format!("`fonts.{slot}`: '{id}' is not a font id. GET /api/v1/fonts lists them.").into())
}

#[derive(Debug, Clone, Copy)]
enum FontSlot {
    Ui,
    Mono,
}

impl FontSlot {
    fn name(self) -> &'static str {
        match self {
            FontSlot::Ui => "ui",
            FontSlot::Mono => "mono",
        }
    }

    fn id(self, fonts: &ThemeFonts) -> Option<&str> {
        match self {
            FontSlot::Ui => fonts.ui.as_deref(),
            FontSlot::Mono => fonts.mono.as_deref(),
        }
    }

    fn fits(self, kind: FontKind) -> bool {
        match self {
            FontSlot::Ui => kind.fits_ui(),
            FontSlot::Mono => kind.fits_code(),
        }
    }

    fn unfit(self, id: &str) -> String {
        match self {
            FontSlot::Ui => format!("`fonts.ui`: '{id}' is fit only for code."),
            FontSlot::Mono => {
                format!("`fonts.mono`: '{id}' is proportional, so it cannot be the code font.")
            }
        }
    }
}

const FONT_SLOTS: [FontSlot; 2] = [FontSlot::Ui, FontSlot::Mono];

fn validate_fonts(def: &ThemeDefinition) -> ThemeResult<()> {
    for slot in FONT_SLOTS {
        let Some(id) = slot.id(&def.fonts) else {
            continue;
        };
        if validate_font(slot.name(), id)?.is_some_and(|font| !slot.fits(font.kind)) {
            return Err(slot.unfit(id).into());
        }
    }
    if def.fonts.mono.is_some() && def.maps().any(|map| map.contains_key(FONT_MONO_TOKEN)) {
        return Err(format!(
            "set the code font with `fonts.mono` or a {FONT_MONO_TOKEN} token, not both"
        )
        .into());
    }
    Ok(())
}

fn validate_text(label: &str, text: &str, max: usize) -> ThemeResult<()> {
    if text.chars().count() > max {
        return Err(format!("`{label}` is longer than {max} characters").into());
    }
    Ok(())
}

pub fn validate_definition(def: &ThemeDefinition) -> ThemeResult<()> {
    if def.name.trim().is_empty() {
        return Err("`name` is required".into());
    }
    validate_text("name", &def.name, MAX_NAME_LENGTH)?;
    validate_text("description", &def.description, MAX_TEXT_LENGTH)?;
    validate_text("author", &def.author, MAX_NAME_LENGTH)?;
    validate_text("credit", &def.credit, MAX_TEXT_LENGTH)?;
    validate_map("tokens", &def.tokens)?;
    parts::validate_parts("parts", &def.parts)?;
    for mode in [ThemeMode::Dark, ThemeMode::Light] {
        if let Some(map) = def.mode_map(mode) {
            validate_map(mode.label(), &map.tokens)?;
            parts::validate_parts(&format!("{}.parts", mode.label()), &map.parts)?;
        }
    }
    validate_fonts(def)?;
    for mode in [ThemeMode::Dark, ThemeMode::Light] {
        validate_readable(&resolve_theme_tokens(def, mode, None), mode)?;
        let resolved = resolve_mode(def, mode, def.fonts.mono.as_deref());
        validate_part_contrast(&resolved, mode)?;
        validate_screen_contrast(&resolved, mode)?;
        let size = resolved.len();
        if size > MAX_RESOLVED_TOKENS {
            return Err(format!(
                "in {} mode, the theme resolves to {size} tokens; the limit is {MAX_RESOLVED_TOKENS}",
                mode.label()
            )
            .into());
        }
    }
    Ok(())
}

fn validate_readable(tokens: &TokenMap, mode: ThemeMode) -> ThemeResult<()> {
    let colour = |token: &str| colour_in(tokens, mode, token);
    let m = mode.label();
    if let (Some(text), Some(page)) = (colour("--text-primary"), colour("--bg-primary")) {
        let page = page.over(Rgba::WHITE);
        let ratio = contrast(text.over(page), page);
        if ratio < MIN_PAGE_TEXT_CONTRAST {
            return Err(format!(
                "in {m} mode, --text-primary on --bg-primary is {ratio:.1}:1. Text needs at least {MIN_PAGE_TEXT_CONTRAST}:1 against the page."
            )
            .into());
        }
    }
    if colour("--accent-green").is_some_and(|c| reads_as(RED_HUES, c)) {
        return Err(format!(
            "in {m} mode, --accent-green reads as red. It marks Allow and success, so it must not look like --accent-red."
        )
        .into());
    }
    if colour("--accent-red").is_some_and(|c| reads_as(GREEN_HUES, c)) {
        return Err(format!(
            "in {m} mode, --accent-red reads as green. It marks Deny and danger, so it must not look like --accent-green."
        )
        .into());
    }
    Ok(())
}

const SCREEN_SCANLINES_TOKEN: &str = "--part-screen-background-image";

/// INVARIANT: Part text colours and the background each one sits on. A background part
/// token falls back to the token after it, as its stylesheet rule does.
const PART_TEXT_ON: &[(&str, &[&str])] = &[
    ("--part-chat-text-color", &["--bg-primary"]),
    ("--part-chat-heading-color", &["--bg-primary"]),
    ("--part-chat-link-color", &["--bg-primary"]),
    (
        "--part-composer-text-color",
        &["--part-composer-background-color", "--bg-secondary"],
    ),
    (
        "--part-inline-code-color",
        &["--part-inline-code-background-color", "--bg-tertiary"],
    ),
];

fn validate_part_contrast(resolved: &TokenMap, mode: ThemeMode) -> ThemeResult<()> {
    let page = colour_in(resolved, mode, "--bg-primary")
        .unwrap_or(Rgba::WHITE)
        .over(Rgba::WHITE);
    for (text, backgrounds) in PART_TEXT_ON {
        let Some(colour) = resolved
            .get(*text)
            .and_then(|v| evaluate_in(resolved, mode, v))
        else {
            continue;
        };
        let background = backgrounds
            .iter()
            .find(|b| !b.starts_with(PART_TOKEN_PREFIX) || resolved.contains_key(**b))
            .expect("every list ends in a plain token");
        let Some(fill) = colour_in(resolved, mode, background) else {
            continue;
        };
        let fill = fill.over(page);
        let ratio = contrast(colour.over(fill), fill);
        if ratio < MIN_PAGE_TEXT_CONTRAST {
            return Err(format!(
                "in {} mode, {} is {ratio:.1}:1 on {background}. Text needs at least {MIN_PAGE_TEXT_CONTRAST}:1 against its background.",
                mode.label(),
                text_label(text)
            )
            .into());
        }
    }
    Ok(())
}

fn validate_screen_contrast(resolved: &TokenMap, mode: ThemeMode) -> ThemeResult<()> {
    let Some(scanlines) = resolved.get(SCREEN_SCANLINES_TOKEN) else {
        return Ok(());
    };
    let Some(page) = colour_in(resolved, mode, "--bg-primary") else {
        return Ok(());
    };
    let page = page.over(Rgba::WHITE);
    let bands: Vec<Rgba> = grammar::scanline_colours(scanlines)
        .into_iter()
        .filter_map(|stop| evaluate_in(resolved, mode, stop))
        .map(|stop| stop.over(page))
        .collect();
    let on_page = PART_TEXT_ON
        .iter()
        .filter(|(_, backgrounds)| *backgrounds == ["--bg-primary"])
        .map(|(text, _)| *text);
    for name in std::iter::once("--text-primary").chain(on_page) {
        let Some(text) = colour_in(resolved, mode, name) else {
            continue;
        };
        for band in &bands {
            let ratio = contrast(text.over(*band), *band);
            if ratio < MIN_PAGE_TEXT_CONTRAST {
                return Err(format!(
                    "in {} mode, {} is {ratio:.1}:1 on --bg-primary under the screen scanlines. Text needs at least {MIN_PAGE_TEXT_CONTRAST}:1 against its background.",
                    mode.label(),
                    text_label(name)
                )
                .into());
            }
        }
    }
    Ok(())
}

fn text_label(name: &str) -> String {
    parts::CATALOG
        .tokens()
        .find(|(_, usage)| usage.token == name)
        .map_or_else(
            || name.to_string(),
            |(part, usage)| format!("the {} {}", part.id, usage.name),
        )
}

pub fn parse_definition(bytes: &[u8]) -> ThemeResult<ThemeDefinition> {
    let raw: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|e| format!("not a valid theme file: {e}"))?;
    check_parts_shapes(&raw)?;
    let def: ThemeDefinition =
        serde_json::from_value(raw).map_err(|e| format!("not a valid theme file: {e}"))?;
    validate_definition(&def)?;
    Ok(def)
}

pub const STYLE_OVERRIDES_KEY: &str = "style_overrides";

pub fn validate_style_overrides(value: &str) -> Result<(), String> {
    let Ok(serde_json::Value::Object(map)) = serde_json::from_str(value) else {
        return Ok(());
    };
    for (name, value) in &map {
        let value = value.as_str().unwrap_or_default();
        let checked = if name == SCREEN_SCANLINES_TOKEN {
            Err(
                "a theme sets it, never an override: the protected palette is clamped against it."
                    .into(),
            )
        } else if name.starts_with(PART_TOKEN_PREFIX) {
            parts::check_part_token(name, value).map(drop)
        } else if is_shadow_token(name) {
            validate_shadow_reach(value).map_err(|why| format!("the shadow {why}"))
        } else {
            Ok(())
        };
        checked.map_err(|why| format!("{STYLE_OVERRIDES_KEY}: {name}: {why}"))?;
    }
    Ok(())
}

pub fn is_shadow_token(name: &str) -> bool {
    catalog_token(name).is_some_and(|t| t.kind == SHADOW_KIND)
}

fn check_parts_shapes(raw: &serde_json::Value) -> ThemeResult<()> {
    if let Some(parts) = raw.get("parts") {
        parts::check_shape("parts", parts)?;
    }
    for mode in [ThemeMode::Dark, ThemeMode::Light] {
        let label = mode.label();
        let Some(map) = raw.get(label).and_then(|map| map.as_object()) else {
            continue;
        };
        for (key, value) in map {
            if key == "parts" {
                parts::check_shape(&format!("{label}.parts"), value)?;
            } else if !value.is_string() {
                validate_token_name(label, key)?;
                return Err(format!("`{label}`: the value of {key} must be a string").into());
            }
        }
    }
    Ok(())
}

pub fn id_from_file_name(file_name: &str) -> ThemeResult<&str> {
    let id = file_name
        .strip_suffix(".json")
        .filter(|stem| !stem.contains('/'))
        .ok_or_else(|| format!("'{file_name}' is not a theme file: a theme is themes/<id>.json"))?;
    validate_id(id)?;
    Ok(id)
}

pub fn check_installed_fonts(fonts: &ThemeFonts, installed: &WorkspaceFonts) -> ThemeResult<()> {
    for slot in FONT_SLOTS {
        let Some(id) = slot
            .id(fonts)
            .filter(|id| workspace_fonts::is_workspace_id(id))
        else {
            continue;
        };
        let Some(font) = installed.find(id) else {
            return Err(format!(
                "`fonts.{}`: '{id}' is not an installed workspace font. GET /api/v1/fonts lists them.",
                slot.name()
            )
            .into());
        };
        if !slot.fits(font.kind) {
            return Err(slot.unfit(id).into());
        }
    }
    Ok(())
}

fn names_workspace_font(fonts: &ThemeFonts) -> bool {
    FONT_SLOTS
        .iter()
        .any(|slot| slot.id(fonts).is_some_and(workspace_fonts::is_workspace_id))
}

pub fn validate_workspace_write(
    file_name: &str,
    bytes: &[u8],
    installed: impl FnOnce() -> WorkspaceFonts,
) -> ThemeResult<()> {
    let id = id_from_file_name(file_name)?;
    if is_built_in(id) {
        return Err(
            format!("'{id}' is a built-in theme, so a workspace theme cannot use that id").into(),
        );
    }
    let fonts = parse_definition(bytes)?.fonts;
    if !names_workspace_font(&fonts) {
        return Ok(());
    }
    check_installed_fonts(&fonts, &installed())
}

fn themes_relative(data_path: &str) -> Option<&str> {
    data_path.strip_prefix(THEMES_DIR)?.strip_prefix('/')
}

pub fn is_theme_path(data_path: &str) -> bool {
    themes_relative(data_path).is_some()
}

pub fn validate_data_write(data_dir: &Path, data_path: &str, bytes: &[u8]) -> Result<(), String> {
    match themes_relative(data_path) {
        Some(file_name) => {
            validate_workspace_write(file_name, bytes, || workspace_fonts::list(data_dir))
                .map_err(|e| e.to_string())
        }
        None => Ok(()),
    }
}

pub fn resolve_mode(def: &ThemeDefinition, mode: ThemeMode, mono_stack: Option<&str>) -> TokenMap {
    let mut resolved = resolve_theme_tokens(def, mode, mono_stack);
    let sets_tokens = !resolved.is_empty();
    if let Some(strong) = strong_text(&resolved, mode) {
        resolved.insert(TEXT_STRONG_TOKEN.to_string(), strong);
    }
    let aliases: TokenMap = parts::CATALOG
        .aliases
        .iter()
        .filter_map(|a| resolved.remove_entry(&a.token))
        .map(|(name, value)| (name, alias_value(&value)))
        .collect();
    let mode_parts = def.mode_map(mode).map(|m| &m.parts);
    resolved.extend(parts::resolve(&def.parts, mode_parts, &aliases));
    // INVARIANT: Scanlines sit under protected text, so a theme that sets them gets a
    // palette clamped against them even when it sets no token.
    if sets_tokens || resolved.contains_key(SCREEN_SCANLINES_TOKEN) {
        let palette = protected_palette(&resolved, mode);
        resolved.extend(palette.into_iter().map(|(k, v)| (k.to_string(), v)));
        resolved.insert(
            PROTECTED_FONT_MONO_TOKEN.to_string(),
            protected_font_mono(def).to_string(),
        );
    }
    resolved
}

/// INVARIANT: Bold text where the UI font has no bold face. The catalog default steps
/// toward white in dark mode and toward black in light mode. That is wrong for
/// a theme that paints a dark page in both. So a theme that sets a page or text
/// colour steps away from its own page instead. `None` leaves the token alone.
fn strong_text(tokens: &TokenMap, mode: ThemeMode) -> Option<String> {
    let sets_seed = ["--bg-primary", "--text-primary"]
        .iter()
        .any(|seed| tokens.contains_key(*seed));
    if !sets_seed || tokens.contains_key(TEXT_STRONG_TOKEN) {
        return None;
    }
    let page = colour_in(tokens, mode, "--bg-primary")?;
    let away = if is_dark_page(page) { "white" } else { "black" };
    Some(format!(
        "color-mix(in oklab, var(--text-primary) 60%, {away})"
    ))
}

fn protected_font_mono(def: &ThemeDefinition) -> &'static str {
    def.fonts
        .mono
        .as_deref()
        .and_then(fonts::find)
        .map(|font| font.stack)
        .or_else(|| catalog_token(FONT_MONO_TOKEN).map(|t| t.default.dark.as_str()))
        .expect("the catalog lists the code font")
}

fn protected_palette(tokens: &TokenMap, mode: ThemeMode) -> Vec<(&'static str, String)> {
    let default = |token: &str| {
        colour_in(&TokenMap::new(), mode, token)
            .expect("every protected source has an evaluable catalog default")
    };
    let scanlines: Vec<Rgba> = tokens
        .get(SCREEN_SCANLINES_TOKEN)
        .map(|value| grammar::scanline_colours(value))
        .unwrap_or_default()
        .into_iter()
        .filter_map(|stop| evaluate_in(tokens, mode, stop))
        .collect();
    protected::palette(&protected::Source {
        colour: &|token| colour_in(tokens, mode, token),
        default: &default,
        scanlines: &scanlines,
    })
}

fn colour_in(tokens: &TokenMap, mode: ThemeMode, token: &str) -> Option<Rgba> {
    evaluate_in(tokens, mode, &token_value(tokens, mode, token)?)
}

fn token_value(tokens: &TokenMap, mode: ThemeMode, name: &str) -> Option<String> {
    tokens
        .get(name)
        .cloned()
        .or_else(|| catalog_token(name).map(|t| t.default_in(mode).to_string()))
}

fn evaluate_in(tokens: &TokenMap, mode: ThemeMode, value: &str) -> Option<Rgba> {
    color::evaluate(value, &|name| token_value(tokens, mode, name))
}

/// INVARIANT: The theme's own tokens for one mode: shared tokens, then the mode's own,
/// then every catalog derivation whose seed the theme set and whose token it
/// did not. `mono_stack` fills the code font, which validation keeps from
/// colliding with an explicit token.
fn resolve_theme_tokens(
    def: &ThemeDefinition,
    mode: ThemeMode,
    mono_stack: Option<&str>,
) -> TokenMap {
    let mut explicit = def.tokens.clone();
    if let Some(map) = def.mode_map(mode) {
        explicit.extend(map.tokens.iter().map(|(k, v)| (k.clone(), v.clone())));
    }
    let mut resolved = explicit.clone();
    for token in &CATALOG.tokens {
        let Some(derivation) = &token.derive else {
            continue;
        };
        if explicit.contains_key(&token.name) {
            continue;
        }
        if derivation
            .seeds
            .iter()
            .any(|seed| explicit.contains_key(seed))
        {
            resolved.insert(token.name.clone(), derivation.value.clone());
        }
    }
    if let Some(stack) = mono_stack {
        resolved.insert(FONT_MONO_TOKEN.to_string(), stack.to_string());
    }
    resolved
}

fn usable_fonts(
    fonts: &ThemeFonts,
    installed: &WorkspaceFonts,
) -> (ThemeFonts, Vec<WorkspaceFont>) {
    let mut usable = ThemeFonts::default();
    let mut entries: Vec<WorkspaceFont> = Vec::new();
    for slot in FONT_SLOTS {
        let Some(id) = slot.id(fonts) else {
            continue;
        };
        let workspace = installed.find(id);
        let Some(kind) = fonts::find(id)
            .map(|font| font.kind)
            .or(workspace.map(|font| font.kind))
        else {
            crate::log_legacy!(
                "[Themes] `fonts.{}`: '{id}' is not installed, so it is skipped",
                slot.name()
            );
            continue;
        };
        if !slot.fits(kind) {
            crate::log_legacy!("[Themes] {}, so it is skipped", slot.unfit(id));
            continue;
        }
        if let Some(font) = workspace.filter(|font| !entries.contains(font)) {
            entries.push(font.clone());
        }
        let kept = Some(id.to_string());
        match slot {
            FontSlot::Ui => usable.ui = kept,
            FontSlot::Mono => usable.mono = kept,
        }
    }
    (usable, entries)
}

fn stack_of<'a>(id: &str, entries: &'a [WorkspaceFont]) -> Option<&'a str> {
    fonts::find(id).map(|font| font.stack).or_else(|| {
        entries
            .iter()
            .find(|font| font.id == id)
            .map(|font| font.stack.as_str())
    })
}

pub fn resolve(definition: &ThemeDefinition, installed: &WorkspaceFonts) -> ResolvedTheme {
    let (fonts, workspace_fonts) = usable_fonts(&definition.fonts, installed);
    let mono_stack = fonts
        .mono
        .as_deref()
        .and_then(|id| stack_of(id, &workspace_fonts));
    ResolvedTheme {
        dark: resolve_mode(definition, ThemeMode::Dark, mono_stack),
        light: resolve_mode(definition, ThemeMode::Light, mono_stack),
        fonts,
        workspace_fonts,
    }
}

fn build(
    id: &str,
    source: ThemeSource,
    definition: ThemeDefinition,
    installed: &WorkspaceFonts,
) -> Theme {
    let resolved = resolve(&definition, installed);
    Theme {
        id: id.to_string(),
        source,
        modes: definition.modes(),
        definition,
        resolved,
    }
}

static BUILT_INS: LazyLock<Vec<Theme>> = LazyLock::new(|| {
    BUILT_IN_THEMES
        .iter()
        .map(|(id, json)| {
            let def = parse_definition(json.as_bytes()).expect("built-in themes are valid");
            build(id, ThemeSource::BuiltIn, def, &WorkspaceFonts::default())
        })
        .collect()
});

fn built_in(id: &str) -> Option<Theme> {
    BUILT_INS.iter().find(|theme| theme.id == id).cloned()
}

fn workspace_theme_path(data_dir: &Path, id: &str) -> std::path::PathBuf {
    data_dir.join(THEMES_DIR).join(format!("{id}.json"))
}

pub const LEGACY_THEMES_DIR: &str = "looks";

pub fn get(data_dir: &Path, id: &str) -> ThemeResult<Option<Theme>> {
    get_with(data_dir, id, &mut None)
}

fn get_with(
    data_dir: &Path,
    id: &str,
    installed: &mut Option<WorkspaceFonts>,
) -> ThemeResult<Option<Theme>> {
    validate_id(id)?;
    if let Some(theme) = built_in(id) {
        return Ok(Some(theme));
    }
    let bytes = match std::fs::read(workspace_theme_path(data_dir, id)) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let definition = parse_definition(&bytes)?;
    let none = WorkspaceFonts::default();
    let fonts = if names_workspace_font(&definition.fonts) {
        &*installed.get_or_insert_with(|| workspace_fonts::list(data_dir))
    } else {
        &none
    };
    Ok(Some(build(id, ThemeSource::Workspace, definition, fonts)))
}

pub fn list(data_dir: &Path) -> Vec<Theme> {
    let mut themes: Vec<Theme> = BUILT_INS.clone();

    let Ok(entries) = std::fs::read_dir(data_dir.join(THEMES_DIR)) else {
        return themes;
    };
    let mut ids: Vec<String> = entries
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            id_from_file_name(&name).ok().map(str::to_string)
        })
        .filter(|id| !is_built_in(id))
        .collect();
    ids.sort();
    let mut installed = None;
    for id in ids {
        match get_with(data_dir, &id, &mut installed) {
            Ok(Some(theme)) => themes.push(theme),
            Ok(None) => {}
            Err(e) => crate::log_legacy!("[Themes] skipping themes/{id}.json: {e}"),
        }
    }
    themes
}

pub fn resolved_json_for_preference(data_dir: &Path, id: &str) -> Option<String> {
    if id == DEFAULT_THEME_ID {
        return None;
    }
    let theme = get(data_dir, id).ok()??;
    serde_json::to_string(&theme.resolved).ok()
}

#[cfg(test)]
mod tests;
