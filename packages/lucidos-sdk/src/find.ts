/**
 * The find bar's matcher: one root-scoped finder for every target.
 *
 * Three places run it. An app frame serves it as the `find` host op, since
 * the host cannot read an isolated frame (ADR 0227). The HTML artifact preview
 * gets a bundle of it (`previewBundles.build.mjs`). The host imports it as
 * `@lucidos/find` for text previews and the transcript.
 *
 * It never edits the DOM it searches. Matches are Ranges in two CSS Custom
 * Highlights, so a page's own framework keeps sole ownership of its markup.
 * Where the highlights cannot show, the current match is shown as the selection.
 */

/** Every match. */
export const ALL_HIGHLIGHT = 'lucidos-find';
/** The match the reader is on. */
export const CURRENT_HIGHLIGHT = 'lucidos-find-current';

/** A query that matches more than this stops counting. Every match is a Range
 *  the page repaints, and 2000 of them froze an iPhone. */
export const MAX_FIND_MATCHES = 500;

/** Subtrees whose text the reader never sees as page text. Form fields are
 *  skipped too: their text nodes hold a default, not what the field shows. */
const SKIPPED_TAGS = new Set([
  'SCRIPT', 'STYLE', 'NOSCRIPT', 'TEMPLATE', 'TEXTAREA', 'SELECT', 'OPTION',
]);

/** The block elements, for text read off the page (`'markup'` layout), where
 *  nothing is laid out and no computed style answers. */
const BLOCK_TAGS = new Set([
  'ADDRESS', 'ARTICLE', 'ASIDE', 'BLOCKQUOTE', 'DD', 'DETAILS', 'DIV', 'DL', 'DT',
  'FIGCAPTION', 'FIGURE', 'FOOTER', 'H1', 'H2', 'H3', 'H4', 'H5', 'H6', 'HEADER',
  'HR', 'LI', 'MAIN', 'NAV', 'OL', 'P', 'PRE', 'SECTION', 'SUMMARY', 'TABLE',
  'TBODY', 'TD', 'TFOOT', 'TH', 'THEAD', 'TR', 'UL',
]);

/** How a block is told apart. `'rendered'` asks the live page's computed
 *  style. `'markup'` reads the tag, for a document parsed but never shown,
 *  such as rendered Markdown counted before it is drawn. */
export type TextLayout = 'rendered' | 'markup';

/** Joins the text of two blocks, so a match cannot run from one paragraph into
 *  the next. Nothing a reader types matches it. */
const BLOCK_BREAK = '\u0000';

/** What the host gets back. `current` is 1-based, and 0 when nothing matched. */
export interface FindResult {
  total: number;
  current: number;
  capped: boolean;
}

/** Build the matcher for a query: case-insensitive, and any run of whitespace
 *  in the query matches any run in the page, the way rendered text collapses
 *  it. Null for a query with nothing to find. */
