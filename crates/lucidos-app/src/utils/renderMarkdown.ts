import { marked } from 'marked';
import type { Tokens } from 'marked';
import { lucidos } from '@lucidos/sdk';
import { COPY_ICON, escapeHtmlAttr } from './markedConfig';
import { addMarkdownParseMs } from './renderPhaseTimers';
import { WORKSPACE_ID } from './basePath';
import { slugifyWorkspaceName } from './slug';

/** Real destination for a thread link, so HOVERING shows where it goes instead
 *  of the `#`-resolves-to-the-current-page URL (the confusing `…/<current>/#`).
 *  Behind the gateway every workspace is same-origin under `/<slug>/`, so we point
 *  straight at `/<slug>/#thread=<id>` — slug from the ref's workspace (slugified,
 *  exact when name === slug, the common case), or the current workspace for an
 *  untagged / same-workspace link. The left-click is still intercepted by the
 *  global `.thread-link` handler (`useStartup`), which does the authoritative
 *  routing; this href is for the hover tooltip, middle/⌘-click, and accessibility.
 *  Served directly on an engine port (no gateway, `WORKSPACE_ID` null) we can't
 *  build a peer URL synchronously, so we keep `#` and let the handler route. */
function threadLinkHref(workspace: string | undefined, threadId: string): string {
  if (WORKSPACE_ID === null || typeof location === 'undefined') return '#';
  const slug = workspace ? slugifyWorkspaceName(workspace) : WORKSPACE_ID;
  return `${location.origin}/${encodeURIComponent(slug)}/#thread=${threadId}`;
}

// Inline renderer for renderMarkdownInline — overrides `link` to drop the
// <a> wrapper while preserving any nested inline markdown (e.g. **bold**
// inside link text). Two reasons: (1) the helper's outputs nest inside
// <button>, where <a> is an invalid interactive-in-interactive descendant;
// (2) discarding href also neutralizes javascript:-scheme URLs from LLM-
// supplied text.
const inlineLinkStripRenderer = new marked.Renderer();
inlineLinkStripRenderer.link = function({ tokens }: Tokens.Link): string {
  return this.parser.parseInline(tokens);
};

// Inline renderer for renderMarkdownInlineWithLinks — KEEPS http(s) links as
// real <a> elements (covers both `[label](url)` and gfm-autolinked bare URLs),
// forcing safe new-tab attributes. Any non-http(s) href (javascript:, data:,
// mailto:, relative, app:, …) collapses to its label text, so an LLM-supplied
// scheme can neither execute nor dead-end. Only valid in NON-interactive
// containers — an <a> nested in a <button> is interactive-in-interactive, so
// option buttons keep inlineLinkStripRenderer.
const inlineLinkKeepRenderer = new marked.Renderer();
inlineLinkKeepRenderer.link = function({ href, tokens }: Tokens.Link): string {
  const text = this.parser.parseInline(tokens);
  if (!/^https?:\/\//i.test(href)) return text;
  return `<a href="${escapeHtmlAttr(href)}" target="_blank" rel="noopener">${text}</a>`;
};

// Unique marker prefix for copy block boundaries (survives marked processing)
const COPY_MARKER = 'LUCIDOS_COPY_BLOCK';
const COPY_MARKER_PATTERN = new RegExp(
  `<!--${COPY_MARKER}_START_(\\d+)-->([\\s\\S]*?)<!--${COPY_MARKER}_END_\\1-->`,
  'g',
);

// Matches fenced code blocks and inline code spans for protection during copy block preprocessing
const CODE_PROTECTION_PATTERN = /```[\s\S]*?```|`[^`\n]+`/g;

/**
 * Convert <copy>...</copy> tags to copyable UI blocks.
 *
 * Inline (single-line): wraps directly in a <span> before marked — works fine
 * because there are no blank lines to break the HTML block.
 *
 * Multiline: uses HTML comment markers that survive marked processing, then
 * postprocessCopyBlocks wraps the rendered content. This avoids CommonMark's
 * HTML block rule that ends a <div> at the first blank line.
 */
function preprocessCopyBlocks(md: string, encodedTexts: Map<number, string>): string {
  // Protect fenced code blocks and inline code spans from copy tag matching.
  // Replace them with placeholders, process copy blocks, then restore.
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

    // Multiline: stash encoded text, emit comment markers around raw markdown.
    const id = counter++;
    encodedTexts.set(id, restoredEncoded);
    return `<!--${COPY_MARKER}_START_${id}-->\n\n${trimmed}\n\n<!--${COPY_MARKER}_END_${id}-->`;
  });

  // Restore protected code blocks
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

// Escape dangerous HTML elements that survive marked processing.
// Raw HTML in markdown source (e.g., user typing "<iframe>") passes through
// marked unescaped and renders as actual elements — blank boxes, XSS vectors, etc.
// Copy block elements (<span>, <button>, <svg>, <div>) and marked-generated
// elements (<p>, <strong>, <code>, etc.) are NOT in this list and pass through.
// `animate` / `animateTransform` / `set` are here rather than in the URL-attribute
// filter because they reach a URL by INDIRECTION: `<animate attributeName="href"
// values="javascript:…">` animates a sibling `<a>`'s href to a scheme this file
// exists to strip, and a name-based attribute filter cannot see it (the names
// present are `attributeName` and `values`). Escaping the element is the only
// check that holds.
const DANGEROUS_TAG =
  /<(\/?)(iframe|script|style|object|embed|applet|base|meta|link|animate|animateTransform|set)(\s[^>]*)?>/gi;
