import { marked } from 'marked';
import type { Tokens } from 'marked';
import DOMPurify from 'dompurify';
import { lucidos } from '@lucidos/sdk';
import { BARE_EMAIL_ATTR, CODE_COPY_ATTR, COPY_ICON, COPY_ID_NONCE, escapeHtmlAttr } from './markedConfig';
import { makeInertBody } from './escapeHtml';
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
 *  (`startClient`), which does the authoritative routing. This href is for the
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
// It carries `COPY_ID_NONCE`, because content can write an HTML comment too.
const COPY_MARKER = `LUCIDOS_COPY_BLOCK_${COPY_ID_NONCE}`;
const COPY_MARKER_PATTERN = new RegExp(
  `<!--${COPY_MARKER}_START_(\\d+)-->([\\s\\S]*?)<!--${COPY_MARKER}_END_\\1-->`,
  'g',
);

const CODE_PROTECTION_PATTERN = /```[\s\S]*?```|~~~[\s\S]*?~~~|`[^`\n]+`/g;

/** A label that could open a link reference definition, in any container.
 *  A definition renders as nothing, so in a copy label it would hide a line. */
const LINK_DEFINITION_LABEL = /\[([^\]]*)\]:/g;

/** Attribute carrying a copy block's slot while the markup crosses the
 *  sanitizer. `resolveCopyTargets` writes the real payload attribute
 *  afterwards, in the DOM.
 *
 *  Raw HTML in markdown source reaches DOMPurify, which keeps `class` and every
 *  `data-*` by default. Model output could therefore write
 *  `class="copyable-block" data-copy-text="…"` itself, and the click handler in
 *  `startClient` hands that value straight to the clipboard. The user then
 *  pastes a command they never read. `data-copy-text` is in `FORBID_ATTR`, so
 *  content cannot write the payload. */
const COPY_ID_ATTR = 'data-copy-id';

// Every slot id carries `COPY_ID_NONCE` (markedConfig.ts), so content has no id
// to forge. The nonce never reaches the DOM: `resolveCopyTargets` removes the
// attribute it rides on, and `resolveCodeBlockCopy` removes the code one.

/** The slot a `data-copy-id` names, or `undefined` when content wrote it. */
function copyTextForSlot(attr: string | null, copyTexts: Map<number, string>): string | undefined {
  const prefix = `${COPY_ID_NONCE}-`;
  if (attr === null || !attr.startsWith(prefix)) return undefined;
  return copyTexts.get(Number(attr.slice(prefix.length)));
}

/**
 * Convert <copy>...</copy> tags to copyable UI blocks.
 *
 * A single-line tag wraps directly in a <span> before marked. A multiline one
 * uses HTML comment markers that survive marked, which postprocessCopyBlocks
 * then wraps: CommonMark's HTML block rule ends a <div> at the first blank
 * line.
 *
 * Both shapes carry a slot id rather than the text: see `COPY_ID_ATTR`.
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
    const isMultiline = trimmed.includes('\n');

    // Restore any protected code spans in the copy text so backticks are
    // preserved. Stored RAW: `setAttribute` in `resolveCopyTargets` escapes it.
    const restored = trimmed.replace(/\x00CODE(\d+)\x00/g, (_, idx) =>
      codeSlots[parseInt(idx, 10)],
    );

    const id = counter++;
    encodedTexts.set(id, restored);

    // The payload is the source text, so the label shows that text too. Raw
    // HTML or a link definition in it would hide part of what gets copied.
    const label = escapeHtmlAttr(trimmed).replace(LINK_DEFINITION_LABEL, '&#91;$1]:');

    if (!isMultiline) {
      return `<span class="copyable-block" ${COPY_ID_ATTR}="${COPY_ID_NONCE}-${id}">` +
        label +
        `<button type="button" class="copy-btn" aria-label="Copy to clipboard">${COPY_ICON}</button>` +
        `</span>`;
    }

    return `<!--${COPY_MARKER}_START_${id}-->\n\n${label}\n\n<!--${COPY_MARKER}_END_${id}-->`;
  });

  return safeMd.replace(/\x00CODE(\d+)\x00/g, (_, idx) => codeSlots[parseInt(idx, 10)]);
}

function postprocessCopyBlocks(html: string): string {
  return html.replace(COPY_MARKER_PATTERN, (_match, idStr: string, inner: string) =>
    `<div class="copyable-block copyable-block-multi" ${COPY_ID_ATTR}="${COPY_ID_NONCE}-${idStr}">` +
      inner.trim() +
      `<button type="button" class="copy-btn" aria-label="Copy to clipboard">${COPY_ICON}</button>` +
      `</div>`,
  );
}

/** Write each copy block's payload onto the sanitized tree, from the slot map.
 *
 *  Runs AFTER the sanitizer, which is the whole point: `data-copy-text` is
 *  forbidden there, so every surviving one was written here, from a slot this
 *  render allocated and named with `COPY_ID_NONCE`. An id content invented loses
 *  its attribute. The copy handler then reads that block as having no text,
 *  and `startClient` returns on the null. */
function resolveCopyTargets(html: string, copyTexts: Map<number, string>): string {
  return inDom(html, [COPY_ID_ATTR], (body) => {
    for (const el of Array.from(body.querySelectorAll(`[${COPY_ID_ATTR}]`))) {
      const raw = copyTextForSlot(el.getAttribute(COPY_ID_ATTR), copyTexts);
      el.removeAttribute(COPY_ID_ATTR);
      if (raw !== undefined) el.setAttribute('data-copy-text', raw);
    }
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
  // The two clipboard sources, which only `resolveCopyTargets` and
  // `resolveCodeBlockCopy` may write, and which DOMPurify's defaults would
  // otherwise let content author. See `COPY_ID_ATTR`.
  //
  // `style` is NOT here. A kept `position: fixed` does let content paint its
  // own chrome over the app, and `opacity: 0` does hide text from the reader.
  // But this config is shared. `sanitizeHtmlFragments` also scrubs
  // `SlidesPreview` and `RenderedDiff`, where authored markup carries inline
  // style as its only styling channel. Forbidding it here silently unstyles
  // every existing deck, so that hole wants a per-caller config.
  FORBID_ATTR: ['data-copy-text', 'data-copy-code'],
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
  return resolveCodeBlockCopy(
    DOMPurify.sanitize(html.replace(ESCAPE_TO_TEXT_TAG, escapeHtmlAttr), PURIFY_CONFIG),
  );
}

/** Flag each code block the renderer wrote, so its Copy button copies its code.
 *
 *  The Copy button copies only inside `data-copy-code`, which the sanitizer
 *  forbids. So a lookalike wrapper that content wrote copies nothing, however
 *  it hides text inside. The renderer's own code is escaped text with no
 *  markup, so its `textContent` is exactly what the reader sees. */
function resolveCodeBlockCopy(html: string): string {
  return inDom(html, [CODE_COPY_ATTR], (body) => {
    for (const el of Array.from(body.querySelectorAll(`[${CODE_COPY_ATTR}]`))) {
      if (el.getAttribute(CODE_COPY_ATTR) === COPY_ID_NONCE) el.setAttribute('data-copy-code', '');
      el.removeAttribute(CODE_COPY_ATTR);
    }
  });
}

/** At or above this many columns a table stacks into labeled cards on a phone
 *  (shared-components.css, `table[data-stack]`). Below it the grid is kept:
 *  two or three columns read fine with the bounded horizontal scroll. The
 *  threshold lives here because an attribute selector cannot compare a
 *  number. */
const STACK_MIN_COLUMNS = 4;

/** The inert body the post-sanitizer passes work in, made once.
 *
 *  `createHTMLDocument` has no browsing context, so it runs no script and
 *  loads no resource. Its input here is already sanitized, so that is belt to
 *  the braces rather than the guard itself.
 *
 *  Its own body rather than the one `utils/escapeHtml.ts` memoizes for
 *  `stripHtml`, sharing only the probe. One shared node would let either pass
 *  read markup the other left behind.
 *
 *  `undefined` means not yet probed, `null` means this environment has no
 *  `createHTMLDocument`. Every browser Lucidos ships on has carried it for
 *  over a decade, and `sanitizeHtmlFragments` has already refused a caller
 *  with no DOM at all. */
let renderBody: HTMLElement | null | undefined;

function inertRenderBody(): HTMLElement | null {
  if (renderBody === undefined) renderBody = makeInertBody();
  return renderBody;
}

/** Run `pass` over `html` parsed into the inert body, and serialize the result.
 *
 *  **A pass that anchors on `<` or `>` belongs here, never in a regex.** The
 *  serializer leaves both raw inside an attribute value. So no pattern can
 *  tell a real `<td` or `<img` from the same characters in a `title`. Both
 *  string passes this replaced were injection sinks. One put a column label in
 *  attribute-name position. The other freed a real `<img onerror>` out of a
 *  `title` into element position.
 *
 *  The `thread:` rewrite below stays a regex for the reason that names: it
 *  anchors on `"`, which the serializer DOES escape inside a value, so it
 *  cannot match inside one.
 *
 *  `needles` gate the parse. The live streaming buffer re-renders per token,
 *  and most documents carry no image and no table, so those pay one substring
 *  search and nothing else. `docs/code-review-priors.md` carries the measured
 *  cost of the parse, so a later reviewer need not re-derive it. */
