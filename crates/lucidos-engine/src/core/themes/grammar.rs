//! The whitelist grammar for theme part values (ADR 0307).
//!
//! Each part property takes one grammar with caps. The parser names every
//! allowed form, so anything else is refused, including a CSS feature that
//! does not exist yet. A valid value comes back in canonical form, which is
//! what the engine emits. The author's string never reaches a page.
//!
//! `packages/lucidos-sdk/src/themeParts.ts` is the TypeScript twin, and
//! `theme-part-cases.json` pins the two together.

use serde::{Deserialize, Serialize};

use super::color::{function_call, split_top_level};

/// How deep `color-mix()` may nest inside a part colour.
pub(super) const MAX_MIX_DEPTH: usize = 2;

/// The unit a part length takes. A bare `0` is always accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Unit {
    Em,
    Px,
}

impl Unit {
    fn suffix(self) -> &'static str {
        match self {
            Unit::Em => "em",
            Unit::Px => "px",
        }
    }
}

/// One property's grammar and caps, as `theme-parts.json` declares them.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "grammar", rename_all = "kebab-case")]
pub enum Grammar {
    /// A colour. `min_alpha` is the literal-alpha floor for text.
    #[serde(rename_all = "camelCase")]
    Colour {
        #[serde(default)]
        min_alpha: Option<f64>,
    },
    /// `none`, or up to `max_layers` of `[inset] <x> <y> <blur> [<spread>] <colour>`.
    /// `inset` and a spread need `max_spread`, which only `box-shadow` has.
    #[serde(rename_all = "camelCase")]
    Shadow {
        unit: Unit,
        max_layers: usize,
        max_offset: f64,
        max_blur: f64,
        #[serde(default)]
        max_spread: Option<f64>,
    },
    /// `none`, or exactly one `drop-shadow(<x> <y> <blur> <colour>)`.
    #[serde(rename_all = "camelCase")]
    DropShadow {
        unit: Unit,
        max_offset: f64,
        max_blur: f64,
    },
    /// `normal`, or one length between `min` and `max`.
    Spacing { unit: Unit, min: f64, max: f64 },
    /// One length between `min` and `max`.
    Length { unit: Unit, min: f64, max: f64 },
    /// One of `values`.
    Keyword { values: Vec<String> },
    /// `none`, or a `repeating-linear-gradient` of literal stops running top
    /// to bottom, each at most `max_alpha`, repeating every `max_period` px.
    #[serde(rename_all = "camelCase")]
    Scanlines {
        max_alpha: f64,
        max_period: f64,
        max_stops: usize,
    },
}

/// What a value is checked against: the property, its grammar, and what the
/// catalog knows about the part and the colour tokens.
pub struct Slot<'a> {
    pub property: &'a str,
    pub part: &'a str,
    pub grammar: &'a Grammar,
    /// The part takes inset box shadows only, since it can overlap content.
    pub inset_only: bool,
    /// Whether `var(--name)` names a colour token registered as a `<color>`
    /// wherever this part paints.
    pub is_colour_token: &'a dyn Fn(&str) -> bool,
    /// The part paints in app frames, which register the frame tokens only.
    pub frames: bool,
}

