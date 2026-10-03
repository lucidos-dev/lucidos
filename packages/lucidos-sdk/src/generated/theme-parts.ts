// AUTO-GENERATED. Do not edit by hand.
// Regenerate: cargo test -p lucidos-engine --lib generate_theme_parts_files -- --ignored
//
// Source of truth: crates/lucidos-engine/src/core/themes/theme-parts.json
// and the colour tokens of theme-tokens.json.

export type PartUnit = 'em' | 'px';

export type PartGrammar =
  | { grammar: 'colour'; minAlpha?: number }
  | { grammar: 'shadow'; unit: PartUnit; maxLayers: number; maxOffset: number; maxBlur: number; maxSpread?: number }
  | { grammar: 'drop-shadow'; unit: PartUnit; maxOffset: number; maxBlur: number }
  | { grammar: 'spacing'; unit: PartUnit; min: number; max: number }
  | { grammar: 'length'; unit: PartUnit; min: number; max: number }
  | { grammar: 'keyword'; values: readonly string[] }
  | { grammar: 'scanlines'; maxAlpha: number; maxPeriod: number; maxStops: number };

export interface PartTokenSpec {
  part: string;
  property: string;
  insetOnly: boolean;
  frames: boolean;
}

export const PART_PROPERTIES: Record<string, PartGrammar> = {
  'background-color': { grammar: 'colour' },
  'background-image': { grammar: 'scanlines', maxAlpha: 0.25, maxPeriod: 8, maxStops: 4 },
  'border-color': { grammar: 'colour' },
  'border-style': { grammar: 'keyword', values: ['solid', 'double'] },
  'border-width': { grammar: 'length', unit: 'px', min: 1, max: 4 },
  'box-shadow': { grammar: 'shadow', unit: 'px', maxLayers: 2, maxOffset: 4, maxBlur: 16, maxSpread: 2 },
  'caret-color': { grammar: 'colour', minAlpha: 0.6 },
  'caret-shape': { grammar: 'keyword', values: ['auto', 'bar', 'block', 'underscore'] },
  'color': { grammar: 'colour', minAlpha: 0.6 },
  'filter': { grammar: 'drop-shadow', unit: 'px', maxOffset: 2, maxBlur: 6 },
  'letter-spacing': { grammar: 'spacing', unit: 'em', min: -0.02, max: 0.12 },
  'text-decoration-color': { grammar: 'colour' },
  'text-shadow': { grammar: 'shadow', unit: 'em', maxLayers: 2, maxOffset: 0.1, maxBlur: 0.6 },
};

