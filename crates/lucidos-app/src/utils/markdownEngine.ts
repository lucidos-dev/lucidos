import type { Tokens, Renderer } from 'marked';
import type { marked as Marked } from 'marked';
import type Purifier from 'dompurify';
import { signal, type ReadonlySignal } from '@preact/signals';
import { lazyModule } from './lazyComponent';
import { configureMarked } from './markedConfig';
import { escapeHtmlAttr } from './escapeHtmlAttr';
import {
  BARE_EMAIL_ATTR, CODE_COPY_ATTR, COPY_ID_NONCE, TASK_CHECKBOX_ATTR, WIDGET_EMBED_SLOT_ATTR, taskCheckboxTag,
} from './markdownMarkers';
import { COPY_ICON } from './copyIcons';
import { makeInertBody } from './escapeHtml';
import { addMarkdownParseMs } from './renderPhaseTimers';
import { BASE_PATH, REMOTE_GATEWAY, WORKSPACE_ID, workspaceRouteUrlSegment } from './basePath';
import { scopeRemoteContent } from './remoteContent';
import { THREAD_LINK_SOURCE, linkedWorkspaceRoute, resolveLinkedWorkspace } from './threadLinkWorkspace';
import { landingHash } from './workspaceLanding';
import { createHeadingSlugger, HEADING_ID_PREFIX } from './headingSlug';
import { markImageUnavailable } from './markdownImageFallback';
import { markdownImageSource, type MarkdownDocumentLocation } from './markdownImageSource';
import { markdownLinkTooltip } from './markdownLinkTooltip';

let marked: typeof Marked;
let DOMPurify: typeof Purifier;
const ready = signal(false);
export const markdownLibrariesReady: ReadonlySignal<boolean> = ready;
const libraries = lazyModule(() => import('./markdownLibraries'));
let requested = false;

export async function loadMarkdownLibraries(): Promise<boolean> {
  requested = true;
  if (!await libraries.preload()) return false;
  if (markdownLibrariesReady.peek()) return true;
  ({ marked, DOMPurify } = libraries.get());
  configureMarked(marked);
  inlineLinkStripRenderer = new marked.Renderer();
  inlineLinkStripRenderer.link = function({ tokens }: Tokens.Link): string {
    return this.parser.parseInline(tokens);
  };
  ready.value = true;
  return true;
}

export function markdownParser(): typeof Marked {
  if (!ready.peek()) throw new Error('Markdown parser was read before its libraries loaded');
  return marked;
}

function librariesReady(): boolean {
  if (markdownLibrariesReady.value) return true;
  if (!requested) void loadMarkdownLibraries();
  return false;
}

function threadLinkHref(workspace: string | undefined, threadId: string): string {
  if (WORKSPACE_ID === null || typeof location === 'undefined') return '#';
  const route = workspace
    ? linkedWorkspaceRoute(resolveLinkedWorkspace(workspace, REMOTE_GATEWAY))
    : WORKSPACE_ID;
  return `${location.origin}/${workspaceRouteUrlSegment(route)}/${landingHash({ thread: threadId })}`;
}

const THREAD_HREF_RE = new RegExp(`href="${THREAD_LINK_SOURCE}"`, 'g');

let inlineLinkStripRenderer: Renderer;

const COPY_MARKER = `LUCIDOS_COPY_BLOCK_${COPY_ID_NONCE}`;
const COPY_MARKER_PATTERN = new RegExp(
  `<!--${COPY_MARKER}_START_(\\d+)-->([\\s\\S]*?)<!--${COPY_MARKER}_END_\\1-->`,
  'g',
);