/** An attribute NAME that carries executable script. */
const EVENT_HANDLER_NAME = /^on[a-z0-9_-]+$/i;
/** The attribute NAMES whose value is fetched/navigated as a URL.
 *
 *  `xlink:href` is the SVG spelling of `href` and navigates identically
 *  (`<svg><a xlink:href="javascript:…">`), so leaving it out meant the two
 *  spellings of one attribute disagreed. `action` / `formaction` submit to their
 *  value and take a `javascript:` URL the same way. Only a dangerous SCHEME is
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
 *  `<!--->` are COMPLETE empty comments: starting later would miss their
 *  terminator, swallow the rest of the document as comment text and hand it back
 *  unscrubbed. Both spec terminators are accepted, and the earlier one wins, for
 *  the same reason. An unterminated comment runs to the end, which matches what
 *  the browser does with it. */
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
 *  so stopping early would leave the `href` outside the tag and therefore
 *  unscrubbed.
 *
 *  A quote counts ONLY where a value is expected, i.e. directly after `=` (any
 *  whitespace between them). Treating every quote as a delimiter is what
 *  reopened the prose-deletion bug through a second door: an apostrophe in
 *  `<h2 id=it's>` opened a quote that never closed, the tag ran to the end of
 *  the document, and the attribute walk then scrubbed plain prose. Comments are
 *  the other door and are handled by [`commentEnd`] before this is reached.
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
 *  Walks the tag's attributes rather than pattern-matching `\s+name=value` over
 *  the text, because that shape occurs in two places it must NOT be removed
 *  from. In an attribute VALUE: `<span data-copy-text="lucidos trigger set
 *  on_event=X">` lost the `on_event=X` out of the clipboard payload. And, once
 *  the same regexes were run over the whole rendered document, in ordinary
 *  PROSE: "Set online=yes in the config" rendered as "Set in the config", and
 *  "Values: on_error=retry, on_success=stop." rendered as "Values:". Nothing
 *  marked either loss, so the reader saw a mangled sentence with no sign that
 *  anything had been dropped.
 *
 *  Deletions are spliced out of the ORIGINAL text rather than re-emitted from
 *  parsed parts, so attribute quoting, spacing and order survive untouched and
 *  this can only ever remove. */
