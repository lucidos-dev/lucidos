/* GENERATED from packages/lucidos-sdk/src/boot/ by appearanceBoot.build.mjs.
   Do not edit: run `npm run build` in packages/lucidos-sdk. */
"use strict";
(() => {
  // src/generated/preference-catalog.ts
  var PREF_THEME_MODE = {
    key: "theme-mode",
    scope: "device",
    type: "enum",
    values: ["light", "dark", "system"],
    fallback: "system"
  };
  var PREF_FONT_FAMILY = {
    key: "font-family",
    scope: "device",
    type: "text",
    fallback: "theme"
  };
  var PREF_UI_SCALE = {
    key: "ui-scale",
    scope: "device",
    type: "number",
    min: 75,
    max: 200,
    fallback: "100"
  };
  var PREF_MOTION = {
    key: "motion",
    scope: "device",
    type: "enum",
    values: ["system", "reduce", "full"],
    fallback: "system"
  };
  var PREF_THEME_EFFECTS = {
    key: "theme-effects",
    scope: "device",
    type: "enum",
    values: ["system", "reduce", "full"],
    fallback: "system"
  };
  var PREF_THEME = {
    key: "theme",
    scope: "device",
    type: "text",
    fallback: "lucidos"
  };

  // src/generated/font-catalog.ts
  var FOLLOW_THEME = "theme";
  var FALLBACK_FONT = "fira-code";
  var WORKSPACE_FONT_ID_PREFIX = "ws-";
  var WORKSPACE_FONT_FALLBACKS = {
    sans: "system-ui, -apple-system, 'Segoe UI', sans-serif",
    serif: "Georgia, 'Times New Roman', serif",
    mono: "ui-monospace, SFMono-Regular, Menlo, Consolas, monospace"
  };
  var WORKSPACE_FONT_LIMITS = { slug: 40, label: 60, faces: 16, fonts: 100 };
  var FONT_CATALOG = [
    {
      id: "system",
      label: "System",
      stack: "system-ui, -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif",
      kind: "ui",
      group: "sans",
      source: "device",
      ligatures: false,
      bold: true
    },
    {
      id: "geist",
      label: "Geist",
      stack: "'Geist', system-ui, -apple-system, 'Segoe UI', sans-serif",
      kind: "ui",
      group: "sans",
      source: "vendored",
      ligatures: false,
      bold: true
    },
    {
      id: "atkinson-hyperlegible-next",
      label: "Atkinson Hyperlegible Next",
      stack: "'Atkinson Hyperlegible Next', system-ui, -apple-system, 'Segoe UI', sans-serif",
      kind: "ui",
      group: "sans",
      source: "vendored",
      ligatures: false,
      bold: true
    },
    {
      id: "inter",
      label: "Inter",
      stack: "'Inter', system-ui, -apple-system, 'Segoe UI', sans-serif",
      kind: "ui",
      group: "sans",
      source: "vendored",
      ligatures: false,
      bold: true
    },
    {
      id: "roboto",
      label: "Roboto",
      stack: "'Roboto', system-ui, -apple-system, 'Segoe UI', sans-serif",
      kind: "ui",
      group: "sans",
      source: "vendored",
      ligatures: false,
      bold: true
    },
    {
      id: "open-sans",
      label: "Open Sans",
      stack: "'Open Sans', system-ui, -apple-system, 'Segoe UI', sans-serif",
      kind: "ui",
      group: "sans",
      source: "vendored",
      ligatures: false,
      bold: true
    },
    {
      id: "manrope",
      label: "Manrope",
      stack: "'Manrope', system-ui, -apple-system, 'Segoe UI', sans-serif",
      kind: "ui",
      group: "sans",
      source: "vendored",
      ligatures: false,
      bold: true
    },
    {
      id: "source-serif-4",
      label: "Source Serif 4",
      stack: "'Source Serif 4', Georgia, 'Times New Roman', serif",
      kind: "ui",
      group: "serif",
      source: "vendored",
      ligatures: false,
      bold: true
    },
    {
      id: "lora",
      label: "Lora",
      stack: "'Lora', Georgia, 'Times New Roman', serif",
      kind: "ui",
      group: "serif",
      source: "vendored",
      ligatures: false,
      bold: true
    },
    {
      id: "literata",
      label: "Literata",
      stack: "'Literata', Georgia, 'Times New Roman', serif",
      kind: "ui",
      group: "serif",
      source: "vendored",
      ligatures: false,
      bold: true
    },
    {
      id: "fira-code",
      label: "Fira Code",
      stack: "'Fira Code', ui-monospace, SFMono-Regular, 'SF Mono', Menlo, 'JetBrains Mono', Monaco, Consolas, monospace",
      kind: "both",
      group: "mono",
      source: "vendored",
      ligatures: true,
      bold: true
    },
    {
      id: "monospace",
      label: "Monospace",
      stack: "ui-monospace, SFMono-Regular, 'SF Mono', Menlo, 'Fira Code', 'JetBrains Mono', Monaco, Consolas, monospace",
      kind: "both",
      group: "mono",
      source: "device",
      ligatures: false,
      bold: true
    },
    {
      id: "geist-mono",
      label: "Geist Mono",
      stack: "'Geist Mono', ui-monospace, SFMono-Regular, Menlo, Consolas, monospace",
      kind: "both",
      group: "mono",
      source: "vendored",
      ligatures: false,
      bold: true
    },
    {
      id: "atkinson-hyperlegible-mono",
      label: "Atkinson Hyperlegible Mono",
      stack: "'Atkinson Hyperlegible Mono', ui-monospace, SFMono-Regular, Menlo, Consolas, monospace",
      kind: "both",
      group: "mono",
      source: "vendored",
      ligatures: false,
      bold: true
    },
    {
      id: "jetbrains-mono",
      label: "JetBrains Mono",
      stack: "'JetBrains Mono', ui-monospace, SFMono-Regular, Menlo, Consolas, monospace",
      kind: "both",
      group: "mono",
      source: "vendored",
      ligatures: true,
      bold: true
    },
    {
      id: "ibm-plex-mono",
      label: "IBM Plex Mono",
      stack: "'IBM Plex Mono', ui-monospace, SFMono-Regular, Menlo, Consolas, monospace",
      kind: "both",
      group: "mono",
      source: "vendored",
      ligatures: false,
      bold: true
    },
    {
      id: "source-code-pro",
      label: "Source Code Pro",
      stack: "'Source Code Pro', ui-monospace, SFMono-Regular, Menlo, Consolas, monospace",
      kind: "both",
      group: "mono",
      source: "vendored",
      ligatures: false,
      bold: true
    },
    {
      id: "commit-mono",
      label: "Commit Mono",
      stack: "'Commit Mono', ui-monospace, SFMono-Regular, Menlo, Consolas, monospace",
      kind: "both",
      group: "mono",
      source: "vendored",
      ligatures: false,
      bold: true
    },
    {
      id: "cascadia-code",
      label: "Cascadia Code",
      stack: "'Cascadia Code', ui-monospace, SFMono-Regular, Menlo, Consolas, monospace",
      kind: "both",
      group: "mono",
      source: "vendored",
      ligatures: true,
      bold: true
    },
    {
      id: "vt323",
      label: "VT323",
      stack: "'VT323', ui-monospace, SFMono-Regular, Menlo, Consolas, monospace",
      kind: "both",
      group: "mono",
      source: "vendored",
      ligatures: false,
      bold: false
    }
  ];

  // src/generated/theme-parts.ts
  var PART_PROPERTIES = {
    "background-color": { grammar: "colour" },
    "background-image": { grammar: "scanlines", maxAlpha: 0.25, maxPeriod: 8, maxStops: 4 },
    "border-color": { grammar: "colour" },
    "border-style": { grammar: "keyword", values: ["solid", "double"] },
    "border-width": { grammar: "length", unit: "px", min: 1, max: 4 },
    "box-shadow": { grammar: "shadow", unit: "px", maxLayers: 2, maxOffset: 4, maxBlur: 16, maxSpread: 2 },
    "caret-color": { grammar: "colour", minAlpha: 0.6 },
    "caret-shape": { grammar: "keyword", values: ["auto", "bar", "block", "underscore"] },
    "color": { grammar: "colour", minAlpha: 0.6 },
    "filter": { grammar: "drop-shadow", unit: "px", maxOffset: 2, maxBlur: 6 },
    "letter-spacing": { grammar: "spacing", unit: "em", min: -0.02, max: 0.12 },
    "text-decoration-color": { grammar: "colour" },
    "text-shadow": { grammar: "shadow", unit: "em", maxLayers: 2, maxOffset: 0.1, maxBlur: 0.6 }
  };
  var PART_TOKENS = {
    "--part-chat-text-color": { part: "chat-text", property: "color", insetOnly: false, frames: false },
    "--part-chat-text-text-shadow": { part: "chat-text", property: "text-shadow", insetOnly: false, frames: false },
    "--part-chat-text-letter-spacing": { part: "chat-text", property: "letter-spacing", insetOnly: false, frames: false },
    "--part-chat-heading-color": { part: "chat-heading", property: "color", insetOnly: false, frames: false },
    "--part-chat-heading-text-shadow": { part: "chat-heading", property: "text-shadow", insetOnly: false, frames: false },
    "--part-chat-heading-letter-spacing": { part: "chat-heading", property: "letter-spacing", insetOnly: false, frames: false },
    "--part-chat-link-color": { part: "chat-link", property: "color", insetOnly: false, frames: false },
    "--part-chat-link-text-shadow": { part: "chat-link", property: "text-shadow", insetOnly: false, frames: false },
    "--part-inline-code-color": { part: "inline-code", property: "color", insetOnly: false, frames: false },
    "--part-inline-code-background-color": { part: "inline-code", property: "background-color", insetOnly: false, frames: false },
    "--part-code-block-background-color": { part: "code-block", property: "background-color", insetOnly: false, frames: false },
    "--part-code-block-box-shadow": { part: "code-block", property: "box-shadow", insetOnly: false, frames: false },
    "--part-code-block-text-shadow": { part: "code-block", property: "text-shadow", insetOnly: false, frames: false },
    "--part-actor-label-text-shadow": { part: "actor-label", property: "text-shadow", insetOnly: false, frames: false },
    "--part-actor-label-letter-spacing": { part: "actor-label", property: "letter-spacing", insetOnly: false, frames: false },
    "--part-actor-icon-filter": { part: "actor-icon", property: "filter", insetOnly: false, frames: false },
    "--part-header-icon-filter": { part: "header-icon", property: "filter", insetOnly: false, frames: false },
    "--part-header-title-text-shadow": { part: "header-title", property: "text-shadow", insetOnly: false, frames: false },
    "--part-header-title-letter-spacing": { part: "header-title", property: "letter-spacing", insetOnly: false, frames: false },
    "--part-composer-background-color": { part: "composer", property: "background-color", insetOnly: false, frames: false },
    "--part-composer-border-color": { part: "composer", property: "border-color", insetOnly: false, frames: false },
    "--part-composer-box-shadow": { part: "composer", property: "box-shadow", insetOnly: true, frames: false },
    "--part-composer-text-color": { part: "composer-text", property: "color", insetOnly: false, frames: false },
    "--part-composer-text-caret-color": { part: "composer-text", property: "caret-color", insetOnly: false, frames: false },
    "--part-composer-text-caret-shape": { part: "composer-text", property: "caret-shape", insetOnly: false, frames: false },
    "--part-composer-text-text-shadow": { part: "composer-text", property: "text-shadow", insetOnly: false, frames: false },
    "--part-composer-text-letter-spacing": { part: "composer-text", property: "letter-spacing", insetOnly: false, frames: false },
    "--part-card-border-style": { part: "card", property: "border-style", insetOnly: false, frames: false },
    "--part-card-border-width": { part: "card", property: "border-width", insetOnly: false, frames: false },
    "--part-card-border-color": { part: "card", property: "border-color", insetOnly: false, frames: false },
    "--part-surface-border-style": { part: "surface", property: "border-style", insetOnly: false, frames: false },
    "--part-surface-border-width": { part: "surface", property: "border-width", insetOnly: false, frames: false },
    "--part-surface-border-color": { part: "surface", property: "border-color", insetOnly: false, frames: false },
    "--part-screen-background-image": { part: "screen", property: "background-image", insetOnly: false, frames: false },
    "--part-app-text-text-shadow": { part: "app-text", property: "text-shadow", insetOnly: false, frames: true },
    "--part-app-text-letter-spacing": { part: "app-text", property: "letter-spacing", insetOnly: false, frames: true },
    "--part-app-link-text-decoration-color": { part: "app-link", property: "text-decoration-color", insetOnly: false, frames: true },
    "--part-app-link-text-shadow": { part: "app-link", property: "text-shadow", insetOnly: false, frames: true },
    "--part-app-control-border-color": { part: "app-control", property: "border-color", insetOnly: false, frames: true },
    "--part-app-control-box-shadow": { part: "app-control", property: "box-shadow", insetOnly: false, frames: true }
  };
  var COLOUR_TOKENS = [
    "--bg-primary",
    "--bg-secondary",
    "--bg-tertiary",
    "--bg-quaternary",
    "--bg-hover",
    "--bg-selected",
    "--border-color",
    "--surface-bg",
    "--picked-surface",
    "--scrim",
    "--text-primary",
    "--text-secondary",
    "--text-muted",
    "--text-on-accent",
    "--text-strong",
    "--accent",
    "--accent-light",
    "--accent-action",
    "--brand-blue",
    "--brand-blue-deep",
    "--initiator-coding-agent",
    "--lucidos-mark-bg-top",
    "--lucidos-mark-bg-bottom",
    "--lucidos-mark-fg",
    "--claude-mark",
    "--codex-mark",
    "--accent-green",
    "--accent-green-soft",
    "--accent-yellow",
    "--accent-red",
    "--accent-orange",
    "--accent-notable",
    "--header-bar-top",
    "--header-bar-bottom",
    "--titlebar-strip-bg",
    "--header-fg",
    "--header-fg-muted",
    "--header-divider",
    "--header-badge-bg",
    "--header-badge-fg",
    "--focus-header-tint",
    "--focus-header-underline",
    "--focus-pill-bg",
    "--nav-focus-glow",
    "--syntax-key",
    "--syntax-string",
    "--syntax-number",
    "--syntax-keyword",
    "--syntax-comment",
    "--syntax-control",
    "--syntax-type",
    "--syntax-function"
  ];
  var FRAME_COLOUR_TOKENS = [
    "--bg-primary",
    "--bg-secondary",
    "--bg-tertiary",
    "--bg-quaternary",
    "--bg-hover",
    "--bg-selected",
    "--border-color",
    "--surface-bg",
    "--text-primary",
    "--text-secondary",
    "--text-muted",
    "--text-on-accent",
    "--text-strong",
    "--accent",
    "--accent-light",
    "--accent-action",
    "--brand-blue",
    "--brand-blue-deep",
    "--accent-green",
    "--accent-yellow",
    "--accent-red"
  ];
  var NAMED_COLOURS = [
    "aliceblue",
    "antiquewhite",
    "aqua",
    "aquamarine",
    "azure",
    "beige",
    "bisque",
    "black",
    "blanchedalmond",
    "blue",
    "blueviolet",
    "brown",
    "burlywood",
    "cadetblue",
    "chartreuse",
    "chocolate",
    "coral",
    "cornflowerblue",
    "cornsilk",
    "crimson",
    "cyan",
    "darkblue",
    "darkcyan",
    "darkgoldenrod",
    "darkgray",
    "darkgreen",
    "darkgrey",
    "darkkhaki",
    "darkmagenta",
    "darkolivegreen",
    "darkorange",
    "darkorchid",
    "darkred",
    "darksalmon",
    "darkseagreen",
    "darkslateblue",
    "darkslategray",
    "darkslategrey",
    "darkturquoise",
    "darkviolet",
    "deeppink",
    "deepskyblue",
    "dimgray",
    "dimgrey",
    "dodgerblue",
    "firebrick",
    "floralwhite",
    "forestgreen",
    "fuchsia",
    "gainsboro",
    "ghostwhite",
    "gold",
    "goldenrod",
    "gray",
    "green",
    "greenyellow",
    "grey",
    "honeydew",
    "hotpink",
    "indianred",
    "indigo",
    "ivory",
    "khaki",
    "lavender",
    "lavenderblush",
    "lawngreen",
    "lemonchiffon",
    "lightblue",
    "lightcoral",
    "lightcyan",
    "lightgoldenrodyellow",
    "lightgray",
    "lightgreen",
    "lightgrey",
    "lightpink",
    "lightsalmon",
    "lightseagreen",
    "lightskyblue",
    "lightslategray",
    "lightslategrey",
    "lightsteelblue",
    "lightyellow",
    "lime",
    "limegreen",
    "linen",
    "magenta",
    "maroon",
    "mediumaquamarine",
    "mediumblue",
    "mediumorchid",
    "mediumpurple",
    "mediumseagreen",
    "mediumslateblue",
    "mediumspringgreen",
    "mediumturquoise",
    "mediumvioletred",
    "midnightblue",
    "mintcream",
    "mistyrose",
    "moccasin",
    "navajowhite",
    "navy",
    "oldlace",
    "olive",
    "olivedrab",
    "orange",
    "orangered",
    "orchid",
    "palegoldenrod",
    "palegreen",
    "paleturquoise",
    "palevioletred",
    "papayawhip",
    "peachpuff",
    "peru",
    "pink",
    "plum",
    "powderblue",
    "purple",
    "rebeccapurple",
    "red",
    "rosybrown",
    "royalblue",
    "saddlebrown",
    "salmon",
    "sandybrown",
    "seagreen",
    "seashell",
    "sienna",
    "silver",
    "skyblue",
    "slateblue",
    "slategray",
    "slategrey",
    "snow",
    "springgreen",
    "steelblue",
    "tan",
    "teal",
    "thistle",
    "tomato",
    "turquoise",
    "violet",
    "wheat",
    "white",
    "whitesmoke",
    "yellow",
    "yellowgreen"
  ];
  var MAX_MIX_DEPTH = 2;
  var MAX_VALUE_LENGTH = 120;
  var MAX_RESOLVED_TOKENS = 200;
  var MAX_SHADOW_PX = 32;
  var PX_PER_REM = 16;
  var SHADOW_COLOUR_FUNCTIONS = ["rgb", "rgba", "hsl", "hsla", "hwb", "lab", "lch", "oklab", "oklch", "color", "color-mix"];

  // src/themeParts.ts
  var PART_TOKEN_PREFIX = "--part-";
  var VALUE_BANNED_RE = /[;{}<>@\\]|url\s*\(|image-set\s*\(|expression\s*\(|\/\*/i;
  var COLOUR_TOKEN_SET = new Set(COLOUR_TOKENS);
  var FRAME_COLOUR_TOKEN_SET = new Set(FRAME_COLOUR_TOKENS);
  var NAMED_COLOUR_SET = new Set(NAMED_COLOURS);
  var COLOUR_FUNCTIONS = ["rgb", "rgba", "hsl", "hsla", "oklab", "oklch", "color-mix", "var"];
  var Refusal = class extends Error {
  };
  function refuse(why) {
    throw new Refusal(why);
  }
  function checkPartToken(name, value) {
    const spec = Object.prototype.hasOwnProperty.call(PART_TOKENS, name) ? PART_TOKENS[name] : null;
    if (!spec) return { error: "not a part token. GET /api/v1/themes/parts lists them." };
    const trimmed = typeof value === "string" ? value.trim() : "";
    if (trimmed === "" || trimmed.length > MAX_VALUE_LENGTH || VALUE_BANNED_RE.test(trimmed)) {
      return {
        error: `the value is empty, longer than ${MAX_VALUE_LENGTH} characters, or uses a banned form (url(), ;, braces, @, backslash, a comment).`
      };
    }
    const slot = {
      property: spec.property,
      part: spec.part,
      grammar: PART_PROPERTIES[spec.property],
      insetOnly: spec.insetOnly,
      frames: spec.frames
    };
    try {
      const out = canonical(slot, trimmed.toLowerCase());
      if (out.length > MAX_VALUE_LENGTH) {
        return { error: `the value is longer than ${MAX_VALUE_LENGTH} characters once written out in full.` };
      }
      return { ok: out };
    } catch (e) {
      if (e instanceof Refusal) return { error: e.message };
      throw e;
    }
  }
  function canonical(slot, value) {
    const g = slot.grammar;
    switch (g.grammar) {
      case "colour": {
        const colour = parseColour(slot, value, false, 0);
        if (g.minAlpha !== void 0 && colour.alpha < g.minAlpha) {
          refuse(`alpha ${num(Math.round(colour.alpha * 100) / 100)} is under the ${num(g.minAlpha)} floor for text.`);
        }
        return colour.css;
      }
      case "shadow": {
        if (value === "none") return value;
        const layers = splitTopLevel(value, (c) => c === ",");
        if (layers.length > g.maxLayers) refuse(`${layers.length} layers; the limit is ${g.maxLayers}.`);
        const caps = { unit: g.unit, maxOffset: g.maxOffset, maxBlur: g.maxBlur };
        return layers.map((layer) => shadowLayer(slot, layer, caps, g.maxSpread)).join(", ");
      }
      case "drop-shadow": {
        if (value === "none") return value;
        const calls = splitTopLevel(value, isSpace);
        if (calls.length !== 1) refuse(`${slot.property} takes exactly one drop-shadow().`);
        const call = functionCall(calls[0]);
        if (!call || call[0] !== "drop-shadow") refuse(notAllowed(calls[0]));
        const parts = splitTopLevel(call[1], isSpace);
        if (parts.length !== 4) refuse("drop-shadow() takes <x> <y> <blur> <colour>.");
        const caps = { unit: g.unit, maxOffset: g.maxOffset, maxBlur: g.maxBlur };
        const geometry = shadowGeometry(slot, caps, parts[0], parts[1], parts[2]);
        return `drop-shadow(${geometry} ${parseColour(slot, parts[3], true, 0).css})`;
      }
      case "spacing":
        return value === "normal" ? value : boundedLength(slot, value, g.unit, g.min, g.max);
      case "length":
        return boundedLength(slot, value, g.unit, g.min, g.max);
      case "keyword":
        if (!g.values.includes(value)) refuse(`${slot.property} takes ${g.values.join(", ")}.`);
        return value;
      case "scanlines":
        return value === "none" ? value : scanlines(slot, value, g.maxAlpha, g.maxPeriod, g.maxStops);
    }
  }
  function boundedLength(slot, value, unit, min, max) {
    const v = length(slot, value, unit);
    const shown = withUnit(v, unit);
    if (v > max) refuse(`${shown} is over the ${withUnit(max, unit)} cap.`);
    if (v < min) refuse(`${shown} is under the ${withUnit(min, unit)} floor.`);
    return shown;
  }
  function isAngle(token) {
    return ["deg", "grad", "rad", "turn"].some((unit) => token.endsWith(unit) && plainNumber(token.slice(0, -unit.length)) !== null);
  }
  function scanlines(slot, value, maxAlpha, maxPeriod, maxStops) {
    const call = functionCall(value);
    if (!call) refuse(`${slot.property} takes none or repeating-linear-gradient().`);
    if (call[0] !== "repeating-linear-gradient") refuse(notAllowed(value));
    const stops = splitTopLevel(call[1], (c) => c === ",");
    if (stops.length < 2) refuse("a gradient takes at least 2 stops.");
    if (stops.length > maxStops) refuse(`${stops.length} stops; the limit is ${maxStops}.`);
    let previous = 0;
    let period = 0;
    const out = stops.map((stop, i) => {
      const tokens = splitTopLevel(stop, isSpace);
      if (i === 0 && (tokens[0] === "to" || isAngle(tokens[0]))) {
        refuse("scanlines take no direction: they run top to bottom.");
      }
      if (tokens.length > 2) refuse("a stop is a colour and an optional px position.");
      const colour = scanlineColour(tokens[0]);
      if (colour.alpha > maxAlpha) {
        refuse(`stop alpha ${num(Math.round(colour.alpha * 100) / 100)} is over the ${num(maxAlpha)} cap.`);
      }
      if (tokens.length === 1) return colour.css;
      const v = length(slot, tokens[1], "px");
      const shown = withUnit(v, "px");
      if (v < 0) refuse(`position ${shown} is under 0.`);
      if (v > maxPeriod) refuse(`position ${shown} is over the ${withUnit(maxPeriod, "px")} period cap.`);
      if (v < previous) refuse("stop positions must not go down.");
      previous = v;
      if (i === stops.length - 1) period = v;
      return `${colour.css} ${shown}`;
    });
    if (period <= 0) refuse("the last stop needs a px position above 0: it sets the period.");
    return `repeating-linear-gradient(${out.join(", ")})`;
  }
  function scanlineColour(value) {
    if (value === "transparent") return { css: value, alpha: 0 };
    if (value.startsWith("#")) return parseHex(value);
    const call = functionCall(value);
    if (call && ["rgb", "rgba", "hsl", "hsla"].includes(call[0])) return channels(call[0], call[1]);
    refuse("a scanline stop takes a hex, rgb(), rgba(), hsl() or hsla() colour, or transparent.");
  }
  function shadowGeometry(slot, caps, x, y, blur) {
    const offset = (axis, token) => {
      const v = length(slot, token, caps.unit);
      if (Math.abs(v) > caps.maxOffset) {
        refuse(`${axis} ${withUnit(v, caps.unit)} is over the \xB1${withUnit(caps.maxOffset, caps.unit)} cap.`);
      }
      return withUnit(v, caps.unit);
    };
    const xs = offset("x", x);
    const ys = offset("y", y);
    const b = length(slot, blur, caps.unit);
    if (b < 0) refuse(`blur ${withUnit(b, caps.unit)} is under 0.`);
    if (b > caps.maxBlur) refuse(`blur ${withUnit(b, caps.unit)} is over the ${withUnit(caps.maxBlur, caps.unit)} cap.`);
    return `${xs} ${ys} ${withUnit(b, caps.unit)}`;
  }
  function shadowLayer(slot, layer, caps, maxSpread) {
    const tokens = splitTopLevel(layer, isSpace);
    const inset = tokens[0] === "inset";
    if (inset) {
      if (maxSpread === void 0) refuse(notAllowed("inset"));
      tokens.shift();
    } else if (slot.insetOnly) {
      refuse(`the ${slot.part} takes inset shadows only.`);
    }
    const shape = maxSpread !== void 0 ? "[inset] <x> <y> <blur> [<spread>] <colour>" : "<x> <y> <blur> <colour>";
    let geometry;
    let spread = null;
    let colour;
    if (tokens.length === 4) {
      geometry = shadowGeometry(slot, caps, tokens[0], tokens[1], tokens[2]);
      colour = tokens[3];
    } else if (tokens.length === 5 && maxSpread !== void 0) {
      const s = length(slot, tokens[3], caps.unit);
      if (Math.abs(s) > maxSpread) {
        refuse(`spread ${withUnit(s, caps.unit)} is over the \xB1${withUnit(maxSpread, caps.unit)} cap.`);
      }
      geometry = shadowGeometry(slot, caps, tokens[0], tokens[1], tokens[2]);
      spread = withUnit(s, caps.unit);
      colour = tokens[4];
    } else {
      const bad = tokens.find((t) => t.includes("(") && !isColourCall(t));
      if (bad !== void 0) refuse(notAllowed(bad));
      refuse(`${slot.property} takes ${shape}.`);
    }
    const out = [];
    if (inset) out.push("inset");
    out.push(geometry);
    if (spread !== null) out.push(spread);
    out.push(parseColour(slot, colour, true, 0).css);
    return out.join(" ");
  }
  function isColourCall(token) {
    const call = functionCall(token);
    return !!call && COLOUR_FUNCTIONS.includes(call[0]);
  }
  function isSpace(c) {
    return /\s/.test(c);
  }
  function length(slot, token, unit) {
    if (token.includes("(")) refuse(notAllowed(token));
    const match = /[a-z%]/.exec(token);
    const split = match ? match.index : token.length;
    const digits = token.slice(0, split);
    const suffix = token.slice(split);
    const v = plainNumber(digits);
    if (v === null || !/^[a-z%]*$/.test(suffix)) refuse(`'${token}' is not a length.`);
    if (suffix === "" && v === 0) return 0;
    if (suffix === unit) return v;
    refuse(`use ${unit} for ${slot.property} lengths.`);
  }
  function plainNumber(token) {
    return /^[+-]?(?:\d{1,6}(?:\.\d{1,6})?|\.\d{1,6})$/.test(token) ? Number(token) : null;
  }
  function num(v) {
    return v === 0 ? "0" : String(v);
  }
  function withUnit(v, unit) {
    return v === 0 ? "0" : `${num(v)}${unit}`;
  }
  function notAllowed(token) {
    const call = functionCall(token);
    if (call) return `${call[0]}() is not allowed here.`;
    const open = token.indexOf("(");
    return open >= 0 ? `${token.slice(0, open)}() is not allowed here.` : `'${token}' is not allowed here.`;
  }
  function functionCall(token) {
    const open = token.indexOf("(");
    if (open < 0 || !token.endsWith(")")) return null;
    const name = token.slice(0, open);
    const inner = token.slice(open + 1, -1);
    let depth = 0;
    for (const c of inner) {
      if (c === "(") depth++;
      else if (c === ")") depth--;
      if (depth < 0) return null;
    }
    return depth === 0 && /^[a-z-]+$/.test(name) ? [name, inner] : null;
  }
  function splitTopLevel(value, sep) {
    const parts = [];
    let depth = 0;
    let start = 0;
    for (let i = 0; i < value.length; i++) {
      const c = value[i];
      if (c === "(") depth++;
      else if (c === ")") depth--;
      else if (depth === 0 && sep(c)) {
        parts.push(value.slice(start, i));
        start = i + 1;
      }
    }
    parts.push(value.slice(start));
    return parts.map((p) => p.trim()).filter((p) => p !== "");
  }
  function parseColour(slot, raw, shadow, mixDepth) {
    const value = raw.trim();
    if (value.startsWith("#")) return parseHex(value);
    const call = functionCall(value);
    if (call) {
      const [name, args] = call;
      switch (name) {
        case "rgb":
        case "rgba":
        case "hsl":
        case "hsla":
        case "oklab":
        case "oklch":
          return channels(name, args);
        case "color-mix":
          return colorMix(slot, args, mixDepth);
        case "var": {
          if (args.includes(",")) refuse("var() takes no fallback here.");
          const token = args.trim();
          if (slot.frames && !FRAME_COLOUR_TOKEN_SET.has(token)) {
            refuse(`var(${token}) is not a colour token app frames define.`);
          }
          if (!COLOUR_TOKEN_SET.has(token)) refuse(`var(${token}) is not a catalog colour token.`);
          return { css: `var(${token})`, alpha: 1 };
        }
        default:
          refuse(`${name}() is not allowed here.`);
      }
    }
    if (value.includes("(")) refuse(notAllowed(value));
    if (value === "currentcolor") return { css: value, alpha: 1 };
    if (value === "transparent") {
      if (shadow || mixDepth > 0) return { css: value, alpha: 0 };
      refuse("transparent is not allowed here.");
    }
    if (NAMED_COLOUR_SET.has(value)) return { css: value, alpha: 1 };
    refuse(`'${value}' is not a colour.`);
  }
  function parseHex(value) {
    const hex = value.slice(1);
    if (![3, 4, 6, 8].includes(hex.length) || !/^[0-9a-f]+$/.test(hex)) refuse(`'${value}' is not a hex colour.`);
    const alphaByte = hex.length === 4 ? parseInt(hex[3], 16) * 17 : hex.length === 8 ? parseInt(hex.slice(6), 16) : 255;
    return { css: value, alpha: alphaByte / 255 };
  }
  function component(token) {
    if (token.endsWith("%")) {
      const v2 = plainNumber(token.slice(0, -1));
      return v2 === null ? null : [v2, true];
    }
    const v = plainNumber(token);
    return v === null ? null : [v, false];
  }
  function fmtComponent([v, pct]) {
    return pct ? `${num(v)}%` : num(v);
  }
  function channels(name, args) {
    const bad = `${name}() takes 3 numbers or percentages and an optional alpha.`;
    if (args.includes("(")) refuse(bad);
    let values;
    let css;
    if (args.includes(",")) {
      const parsed = args.split(",").map((t) => component(t.trim()));
      if (parsed.some((v) => v === null) || parsed.length !== 3 && parsed.length !== 4) refuse(bad);
      values = parsed;
      css = `${name}(${values.map(fmtComponent).join(", ")})`;
    } else {
      const slash = args.indexOf("/");
      const colour = slash >= 0 ? args.slice(0, slash) : args;
      const alpha2 = slash >= 0 ? args.slice(slash + 1).trim() : null;
      const parsed = colour.split(/\s+/).filter((t) => t !== "").map(component);
      if (parsed.some((v) => v === null) || parsed.length !== 3) refuse(bad);
      values = parsed;
      let text = values.map(fmtComponent).join(" ");
      if (alpha2 !== null) {
        const a2 = component(alpha2);
        if (!a2) refuse(bad);
        text += ` / ${fmtComponent(a2)}`;
        values.push(a2);
      }
      css = `${name}(${text})`;
    }
    const a = values[3];
    const alpha = a ? a[1] ? a[0] / 100 : a[0] : 1;
    return { css, alpha: Math.min(1, Math.max(0, alpha)) };
  }
  function colorMix(slot, args, depth) {
    if (depth >= MAX_MIX_DEPTH) refuse(`color-mix() nests at most ${MAX_MIX_DEPTH} deep.`);
    const parts = splitTopLevel(args, (c) => c === ",");
    if (parts.length !== 3) refuse("color-mix() takes a colour space and two colours.");
    const space = parts[0].split(/\s+/).filter((t) => t !== "");
    if (space.length !== 2 || space[0] !== "in" || !["srgb", "oklab", "oklch"].includes(space[1])) {
      refuse("color-mix() mixes in srgb, oklab or oklch.");
    }
    const [c1, p1] = mixInput(slot, parts[1], depth);
    const [c2, p2] = mixInput(slot, parts[2], depth);
    const w = (p) => p === null ? null : p / 100;
    let w1;
    let w2;
    const [a, b] = [w(p1), w(p2)];
    if (a === null && b === null) [w1, w2] = [0.5, 0.5];
    else if (b === null) [w1, w2] = [a, 1 - a];
    else if (a === null) [w1, w2] = [1 - b, b];
    else [w1, w2] = [a, b];
    const total = w1 + w2;
    const alpha = total <= 0 ? 0 : (c1.alpha * w1 + c2.alpha * w2) / total * Math.min(total, 1);
    const input = (c, p) => p === null ? c.css : `${c.css} ${num(p)}%`;
    return { css: `color-mix(in ${space[1]}, ${input(c1, p1)}, ${input(c2, p2)})`, alpha };
  }
  function mixInput(slot, arg, depth) {
    const tokens = splitTopLevel(arg, isSpace);
    const pctOf = (t) => {
      if (t === void 0 || !t.endsWith("%")) return null;
      const v = plainNumber(t.slice(0, -1));
      return v !== null && v >= 0 && v <= 100 ? v : null;
    };
    let pct = pctOf(tokens[tokens.length - 1]);
    if (pct !== null) tokens.pop();
    else {
      pct = pctOf(tokens[0]);
      if (pct !== null) tokens.shift();
    }
    if (tokens.length !== 1) refuse("a color-mix() input is one colour and an optional percentage.");
    return [parseColour(slot, tokens[0], false, depth + 1), pct];
  }

  // src/appearance.ts
  var THEME_MODES = PREF_THEME_MODE.values;
  var THEME_MODE_KEY = PREF_THEME_MODE.key;
  var THEME_MODE_STORAGE_KEY = "lucidos-theme-mode";
  var THEME_MODE_ATTRIBUTE = "data-theme-mode";
  var DEFAULT_THEME_MODE = PREF_THEME_MODE.fallback;
  var THEME_MODE_BG = {
    light: "#ffffff",
    dark: "#07172e"
  };
  var FONT_PREFERENCES = [
    FOLLOW_THEME,
    ...FONT_CATALOG.map((font) => font.id)
  ];
  var FONTS_BY_ID = {};
  for (const font of FONT_CATALOG) FONTS_BY_ID[font.id] = font;
  var FONT_STACKS = {};
  for (const font of FONT_CATALOG) FONT_STACKS[font.id] = font.stack;
  function isFontId(value) {
    return !!value && hasOwn(FONTS_BY_ID, value);
  }
  var FONT_BOLD_ATTRIBUTE = "data-font-bold";
  var BOLD_WEIGHT = 600;
  function heaviestWeight(weight) {
    return Number(weight.trim().split(/\s+/).pop());
  }
  function weightReachesBold(weight) {
    return heaviestWeight(weight) >= BOLD_WEIGHT;
  }
  function workspaceFontHasBold(font, style = "normal") {
    return font.faces.some((face) => face.style === style && weightReachesBold(face.weight));
  }
  function registeredFaceWeight(font, face) {
    if (workspaceFontHasBold(font, face.style)) return face.weight;
    const top = font.faces.filter((other) => other.style === face.style).reduce((a, b) => heaviestWeight(b.weight) > heaviestWeight(a.weight) ? b : a);
    if (top !== face) return face.weight;
    return `${face.weight.trim().split(/\s+/)[0]} 900`;
  }
  function fontBoldMark(font) {
    if (font.workspaceFont) return workspaceFontHasBold(font.workspaceFont) ? "face" : "none";
    return isFontId(font.key) && !FONTS_BY_ID[font.key].bold ? "none" : "face";
  }
  var FONT_FEATURES_DEFAULT = { text: "normal", code: "normal" };
  var FONT_FEATURES_LIGATURES = {
    text: '"liga" 0, "calt" 0',
    code: '"liga" 1, "calt" 1'
  };
  var UI_SCALE_MIN = PREF_UI_SCALE.min;
  var UI_SCALE_MAX = PREF_UI_SCALE.max;
  var UI_SCALE_STEP = 12.5;
  var LEGACY_UI_SCALES = {
    small: 100,
    medium: 112.5,
    large: 125
  };
  function resolveThemeMode(mode, prefersLight) {
    if (mode === "system") return prefersLight ? "light" : "dark";
    return mode;
  }
  var MOTION_PREFS = PREF_MOTION.values;
  var DEFAULT_MOTION = PREF_MOTION.fallback;
  var MOTION_STORAGE_KEY = "lucidos-motion";
  var REDUCED_MOTION_QUERY = "(prefers-reduced-motion: reduce)";
  function parseMotion(raw) {
    return raw && MOTION_PREFS.includes(raw) ? raw : DEFAULT_MOTION;
  }
  function resolveReducedMotion(pref, osReduces) {
    if (pref === "system") return osReduces;
    return pref === "reduce";
  }
  function motionAttribute(reduced) {
    return reduced ? "reduce" : "full";
  }
  var THEME_EFFECTS_PREFS = PREF_THEME_EFFECTS.values;
  var DEFAULT_THEME_EFFECTS = PREF_THEME_EFFECTS.fallback;
  var THEME_EFFECTS_STORAGE_KEY = "lucidos-theme-effects";
  var REDUCED_TRANSPARENCY_QUERY = "(prefers-reduced-transparency: reduce)";
  var MORE_CONTRAST_QUERY = "(prefers-contrast: more)";
  function parseThemeEffects(raw) {
    return raw && THEME_EFFECTS_PREFS.includes(raw) ? raw : DEFAULT_THEME_EFFECTS;
  }
  function resolveReducedThemeEffects(pref, osReducesTransparency, osPrefersMoreContrast) {
    if (pref === "system") return osReducesTransparency || osPrefersMoreContrast;
    return pref === "reduce";
  }
  function themeEffectsAttribute(reduced) {
    return reduced ? "reduce" : "full";
  }
  var ANIMATION_SPEED_STORAGE_KEY = "lucidos-animation-speed-slider";
  var ANIMATION_SPEED_MIN = -10;
  var ANIMATION_SPEED_MAX = 10;
  function parseAnimationSpeed(raw) {
    const n = parseInt(raw != null ? raw : "", 10);
    if (isNaN(n)) return 0;
    return Math.max(ANIMATION_SPEED_MIN, Math.min(ANIMATION_SPEED_MAX, n));
  }
  function speedMultiplierFor(position) {
    return Math.pow(10, position / 10);
  }
  var REDUCED_MOTION_DURATION_SCALE = 1e-3;
  function durationScaleFor(sliderPosition, reduced) {
    return reduced ? REDUCED_MOTION_DURATION_SCALE : 1 / speedMultiplierFor(sliderPosition);
  }
  function resolveFontKey(stored, themeFonts, known = []) {
    const usable = (value) => isFontId(value) || known.some((entry) => entry.id === value);
    if (stored !== FOLLOW_THEME && usable(stored)) return stored;
    return usable(themeFonts.ui) ? themeFonts.ui : FALLBACK_FONT;
  }
  function fontFeaturesFor(font, known = []) {
    const ligatures = isFontId(font) ? FONTS_BY_ID[font].ligatures : known.some((entry) => entry.id === font && entry.ligatures);
    return ligatures ? FONT_FEATURES_LIGATURES : FONT_FEATURES_DEFAULT;
  }
  function fontStackFor(font, known = []) {
    var _a, _b;
    if (isFontId(font)) return FONT_STACKS[font];
    return (_b = (_a = known.find((entry) => entry.id === font)) == null ? void 0 : _a.stack) != null ? _b : FONT_STACKS[FALLBACK_FONT];
  }
  function resolveFont(stored, themeFonts, known = []) {
    var _a;
    const key = resolveFontKey(stored, themeFonts, known);
    return {
      key,
      stack: fontStackFor(key, known),
      features: fontFeaturesFor(key, known),
      workspaceFont: (_a = known.find((entry) => entry.id === key)) != null ? _a : null
    };
  }
  function hasOwn(obj, key) {
    return Object.prototype.hasOwnProperty.call(obj, key);
  }
  function clampUiScale(scale) {
    const snapped = Math.round(scale / UI_SCALE_STEP) * UI_SCALE_STEP;
    return Math.max(UI_SCALE_MIN, Math.min(UI_SCALE_MAX, snapped));
  }
  function parseUiScale(raw) {
    if (!raw) return null;
    const n = hasOwn(LEGACY_UI_SCALES, raw) ? LEGACY_UI_SCALES[raw] : parseFloat(raw);
    if (isNaN(n)) return null;
    return clampUiScale(n);
  }
  var STYLE_OVERRIDES_STORAGE_KEY = "lucidos-style-overrides";
  var STYLE_RESET_PARAM = "style-reset";
  var MAX_STYLE_OVERRIDES = MAX_RESOLVED_TOKENS;
  var MAX_STYLE_VALUE_LENGTH = MAX_VALUE_LENGTH;
  var NAME_RE = /^--[a-z][a-z0-9-]*$/;
  var VALUE_BANNED_RE2 = /[;{}<>@\\]|url\s*\(|image-set\s*\(|expression\s*\(|\/\*/i;
  function isValidOverrideName(name) {
    return NAME_RE.test(name);
  }
  var RESERVED_OVERRIDE_NAME_RE = /^--(?:protected-|z-)|^--part-screen-background-image$|^--font(?:-ui|-family|-features-text|-features-code)?$|^--user-ui-scale$/;
  function isReservedOverrideName(name) {
    return RESERVED_OVERRIDE_NAME_RE.test(name);
  }
  var SHADOW_OVERRIDE_TOKENS = [
    "--text-glow",
    "--focus-pill-glow",
    "--focus-ring",
    "--shadow-sm",
    "--shadow-md",
    "--shadow-lg",
    "--shadow-up"
  ];
  var SHADOW_COLOUR_FUNCTION_SET = new Set(SHADOW_COLOUR_FUNCTIONS);
  function splitTopLevel2(value, isSep) {
    const parts = [];
    let depth = 0;
    let start = 0;
    for (let i = 0; i < value.length; i++) {
      const c = value[i];
      if (c === "(") depth++;
      else if (c === ")") depth--;
      else if (depth === 0 && isSep(c)) {
        parts.push(value.slice(start, i));
        start = i + 1;
      }
    }
    parts.push(value.slice(start));
    return parts.map((p) => p.trim()).filter((p) => p !== "");
  }
  function wholeCallName(word) {
    const m = /^([a-z-]+)\(([\s\S]*)\)$/.exec(word);
    if (!m) return null;
    let depth = 0;
    for (const c of m[2]) {
      if (c === "(") depth++;
      else if (c === ")" && --depth < 0) return null;
    }
    return depth === 0 ? m[1] : null;
  }
  function shadowLengthPx(word) {
    if (!/^[0-9.+-]/.test(word)) return word.includes("+") ? void 0 : null;
    const m = /^([+-]?(?:\d+\.?\d*|\.\d+))(.*)$/.exec(word);
    if (!m) return void 0;
    const length2 = Number(m[1]);
    if (!Number.isFinite(length2)) return void 0;
    const unit = m[2];
    if (unit === "" && length2 === 0) return 0;
    if (unit === "px") return length2;
    if (unit === "rem" || unit === "em") return length2 * PX_PER_REM;
    return void 0;
  }
  function shadowWithinReach(value) {
    for (const layer of splitTopLevel2(value, (c) => c === ",")) {
      const lengths = [];
      for (const raw of splitTopLevel2(layer, (c) => /\s/.test(c))) {
        const word = raw.toLowerCase();
        if (word.includes("(")) {
          const name = wholeCallName(word);
          if (name === null || !SHADOW_COLOUR_FUNCTION_SET.has(name)) return false;
          continue;
        }
        const px = shadowLengthPx(word);
        if (px === void 0) return false;
        if (px !== null) lengths.push(px);
      }
      if (lengths.length === 0) continue;
      if (lengths.length === 1 || lengths.length > 4) return false;
      const [x, y, blur = 0, spread = 0] = lengths;
      const reach = Math.max(Math.abs(x), Math.abs(y)) + Math.max(spread, 0) + Math.max(blur, 0) / 2;
      if (reach > MAX_SHADOW_PX) return false;
    }
    return true;
  }
  function isAllowedOverride(name, value) {
    if (!isValidOverrideName(name) || isReservedOverrideName(name)) return false;
    if (!isValidOverrideValue(value)) return false;
    return !SHADOW_OVERRIDE_TOKENS.includes(name) || shadowWithinReach(value.trim());
  }
  function isValidOverrideValue(value) {
    if (typeof value !== "string") return false;
    const trimmed = value.trim();
    if (trimmed === "") return false;
    if (trimmed.length > MAX_STYLE_VALUE_LENGTH) return false;
    return !VALUE_BANNED_RE2.test(trimmed);
  }
  function parseStyleOverrides(raw) {
    const map = sanitizeTokenMap(parseJson(raw));
    for (const [name, value] of Object.entries(map)) {
      if (!isAllowedOverride(name, value)) delete map[name];
    }
    return map;
  }
  function parseJson(raw) {
    if (!raw) return null;
    try {
      return JSON.parse(raw);
    } catch (e) {
      return null;
    }
  }
  function sanitizeTokenMap(value) {
    if (!value || typeof value !== "object" || Array.isArray(value)) return {};
    const out = {};
    let n = 0;
    for (const [name, token] of Object.entries(value)) {
      if (n >= MAX_STYLE_OVERRIDES) break;
      if (!isValidOverrideName(name)) continue;
      if (typeof token !== "string" || !isValidOverrideValue(token)) continue;
      if (name.startsWith(PART_TOKEN_PREFIX)) {
        const part = checkPartToken(name, token);
        if (!("ok" in part)) continue;
        out[name] = part.ok;
      } else {
        out[name] = token.trim();
      }
      n++;
    }
    return out;
  }
  function styleResetRequested(search) {
    return new RegExp(`[?&]${STYLE_RESET_PARAM}(?:[=&]|$)`).test(search);
  }
  var THEME_KEY = PREF_THEME.key;
  var THEME_STORAGE_KEY = "lucidos-theme-resolved";
  var THEME_SEED_KEY = "theme_resolved";
  var EMPTY_THEME = { dark: {}, light: {}, fonts: {}, workspace_fonts: [] };
  function parseResolvedTheme(raw) {
    return sanitizeResolvedTheme(parseJson(raw));
  }
  function sanitizeResolvedTheme(value) {
    if (!value || typeof value !== "object" || Array.isArray(value)) return EMPTY_THEME;
    const theme = value;
    return {
      dark: sanitizeTokenMap(theme.dark),
      light: sanitizeTokenMap(theme.light),
      fonts: sanitizeThemeFonts(theme.fonts),
      workspace_fonts: sanitizeWorkspaceFonts(theme.workspace_fonts)
    };
  }
  function sanitizeThemeFonts(value) {
    if (!value || typeof value !== "object" || Array.isArray(value)) return {};
    const fonts = value;
    const out = {};
    for (const slot of ["ui", "mono"]) {
      const id = fonts[slot];
      if (typeof id === "string" && (isFontId(id) || isWorkspaceFontId(id))) out[slot] = id;
    }
    return out;
  }
  function themeBackground(tokens) {
    const bg = tokens["--bg-primary"];
    return bg && /^#(?:[0-9a-f]{3}|[0-9a-f]{6})$/i.test(bg) ? bg : null;
  }
  var WORKSPACE_FONT_STORAGE_KEY = "lucidos-workspace-font";
  var WORKSPACE_FONT_SEED_KEY = "workspace_font";
  var WORKSPACE_FONT_SLUG = "[a-z0-9]+(?:-[a-z0-9]+)*";
  var WORKSPACE_FONT_ID = new RegExp(`^${WORKSPACE_FONT_ID_PREFIX}${WORKSPACE_FONT_SLUG}$`);
  var WORKSPACE_FONT_ID_MAX = WORKSPACE_FONT_ID_PREFIX.length + WORKSPACE_FONT_LIMITS.slug;
  var FACE_FILE = /^[A-Za-z0-9_-][A-Za-z0-9._-]{0,99}$/;
  var FACE_EXTENSION = /\.(?:woff2|woff|ttf|otf)$/i;
  var FACE_WEIGHT = /^(\d{1,4})(?: (\d{1,4}))?$/;
  var FONT_GROUPS = ["sans", "serif", "mono"];
  function isWorkspaceFontId(value) {
    return !!value && value.length <= WORKSPACE_FONT_ID_MAX && WORKSPACE_FONT_ID.test(value);
  }
  function workspaceFontStack(id, group) {
    return `'${id}', ${WORKSPACE_FONT_FALLBACKS[group]}`;
  }
  function isFaceWeight(weight) {
    const match = FACE_WEIGHT.exec(weight);
    if (!match) return false;
    const min = Number(match[1]);
    const max = match[2] === void 0 ? min : Number(match[2]);
    return min >= 1 && max <= 1e3 && min <= max;
  }
  function sanitizeFace(value, slug) {
    if (!value || typeof value !== "object") return null;
    const face = value;
    const { path, weight, style } = face;
    if (typeof path !== "string" || typeof weight !== "string") return null;
    const prefix = `fonts/${slug}/`;
    const file = path.startsWith(prefix) ? path.slice(prefix.length) : "";
    if (!FACE_FILE.test(file) || !FACE_EXTENSION.test(file)) return null;
    if (!isFaceWeight(weight)) return null;
    if (style !== "normal" && style !== "italic") return null;
    return { path, weight, style };
  }
  function sanitizeWorkspaceFont(value) {
    if (!value || typeof value !== "object" || Array.isArray(value)) return null;
    const font = value;
    const { id, group, faces } = font;
    if (typeof id !== "string" || !isWorkspaceFontId(id)) return null;
    if (typeof group !== "string" || !FONT_GROUPS.includes(group)) return null;
    if (!Array.isArray(faces) || faces.length === 0 || faces.length > WORKSPACE_FONT_LIMITS.faces) {
      return null;
    }
    const slug = id.slice(WORKSPACE_FONT_ID_PREFIX.length);
    const clean = [];
    for (const face of faces) {
      const checked = sanitizeFace(face, slug);
      if (!checked) return null;
      clean.push(checked);
    }
    const label = typeof font.label === "string" && font.label.trim() ? font.label.slice(0, WORKSPACE_FONT_LIMITS.label) : id;
    return {
      id,
      label,
      family: id,
      stack: workspaceFontStack(id, group),
      group,
      ligatures: font.ligatures === true,
      faces: clean
    };
  }
  function sanitizeWorkspaceFonts(value) {
    if (!Array.isArray(value)) return [];
    const out = [];
    for (const entry of value) {
      if (out.length >= WORKSPACE_FONT_LIMITS.fonts) break;
      const font = sanitizeWorkspaceFont(entry);
      if (font && !out.some((known) => known.id === font.id)) out.push(font);
    }
    return out;
  }
  function parseWorkspaceFont(raw) {
    return sanitizeWorkspaceFont(parseJson(raw));
  }
  var FONT_FAMILY_STORAGE_KEY = "lucidos-font-family";
  var UI_SCALE_STORAGE_KEY = "lucidos-ui-scale";
  var APPEARANCE_PUSH_SOURCES = [
    [THEME_MODE_KEY, THEME_MODE_STORAGE_KEY],
    [THEME_SEED_KEY, THEME_STORAGE_KEY],
    [PREF_FONT_FAMILY.key, FONT_FAMILY_STORAGE_KEY],
    [WORKSPACE_FONT_SEED_KEY, WORKSPACE_FONT_STORAGE_KEY],
    [PREF_UI_SCALE.key, UI_SCALE_STORAGE_KEY],
    ["style_overrides", STYLE_OVERRIDES_STORAGE_KEY],
    [PREF_MOTION.key, MOTION_STORAGE_KEY],
    [PREF_THEME_EFFECTS.key, THEME_EFFECTS_STORAGE_KEY]
  ];

  // src/_bridge.ts
  var BRIDGE_TYPE = "lucidos:bridge";
  var bridged = null;
  function isBridged() {
    if (bridged !== null) return bridged;
    bridged = typeof window !== "undefined" && window.parent !== window && globalThis.origin === "null";
    return bridged;
  }
  function hostOrigin() {
    try {
      return location.origin || "*";
    } catch (e) {
      return "*";
    }
  }
  function tellHost(op, args) {
    const message = { type: BRIDGE_TYPE, id: "", op, args };
    window.parent.postMessage(message, hostOrigin());
  }

  // src/appStorage.ts
  var APP_STORAGE_QUOTA = 512 * 1024;
  var APP_STORAGE_VALUE_MAX = 256 * 1024;
  var SDK_STORAGE_QUOTA = 8 * 1024;

  // src/_storage.ts
  function workspaceSlug() {
    const base = getBaseUrl();
    if (!base) return null;
    const seg = base.replace(/^\/+|\/+$/g, "");
    return seg === "" || seg === "~" ? null : seg;
  }
  function nsKey(key) {
    const slug = workspaceSlug();
    return slug ? `ws:${slug}:${key}` : key;
  }
  function emptyMirror() {
    return {
      sdk: { local: /* @__PURE__ */ new Map(), session: /* @__PURE__ */ new Map() },
      app: { local: /* @__PURE__ */ new Map(), session: /* @__PURE__ */ new Map() }
    };
  }
  var mirror = emptyMirror();
  var primeSettled = false;
  var writtenBeforePrime = /* @__PURE__ */ new Set();
  function noteWrite(space, area, key) {
    if (primeSettled) return;
    writtenBeforePrime.add(key === null ? `${space}:${area}` : `${space}:${area}:${key}`);
  }
  function sdkGet(key, area) {
    var _a;
    return (_a = mirror.sdk[area].get(key)) != null ? _a : null;
  }
  function sdkRemove(key, area) {
    noteWrite("sdk", area, key);
    mirror.sdk[area].delete(key);
    tellHost("storage.remove", { space: "sdk", area, key });
  }
  function wsLocalGet(key) {
    if (isBridged()) return sdkGet(key, "local");
    try {
      return localStorage.getItem(nsKey(key));
    } catch (e) {
      return null;
    }
  }
  function wsLocalRemove(key) {
    if (isBridged()) return sdkRemove(key, "local");
    try {
      localStorage.removeItem(nsKey(key));
    } catch (e) {
    }
  }

  // src/frameCapability.ts
  var CAPABILITY_SEGMENT = "~cap";
  function splitCapability(path) {
    const marker = `/${CAPABILITY_SEGMENT}/`;
    const at = path.indexOf(marker);
    if (at < 0) return null;
    const after = path.slice(at + marker.length);
    const slash = after.indexOf("/");
    if (slash <= 0) return null;
    return { prefix: path.slice(0, at), capability: after.slice(0, slash), rest: after.slice(slash) };
  }
  function baseElement() {
    if (typeof document === "undefined") return null;
    return document.querySelector("base");
  }
  function currentCapability() {
    var _a, _b, _c;
    const href = (_a = baseElement()) == null ? void 0 : _a.getAttribute("href");
    if (!href) return null;
    return (_c = (_b = splitCapability(href)) == null ? void 0 : _b.capability) != null ? _c : null;
  }
  function capabilityCarrier() {
    const capability = currentCapability();
    return capability ? `/${CAPABILITY_SEGMENT}/${capability}` : "";
  }

  // src/_fetch.ts
  function computeBaseUrl() {
    var _a;
    if (typeof document !== "undefined") {
      const href = (_a = document.querySelector("base")) == null ? void 0 : _a.getAttribute("href");
      if (href) {
        let path2 = href;
        try {
          if (/^https?:\/\//i.test(href)) path2 = new URL(href).pathname;
        } catch (e) {
        }
        const capability = splitCapability(path2);
        if (capability) return capability.prefix;
        return path2.replace(/\/+$/, "");
      }
    }
    const path = typeof window !== "undefined" && window.location && window.location.pathname || "";
    const i = path.indexOf("/app/");
    return i >= 0 ? path.slice(0, i) : "";
  }
  var _baseUrl = computeBaseUrl();
  function getBaseUrl() {
    return _baseUrl;
  }
  function dataMountUrl(path) {
    return `${_baseUrl}${capabilityCarrier()}/data/${path}`;
  }

  // src/fontFaces.ts
  function registeredFaces() {
    var _a;
    const holder = globalThis;
    (_a = holder.__lucidosWorkspaceFaceRegistry) != null ? _a : holder.__lucidosWorkspaceFaceRegistry = /* @__PURE__ */ new Map();
    return holder.__lucidosWorkspaceFaceRegistry;
  }
  function registerWorkspaceFont(font, urlFor) {
    if (typeof FontFace === "undefined" || typeof document === "undefined" || !document.fonts) return;
    const registered = registeredFaces();
    for (const face of font.faces) {
      const key = `${font.family}
${face.path}`;
      if (registered.has(key)) continue;
      const weight = registeredFaceWeight(font, face);
      registered.set(key, { family: font.family, path: face.path, weight, style: face.style });
      const fontFace = new FontFace(font.family, `url("${urlFor(face.path)}")`, {
        weight,
        style: face.style,
        display: "swap"
      });
      document.fonts.add(fontFace);
      fontFace.load().catch((err) => console.warn(`[lucidos] workspace font face ${face.path} failed to load:`, err));
    }
  }
  function registerFontsInUse(font, known, monoId, urlFor) {
    for (const entry of known) {
      if (entry === font.workspaceFont || entry.id === monoId) registerWorkspaceFont(entry, urlFor);
    }
  }

  // src/boot/appearanceBoot.ts
  function servedPrefs() {
    const served = globalThis.__lucidosPrefs;
    return served && typeof served === "object" ? served : null;
  }
  function seeded(served, serverKey, storageKey) {
    const value = served == null ? void 0 : served[serverKey];
    return typeof value === "string" && value !== "" ? value : wsLocalGet(storageKey);
  }
  function applyAppearanceBoot(opts) {
    var _a;
    const d = document.documentElement;
    const served = servedPrefs();
    const raw = seeded(served, THEME_MODE_KEY, THEME_MODE_STORAGE_KEY);
    const mode = raw && THEME_MODES.includes(raw) ? raw : DEFAULT_THEME_MODE;
    const prefersLight = matchMedia("(prefers-color-scheme: light)").matches;
    const resolved = resolveThemeMode(mode, prefersLight);
    d.setAttribute(THEME_MODE_ATTRIBUTE, resolved);
    const styleReset = opts.styleReset && styleResetRequested(location.search);
    if (styleReset) wsLocalRemove(THEME_STORAGE_KEY);
    const theme = parseResolvedTheme(seeded(served, THEME_SEED_KEY, THEME_STORAGE_KEY));
    const themeTokens = theme[resolved];
    const bg = (_a = themeBackground(themeTokens)) != null ? _a : THEME_MODE_BG[resolved];
    d.style.setProperty("--bg-primary", bg);
    d.style.background = bg;
    const picked = parseWorkspaceFont(
      seeded(served, WORKSPACE_FONT_SEED_KEY, WORKSPACE_FONT_STORAGE_KEY)
    );
    const known = picked ? [picked, ...theme.workspace_fonts] : theme.workspace_fonts;
    const font = resolveFont(seeded(served, PREF_FONT_FAMILY.key, "lucidos-font-family"), theme.fonts, known);
    d.style.setProperty("--font-ui", font.stack);
    d.style.setProperty("--font-features-text", font.features.text);
    d.style.setProperty("--font-features-code", font.features.code);
    d.setAttribute(FONT_BOLD_ATTRIBUTE, fontBoldMark(font));
    registerFontsInUse(font, known, theme.fonts.mono, dataMountUrl);
    const scale = parseUiScale(
      (served == null ? void 0 : served[PREF_UI_SCALE.key]) || (served == null ? void 0 : served["text-size"]) || (served == null ? void 0 : served["font-size"]) || wsLocalGet("lucidos-ui-scale")
    );
    if (scale !== null) d.style.setProperty("--user-ui-scale", `${scale}%`);
    const reducedMotion = resolveReducedMotion(
      parseMotion(seeded(served, PREF_MOTION.key, MOTION_STORAGE_KEY)),
      matchMedia(REDUCED_MOTION_QUERY).matches
    );
    d.setAttribute("data-motion", motionAttribute(reducedMotion));
    if (opts.durationScale) {
      const position = parseAnimationSpeed(wsLocalGet(ANIMATION_SPEED_STORAGE_KEY));
      d.style.setProperty("--duration-scale", String(durationScaleFor(position, reducedMotion)));
    }
    const reducedThemeEffects = resolveReducedThemeEffects(
      parseThemeEffects(seeded(served, PREF_THEME_EFFECTS.key, THEME_EFFECTS_STORAGE_KEY)),
      matchMedia(REDUCED_TRANSPARENCY_QUERY).matches,
      matchMedia(MORE_CONTRAST_QUERY).matches
    );
    d.setAttribute("data-theme-effects", themeEffectsAttribute(reducedThemeEffects));
    for (const name of Object.keys(themeTokens)) {
      d.style.setProperty(name, themeTokens[name]);
    }
    if (themeTokens["--bg-primary"] && !themeBackground(themeTokens)) d.style.background = "var(--bg-primary)";
    try {
      if (styleReset) {
        wsLocalRemove(STYLE_OVERRIDES_STORAGE_KEY);
      } else {
        const overrides = parseStyleOverrides(
          seeded(served, "style_overrides", STYLE_OVERRIDES_STORAGE_KEY)
        );
        for (const name of Object.keys(overrides)) {
          d.style.setProperty(name, overrides[name]);
        }
      }
    } catch (e) {
    }
    return { raw, mode, resolved, prefersLight, reducedMotion };
  }

  // src/boot/iframe.ts
  applyAppearanceBoot({
    styleReset: false,
    durationScale: false
  });
})();
