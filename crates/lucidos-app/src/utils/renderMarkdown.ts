import { marked } from 'marked';
import type { Tokens } from 'marked';
import { lucidos } from '@lucidos/sdk';
import { COPY_ICON, escapeHtmlAttr } from './markedConfig';
import { addMarkdownParseMs } from './renderPhaseTimers';
import { WORKSPACE_ID } from './basePath';
import { slugifyWorkspaceName } from './slug';

/** Real destination for a thread link, so hovering shows where it goes instead
 *  of the `#`-resolves-to-the-current-page URL. Behind the gateway every
 *  workspace is same-origin under `/<slug>/`, so the href points straight at
 *  the peer.
 *
 *  The left-click is still intercepted by the global `.thread-link` handler
 *  (`useStartup`), which does the authoritative routing. This href is for the
 *  hover tooltip, middle-click and accessibility. On a bare engine port there
 *  is no peer URL to build synchronously, so `#` stands. */
function threadLinkHref(workspace: string | undefined, threadId: string): string {
  if (WORKSPACE_ID === null || typeof location === 'undefined') return '#';
  const slug = workspace ? slugifyWorkspaceName(workspace) : WORKSPACE_ID;
  return `${location.origin}/${encodeURIComponent(slug)}/#thread=${threadId}`;
}

// Drops the <a> wrapper while preserving nested inline markdown. Two reasons:
// the helper's outputs nest inside <button>, where <a> is an invalid
// interactive-in-interactive descendant; and discarding href neutralizes
// javascript:-scheme URLs from LLM-supplied text.
const inlineLinkStripRenderer = new marked.Renderer();
inlineLinkStripRenderer.link = function({ tokens }: Tokens.Link): string {
  return this.parser.parseInline(tokens);
};

// KEEPS http(s) links as real <a> elements, forcing safe new-tab attributes.
// Any other href (javascript:, data:, mailto:, relative) collapses to its
// label text, so an LLM-supplied scheme can neither execute nor dead-end.
// Only valid in NON-interactive containers: an <a> nested in a <button> is
// interactive-in-interactive, so option buttons keep inlineLinkStripRenderer.
const inlineLinkKeepRenderer = new marked.Renderer();
inlineLinkKeepRenderer.link = function({ href, tokens }: Tokens.Link): string {
  const text = this.parser.parseInline(tokens);
  if (!/^https?:\/\//i.test(href)) return text;
  return `<a href="${escapeHtmlAttr(href)}" target="_blank" rel="noopener">${text}</a>`;
};

// Boundary marker for a multiline copy block. Survives marked processing.
const COPY_MARKER = 'LUCIDOS_COPY_BLOCK';
const COPY_MARKER_PATTERN = new RegExp(
  `<!--${COPY_MARKER}_START_(\\d+)-->([\\s\\S]*?)<!--${COPY_MARKER}_END_\\1-->`,
  'g',
);

const CODE_PROTECTION_PATTERN = /```[\s\S]*?```|`[^`\n]+`/g;

/**
 * Convert <copy>...</copy> tags to copyable UI blocks.
 *
 * A single-line tag wraps directly in a <span> before marked. A multiline one
 * uses HTML comment markers that survive marked, which postprocessCopyBlocks
 * then wraps: CommonMark's HTML block rule ends a <div> at the first blank
 * line.
 */
function preprocessCopyBlocks(md: string, encodedTexts: Map<number, string>): string {
  // Protect fenced code blocks and inline code spans from copy tag matching.
  const codeSlots: string[] = [];
  let safeMd = md.replace(CODE_PROTECTION_PATTERN, (match) => {
    const idx = codeSlots.length;
    codeSlots.push(match);
    return `\x00CODE${idx}\x00`;
  });

  let counter = 0;
  safeMd = safeMd.replace(/<copy>([\s\S]*?)<\/copy>/g, (_match, content: string) => {
    const trimmed = content.trim();
    const encoded = trimmed
      .replace(/&/g, '&amp;')
      .replace(/"/g, '&quot;')
      .replace(/\n/g, '&#10;');

    const isMultiline = trimmed.includes('\n');

    // Restore any protected code spans in the copy text so backticks are preserved
    const restoredEncoded = encoded.replace(/\x00CODE(\d+)\x00/g, (_, idx) => {
      const original = codeSlots[parseInt(idx, 10)];
      return original.replace(/&/g, '&amp;').replace(/"/g, '&quot;');
    });

    if (!isMultiline) {
      return `<span class="copyable-block" data-copy-text="${restoredEncoded}">` +
        trimmed +
        `<button type="button" class="copy-btn" aria-label="Copy to clipboard">${COPY_ICON}</button>` +
        `</span>`;
    }

    const id = counter++;
    encodedTexts.set(id, restoredEncoded);
    return `<!--${COPY_MARKER}_START_${id}-->\n\n${trimmed}\n\n<!--${COPY_MARKER}_END_${id}-->`;
  });

  return safeMd.replace(/\x00CODE(\d+)\x00/g, (_, idx) => codeSlots[parseInt(idx, 10)]);
}

function postprocessCopyBlocks(html: string, encodedTexts: Map<number, string>): string {
  return html.replace(COPY_MARKER_PATTERN, (_match, idStr: string, inner: string) => {
    const id = parseInt(idStr, 10);
    const encoded = encodedTexts.get(id) ?? '';
    return `<div class="copyable-block copyable-block-multi" data-copy-text="${encoded}">` +
      inner.trim() +
      `<button type="button" class="copy-btn" aria-label="Copy to clipboard">${COPY_ICON}</button>` +
      `</div>`;
  });
}

// Escape the HTML elements that survive marked processing. Raw HTML in
// markdown source passes through marked unescaped and renders as real
// elements.
//
// `animate` / `animateTransform` / `set` are here rather than in the
// URL-attribute filter because they reach a URL by INDIRECTION: `<animate
// attributeName="href" values="javascript:...">` animates a sibling `<a>`'s
// href, and a name-based attribute filter cannot see it. Escaping the element
// is the only check that holds.
const DANGEROUS_TAG =
  /<(\/?)(iframe|script|style|object|embed|applet|base|meta|link|animate|animateTransform|set)(\s[^>]*)?>/gi;