export function queryPattern(query: string): RegExp | null {
  const words = query.trim().split(/\s+/).filter(Boolean);
  if (words.length === 0) return null;
  const escaped = words.map((w) => w.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'));
  return new RegExp(escaped.join('\\s+'), 'giu');
}

/** Where each match starts and ends in `text`, at most `limit` of them. */
export function matchSpans(text: string, pattern: RegExp, limit: number): Array<[number, number]> {
  const spans: Array<[number, number]> = [];
  for (const m of text.matchAll(pattern)) {
    if (spans.length >= limit) break;
    spans.push([m.index, m.index + m[0].length]);
  }
  return spans;
}

/** The next current index after a step, wrapping at both ends. */
export function stepIndex(current: number, step: 1 | -1, total: number): number {
  if (total === 0) return 0;
  return (current + step + total) % total;
}

interface Segment {
  node: Text;
  /** Offset of this node's first character in the joined text. */
  start: number;
}

/** The page's visible text joined into one string, with a map back to nodes. */
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
    // jsdom and old engines lack it. There the text counts, as it would for a
    // reader who cannot tell either.
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
    // A line break reads as whitespace, so a query's space matches across it.
    // It belongs to no segment, and a trimmed query never starts or ends on it.
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

/** The segment holding joined-text offset `at`. With `atEnd`, an offset on a
 *  boundary belongs to the node it ends, not the one it starts. */
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

/** Whether this frame can show the two highlights. `sdk-iframe.css` styles
 *  them and sets `--find-highlights`. Without that stylesheet a highlight
 *  would paint nothing, so the selection stands in. */
export function frameStylesHighlights(): boolean {
  return highlightApiPresent()
    && getComputedStyle(document.documentElement).getPropertyValue('--find-highlights').trim() === 'styled';
}

export function highlightApiPresent(): boolean {
  return typeof CSS !== 'undefined' && 'highlights' in CSS && typeof Highlight === 'function';
}

/** The first match at or below the top of `root`'s visible box, and of the
 *  clear band. A fresh query lands where the reader is looking, instead of
 *  jumping to the top. */
function firstInView(ranges: Range[], root: Element, band: ClearBand | undefined): number {
  const top = Math.max(0, root.getBoundingClientRect().top, band?.top ?? 0);
  const index = ranges.findIndex((r) => r.getBoundingClientRect().bottom >= top);
  return index === -1 ? 0 : index;
}

/** The first match starting at or after `from`, or -1 when none does. */
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

/** How far to move a box so `span` sits centred in it, or 0 when it already
 *  shows whole. */
function centring(spanStart: number, spanSize: number, boxStart: number, boxSize: number): number {
  const visible = spanStart >= boxStart && spanStart + spanSize <= boxStart + boxSize;
  return visible ? 0 : spanStart - boxStart - (boxSize - spanSize) / 2;
}

/** The rows of the viewport nothing covers, top inclusive, bottom exclusive. */
export interface ClearBand {
  top: number;
  bottom: number;
}

/** `[start, end)` cut to `band`, or left whole when nothing of it is clear. */
function clipToBand(start: number, end: number, band: ClearBand | undefined): [number, number] {
  if (!band) return [start, end];
  const top = Math.max(start, band.top);
  const bottom = Math.min(end, band.bottom);
  return bottom > top ? [top, bottom] : [start, end];
}

/** Bring the match itself into view. Its element may be taller than the
 *  screen, such as a long `<pre>`. So every scroller from the match out to the
 *  page moves by the range's own box, innermost first. Instant, as in every
 *  browser's find: a smooth scroll per keystroke lags the typing.
 *
 *  `band` is the part of the viewport nothing covers. A match under a floating
 *  bar is not in view, so it is centred in what stays clear. */
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

/** Paints matches and the current one. `current` is an index into `ranges`,
 *  or -1 when the current match is not among them. */
export interface FindPainter {
  paint(ranges: Range[], current: number): void;
  clear(): void;
}

/** Put `ranges` under `name`, or take the name away when there are none.
 *  WebKit repaints a range only when its own Highlight drops it, not when the
 *  registry drops the Highlight. So the old one is emptied first, or its
 *  matches stay painted. */
function setHighlight(name: string, ranges: Range[]): void {
  CSS.highlights.get(name)?.clear();
  if (ranges.length > 0) CSS.highlights.set(name, new Highlight(...ranges));
  else CSS.highlights.delete(name);
}

/** Paints with the two CSS Custom Highlights when `highlights()` says they
 *  show, and otherwise selects the current match. One per document: the
 *  highlight registry is the document's. */
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
  /** Run one query or step. The text is re-read every time, because the page
   *  may have re-rendered since the last call. `from` starts the query over,
   *  on the first match at or after that node. */
  find(query: string, step?: 1 | -1, from?: Node): FindResult;
  clear(): void;
}

/** A find session over the text under `root()`, painted by `painter`. `band`
 *  names the part of the viewport nothing floats over, as `revealRange` takes. */
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

/** This frame's own finder, behind the `find` host op. */
const frameFinder = createFinder(() => document.body, createPainter(frameStylesHighlights));

export const find = (query: string, step?: 1 | -1): FindResult => frameFinder.find(query, step);
export const clearFind = (): void => frameFinder.clear();

/** The `find` host op's handler. Its args crossed a `postMessage`. */
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
