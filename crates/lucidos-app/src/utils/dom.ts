export function focusIfNeeded(el: HTMLElement | null | undefined): void {
  if (el && document.activeElement !== el) {
    el.focus({ preventScroll: true });
  }
}

export function keepFocusOnPress(e: Event): void {
  e.preventDefault();
}

export function isTextInput(el: EventTarget | Element | null): boolean {
  if (!(el instanceof HTMLElement)) return false;
  if (el.isContentEditable) return true;
  const tag = el.tagName;
  return tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT';
}

export function isInteractiveTarget(el: EventTarget | null): boolean {
  if (!(el instanceof Element)) return false;
  return !!el.closest('button, a, input, textarea, select, label, [role="button"], [contenteditable]');
}

export function isThreadTranscript(el: EventTarget | null): boolean {
  return el instanceof HTMLElement && el.closest('.thread-content') !== null;
}

const KEYBOARD_INPUT_TYPES = new Set([
  'text', 'search', 'email', 'password', 'tel', 'url', 'number',
]);

export function opensSoftwareKeyboard(el: EventTarget | Element | null): boolean {
  if (!(el instanceof HTMLElement)) return false;
  if (el.isContentEditable) return true;
  if (el.tagName === 'TEXTAREA') return true;
  if (el.tagName !== 'INPUT') return false;
  return KEYBOARD_INPUT_TYPES.has((el as HTMLInputElement).type);
}

const KEYBOARD_SHRINK_MIN_PX = 100;

function isViewportReading(vvHeight: number): boolean {
  return vvHeight > 0;
}

export function readableVisualViewport(): VisualViewport | null {
  const vv = window.visualViewport;
  return vv && isViewportReading(vv.height) ? vv : null;
}

export function viewportIsKeyboardShrunk(vvHeight: number, innerHeight: number): boolean {
  return isViewportReading(vvHeight) && vvHeight < innerHeight - KEYBOARD_SHRINK_MIN_PX;
}

export function getRemPx(): number {
  return parseFloat(getComputedStyle(document.documentElement).fontSize) || 16;
}

export function matrixTranslateX(transform: string): number {
  const m = transform.match(/matrix(?:3d)?\(([^)]+)\)/);
  if (!m) return 0;
  const v = m[1].split(',').map((s) => parseFloat(s.trim()));
  if (v.length === 6) return v[4];
  if (v.length === 16) return v[12];
  return 0;
}

export function computedTranslateX(el: Element): number {
  return matrixTranslateX(getComputedStyle(el).transform);
}

let safeAreaProbe: HTMLElement | null = null;

export function safeAreaTopPx(): number {
  if (typeof document === 'undefined' || !document.body) return 0;
  if (!safeAreaProbe?.isConnected) {
    safeAreaProbe = document.createElement('div');
    safeAreaProbe.setAttribute('aria-hidden', 'true');
    safeAreaProbe.style.cssText =
      'position:fixed;top:0;left:0;visibility:hidden;pointer-events:none;padding-top:var(--safe-area-top)';
    document.body.appendChild(safeAreaProbe);
  }
  return parseFloat(getComputedStyle(safeAreaProbe).paddingTop) || 0;
}

export { clampWithin } from '@lucidos/geometry';

export function autoResizeTextarea(el: HTMLTextAreaElement | null) {
  if (!el) return;
  el.style.height = 'auto';
  const cs = getComputedStyle(el);
  const borderY = (parseFloat(cs.borderTopWidth) || 0) + (parseFloat(cs.borderBottomWidth) || 0);
  el.style.height = `${el.scrollHeight + borderY}px`;
}