/** An attribute NAME that carries executable script. */
const EVENT_HANDLER_NAME = /^on[a-z0-9_-]+$/i;
/** The attribute NAMES whose value is fetched or navigated as a URL.
 *
 *  `xlink:href` is the SVG spelling of `href` and navigates identically, and
 *  `action` / `formaction` submit to their value. Only a dangerous SCHEME is
 *  ever stripped, so widening the name list cannot touch an ordinary link. */
const URL_ATTR_NAME =
  /^(href|xlink:href|src|srcset|action|formaction|poster|background|ping|data)$/i;

function entityCodePoint(code: number): string | null {
  if (!Number.isInteger(code) || code < 0 || code > 0x10ffff) return null;
  return String.fromCodePoint(code);
}

function decodeHtmlEntitiesForScheme(value: string): string {
  return value.replace(/&(#x[0-9a-f]+|#[0-9]+|[a-z][a-z0-9]+);?/gi, (match, entity: string) => {
    const e = entity.toLowerCase();
    if (e.startsWith('#x')) {
      const code = Number.parseInt(e.slice(2), 16);
      return entityCodePoint(code) ?? match;
    }
    if (e.startsWith('#')) {
      const code = Number.parseInt(e.slice(1), 10);
      return entityCodePoint(code) ?? match;
    }
    switch (e) {
      case 'colon': return ':';
      case 'tab': return '\t';
      case 'newline': return '\n';
      case 'amp': return '&';
      default: return match;
    }
  });
}

function isDangerousUrlAttrValue(rawValue: string): boolean {
  const unquoted = (rawValue.startsWith('"') && rawValue.endsWith('"'))
    || (rawValue.startsWith("'") && rawValue.endsWith("'"))
    ? rawValue.slice(1, -1)
    : rawValue;
  const normalized = decodeHtmlEntitiesForScheme(unquoted)
    .trimStart()
    .replace(/[\u0000-\u0020]+/g, '')
    .toLowerCase();
  return normalized.startsWith('javascript:') || normalized.startsWith('data:');
}

/** ASCII whitespace, per the HTML spec's definition. Module scope rather than a
 *  closure: this is called for every character of every tag on every
 *  cache-missing render. */
function isHtmlSpace(c: string): boolean {
  return c === ' ' || c === '\t' || c === '\n' || c === '\r' || c === '\f';
}