/// Parse `value` against `slot`. `Ok` holds the canonical form; `Err` says
/// what is wrong and what the limit is, without the field path.
pub fn canonical(slot: &Slot, value: &str) -> Result<String, String> {
    let value = value.trim().to_ascii_lowercase();
    match slot.grammar {
        Grammar::Colour { min_alpha } => {
            let colour = parse_colour(slot, &value, false, 0)?;
            if let Some(floor) = min_alpha {
                let alpha = colour.alpha;
                if alpha < *floor {
                    return Err(format!(
                        "alpha {} is under the {} floor for text.",
                        num((alpha * 100.0).round() / 100.0),
                        num(*floor)
                    ));
                }
            }
            Ok(colour.css)
        }
        Grammar::Shadow {
            unit,
            max_layers,
            max_offset,
            max_blur,
            max_spread,
        } => {
            if value == "none" {
                return Ok(value);
            }
            let layers = split_top_level(&value, |c| c == ',');
            if layers.len() > *max_layers {
                return Err(format!(
                    "{} layers; the limit is {max_layers}.",
                    layers.len()
                ));
            }
            let caps = Caps {
                unit: *unit,
                max_offset: *max_offset,
                max_blur: *max_blur,
            };
            layers
                .iter()
                .map(|layer| shadow_layer(slot, layer, &caps, *max_spread))
                .collect::<Result<Vec<_>, _>>()
                .map(|layers| layers.join(", "))
        }
        Grammar::DropShadow {
            unit,
            max_offset,
            max_blur,
        } => {
            if value == "none" {
                return Ok(value);
            }
            let calls = split_top_level(&value, char::is_whitespace);
            let [call] = calls.as_slice() else {
                return Err(format!(
                    "{} takes exactly one drop-shadow().",
                    slot.property
                ));
            };
            let Some(("drop-shadow", args)) = function_call(call) else {
                return Err(not_allowed(call));
            };
            let caps = Caps {
                unit: *unit,
                max_offset: *max_offset,
                max_blur: *max_blur,
            };
            let parts = split_top_level(args, char::is_whitespace);
            let [x, y, blur, colour] = parts.as_slice() else {
                return Err("drop-shadow() takes <x> <y> <blur> <colour>.".into());
            };
            let geometry = caps.geometry(slot, x, y, blur)?;
            let colour = parse_colour(slot, colour, true, 0)?;
            Ok(format!("drop-shadow({geometry} {})", colour.css))
        }
        Grammar::Spacing { .. } if value == "normal" => Ok(value),
        Grammar::Spacing { unit, min, max } | Grammar::Length { unit, min, max } => {
            bounded_length(slot, &value, *unit, *min, *max)
        }
        Grammar::Keyword { values } => {
            if values.contains(&value) {
                Ok(value)
            } else {
                Err(format!("{} takes {}.", slot.property, values.join(", ")))
            }
        }
        Grammar::Scanlines { .. } if value == "none" => Ok(value),
        Grammar::Scanlines {
            max_alpha,
            max_period,
            max_stops,
        } => scanlines(slot, &value, *max_alpha, *max_period, *max_stops),
    }
}

fn bounded_length(
    slot: &Slot,
    value: &str,
    unit: Unit,
    min: f64,
    max: f64,
) -> Result<String, String> {
    let v = length(slot, value, unit)?;
    let shown = with_unit(v, unit);
    if v > max {
        return Err(format!("{shown} is over the {} cap.", with_unit(max, unit)));
    }
    if v < min {
        return Err(format!(
            "{shown} is under the {} floor.",
            with_unit(min, unit)
        ));
    }
    Ok(shown)
}

fn is_angle(token: &str) -> bool {
    ["deg", "grad", "rad", "turn"]
        .iter()
        .any(|unit| token.strip_suffix(unit).and_then(plain_number).is_some())
}