function inDom(html: string, needles: string[], pass: (body: HTMLElement) => void): string {
  if (!needles.some((needle) => html.includes(needle))) return html;
  const body = inertRenderBody();
  if (!body) return html;
  body.innerHTML = html;
  try {
    pass(body);
    return body.innerHTML;
  } finally {
    // Emptied whatever happened, so a throw cannot leave one render's markup
    // visible to the next one.
    body.textContent = '';
  }
}

/** A table's own `<th>` and `<tr>`, never a nested table's.
 *
 *  `querySelectorAll` reaches every descendant. Unscoped, it counted a nested
 *  table's headers toward the outer column count, and stamped the outer
 *  labels onto the inner cells. */
function ownDescendants<T extends Element>(table: Element, selector: string): T[] {
  return Array.from(table.querySelectorAll<T>(selector))
    .filter((el) => el.closest('table') === table);
}

/** A column header's plain text, with angle brackets dropped.
 *
 *  Inline markup (`<code>`, a link) goes with them: `content: attr(data-label)`
 *  can only produce a text run, so `textContent` is the whole label.
 *
 *  **Temporary measure**, `docs/temporary-measures.md` § "Angle brackets kept
 *  out of `data-label`". The serializer writes `<` and `>` raw inside an
 *  attribute value, and `utils/linkifyPaths.ts` then re-scans this output with
 *  a tag regex. A raw `>` ends its idea of the tag mid-value. It splices a
 *  link into what is left, which puts the rest of the label in attribute-name
 *  position. Dropping the two characters costs a rare header the bracket it
 *  typed, on the phone caption only. */
