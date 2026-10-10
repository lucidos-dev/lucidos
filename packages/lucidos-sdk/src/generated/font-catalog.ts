// AUTO-GENERATED. Do not edit by hand.
// Regenerate: cargo test -p lucidos-engine --lib generate_font_catalog_files -- --ignored
//
// Source of truth: FONT_CATALOG in crates/lucidos-engine/src/core/fonts.rs.

export type FontId =
  | 'system'
  | 'geist'
  | 'atkinson-hyperlegible-next'
  | 'inter'
  | 'roboto'
  | 'open-sans'
  | 'manrope'
  | 'source-serif-4'
  | 'lora'
  | 'literata'
  | 'fira-code'
  | 'monospace'
  | 'geist-mono'
  | 'atkinson-hyperlegible-mono'
  | 'jetbrains-mono'
  | 'ibm-plex-mono'
  | 'source-code-pro'
  | 'commit-mono'
  | 'cascadia-code'
  | 'vt323';

export type FontKind = 'ui' | 'mono' | 'both';
export type FontGroup = 'sans' | 'serif' | 'mono';
export type FontSource = 'vendored' | 'device';

export interface FontEntry {
  id: FontId;
  label: string;
  stack: string;
  kind: FontKind;
  /** The generic family the stack ends in. Settings groups by it. */
  group: FontGroup;
  source: FontSource;
  /** Programming ligatures, applied to code and never to prose. */
  ligatures: boolean;
  /** Has a bold face. Without one, bold text paints with the regular outlines. */
  bold: boolean;
}

/** The `font-family` value that follows the active theme's suggestion. */
export const FOLLOW_THEME = 'theme';

/** The font a device paints when neither the user nor the theme names one. */
export const FALLBACK_FONT: FontId = 'fira-code';

/** A workspace font's id starts with this (ADR 0308). */
export const WORKSPACE_FONT_ID_PREFIX = 'ws-';

/** The chain a workspace font's stack falls back to, by its group. */
export const WORKSPACE_FONT_FALLBACKS: Readonly<Record<FontGroup, string>> = {
  sans: "system-ui, -apple-system, 'Segoe UI', sans-serif",
  serif: "Georgia, 'Times New Roman', serif",
  mono: "ui-monospace, SFMono-Regular, Menlo, Consolas, monospace",
};

/** The engine's caps on a workspace font, which clients re-check. */
export const WORKSPACE_FONT_LIMITS = { slug: 40, label: 60, faces: 16, fonts: 100 } as const;

