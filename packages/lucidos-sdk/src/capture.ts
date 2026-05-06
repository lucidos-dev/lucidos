type Html2Canvas = (element: HTMLElement, opts: Record<string, unknown>) => Promise<HTMLCanvasElement>;

const DOM_MAX = 48000;
const TEXT_MAX = 150;
const HTML2CANVAS_URL = '/api/static/html2canvas.min.js';

let html2canvasPromise: Promise<Html2Canvas> | undefined;

function loadHtml2Canvas(): Promise<Html2Canvas> {
  if (html2canvasPromise) return html2canvasPromise;
  const existing = (window as { html2canvas?: Html2Canvas }).html2canvas;
  if (existing) {
    html2canvasPromise = Promise.resolve(existing);
    return html2canvasPromise;
  }
  // Clear the cached promise on failure so a later call can retry — without
  // this, transient network errors poison capture for the iframe's lifetime.
  const promise = new Promise<Html2Canvas>((resolve, reject) => {
    const fail = (err: Error) => {
      if (html2canvasPromise === promise) html2canvasPromise = undefined;
      reject(err);
    };
    const script = document.createElement('script');
    script.src = HTML2CANVAS_URL;
    script.async = true;
    script.onload = () => {
      const fn = (window as { html2canvas?: Html2Canvas }).html2canvas;
      if (fn) resolve(fn);
      else fail(new Error('html2canvas script loaded but window.html2canvas undefined'));
    };
    script.onerror = () => fail(new Error(`Failed to load ${HTML2CANVAS_URL}`));
    document.head.appendChild(script);
  });
  html2canvasPromise = promise;
  return promise;
}

function serializeNode(el: Node, depth: number, state: { len: number }, foldY: number): string {
  if (state.len >= DOM_MAX || depth > 8) return '';

  if (el.nodeType === 3) {
    let t = (el.textContent || '').trim();
    if (!t) return '';
    if (t.length > TEXT_MAX) t = t.substring(0, TEXT_MAX) + '...';
    state.len += t.length;
    return t;
  }

  if (el.nodeType !== 1) return '';
  const htmlEl = el as HTMLElement;
  const tag = htmlEl.tagName.toLowerCase();
  if (tag === 'script' || tag === 'style' || tag === 'link' || tag === 'svg') return '';

  const rect = htmlEl.getBoundingClientRect();
  if (rect.y > foldY && rect.width > 0) return '';

  const attrs: string[] = [];
  if (htmlEl.id) attrs.push(`id="${htmlEl.id}"`);
  if (htmlEl.className && typeof htmlEl.className === 'string' && htmlEl.className.trim())
    attrs.push(`class="${htmlEl.className.trim()}"`);

  const indent = '  '.repeat(depth);
  let out = `${indent}<${tag}${attrs.length ? ' ' + attrs.join(' ') : ''} [${Math.round(rect.x)},${Math.round(rect.y)} ${Math.round(rect.width)}x${Math.round(rect.height)}]>`;
  state.len += out.length;

  const children: string[] = [];
  for (let i = 0; i < htmlEl.childNodes.length; i++) {
    if (state.len >= DOM_MAX) break;
    const s = serializeNode(htmlEl.childNodes[i], depth + 1, state, foldY);
    if (s) children.push(s);
  }
  if (children.length) {
    out += '\n' + children.join('\n');
  }
  return out;
}

export async function capture(): Promise<{ screenshot: string; dom: string }> {
  const vpW = Math.min(document.body.scrollWidth, 1024);
  const vpH = Math.min(window.innerHeight || 800, 1200);

  const html2canvas = await loadHtml2Canvas();
  const canvas = await html2canvas(document.body, {
    scale: 1,
    useCORS: true,
    logging: false,
    width: vpW,
    height: vpH,
    windowWidth: vpW,
    windowHeight: vpH,
    y: window.scrollY,
  });
  const screenshot = canvas.toDataURL('image/jpeg', 0.65).split(',')[1];

  const foldY = vpH + 200;
  const state = { len: 0 };
  const dom = serializeNode(document.body, 0, state, foldY);
  const truncated = state.len >= DOM_MAX
    ? `\n[DOM snapshot truncated at ~${Math.round(DOM_MAX / 1000)}KB]` : '';

  return { screenshot, dom: dom + truncated };
}