function scrubTagAttributes(tag: string): string {
  // Char tests, not regex literals: this walks every character of every tag on
  // every cache-missing render, and a literal in a loop body allocates a fresh
  // RegExp per iteration. Both live at module scope for the same reason.
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
    // scrubbed, though: passing it through verbatim was an escape hatch, because
    // inside RCDATA (`<textarea>`, `<title>`) a `<!--` is plain text to the
    // browser while this scan reads it as a comment, so an unterminated one
    // handed back every following byte with its handlers intact. Scrubbing the
    // region costs nothing real, since a comment body never renders.
    const end = escaped.startsWith('<!--', lt) ? commentEnd(escaped, lt) : tagEnd(escaped, lt);
    const tag = escaped.slice(lt, end);
    out += scrubTagAttributes(tag);
    i = end;

    // A raw-text / RCDATA element stops the browser tokenizing tags until its
    // own end tag, so everything between is TEXT no matter what it looks like.
    // Keep walking as if it were markup and the models diverge: in
    // `<textarea><a title="</textarea><img src=x onerror=…>` the browser ends
    // the textarea at `</textarea>` and the img is a live element, while the
    // scan reads one `<a>` whose `title` value swallows the img, finds no
    // attribute named `onerror`, and hands the whole thing back untouched.
    //
    // BOUNDING the region at the end tag is what realigns the two models, and
    // it is the whole fix. The content is then scrubbed rather than copied,
    // even though the browser reads it as text: `title` is RCDATA in HTML but
    // ordinary markup inside `<svg>` / `<math>`, and this walk does not track
    // foreign content, so copying would be right for one and wrong for the
    // other. Scrubbing is right for both, and the only cost is that literal
    // markup a reader typed into a textarea loses its handler attributes when
    // displayed. Passing a region through untouched is what opened this hole
    // and the comment hole before it; the rule now is that nothing skips the
    // scrub.
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
 *  two or three columns read fine with the bounded horizontal scroll, and
 *  stacking them would cost vertical space for nothing. The threshold lives
 *  here rather than in CSS because an attribute selector cannot compare a
 *  number, and because it is testable at this layer. */
const STACK_MIN_COLUMNS = 4;

const TABLE_BLOCK = /<table>([\s\S]*?)<\/table>/g;
const TABLE_ROW = /<tr>[\s\S]*?<\/tr>/g;
const HEADER_CELL = /<th\b[^>]*>([\s\S]*?)<\/th>/g;
const BODY_CELL = /<td\b([^>]*)>([\s\S]*?)<\/td>/g;

/** Undo marked's text escaping so a header can be re-escaped for an attribute
 *  without double-escaping (`&amp;` must not reach the DOM as `&amp;amp;` and
 *  render as the literal text "&amp;"). `&amp;` is decoded LAST, so a source
 *  `&lt;` that marked wrote as `&amp;lt;` survives as the four characters the
 *  author typed instead of collapsing into a `<`.
 *
 *  Deliberately NOT `decodeHtmlEntitiesForScheme` above, despite the overlap.
 *  That one exists to defeat scheme obfuscation, so it decodes numeric entities
 *  plus a narrow named set (`colon`, `tab`, `newline`, `amp`) chosen for that
 *  job, and it leaves `lt`/`gt`/`quot` alone: exactly the ones a header needs.
 *  Widening it would change what the `javascript:` / `data:` guard sees, which
 *  is not a change to make on behalf of a label. */
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
 *  A string transform rather than a DOM one on purpose: this runs inline on
 *  every render of every exchange, and the unit tests run under node with stub
 *  `document` objects (src/test-setup.ts), where there is no DOMParser. */
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
 *  get rewritten. An allowlist rather than "every relative path" on purpose: a
 *  relative src that means something else in its own context (an app's own
 *  asset, a path the author expects to resolve against the page) must not be
 *  silently redirected at the workspace. */
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
 *  `artifacts/x.png` resolves against the SPA base instead and asks for
 *  `/<slug>/artifacts/x.png`, which no route owns: the SPA fallback answers it
 *  with `index.html`, and the `<img>` shows the broken-image icon. Building the
 *  URL through `lucidos.data.url` rather than pasting a prefix is what keeps
 *  that correct in every topology at once, since it already resolves the
 *  gateway's `/<slug>` prefix (and its absence on a bare engine port) and routes
 *  `system-knowhow/` to the API endpoint that serves it from the engine repo
 *  rather than from the workspace. */
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
  // Decode before inspecting, for two reasons. marked percent-encodes the src it
  // emits and `data.url` encodes each segment again, so a space would round-trip
  // to `%2520` and miss the file. And decoding is what turns an obfuscated
  // `%2e%2e` into the `..` the traversal check below can actually see.
  let path: string;
  try {
    path = rawPath.split('/').map(decodeURIComponent).join('/');
  } catch {
    return null; // Malformed escape: leave the source exactly as authored.
  }
  // Re-split AFTER joining, because that is the boundary `data.url`'s encoder
  // will apply and the only one that matters. A decoded segment can itself
  // contain a separator, so checking the pre-join segments would let
  // `%2e%2e%2f%2e%2e` through as one innocent-looking `../..` segment that the
  // encoder then splits back into two real ones (`.` is unreserved, so nothing
  // downstream re-escapes them) and the browser normalizes straight out of the
  // mount.
  const segments = path.split('/');
  if (!WORKSPACE_DATA_DIRS.has(segments[0])) return null;
  if (segments.includes('..')) return null;
  // Only the path half is escaped for the attribute, and even there it is
  // defence in depth: every segment comes back percent-encoded. The suffix is
  // NOT re-escaped, because it is a verbatim slice of an attribute value that
  // is already escaped, so an author's `?a=1&amp;b=2` would become
  // `&amp;amp;` and reach the server as that literal text.
  return `${escapeHtmlAttr(lucidos.data.url(path))}${suffix}`;
}

