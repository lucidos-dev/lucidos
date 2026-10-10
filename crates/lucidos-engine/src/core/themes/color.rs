//! Colour evaluation for themes: a token's CSS value to sRGB, so the engine can
//! measure contrast and clamp the protected palette (`protected.rs`).
//!
//! It reads the forms themes use: hex, `rgb()`, `hsl()`, `oklch()`, `oklab()`,
//! named colours, `var()` and `color-mix()` in `srgb` or `oklab`. Anything else
//! evaluates to `None`, and every caller treats `None` as "cannot prove it is
//! safe", never as a pass.

use std::collections::HashMap;
use std::sync::LazyLock;

/// How deep `var()` may chain before evaluation gives up. It also stops a
/// cycle such as `--a: var(--b); --b: var(--a)`.
const MAX_VAR_DEPTH: usize = 16;

/// A gamma-encoded sRGB colour, every channel in `0.0..=1.0`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rgba {
    pub r: f64,
    pub g: f64,
    pub b: f64,
    pub a: f64,
}

impl Rgba {
    pub const BLACK: Rgba = Rgba::opaque(0.0, 0.0, 0.0);
    pub const WHITE: Rgba = Rgba::opaque(1.0, 1.0, 1.0);
    const TRANSPARENT: Rgba = Rgba {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 0.0,
    };

    pub const fn opaque(r: f64, g: f64, b: f64) -> Rgba {
        Rgba { r, g, b, a: 1.0 }
    }

    fn clamped(self) -> Rgba {
        Rgba {
            r: self.r.clamp(0.0, 1.0),
            g: self.g.clamp(0.0, 1.0),
            b: self.b.clamp(0.0, 1.0),
            a: self.a.clamp(0.0, 1.0),
        }
    }

    /// This colour painted over `backdrop`, the way the browser composites it.
    pub fn over(self, backdrop: Rgba) -> Rgba {
        let a = self.a + backdrop.a * (1.0 - self.a);
        if a == 0.0 {
            return Rgba::TRANSPARENT;
        }
        let mix = |fg: f64, bg: f64| (fg * self.a + bg * backdrop.a * (1.0 - self.a)) / a;
        Rgba {
            r: mix(self.r, backdrop.r),
            g: mix(self.g, backdrop.g),
            b: mix(self.b, backdrop.b),
            a,
        }
    }

    /// WCAG 2 relative luminance of the colour's own channels.
    pub fn luminance(self) -> f64 {
        let [r, g, b] = self.linear();
        0.2126 * r + 0.7152 * g + 0.0722 * b
    }

