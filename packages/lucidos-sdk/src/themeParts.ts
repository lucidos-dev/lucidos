/**
 * The TypeScript twin of the engine's theme part grammar
 * (`crates/lucidos-engine/src/core/themes/grammar.rs`, ADR 0307).
 *
 * The engine checks every part value in a theme, and every `--part-*` style
 * override at the preference write. Each apply site checks again here, so a
 * value that reached storage some other way still never reaches
 * `setProperty`. `theme-part-cases.json` feeds both sides, so they accept,
 * refuse and canonicalise alike.
 *
 * Pure, like `appearance.ts`: the boot script bundles it.
 */
import {
  COLOUR_TOKENS,
  FRAME_COLOUR_TOKENS,
  NAMED_COLOURS,
  PART_PROPERTIES,
  PART_TOKENS,
  type PartGrammar,
  type PartUnit,
} from './generated/theme-parts';

export const PART_TOKEN_PREFIX = '--part-';

const MAX_MIX_DEPTH = 2;
const MAX_VALUE_LENGTH = 120;
const VALUE_BANNED_RE = /[;{}<>@\\]|url\s*\(|image-set\s*\(|expression\s*\(|\/\*/i;

const COLOUR_TOKEN_SET = new Set<string>(COLOUR_TOKENS);
const FRAME_COLOUR_TOKEN_SET = new Set<string>(FRAME_COLOUR_TOKENS);
const NAMED_COLOUR_SET = new Set<string>(NAMED_COLOURS);
const COLOUR_FUNCTIONS = ['rgb', 'rgba', 'hsl', 'hsla', 'oklab', 'oklch', 'color-mix', 'var'];

/** `{ ok: canonical }` or `{ error: why }`, the engine's messages word for word. */
export type PartCheck = { ok: string } | { error: string };

interface Slot {
  property: string;
  part: string;
  grammar: PartGrammar;
  insetOnly: boolean;
  /** The part paints in app frames, which register the frame tokens only. */
  frames: boolean;
}

class Refusal extends Error {}

function refuse(why: string): never {
  throw new Refusal(why);
}

/** Check a value for a part token. Unknown tokens are refused. */
export function checkPartToken(name: string, value: string): PartCheck {
  const spec = Object.prototype.hasOwnProperty.call(PART_TOKENS, name) ? PART_TOKENS[name] : null;
  if (!spec) return { error: 'not a part token. GET /api/v1/themes/parts lists them.' };
  const trimmed = typeof value === 'string' ? value.trim() : '';
  if (trimmed === '' || trimmed.length > MAX_VALUE_LENGTH || VALUE_BANNED_RE.test(trimmed)) {
    return {
      error: `the value is empty, longer than ${MAX_VALUE_LENGTH} characters, or uses a banned form (url(), ;, braces, @, backslash, a comment).`,
    };
  }
  const slot: Slot = {
    property: spec.property,
    part: spec.part,
    grammar: PART_PROPERTIES[spec.property],
    insetOnly: spec.insetOnly,
    frames: spec.frames,
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

function canonical(slot: Slot, value: string): string {
  const g = slot.grammar;
  switch (g.grammar) {
    case 'colour': {
      const colour = parseColour(slot, value, false, 0);
      if (g.minAlpha !== undefined && colour.alpha < g.minAlpha) {
        refuse(`alpha ${num(Math.round(colour.alpha * 100) / 100)} is under the ${num(g.minAlpha)} floor for text.`);
      }
      return colour.css;
    }
    case 'shadow': {
      if (value === 'none') return value;
      const layers = splitTopLevel(value, c => c === ',');
      if (layers.length > g.maxLayers) refuse(`${layers.length} layers; the limit is ${g.maxLayers}.`);
      const caps: Caps = { unit: g.unit, maxOffset: g.maxOffset, maxBlur: g.maxBlur };
      return layers.map(layer => shadowLayer(slot, layer, caps, g.maxSpread)).join(', ');
    }
    case 'drop-shadow': {
      if (value === 'none') return value;
      const calls = splitTopLevel(value, isSpace);
      if (calls.length !== 1) refuse(`${slot.property} takes exactly one drop-shadow().`);
      const call = functionCall(calls[0]);
      if (!call || call[0] !== 'drop-shadow') refuse(notAllowed(calls[0]));
      const parts = splitTopLevel(call[1], isSpace);
      if (parts.length !== 4) refuse('drop-shadow() takes <x> <y> <blur> <colour>.');
      const caps: Caps = { unit: g.unit, maxOffset: g.maxOffset, maxBlur: g.maxBlur };
      const geometry = shadowGeometry(slot, caps, parts[0], parts[1], parts[2]);
      return `drop-shadow(${geometry} ${parseColour(slot, parts[3], true, 0).css})`;
    }
    case 'spacing':
      return value === 'normal' ? value : boundedLength(slot, value, g.unit, g.min, g.max);
    case 'length':
      return boundedLength(slot, value, g.unit, g.min, g.max);
    case 'keyword':
      if (!g.values.includes(value)) refuse(`${slot.property} takes ${g.values.join(', ')}.`);
      return value;
    case 'scanlines':
      return value === 'none' ? value : scanlines(slot, value, g.maxAlpha, g.maxPeriod, g.maxStops);
  }
}

function boundedLength(slot: Slot, value: string, unit: PartUnit, min: number, max: number): string {
  const v = length(slot, value, unit);
  const shown = withUnit(v, unit);
  if (v > max) refuse(`${shown} is over the ${withUnit(max, unit)} cap.`);
  if (v < min) refuse(`${shown} is under the ${withUnit(min, unit)} floor.`);
  return shown;
}

function isAngle(token: string): boolean {
  return ['deg', 'grad', 'rad', 'turn'].some(unit =>
    token.endsWith(unit) && plainNumber(token.slice(0, -unit.length)) !== null);
}

/** A vertical `repeating-linear-gradient` of faint literal stops: the
 *  engine's `scanlines` grammar. */
function scanlines(slot: Slot, value: string, maxAlpha: number, maxPeriod: number, maxStops: number): string {
  const call = functionCall(value);
  if (!call) refuse(`${slot.property} takes none or repeating-linear-gradient().`);
  if (call[0] !== 'repeating-linear-gradient') refuse(notAllowed(value));
  const stops = splitTopLevel(call[1], c => c === ',');
  if (stops.length < 2) refuse('a gradient takes at least 2 stops.');
  if (stops.length > maxStops) refuse(`${stops.length} stops; the limit is ${maxStops}.`);
  let previous = 0;
  let period = 0;
  const out = stops.map((stop, i) => {
    const tokens = splitTopLevel(stop, isSpace);
    if (i === 0 && (tokens[0] === 'to' || isAngle(tokens[0]))) {
      refuse('scanlines take no direction: they run top to bottom.');
    }
    if (tokens.length > 2) refuse('a stop is a colour and an optional px position.');
    const colour = scanlineColour(tokens[0]);
    if (colour.alpha > maxAlpha) {
      refuse(`stop alpha ${num(Math.round(colour.alpha * 100) / 100)} is over the ${num(maxAlpha)} cap.`);
    }
    if (tokens.length === 1) return colour.css;
    const v = length(slot, tokens[1], 'px');
    const shown = withUnit(v, 'px');
    if (v < 0) refuse(`position ${shown} is under 0.`);
    if (v > maxPeriod) refuse(`position ${shown} is over the ${withUnit(maxPeriod, 'px')} period cap.`);
    if (v < previous) refuse('stop positions must not go down.');
    previous = v;
    if (i === stops.length - 1) period = v;
    return `${colour.css} ${shown}`;
  });
  if (period <= 0) refuse('the last stop needs a px position above 0: it sets the period.');
  return `repeating-linear-gradient(${out.join(', ')})`;
}

/** A literal colour whose alpha is known, so the cap holds for any input. */
function scanlineColour(value: string): Colour {
  if (value === 'transparent') return { css: value, alpha: 0 };
  if (value.startsWith('#')) return parseHex(value);
  const call = functionCall(value);
  if (call && ['rgb', 'rgba', 'hsl', 'hsla'].includes(call[0])) return channels(call[0], call[1]);
  refuse('a scanline stop takes a hex, rgb(), rgba(), hsl() or hsla() colour, or transparent.');
}

interface Caps {
  unit: PartUnit;
  maxOffset: number;
  maxBlur: number;
}

function shadowGeometry(slot: Slot, caps: Caps, x: string, y: string, blur: string): string {
  const offset = (axis: string, token: string): string => {
    const v = length(slot, token, caps.unit);
    if (Math.abs(v) > caps.maxOffset) {
      refuse(`${axis} ${withUnit(v, caps.unit)} is over the ±${withUnit(caps.maxOffset, caps.unit)} cap.`);
    }
    return withUnit(v, caps.unit);
  };
  const xs = offset('x', x);
  const ys = offset('y', y);
  const b = length(slot, blur, caps.unit);
  if (b < 0) refuse(`blur ${withUnit(b, caps.unit)} is under 0.`);
  if (b > caps.maxBlur) refuse(`blur ${withUnit(b, caps.unit)} is over the ${withUnit(caps.maxBlur, caps.unit)} cap.`);
  return `${xs} ${ys} ${withUnit(b, caps.unit)}`;
}

function shadowLayer(slot: Slot, layer: string, caps: Caps, maxSpread: number | undefined): string {
  const tokens = splitTopLevel(layer, isSpace);
  const inset = tokens[0] === 'inset';
  if (inset) {
    if (maxSpread === undefined) refuse(notAllowed('inset'));
    tokens.shift();
  } else if (slot.insetOnly) {
    refuse(`the ${slot.part} takes inset shadows only.`);
  }
  const shape = maxSpread !== undefined
    ? '[inset] <x> <y> <blur> [<spread>] <colour>'
    : '<x> <y> <blur> <colour>';
  let geometry: string;
  let spread: string | null = null;
  let colour: string;
  if (tokens.length === 4) {
    geometry = shadowGeometry(slot, caps, tokens[0], tokens[1], tokens[2]);
    colour = tokens[3];
  } else if (tokens.length === 5 && maxSpread !== undefined) {
    const s = length(slot, tokens[3], caps.unit);
    if (Math.abs(s) > maxSpread) {
      refuse(`spread ${withUnit(s, caps.unit)} is over the ±${withUnit(maxSpread, caps.unit)} cap.`);
    }
    geometry = shadowGeometry(slot, caps, tokens[0], tokens[1], tokens[2]);
    spread = withUnit(s, caps.unit);
    colour = tokens[4];
  } else {
    const bad = tokens.find(t => t.includes('(') && !isColourCall(t));
    if (bad !== undefined) refuse(notAllowed(bad));
    refuse(`${slot.property} takes ${shape}.`);
  }
  const out: string[] = [];
  if (inset) out.push('inset');
  out.push(geometry);
  if (spread !== null) out.push(spread);
  out.push(parseColour(slot, colour, true, 0).css);
  return out.join(' ');
}

function isColourCall(token: string): boolean {
  const call = functionCall(token);
  return !!call && COLOUR_FUNCTIONS.includes(call[0]);
}

function isSpace(c: string): boolean {
  return /\s/.test(c);
}

function length(slot: Slot, token: string, unit: PartUnit): number {
  if (token.includes('(')) refuse(notAllowed(token));
  const match = /[a-z%]/.exec(token);
  const split = match ? match.index : token.length;
  const digits = token.slice(0, split);
  const suffix = token.slice(split);
  const v = plainNumber(digits);
  if (v === null || !/^[a-z%]*$/.test(suffix)) refuse(`'${token}' is not a length.`);
  if (suffix === '' && v === 0) return 0;
  if (suffix === unit) return v;
  refuse(`use ${unit} for ${slot.property} lengths.`);
}

/** Optional sign, digits, optional fraction. No exponent, and at most six
 *  digits either side of the point, where Rust and JavaScript write a number
 *  the same way. */
function plainNumber(token: string): number | null {
  return /^[+-]?(?:\d{1,6}(?:\.\d{1,6})?|\.\d{1,6})$/.test(token) ? Number(token) : null;
}

/** The engine's number form: shortest round trip, never `-0`. */
function num(v: number): string {
  return v === 0 ? '0' : String(v);
}

function withUnit(v: number, unit: PartUnit): string {
  return v === 0 ? '0' : `${num(v)}${unit}`;
}

function notAllowed(token: string): string {
  const call = functionCall(token);
  if (call) return `${call[0]}() is not allowed here.`;
  const open = token.indexOf('(');
  return open >= 0 ? `${token.slice(0, open)}() is not allowed here.` : `'${token}' is not allowed here.`;
}

function functionCall(token: string): [string, string] | null {
  const open = token.indexOf('(');
  if (open < 0 || !token.endsWith(')')) return null;
  const name = token.slice(0, open);
  const inner = token.slice(open + 1, -1);
  let depth = 0;
  for (const c of inner) {
    if (c === '(') depth++;
    else if (c === ')') depth--;
    if (depth < 0) return null;
  }
  return depth === 0 && /^[a-z-]+$/.test(name) ? [name, inner] : null;
}

/** Split at `sep` outside parentheses, trimming and dropping empty pieces. */
function splitTopLevel(value: string, sep: (c: string) => boolean): string[] {
  const parts: string[] = [];
  let depth = 0;
  let start = 0;
  for (let i = 0; i < value.length; i++) {
    const c = value[i];
    if (c === '(') depth++;
    else if (c === ')') depth--;
    else if (depth === 0 && sep(c)) {
      parts.push(value.slice(start, i));
      start = i + 1;
    }
  }
  parts.push(value.slice(start));
  return parts.map(p => p.trim()).filter(p => p !== '');
}

interface Colour {
  css: string;
  /** The most alpha the colour can have. A `var()` counts as opaque. */
  alpha: number;
}

function parseColour(slot: Slot, raw: string, shadow: boolean, mixDepth: number): Colour {
  const value = raw.trim();
  if (value.startsWith('#')) return parseHex(value);
  const call = functionCall(value);
  if (call) {
    const [name, args] = call;
    switch (name) {
      case 'rgb': case 'rgba': case 'hsl': case 'hsla': case 'oklab': case 'oklch':
        return channels(name, args);
      case 'color-mix':
        return colorMix(slot, args, mixDepth);
      case 'var': {
        if (args.includes(',')) refuse('var() takes no fallback here.');
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
  if (value.includes('(')) refuse(notAllowed(value));
  if (value === 'currentcolor') return { css: value, alpha: 1 };
  if (value === 'transparent') {
    if (shadow || mixDepth > 0) return { css: value, alpha: 0 };
    refuse('transparent is not allowed here.');
  }
  if (NAMED_COLOUR_SET.has(value)) return { css: value, alpha: 1 };
  refuse(`'${value}' is not a colour.`);
}

function parseHex(value: string): Colour {
  const hex = value.slice(1);
  if (![3, 4, 6, 8].includes(hex.length) || !/^[0-9a-f]+$/.test(hex)) refuse(`'${value}' is not a hex colour.`);
  const alphaByte = hex.length === 4 ? parseInt(hex[3], 16) * 17
    : hex.length === 8 ? parseInt(hex.slice(6), 16)
      : 255;
  return { css: value, alpha: alphaByte / 255 };
}

type Component = [number, boolean];

function component(token: string): Component | null {
  if (token.endsWith('%')) {
    const v = plainNumber(token.slice(0, -1));
    return v === null ? null : [v, true];
  }
  const v = plainNumber(token);
  return v === null ? null : [v, false];
}

function fmtComponent([v, pct]: Component): string {
  return pct ? `${num(v)}%` : num(v);
}

function channels(name: string, args: string): Colour {
  const bad = `${name}() takes 3 numbers or percentages and an optional alpha.`;
  if (args.includes('(')) refuse(bad);
  let values: Component[];
  let css: string;
  if (args.includes(',')) {
    const parsed = args.split(',').map(t => component(t.trim()));
    if (parsed.some(v => v === null) || (parsed.length !== 3 && parsed.length !== 4)) refuse(bad);
    values = parsed as Component[];
    css = `${name}(${values.map(fmtComponent).join(', ')})`;
  } else {
    const slash = args.indexOf('/');
    const colour = slash >= 0 ? args.slice(0, slash) : args;
    const alpha = slash >= 0 ? args.slice(slash + 1).trim() : null;
    const parsed = colour.split(/\s+/).filter(t => t !== '').map(component);
    if (parsed.some(v => v === null) || parsed.length !== 3) refuse(bad);
    values = parsed as Component[];
    let text = values.map(fmtComponent).join(' ');
    if (alpha !== null) {
      const a = component(alpha);
      if (!a) refuse(bad);
      text += ` / ${fmtComponent(a)}`;
      values.push(a);
    }
    css = `${name}(${text})`;
  }
  const a = values[3];
  const alpha = a ? (a[1] ? a[0] / 100 : a[0]) : 1;
  return { css, alpha: Math.min(1, Math.max(0, alpha)) };
}

function colorMix(slot: Slot, args: string, depth: number): Colour {
  if (depth >= MAX_MIX_DEPTH) refuse(`color-mix() nests at most ${MAX_MIX_DEPTH} deep.`);
  const parts = splitTopLevel(args, c => c === ',');
  if (parts.length !== 3) refuse('color-mix() takes a colour space and two colours.');
  const space = parts[0].split(/\s+/).filter(t => t !== '');
  if (space.length !== 2 || space[0] !== 'in' || !['srgb', 'oklab', 'oklch'].includes(space[1])) {
    refuse('color-mix() mixes in srgb, oklab or oklch.');
  }
  const [c1, p1] = mixInput(slot, parts[1], depth);
  const [c2, p2] = mixInput(slot, parts[2], depth);
  const w = (p: number | null): number | null => (p === null ? null : p / 100);
  let w1: number;
  let w2: number;
  const [a, b] = [w(p1), w(p2)];
  if (a === null && b === null) [w1, w2] = [0.5, 0.5];
  else if (b === null) [w1, w2] = [a as number, 1 - (a as number)];
  else if (a === null) [w1, w2] = [1 - b, b];
  else [w1, w2] = [a, b];
  const total = w1 + w2;
  const alpha = total <= 0 ? 0 : (c1.alpha * w1 + c2.alpha * w2) / total * Math.min(total, 1);
  const input = (c: Colour, p: number | null): string => (p === null ? c.css : `${c.css} ${num(p)}%`);
  return { css: `color-mix(in ${space[1]}, ${input(c1, p1)}, ${input(c2, p2)})`, alpha };
}

function mixInput(slot: Slot, arg: string, depth: number): [Colour, number | null] {
  const tokens = splitTopLevel(arg, isSpace);
  const pctOf = (t: string | undefined): number | null => {
    if (t === undefined || !t.endsWith('%')) return null;
    const v = plainNumber(t.slice(0, -1));
    return v !== null && v >= 0 && v <= 100 ? v : null;
  };
  let pct = pctOf(tokens[tokens.length - 1]);
  if (pct !== null) tokens.pop();
  else {
    pct = pctOf(tokens[0]);
    if (pct !== null) tokens.shift();
  }
  if (tokens.length !== 1) refuse('a color-mix() input is one colour and an optional percentage.');
  return [parseColour(slot, tokens[0], false, depth + 1), pct];
}