const IMG_TAG = /<img\b[^>]*>/gi;
/** Leading whitespace is required so `data-src="…"` is not read as `src="…"`
 *  (`-` is a word boundary, so a `\b` anchor would match inside it). */
const IMG_SRC_ATTR = /(\ssrc=")([^"]*)(")/i;

/** Point every workspace-relative image at the mount that actually serves it.
 *
 *  Runs AFTER `sanitizeHtmlFragments`, deliberately: the sanitizer is what
 *  decides which `src` attributes exist at all (it deletes `javascript:` and
 *  `data:` ones outright), so running after it means only an attribute that
 *  already passed that gate is ever rewritten, and the sanitizer is never handed
 *  a value it would have to re-judge. Nothing about `data:` image URIs changes
 *  either way: they are stripped today and still are, and an allowlisted
 *  workspace path can never carry a scheme for this to reinstate one. */
function rewriteImageSources(html: string): string {
  return html.replace(IMG_TAG, (tag) =>
    tag.replace(IMG_SRC_ATTR, (whole, open: string, src: string, close: string) => {
      const rewritten = workspaceDataImageSrc(src);
      return rewritten === null ? whole : `${open}${rewritten}${close}`;
    })
  );
}

/** Wrap every image in the scroll container that lets an oversized screenshot
 *  pan sideways instead of widening the pane, the same treatment
 *  `transformTables` gives a table (`.image-scroll-wrapper` in
 *  shared-components.css carries the sizing and the cap).
 *
 *  A `<span>` rather than the table wrapper's `<div>`, because marked puts an
 *  image inside a `<p>` and a `<div>` there is invalid: the HTML parser closes
 *  the paragraph at it, stranding whatever prose followed the image outside the
 *  paragraph. The CSS gives the span `display: block`, so the layout is the
 *  table wrapper's regardless.
 *
 *  Independent of `transformTables` in both directions (neither transform's
 *  pattern can match the other's output), so the order between them is free;
 *  images run first so the table pass copies cell content that is already final.
 *
 *  A string transform rather than a DOM one for the same reason as
 *  `transformTables`: this runs inline on every render of every exchange, and
 *  the unit tests run under node with stub `document` objects
 *  (src/test-setup.ts), where there is no DOMParser. */
function wrapImages(html: string): string {
  return html.replace(IMG_TAG, (tag) => `<span class="image-scroll-wrapper">${tag}</span>`);
}

// LRU cache for parsed markdown. `renderMarkdown` is pure (same input → same
// HTML), but the chat timeline calls it INLINE on every render of every exchange
// (response text, each tool result, each thought, prompts). A re-render of a
// thread — e.g. the thread flipping idle→running when a follow-up is sent busts
// the per-exchange memo for ALL exchanges — therefore re-parsed every block's
// markdown synchronously. On a heavy thread (dozens of tool results + thoughts)
// that storm froze the main thread for a second or two. Caching by raw input
// makes those re-parses O(1) lookups. The live streaming buffer opts out
// (`cache: false`) — its text changes every token, so caching it would only
// thrash the cache and evict the stable, reused entries.
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
  // Perf: time only the real parse path (cache MISS, or cache:false streaming) —
  // cache hits returned above add nothing, so the parse share isn't inflated by
  // O(1) lookups. try/finally records elapsed even if marked.parse throws, and
  // re-throws unchanged. See utils/renderPhaseTimers.ts. Fire-and-forget.
  const parseStart = performance.now();
  try {
    // Preprocess copy blocks before marked parsing
    const encodedTexts = new Map<number, string>();
    const preprocessed = preprocessCopyBlocks(md, encodedTexts);
    let html = marked.parse(preprocessed, { async: false }) as string;
    // Wrap multiline copy blocks that used comment markers
    html = postprocessCopyBlocks(html, encodedTexts);
    // Escape dangerous HTML elements from raw markdown source
    html = sanitizeHtmlFragments(html);
    // Resolve workspace-relative image sources against the mount that serves
    // them, then give each image the scroll container that keeps an oversized
    // one from widening the pane
    html = rewriteImageSources(html);
    html = wrapImages(html);
    // Wrap tables in a scrollable container so columns auto-size naturally,
    // and mark the wide ones for the stacked mobile layout
    html = transformTables(html);
    // Convert thread: links to clickable thread navigation. Accepts both the
    // bare-UUID form (`thread:UUID`, same workspace) and the workspace-qualified
    // form emitted by the copy-ref button (`thread:workspace/UUID`).
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

/** Phrasing-content-only variant of renderMarkdown — wraps `marked.parseInline`
 *  so the output is safe to nest inside elements that forbid flow content
 *  *or interactive descendants* (e.g. `<button>` or `<span>`). Inline markdown
 *  (bold, italic, code, soft breaks) renders; block constructs (paragraphs,
 *  lists, code fences, tables) appear as their literal source text; markdown
 *  links render as their label text only — see `inlineLinkStripRenderer`.
 *  Use this for short LLM-supplied snippets like AskUserQuestion option
 *  descriptions.
 *
 *  `breaks: true` is passed locally so a future edit to markedConfig.ts's
 *  global options can't silently turn newlines back into spaces.
 *
 *  `parseInline` DOES emit `<img>` for `![alt](src)`, so a workspace-relative
 *  image source is rewritten here too: an AskUserQuestion option can carry one,
 *  and it breaks in exactly the same way. The `.image-scroll-wrapper` is NOT
 *  applied, though. It is a block scroll container, and these helpers exist to
 *  emit phrasing content that nests inside a `<button>` or a `<span>`, where a
 *  block box is invalid and would break the line mid-sentence. An image in a
 *  short option label is small by nature, so there is nothing to pan. */
export function renderMarkdownInline(md: string): string {
  return rewriteImageSources(sanitizeHtmlFragments(marked.parseInline(md, {
    async: false,
    breaks: true,
    renderer: inlineLinkStripRenderer,
  }) as string));
}

/** Like renderMarkdownInline (phrasing content only — no block wrappers) but
 *  KEEPS http(s) links as clickable `<a target="_blank" rel="noopener">`. Used
 *  for the AskUserQuestion *question text*, where the LLM commonly pastes a bare
 *  URL the user needs to open — `renderMarkdownInline` would flatten it to dead
 *  text. Bare URLs are autolinked (gfm) and `[label](url)` links survive;
 *  non-http(s) schemes collapse to label text — see `inlineLinkKeepRenderer`.
 *  Safe ONLY in non-interactive containers: an `<a>` inside a `<button>` is
 *  invalid, so option buttons must keep `renderMarkdownInline`. Images get the
 *  same source rewrite and the same no-wrapper treatment as there. */
export function renderMarkdownInlineWithLinks(md: string): string {
  return rewriteImageSources(sanitizeHtmlFragments(marked.parseInline(md, {
    async: false,
    breaks: true,
    gfm: true,
    renderer: inlineLinkKeepRenderer,
  }) as string));
}