export const PART_TOKENS: Record<string, PartTokenSpec> = {
  '--part-chat-text-color': { part: 'chat-text', property: 'color', insetOnly: false, frames: false },
  '--part-chat-text-text-shadow': { part: 'chat-text', property: 'text-shadow', insetOnly: false, frames: false },
  '--part-chat-text-letter-spacing': { part: 'chat-text', property: 'letter-spacing', insetOnly: false, frames: false },
  '--part-chat-heading-color': { part: 'chat-heading', property: 'color', insetOnly: false, frames: false },
  '--part-chat-heading-text-shadow': { part: 'chat-heading', property: 'text-shadow', insetOnly: false, frames: false },
  '--part-chat-heading-letter-spacing': { part: 'chat-heading', property: 'letter-spacing', insetOnly: false, frames: false },
  '--part-chat-link-color': { part: 'chat-link', property: 'color', insetOnly: false, frames: false },
  '--part-chat-link-text-shadow': { part: 'chat-link', property: 'text-shadow', insetOnly: false, frames: false },
  '--part-inline-code-color': { part: 'inline-code', property: 'color', insetOnly: false, frames: false },
  '--part-inline-code-background-color': { part: 'inline-code', property: 'background-color', insetOnly: false, frames: false },
  '--part-code-block-background-color': { part: 'code-block', property: 'background-color', insetOnly: false, frames: false },
  '--part-code-block-box-shadow': { part: 'code-block', property: 'box-shadow', insetOnly: false, frames: false },
  '--part-code-block-text-shadow': { part: 'code-block', property: 'text-shadow', insetOnly: false, frames: false },
  '--part-actor-label-text-shadow': { part: 'actor-label', property: 'text-shadow', insetOnly: false, frames: false },
  '--part-actor-label-letter-spacing': { part: 'actor-label', property: 'letter-spacing', insetOnly: false, frames: false },
  '--part-actor-icon-filter': { part: 'actor-icon', property: 'filter', insetOnly: false, frames: false },
  '--part-header-icon-filter': { part: 'header-icon', property: 'filter', insetOnly: false, frames: false },
  '--part-header-title-text-shadow': { part: 'header-title', property: 'text-shadow', insetOnly: false, frames: false },
  '--part-header-title-letter-spacing': { part: 'header-title', property: 'letter-spacing', insetOnly: false, frames: false },
  '--part-composer-background-color': { part: 'composer', property: 'background-color', insetOnly: false, frames: false },
  '--part-composer-border-color': { part: 'composer', property: 'border-color', insetOnly: false, frames: false },
  '--part-composer-box-shadow': { part: 'composer', property: 'box-shadow', insetOnly: true, frames: false },
  '--part-composer-text-color': { part: 'composer-text', property: 'color', insetOnly: false, frames: false },
  '--part-composer-text-caret-color': { part: 'composer-text', property: 'caret-color', insetOnly: false, frames: false },
  '--part-composer-text-caret-shape': { part: 'composer-text', property: 'caret-shape', insetOnly: false, frames: false },
  '--part-composer-text-text-shadow': { part: 'composer-text', property: 'text-shadow', insetOnly: false, frames: false },
  '--part-composer-text-letter-spacing': { part: 'composer-text', property: 'letter-spacing', insetOnly: false, frames: false },
  '--part-card-border-style': { part: 'card', property: 'border-style', insetOnly: false, frames: false },
  '--part-card-border-width': { part: 'card', property: 'border-width', insetOnly: false, frames: false },
  '--part-card-border-color': { part: 'card', property: 'border-color', insetOnly: false, frames: false },
  '--part-surface-border-style': { part: 'surface', property: 'border-style', insetOnly: false, frames: false },
  '--part-surface-border-width': { part: 'surface', property: 'border-width', insetOnly: false, frames: false },
  '--part-surface-border-color': { part: 'surface', property: 'border-color', insetOnly: false, frames: false },
  '--part-screen-background-image': { part: 'screen', property: 'background-image', insetOnly: false, frames: false },
  '--part-app-text-text-shadow': { part: 'app-text', property: 'text-shadow', insetOnly: false, frames: true },
  '--part-app-text-letter-spacing': { part: 'app-text', property: 'letter-spacing', insetOnly: false, frames: true },
  '--part-app-link-text-decoration-color': { part: 'app-link', property: 'text-decoration-color', insetOnly: false, frames: true },
  '--part-app-link-text-shadow': { part: 'app-link', property: 'text-shadow', insetOnly: false, frames: true },
  '--part-app-control-border-color': { part: 'app-control', property: 'border-color', insetOnly: false, frames: true },
  '--part-app-control-box-shadow': { part: 'app-control', property: 'box-shadow', insetOnly: false, frames: true },
};

export const COLOUR_TOKENS: readonly string[] = [
  '--bg-primary',
  '--bg-secondary',
  '--bg-tertiary',
  '--bg-quaternary',
  '--bg-hover',
  '--bg-selected',
  '--border-color',
  '--surface-bg',
  '--picked-surface',
  '--scrim',
  '--text-primary',
  '--text-secondary',
  '--text-muted',
  '--text-on-accent',
  '--text-strong',
  '--accent',
  '--accent-light',
  '--accent-action',
  '--brand-blue',
  '--brand-blue-deep',
  '--initiator-coding-agent',
  '--lucidos-mark-bg-top',
  '--lucidos-mark-bg-bottom',
  '--lucidos-mark-fg',
  '--claude-mark',
  '--codex-mark',
  '--accent-green',
  '--accent-green-soft',
  '--accent-yellow',
  '--accent-red',
  '--accent-orange',
  '--accent-notable',
  '--header-bar-top',
  '--header-bar-bottom',
  '--titlebar-strip-bg',
  '--header-fg',
  '--header-fg-muted',
  '--header-divider',
  '--header-badge-bg',
  '--header-badge-fg',
  '--focus-header-tint',
  '--focus-header-underline',
  '--focus-pill-bg',
  '--nav-focus-glow',
  '--syntax-key',
  '--syntax-string',
  '--syntax-number',
  '--syntax-keyword',
  '--syntax-comment',
  '--syntax-control',
  '--syntax-type',
  '--syntax-function',
];