/** End index (exclusive) of the comment that opens at `start` (`<!--`).
 *
 *  Searched from `start + 2`, not past the whole `<!--`, because `<!-->` and
 *  `<!--->` are COMPLETE empty comments. Starting later would miss their
 *  terminator and swallow the rest of the document unscrubbed. Both spec
 *  terminators are accepted, earliest wins, for the same reason. An
 *  unterminated comment runs to the end, as it does in the browser. */
function commentEnd(html: string, start: number): number {
  const from = start + 2;
  const plain = html.indexOf('-->', from);
  const bang = html.indexOf('--!>', from);
  if (plain === -1 && bang === -1) return html.length;
  if (bang === -1 || (plain !== -1 && plain <= bang)) return plain + 3;
  return bang + 4;
}

/** End index (exclusive) of the tag that opens at `start`, quote-aware.
 *
 *  Quote-aware rather than "up to the first `>`", and that is load-bearing: an
 *  attribute value may itself contain `>` (`<a title="x>" href="javascript:...">`),
 *  so stopping early would leave the `href` outside the tag and unscrubbed.
 *
 *  A quote counts ONLY where a value is expected, directly after `=`. Treat
 *  every quote as a delimiter and an apostrophe in `<h2 id=it's>` opens one
 *  that never closes: the tag runs to the end of the document and the
 *  attribute walk scrubs plain prose. Comments are the other such door, and
 *  [`commentEnd`] handles them before this is reached.
 *
 *  Returns `html.length` for a tag that never closes, so the caller treats the
 *  whole remainder as markup and scrubs it: the scan fails CLOSED. */
function tagEnd(html: string, start: number): number {
  let i = start + 1;
  let quote = '';
  let valueExpected = false;
  while (i < html.length) {
    const ch = html[i];
    if (quote) {
      if (ch === quote) quote = '';
    } else if (valueExpected && (ch === '"' || ch === "'")) {
      quote = ch;
      valueExpected = false;
    } else if (ch === '>') {
      return i + 1;
    } else if (ch === '=') {
      valueExpected = true;
    } else if (!isHtmlSpace(ch)) {
      valueExpected = false;
    }
    i++;
  }
  return html.length;
}

/** Drop the script-bearing and dangerous-URL attributes from one tag, keeping
 *  every other byte of it exactly as it was.
 *
 *  Walks the tag's attributes rather than pattern-matching `\s+name=value`
 *  over the text, because that shape also occurs in two places it must NOT be
 *  removed from: an attribute VALUE (`data-copy-text="... on_event=X"`) and
 *  ordinary PROSE ("Set online=yes in the config"). Nothing marks such a loss,
 *  so the reader sees a mangled sentence with no sign anything was dropped.
 *
 *  Deletions are spliced out of the ORIGINAL text rather than re-emitted from
 *  parsed parts, so attribute quoting, spacing and order survive untouched and
 *  this can only ever remove. */
function scrubTagAttributes(tag: string): string {
  // Char tests, not regex literals: a literal in a loop body allocates a fresh
  // RegExp per iteration, and this walks every character of every tag on every
  // cache-missing render.
  const isSpace = isHtmlSpace;
  const endsName = (c: string): boolean => isSpace(c) || c === '=' || c === '/' || c === '>';

  // Past `<`, an optional `/`, and the tag name. Anything before the first
  // whitespace is the name, never an attribute.
  let i = 1;
  while (i < tag.length && !isSpace(tag[i]) && tag[i] !== '/' && tag[i] !== '>') i++;

  const drop: Array<[number, number]> = [];
  while (i < tag.length) {
    let nameStart = i;
    while (nameStart < tag.length && isSpace(tag[nameStart])) nameStart++;
    if (nameStart >= tag.length) break;
    if (tag[nameStart] === '>' || tag[nameStart] === '/') { i = nameStart + 1; continue; }

    let nameEnd = nameStart;
    while (nameEnd < tag.length && !endsName(tag[nameEnd])) nameEnd++;
    const name = tag.slice(nameStart, nameEnd);
    if (!name) { i = nameStart + 1; continue; }

    // An `=` (with optional surrounding whitespace) means a value follows;
    // otherwise this is a bare boolean attribute and ends at the name.
    let cursor = nameEnd;
    while (cursor < tag.length && isSpace(tag[cursor])) cursor++;
    let value = '';
    let attrEnd = nameEnd;
    if (tag[cursor] === '=') {
      cursor++;
      while (cursor < tag.length && isSpace(tag[cursor])) cursor++;
      const q = tag[cursor];
      if (q === '"' || q === "'") {
        const close = tag.indexOf(q, cursor + 1);
        // An unterminated quote runs to the end of the tag, which the tag scan
        // already extended to the end of the input. Same fail-closed direction.
        attrEnd = close === -1 ? tag.length : close + 1;
      } else {
        attrEnd = cursor;
        while (attrEnd < tag.length && !isSpace(tag[attrEnd]) && tag[attrEnd] !== '>') attrEnd++;
      }
      value = tag.slice(cursor, attrEnd);
    }

    if (EVENT_HANDLER_NAME.test(name)
      || (URL_ATTR_NAME.test(name) && isDangerousUrlAttrValue(value))) {
      // Take the leading whitespace with it so removing an attribute never
      // leaves a double space behind.
      let from = nameStart;
      while (from > 0 && isSpace(tag[from - 1])) from--;
      drop.push([from, attrEnd]);
    }
    i = attrEnd > nameStart ? attrEnd : nameStart + 1;
  }

  if (drop.length === 0) return tag;
  let out = '';
  let pos = 0;
  for (const [from, to] of drop) {
    out += tag.slice(pos, from);
    pos = to;
  }
  return out + tag.slice(pos);
}