    fn linear(self) -> [f64; 3] {
        let lin = |c: f64| {
            if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        [lin(self.r), lin(self.g), lin(self.b)]
    }

    fn from_linear([r, g, b]: [f64; 3], a: f64) -> Rgba {
        let enc = |c: f64| {
            if c <= 0.003_130_8 {
                12.92 * c
            } else {
                1.055 * c.powf(1.0 / 2.4) - 0.055
            }
        };
        Rgba {
            r: enc(r),
            g: enc(g),
            b: enc(b),
            a,
        }
        .clamped()
    }

    fn channel_byte(c: f64) -> u8 {
        (c.clamp(0.0, 1.0) * 255.0).round() as u8
    }

    /// `weight` of `self` mixed into `other` in sRGB, as `color-mix(in srgb)`
    /// does for two opaque colours.
    pub fn mix(self, weight: f64, other: Rgba) -> Rgba {
        let lerp = |a: f64, b: f64| a * weight + b * (1.0 - weight);
        Rgba::opaque(
            lerp(self.r, other.r),
            lerp(self.g, other.g),
            lerp(self.b, other.b),
        )
    }

    /// The opaque colour `to_hex` writes, so a check runs on what is emitted.
    pub fn quantised(self) -> Rgba {
        let q = |c: f64| f64::from(Self::channel_byte(c)) / 255.0;
        Rgba::opaque(q(self.r), q(self.g), q(self.b))
    }

    /// `#rrggbb`, ignoring alpha. Callers composite first.
    pub fn to_hex(self) -> String {
        format!(
            "#{:02x}{:02x}{:02x}",
            Self::channel_byte(self.r),
            Self::channel_byte(self.g),
            Self::channel_byte(self.b)
        )
    }

    /// `rgba(r, g, b, a)`, alpha rounded to two places.
    pub fn to_rgba_css(self) -> String {
        format!(
            "rgba({}, {}, {}, {})",
            Self::channel_byte(self.r),
            Self::channel_byte(self.g),
            Self::channel_byte(self.b),
            (self.a * 100.0).round() / 100.0
        )
    }

    pub fn to_oklch(self) -> Oklch {
        let [l, a, b] = self.to_oklab();
        let h = b.atan2(a).to_degrees().rem_euclid(360.0);
        Oklch {
            l,
            c: a.hypot(b),
            h,
        }
    }

    fn to_oklab(self) -> [f64; 3] {
        let [r, g, b] = self.linear();
        let l = (0.412_221_470_8 * r + 0.536_332_536_3 * g + 0.051_445_992_9 * b).cbrt();
        let m = (0.211_903_498_2 * r + 0.680_699_545_1 * g + 0.107_396_956_6 * b).cbrt();
        let s = (0.088_302_461_9 * r + 0.281_718_837_6 * g + 0.629_978_700_5 * b).cbrt();
        [
            0.210_454_255_3 * l + 0.793_617_785_0 * m - 0.004_072_046_8 * s,
            1.977_998_495_1 * l - 2.428_592_205_0 * m + 0.450_593_709_9 * s,
            0.025_904_037_1 * l + 0.782_771_766_2 * m - 0.808_675_766_0 * s,
        ]
    }

    /// An OKLab colour, clipped into the sRGB gamut channel by channel.
    fn from_oklab(lab: [f64; 3], alpha: f64) -> Rgba {
        Rgba::from_linear(Rgba::oklab_to_linear(lab), alpha)
    }

    fn oklab_to_linear([l, a, b]: [f64; 3]) -> [f64; 3] {
        let l_ = (l + 0.396_337_777_4 * a + 0.215_803_757_3 * b).powi(3);
        let m_ = (l - 0.105_561_345_8 * a - 0.063_854_172_8 * b).powi(3);
        let s_ = (l - 0.089_484_177_5 * a - 1.291_485_548_0 * b).powi(3);
        [
            4.076_741_662_1 * l_ - 3.307_711_591_3 * m_ + 0.230_969_929_2 * s_,
            -1.268_438_004_6 * l_ + 2.609_757_401_1 * m_ - 0.341_319_396_5 * s_,
            -0.004_196_086_3 * l_ - 0.703_418_614_7 * m_ + 1.707_614_701_0 * s_,
        ]
    }
}

/// OKLCH: perceptual lightness `0..=1`, chroma, and hue in degrees.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Oklch {
    pub l: f64,
    pub c: f64,
    pub h: f64,
}

