import { apiUrl } from './_fetch';

type Html2Canvas = (element: HTMLElement, opts: Record<string, unknown>) => Promise<HTMLCanvasElement>;

const DOM_MAX = 48000;
const TEXT_MAX = 150;

let html2canvasPromise: Promise<Html2Canvas> | undefined;

function loadHtml2Canvas(): Promise<Html2Canvas> {
  if (html2canvasPromise) return html2canvasPromise;
  const existing = (window as { html2canvas?: Html2Canvas }).html2canvas;
  if (existing) {
    html2canvasPromise = Promise.resolve(existing);
    return html2canvasPromise;
  }
  // Resolved per call rather than at module load, because `apiUrl` reads a base
  // URL that `configure({ baseUrl })` can still change.
  const scriptUrl = apiUrl('/static/html2canvas.min.js');
  // Clear the cached promise on failure so a later call can retry. Without
  // this, transient network errors poison capture for the iframe's lifetime.
  const promise = new Promise<Html2Canvas>((resolve, reject) => {
    const fail = (err: Error) => {
      if (html2canvasPromise === promise) html2canvasPromise = undefined;
      reject(err);
    };
    const script = document.createElement('script');
    script.src = scriptUrl;
    script.async = true;
    script.onload = () => {
      const fn = (window as { html2canvas?: Html2Canvas }).html2canvas;
      if (fn) resolve(fn);
      else fail(new Error('html2canvas script loaded but window.html2canvas undefined'));
    };
    script.onerror = () => fail(new Error(`Failed to load ${scriptUrl}`));
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

  // Build the DOM snapshot FIRST, independently of html2canvas. html2canvas
  // throws on CSS Color 4 functions (color(), oklab(), oklch(), color-mix())
  // that modern stylesheets routinely use — and if it ran first and threw, the
  // whole capture rejected, so the agent lost this textual layout too and went
  // fully blind (it would then ship UI it couldn't see and claim it "renders
  // fine"). This DOM walk only reads geometry + classes, so it can't fail on
  // CSS the rasterizer can't parse — it's the reliable fallback.
  const foldY = vpH + 200;
  const state = { len: 0 };
  const domSnapshot = serializeNode(document.body, 0, state, foldY);
  const truncated = state.len >= DOM_MAX
    ? `\n[DOM snapshot truncated at ~${Math.round(DOM_MAX / 1000)}KB]` : '';
  const dom = domSnapshot + truncated;

  // Screenshot is best-effort. On any failure (unsupported CSS color function,
  // script load error) degrade to DOM-only: empty screenshot + a note, and
  // resolve rather than reject. The engine's `format_capture_result` drops the
  // image marker when the screenshot is empty and passes the DOM text through,
  // so the agent still sees the rendered layout (element positions reveal
  // overlaps/clipping) instead of just an error string.
  try {
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
    return { screenshot, dom };
  } catch (err) {
    const reason = err instanceof Error ? err.message : String(err);
    return {
      screenshot: '',
      dom: `[screenshot unavailable: ${reason} — verify the layout from the DOM snapshot below, do not assume it renders correctly]\n${dom}`,
    };
  }
}
