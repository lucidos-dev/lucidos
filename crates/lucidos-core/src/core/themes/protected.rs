//! The protected palette: the colours protected surfaces paint, derived from a
//! theme and clamped so no theme can make them unreadable or misleading.
//!
//! Every value is a literal colour. A `var()` of an ordinary token would let a
//! style override flow into a protected surface. The shell maps these onto
//! `.protected-surface` (`styles/global/protected-surface.css`). Design:
//! ADR 0309.

use super::color::{contrast, Rgba};

/// The prefix every protected token carries. A theme may not set one, and the
/// SDK drops one from a style override map.
pub const PROTECTED_PREFIX: &str = "--protected-";

/// WCAG AA for body text. Every text and label pair meets it.
const TEXT_CONTRAST: f64 = 4.5;

/// The least a blocking dialog's scrim may dim the page.
const MIN_SCRIM_ALPHA: f64 = 0.4;
/// A scrim lighter than this veils instead of dimming, so it turns black.
const MAX_SCRIM_LUMINANCE: f64 = 0.05;

/// A page darker than this is a dark theme. It is the luminance where white and
/// black text contrast equally.
const POLARITY_SPLIT: f64 = 0.18;
/// Fills stay clear of the middle greys, where no text colour reaches AA on
/// all of them at once.
const DARK_FILL_MAX_LUMINANCE: f64 = 0.10;
const LIGHT_FILL_MIN_LUMINANCE: f64 = 0.40;

/// Confirm and deny keep at least this OKLCH chroma, so neither turns grey.
const MIN_BUTTON_CHROMA: f64 = 0.1;
/// Below this chroma a colour reads as grey, and its hue means nothing.
const HUE_CHROMA: f64 = 0.05;
const LIGHTNESS_STEP: f64 = 0.005;
/// How much accent a picked option's fill carries.
const PICKED_TINT: f64 = 0.12;
/// How far inside a band a pinned hue lands. Rounding to a hex colour and
/// the lightness walk each move the hue a little, and an edge would lose it.
const HUE_PIN_MARGIN: f64 = 10.0;

/// An arc of OKLCH hue, clockwise from `from` to `to`. It may wrap past 0.
#[derive(Debug, Clone, Copy)]
pub struct HueBand {
    from: f64,
    to: f64,
}

pub const GREEN_HUES: HueBand = HueBand {
    from: 100.0,
    to: 175.0,
};
pub const RED_HUES: HueBand = HueBand {
    from: 345.0,
    to: 40.0,
};

impl HueBand {
    fn contains(self, h: f64) -> bool {
        if self.from <= self.to {
            (self.from..=self.to).contains(&h)
        } else {
            h >= self.from || h <= self.to
        }
    }

    fn width(self) -> f64 {
        (self.to - self.from).rem_euclid(360.0)
    }

    fn centre(self) -> f64 {
        (self.from + self.width() / 2.0).rem_euclid(360.0)
    }

    /// The band with `margin` degrees shaved off each end.
    fn inset(self, margin: f64) -> HueBand {
        HueBand {
            from: (self.from + margin).rem_euclid(360.0),
            to: (self.to - margin).rem_euclid(360.0),
        }
    }

    /// `h` if it is inside the band, else the nearer edge.
    fn pin(self, h: f64) -> f64 {
        if self.contains(h) {
            return h;
        }
        let distance = |edge: f64| {
            let d = (h - edge).rem_euclid(360.0);
            d.min(360.0 - d)
        };
        if distance(self.from) <= distance(self.to) {
            self.from
        } else {
            self.to
        }
    }
}

/// Whether `colour` is saturated enough to read as a hue in `band`.
pub fn reads_as(band: HueBand, colour: Rgba) -> bool {
    let lch = colour.to_oklch();
    lch.c >= HUE_CHROMA && band.contains(lch.h)
}