/** Neutralize the raw HTML that marked passes through, without touching the
 *  text around it. Escapes the tags that can never be safe, then drops
 *  script-bearing / dangerous-URL attributes from the tags that remain. */
function sanitizeHtmlFragments(html: string): string {
  // The dangerous-tag pass runs first and rewrites those tags into escaped
  // TEXT, so the tag walk below never sees them and their attributes stay
  // visible as the text they have become.
  const escaped = html.replace(DANGEROUS_TAG, (match) => escapeHtmlAttr(match));
  let out = '';
  let i = 0;
  while (i < escaped.length) {
    const lt = escaped.indexOf('<', i);
    if (lt === -1) {
      out += escaped.slice(i);
      break;
    }
    out += escaped.slice(i, lt);
    // A comment BOUNDS differently from a tag (an apostrophe in it is not an
    // attribute quote), so its extent is measured separately. It is still
    // scrubbed. Inside RCDATA (`<textarea>`, `<title>`) a `<!--` is plain text
    // to the browser while this scan reads it as a comment. An unterminated
    // one would hand back every following byte with its handlers intact.
    const end = escaped.startsWith('<!--', lt) ? commentEnd(escaped, lt) : tagEnd(escaped, lt);
    const tag = escaped.slice(lt, end);
    out += scrubTagAttributes(tag);
    i = end;

    // A raw-text / RCDATA element stops the browser tokenizing tags until its
    // own end tag, so everything between is TEXT whatever it looks like. Keep
    // walking as if it were markup and the models diverge: in
    // `<textarea><a title="</textarea><img src=x onerror=...>` the browser
    // ends the textarea and the img is live, while the scan reads one `<a>`
    // whose `title` value swallows it.
    //
    // BOUNDING the region at the end tag realigns the two models. The content
    // is then scrubbed rather than copied: `title` is RCDATA in HTML but
    // ordinary markup inside `<svg>` / `<math>`, and this walk does not track
    // foreign content. Nothing skips the scrub.
    const raw = rawTextTagName(tag);
    if (raw) {
      // No end tag means the element runs to EOF, which is what the browser
      // does with it too.
      const close = closeTagIndex(escaped, raw, i);
      out += sanitizeHtmlFragments(escaped.slice(i, close));
      i = close;
    }
  }
  return out;
}

/** Elements whose content the HTML parser reads as text rather than markup, and
 *  that are NOT already escaped by [`DANGEROUS_TAG`] (`script`, `style`,
 *  `iframe`, `noembed` and friends never reach the walk as tags). */
const RAW_TEXT_TAGS = new Set(['textarea', 'title', 'xmp', 'noscript', 'plaintext']);

/** The lower-cased name of `tag` when it is a raw-text START tag, else null. */
function rawTextTagName(tag: string): string | null {
  if (!tag.startsWith('<') || tag.startsWith('</') || tag.startsWith('<!')) return null;
  let i = 1;
  while (i < tag.length && !isHtmlSpace(tag[i]) && tag[i] !== '/' && tag[i] !== '>') i++;
  const name = tag.slice(1, i).toLowerCase();
  return RAW_TEXT_TAGS.has(name) ? name : null;
}