/** The colour tokens app frames define and register. */
export const FRAME_COLOUR_TOKENS: readonly string[] = [
  '--bg-primary',
  '--bg-secondary',
  '--bg-tertiary',
  '--bg-quaternary',
  '--bg-hover',
  '--bg-selected',
  '--border-color',
  '--surface-bg',
  '--text-primary',
  '--text-secondary',
  '--text-muted',
  '--text-on-accent',
  '--text-strong',
  '--accent',
  '--accent-light',
  '--accent-action',
  '--brand-blue',
  '--brand-blue-deep',
  '--accent-green',
  '--accent-yellow',
  '--accent-red',
];

export const NAMED_COLOURS: readonly string[] = [
  'aliceblue', 'antiquewhite', 'aqua', 'aquamarine', 'azure', 'beige',
  'bisque', 'black', 'blanchedalmond', 'blue', 'blueviolet', 'brown',
  'burlywood', 'cadetblue', 'chartreuse', 'chocolate', 'coral', 'cornflowerblue',
  'cornsilk', 'crimson', 'cyan', 'darkblue', 'darkcyan', 'darkgoldenrod',
  'darkgray', 'darkgreen', 'darkgrey', 'darkkhaki', 'darkmagenta', 'darkolivegreen',
  'darkorange', 'darkorchid', 'darkred', 'darksalmon', 'darkseagreen', 'darkslateblue',
  'darkslategray', 'darkslategrey', 'darkturquoise', 'darkviolet', 'deeppink', 'deepskyblue',
  'dimgray', 'dimgrey', 'dodgerblue', 'firebrick', 'floralwhite', 'forestgreen',
  'fuchsia', 'gainsboro', 'ghostwhite', 'gold', 'goldenrod', 'gray',
  'green', 'greenyellow', 'grey', 'honeydew', 'hotpink', 'indianred',
  'indigo', 'ivory', 'khaki', 'lavender', 'lavenderblush', 'lawngreen',
  'lemonchiffon', 'lightblue', 'lightcoral', 'lightcyan', 'lightgoldenrodyellow', 'lightgray',
  'lightgreen', 'lightgrey', 'lightpink', 'lightsalmon', 'lightseagreen', 'lightskyblue',
  'lightslategray', 'lightslategrey', 'lightsteelblue', 'lightyellow', 'lime', 'limegreen',
  'linen', 'magenta', 'maroon', 'mediumaquamarine', 'mediumblue', 'mediumorchid',
  'mediumpurple', 'mediumseagreen', 'mediumslateblue', 'mediumspringgreen', 'mediumturquoise', 'mediumvioletred',
  'midnightblue', 'mintcream', 'mistyrose', 'moccasin', 'navajowhite', 'navy',
  'oldlace', 'olive', 'olivedrab', 'orange', 'orangered', 'orchid',
  'palegoldenrod', 'palegreen', 'paleturquoise', 'palevioletred', 'papayawhip', 'peachpuff',
  'peru', 'pink', 'plum', 'powderblue', 'purple', 'rebeccapurple',
  'red', 'rosybrown', 'royalblue', 'saddlebrown', 'salmon', 'sandybrown',
  'seagreen', 'seashell', 'sienna', 'silver', 'skyblue', 'slateblue',
  'slategray', 'slategrey', 'snow', 'springgreen', 'steelblue', 'tan',
  'teal', 'thistle', 'tomato', 'turquoise', 'violet', 'wheat',
  'white', 'whitesmoke', 'yellow', 'yellowgreen',
];
