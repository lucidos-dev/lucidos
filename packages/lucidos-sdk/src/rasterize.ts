/**
 * Draw this document's viewport to a JPEG, with no iframe.
 *
 * An app frame is sandboxed at an opaque origin (ADR 0227), and every child
 * frame it makes gets an opaque origin of its own. So this never renders
 * through a child frame: it could not read it back.
 *
 * It hands the browser an SVG image instead: a `<foreignObject>` holding a copy
 * of the page with every element's computed style inlined. The browser renders
 * it, so any CSS it can paint comes out right. An SVG image loads no
 * subresource, so fonts and images ride inside as `data:` URLs
 * (`captureResources.ts`).
 */

import {
  EMBED_BUDGET_MS, createInliner, embeddedFontFaces, fontFamilies, inlineCssUrls, type Inliner,
} from './captureResources';

const XHTML = 'http://www.w3.org/1999/xhtml';
const SVG = 'http://www.w3.org/2000/svg';

/** Elements that paint nothing, or whose job the inlined styles already did. */
const SKIPPED = new Set([
  'head', 'script', 'style', 'link', 'meta', 'title', 'base', 'noscript', 'template', 'source', 'track',
]);

/** Elements whose content this document cannot copy. Each keeps its box. */
const OPAQUE = new Set(['iframe', 'video', 'audio', 'object', 'embed']);

const SCROLLS = /^(auto|scroll)$/;

/** The snapshot is a still, so nothing in it may animate. An animation would
 *  restart at its first frame, and a fade-in rasterizes invisible. */
const STILL = '*,*::before,*::after{animation:none!important;transition:none!important}';

/** Computed properties left out of the copy: custom properties are already
 *  substituted, and motion is stopped by {@link STILL}. */
function isCopied(name: string): boolean {
  return !name.startsWith('--') && !name.startsWith('animation') && !name.startsWith('transition');
}

/** An element's computed style as declarations, in the order the browser lists them. */
export function declarations(style: CSSStyleDeclaration): string {
  let out = '';
  for (let i = 0; i < style.length; i++) {
    const name = style[i];
    if (!isCopied(name)) continue;
    const value = style.getPropertyValue(name);
    if (value) out += `${name}:${value};`;
  }
  return out;
}

/** One class per distinct declaration block, so siblings that look alike share a rule. */
export class StyleSheetBuilder {
  private readonly classes = new Map<string, string>();
  private readonly rules: string[] = [];

  /** The class carrying `body`, as `selectorSuffix` (`''` or a pseudo-element). */
  classFor(body: string, selectorSuffix = ''): string {
    const key = `${selectorSuffix}{${body}`;
    let name = this.classes.get(key);
    if (!name) {
      name = `c${this.classes.size}`;
      this.classes.set(key, name);
      this.rules.push(`.${name}${selectorSuffix}{${body}}`);
    }
    return name;
  }

  css(): string {
    return this.rules.join('\n');
  }
}

/** The page scroll and size the snapshot shows. */
interface Viewport {
  width: number;
  height: number;
  scrollX: number;
  scrollY: number;
}

class Cloner {
  readonly styles = new StyleSheetBuilder();
  readonly families = new Set<string>();
  readonly pending: Promise<void>[] = [];
  /** Fixed and sticky elements, lifted to the root at the place they paint now. */
  readonly lifted: Element[] = [];
  /** Each copy's computed transform, where it has one. */
  private readonly transforms = new WeakMap<Element, string>();

  constructor(
    private readonly doc: Document,
    private readonly inline: Inliner,
    private readonly viewport: Viewport,
  ) {}

  private computed(el: Element, pseudo?: string): CSSStyleDeclaration {
    return (this.doc.defaultView as Window).getComputedStyle(el, pseudo);
  }

  private pseudoClass(el: Element, pseudo: '::before' | '::after'): string | null {
    const style = this.computed(el, pseudo);
    const content = style.getPropertyValue('content');
    if (!content || content === 'none' || content === 'normal') return null;
    // An icon font usually lives here and nowhere else, so its face is used.
    for (const family of fontFamilies(style.fontFamily)) this.families.add(family);
    return this.styles.classFor(declarations(style), pseudo);
  }

  /** A shallow copy carrying no script, no stylesheet hook and no lazy source. */
  private shell(source: Element, tag: string): Element {
    const copy = OPAQUE.has(tag) ? this.doc.createElementNS(XHTML, 'div') : source.cloneNode(false) as Element;
    for (const attr of Array.from(copy.attributes)) {
      const name = attr.name.toLowerCase();
      if (name === 'class' || name === 'style' || name === 'srcset' || name === 'sizes' || name.startsWith('on')) {
        copy.removeAttribute(attr.name);
      }
    }
    return copy;
  }