const CODE_PROTECTION_PATTERN =
  /^ {0,3}(?=(`{4,}|~{4,}))\1[^\n]*\n[\s\S]*?^ {0,3}\1[ \t]*\r?$|```[\s\S]*?```|~~~[\s\S]*?~~~|`[^`\n]+`/gm;

const LINK_DEFINITION_LABEL = /\[([^\]]*)\]:/g;

const COPY_ID_ATTR = 'data-copy-id';


function copyTextForSlot(attr: string | null, copyTexts: Map<number, string>): string | undefined {
  const prefix = `${COPY_ID_NONCE}-`;
  if (attr === null || !attr.startsWith(prefix)) return undefined;
  return copyTexts.get(Number(attr.slice(prefix.length)));
}

function preprocessCopyBlocks(md: string, encodedTexts: Map<number, string>): string {
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

    const restored = trimmed.replace(/\x00CODE(\d+)\x00/g, (_, idx) =>
      codeSlots[parseInt(idx, 10)],
    );

    const id = counter++;
    encodedTexts.set(id, restored);

    const label = escapeHtmlAttr(trimmed).replace(LINK_DEFINITION_LABEL, '&#91;$1]:');

    if (!isMultiline) {
      return `<span class="copyable-block" ${COPY_ID_ATTR}="${COPY_ID_NONCE}-${id}">${label}</span>`;
    }

    return `<!--${COPY_MARKER}_START_${id}-->\n\n${label}\n\n<!--${COPY_MARKER}_END_${id}-->`;
  });

  return safeMd.replace(/\x00CODE(\d+)\x00/g, (_, idx) => codeSlots[parseInt(idx, 10)]);
}

function postprocessCopyBlocks(html: string): string {
  return html.replace(COPY_MARKER_PATTERN, (_match, idStr: string, inner: string) =>
    `<div class="copyable-block copyable-block-multi" ${COPY_ID_ATTR}="${COPY_ID_NONCE}-${idStr}">${inner.trim()}</div>`,
  );
}

function copyButton(extraClass: string, label: string): string {
  return `<button type="button" class="copy-btn${extraClass}" aria-label="${label}">${COPY_ICON}</button>`;
}

function resolveCopyTargets(html: string, copyTexts: Map<number, string>): string {
  return inDom(html, [COPY_ID_ATTR], (body) => {
    for (const el of Array.from(body.querySelectorAll(`[${COPY_ID_ATTR}]`))) {
      const raw = copyTextForSlot(el.getAttribute(COPY_ID_ATTR), copyTexts);
      el.removeAttribute(COPY_ID_ATTR);
      if (raw === undefined) continue;
      el.setAttribute('data-copy-text', raw);
      el.insertAdjacentHTML('beforeend', copyButton('', 'Copy to clipboard'));
    }
  });
}


const ESCAPE_TO_TEXT_TAG =
  /<(\/?)(iframe|script|style|object|embed|applet|base|meta|link|xmp|plaintext|noscript|noembed|noframes|animate|animateTransform|set|form|button|select|textarea|input|map|area)([\s/][^>]*)?>/gi;

const TASK_CHECKBOX_MARK = `${TASK_CHECKBOX_ATTR}="${COPY_ID_NONCE}"`;

function escapeTagToText(tag: string, _close: string, name: string): string {
  const isTaskCheckbox = name.toLowerCase() === 'input' && tag.includes(TASK_CHECKBOX_MARK);
  return isTaskCheckbox ? tag : escapeHtmlAttr(tag);
}

function removeAuthoredInputs(html: string): string {
  return inDom(html, ['<input'], (body) => {
    for (const input of Array.from(body.querySelectorAll('input'))) {
      if (input.getAttribute(TASK_CHECKBOX_ATTR) === COPY_ID_NONCE) {
        input.insertAdjacentHTML('beforebegin', taskCheckboxTag(input.hasAttribute('checked')));
      }
      input.remove();
    }
  });
}

const URL_ATTRIBUTES = [
  'href', 'xlink:href', 'src', 'srcset', 'action',
  'formaction', 'poster', 'background', 'ping', 'data',
];

function stripDangerousUrlSchemes(node: Node): void {
  const el = node as Element;
  if (typeof el.getAttribute !== 'function') return;
  for (const name of URL_ATTRIBUTES) {
    const value = el.getAttribute(name);
    if (value === null) continue;
    const scheme = value.replace(/[\u0000-\u0020]+/g, '').toLowerCase();
    if (scheme.startsWith('javascript:') || scheme.startsWith('data:')) {
      el.removeAttribute(name);
    }
  }
}

let urlSchemeHookInstalled = false;

function installUrlSchemeHook(): void {
  if (urlSchemeHookInstalled) return;
  DOMPurify.addHook('afterSanitizeAttributes', stripDangerousUrlSchemes);
  urlSchemeHookInstalled = true;
}

const ALLOWED_URI_REGEXP =
  /^(?:(?:(?:f|ht)tps?|mailto|tel|callto|sms|cid|xmpp|matrix|thread|app|trigger|repo|file|settings):|[^a-z]|[a-z+.\-]+(?:[^a-z+.\-:]|$))/i;