impl Oklch {
    /// The sRGB colour, with chroma reduced until it fits the gamut. Reducing
    /// chroma keeps the hue, where clipping each channel would shift it.
    pub fn to_rgba(self) -> Rgba {
        let (sin, cos) = self.h.to_radians().sin_cos();
        let linear = |c: f64| Rgba::oklab_to_linear([self.l, c * cos, c * sin]);
        let in_gamut = |c: f64| linear(c).iter().all(|ch| (-1e-4..=1.0 + 1e-4).contains(ch));
        let chroma = if in_gamut(self.c) {
            self.c
        } else {
            let (mut lo, mut hi) = (0.0, self.c);
            for _ in 0..24 {
                let mid = (lo + hi) / 2.0;
                if in_gamut(mid) {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            lo
        };
        Rgba::from_linear(linear(chroma), 1.0)
    }
}

/// The WCAG 2 contrast ratio of two opaque colours, `1.0..=21.0`.
pub fn contrast(a: Rgba, b: Rgba) -> f64 {
    let (la, lb) = (a.luminance(), b.luminance());
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// Evaluate a CSS colour value. `lookup` gives the value a custom property
/// holds, for `var()`.
pub fn evaluate(value: &str, lookup: &dyn Fn(&str) -> Option<String>) -> Option<Rgba> {
    eval(value.trim(), lookup, 0).filter(|c| [c.r, c.g, c.b, c.a].iter().all(|ch| ch.is_finite()))
}

fn eval(value: &str, lookup: &dyn Fn(&str) -> Option<String>, depth: usize) -> Option<Rgba> {
    if depth > MAX_VAR_DEPTH {
        return None;
    }
    let value = value.trim().to_ascii_lowercase();
    if let Some(hex) = value.strip_prefix('#') {
        return parse_hex(hex);
    }
    let Some((name, args)) = function_call(&value) else {
        return named(&value);
    };
    match name {
        "var" => eval_var(args, lookup, depth),
        "color-mix" => eval_color_mix(args, lookup, depth),
        "rgb" | "rgba" => parse_rgb(args),
        "hsl" | "hsla" => parse_hsl(args),
        "oklch" => parse_oklch(args),
        "oklab" => parse_oklab(args),
        _ => None,
    }
}

/// `name(args)` when the whole value is one call, else `None`.
pub(super) fn function_call(value: &str) -> Option<(&str, &str)> {
    let open = value.find('(')?;
    let inner = value[open + 1..].strip_suffix(')')?;
    let name = &value[..open];
    let balanced = inner.chars().try_fold(0i32, |depth, c| {
        let next = match c {
            '(' => depth + 1,
            ')' => depth - 1,
            _ => depth,
        };
        (next >= 0).then_some(next)
    }) == Some(0);
    let named = !name.is_empty() && name.chars().all(|c| c.is_ascii_alphabetic() || c == '-');
    (balanced && named).then_some((name, inner))
}

/// Split at `sep` characters outside parentheses, dropping empty pieces.
pub fn split_top_level(value: &str, sep: impl Fn(char) -> bool) -> Vec<&str> {
    let mut parts = Vec::new();
    let (mut depth, mut start) = (0i32, 0);
    for (i, c) in value.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            c if depth == 0 && sep(c) => {
                parts.push(&value[start..i]);
                start = i + c.len_utf8();
            }
            _ => {}
        }
    }
    parts.push(&value[start..]);
    parts
        .into_iter()
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .collect()
}

fn eval_var(args: &str, lookup: &dyn Fn(&str) -> Option<String>, depth: usize) -> Option<Rgba> {
    let (name, fallback) = match args.split_once(',') {
        Some((name, fallback)) => (name.trim(), Some(fallback)),
        None => (args.trim(), None),
    };
    match lookup(name) {
        Some(held) => eval(&held, lookup, depth + 1),
        None => fallback.and_then(|f| eval(f, lookup, depth + 1)),
    }
}

/// One `color-mix()` input: a colour and its optional percentage.
fn mix_input<'a>(arg: &'a str) -> Option<(String, Option<f64>)> {
    let pieces = split_top_level(arg, char::is_whitespace);
    let mut pct = None;
    let mut colour: Vec<&'a str> = Vec::new();
    for piece in pieces {
        match piece.strip_suffix('%').and_then(finite) {
            Some(p) if pct.is_none() => pct = Some(p / 100.0),
            _ => colour.push(piece),
        }
    }
    (!colour.is_empty()).then(|| (colour.join(" "), pct))
}

fn eval_color_mix(
    args: &str,
    lookup: &dyn Fn(&str) -> Option<String>,
    depth: usize,
) -> Option<Rgba> {
    let parts = split_top_level(args, |c| c == ',');
    let [space, first, second] = parts.as_slice() else {
        return None;
    };
    let (c1, p1) = mix_input(first)?;
    let (c2, p2) = mix_input(second)?;
    let (p1, p2) = match (p1, p2) {
        (None, None) => (0.5, 0.5),
        (Some(p), None) => (p, 1.0 - p),
        (None, Some(p)) => (1.0 - p, p),
        (Some(a), Some(b)) => (a, b),
    };
    let total = p1 + p2;
    if total <= 0.0 {
        return None;
    }
    let (w1, w2, alpha_scale) = (p1 / total, p2 / total, total.min(1.0));
    let c1 = eval(&c1, lookup, depth + 1)?;
    let c2 = eval(&c2, lookup, depth + 1)?;
    let alpha = c1.a * w1 + c2.a * w2;
    let blend = |x: f64, y: f64| {
        if alpha == 0.0 {
            0.0
        } else {
            (x * c1.a * w1 + y * c2.a * w2) / alpha
        }
    };
    let alpha_out = alpha * alpha_scale;
    match space.split_whitespace().collect::<Vec<_>>().as_slice() {
        ["in", "srgb"] => Some(Rgba {
            r: blend(c1.r, c2.r),
            g: blend(c1.g, c2.g),
            b: blend(c1.b, c2.b),
            a: alpha_out,
        }),
        ["in", "oklab"] => {
            let (a, b) = (c1.to_oklab(), c2.to_oklab());
            Some(Rgba::from_oklab(
                [blend(a[0], b[0]), blend(a[1], b[1]), blend(a[2], b[2])],
                alpha_out,
            ))
        }
        _ => None,
    }
}