  /** Carry what the user typed or picked, which lives in properties, not attributes. */
  private copyState(source: Element, copy: Element): void {
    if (source instanceof HTMLInputElement) {
      if (source.type === 'checkbox' || source.type === 'radio') {
        if (source.checked) copy.setAttribute('checked', '');
        else copy.removeAttribute('checked');
      } else if (source.type !== 'file' && source.type !== 'password') {
        copy.setAttribute('value', source.value);
      }
    } else if (source instanceof HTMLTextAreaElement) {
      copy.textContent = source.value;
    } else if (source instanceof HTMLOptionElement) {
      if (source.selected) copy.setAttribute('selected', '');
      else copy.removeAttribute('selected');
    } else if (source instanceof HTMLImageElement) {
      this.embed(copy, 'src', source.currentSrc || source.src);
    } else if (source instanceof SVGImageElement) {
      copy.removeAttribute('xlink:href');
      const href = source.href.baseVal;
      this.embed(copy, 'href', href ? new URL(href, this.doc.baseURI).href : '');
    }
  }

  /** Point `attr` at `url`'s bytes once read, or at nothing if unreadable. */
  private embed(copy: Element, attr: string, url: string): void {
    copy.removeAttribute(attr);
    if (!url) return;
    this.pending.push(this.inline(url).then((data) => {
      if (data) copy.setAttribute(attr, data);
    }));
  }

  /** A canvas is pixels, not DOM, so it travels as a picture of itself. */
  private canvasImage(source: HTMLCanvasElement): Element {
    const img = this.doc.createElementNS(XHTML, 'img');
    try {
      img.setAttribute('src', source.toDataURL());
    } catch {
      // A canvas holding another origin's pixels refuses to export. Its box stays.
    }
    return img;
  }

  /** Offset a scrolled box's children, since the copy has no scroll position.
   *  The offset goes in front of a child's own transform, never over it. */
  private applyScroll(source: Element, copy: Element): void {
    const isRoot = source === this.doc.documentElement;
    const left = isRoot ? this.viewport.scrollX : source.scrollLeft;
    const top = isRoot ? this.viewport.scrollY : source.scrollTop;
    if (!left && !top) return;
    for (const child of Array.from(copy.children)) {
      const own = this.transforms.get(child);
      (child as HTMLElement).style.transform = `translate(${-left}px,${-top}px)${own ? ` ${own}` : ''}`;
    }
  }

  /** Pin a sticky element where its box is now, ready to lift to the root. */
  private pinned(source: Element, copy: HTMLElement): HTMLElement {
    const rect = source.getBoundingClientRect();
    copy.style.position = 'absolute';
    copy.style.top = `${rect.top}px`;
    copy.style.left = `${rect.left}px`;
    copy.style.right = 'auto';
    copy.style.bottom = 'auto';
    copy.style.margin = '0';
    return copy;
  }

  clone(source: Element): Element | null {
    const tag = source.localName;
    const inSvg = source.namespaceURI === SVG;
    if (!inSvg && SKIPPED.has(tag)) return null;
    const style = this.computed(source);
    // Inside an SVG a hidden `<symbol>` or `<defs>` is still what a `<use>` draws.
    if (!inSvg && style.display === 'none') return null;

    const copy = source instanceof HTMLCanvasElement ? this.canvasImage(source) : this.shell(source, tag);
    const classes = [this.styles.classFor(declarations(style))];
    for (const pseudo of ['::before', '::after'] as const) {
      const name = this.pseudoClass(source, pseudo);
      if (name) classes.push(name);
    }
    copy.setAttribute('class', classes.join(' '));
    for (const family of fontFamilies(style.fontFamily)) this.families.add(family);
    this.copyState(source, copy);

    if (!OPAQUE.has(tag) && !(source instanceof HTMLTextAreaElement) && !(source instanceof HTMLCanvasElement)) {
      for (const child of Array.from(source.childNodes)) {
        if (child.nodeType === Node.TEXT_NODE) copy.appendChild(this.doc.createTextNode(child.textContent ?? ''));
        else if (child.nodeType === Node.ELEMENT_NODE) {
          const childCopy = this.clone(child as Element);
          if (childCopy) copy.appendChild(childCopy);
        }
      }
    }
    if (style.transform && style.transform !== 'none') this.transforms.set(copy, style.transform);
    this.applyScroll(source, copy);
    // The copy fakes scrolling with offsets, so a scroll box only clips. Left
    // scrollable, it would draw scrollbars the page does not show.
    if (copy instanceof HTMLElement && (SCROLLS.test(style.overflowX) || SCROLLS.test(style.overflowY))) {
      copy.style.overflow = 'hidden';
    }

    // The root copy is exactly the viewport, so a fixed element lifted to it
    // keeps its own geometry. Left in place, a scrolled ancestor's offset would
    // carry it away.
    if (style.position === 'fixed') {
      this.lifted.push(copy);
      return null;
    }
    if (style.position === 'sticky') {
      // Whether stuck or not, it paints where its box is now. It still holds
      // its place in the flow, so a hidden copy stays behind and what follows
      // does not move up. Opacity, because every descendant's class sets its
      // own `visibility`.
      const placeholder = copy.cloneNode(true) as HTMLElement;
      placeholder.style.opacity = '0';
      this.lifted.push(this.pinned(source, copy as HTMLElement));
      return placeholder;
    }
    return copy;
  }
}