export const FONT_CATALOG: readonly FontEntry[] = [
  {
    id: 'system',
    label: "System",
    stack: "system-ui, -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif",
    kind: 'ui',
    group: 'sans',
    source: 'device',
    ligatures: false,
    bold: true,
  },
  {
    id: 'geist',
    label: "Geist",
    stack: "'Geist', system-ui, -apple-system, 'Segoe UI', sans-serif",
    kind: 'ui',
    group: 'sans',
    source: 'vendored',
    ligatures: false,
    bold: true,
  },
  {
    id: 'atkinson-hyperlegible-next',
    label: "Atkinson Hyperlegible Next",
    stack: "'Atkinson Hyperlegible Next', system-ui, -apple-system, 'Segoe UI', sans-serif",
    kind: 'ui',
    group: 'sans',
    source: 'vendored',
    ligatures: false,
    bold: true,
  },
  {
    id: 'inter',
    label: "Inter",
    stack: "'Inter', system-ui, -apple-system, 'Segoe UI', sans-serif",
    kind: 'ui',
    group: 'sans',
    source: 'vendored',
    ligatures: false,
    bold: true,
  },
  {
    id: 'roboto',
    label: "Roboto",
    stack: "'Roboto', system-ui, -apple-system, 'Segoe UI', sans-serif",
    kind: 'ui',
    group: 'sans',
    source: 'vendored',
    ligatures: false,
    bold: true,
  },
  {
    id: 'open-sans',
    label: "Open Sans",
    stack: "'Open Sans', system-ui, -apple-system, 'Segoe UI', sans-serif",
    kind: 'ui',
    group: 'sans',
    source: 'vendored',
    ligatures: false,
    bold: true,
  },
  {
    id: 'manrope',
    label: "Manrope",
    stack: "'Manrope', system-ui, -apple-system, 'Segoe UI', sans-serif",
    kind: 'ui',
    group: 'sans',
    source: 'vendored',
    ligatures: false,
    bold: true,
  },
  {
    id: 'source-serif-4',
    label: "Source Serif 4",
    stack: "'Source Serif 4', Georgia, 'Times New Roman', serif",
    kind: 'ui',
    group: 'serif',
    source: 'vendored',
    ligatures: false,
    bold: true,
  },
  {
    id: 'lora',
    label: "Lora",
    stack: "'Lora', Georgia, 'Times New Roman', serif",
    kind: 'ui',
    group: 'serif',
    source: 'vendored',
    ligatures: false,
    bold: true,
  },
  {
    id: 'literata',
    label: "Literata",
    stack: "'Literata', Georgia, 'Times New Roman', serif",
    kind: 'ui',
    group: 'serif',
    source: 'vendored',
    ligatures: false,
    bold: true,
  },
  {
    id: 'fira-code',
    label: "Fira Code",
    stack: "'Fira Code', ui-monospace, SFMono-Regular, 'SF Mono', Menlo, 'JetBrains Mono', Monaco, Consolas, monospace",
    kind: 'both',
    group: 'mono',
    source: 'vendored',
    ligatures: true,
    bold: true,
  },
  {
    id: 'monospace',
    label: "Monospace",
    stack: "ui-monospace, SFMono-Regular, 'SF Mono', Menlo, 'Fira Code', 'JetBrains Mono', Monaco, Consolas, monospace",
    kind: 'both',
    group: 'mono',
    source: 'device',
    ligatures: false,
    bold: true,
  },
  {
    id: 'geist-mono',
    label: "Geist Mono",
    stack: "'Geist Mono', ui-monospace, SFMono-Regular, Menlo, Consolas, monospace",
    kind: 'both',
    group: 'mono',
    source: 'vendored',
    ligatures: false,
    bold: true,
  },
  {
    id: 'atkinson-hyperlegible-mono',
    label: "Atkinson Hyperlegible Mono",
    stack: "'Atkinson Hyperlegible Mono', ui-monospace, SFMono-Regular, Menlo, Consolas, monospace",
    kind: 'both',
    group: 'mono',
    source: 'vendored',
    ligatures: false,
    bold: true,
  },
  {
    id: 'jetbrains-mono',
    label: "JetBrains Mono",
    stack: "'JetBrains Mono', ui-monospace, SFMono-Regular, Menlo, Consolas, monospace",
    kind: 'both',
    group: 'mono',
    source: 'vendored',
    ligatures: true,
    bold: true,
  },
  {
    id: 'ibm-plex-mono',
    label: "IBM Plex Mono",
    stack: "'IBM Plex Mono', ui-monospace, SFMono-Regular, Menlo, Consolas, monospace",
    kind: 'both',
    group: 'mono',
    source: 'vendored',
    ligatures: false,
    bold: true,
  },
  {
    id: 'source-code-pro',
    label: "Source Code Pro",
    stack: "'Source Code Pro', ui-monospace, SFMono-Regular, Menlo, Consolas, monospace",
    kind: 'both',
    group: 'mono',
    source: 'vendored',
    ligatures: false,
    bold: true,
  },
  {
    id: 'commit-mono',
    label: "Commit Mono",
    stack: "'Commit Mono', ui-monospace, SFMono-Regular, Menlo, Consolas, monospace",
    kind: 'both',
    group: 'mono',
    source: 'vendored',
    ligatures: false,
    bold: true,
  },
  {
    id: 'cascadia-code',
    label: "Cascadia Code",
    stack: "'Cascadia Code', ui-monospace, SFMono-Regular, Menlo, Consolas, monospace",
    kind: 'both',
    group: 'mono',
    source: 'vendored',
    ligatures: true,
    bold: true,
  },
  {
    id: 'vt323',
    label: "VT323",
    stack: "'VT323', ui-monospace, SFMono-Regular, Menlo, Consolas, monospace",
    kind: 'both',
    group: 'mono',
    source: 'vendored',
    ligatures: false,
    bold: false,
  },
];
