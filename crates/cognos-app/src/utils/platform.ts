/** Check if running inside Tauri webview (v2 uses __TAURI_INTERNALS__) */
export function isTauri(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
}

/** Check if running on a Mac or iOS device */
export const isMac = /Mac|iPhone|iPad|iPod/.test(navigator.userAgent);

/** Check if running on iOS (iPhone/iPad) — cached, never changes in a session */
const _isIOS = /iPad|iPhone|iPod/.test(navigator.userAgent) ||
  (navigator.platform === 'MacIntel' && navigator.maxTouchPoints > 1);
export function isIOS(): boolean { return _isIOS; }

/** Check if running as an installed PWA (standalone/fullscreen) — cached */
// `navigator.standalone` is iOS Safari only and missing from the standard Navigator type.
const _isStandalone = window.matchMedia('(display-mode: standalone)').matches ||
  (navigator as Navigator & { standalone?: boolean }).standalone === true;
export function isStandalone(): boolean { return _isStandalone; }

/** Check if running as an iOS standalone PWA — cached */
const _isIOSPwa = _isIOS && _isStandalone;
export function isIOSPwa(): boolean { return _isIOSPwa; }

/** Check if user prefers reduced motion */
export function prefersReducedMotion(): boolean {
  return window.matchMedia('(prefers-reduced-motion: reduce)').matches;
}