/// Whether a page this colour is a dark one, so text on it runs light.
pub fn is_dark_page(page: Rgba) -> bool {
    page.luminance() < POLARITY_SPLIT
}

#[derive(Debug, Clone, Copy)]
enum Polarity {
    Dark,
    Light,
}

impl Polarity {
    fn of(page: Rgba) -> Polarity {
        if is_dark_page(page) {
            Polarity::Dark
        } else {
            Polarity::Light
        }
    }

    fn fill_fits(self, fill: Rgba) -> bool {
        match self {
            Polarity::Dark => fill.luminance() <= DARK_FILL_MAX_LUMINANCE,
            Polarity::Light => fill.luminance() >= LIGHT_FILL_MIN_LUMINANCE,
        }
    }

    /// The lightness a fill moves toward to fit.
    fn fill_end(self) -> f64 {
        match self {
            Polarity::Dark => 0.0,
            Polarity::Light => 1.0,
        }
    }

    /// The lightness text moves toward: away from the fills.
    fn text_end(self) -> f64 {
        1.0 - self.fill_end()
    }
}

/// Step the OKLCH lightness of `colour` toward `end` (0 or 1) until `fits`
/// holds. Hue and chroma stay. At the end of the range it returns pure black
/// or white, which the caller's bounds guarantee fits. The result is opaque and
/// quantised, so `fits` judged exactly the colour that gets written.
fn walk_lightness(colour: Rgba, end: f64, fits: impl Fn(Rgba) -> bool) -> Rgba {
    let mut lch = colour.to_oklch();
    lch.l = lch.l.clamp(0.0, 1.0);
    let mut current = colour.quantised();
    // Lightness spans 0..=1, so this many steps reach either end.
    let max_steps = (1.0 / LIGHTNESS_STEP) as usize + 1;
    for _ in 0..max_steps {
        if fits(current) {
            return current;
        }
        if (lch.l - end).abs() <= LIGHTNESS_STEP {
            break;
        }
        lch.l += (end - lch.l).signum() * LIGHTNESS_STEP;
        current = lch.to_rgba().quantised();
    }
    if end >= 0.5 {
        Rgba::WHITE
    } else {
        Rgba::BLACK
    }
}

/// Move `colour` well inside `band` and give it at least the button chroma.
fn pin_hue(colour: Rgba, band: HueBand) -> Rgba {
    let mut lch = colour.to_oklch();
    lch.h = if lch.c < HUE_CHROMA {
        band.centre()
    } else {
        band.inset(HUE_PIN_MARGIN).pin(lch.h)
    };
    lch.c = lch.c.max(MIN_BUTTON_CHROMA);
    lch.to_rgba()
}

/// A button fill and its label. The fill's lightness moves away from the
/// label until the label reads. If no fill can carry that label, the label
/// turns black or white, whichever reads better.
fn button(fill: Rgba, label: Rgba) -> (Rgba, Rgba) {
    let away_from = |label: Rgba| {
        if label.luminance() > POLARITY_SPLIT {
            0.0
        } else {
            1.0
        }
    };
    let reads = |label: Rgba| move |f: Rgba| contrast(f, label) >= TEXT_CONTRAST;
    let label = label.quantised();
    let walked = walk_lightness(fill, away_from(label), reads(label));
    if reads(label)(walked) {
        return (walked, label);
    }
    let label = if contrast(fill, Rgba::WHITE) >= contrast(fill, Rgba::BLACK) {
        Rgba::WHITE
    } else {
        Rgba::BLACK
    };
    (walk_lightness(fill, away_from(label), reads(label)), label)
}

/// Where the palette reads a theme's colours in one mode.
pub struct Source<'a> {
    /// A token's colour under the theme, or `None` when it cannot be evaluated.
    pub colour: &'a dyn Fn(&str) -> Option<Rgba>,
    /// A token's colour in the default theme. Always evaluable.
    pub default: &'a dyn Fn(&str) -> Rgba,
    /// The screen's scanline stops. A protected surface draws no fill of its
    /// own, so its text may sit on any of them (ADR 0313).
    pub scanlines: &'a [Rgba],
}

