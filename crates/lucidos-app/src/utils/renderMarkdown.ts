import { marked } from 'marked';
import type { Tokens } from 'marked';
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
const DANGEROUS_TAG = /<(\/?)(iframe|script|style|object|embed|applet|base|meta|link)(\s[^>]*)?>/gi;
const EVENT_HANDLER_ATTR = /\s+on[a-z0-9_-]+\s*=\s*(?:"[^"]*"|'[^']*'|[^\s"'=<>`]+)/gi;
const URL_ATTR = /\s+(href|src)\s*=\s*("[^"]*"|'[^']*'|[^\s"'=<>`]+)/gi;

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

function sanitizeHtmlFragments(html: string): string {
  return html
    .replace(DANGEROUS_TAG, (match) => escapeHtmlAttr(match))
    .replace(EVENT_HANDLER_ATTR, '')
    .replace(URL_ATTR, (match, _name: string, value: string) =>
      isDangerousUrlAttrValue(value) ? '' : match
    );
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
    // Wrap tables in a scrollable container so columns auto-size naturally
    html = html.replace(/<table>/g, '<div class="table-scroll-wrapper"><table>');
    html = html.replace(/<\/table>/g, '</table></div>');
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
 *  global options can't silently turn newlines back into spaces. */
export function renderMarkdownInline(md: string): string {
  return sanitizeHtmlFragments(marked.parseInline(md, {
    async: false,
    breaks: true,
    renderer: inlineLinkStripRenderer,
  }) as string);
}

/** Like renderMarkdownInline (phrasing content only — no block wrappers) but
 *  KEEPS http(s) links as clickable `<a target="_blank" rel="noopener">`. Used
 *  for the AskUserQuestion *question text*, where the LLM commonly pastes a bare
 *  URL the user needs to open — `renderMarkdownInline` would flatten it to dead
 *  text. Bare URLs are autolinked (gfm) and `[label](url)` links survive;
 *  non-http(s) schemes collapse to label text — see `inlineLinkKeepRenderer`.
 *  Safe ONLY in non-interactive containers: an `<a>` inside a `<button>` is
 *  invalid, so option buttons must keep `renderMarkdownInline`. */
export function renderMarkdownInlineWithLinks(md: string): string {
  return sanitizeHtmlFragments(marked.parseInline(md, {
    async: false,
    breaks: true,
    gfm: true,
    renderer: inlineLinkKeepRenderer,
  }) as string);
}