/// A vertical `repeating-linear-gradient` of faint literal stops.
fn scanlines(
    slot: &Slot,
    value: &str,
    max_alpha: f64,
    max_period: f64,
    max_stops: usize,
) -> Result<String, String> {
    let Some((name, args)) = function_call(value) else {
        return Err(format!(
            "{} takes none or repeating-linear-gradient().",
            slot.property
        ));
    };
    if name != "repeating-linear-gradient" {
        return Err(not_allowed(value));
    }
    let stops = split_top_level(args, |c| c == ',');
    if stops.len() < 2 {
        return Err("a gradient takes at least 2 stops.".into());
    }
    if stops.len() > max_stops {
        return Err(format!("{} stops; the limit is {max_stops}.", stops.len()));
    }
    let mut previous = 0.0;
    let mut period = 0.0;
    let mut out = Vec::new();
    for (i, stop) in stops.iter().enumerate() {
        let tokens = split_top_level(stop, char::is_whitespace);
        if i == 0 && (tokens[0] == "to" || is_angle(tokens[0])) {
            return Err("scanlines take no direction: they run top to bottom.".into());
        }
        if tokens.len() > 2 {
            return Err("a stop is a colour and an optional px position.".into());
        }
        let colour = scanline_colour(tokens[0])?;
        if colour.alpha > max_alpha {
            return Err(format!(
                "stop alpha {} is over the {} cap.",
                num((colour.alpha * 100.0).round() / 100.0),
                num(max_alpha)
            ));
        }
        let Some(position) = tokens.get(1) else {
            out.push(colour.css);
            continue;
        };
        let v = length(slot, position, Unit::Px)?;
        let shown = with_unit(v, Unit::Px);
        if v < 0.0 {
            return Err(format!("position {shown} is under 0."));
        }
        if v > max_period {
            return Err(format!(
                "position {shown} is over the {} period cap.",
                with_unit(max_period, Unit::Px)
            ));
        }
        if v < previous {
            return Err("stop positions must not go down.".into());
        }
        previous = v;
        if i == stops.len() - 1 {
            period = v;
        }
        out.push(format!("{} {shown}", colour.css));
    }
    if period <= 0.0 {
        return Err("the last stop needs a px position above 0: it sets the period.".into());
    }
    Ok(format!("repeating-linear-gradient({})", out.join(", ")))
}

/// A literal colour whose alpha is known, so the cap holds for any input.
fn scanline_colour(value: &str) -> Result<Colour, String> {
    if value == "transparent" {
        return Ok(Colour {
            css: value.into(),
            alpha: 0.0,
        });
    }
    if let Some(hex) = value.strip_prefix('#') {
        return parse_hex(value, hex);
    }
    match function_call(value) {
        Some((name @ ("rgb" | "rgba" | "hsl" | "hsla"), args)) => channels(name, args),
        _ => Err(
            "a scanline stop takes a hex, rgb(), rgba(), hsl() or hsla() colour, or transparent."
                .into(),
        ),
    }
}

/// The stop colours of a canonical `scanlines` value, in order. Empty for
/// `none`.
pub fn scanline_colours(canonical: &str) -> Vec<&str> {
    let Some(("repeating-linear-gradient", args)) = function_call(canonical) else {
        return Vec::new();
    };
    split_top_level(args, |c| c == ',')
        .into_iter()
        .filter_map(|stop| split_top_level(stop, char::is_whitespace).first().copied())
        .collect()
}

struct Caps {
    unit: Unit,
    max_offset: f64,
    max_blur: f64,
}

impl Caps {
    /// `<x> <y> <blur>`, capped, in canonical form.
    fn geometry(&self, slot: &Slot, x: &str, y: &str, blur: &str) -> Result<String, String> {
        let x = self.offset(slot, "x", x)?;
        let y = self.offset(slot, "y", y)?;
        let b = length(slot, blur, self.unit)?;
        if b < 0.0 {
            return Err(format!("blur {} is under 0.", with_unit(b, self.unit)));
        }
        if b > self.max_blur {
            return Err(format!(
                "blur {} is over the {} cap.",
                with_unit(b, self.unit),
                with_unit(self.max_blur, self.unit)
            ));
        }
        Ok(format!("{x} {y} {}", with_unit(b, self.unit)))
    }

    fn offset(&self, slot: &Slot, axis: &str, token: &str) -> Result<String, String> {
        let v = length(slot, token, self.unit)?;
        if v.abs() > self.max_offset {
            return Err(format!(
                "{axis} {} is over the ±{} cap.",
                with_unit(v, self.unit),
                with_unit(self.max_offset, self.unit)
            ));
        }
        Ok(with_unit(v, self.unit))
    }
}