/** Index of the `</name` end tag at or after `from`, or `html.length`.
 *
 *  Matched the way the tokenizer does: the name is case-insensitive and must be
 *  followed by whitespace, `/` or `>`, so `</textareax>` does not close a
 *  `<textarea>`. `plaintext` has no end tag at all and correctly runs to EOF. */
function closeTagIndex(html: string, name: string, from: number): number {
  const needle = `</${name}`;
  const hay = html.toLowerCase();
  let at = from;
  for (;;) {
    const found = hay.indexOf(needle, at);
    if (found === -1) return html.length;
    const after = html[found + needle.length];
    if (after === undefined || isHtmlSpace(after) || after === '/' || after === '>') return found;
    at = found + needle.length;
  }
}

/** At or above this many columns a table stacks into labeled cards on a phone
 *  (shared-components.css, `table[data-stack]`). Below it the grid is kept:
 *  two or three columns read fine with the bounded horizontal scroll. The
 *  threshold lives here because an attribute selector cannot compare a
 *  number. */
const STACK_MIN_COLUMNS = 4;

const TABLE_BLOCK = /<table>([\s\S]*?)<\/table>/g;
const TABLE_ROW = /<tr>[\s\S]*?<\/tr>/g;
const HEADER_CELL = /<th\b[^>]*>([\s\S]*?)<\/th>/g;
const BODY_CELL = /<td\b([^>]*)>([\s\S]*?)<\/td>/g;

/** Undo marked's text escaping so a header can be re-escaped for an attribute
 *  without double-escaping. `&amp;` is decoded LAST: a source `&lt;` that
 *  marked wrote as `&amp;lt;` then survives as the four characters the author
 *  typed, instead of collapsing into a `<`.
 *
 *  Deliberately NOT `decodeHtmlEntitiesForScheme` above, despite the overlap.
 *  That one leaves `lt` / `gt` / `quot` alone, which is what a header needs.
 *  Widening it would change what the `javascript:` / `data:` guard sees. */