fn parse_hex(hex: &str) -> Option<Rgba> {
    if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let nibble = |i: usize| u8::from_str_radix(&hex[i..=i], 16).ok().map(|n| n * 17);
    let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    let bytes: Vec<u8> = match hex.len() {
        3 | 4 => (0..hex.len()).map(nibble).collect::<Option<_>>()?,
        6 | 8 => (0..hex.len()).step_by(2).map(byte).collect::<Option<_>>()?,
        _ => return None,
    };
    let f = |b: u8| f64::from(b) / 255.0;
    Some(Rgba {
        r: f(bytes[0]),
        g: f(bytes[1]),
        b: f(bytes[2]),
        a: bytes.get(3).map_or(1.0, |a| f(*a)),
    })
}

/// A finite number. Rust also parses `nan`, `inf` and `1e999`, and a NaN
/// channel would never settle the palette's lightness walk.
fn finite(text: &str) -> Option<f64> {
    text.parse::<f64>().ok().filter(|v| v.is_finite())
}

/// A numeric component: `(value, is_percentage)`. `none` reads as zero.
fn number(token: &str) -> Option<(f64, bool)> {
    if token == "none" {
        return Some((0.0, false));
    }
    if let Some(p) = token.strip_suffix('%') {
        return finite(p).map(|v| (v, true));
    }
    finite(token.strip_suffix("deg").unwrap_or(token)).map(|v| (v, false))
}

/// The three channels and optional alpha of a colour function, in legacy
/// comma or modern slash syntax. A nested call makes the value unevaluable.
fn components(args: &str) -> Option<([(f64, bool); 3], f64)> {
    if args.contains('(') {
        return None;
    }
    let parts: Vec<(f64, bool)> = args
        .split(|c: char| c == ',' || c == '/' || c.is_whitespace())
        .filter(|p| !p.is_empty())
        .map(number)
        .collect::<Option<_>>()?;
    let alpha = match parts.get(3) {
        None => 1.0,
        Some((a, true)) => a / 100.0,
        Some((a, false)) => *a,
    };
    match parts.len() {
        3 | 4 => Some(([parts[0], parts[1], parts[2]], alpha.clamp(0.0, 1.0))),
        _ => None,
    }
}

fn parse_rgb(args: &str) -> Option<Rgba> {
    let (channels, a) = components(args)?;
    let ch = |(v, pct): (f64, bool)| if pct { v / 100.0 } else { v / 255.0 };
    Some(
        Rgba {
            r: ch(channels[0]),
            g: ch(channels[1]),
            b: ch(channels[2]),
            a,
        }
        .clamped(),
    )
}

fn parse_hsl(args: &str) -> Option<Rgba> {
    let ([(h, _), (s, _), (l, _)], a) = components(args)?;
    let (s, l) = ((s / 100.0).clamp(0.0, 1.0), (l / 100.0).clamp(0.0, 1.0));
    let k = |n: f64| (n + h.rem_euclid(360.0) / 30.0).rem_euclid(12.0);
    let chroma = s * l.min(1.0 - l);
    let f = |n: f64| l - chroma * (k(n) - 3.0).min(9.0 - k(n)).clamp(-1.0, 1.0);
    Some(Rgba {
        r: f(0.0),
        g: f(8.0),
        b: f(4.0),
        a,
    })
}

/// OKLab lightness: a number, or a percentage of 1.
fn ok_lightness((v, pct): (f64, bool)) -> f64 {
    if pct {
        v / 100.0
    } else {
        v
    }
}

/// OKLab chroma and axes: a number, or a percentage of 0.4.
fn ok_axis((v, pct): (f64, bool)) -> f64 {
    if pct {
        v / 100.0 * 0.4
    } else {
        v
    }
}

fn parse_oklch(args: &str) -> Option<Rgba> {
    let ([l, c, (h, _)], a) = components(args)?;
    let rgb = Oklch {
        l: ok_lightness(l),
        c: ok_axis(c),
        h,
    }
    .to_rgba();
    Some(Rgba { a, ..rgb })
}

fn parse_oklab(args: &str) -> Option<Rgba> {
    let ([l, x, y], a) = components(args)?;
    Some(Rgba::from_oklab(
        [ok_lightness(l), ok_axis(x), ok_axis(y)],
        a,
    ))
}