/** The colour the page paints behind everything, for a JPEG that has no alpha. */
function backdrop(doc: Document): string {
  const view = doc.defaultView as Window;
  for (const el of [doc.documentElement, doc.body]) {
    if (!el) continue;
    const colour = view.getComputedStyle(el).backgroundColor;
    if (colour && colour !== 'transparent' && !/rgba\(.*,\s*0\)$/.test(colour)) return colour;
  }
  return '#ffffff';
}

function decode(src: string): Promise<HTMLImageElement> {
  const img = new Image();
  img.src = src;
  return img.decode().then(() => img);
}

/** The largest picture a capture produces, in CSS pixels after scaling. */
export const CAPTURE_MAX_WIDTH = 1024;
export const CAPTURE_MAX_HEIGHT = 1200;

/** The viewport as a base64 JPEG, scaled down to {@link CAPTURE_MAX_WIDTH}. */
export async function rasterize(doc: Document): Promise<string> {
  const until = performance.now() + EMBED_BUDGET_MS;
  const inline = createInliner(until);
  const root = doc.documentElement;
  const view = doc.defaultView as Window;
  const viewport: Viewport = {
    width: root.clientWidth || view.innerWidth,
    height: root.clientHeight || view.innerHeight,
    scrollX: view.scrollX,
    scrollY: view.scrollY,
  };
  const cloner = new Cloner(doc, inline, viewport);
  const page = cloner.clone(root) as HTMLElement;
  page.style.overflow = 'hidden';
  page.style.width = `${viewport.width}px`;
  page.style.height = `${viewport.height}px`;
  for (const lifted of cloner.lifted) page.appendChild(lifted);
  // Every resource read shares one deadline, so a slow one costs its own
  // picture and never the capture.
  const [css, fonts] = await Promise.all([
    inlineCssUrls(cloner.styles.css(), doc.baseURI, inline),
    embeddedFontFaces(cloner.families, inline, until),
    ...cloner.pending,
  ]);
  const sheet = doc.createElementNS(XHTML, 'style');
  sheet.textContent = `${fonts}\n${STILL}\n${css}`;
  const head = doc.createElementNS(XHTML, 'head');
  head.appendChild(sheet);
  page.insertBefore(head, page.firstChild);

  const { width, height } = viewport;
  const markup = new XMLSerializer().serializeToString(page);
  const svg = `<svg xmlns="${SVG}" width="${width}" height="${height}">`
    + `<foreignObject x="0" y="0" width="100%" height="100%">${markup}</foreignObject></svg>`;
  const image = await decode(`data:image/svg+xml;charset=utf-8,${encodeURIComponent(svg)}`);

  const scale = Math.min(1, CAPTURE_MAX_WIDTH / width);
  const canvas = doc.createElement('canvas');
  canvas.width = Math.round(width * scale);
  canvas.height = Math.round(Math.min(height * scale, CAPTURE_MAX_HEIGHT));
  const ctx = canvas.getContext('2d');
  if (!ctx) throw new Error('2D canvas unavailable');
  ctx.fillStyle = backdrop(doc);
  ctx.fillRect(0, 0, canvas.width, canvas.height);
  ctx.drawImage(image, 0, 0, width * scale, height * scale);
  return canvas.toDataURL('image/jpeg', 0.65).split(',')[1];
}