fn shadow_layer(
    slot: &Slot,
    layer: &str,
    caps: &Caps,
    max_spread: Option<f64>,
) -> Result<String, String> {
    let mut tokens = split_top_level(layer, char::is_whitespace);
    let inset = tokens.first() == Some(&"inset");
    if inset {
        if max_spread.is_none() {
            return Err(not_allowed("inset"));
        }
        tokens.remove(0);
    } else if slot.inset_only {
        return Err(format!("the {} takes inset shadows only.", slot.part));
    }
    let shape = match max_spread {
        Some(_) => "[inset] <x> <y> <blur> [<spread>] <colour>",
        None => "<x> <y> <blur> <colour>",
    };
    let (geometry, spread, colour) = match (tokens.as_slice(), max_spread) {
        ([x, y, blur, colour], _) => (caps.geometry(slot, x, y, blur)?, None, colour),
        ([x, y, blur, spread, colour], Some(max)) => {
            let s = length(slot, spread, caps.unit)?;
            if s.abs() > max {
                return Err(format!(
                    "spread {} is over the ±{} cap.",
                    with_unit(s, caps.unit),
                    with_unit(max, caps.unit)
                ));
            }
            (
                caps.geometry(slot, x, y, blur)?,
                Some(with_unit(s, caps.unit)),
                colour,
            )
        }
        _ => {
            if let Some(bad) = tokens
                .iter()
                .find(|t| t.contains('(') && !is_colour_call(t))
            {
                return Err(not_allowed(bad));
            }
            return Err(format!("{} takes {shape}.", slot.property));
        }
    };
    let colour = parse_colour(slot, colour, true, 0)?;
    let mut out = Vec::new();
    if inset {
        out.push("inset".to_string());
    }
    out.push(geometry);
    out.extend(spread);
    out.push(colour.css);
    Ok(out.join(" "))
}

fn is_colour_call(token: &str) -> bool {
    function_call(token).is_some_and(|(name, _)| COLOUR_FUNCTIONS.contains(&name))
}

const COLOUR_FUNCTIONS: &[&str] = &[
    "rgb",
    "rgba",
    "hsl",
    "hsla",
    "oklab",
    "oklch",
    "color-mix",
    "var",
];

/// A length in `unit`, or a bare zero.
fn length(slot: &Slot, token: &str, unit: Unit) -> Result<f64, String> {
    if token.contains('(') {
        return Err(not_allowed(token));
    }
    let split = token
        .find(|c: char| c.is_ascii_alphabetic() || c == '%')
        .unwrap_or(token.len());
    let (digits, suffix) = token.split_at(split);
    let unit_like = suffix.chars().all(|c| c.is_ascii_alphabetic() || c == '%');
    let Some(v) = plain_number(digits).filter(|_| unit_like) else {
        return Err(format!("'{token}' is not a length."));
    };
    match suffix {
        "" if v == 0.0 => Ok(0.0),
        s if s == unit.suffix() => Ok(v),
        _ => Err(format!(
            "use {} for {} lengths.",
            unit.suffix(),
            slot.property
        )),
    }
}

/// Most digits either side of the point. Inside this range Rust and
/// JavaScript write a number the same way, with no exponent.
const MAX_DIGITS: usize = 6;

/// A plain decimal: optional sign, digits, optional fraction. No exponent.
fn plain_number(token: &str) -> Option<f64> {
    let body = token.strip_prefix(['+', '-']).unwrap_or(token);
    let (int, frac) = body.split_once('.').unwrap_or((body, ""));
    let digits = |s: &str| s.len() <= MAX_DIGITS && s.chars().all(|c| c.is_ascii_digit());
    let well_formed = digits(int)
        && digits(frac)
        && (!int.is_empty() || !frac.is_empty())
        && !(body.contains('.') && frac.is_empty());
    well_formed.then(|| token.parse().ok()).flatten()
}

/// A number the way the engine writes it: shortest form, no `-0`.
pub fn num(v: f64) -> String {
    if v == 0.0 {
        "0".into()
    } else {
        format!("{v}")
    }
}

fn with_unit(v: f64, unit: Unit) -> String {
    if v == 0.0 {
        "0".into()
    } else {
        format!("{}{}", num(v), unit.suffix())
    }
}

