import { marked } from 'marked';
import type { Tokens } from 'marked';
import DOMPurify from 'dompurify';
import { BARE_EMAIL_ATTR, CODE_COPY_ATTR, COPY_ICON, COPY_ID_NONCE, WIDGET_EMBED_SLOT_ATTR, escapeHtmlAttr } from './markedConfig';
import { makeInertBody } from './escapeHtml';
import { addMarkdownParseMs } from './renderPhaseTimers';
import { WORKSPACE_ID } from './basePath';
import { slugifyWorkspaceName } from './slug';
import { createHeadingSlugger, HEADING_ID_PREFIX } from './headingSlug';
import { markImageUnavailable } from './markdownImageFallback';
import { markdownImageSource, type MarkdownDocumentLocation } from './markdownImageSource';
import { markdownLinkTooltip } from './markdownLinkTooltip';

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

/** DOMPurify's default scheme list plus the six Lucidos schemes.
 *
 *  Each of the six is claimed by an extractor that runs AFTER sanitization:
 *  `thread:` below in this file, and `app:`, `trigger:`, `repo:`, `file:` and
 *  `settings:` in `linkifyPaths`. Stripping one here would break every such
 *  link, because the extractor would find no href left to read. */
const ALLOWED_URI_REGEXP =
  /^(?:(?:(?:f|ht)tps?|mailto|tel|callto|sms|cid|xmpp|matrix|thread|app|trigger|repo|file|settings):|[^a-z]|[a-z+.\-]+(?:[^a-z+.\-:]|$))/i;