impl Source<'_> {
    fn get(&self, token: &str) -> Rgba {
        (self.colour)(token).unwrap_or_else(|| (self.default)(token))
    }
}

/// Every protected token, in the order `palette` returns them.
pub const PROTECTED_TOKENS: [&str; 18] = [
    "--protected-bg",
    "--protected-surface",
    "--protected-inset",
    "--protected-raised",
    "--protected-picked",
    "--protected-text",
    "--protected-text-muted",
    "--protected-accent",
    "--protected-caution",
    "--protected-confirm-text",
    "--protected-danger-text",
    "--protected-confirm",
    "--protected-on-confirm",
    "--protected-danger",
    "--protected-on-danger",
    "--protected-action",
    "--protected-on-action",
    "--protected-scrim",
];

/// The protected palette for one mode, as `(token, literal value)` pairs.
pub fn palette(source: &Source) -> Vec<(&'static str, String)> {
    let page = source.get("--bg-primary").over(Rgba::WHITE);
    let polarity = Polarity::of(page);
    let fill = |c: Rgba| walk_lightness(c, polarity.fill_end(), |c| polarity.fill_fits(c));
    let bg = fill(page);
    let raised = fill(source.get("--bg-tertiary").over(bg));
    // A picked option wears a tint of the accent over the raised fill.
    let picked = fill(source.get("--accent").over(bg).mix(PICKED_TINT, raised));
    let fills = [
        bg,
        fill(source.get("--surface-bg").over(bg)),
        fill(source.get("--bg-secondary").over(bg)),
        raised,
        picked,
    ];
    let bands: Vec<Rgba> = source.scanlines.iter().map(|s| s.over(bg)).collect();
    let readable = |c: Rgba| {
        walk_lightness(c, polarity.text_end(), |c| {
            fills
                .iter()
                .chain(&bands)
                .all(|f| contrast(c, *f) >= TEXT_CONTRAST)
        })
    };
    let text = |token: &str| readable(source.get(token).over(bg));
    let green = pin_hue(source.get("--accent-green").over(bg), GREEN_HUES);
    let red = pin_hue(source.get("--accent-red").over(bg), RED_HUES);

    let (confirm, on_confirm) = button(green, Rgba::WHITE);
    let (danger, on_danger) = button(red, Rgba::WHITE);
    // A neutral action painted red would read as destructive.
    let action = match source.get("--accent-action").over(bg) {
        red if reads_as(RED_HUES, red) => (source.default)("--accent-action").over(bg),
        action => action,
    };
    let (action, on_action) = button(action, source.get("--text-on-accent").over(action));

    let scrim = source.get("--scrim");
    let tint = Rgba { a: 1.0, ..scrim };
    let tint = if tint.luminance() > MAX_SCRIM_LUMINANCE {
        Rgba::BLACK
    } else {
        tint
    };
    let scrim = Rgba {
        a: scrim.a.max(MIN_SCRIM_ALPHA),
        ..tint
    };

    let values = [
        fills[0].to_hex(),
        fills[1].to_hex(),
        fills[2].to_hex(),
        fills[3].to_hex(),
        fills[4].to_hex(),
        text("--text-primary").to_hex(),
        text("--text-secondary").to_hex(),
        text("--accent").to_hex(),
        text("--accent-yellow").to_hex(),
        readable(green).to_hex(),
        readable(red).to_hex(),
        confirm.to_hex(),
        on_confirm.to_hex(),
        danger.to_hex(),
        on_danger.to_hex(),
        action.to_hex(),
        on_action.to_hex(),
        scrim.to_rgba_css(),
    ];
    PROTECTED_TOKENS.into_iter().zip(values).collect()
}