const PURIFY_CONFIG = {
  ADD_ATTR: ['target'],
  FORBID_TAGS: [
    'style', 'animate', 'animateTransform', 'set',
    'form', 'button', 'select', 'textarea', 'option', 'optgroup', 'datalist',
    'map', 'area',
  ],
  FORBID_ATTR: [
    'data-copy-text', 'data-copy-code',
    'popover', 'popovertarget', 'popovertargetaction', 'command', 'commandfor',
  ],
  ALLOWED_URI_REGEXP,
};

export function sanitizeHtmlFragments(html: string, doc?: MarkdownDocumentLocation): string {
  if (!librariesReady()) return escapeHtmlAttr(html);
  if (!DOMPurify.isSupported) {
    throw new Error(
      'sanitizeHtmlFragments needs a DOM: add "// @vitest-environment jsdom" to this test file',
    );
  }
  installUrlSchemeHook();
  const sanitized = DOMPurify.sanitize(html.replace(ESCAPE_TO_TEXT_TAG, escapeTagToText), PURIFY_CONFIG);
  return finalizeHtml(resolveCodeBlockCopy(removeAuthoredInputs(sanitized)), doc);
}

function resolveCodeBlockCopy(html: string): string {
  return inDom(html, [CODE_COPY_ATTR], (body) => {
    for (const el of Array.from(body.querySelectorAll(`[${CODE_COPY_ATTR}]`))) {
      const isRenderers = el.getAttribute(CODE_COPY_ATTR) === COPY_ID_NONCE;
      el.removeAttribute(CODE_COPY_ATTR);
      if (!isRenderers) continue;
      el.setAttribute('data-copy-code', '');
      el.querySelector(':scope > .code-block-header')
        ?.insertAdjacentHTML('beforeend', copyButton(' code-block-copy-btn', 'Copy code'));
    }
  });
}

const STACK_MIN_COLUMNS = 4;

let renderBody: HTMLElement | null | undefined;

function inertRenderBody(): HTMLElement | null {
  if (renderBody === undefined) renderBody = makeInertBody();
  return renderBody;
}

function inDom(html: string, needles: string[], pass: (body: HTMLElement) => void): string {
  if (!needles.some((needle) => html.includes(needle))) return html;
  const body = inertRenderBody();
  if (!body) return html;
  body.innerHTML = html;
  try {
    pass(body);
    return body.innerHTML;
  } finally {
    body.textContent = '';
  }
}

function ownDescendants<T extends Element>(table: Element, selector: string): T[] {
  return Array.from(table.querySelectorAll<T>(selector))
    .filter((el) => el.closest('table') === table);
}

function stackLabelText(cell: Element): string {
  return (cell.textContent ?? '').trim();
}

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

function resolveImageSourcesIn(body: HTMLElement, doc: MarkdownDocumentLocation | undefined): void {
  for (const img of Array.from(body.querySelectorAll('img'))) {
    const src = img.getAttribute('src');
    if (src === null) continue;
    const verdict = markdownImageSource(src, doc);
    if (verdict.kind === 'serve') img.setAttribute('src', verdict.src);
    else if (verdict.kind === 'refuse') markImageUnavailable(img);
  }
}

function finalizeHtml(html: string, doc?: MarkdownDocumentLocation): string {
  return inDom(html, ['<'], (body) => {
    prepareImagesIn(body, doc);
    if (REMOTE_GATEWAY !== null) {
      for (const el of Array.from(body.querySelectorAll('*'))) {
        scopeRemoteContent(el, BASE_PATH, location.origin);
      }
    }
  });
}

function prepareImagesIn(body: HTMLElement, doc?: MarkdownDocumentLocation): void {
  applySizeHintsIn(body);
  wrapImagesIn(body);
  resolveImageSourcesIn(body, doc);
}

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

function stampLinkTooltips(html: string, doc: MarkdownDocumentLocation): string {
  return inDom(html, ['<a '], (body) => {
    const fragmentCandidates = Array.from(body.querySelectorAll('[id], [name]'));
    for (const anchor of Array.from(body.querySelectorAll('a[href]'))) {
      const href = anchor.getAttribute('href') ?? '';
      const title = anchor.getAttribute('title') ?? '';
      const tooltip = markdownLinkTooltip(href, title, doc, fragmentCandidates);
      if (!tooltip) continue;
      anchor.removeAttribute('title');
      anchor.setAttribute('data-tooltip', tooltip.text);
      if (tooltip.title) anchor.setAttribute('data-tooltip-title', tooltip.title);
      anchor.setAttribute('data-tooltip-longpress', '');
    }
  });
}

