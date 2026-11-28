
export const ALL_HIGHLIGHT = 'lucidos-find';
export const CURRENT_HIGHLIGHT = 'lucidos-find-current';

export const MAX_FIND_MATCHES = 500;

const SKIPPED_TAGS = new Set([
  'SCRIPT', 'STYLE', 'NOSCRIPT', 'TEMPLATE', 'TEXTAREA', 'SELECT', 'OPTION',
]);

const BLOCK_TAGS = new Set([
  'ADDRESS', 'ARTICLE', 'ASIDE', 'BLOCKQUOTE', 'DD', 'DETAILS', 'DIV', 'DL', 'DT',
  'FIGCAPTION', 'FIGURE', 'FOOTER', 'H1', 'H2', 'H3', 'H4', 'H5', 'H6', 'HEADER',
  'HR', 'LI', 'MAIN', 'NAV', 'OL', 'P', 'PRE', 'SECTION', 'SUMMARY', 'TABLE',
  'TBODY', 'TD', 'TFOOT', 'TH', 'THEAD', 'TR', 'UL',
]);

export type TextLayout = 'rendered' | 'markup';

const BLOCK_BREAK = '\u0000';

export interface FindResult {
  total: number;
  current: number;
  capped: boolean;
}

export function queryPattern(query: string): RegExp | null {
  const words = query.trim().split(/\s+/).filter(Boolean);
  if (words.length === 0) return null;
  const escaped = words.map((w) => w.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'));
  return new RegExp(escaped.join('\\s+'), 'giu');
}

export function matchSpans(text: string, pattern: RegExp, limit: number): Array<[number, number]> {
  const spans: Array<[number, number]> = [];
  for (const m of text.matchAll(pattern)) {
    if (spans.length >= limit) break;
    spans.push([m.index, m.index + m[0].length]);
  }
  return spans;
}

export function stepIndex(current: number, step: 1 | -1, total: number): number {
  if (total === 0) return 0;
  return (current + step + total) % total;
}

interface Segment {
  node: Text;
  start: number;
}

interface PageText {
  text: string;
  segments: Segment[];
}

function isBlock(el: Element, layout: TextLayout, cache: Map<Element, boolean>): boolean {
  let known = cache.get(el);
  if (known === undefined) {
    known = layout === 'markup'
      ? BLOCK_TAGS.has(el.tagName)
      : !getComputedStyle(el).display.startsWith('inline');
    cache.set(el, known);
  }
  return known;
}

function blockOf(el: Element, root: Element, layout: TextLayout, cache: Map<Element, boolean>): Element {
  let at: Element | null = el;
  while (at && at !== root && !isBlock(at, layout, cache)) at = at.parentElement;
  return at ?? root;
}

function isVisible(el: Element, layout: TextLayout, cache: Map<Element, boolean>): boolean {
  if (layout === 'markup') return true;
  let known = cache.get(el);
  if (known === undefined) {
    known = typeof el.checkVisibility === 'function'
      ? el.checkVisibility({ visibilityProperty: true })
      : true;
    cache.set(el, known);
  }
  return known;
}

export function collectPageText(root: Element = document.body, layout: TextLayout = 'rendered'): PageText {
  const visible = new Map<Element, boolean>();
  const blocks = new Map<Element, boolean>();
  const walker = root.ownerDocument.createTreeWalker(root, NodeFilter.SHOW_TEXT | NodeFilter.SHOW_ELEMENT, {
    acceptNode(node) {
      if (node.nodeType === Node.ELEMENT_NODE) {
        const tag = (node as Element).tagName;
        if (tag === 'BR') return NodeFilter.FILTER_ACCEPT;
        return SKIPPED_TAGS.has(tag) ? NodeFilter.FILTER_REJECT : NodeFilter.FILTER_SKIP;
      }
      const parent = node.parentElement;
      if (!parent || !node.nodeValue) return NodeFilter.FILTER_REJECT;
      return isVisible(parent, layout, visible) ? NodeFilter.FILTER_ACCEPT : NodeFilter.FILTER_REJECT;
    },
  });
  let text = '';
  const segments: Segment[] = [];
  let lastBlock: Element | null = null;
  for (let node = walker.nextNode(); node; node = walker.nextNode()) {
    if (node.nodeType === Node.ELEMENT_NODE) {
      text += '\n';
      continue;
    }
    const textNode = node as Text;
    const block = blockOf(textNode.parentElement!, root, layout, blocks);
    if (lastBlock && block !== lastBlock) text += BLOCK_BREAK;
    lastBlock = block;
    segments.push({ node: textNode, start: text.length });
    text += textNode.nodeValue;
  }
  return { text, segments };
}

function segmentAt(segments: Segment[], at: number, atEnd: boolean): Segment {
  let lo = 0;
  let hi = segments.length - 1;
  while (lo < hi) {
    const mid = (lo + hi + 1) >> 1;
    const start = segments[mid].start;
    if (start < at || (!atEnd && start === at)) lo = mid;
    else hi = mid - 1;
  }
  return segments[lo];
}

export function spanToRange(page: PageText, [from, to]: [number, number]): Range {
  const a = segmentAt(page.segments, from, false);
  const b = segmentAt(page.segments, to, true);
  const range = document.createRange();
  range.setStart(a.node, from - a.start);
  range.setEnd(b.node, to - b.start);
  return range;
}

export function frameStylesHighlights(): boolean {
  return highlightApiPresent()
    && getComputedStyle(document.documentElement).getPropertyValue('--find-highlights').trim() === 'styled';
}

export function highlightApiPresent(): boolean {
  return typeof CSS !== 'undefined' && 'highlights' in CSS && typeof Highlight === 'function';
}

function firstInView(ranges: Range[], root: Element, band: ClearBand | undefined): number {
  const top = Math.max(0, root.getBoundingClientRect().top, band?.top ?? 0);
  const index = ranges.findIndex((r) => r.getBoundingClientRect().bottom >= top);
  return index === -1 ? 0 : index;
}

function firstFrom(ranges: Range[], from: Node): number {
  const at = from.ownerDocument!.createRange();
  at.setStartBefore(from);
  return ranges.findIndex((r) => r.compareBoundaryPoints(Range.START_TO_START, at) >= 0);
}

function scrolls(el: Element): boolean {
  const style = getComputedStyle(el);
  const canScroll = /auto|scroll|overlay/.test(`${style.overflowY} ${style.overflowX}`);
  return canScroll && (el.scrollHeight > el.clientHeight || el.scrollWidth > el.clientWidth);
}

function centring(spanStart: number, spanSize: number, boxStart: number, boxSize: number): number {
  const visible = spanStart >= boxStart && spanStart + spanSize <= boxStart + boxSize;
  return visible ? 0 : spanStart - boxStart - (boxSize - spanSize) / 2;
}

export interface ClearBand {
  top: number;
  bottom: number;
}

function clipToBand(start: number, end: number, band: ClearBand | undefined): [number, number] {
  if (!band) return [start, end];
  const top = Math.max(start, band.top);
  const bottom = Math.min(end, band.bottom);
  return bottom > top ? [top, bottom] : [start, end];
}

export function revealRange(range: Range, band?: ClearBand): void {
  for (let el = range.startContainer.parentElement; el && el !== document.documentElement; el = el.parentElement) {
    if (!scrolls(el)) continue;
    const box = el.getBoundingClientRect();
    const r = range.getBoundingClientRect();
    const [top, bottom] = clipToBand(box.top, box.bottom, band);
    el.scrollTop += centring(r.top, r.height, top, bottom - top);
    el.scrollLeft += centring(r.left, r.width, box.left, box.width);
  }
  const r = range.getBoundingClientRect();
  const [top, bottom] = clipToBand(0, window.innerHeight, band);
  const dy = centring(r.top, r.height, top, bottom - top);
  const dx = centring(r.left, r.width, 0, window.innerWidth);
  if (dx || dy) window.scrollBy(dx, dy);
}

export interface FindPainter {
  paint(ranges: Range[], current: number): void;
  clear(): void;
}

function setHighlight(name: string, ranges: Range[]): void {
  CSS.highlights.get(name)?.clear();
  if (ranges.length > 0) CSS.highlights.set(name, new Highlight(...ranges));
  else CSS.highlights.delete(name);
}

export function createPainter(highlights: () => boolean): FindPainter {
  let selectionOwned = false;
  return {
    paint(ranges, current) {
      const at = current >= 0 ? ranges[current] : undefined;
      if (highlights()) {
        setHighlight(ALL_HIGHLIGHT, ranges);
        setHighlight(CURRENT_HIGHLIGHT, at ? [at] : []);
        return;
      }
      const selection = window.getSelection();
      if (!selection) return;
      selection.removeAllRanges();
      if (at) selection.addRange(at);
      selectionOwned = !!at;
    },
    clear() {
      if (typeof CSS !== 'undefined' && 'highlights' in CSS) {
        setHighlight(ALL_HIGHLIGHT, []);
        setHighlight(CURRENT_HIGHLIGHT, []);
      }
      if (selectionOwned) {
        window.getSelection()?.removeAllRanges();
        selectionOwned = false;
      }
    },
  };
}

export interface Finder {
  find(query: string, step?: 1 | -1, from?: Node): FindResult;
  clear(): void;
}

export function createFinder(
  root: () => Element | null, painter: FindPainter, band?: () => ClearBand | undefined,
): Finder {
  let session: { query: string; current: number } | null = null;
  const none: FindResult = { total: 0, current: 0, capped: false };
  const clear = () => {
    session = null;
    painter.clear();
  };
  return {
    clear,
    find(query, step, from) {
      const pattern = queryPattern(query);
      const el = root();
      if (!pattern || !el) {
        clear();
        return none;
      }
      const page = collectPageText(el);
      const spans = matchSpans(page.text, pattern, MAX_FIND_MATCHES + 1);
      const capped = spans.length > MAX_FIND_MATCHES;
      const ranges = spans.slice(0, MAX_FIND_MATCHES).map((s) => spanToRange(page, s));
      const total = ranges.length;
      const uncovered = band?.();

      let current: number;
      const start = from ? firstFrom(ranges, from) : -1;
      if (start !== -1) current = start;
      else if (from || session?.query !== query) current = firstInView(ranges, el, uncovered);
      else if (step) current = stepIndex(session.current, step, total);
      else current = Math.min(session.current, Math.max(total - 1, 0));

      session = { query, current };
      painter.paint(ranges, total > 0 ? current : -1);
      if (total > 0) revealRange(ranges[current], uncovered);
      return { total, current: total > 0 ? current + 1 : 0, capped };
    },
  };
}

const frameFinder = createFinder(() => document.body, createPainter(frameStylesHighlights));

export const find = (query: string, step?: 1 | -1): FindResult => frameFinder.find(query, step);
export const clearFind = (): void => frameFinder.clear();

export function serveFind(args: unknown): FindResult {
  const a = (args ?? {}) as { query?: unknown; step?: unknown; clear?: unknown };
  if (a.clear === true) {
    clearFind();
    return { total: 0, current: 0, capped: false };
  }
  if (typeof a.query !== 'string') throw new Error('find needs a query');
  const step = a.step === 1 || a.step === -1 ? a.step : undefined;
  return find(a.query, step);
}