/// The CSS named colours, as `name rrggbb` pairs.
const NAMED_COLOURS: &str = "aliceblue f0f8ff antiquewhite faebd7 aqua 00ffff aquamarine 7fffd4 \
azure f0ffff beige f5f5dc bisque ffe4c4 black 000000 blanchedalmond ffebcd blue 0000ff \
blueviolet 8a2be2 brown a52a2a burlywood deb887 cadetblue 5f9ea0 chartreuse 7fff00 \
chocolate d2691e coral ff7f50 cornflowerblue 6495ed cornsilk fff8dc crimson dc143c cyan 00ffff \
darkblue 00008b darkcyan 008b8b darkgoldenrod b8860b darkgray a9a9a9 darkgreen 006400 \
darkgrey a9a9a9 darkkhaki bdb76b darkmagenta 8b008b darkolivegreen 556b2f darkorange ff8c00 \
darkorchid 9932cc darkred 8b0000 darksalmon e9967a darkseagreen 8fbc8f darkslateblue 483d8b \
darkslategray 2f4f4f darkslategrey 2f4f4f darkturquoise 00ced1 darkviolet 9400d3 \
deeppink ff1493 deepskyblue 00bfff dimgray 696969 dimgrey 696969 dodgerblue 1e90ff \
firebrick b22222 floralwhite fffaf0 forestgreen 228b22 fuchsia ff00ff gainsboro dcdcdc \
ghostwhite f8f8ff gold ffd700 goldenrod daa520 gray 808080 green 008000 greenyellow adff2f \
grey 808080 honeydew f0fff0 hotpink ff69b4 indianred cd5c5c indigo 4b0082 ivory fffff0 \
khaki f0e68c lavender e6e6fa lavenderblush fff0f5 lawngreen 7cfc00 lemonchiffon fffacd \
lightblue add8e6 lightcoral f08080 lightcyan e0ffff lightgoldenrodyellow fafad2 \
lightgray d3d3d3 lightgreen 90ee90 lightgrey d3d3d3 lightpink ffb6c1 lightsalmon ffa07a \
lightseagreen 20b2aa lightskyblue 87cefa lightslategray 778899 lightslategrey 778899 \
lightsteelblue b0c4de lightyellow ffffe0 lime 00ff00 limegreen 32cd32 linen faf0e6 \
magenta ff00ff maroon 800000 mediumaquamarine 66cdaa mediumblue 0000cd mediumorchid ba55d3 \
mediumpurple 9370db mediumseagreen 3cb371 mediumslateblue 7b68ee mediumspringgreen 00fa9a \
mediumturquoise 48d1cc mediumvioletred c71585 midnightblue 191970 mintcream f5fffa \
mistyrose ffe4e1 moccasin ffe4b5 navajowhite ffdead navy 000080 oldlace fdf5e6 olive 808000 \
olivedrab 6b8e23 orange ffa500 orangered ff4500 orchid da70d6 palegoldenrod eee8aa \
palegreen 98fb98 paleturquoise afeeee palevioletred db7093 papayawhip ffefd5 peachpuff ffdab9 \
peru cd853f pink ffc0cb plum dda0dd powderblue b0e0e6 purple 800080 rebeccapurple 663399 \
red ff0000 rosybrown bc8f8f royalblue 4169e1 saddlebrown 8b4513 salmon fa8072 \
sandybrown f4a460 seagreen 2e8b57 seashell fff5ee sienna a0522d silver c0c0c0 skyblue 87ceeb \
slateblue 6a5acd slategray 708090 slategrey 708090 snow fffafa springgreen 00ff7f \
steelblue 4682b4 tan d2b48c teal 008080 thistle d8bfd8 tomato ff6347 turquoise 40e0d0 \
violet ee82ee wheat f5deb3 white ffffff whitesmoke f5f5f5 yellow ffff00 yellowgreen 9acd32";

static NAMED: LazyLock<HashMap<&'static str, &'static str>> = LazyLock::new(|| {
    let words: Vec<&str> = NAMED_COLOURS.split_whitespace().collect();
    words.chunks(2).map(|pair| (pair[0], pair[1])).collect()
});

/// Whether `name` is a CSS named colour. `transparent` is not one here.
pub fn is_named_colour(name: &str) -> bool {
    NAMED.contains_key(name)
}

/// Every CSS named colour, for the generated TypeScript twin of the part
/// grammar.
#[cfg(test)]
pub fn named_colours() -> impl Iterator<Item = &'static str> {
    NAMED_COLOURS.split_whitespace().step_by(2)
}