const SIZE_HINT = /#([1-9]\d{0,4})x([1-9]\d{0,4})$/;

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

function wrapImagesIn(body: HTMLElement): void {
  for (const img of Array.from(body.querySelectorAll('img'))) {
    const wrapper = body.ownerDocument.createElement('span');
    wrapper.className = 'image-scroll-wrapper';
    img.replaceWith(wrapper);
    wrapper.append(img);
  }
}

const EM_DASH = String.fromCharCode(0x2014);

const INLINE_TEXT_TAGS = new Set(['A', 'B', 'CODE', 'DEL', 'EM', 'I', 'KBD', 'MARK', 'S', 'SPAN', 'STRONG', 'SUB', 'SUP']);

const isInline = (node: Node): boolean => node.nodeType === Node.TEXT_NODE
  || (node.nodeType === Node.ELEMENT_NODE && INLINE_TEXT_TAGS.has((node as Element).tagName));

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

function acceptsSpan(node: Text): boolean {
  const parent = node.parentElement;
  return parent !== null
    && parent.namespaceURI === HTML_NS
    && parent.closest('code, pre, title') === null;
}

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

const PATH_EMAIL_AUTOLINK = new RegExp(`/<a ${BARE_EMAIL_ATTR} [^>]*>([^<]*)</a>`, 'g');
const BARE_EMAIL_MARK = new RegExp(`<a ${BARE_EMAIL_ATTR} `, 'g');

export function unwrapPathEmailAutolinks(html: string): string {
  return html.replace(PATH_EMAIL_AUTOLINK, '/$1').replace(BARE_EMAIL_MARK, '<a ');
}

const MARKDOWN_CACHE_MAX = 400;
const markdownCache = new Map<string, string>();

export function renderMarkdown(md: string, opts?: { cache?: boolean; document?: MarkdownDocumentLocation }): string {
  if (!librariesReady()) return `<p>${escapeHtmlAttr(md).replace(/\n/g, '<br>')}</p>`;
  const useCache = opts?.cache !== false;
  const doc = opts?.document;
  const cacheKey = doc ? `${JSON.stringify(doc)}\u0000${md}` : md;
  if (useCache) {
    const hit = markdownCache.get(cacheKey);
    if (hit !== undefined) {
      markdownCache.delete(cacheKey);
      markdownCache.set(cacheKey, hit);
      return hit;
    }
  }
  const parseStart = performance.now();
  try {
    const copyTexts = new Map<number, string>();
    const preprocessed = preprocessCopyBlocks(md, copyTexts);
    let html = marked.parse(preprocessed, { async: false }) as string;
    html = unwrapPathEmailAutolinks(html);
    html = postprocessCopyBlocks(html);
    html = sanitizeHtmlFragments(html, doc);
    html = resolveCopyTargets(html, copyTexts);
    if (doc) html = stampHeadingIds(html);
    html = transformTables(html);
    html = spaceUnspacedEmDashes(html);
    html = html.replace(
      THREAD_HREF_RE,
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
        const oldest = markdownCache.keys().next().value;
        if (oldest !== undefined) markdownCache.delete(oldest);
      }
    }
    return html;
  } finally {
    addMarkdownParseMs(performance.now() - parseStart);
  }
}

export function renderMarkdownInline(md: string): string {
  if (!librariesReady()) return escapeHtmlAttr(md);
  const html = sanitizeHtmlFragments(marked.parseInline(md, {
    async: false,
    breaks: true,
    renderer: inlineLinkStripRenderer,
  }) as string);
  return inDom(html, ['<'], (body) => {
    for (const el of Array.from(body.querySelectorAll(INTERACTIVE_ELEMENTS))) {
      el.replaceWith(...Array.from(el.childNodes));
    }
    for (const slot of Array.from(body.querySelectorAll(`[${WIDGET_EMBED_SLOT_ATTR}]`))) {
      slot.replaceWith(slot.textContent ?? '');
    }
  });
}

const INTERACTIVE_ELEMENTS = 'a, label, details, audio, video';
