/**
 * The *widget fit check*'s measurement, run inside a widget frame.
 *
 * The host's probe pushes a `measure` token with each size. The frame waits for
 * its layout to settle, then reports what the box cuts off. A finding names the
 * edge, the px past it, the element and its text. The engine turns that into a
 * note the agent can act on.
 */

/** How long the DOM and the body's size must hold still to count as settled. */
export const SETTLE_QUIET_MS = 300;

/** A widget that never holds still is measured at this point anyway. */
export const SETTLE_CAP_MS = 4000;

/** Findings a report carries per box. `total` still counts every one. */
export const MAX_FIT_FINDINGS = 8;

/** Characters of text a finding quotes. */
const QUOTE_CHARS = 60;

/** Layout rounding a fractional box absorbs before anything counts as cut. */
const SLACK_PX = 1;

export interface Rect {
  left: number;
  top: number;
  right: number;
  bottom: number;
}

export const FIT_EDGES = ['top', 'right', 'bottom', 'left'] as const;

export type FitEdge = (typeof FIT_EDGES)[number];

/** `past-edge`: text past an edge of the box. `clipped`: text an
 *  `overflow: hidden` or `clip` ancestor cuts inside the box. `ellipsis`: text
 *  an ellipsis cuts. `media`: an image, SVG, canvas or video past an edge. */
export const FIT_KINDS = ['past-edge', 'clipped', 'ellipsis', 'media'] as const;

export type FitKind = (typeof FIT_KINDS)[number];

export interface FitFinding {
  kind: FitKind;
  edge: FitEdge;
  /** Whole CSS pixels past the edge. */
  px: number;
  /** A short element path, such as `div.row > span`. */
  path: string;
  /** The first characters of the element's text, or an image's alt text. */
  text: string;
}

export interface FitResult {
  findings: FitFinding[];
  total: number;
}

/** Where things are drawn. The real one reads the layout; a test stubs it. */
export interface FitGeometry {
  box(): { width: number; height: number };
  rect(el: Element): Rect;
  textRect(text: Text): Rect | null;
}

export const domGeometry: FitGeometry = {
  box: () => ({ width: document.documentElement.clientWidth, height: document.documentElement.clientHeight }),
  rect: (el) => el.getBoundingClientRect(),
  textRect: (text) => {
    const range = document.createRange();
    range.selectNodeContents(text);
    const r = range.getBoundingClientRect();
    return r.width === 0 && r.height === 0 ? null : r;
  },
};

const MEDIA = 'img, svg, canvas, video';

/** The edge of `edges` that `inner` runs furthest past `outer`, if any runs
 *  past the slack. */
function overshoot(inner: Rect, outer: Rect, edges: readonly FitEdge[] = FIT_EDGES): { edge: FitEdge; px: number } | null {
  const past: Array<[FitEdge, number]> = (
    [
      ['top', outer.top - inner.top],
      ['right', inner.right - outer.right],
      ['bottom', inner.bottom - outer.bottom],
      ['left', outer.left - inner.left],
    ] as Array<[FitEdge, number]>
  ).filter(([edge]) => edges.includes(edge));
  const [edge, px] = past.reduce((a, b) => (b[1] > a[1] ? b : a), ['top', 0]);
  return px > SLACK_PX ? { edge, px: Math.round(px) } : null;
}

function quote(text: string | null | undefined): string {
  return (text ?? '').replace(/\s+/g, ' ').trim().slice(0, QUOTE_CHARS);
}

function step(el: Element): string {
  const tag = el.tagName.toLowerCase();
  const cls = el.classList[0];
  return cls ? `${tag}.${cls}` : tag;
}

function isBox(el: Element): boolean {
  return el === el.ownerDocument.body || el === el.ownerDocument.documentElement;
}

/** The element and its parent, short enough to read in a note. */
function pathOf(el: Element): string {
  const parent = el.parentElement;
  return parent && !isBox(parent) ? `${step(parent)} > ${step(el)}` : step(el);
}

function styleOf(el: Element): CSSStyleDeclaration {
  return el.ownerDocument.defaultView!.getComputedStyle(el);
}

const clips = (value: string) => value === 'hidden' || value === 'clip';

/** The edges `el` cuts its content at: those of each axis whose overflow
 *  clips. The other axis may still scroll or show its content. */