fn not_allowed(token: &str) -> String {
    match function_call(token) {
        Some((name, _)) => format!("{name}() is not allowed here."),
        None => match token.find('(') {
            Some(open) => format!("{}() is not allowed here.", &token[..open]),
            None => format!("'{token}' is not allowed here."),
        },
    }
}

/// A parsed part colour: its canonical text and the most alpha it can have.
/// A `var()` or `currentcolor` counts as opaque, so the floor catches only
/// a literal that is surely faint.
struct Colour {
    css: String,
    alpha: f64,
}

fn parse_colour(
    slot: &Slot,
    value: &str,
    shadow: bool,
    mix_depth: usize,
) -> Result<Colour, String> {
    let value = value.trim();
    if let Some(hex) = value.strip_prefix('#') {
        return parse_hex(value, hex);
    }
    if let Some((name, args)) = function_call(value) {
        return match name {
            "rgb" | "rgba" | "hsl" | "hsla" | "oklab" | "oklch" => channels(name, args),
            "color-mix" => color_mix(slot, args, mix_depth),
            "var" => {
                if args.contains(',') {
                    return Err("var() takes no fallback here.".into());
                }
                let token = args.trim();
                if !(slot.is_colour_token)(token) {
                    return Err(if slot.frames {
                        format!("var({token}) is not a colour token app frames define.")
                    } else {
                        format!("var({token}) is not a catalog colour token.")
                    });
                }
                Ok(Colour {
                    css: format!("var({token})"),
                    alpha: 1.0,
                })
            }
            _ => Err(format!("{name}() is not allowed here.")),
        };
    }
    if value.contains('(') {
        return Err(not_allowed(value));
    }
    match value {
        "currentcolor" => Ok(Colour {
            css: value.into(),
            alpha: 1.0,
        }),
        "transparent" if shadow || mix_depth > 0 => Ok(Colour {
            css: value.into(),
            alpha: 0.0,
        }),
        "transparent" => Err("transparent is not allowed here.".into()),
        name if super::color::is_named_colour(name) => Ok(Colour {
            css: name.into(),
            alpha: 1.0,
        }),
        other => Err(format!("'{other}' is not a colour.")),
    }
}

fn parse_hex(value: &str, hex: &str) -> Result<Colour, String> {
    if !matches!(hex.len(), 3 | 4 | 6 | 8) || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!("'{value}' is not a hex colour."));
    }
    let alpha = match hex.len() {
        4 => u8::from_str_radix(&hex[3..4], 16).map(|n| n * 17).ok(),
        8 => u8::from_str_radix(&hex[6..8], 16).ok(),
        _ => Some(255),
    }
    .map_or(1.0, |a| f64::from(a) / 255.0);
    Ok(Colour {
        css: value.into(),
        alpha,
    })
}

/// A number or a percentage: `(value, is_percentage)`.
fn component(token: &str) -> Option<(f64, bool)> {
    match token.strip_suffix('%') {
        Some(n) => plain_number(n).map(|v| (v, true)),
        None => plain_number(token).map(|v| (v, false)),
    }
}

fn fmt_component((v, pct): (f64, bool)) -> String {
    if pct {
        format!("{}%", num(v))
    } else {
        num(v)
    }
}

/// `rgb()` and its siblings, in comma or space syntax, plain numbers and
/// percentages only.
fn channels(name: &str, args: &str) -> Result<Colour, String> {
    let bad = || format!("{name}() takes 3 numbers or percentages and an optional alpha.");
    if args.contains('(') {
        return Err(bad());
    }
    let (values, css) = if args.contains(',') {
        let values = args
            .split(',')
            .map(|t| component(t.trim()))
            .collect::<Option<Vec<_>>>()
            .ok_or_else(bad)?;
        if !matches!(values.len(), 3 | 4) {
            return Err(bad());
        }
        let text: Vec<String> = values.iter().copied().map(fmt_component).collect();
        (values, format!("{name}({})", text.join(", ")))
    } else {
        let (colour, alpha) = match args.split_once('/') {
            Some((c, a)) => (c, Some(a.trim())),
            None => (args, None),
        };
        let mut values = colour
            .split_whitespace()
            .map(component)
            .collect::<Option<Vec<_>>>()
            .ok_or_else(bad)?;
        if values.len() != 3 {
            return Err(bad());
        }
        let mut text = values
            .iter()
            .copied()
            .map(fmt_component)
            .collect::<Vec<_>>()
            .join(" ");
        if let Some(a) = alpha {
            let a = component(a).ok_or_else(bad)?;
            text.push_str(&format!(" / {}", fmt_component(a)));
            values.push(a);
        }
        (values, format!("{name}({text})"))
    };
    let alpha = match values.get(3) {
        Some((a, true)) => a / 100.0,
        Some((a, false)) => *a,
        None => 1.0,
    }
    .clamp(0.0, 1.0);
    Ok(Colour { css, alpha })
}

