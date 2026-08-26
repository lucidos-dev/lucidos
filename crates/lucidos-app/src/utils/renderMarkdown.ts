import { marked } from 'marked';
import type { Tokens } from 'marked';
import DOMPurify from 'dompurify';
import { lucidos } from '@lucidos/sdk';
import { COPY_ICON, escapeHtmlAttr } from './markedConfig';
import { addMarkdownParseMs } from './renderPhaseTimers';
import { WORKSPACE_ID } from './basePath';
import { DATA_PATH_PREFIXES } from './linkifyPaths';
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

// Raw HTML in markdown source passes through marked unescaped. So the string
// reaching the sanitizer mixes the renderer's own markup with whatever the
// author typed. DOMPurify parses that with the browser's own HTML parser and
// keeps an allowlist of elements and attributes. Nothing here re-implements
// that judgment; the two pieces below only add policy DOMPurify has no knob for.

/** Tags rewritten to visible text BEFORE the parser sees them.
 *
 *  DOMPurify deletes these silently. A chat transcript that quietly loses what
 *  the model wrote is the worse default, so they are escaped instead.
 *
 *  The pass cannot create markup: escaping only turns `<`, `>`, `&` and `"`
 *  into entities. A tag it misses is still removed by DOMPurify. So this is a
 *  rendering choice, and the security decision stays DOMPurify's.
 *
 *  It runs before the parse rather than after. An unclosed `<iframe>` swallows
 *  the rest of the document as raw text, and no hook on the parsed tree can
 *  give that back.
 *
 *  `animate` / `animateTransform` / `set` reach a URL by INDIRECTION, which a
 *  name-based attribute filter cannot see: `<animate attributeName="href"
 *  values="javascript:...">` animates a sibling `<a>`'s href.
 *
 *  `title`, `desc` and the MathML text elements stay out: each is real markup
 *  inside `<svg>` or `<math>`, which this regex cannot see. */
const ESCAPE_TO_TEXT_TAG =
  /<(\/?)(iframe|script|style|object|embed|applet|base|meta|link|xmp|plaintext|noscript|noembed|noframes|animate|animateTransform|set)([\s/][^>]*)?>/gi;

/** The attribute NAMES whose value is fetched or navigated as a URL.
 *
 *  DOMPurify checks the scheme of the attributes it knows carry one. But its
 *  `DATA_URI_TAGS` lets `data:` through on `<img>` and friends, and offers no
 *  negative form. The hook below closes that. It reads the value the DOM
 *  already decoded, so an entity-obfuscated scheme is plain text by the time it
 *  is compared. */
const URL_ATTRIBUTES = [
  'href', 'xlink:href', 'src', 'srcset', 'action',
  'formaction', 'poster', 'background', 'ping', 'data',
];

/** Strip `javascript:` and `data:` from every URL-bearing attribute.
 *
 *  Registered through `installUrlSchemeHook` below, never at module scope. */
function stripDangerousUrlSchemes(node: Node): void {
  const el = node as Element;
  if (typeof el.getAttribute !== 'function') return;
  for (const name of URL_ATTRIBUTES) {
    const value = el.getAttribute(name);
    if (value === null) continue;
    // Control characters are stripped the way a URL parser skips them, so
    // `java\tscript:` is caught with the plain spelling.
    const scheme = value.replace(/[\u0000-\u0020]+/g, '').toLowerCase();
    if (scheme.startsWith('javascript:') || scheme.startsWith('data:')) {
      el.removeAttribute(name);
    }
  }
}

let urlSchemeHookInstalled = false;

/** Register the URL-scheme hook, once.
 *
 *  It is registered on first use, not at module scope. With no DOM, DOMPurify's
 *  export carries no `addHook` at all, so a module-scope call throws at IMPORT
 *  time. That breaks every module that merely imports this one, including the
 *  many that never render markdown. */
function installUrlSchemeHook(): void {
  if (urlSchemeHookInstalled) return;
  DOMPurify.addHook('afterSanitizeAttributes', stripDangerousUrlSchemes);
  urlSchemeHookInstalled = true;
}

/** DOMPurify's default scheme list plus the five Lucidos schemes.
 *
 *  Each of the five is claimed by an extractor that runs AFTER sanitization:
 *  `thread:` below in this file, and `app:`, `trigger:`, `repo:` and `file:` in
 *  `linkifyPaths`. Stripping one here would break every such link, because the
 *  extractor would find no href left to read. */
const ALLOWED_URI_REGEXP =
  /^(?:(?:(?:f|ht)tps?|mailto|tel|callto|sms|cid|xmpp|matrix|thread|app|trigger|repo|file):|[^a-z]|[a-z+.\-]+(?:[^a-z+.\-:]|$))/i;

const PURIFY_CONFIG = {
  // `inlineLinkKeepRenderer` emits `target="_blank"`, which the default
  // attribute allowlist drops.
  ADD_ATTR: ['target'],
  // Paired with `ESCAPE_TO_TEXT_TAG`, so a tag the escape pass misses is still
  // removed. `style` and `animateTransform` are the two DOMPurify would
  // otherwise keep. `animate` and `set` are already in its SVG denylist, and
  // stay listed so this policy does not rest on that default.
  FORBID_TAGS: ['style', 'animate', 'animateTransform', 'set'],
  ALLOWED_URI_REGEXP,
};

/** Neutralize the raw HTML that marked passes through, without touching the
 *  text around it.
 *
 *  Exported for the one caller that drives `marked` itself instead of going
 *  through the helpers below: `components/files/RenderedDiff.tsx` parses token
 *  by token so it can mark each block, and owes its output the same scrub. Any
 *  new `marked.parse*` call site owes it too. */
export function sanitizeHtmlFragments(html: string): string {
  // With no DOM, DOMPurify's export carries no `sanitize`, so the call below
  // would die on a bare TypeError. Say what is actually wrong instead.
  if (!DOMPurify.isSupported) {
    throw new Error(
      'sanitizeHtmlFragments needs a DOM: add "// @vitest-environment jsdom" to this test file',
    );
  }
  installUrlSchemeHook();
  return DOMPurify.sanitize(html.replace(ESCAPE_TO_TEXT_TAG, escapeHtmlAttr), PURIFY_CONFIG);
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
 *  Nothing security-sensitive reads this. It runs on a header cell that is
 *  already sanitized, and its output goes straight back into an attribute
 *  through `escapeHtmlAttr`. */
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
 *  not be silently redirected at the workspace.
 *
 *  Derived from `DATA_PATH_PREFIXES`, the single source of truth for the same
 *  list. A hand-kept copy gave a new sub-tree its links but not its images,
 *  and that miss shows up as an `<img>` served the SPA fallback. */
const WORKSPACE_DATA_DIRS = new Set(DATA_PATH_PREFIXES.map((p) => p.slice(0, -1)));

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
