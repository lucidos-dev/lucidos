import { CAPTURE_MAX_HEIGHT, rasterize } from './rasterize';

const DOM_MAX = 48000;
const TEXT_MAX = 150;

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
  // Built FIRST, independently of the screenshot, so a rasterizer failure never
  // costs the agent this textual layout too. A blind agent ships UI it cannot
  // see and claims it renders fine. This walk only reads geometry and classes,
  // so it cannot fail on anything the page paints.
  const foldY = Math.min(window.innerHeight || 800, CAPTURE_MAX_HEIGHT) + 200;
  const state = { len: 0 };
  const domSnapshot = serializeNode(document.body, 0, state, foldY);
  const truncated = state.len >= DOM_MAX
    ? `\n[DOM snapshot truncated at ~${Math.round(DOM_MAX / 1000)}KB]` : '';
  const dom = domSnapshot + truncated;

  // The screenshot is best-effort. On any failure, degrade to DOM-only: an
  // empty screenshot and a note, resolving rather than rejecting. The engine's
  // `format_capture_result` drops the image marker for an empty screenshot and
  // passes the DOM text through, so the agent still sees element positions.
  try {
    return { screenshot: await rasterize(document), dom };
  } catch (err) {
    const reason = err instanceof Error ? err.message : String(err);
    return {
      screenshot: '',
      dom: `[screenshot unavailable: ${reason} — verify the layout from the DOM snapshot below, do not assume it renders correctly]\n${dom}`,
    };
  }
}