function clippedEdges(el: Element): FitEdge[] {
  const style = styleOf(el);
  return [
    ...(clips(style.overflowY) ? (['top', 'bottom'] as const) : []),
    ...(clips(style.overflowX) ? (['left', 'right'] as const) : []),
  ];
}

function cutByEllipsis(el: Element): boolean {
  return styleOf(el).getPropertyValue('text-overflow') === 'ellipsis' && el.scrollWidth - el.clientWidth > SLACK_PX;
}

/** The element and its ancestors below the body, since `<html>` and `<body>`
 *  are the box itself. */
function ancestorsInBox(el: Element): Element[] {
  const chain: Element[] = [];
  for (let at: Element | null = el; at && !isBox(at); at = at.parentElement) chain.push(at);
  return chain;
}

function textNodes(doc: Document): Text[] {
  const walker = doc.createTreeWalker(doc.body, NodeFilter.SHOW_TEXT);
  const nodes: Text[] = [];
  for (let node = walker.nextNode(); node; node = walker.nextNode()) nodes.push(node as Text);
  return nodes;
}

/** What the box cuts off: ellipsis cuts, then text, then media. Text inside an
 *  element already reported as an ellipsis cut is not reported again. */
export function measureFit(doc: Document, geometry: FitGeometry = domGeometry): FitResult {
  if (!doc.body) return { findings: [], total: 0 };
  const { width, height } = geometry.box();
  const box: Rect = { left: 0, top: 0, right: width, bottom: height };
  const found: FitFinding[] = [];

  const ellipsized = new Set<Element>();
  for (const el of doc.body.querySelectorAll('*')) {
    if (!cutByEllipsis(el)) continue;
    ellipsized.add(el);
    const px = Math.round(el.scrollWidth - el.clientWidth);
    found.push({ kind: 'ellipsis', edge: 'right', px, path: pathOf(el), text: quote(el.textContent) });
  }

  for (const text of textNodes(doc)) {
    const owner = text.parentElement;
    if (!owner || !text.data.trim() || styleOf(owner).visibility === 'hidden') continue;
    const chain = ancestorsInBox(owner);
    if (chain.some((el) => ellipsized.has(el))) continue;
    const r = geometry.textRect(text);
    if (!r) continue;
    const past = overshoot(r, box);
    if (past) {
      found.push({ kind: 'past-edge', ...past, path: pathOf(owner), text: quote(text.data) });
      continue;
    }
    for (const el of chain) {
      const edges = clippedEdges(el);
      const cut = edges.length > 0 ? overshoot(r, geometry.rect(el), edges) : null;
      if (!cut) continue;
      found.push({ kind: 'clipped', ...cut, path: pathOf(owner), text: quote(text.data) });
      break;
    }
  }

  for (const el of doc.body.querySelectorAll(MEDIA)) {
    const past = overshoot(geometry.rect(el), box);
    if (past) found.push({ kind: 'media', ...past, path: pathOf(el), text: quote(el.getAttribute('alt')) });
  }

  return { findings: found.slice(0, MAX_FIT_FINDINGS), total: found.length };
}

/** Resolves once the fonts are ready and the DOM and the body's size then hold
 *  still for {@link SETTLE_QUIET_MS}. Never later than {@link SETTLE_CAP_MS}
 *  after the call. */
export function settle(doc: Document): Promise<void> {
  return new Promise((resolve) => {
    let quiet: ReturnType<typeof setTimeout> | undefined;
    let mutations: MutationObserver | undefined;
    let resizes: ResizeObserver | undefined;
    let finished = false;
    const done = () => {
      finished = true;
      clearTimeout(quiet);
      clearTimeout(cap);
      mutations?.disconnect();
      resizes?.disconnect();
      resolve();
    };
    const cap = setTimeout(done, SETTLE_CAP_MS);
    const restart = () => {
      clearTimeout(quiet);
      quiet = setTimeout(done, SETTLE_QUIET_MS);
    };
    const fontsReady = doc.fonts?.ready ?? Promise.resolve();
    void fontsReady.then(() => {
      if (finished) return;
      mutations = new MutationObserver(restart);
      mutations.observe(doc.documentElement, { subtree: true, childList: true, attributes: true, characterData: true });
      if (doc.body && typeof ResizeObserver === 'function') {
        resizes = new ResizeObserver(restart);
        resizes.observe(doc.body);
      }
      restart();
    });
  });
}
