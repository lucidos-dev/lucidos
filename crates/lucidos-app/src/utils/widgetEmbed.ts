/** The *widget embed* grammar (ADR 0415): `![label](app:<id>?params={...})`
 *  in markdown draws a widget wherever a picture draws.
 *
 *  This is the TypeScript definition; the engine's `engine/widget_embed.rs` is
 *  the Rust one. Both read `widget_embeds.fixture.json`, so an embed the engine
 *  accepts is one this draws, and the reverse. Raw JSON params may hold spaces
 *  and `)`, which a markdown link cannot, so this scans the text itself. */

import { isWidgetParams } from '@lucidos/sdk';
import type { WidgetEmbed } from '../generated/thread-event-wire';

const OPEN = '](app:';
/** The embed grammar's params marker. It reads like the frame URL's query
 *  key, but it is markdown syntax the agent writes, and stays its own. */
const EMBED_PARAMS = '?params=';

/** One embed found in markdown: where it sits, and what it says. */
export interface EmbedMatch {
  start: number;
  end: number;
  /** The embed, or why its syntax is broken. */
  embed: { ok: true; value: WidgetEmbed } | { ok: false; reason: string };
}

/** Every embed in `text`, in order. A broken one is still a match. */
export function scanWidgetEmbeds(text: string): EmbedMatch[] {
  const code = codeRanges(text);
  const found: EmbedMatch[] = [];
  let from = 0;
  for (;;) {
    const open = text.indexOf(OPEN, from);
    if (open < 0) return found;
    const inCode = code.find(([start, end]) => open >= start && open < end);
    if (inCode) {
      from = inCode[1];
      continue;
    }
    const start = text.lastIndexOf('![', open);
    const match = start < 0 ? null : matchFrom(text, start, open);
    if (!match) {
      from = open + OPEN.length;
      continue;
    }
    found.push(match);
    from = Math.max(match.end, open + OPEN.length);
  }
}

/** The ranges of code in `text`: a run of backticks up to the next run of
 *  the same length, so an inline span and a fenced block alike. An embed in
 *  code is an example, as markdown draws no picture there. */
function codeRanges(text: string): Array<[number, number]> {
  const runAt = (i: number) => {
    let n = 0;
    while (text[i + n] === '`') n += 1;
    return n;
  };
  const ranges: Array<[number, number]> = [];
  let i = 0;
  while (i < text.length) {
    if (text[i] !== '`') {
      i += 1;
      continue;
    }
    const len = runAt(i);
    let j = i + len;
    let close: number | null = null;
    for (;;) {
      const at = text.indexOf('`', j);
      if (at < 0) break;
      const n = runAt(at);
      if (n === len) {
        close = at + n;
        break;
      }
      j = at + n;
    }
    if (close === null) {
      i += len;
    } else {
      ranges.push([i, close]);
      i = close;
    }
  }
  return ranges;
}

/** The embed that starts `text`, if one does. The markdown renderer reads an
 *  embed this way, token by token, so one inside code stays code. */
export function widgetEmbedAtStart(text: string): EmbedMatch | null {
  if (!text.startsWith('![')) return null;
  const open = text.indexOf(OPEN);
  return open < 0 ? null : matchFrom(text, 0, open);
}

/** The embed spanning `start` (its `![`) and `open` (its `](app:`). The label
 *  between must be one line with no `]` and no other `![`. */
function matchFrom(text: string, start: number, open: number): EmbedMatch | null {
  const labelText = text.slice(start + 2, open);
  if (labelText.includes(']') || labelText.includes('\n') || labelText.includes('![')) return null;
  const restAt = open + OPEN.length;
  const target = parseTarget(text.slice(restAt));
  const end = Math.min(restAt + target.length, text.length);
  const label = labelText.trim();
  return {
    start,
    end,
    embed: target.ok
      ? { ok: true, value: { app_id: target.appId, ...(target.params ? { params: target.params } : {}), ...(label ? { label } : {}) } }
      : { ok: false, reason: target.reason },
  };
}