function stackLabelText(cell: Element): string {
  return (cell.textContent ?? '').replace(/[<>]/g, '').trim();
}

/** Stamp `data-stack` plus a per-cell `data-label` carrying its column header,
 *  on a table wide enough to read as cards on a phone.
 *
 *  The column index restarts per row, so a row carrying a different cell count
 *  cannot shift every later row's labels by one. */
function stampStackLabels(table: Element): void {
  const labels = ownDescendants(table, 'th').map(stackLabelText);
  if (labels.length < STACK_MIN_COLUMNS) return;
  table.setAttribute('data-stack', '');
  for (const row of ownDescendants(table, 'tr')) {
    let col = 0;
    for (const cell of Array.from(row.children)) {
      if (cell.tagName !== 'TD') continue;
      cell.setAttribute('data-label', labels[col] ?? '');
      col += 1;
    }
  }
}

/** Wrap every table in the scroll container that lets it pan sideways instead
 *  of squeezing its columns. Stamp the wide ones for the phone layout. */
function transformTables(html: string): string {
  return inDom(html, ['<table'], (body) => {
    for (const table of Array.from(body.querySelectorAll('table'))) {
      stampStackLabels(table);
      const wrapper = body.ownerDocument.createElement('div');
      wrapper.className = 'table-scroll-wrapper';
      table.replaceWith(wrapper);
      wrapper.append(table);
    }
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

/** The served `src` for a workspace-relative image source, or `null` when the
 *  source is not ours to resolve.
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
  // Plain text, not attribute-ready: `setAttribute` is what writes it, and the
  // serializer escapes on the way out. Escaping here too would double it.
  return `${lucidos.data.url(path)}${suffix}`;
}

/** Point every workspace-relative image at the mount that actually serves it.
 *
 *  Runs AFTER `sanitizeHtmlFragments`, deliberately: the sanitizer decides
 *  which `src` attributes exist at all, deleting `javascript:` and `data:`
 *  ones outright. So only an attribute that already passed that gate is ever
 *  rewritten, and the sanitizer is never handed a value to re-judge. */
function rewriteImageSourcesIn(body: HTMLElement): void {
  for (const img of Array.from(body.querySelectorAll('img'))) {
    const src = img.getAttribute('src');
    if (src === null) continue;
    const rewritten = workspaceDataImageSrc(src);
    if (rewritten !== null) img.setAttribute('src', rewritten);
  }
}

/** Point every image at its real source AND wrap it, in one parse. */
function prepareImages(html: string): string {
  return inDom(html, ['<img'], prepareImagesIn);
}

function prepareImagesIn(body: HTMLElement): void {
  rewriteImageSourcesIn(body);
  wrapImagesIn(body);
}

/** Wrap every image in the scroll container that lets an oversized screenshot
 *  pan sideways instead of widening the pane (`.image-scroll-wrapper` in
 *  shared-components.css carries the sizing and the cap).
 *
 *  A `<span>` rather than the table wrapper's `<div>`, because marked puts an
 *  image inside a `<p>` and a `<div>` there is invalid: the parser closes the
 *  paragraph at it, stranding the prose that followed. The CSS gives the span
 *  `display: block`, so the layout is the table wrapper's regardless. */
function wrapImagesIn(body: HTMLElement): void {
  for (const img of Array.from(body.querySelectorAll('img'))) {
    const wrapper = body.ownerDocument.createElement('span');
    wrapper.className = 'image-scroll-wrapper';
    img.replaceWith(wrapper);
    wrapper.append(img);
  }
}

/** A bare email autolink right after a `/`. marked's GFM autolinker reads any
 *  `local@domain.tld` run as an email, even inside a path. A home folder like
 *  `/Users/me.x@example.com/` then grows a `mailto:` link mid-path, and a click
 *  opens Mail. Only the renderer's `BARE_EMAIL_ATTR` marks such a link, so an
 *  authored `<x@y>` or `[x](mailto:x)` keeps its link. */
const PATH_EMAIL_AUTOLINK = new RegExp(`/<a ${BARE_EMAIL_ATTR} [^>]*>([^<]*)</a>`, 'g');
const BARE_EMAIL_MARK = new RegExp(`<a ${BARE_EMAIL_ATTR} `, 'g');

export function unwrapPathEmailAutolinks(html: string): string {
  return html.replace(PATH_EMAIL_AUTOLINK, '/$1').replace(BARE_EMAIL_MARK, '<a ');
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
    const copyTexts = new Map<number, string>();
    const preprocessed = preprocessCopyBlocks(md, copyTexts);
    let html = marked.parse(preprocessed, { async: false }) as string;
    html = unwrapPathEmailAutolinks(html);
    // Before the sanitizer, because DOMPurify drops the HTML comments the
    // multiline marker rides on. It carries the slot id, not the payload.
    html = postprocessCopyBlocks(html);
    html = sanitizeHtmlFragments(html);
    html = resolveCopyTargets(html, copyTexts);
    html = prepareImages(html);
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
 *  `parseInline` DOES emit `<img>`, so an image gets the same source rewrite
 *  and scroll wrapper as a reply image. The wrapper is a `<span>`, so the
 *  output stays phrasing content.
 *
 *  Raw HTML never meets the link renderer, so interactive elements it carries
 *  are unwrapped to their contents here. */
export function renderMarkdownInline(md: string): string {
  const html = sanitizeHtmlFragments(marked.parseInline(md, {
    async: false,
    breaks: true,
    renderer: inlineLinkStripRenderer,
  }) as string);
  return inDom(html, ['<'], (body) => {
    for (const el of Array.from(body.querySelectorAll(INTERACTIVE_ELEMENTS))) {
      el.replaceWith(...Array.from(el.childNodes));
    }
    prepareImagesIn(body);
  });
}

/** What the HTML spec calls interactive content, less what the sanitizer
 *  already drops. Any one of them inside an option `<button>` takes its tap. */
const INTERACTIVE_ELEMENTS = 'a, button, input, select, textarea, label, details, audio, video';

/** Like renderMarkdownInline, but KEEPS http(s) links as clickable
 *  `<a target="_blank" rel="noopener">`. Used for the AskUserQuestion question
 *  text, where a pasted bare URL must stay openable.
 *
 *  Safe ONLY in non-interactive containers: an `<a>` inside a `<button>` is
 *  invalid, so option buttons must keep `renderMarkdownInline`. */
export function renderMarkdownInlineWithLinks(md: string): string {
  return prepareImages(sanitizeHtmlFragments(marked.parseInline(md, {
    async: false,
    breaks: true,
    gfm: true,
    renderer: inlineLinkKeepRenderer,
  }) as string));
}