fn color_mix(slot: &Slot, args: &str, depth: usize) -> Result<Colour, String> {
    if depth >= MAX_MIX_DEPTH {
        return Err(format!("color-mix() nests at most {MAX_MIX_DEPTH} deep."));
    }
    let parts = split_top_level(args, |c| c == ',');
    let [space, first, second] = parts.as_slice() else {
        return Err("color-mix() takes a colour space and two colours.".into());
    };
    let space = space.split_whitespace().collect::<Vec<_>>();
    let space = match space.as_slice() {
        ["in", s @ ("srgb" | "oklab" | "oklch")] => *s,
        _ => return Err("color-mix() mixes in srgb, oklab or oklch.".into()),
    };
    let (c1, p1) = mix_input(slot, first, depth)?;
    let (c2, p2) = mix_input(slot, second, depth)?;
    let weight = |p: Option<f64>| p.map(|p| p / 100.0);
    let (w1, w2) = match (weight(p1), weight(p2)) {
        (None, None) => (0.5, 0.5),
        (Some(p), None) => (p, 1.0 - p),
        (None, Some(p)) => (1.0 - p, p),
        (Some(a), Some(b)) => (a, b),
    };
    let total = w1 + w2;
    let alpha = if total <= 0.0 {
        0.0
    } else {
        (c1.alpha * w1 + c2.alpha * w2) / total * total.min(1.0)
    };
    let input = |c: &Colour, p: Option<f64>| match p {
        Some(p) => format!("{} {}%", c.css, num(p)),
        None => c.css.clone(),
    };
    Ok(Colour {
        css: format!(
            "color-mix(in {space}, {}, {})",
            input(&c1, p1),
            input(&c2, p2)
        ),
        alpha,
    })
}

/// One `color-mix()` input: a colour and an optional percentage on either
/// side, as written (60 for 60%).
fn mix_input(slot: &Slot, arg: &str, depth: usize) -> Result<(Colour, Option<f64>), String> {
    let mut tokens = split_top_level(arg, char::is_whitespace);
    let pct_of = |t: &str| {
        t.strip_suffix('%')
            .and_then(plain_number)
            .filter(|p| (0.0..=100.0).contains(p))
    };
    let mut pct = None;
    if let Some(p) = tokens.last().and_then(|t| pct_of(t)) {
        pct = Some(p);
        tokens.pop();
    } else if let Some(p) = tokens.first().and_then(|t| pct_of(t)) {
        pct = Some(p);
        tokens.remove(0);
    }
    let [colour] = tokens.as_slice() else {
        return Err("a color-mix() input is one colour and an optional percentage.".into());
    };
    Ok((parse_colour(slot, colour, false, depth + 1)?, pct))
}

/// Whether a colour token may hold `value`: any part colour, `transparent`
/// included, with no alpha floor.
pub fn is_token_colour(value: &str, is_colour_token: &dyn Fn(&str) -> bool) -> bool {
    let grammar = Grammar::Colour { min_alpha: None };
    let slot = Slot {
        property: "color",
        part: "",
        grammar: &grammar,
        inset_only: false,
        is_colour_token,
        frames: false,
    };
    parse_colour(&slot, &value.trim().to_ascii_lowercase(), true, 0).is_ok()
}