type Target =
  | { ok: true; length: number; appId: string; params?: Record<string, unknown> }
  | { ok: false; length: number; reason: string };

/** Parse `<id>[?params=<json>])` after the `app:`, with the length up to and
 *  including the closing `)`. A failure still reports a length. */
function parseTarget(rest: string): Target {
  const idMatch = /^[A-Za-z0-9._-]*/.exec(rest);
  const appId = idMatch ? idMatch[0] : '';
  const idLen = appId.length;
  const skipToClose = (at: number) => {
    const i = rest.indexOf(')', at);
    return i < 0 ? rest.length : i + 1;
  };
  if (!appId || appId.startsWith('.') || appId.includes('..')) {
    return { ok: false, length: skipToClose(idLen), reason: `'${appId}' is not a widget id` };
  }
  const afterId = rest.slice(idLen);
  if (afterId.startsWith(')')) return { ok: true, length: idLen + 1, appId };
  if (!afterId.startsWith(EMBED_PARAMS)) {
    return { ok: false, length: skipToClose(idLen), reason: `the embed for '${appId}' must end after the id or carry ?params={…}` };
  }
  const queryAt = idLen + EMBED_PARAMS.length;
  const query = rest.slice(queryAt);
  let json: string;
  let jsonLen: number;
  if (query.startsWith('{')) {
    const len = balancedObjectLength(query);
    if (len === null) return { ok: false, length: rest.length, reason: `the params for '${appId}' are not a closed JSON object` };
    json = query.slice(0, len);
    jsonLen = len;
  } else if (query.startsWith('%')) {
    const close = query.indexOf(')');
    const len = close < 0 ? query.length : close;
    const decoded = percentDecode(query.slice(0, len));
    if (decoded === null) return { ok: false, length: queryAt + len + 1, reason: `the params for '${appId}' are not valid percent-encoding` };
    json = decoded;
    jsonLen = len;
  } else {
    return { ok: false, length: skipToClose(queryAt), reason: `the params for '${appId}' must be a JSON object` };
  }
  const closeAt = queryAt + jsonLen;
  if (!rest.startsWith(')', closeAt)) {
    return { ok: false, length: skipToClose(closeAt), reason: `the embed for '${appId}' must close with ) right after its params` };
  }
  let parsed: unknown;
  try {
    parsed = JSON.parse(json);
  } catch {
    parsed = undefined;
  }
  if (!isWidgetParams(parsed)) {
    return { ok: false, length: closeAt + 1, reason: `the params for '${appId}' are not a JSON object` };
  }
  const params = parsed;
  return { ok: true, length: closeAt + 1, appId, ...(Object.keys(params).length > 0 ? { params } : {}) };
}

/** The length of the JSON object at the start of `s`, string-aware. */
function balancedObjectLength(s: string): number | null {
  let depth = 0;
  let inString = false;
  let escaped = false;
  for (let i = 0; i < s.length; i++) {
    const c = s[i];
    if (inString) {
      if (escaped) escaped = false;
      else if (c === '\\') escaped = true;
      else if (c === '"') inString = false;
      continue;
    }
    if (c === '"') inString = true;
    else if (c === '{' || c === '[') depth += 1;
    else if (c === '}' || c === ']') {
      depth -= 1;
      if (depth < 0) return null;
      if (depth === 0) return i + 1;
    }
  }
  return null;
}

function percentDecode(s: string): string | null {
  if (/%(?![0-9A-Fa-f]{2})/.test(s)) return null;
  try {
    return decodeURIComponent(s);
  } catch {
    return null;
  }
}

/** `text` with the matched embeds cut out and the ends trimmed, or `null`
 *  when nothing else is left. */
export function withoutWidgetEmbeds(text: string, matches: readonly EmbedMatch[]): string | null {
  let out = '';
  let at = 0;
  for (const m of matches) {
    out += text.slice(at, m.start);
    at = m.end;
  }
  out += text.slice(at);
  const trimmed = out.trim();
  return trimmed ? trimmed : null;
}
