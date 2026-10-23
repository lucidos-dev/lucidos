//! The theme part catalog, rendered for the frontend, the SDK and app frames.

use super::super::{ThemeMode, TokenMap};
use super::*;
use crate::codegen::REGENERATE;

/// Where a generated stylesheet loads. App frames define and register only
/// the frame colour tokens, so an app's `var(--token, fallback)` still falls
/// back for the rest.
#[derive(Clone, Copy)]
enum Realm {
    Shell,
    Frame,
}

fn header(comment_open: &str, line: &str) -> String {
    format!(
        "{comment_open} AUTO-GENERATED. Do not edit by hand.\n{line} Regenerate: {REGENERATE}\n{blank}\n{line} Source of truth: crates/lucidos-engine/src/core/themes/theme-parts.json\n{line} and the colour tokens of theme-tokens.json.\n",
        blank = line.trim_end()
    )
}

fn json(value: impl serde::Serialize) -> String {
    serde_json::to_string(&value).expect("serialises")
}

fn grammar_ts(grammar: &Grammar) -> String {
    let unit = |u: &grammar::Unit| format!("'{}'", json(u).trim_matches('"'));
    match grammar {
        Grammar::Colour { min_alpha } => match min_alpha {
            Some(a) => format!("{{ grammar: 'colour', minAlpha: {a} }}"),
            None => "{ grammar: 'colour' }".into(),
        },
        Grammar::Shadow {
            unit: u,
            max_layers,
            max_offset,
            max_blur,
            max_spread,
        } => {
            let spread = max_spread.map_or(String::new(), |s| format!(", maxSpread: {s}"));
            format!(
                "{{ grammar: 'shadow', unit: {}, maxLayers: {max_layers}, maxOffset: {max_offset}, maxBlur: {max_blur}{spread} }}",
                unit(u)
            )
        }
        Grammar::DropShadow {
            unit: u,
            max_offset,
            max_blur,
        } => format!(
            "{{ grammar: 'drop-shadow', unit: {}, maxOffset: {max_offset}, maxBlur: {max_blur} }}",
            unit(u)
        ),
        Grammar::Spacing { unit: u, min, max } => format!(
            "{{ grammar: 'spacing', unit: {}, min: {min}, max: {max} }}",
            unit(u)
        ),
        Grammar::Length { unit: u, min, max } => format!(
            "{{ grammar: 'length', unit: {}, min: {min}, max: {max} }}",
            unit(u)
        ),
        Grammar::Weight { min, max } => {
            format!("{{ grammar: 'weight', min: {min}, max: {max} }}")
        }
        Grammar::Keyword { values } => {
            let quoted: Vec<String> = values.iter().map(|v| format!("'{v}'")).collect();
            format!("{{ grammar: 'keyword', values: [{}] }}", quoted.join(", "))
        }
        Grammar::Scanlines {
            max_alpha,
            max_period,
            max_stops,
        } => format!(
            "{{ grammar: 'scanlines', maxAlpha: {max_alpha}, maxPeriod: {max_period}, maxStops: {max_stops} }}"
        ),
    }
}

/// What the apply-site twin needs: each property's grammar, each part
/// token's part and property, the colour tokens and the named colours.
pub(crate) fn render_ts() -> String {
    let mut out = header("//", "//");
    out.push_str("\nexport type PartUnit = 'em' | 'px';\n\n");
    out.push_str("export type PartGrammar =\n");
    out.push_str("  | { grammar: 'colour'; minAlpha?: number }\n");
    out.push_str("  | { grammar: 'shadow'; unit: PartUnit; maxLayers: number; maxOffset: number; maxBlur: number; maxSpread?: number }\n");
    out.push_str(
        "  | { grammar: 'drop-shadow'; unit: PartUnit; maxOffset: number; maxBlur: number }\n",
    );
    out.push_str("  | { grammar: 'spacing'; unit: PartUnit; min: number; max: number }\n");
    out.push_str("  | { grammar: 'length'; unit: PartUnit; min: number; max: number }\n");
    out.push_str("  | { grammar: 'weight'; min: number; max: number }\n");
    out.push_str("  | { grammar: 'keyword'; values: readonly string[] }\n");
    out.push_str(
        "  | { grammar: 'scanlines'; maxAlpha: number; maxPeriod: number; maxStops: number };\n\n",
    );
    out.push_str("export interface PartTokenSpec {\n  part: string;\n  property: string;\n  insetOnly: boolean;\n  frames: boolean;\n}\n\n");
    out.push_str("export const PART_PROPERTIES: Record<string, PartGrammar> = {\n");
    for (name, property) in &CATALOG.properties {
        out.push_str(&format!("  '{name}': {},\n", grammar_ts(&property.grammar)));
    }
    out.push_str("};\n\nexport const PART_TOKENS: Record<string, PartTokenSpec> = {\n");
    for (part, usage) in CATALOG.tokens() {
        out.push_str(&format!(
            "  '{}': {{ part: '{}', property: '{}', insetOnly: {}, frames: {} }},\n",
            usage.token, part.id, usage.name, usage.inset_only, part.frames
        ));
    }
    out.push_str("};\n\nexport const COLOUR_TOKENS: readonly string[] = [\n");
    for (token, _) in super::super::colour_tokens() {
        out.push_str(&format!("  '{token}',\n"));
    }
    out.push_str("];\n\n/** The colour tokens app frames define and register. */\n");
    out.push_str("export const FRAME_COLOUR_TOKENS: readonly string[] = [\n");
    for (token, _) in super::super::colour_tokens().filter(|(_, frames)| *frames) {
        out.push_str(&format!("  '{token}',\n"));
    }
    out.push_str("];\n\nexport const NAMED_COLOURS: readonly string[] = [\n");
    let names: Vec<&str> = super::super::color::named_colours().collect();
    for chunk in names.chunks(6) {
        let quoted: Vec<String> = chunk.iter().map(|n| format!("'{n}'")).collect();
        out.push_str(&format!("  {},\n", quoted.join(", ")));
    }
    out.push_str("];\n\n// The engine's caps on a theme value, from core/themes.\n");
    let caps = [
        ("MAX_MIX_DEPTH", grammar::MAX_MIX_DEPTH.to_string()),
        (
            "MAX_VALUE_LENGTH",
            super::super::MAX_VALUE_LENGTH.to_string(),
        ),
        (
            "MAX_RESOLVED_TOKENS",
            super::super::MAX_RESOLVED_TOKENS.to_string(),
        ),
        ("MAX_SHADOW_PX", super::super::MAX_SHADOW_PX.to_string()),
        ("PX_PER_REM", super::super::PX_PER_REM.to_string()),
    ];
    for (name, value) in caps {
        out.push_str(&format!("export const {name} = {value};\n"));
    }
    let functions: Vec<String> = super::super::SHADOW_COLOUR_FUNCTIONS
        .iter()
        .map(|f| format!("'{f}'"))
        .collect();
    out.push_str(&format!(
        "export const SHADOW_COLOUR_FUNCTIONS: readonly string[] = [{}];\n",
        functions.join(", ")
    ));
    out
}

