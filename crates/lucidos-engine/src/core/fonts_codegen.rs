//! The font catalog, rendered for the SDK and the shell's `@font-face` rules.

use super::*;
use crate::codegen::{quoted, REGENERATE};

fn kind_name(kind: FontKind) -> &'static str {
    match kind {
        FontKind::Ui => "ui",
        FontKind::Mono => "mono",
        FontKind::Both => "both",
    }
}

fn group_name(group: FontGroup) -> &'static str {
    match group {
        FontGroup::Sans => "sans",
        FontGroup::Serif => "serif",
        FontGroup::Mono => "mono",
    }
}

/// The SDK's view of the catalog. Only what a client needs: no bytes, no
/// license, no face list.
pub(crate) fn render_ts() -> String {
    let mut out = String::new();
    out.push_str("// AUTO-GENERATED. Do not edit by hand.\n");
    out.push_str(&format!("// Regenerate: {REGENERATE}\n"));
    out.push_str("//\n");
    out.push_str(
        "// Source of truth: FONT_CATALOG in crates/lucidos-engine/src/core/fonts.rs.\n\n",
    );
    out.push_str("export type FontId =\n");
    for font in FONT_CATALOG {
        out.push_str(&format!("  | '{}'\n", font.id));
    }
    out.truncate(out.len() - 1);
    out.push_str(";\n\n");
    out.push_str("export type FontKind = 'ui' | 'mono' | 'both';\n");
    out.push_str("export type FontGroup = 'sans' | 'serif' | 'mono';\n");
    out.push_str("export type FontSource = 'vendored' | 'device';\n\n");
    out.push_str("export interface FontEntry {\n");
    out.push_str("  id: FontId;\n");
    out.push_str("  label: string;\n");
    out.push_str("  stack: string;\n");
    out.push_str("  kind: FontKind;\n");
    out.push_str("  /** The generic family the stack ends in. Settings groups by it. */\n");
    out.push_str("  group: FontGroup;\n");
    out.push_str("  source: FontSource;\n");
    out.push_str("  /** Programming ligatures, applied to code and never to prose. */\n");
    out.push_str("  ligatures: boolean;\n");
    out.push_str(
        "  /** Has a bold face. Without one, bold text paints with the regular outlines. */\n",
    );
    out.push_str("  bold: boolean;\n");
    out.push_str("}\n\n");
    out.push_str("/** The `font-family` value that follows the active theme's suggestion. */\n");
    out.push_str(&format!(
        "export const FOLLOW_THEME = '{FOLLOW_THEME}';\n\n"
    ));
    out.push_str(
        "/** The font a device paints when neither the user nor the theme names one. */\n",
    );
    out.push_str(&format!(
        "export const FALLBACK_FONT: FontId = '{FALLBACK_FONT}';\n\n"
    ));
    out.push_str("/** A workspace font's id starts with this (ADR 0308). */\n");
    out.push_str(&format!(
        "export const WORKSPACE_FONT_ID_PREFIX = '{}';\n\n",
        crate::core::workspace_fonts::ID_PREFIX
    ));
    out.push_str("/** The chain a workspace font's stack falls back to, by its group. */\n");
    out.push_str(
        "export const WORKSPACE_FONT_FALLBACKS: Readonly<Record<FontGroup, string>> = {\n",
    );
    for group in crate::core::workspace_fonts::GROUPS {
        out.push_str(&format!(
            "  {}: {},\n",
            group_name(group),
            quoted(crate::core::workspace_fonts::fallback(group))
        ));
    }
    out.push_str("};\n\n");
    {
        use crate::core::workspace_fonts::{MAX_FACES, MAX_FONTS, MAX_LABEL_CHARS, MAX_SLUG_LEN};
        out.push_str("/** The engine's caps on a workspace font, which clients re-check. */\n");
        out.push_str(&format!(
            "export const WORKSPACE_FONT_LIMITS = {{ slug: {MAX_SLUG_LEN}, label: {MAX_LABEL_CHARS}, faces: {MAX_FACES}, fonts: {MAX_FONTS} }} as const;\n\n"
        ));
    }
    out.push_str("export const FONT_CATALOG: readonly FontEntry[] = [\n");
    for font in FONT_CATALOG {
        out.push_str("  {\n");
        out.push_str(&format!("    id: '{}',\n", font.id));
        out.push_str(&format!("    label: {},\n", quoted(font.label)));
        out.push_str(&format!("    stack: {},\n", quoted(font.stack)));
        out.push_str(&format!("    kind: '{}',\n", kind_name(font.kind)));
        out.push_str(&format!("    group: '{}',\n", group_name(font.group())));
        out.push_str(&format!("    source: '{}',\n", font.source_name()));
        out.push_str(&format!("    ligatures: {},\n", font.ligatures));
        out.push_str(&format!("    bold: {},\n", font.has_bold()));
        out.push_str("  },\n");
    }
    out.push_str("];\n");
    out
}

/// The host's `@font-face` rules. Vite hashes each file into `assets/`.
pub(crate) fn render_css() -> String {
    let mut out = String::new();
    out.push_str("/* AUTO-GENERATED. Do not edit by hand.\n");
    out.push_str(&format!("   Regenerate: {REGENERATE}\n"));
    out.push_str(
        "   Source of truth: FONT_CATALOG in crates/lucidos-engine/src/core/fonts.rs.\n\n",
    );
    out.push_str("   Every vendored font, for the host. A face downloads only once text uses\n");
    out.push_str("   it, so an unused font costs nothing but this rule. */\n");
    for font in FONT_CATALOG {
        let rules = font.font_face_rules(|face| format!("../../assets/fonts/{}", face.asset));
        if !rules.is_empty() {
            out.push('\n');
            out.push_str(&rules);
        }
    }
    out
}
