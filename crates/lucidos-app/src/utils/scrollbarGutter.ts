
const CSS_VAR = '--scrollbar-gutter-width';

const TRANSCRIPT_SELECTOR = '.thread-content';

const PROBE_OVERFLOW = 'overflow-x:hidden;overflow-y:scroll;scrollbar-gutter:stable;';

const PROBE_BASE =
  'position:absolute;top:-9999px;left:-9999px;width:100px;height:100px;visibility:hidden;';

function reservedWidth(el: HTMLElement, style: CSSStyleDeclaration): number {
  const borders = parseFloat(style.borderLeftWidth) + parseFloat(style.borderRightWidth);
  const reserved = el.offsetWidth - el.clientWidth - (Number.isFinite(borders) ? borders : 0);
  return Number.isFinite(reserved) && reserved > 0 ? reserved : 0;
}

function measureLiveTranscript(doc: Document): number | null {
  const view = doc.defaultView;
  if (!view || typeof view.getComputedStyle !== 'function') return null;
  if (typeof doc.querySelectorAll !== 'function') return null;
  const els = doc.querySelectorAll<HTMLElement>(TRANSCRIPT_SELECTOR);
  for (let i = 0; i < els.length; i++) {
    const el = els[i];
    if (!(el.offsetWidth > 0)) continue;
    const style = view.getComputedStyle(el);
    if (style.overflowY !== 'scroll' && style.overflowY !== 'auto') continue;
    return reservedWidth(el, style);
  }
  return null;
}

function measureProbe(doc: Document): number {
  const host = doc.body ?? doc.documentElement;
  const el = doc.createElement('div');
  if (!host || typeof host.appendChild !== 'function' || !el || !el.style) return 0;
  el.style.cssText = PROBE_BASE + PROBE_OVERFLOW;
  host.appendChild(el);
  const reserved = el.offsetWidth - el.clientWidth;
  el.remove();
  return Number.isFinite(reserved) && reserved > 0 ? reserved : 0;
}

export function measureScrollbarGutter(doc: Document = document): number {
  return measureLiveTranscript(doc) ?? measureProbe(doc);
}

export function publishScrollbarGutter(doc: Document = document): number {
  const gutter = measureScrollbarGutter(doc);
  doc.documentElement.style.setProperty(CSS_VAR, `${gutter}px`);
  return gutter;
}
