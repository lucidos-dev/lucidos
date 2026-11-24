
import {
  EMBED_BUDGET_MS, createInliner, embeddedFontFaces, fontFamilies, inlineCssUrls, type Inliner,
} from './captureResources';

const XHTML = 'http://www.w3.org/1999/xhtml';
const SVG = 'http://www.w3.org/2000/svg';

const SKIPPED = new Set([
  'head', 'script', 'style', 'link', 'meta', 'title', 'base', 'noscript', 'template', 'source', 'track',
]);

const OPAQUE = new Set(['iframe', 'video', 'audio', 'object', 'embed']);

const SCROLLS = /^(auto|scroll)$/;

const STILL = '*,*::before,*::after{animation:none!important;transition:none!important}';

function isCopied(name: string): boolean {
  return !name.startsWith('--') && !name.startsWith('animation') && !name.startsWith('transition');
}

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

export class StyleSheetBuilder {
  private readonly classes = new Map<string, string>();
  private readonly rules: string[] = [];

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
  readonly lifted: Element[] = [];
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
    for (const family of fontFamilies(style.fontFamily)) this.families.add(family);
    return this.styles.classFor(declarations(style), pseudo);
  }

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

  private embed(copy: Element, attr: string, url: string): void {
    copy.removeAttribute(attr);
    if (!url) return;
    this.pending.push(this.inline(url).then((data) => {
      if (data) copy.setAttribute(attr, data);
    }));
  }

  private canvasImage(source: HTMLCanvasElement): Element {
    const img = this.doc.createElementNS(XHTML, 'img');
    try {
      img.setAttribute('src', source.toDataURL());
    } catch {
    }
    return img;
  }

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
    if (copy instanceof HTMLElement && (SCROLLS.test(style.overflowX) || SCROLLS.test(style.overflowY))) {
      copy.style.overflow = 'hidden';
    }

    if (style.position === 'fixed') {
      this.lifted.push(copy);
      return null;
    }
    if (style.position === 'sticky') {
      const placeholder = copy.cloneNode(true) as HTMLElement;
      placeholder.style.opacity = '0';
      this.lifted.push(this.pinned(source, copy as HTMLElement));
      return placeholder;
    }
    return copy;
  }
}

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

export const CAPTURE_MAX_WIDTH = 1024;
export const CAPTURE_MAX_HEIGHT = 1200;

export const SAVED_MAX_PIXEL_RATIO = 2;
export const SAVED_MAX_EDGE = 2048;

export const CAPTURE_QUALITY = 0.65;
export const SAVED_QUALITY = 0.92;

export const CAPTURE_TYPE = 'image/jpeg';

export interface Rendering {
  scale: number;
  width: number;
  height: number;
  type: string;
  quality: number;
}

export function renderingFor(
  width: number, height: number, pixelRatio: number, format?: string,
): Rendering {
  if (!format) {
    const scale = Math.min(1, CAPTURE_MAX_WIDTH / width);
    return {
      scale,
      width: Math.round(width * scale),
      height: Math.round(Math.min(height * scale, CAPTURE_MAX_HEIGHT)),
      type: CAPTURE_TYPE,
      quality: CAPTURE_QUALITY,
    };
  }
  const ratio = Math.min(Math.max(pixelRatio, 1), SAVED_MAX_PIXEL_RATIO);
  const scale = Math.min(ratio, SAVED_MAX_EDGE / Math.max(width, height));
  return {
    scale,
    width: Math.round(width * scale),
    height: Math.round(height * scale),
    type: `image/${format}`,
    quality: SAVED_QUALITY,
  };
}

export async function rasterize(doc: Document, format?: string): Promise<string> {
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

  const rendering = renderingFor(width, height, view.devicePixelRatio || 1, format);
  const canvas = doc.createElement('canvas');
  canvas.width = rendering.width;
  canvas.height = rendering.height;
  const ctx = canvas.getContext('2d');
  if (!ctx) throw new Error('2D canvas unavailable');
  ctx.fillStyle = backdrop(doc);
  ctx.fillRect(0, 0, canvas.width, canvas.height);
  ctx.drawImage(image, 0, 0, width * rendering.scale, height * rendering.scale);
  return canvas.toDataURL(rendering.type, rendering.quality).split(',')[1];
}