function decodeMarkedTextEscapes(s: string): string {
  return s
    .replace(/&lt;/g, '<')
    .replace(/&gt;/g, '>')
    .replace(/&quot;/g, '"')
    .replace(/&#39;/g, "'")
    .replace(/&amp;/g, '&');
}

/** A header cell's plain text, safe to sit inside a double-quoted attribute.
 *  Inline markup (`<code>`, `<strong>`, a link) is dropped rather than
 *  rendered, since `content: attr(data-label)` can only produce a text run. */
function headerCellLabel(cellHtml: string): string {
  return escapeHtmlAttr(decodeMarkedTextEscapes(cellHtml.replace(/<[^>]*>/g, '')).trim());
}

/** Wrap every table in the scroll container that lets it pan sideways instead
 *  of squeezing its columns, and stamp the ones wide enough to stack on a
 *  phone with `data-stack` plus a per-cell `data-label` carrying its column
 *  header. GFM forbids a nested table, so the non-greedy block match is exact.
 *
 *  A string transform rather than a DOM one: this runs inline on every render
 *  of every exchange. The unit tests also run under node with stub `document`
 *  objects (src/test-setup.ts), where there is no DOMParser. */
function transformTables(html: string): string {
  return html.replace(TABLE_BLOCK, (_match, body: string) => {
    const labels = [...body.matchAll(HEADER_CELL)].map((m) => headerCellLabel(m[1]));
    if (labels.length < STACK_MIN_COLUMNS) {
      return `<div class="table-scroll-wrapper"><table>${body}</table></div>`;
    }
    // Column index resets per row, so a row that somehow carries a different
    // cell count cannot shift every later row's labels by one.
    const labelled = body.replace(TABLE_ROW, (row) => {
      let col = 0;
      return row.replace(BODY_CELL, (_cell, attrs: string, inner: string) => {
        const label = labels[col] ?? '';
        col += 1;
        return `<td${attrs} data-label="${label}">${inner}</td>`;
      });
    });
    return `<div class="table-scroll-wrapper"><table data-stack>${labelled}</table></div>`;
  });
}

/** The workspace's top-level directories, the only relative image sources that
 *  get rewritten. An allowlist rather than "every relative path": a relative
 *  src that means something else in its own context (an app's own asset) must
 *  not be silently redirected at the workspace. */
const WORKSPACE_DATA_DIRS = new Set([
  'artifacts',
  'apps',
  'knowhow',
  'triggers',
  'system-knowhow',
]);

/** The attribute-ready `src` for a workspace-relative image source, or `null`
 *  when the source is not ours to resolve.
 *
 *  Workspace files are served under the `/data` mount, so a bare
 *  `artifacts/x.png` resolves against the SPA base instead, which no route
 *  owns: the fallback answers with `index.html` and the `<img>` breaks.
 *  Building the URL through `lucidos.data.url` rather than pasting a prefix
 *  keeps that correct in every topology at once. It resolves the gateway's
 *  `/<slug>` prefix and its absence on a bare engine port, and routes
 *  `system-knowhow/` to the API endpoint serving the engine repo. */
function workspaceDataImageSrc(src: string): string | null {
  // A scheme (`https:`, `data:`, `blob:`), a protocol-relative `//host/…`, or an
  // already-absolute `/path` all address something the browser resolves without
  // help. Only a bare relative path can be a workspace file.
  if (/^[a-z][a-z0-9+.-]*:/i.test(src) || src.startsWith('/')) return null;
  // A query or fragment is not part of the file name. Splitting it off and
  // re-attaching it afterwards keeps `?v=2` a query instead of letting the path
  // encoder fold it into the name and ask for a file called `x.png%3Fv%3D2`.
  const cut = src.search(/[?#]/);
  const rawPath = cut === -1 ? src : src.slice(0, cut);
  const suffix = cut === -1 ? '' : src.slice(cut);
  // Decode before inspecting, for two reasons. marked percent-encodes the src
  // and `data.url` encodes each segment again, so a space round-trips to
  // `%2520` and misses the file. Decoding also turns an obfuscated `%2e%2e`
  // into the `..` the traversal check below can see.
  let path: string;
  try {
    path = rawPath.split('/').map(decodeURIComponent).join('/');
  } catch {
    return null; // Malformed escape: leave the source exactly as authored.
  }
  // Re-split AFTER joining, because that is the boundary `data.url`'s encoder
  // applies. A decoded segment can itself contain a separator. Check the
  // pre-join segments and `%2e%2e%2f%2e%2e` passes as one innocent-looking
  // segment. The encoder then splits it back into two real ones, which the
  // browser normalizes straight out of the mount.
  const segments = path.split('/');
  if (!WORKSPACE_DATA_DIRS.has(segments[0])) return null;
  if (segments.includes('..')) return null;
  // Only the path half is escaped for the attribute, and even there it is
  // defence in depth: every segment comes back percent-encoded. The suffix is
  // a verbatim slice of an already-escaped attribute value, so re-escaping it
  // would turn `?a=1&amp;b=2` into `&amp;amp;`.
  return `${escapeHtmlAttr(lucidos.data.url(path))}${suffix}`;
}

const IMG_TAG = /<img\b[^>]*>/gi;
/** Leading whitespace is required so `data-src="…"` is not read as `src="…"`
 *  (`-` is a word boundary, so a `\b` anchor would match inside it). */
const IMG_SRC_ATTR = /(\ssrc=")([^"]*)(")/i;

/** Point every workspace-relative image at the mount that actually serves it.
 *
 *  Runs AFTER `sanitizeHtmlFragments`, deliberately: the sanitizer decides
 *  which `src` attributes exist at all, deleting `javascript:` and `data:`
 *  ones outright. So only an attribute that already passed that gate is ever
 *  rewritten, and the sanitizer is never handed a value to re-judge. */
function rewriteImageSources(html: string): string {
  return html.replace(IMG_TAG, (tag) =>
    tag.replace(IMG_SRC_ATTR, (whole, open: string, src: string, close: string) => {
      const rewritten = workspaceDataImageSrc(src);
      return rewritten === null ? whole : `${open}${rewritten}${close}`;
    })
  );
}

/** Wrap every image in the scroll container that lets an oversized screenshot
 *  pan sideways instead of widening the pane (`.image-scroll-wrapper` in
 *  shared-components.css carries the sizing and the cap).
 *
 *  A `<span>` rather than the table wrapper's `<div>`, because marked puts an
 *  image inside a `<p>` and a `<div>` there is invalid: the parser closes the
 *  paragraph at it, stranding the prose that followed. The CSS gives the span
 *  `display: block`, so the layout is the table wrapper's regardless.
 *
 *  A string transform rather than a DOM one, for the same reason as
 *  `transformTables`. */
function wrapImages(html: string): string {
  return html.replace(IMG_TAG, (tag) => `<span class="image-scroll-wrapper">${tag}</span>`);
}

// LRU cache for parsed markdown. `renderMarkdown` is pure, but the chat
// timeline calls it INLINE on every render of every exchange, so one thread
// re-render re-parses every block synchronously. On a heavy thread that storm
// freezes the main thread for a second or two. The live streaming buffer opts
// out (`cache: false`): its text changes every token, so caching it would only
// evict the stable, reused entries.
const MARKDOWN_CACHE_MAX = 400;
const markdownCache = new Map<string, string>();

export function renderMarkdown(md: string, opts?: { cache?: boolean }): string {
  const useCache = opts?.cache !== false;
  if (useCache) {
    const hit = markdownCache.get(md);
    if (hit !== undefined) {
      // LRU touch: move to most-recently-used.
      markdownCache.delete(md);
      markdownCache.set(md, hit);
      return hit;
    }
  }
  // Time only the real parse path, so the recorded parse share is not inflated
  // by O(1) cache hits. try/finally records elapsed even if marked.parse
  // throws. See utils/renderPhaseTimers.ts.
  const parseStart = performance.now();
  try {
    const encodedTexts = new Map<number, string>();
    const preprocessed = preprocessCopyBlocks(md, encodedTexts);
    let html = marked.parse(preprocessed, { async: false }) as string;
    html = postprocessCopyBlocks(html, encodedTexts);
    html = sanitizeHtmlFragments(html);
    html = rewriteImageSources(html);
    html = wrapImages(html);
    html = transformTables(html);
    // The workspace-qualified form is what the copy-ref button emits.
    html = html.replace(
      /href="thread:(?:([a-zA-Z0-9_-]+)\/)?([0-9a-f-]+)"/g,
      (_match, workspace: string | undefined, threadId: string) => {
        const wsAttr = workspace ? ` data-thread-workspace="${escapeHtmlAttr(workspace)}"` : '';
        const href = escapeHtmlAttr(threadLinkHref(workspace, threadId));
        return `href="${href}" data-thread-id="${threadId}"${wsAttr} class="thread-link"`;
      }
    );
    if (useCache) {
      markdownCache.set(md, html);
      if (markdownCache.size > MARKDOWN_CACHE_MAX) {
        // Evict the least-recently-used (first key in insertion order).
        const oldest = markdownCache.keys().next().value;
        if (oldest !== undefined) markdownCache.delete(oldest);
      }
    }
    return html;
  } finally {
    addMarkdownParseMs(performance.now() - parseStart);
  }
}

/** Phrasing-content-only variant of renderMarkdown, safe to nest inside
 *  elements that forbid flow content or interactive descendants (`<button>`,
 *  `<span>`). Inline markdown renders, block constructs appear as their
 *  literal source text, and links render as label text only.
 *
 *  `breaks: true` is passed locally so a future edit to markedConfig.ts's
 *  global options cannot silently turn newlines back into spaces.
 *
 *  `parseInline` DOES emit `<img>`, so a workspace-relative source is
 *  rewritten here too. The `.image-scroll-wrapper` is NOT applied: it is a
 *  block box, invalid inside the phrasing content these helpers exist to
 *  emit. */
export function renderMarkdownInline(md: string): string {
  return rewriteImageSources(sanitizeHtmlFragments(marked.parseInline(md, {
    async: false,
    breaks: true,
    renderer: inlineLinkStripRenderer,
  }) as string));
}

/** Like renderMarkdownInline, but KEEPS http(s) links as clickable
 *  `<a target="_blank" rel="noopener">`. Used for the AskUserQuestion question
 *  text, where a pasted bare URL must stay openable.
 *
 *  Safe ONLY in non-interactive containers: an `<a>` inside a `<button>` is
 *  invalid, so option buttons must keep `renderMarkdownInline`. */
export function renderMarkdownInlineWithLinks(md: string): string {
  return rewriteImageSources(sanitizeHtmlFragments(marked.parseInline(md, {
    async: false,
    breaks: true,
    gfm: true,
    renderer: inlineLinkKeepRenderer,
  }) as string));
}