fn named(value: &str) -> Option<Rgba> {
    if value == "transparent" {
        return Some(Rgba::TRANSPARENT);
    }
    NAMED.get(value).and_then(|hex| parse_hex(hex))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn none(_: &str) -> Option<String> {
        None
    }

    fn hex(value: &str) -> String {
        evaluate(value, &none).expect(value).to_hex()
    }

    #[test]
    fn reads_every_literal_form_a_theme_uses() {
        assert_eq!(hex("#fff"), "#ffffff");
        assert_eq!(hex("#07172E"), "#07172e");
        assert_eq!(hex("rgb(255, 0, 0)"), "#ff0000");
        assert_eq!(hex("rgb(0 128 0 / 50%)"), "#008000");
        assert_eq!(hex("hsl(120, 100%, 25%)"), "#008000");
        assert_eq!(hex("oklch(0.628 0.2577 29.23)"), "#ff0000");
        assert_eq!(hex("oklab(1 0 0)"), "#ffffff");
        assert_eq!(hex("rebeccapurple"), "#663399");
        let scrim = evaluate("rgba(0, 0, 0, 0.5)", &none).unwrap();
        assert_eq!(scrim.a, 0.5);
        assert_eq!(evaluate("transparent", &none).unwrap().a, 0.0);
    }

    #[test]
    fn follows_var_and_its_fallback() {
        let lookup = |name: &str| (name == "--a").then(|| "var(--b)".to_string());
        assert_eq!(evaluate("var(--a)", &lookup), None, "--b is unset");
        let lookup = |name: &str| match name {
            "--a" => Some("var(--b)".to_string()),
            "--b" => Some("#123456".to_string()),
            _ => None,
        };
        assert_eq!(evaluate("var(--a)", &lookup).unwrap().to_hex(), "#123456");
        assert_eq!(hex("var(--unset, #abcdef)"), "#abcdef");
    }

    #[test]
    fn a_var_cycle_is_unevaluable_not_a_hang() {
        let lookup = |name: &str| match name {
            "--a" => Some("var(--b)".to_string()),
            _ => Some("var(--a)".to_string()),
        };
        assert_eq!(evaluate("var(--a)", &lookup), None);
    }

    #[test]
    fn mixes_in_srgb_and_oklab() {
        assert_eq!(hex("color-mix(in srgb, #000000, #ffffff)"), "#808080");
        assert_eq!(hex("color-mix(in srgb, #ff0000 25%, #0000ff)"), "#4000bf");
        let half = evaluate("color-mix(in srgb, #ff0000 40%, transparent)", &none).unwrap();
        assert_eq!(half.to_hex(), "#ff0000");
        assert!((half.a - 0.4).abs() < 1e-9);
        let mid = evaluate("color-mix(in oklab, #000000 50%, #ffffff)", &none).unwrap();
        assert!((mid.to_oklch().l - 0.5).abs() < 0.01);
    }

    #[test]
    fn anything_else_is_unevaluable() {
        for value in [
            "hsl(infinity, 50%, 50%)",
            "oklch(nan 0 0)",
            "oklab(inf 0 0)",
            "oklch(1e200 0.1 30)",
            "color-mix(in srgb, red 1e999%, blue)",
            "currentcolor",
            "light-dark(#fff, #000)",
            "color-mix(in hsl, red, blue)",
            "calc(1px + 2px)",
            "#12345",
            "rgb(var(--x), 0, 0)",
            "notacolour",
        ] {
            assert_eq!(evaluate(value, &none), None, "{value}");
        }
    }

    #[test]
    fn contrast_matches_wcag() {
        assert!((contrast(Rgba::BLACK, Rgba::WHITE) - 21.0).abs() < 1e-9);
        let grey = evaluate("#767676", &none).unwrap();
        assert!((contrast(grey, Rgba::WHITE) - 4.54).abs() < 0.01);
    }

    #[test]
    fn oklch_round_trips() {
        let c = evaluate("#3fb950", &none).unwrap();
        assert_eq!(c.to_oklch().to_rgba().to_hex(), "#3fb950");
    }

    #[test]
    fn compositing_over_a_backdrop_is_opaque() {
        let veil = evaluate("rgba(0, 0, 0, 0.5)", &none).unwrap();
        let out = veil.over(Rgba::WHITE);
        assert_eq!(out.a, 1.0);
        assert_eq!(out.to_hex(), "#808080");
    }
}
