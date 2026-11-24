
let lastScrollInputAt = -Infinity;
let installed = false;

export const USER_SCROLL_WINDOW_MS = 1200;

export function nowMs(): number {
  return typeof performance !== 'undefined' && typeof performance.now === 'function'
    ? performance.now()
    : Date.now();
}

const SCROLL_KEYS = new Set([
  'ArrowUp', 'ArrowDown', 'PageUp', 'PageDown', 'Home', 'End', ' ', 'Spacebar',
]);

export function isScrollKeyPress(e: KeyboardEvent): boolean {
  if (e.metaKey || e.ctrlKey || e.altKey) return false;
  return SCROLL_KEYS.has(e.key);
}

const KEY_CONSUMING_TARGET = 'input,textarea,select,button,a,[contenteditable]:not([contenteditable="false"])';

function onScrollInput(): void {
  lastScrollInputAt = nowMs();
}

function onScrollKey(e: KeyboardEvent): void {
  if (!isScrollKeyPress(e)) return;
  if (e.target instanceof Element && e.target.closest(KEY_CONSUMING_TARGET)) return;
  onScrollInput();
}

function ensureInstalled(): void {
  if (installed || typeof document === 'undefined' || typeof document.addEventListener !== 'function') return;
  installed = true;
  document.addEventListener('touchmove', onScrollInput, { passive: true, capture: true });
  document.addEventListener('wheel', onScrollInput, { passive: true, capture: true });
  document.addEventListener('keydown', onScrollKey, { passive: true, capture: true });
}

export function isUserScrolling(): boolean {
  ensureInstalled();
  return nowMs() - lastScrollInputAt < USER_SCROLL_WINDOW_MS;
}
