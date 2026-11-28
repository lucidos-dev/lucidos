/**
 * The drawn caret: the composer's `caret-shape` where the browser has none
 * (ADR 0317). Chromium draws the part natively; WebKit and Firefox do not.
 *
 * The overlay is a layout copy of the textarea: the same box, text metrics
 * and wrap rules, holding the text before the caret, a caret span, then the
 * rest. So the browser places the caret, not our arithmetic. Its text fill is
 * transparent; only the caret span paints (`host-components.css`).
 *
 * A temporary measure: see `docs/temporary-measures.md` § Drawn caret.
 */

export type DrawnCaretShape = 'block' | 'underscore';

const SHAPE_TOKEN = '--part-composer-text-caret-shape';
const COLOUR_TOKEN = '--part-composer-text-caret-color';
const NATIVE_ATTR = 'data-drawn-caret';

const COPIED_PROPERTIES = [
  'box-sizing', 'width', 'height',
  'padding-top', 'padding-right', 'padding-bottom', 'padding-left',
  'border-top-width', 'border-right-width', 'border-bottom-width', 'border-left-width',
  'font-family', 'font-size', 'font-weight', 'font-style', 'font-stretch',
  'font-variant', 'font-feature-settings', 'font-variation-settings', 'font-kerning',
  'letter-spacing', 'word-spacing', 'line-height', 'text-indent', 'text-align',
  'text-transform', 'tab-size', 'direction', 'unicode-bidi',
];

export function drawnCaretShape(
  token: string,
  nativeCaretShape: boolean,
  inProtectedSurface: boolean,
): DrawnCaretShape | null {
  if (nativeCaretShape || inProtectedSurface) return null;
  return token === 'block' || token === 'underscore' ? token : null;
}

function browserDrawsCaretShape(): boolean {
  return typeof CSS !== 'undefined' && CSS.supports('caret-shape', 'block');
}

const graphemes = typeof Intl.Segmenter === 'function' ? new Intl.Segmenter() : null;

function characterAt(text: string, index: number): string {
  if (index >= text.length) return '';
  const grapheme = graphemes?.segment(text).containing(index);
  if (grapheme?.index === index) return grapheme.segment;
  return String.fromCodePoint(text.codePointAt(index)!);
}

function coveredText(textarea: HTMLTextAreaElement, at: number): string {
  const value = textarea.value;
  if (value === '') return characterAt(textarea.placeholder, 0);
  const under = characterAt(value, at);
  return under === '\n' ? '' : under;
}

export function attachDrawnCaret(
  textarea: HTMLTextAreaElement,
  { nativeCaretShape = browserDrawsCaretShape() }: { nativeCaretShape?: boolean } = {},
): () => void {
  if (nativeCaretShape) return () => {};

  let overlay: HTMLDivElement | null = null;
  let content: HTMLDivElement | null = null;
  let composing = false;
  let frame: number | null = null;

  function remove() {
    overlay?.remove();
    overlay = null;
    content = null;
    textarea.removeAttribute(NATIVE_ATTR);
  }

  function ensureOverlay(): { overlay: HTMLDivElement; content: HTMLDivElement } {
    if (!overlay || !content) {
      overlay = document.createElement('div');
      overlay.className = 'drawn-caret';
      overlay.setAttribute('aria-hidden', 'true');
      content = document.createElement('div');
      content.className = 'drawn-caret-content';
      overlay.append(content);
    }
    if (overlay.previousElementSibling !== textarea) textarea.after(overlay);
    return { overlay, content };
  }

  function draw(shape: DrawnCaretShape) {
    const box = ensureOverlay();
    const style = getComputedStyle(textarea);
    for (const name of COPIED_PROPERTIES) box.overlay.style.setProperty(name, style.getPropertyValue(name));
    const parent = textarea.parentElement!;
    const from = parent.getBoundingClientRect();
    const to = textarea.getBoundingClientRect();
    box.overlay.style.top = `${to.top - from.top - parent.clientTop}px`;
    box.overlay.style.left = `${to.left - from.left - parent.clientLeft}px`;
    box.content.style.transform = `translate(${-textarea.scrollLeft}px, ${-textarea.scrollTop}px)`;

    box.overlay.style.color = style.color;
    box.overlay.style.setProperty('--drawn-caret-color', style.getPropertyValue(COLOUR_TOKEN).trim() || style.color);
    box.overlay.style.setProperty('--drawn-caret-ink', getComputedStyle(parent.closest('.prompt-box') ?? parent).backgroundColor);

    const at = textarea.selectionEnd;
    const covered = coveredText(textarea, at);
    const cursor = document.createElement('span');
    cursor.className = 'drawn-caret-cursor';
    cursor.dataset.shape = shape;
    cursor.textContent = covered;
    const rest = textarea.value === '' ? '' : textarea.value.slice(at + covered.length);
    box.content.replaceChildren(textarea.value.slice(0, at), cursor, rest);
    box.overlay.hidden = false;
  }

  function sync() {
    const token = getComputedStyle(textarea).getPropertyValue(SHAPE_TOKEN).trim();
    const inProtected = textarea.closest('.protected-surface') !== null;
    const shape = textarea.isConnected ? drawnCaretShape(token, false, inProtected) : null;
    if (shape === null) {
      remove();
      return;
    }
    textarea.toggleAttribute(NATIVE_ATTR, !composing);
    const collapsed = textarea.selectionStart === textarea.selectionEnd;
    if (document.activeElement === textarea && collapsed && !composing) draw(shape);
    else if (overlay) overlay.hidden = true;
  }

  function schedule() {
    if (frame === null) frame = requestAnimationFrame(() => { frame = null; sync(); });
  }

  function onCompositionStart() {
    composing = true;
    schedule();
  }

  function onCompositionEnd() {
    composing = false;
    schedule();
  }

  const textareaEvents = ['input', 'keydown', 'pointerup', 'select', 'scroll', 'focus', 'blur'];
  for (const type of textareaEvents) textarea.addEventListener(type, schedule);
  textarea.addEventListener('compositionstart', onCompositionStart);
  textarea.addEventListener('compositionend', onCompositionEnd);
  document.addEventListener('selectionchange', schedule);
  document.fonts?.addEventListener('loadingdone', schedule);
  const resizes = new ResizeObserver(schedule);
  resizes.observe(textarea);
  const themes = new MutationObserver(schedule);
  themes.observe(document.documentElement, { attributes: true });
  schedule();

  return () => {
    if (frame !== null) cancelAnimationFrame(frame);
    frame = null;
    for (const type of textareaEvents) textarea.removeEventListener(type, schedule);
    textarea.removeEventListener('compositionstart', onCompositionStart);
    textarea.removeEventListener('compositionend', onCompositionEnd);
    document.removeEventListener('selectionchange', schedule);
    document.fonts?.removeEventListener('loadingdone', schedule);
    resizes.disconnect();
    themes.disconnect();
    remove();
  };
}
