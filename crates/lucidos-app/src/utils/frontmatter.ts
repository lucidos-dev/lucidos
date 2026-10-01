/** A frontmatter value: a scalar as written, or a list of scalars. */
export type FrontmatterValue = string | string[];

/** A markdown file's leading YAML block. `fields` holds the simple subset this
 *  module understands. Anything richer (nested maps, block scalars, anchors)
 *  stays `raw`, so it is shown as written rather than guessed at. */
export type Frontmatter =
  | { kind: 'fields'; fields: [string, FrontmatterValue][] }
  | { kind: 'raw'; text: string };

const FENCE_OPEN = /^---\s*$/;
const FENCE_CLOSE = /^(---|\.\.\.)\s*$/;
const KEY_LINE = /^([^\s#:\-[{][^:]*):(?:\s+(.*))?$/;
const LIST_ITEM = /^\s*-\s+(.*)$/;

/** Split `md` into its frontmatter and the markdown body after it.
 *
 *  A block counts only when the file opens with a `---` fence, a closing fence
 *  follows, and the first line inside looks like `key:`. The last rule keeps a
 *  document that merely starts with a horizontal rule rendering as one. */
export function splitFrontmatter(md: string): { frontmatter: Frontmatter | null; body: string } {
  const lines = md.replace(/^﻿/, '').split(/\r?\n/);
  if (!FENCE_OPEN.test(lines[0] ?? '')) return { frontmatter: null, body: md };
  const close = lines.findIndex((line, i) => i > 0 && FENCE_CLOSE.test(line));
  if (close < 0) return { frontmatter: null, body: md };
  const block = lines.slice(1, close);
  const firstContent = block.find((line) => line.trim() !== '' && !line.trimStart().startsWith('#'));
  if (!firstContent || !KEY_LINE.test(firstContent)) return { frontmatter: null, body: md };

  const fields = parseFields(block);
  const frontmatter: Frontmatter = fields ? { kind: 'fields', fields } : { kind: 'raw', text: block.join('\n') };
  return { frontmatter, body: lines.slice(close + 1).join('\n') };
}

/** Top-level `key: value`, `key: [a, b]`, and `key:` over `- item` lines.
 *  Returns `null` for anything else, so the caller shows the block raw. */
function parseFields(block: string[]): [string, FrontmatterValue][] | null {
  const fields: [string, FrontmatterValue][] = [];
  for (const line of block) {
    if (line.trim() === '' || line.trimStart().startsWith('#')) continue;
    const item = LIST_ITEM.exec(line);
    const last = fields[fields.length - 1];
    if (item) {
      const text = stripComment(item[1]);
      const isMap = !/^['"]/.test(text) && KEY_LINE.test(text);
      if (!last || !Array.isArray(last[1]) || isMap) return null;
      last[1].push(unquote(text));
      continue;
    }
    const key = /^\s/.test(line) ? null : KEY_LINE.exec(line);
    if (!key) return null;
    const value = stripComment(key[2] ?? '');
    if (value === '') fields.push([key[1].trim(), []]);
    else if (/^[|>&*!]/.test(value)) return null;
    else if (value.startsWith('[') && value.endsWith(']')) {
      const items = splitFlowList(value.slice(1, -1));
      if (!items) return null;
      fields.push([key[1].trim(), items]);
    } else if (value.startsWith('{')) return null;
    else fields.push([key[1].trim(), unquote(value)]);
  }
  // `key:` with no items under it is an empty scalar, not an empty list.
  return fields.map(([k, v]) => [k, Array.isArray(v) && v.length === 0 ? '' : v]);
}

/** The items of a flow list, splitting only on commas outside quotes.
 *  A nested collection or an unclosed quote returns `null`. */
function splitFlowList(inner: string): string[] | null {
  const items: string[] = [];
  let current = '';
  let quote: string | null = null;
  for (const ch of inner) {
    if (quote) {
      if (ch === quote) quote = null;
    } else if (ch === '"' || ch === "'") {
      quote = ch;
    } else if (ch === ',') {
      items.push(current);
      current = '';
      continue;
    } else if ('[]{}'.includes(ch)) {
      return null;
    }
    current += ch;
  }
  if (quote) return null;
  items.push(current);
  return items.map(unquote).filter((item) => item !== '');
}

/** `value` without a trailing ` # comment`. A `#` inside quotes, or one not
 *  preceded by whitespace, is part of the value. */
function stripComment(value: string): string {
  let quote: string | null = null;
  for (let i = 0; i < value.length; i++) {
    const ch = value[i];
    if (quote) {
      if (ch === quote) quote = null;
    } else if (ch === '"' || ch === "'") {
      quote = ch;
    } else if (ch === '#' && (i === 0 || /\s/.test(value[i - 1]))) {
      return value.slice(0, i).trim();
    }
  }
  return value.trim();
}

function unquote(value: string): string {
  const trimmed = value.trim();
  const quoted = trimmed.length >= 2 && (trimmed[0] === '"' || trimmed[0] === "'") && trimmed.endsWith(trimmed[0]);
  return quoted ? trimmed.slice(1, -1) : trimmed;
}