const PURIFY_CONFIG = {
  // A raw `<a target="_blank">` in content keeps its new-tab target, which
  // the default attribute allowlist drops.
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

/** A column header's plain text. Inline markup (`<code>`, a link) drops out:
 *  `content: attr(data-label)` can only produce a text run. */
function stackLabelText(cell: Element): string {
  return (cell.textContent ?? '').trim();
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

/** Point every image at the URL that actually serves it (`markdownImageSource`),
 *  or mark it unavailable when its source may not be fetched.
 *
 *  Runs AFTER `sanitizeHtmlFragments`, deliberately: the sanitizer decides
 *  which `src` attributes exist at all, deleting `javascript:` and `data:`
 *  ones outright. So only an attribute that already passed that gate is ever
 *  rewritten, and the sanitizer is never handed a value to re-judge.
 *
 *  Runs after `wrapImagesIn`, so a refused image's notice lands in its wrapper. */
function resolveImageSourcesIn(body: HTMLElement, doc: MarkdownDocumentLocation | undefined): void {
  for (const img of Array.from(body.querySelectorAll('img'))) {
    const src = img.getAttribute('src');
    if (src === null) continue;
    const verdict = markdownImageSource(src, doc);
    if (verdict.kind === 'serve') img.setAttribute('src', verdict.src);
    else if (verdict.kind === 'refuse') markImageUnavailable(img);
  }
}

/** Point every image at its real source AND wrap it, in one parse. */
function prepareImages(html: string, doc: MarkdownDocumentLocation | undefined): string {
  return inDom(html, ['<img'], (body) => prepareImagesIn(body, doc));
}

function prepareImagesIn(body: HTMLElement, doc?: MarkdownDocumentLocation): void {
  applySizeHintsIn(body);
  wrapImagesIn(body);
  resolveImageSourcesIn(body, doc);
}

/** Give every heading GitHub's anchor id, so a `#section` link in the document
 *  has somewhere to land. A heading whose raw HTML already carries an id keeps
 *  it. A heading with no slug text (only an emoji) gets none. */
function stampHeadingIds(html: string): string {
  return inDom(html, ['<h'], (body) => {
    const slug = createHeadingSlugger();
    for (const heading of Array.from(body.querySelectorAll('h1, h2, h3, h4, h5, h6'))) {
      if (heading.hasAttribute('id')) continue;
      const text = slug(heading.textContent ?? '');
      if (text) heading.id = `${HEADING_ID_PREFIX}${text}`;
    }
  });
}

/** Give every link in a rendered DOCUMENT a hover tooltip naming where it
 *  goes, using the host's own `data-tooltip` layer. That layer reaches it
 *  because the document renders into the host DOM, never an iframe. Must run
 *  after `stampHeadingIds`: an in-page anchor's tooltip reads a heading's
 *  text off the id this just stamped. An author's own markdown `title` moves
 *  to `data-tooltip-title`, so it keeps showing above the resolved target
 *  rather than being lost. */
function stampLinkTooltips(html: string, doc: MarkdownDocumentLocation): string {
  return inDom(html, ['<a '], (body) => {
    // Collected once per document, not once per link: an in-page anchor's
    // lookup only ever reads this list, never the live DOM.
    const fragmentCandidates = Array.from(body.querySelectorAll('[id], [name]'));
    for (const anchor of Array.from(body.querySelectorAll('a[href]'))) {
      const href = anchor.getAttribute('href') ?? '';
      const title = anchor.getAttribute('title') ?? '';
      const tooltip = markdownLinkTooltip(href, title, doc, fragmentCandidates);
      if (!tooltip) continue;
      anchor.removeAttribute('title');
      anchor.setAttribute('data-tooltip', tooltip.text);
      if (tooltip.title) anchor.setAttribute('data-tooltip-title', tooltip.title);
      // A long press reveals the tooltip on touch; a plain tap still
      // navigates (packages/lucidos-sdk/src/tooltip.ts).
      anchor.setAttribute('data-tooltip-longpress', '');
    }
  });
}

/** An *image size hint*: a trailing `#<width>x<height>` on an image source,
 *  in image pixels. The engine and `lucidos data write` add it (see
 *  `system-knowhow/glossary.md`). */
const SIZE_HINT = /#([1-9]\d{0,4})x([1-9]\d{0,4})$/;

/** Move each size hint off the source and onto the image, where the
 *  `[data-size-hint]` rule in host-components.css reserves the loaded box.
 *  The source loses it, so the fetch and the image popup see the plain URL. */
function applySizeHintsIn(body: HTMLElement): void {
  for (const img of Array.from(body.querySelectorAll('img'))) {
    const src = img.getAttribute('src');
    const hint = src?.match(SIZE_HINT);
    if (!src || !hint) continue;
    img.setAttribute('src', src.slice(0, hint.index));
    img.setAttribute('data-size-hint', '');
    img.style.setProperty('--hint-w', hint[1]);
    img.style.setProperty('--hint-h', hint[2]);
  }
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

const EM_DASH = String.fromCharCode(0x2014);

/** Inline elements whose text continues the sentence around them. Any other
 *  neighbour (a `<br>`, a block, nothing) is a line edge, which counts as a
 *  space. */
const INLINE_TEXT_TAGS = new Set(['A', 'B', 'CODE', 'DEL', 'EM', 'I', 'KBD', 'MARK', 'S', 'SPAN', 'STRONG', 'SUB', 'SUP']);

const isInline = (node: Node): boolean => node.nodeType === Node.TEXT_NODE
  || (node.nodeType === Node.ELEMENT_NODE && INLINE_TEXT_TAGS.has((node as Element).tagName));

/** The character a text node meets on one side. With no sibling there, an
 *  inline parent passes the question up, so `**x<dash>**` sees what follows
 *  the bold. */
function neighbourChar(node: Node, side: 'before' | 'after'): string {
  for (let cur: Node = node; ; cur = cur.parentNode as Node) {
    const sibling = side === 'before' ? cur.previousSibling : cur.nextSibling;
    if (sibling) {
      const text = isInline(sibling) ? sibling.textContent ?? '' : '';
      return (side === 'before' ? text[text.length - 1] : text[0]) ?? ' ';
    }
    if (!cur.parentNode || !isInline(cur.parentNode)) return ' ';
  }
}

const HTML_NS = 'http://www.w3.org/1999/xhtml';

/** Whether a span may replace part of this text node and survive a re-parse.
 *  SVG and MathML text would eject an HTML span, and a text-only element such
 *  as `<textarea>` would show its markup as characters. Code is left alone. */
function acceptsSpan(node: Text): boolean {
  const parent = node.parentElement;
  return parent !== null
    && parent.namespaceURI === HTML_NS
    && parent.closest('code, pre, textarea, option, title') === null;
}

/** Wrap each unspaced em dash in `.em-dash-gap`, which pads it in CSS.
 *
 *  A monospace font draws the dash one cell wide, so an unspaced one reads as
 *  a hyphenated word. `.claude/rules/em-dashes.md` bans that form, but a model
 *  still writes it. A span changes only the rendering: the text and what the
 *  user copies stay exactly as written. */
function spaceUnspacedEmDashes(html: string): string {
  return inDom(html, [EM_DASH], (body) => {
    const walker = body.ownerDocument.createTreeWalker(body, NodeFilter.SHOW_TEXT);
    const texts: Text[] = [];
    for (let n = walker.nextNode(); n; n = walker.nextNode()) texts.push(n as Text);
    for (const node of texts) {
      if (node.data.includes(EM_DASH) && acceptsSpan(node)) spaceDashesIn(node);
    }
  });
}

function spaceDashesIn(node: Text): void {
  const { data } = node;
  const parts: (string | Node)[] = [];
  let from = 0;
  for (let i = data.indexOf(EM_DASH); i !== -1; i = data.indexOf(EM_DASH, i + 1)) {
    const before = i > 0 ? data[i - 1] : neighbourChar(node, 'before');
    const after = i < data.length - 1 ? data[i + 1] : neighbourChar(node, 'after');
    if (/\s/.test(before) && /\s/.test(after)) continue;
    const gap = node.ownerDocument.createElement('span');
    gap.className = 'em-dash-gap';
    gap.textContent = EM_DASH;
    parts.push(data.slice(from, i), gap);
    from = i + 1;
  }
  if (parts.length === 0) return;
  parts.push(data.slice(from));
  node.replaceWith(...parts.filter((p) => p !== ''));
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

/** `document` marks the markdown as a file previewed as a document rather than
 *  a chat message. Its headings get anchor ids, and a relative image resolves
 *  against its folder (`markdownImageSource`). Chat passes none: a timeline of
 *  many messages would repeat every id. */
export function renderMarkdown(md: string, opts?: { cache?: boolean; document?: MarkdownDocumentLocation }): string {
  const useCache = opts?.cache !== false;
  const doc = opts?.document;
  // The same text at two locations resolves its images differently.
  const cacheKey = doc ? `${JSON.stringify(doc)}\u0000${md}` : md;
  if (useCache) {
    const hit = markdownCache.get(cacheKey);
    if (hit !== undefined) {
      // LRU touch: move to most-recently-used.
      markdownCache.delete(cacheKey);
      markdownCache.set(cacheKey, hit);
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
    html = prepareImages(html, doc);
    if (doc) html = stampHeadingIds(html);
    html = transformTables(html);
    html = spaceUnspacedEmDashes(html);
    // The workspace-qualified form is what the copy-ref button emits.
    html = html.replace(
      /href="thread:(?:([a-zA-Z0-9_-]+)\/)?([0-9a-f-]+)"/g,
      (_match, workspace: string | undefined, threadId: string) => {
        const wsAttr = workspace ? ` data-thread-workspace="${escapeHtmlAttr(workspace)}"` : '';
        const href = escapeHtmlAttr(threadLinkHref(workspace, threadId));
        return `href="${href}" data-thread-id="${threadId}"${wsAttr} class="thread-link"`;
      }
    );
    if (doc) html = stampLinkTooltips(html, doc);
    if (useCache) {
      markdownCache.set(cacheKey, html);
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
    // Inline markdown mounts no widget: an option lifts its embed first, so
    // what is left reads as its label (ADR 0415).
    for (const slot of Array.from(body.querySelectorAll(`[${WIDGET_EMBED_SLOT_ATTR}]`))) {
      slot.replaceWith(slot.textContent ?? '');
    }
    prepareImagesIn(body);
  });
}

/** What the HTML spec calls interactive content, less what the sanitizer
 *  already drops. Any one of them inside an option `<button>` takes its tap. */
const INTERACTIVE_ELEMENTS = 'a, button, input, select, textarea, label, details, audio, video';