/// A colour token's `@property` initial value: its dark default as a
/// literal, since an initial value may not name another property. Base CSS
/// sets every token on the root, so this paints only under a broken value.
fn initial_value(token: &str) -> String {
    match super::super::colour_in(&TokenMap::new(), ThemeMode::Dark, token) {
        Some(colour) if colour.a < 1.0 => colour.to_rgba_css(),
        Some(colour) => colour.to_hex(),
        None => "transparent".into(),
    }
}

fn declarations(out: &mut String, names: impl Iterator<Item = String>, value: &str) {
    for name in names {
        out.push_str(&format!("    {name}: {value};\n"));
    }
}

/// The shell's part CSS, which Vite imports.
pub(crate) fn render_shell_css() -> String {
    let mut out = header("/*", "  ");
    out.push_str("*/\n\n");
    out.push_str(&part_css(Realm::Shell));
    out
}

/// The app frames' part CSS, which the engine appends to
/// `/api/v1/sdk-iframe.css` at runtime.
pub(crate) fn frame_css() -> String {
    part_css(Realm::Frame)
}

/// The same frame CSS as a generated file, for the frontend tests that parse
/// every stylesheet an app frame receives.
pub(crate) fn render_frame_css() -> String {
    let mut out = header("/*", "  ");
    out.push_str("*/\n\n");
    out.push_str(&frame_css());
    out
}

/// The part CSS for one realm. Consumers stay hand-written.
fn part_css(realm: Realm) -> String {
    let mut out = String::new();
    out.push_str("/* Every colour token this realm defines is a <color>, so a var() in a part\n   value substitutes a computed colour, never text that could add a shadow\n   layer. */\n");
    let registered = super::super::colour_tokens()
        .filter(|(_, frames)| *frames || matches!(realm, Realm::Shell))
        .map(|(token, _)| token);
    for token in registered {
        out.push_str(&format!(
            "@property {token} {{\n    syntax: '<color>';\n    inherits: true;\n    initial-value: {};\n}}\n\n",
            initial_value(token)
        ));
    }
    let all = || CATALOG.tokens().map(|(_, u)| u.token.clone());
    out.push_str("/* A protected surface: no part token reaches inside, and no inherited\n   part paint flows in from a container. */\n.protected-surface {\n");
    declarations(&mut out, all(), "initial");
    out.push_str("}\n\n:where(.protected-surface) {\n");
    for (name, property) in &CATALOG.properties {
        if let Some(value) = &property.protected_value {
            out.push_str(&format!("    {name}: {value};\n"));
        }
    }
    out.push_str("}\n\n");
    out.push_str("/* theme-effects: reduce drops every part effect: shadows, filters and\n   scanlines. On body, because the tokens sit inline on <html>, where no\n   rule can beat them. */\nhtml[data-theme-effects=\"reduce\"] body {\n");
    declarations(
        &mut out,
        CATALOG
            .tokens()
            .filter(|(_, u)| CATALOG.property(&u.name).effect)
            .map(|(_, u)| u.token.clone()),
        "initial",
    );
    out.push_str("}\n\n");
    out.push_str(
        "/* An app frame that opts out paints no part. */\nhtml[data-theme-parts=\"off\"] body {\n",
    );
    declarations(&mut out, all(), "initial");
    out.push_str("}\n");
    out
}
