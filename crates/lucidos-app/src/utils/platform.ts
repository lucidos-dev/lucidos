/** Check if running inside Tauri webview (v2 uses __TAURI_INTERNALS__) */
export function isTauri(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
}

/** Product token appended to the user-agent string we send at device
 *  registration when running in the Tauri native desktop app. The embedded
 *  WKWebView's real `navigator.userAgent` is indistinguishable from Safari, so
 *  without this the engine (and the Lucidos Agent's device context) can't tell a
 *  desktop client from a browser — and the agent wrongly gives browser-permission
 *  notification advice. The engine's `parse_user_agent` recognizes this token and
 *  renders the device as "Lucidos desktop app on <OS>". Standard product-token
 *  practice; kept to ONLY the registration string, never the live navigator UA. */
export const DESKTOP_APP_UA_TOKEN = 'Lucidos-Desktop';

/** The user-agent string to register this device with: the raw `navigator.userAgent`,
 *  plus the desktop-app product token when running as the native desktop client.
 *  Pure (takes the client flag as an argument) so it's unit-testable without `window`. */
export function registrationUserAgent(rawUa: string, isDesktopApp: boolean): string {
  return isDesktopApp ? `${rawUa} ${DESKTOP_APP_UA_TOKEN}` : rawUa;
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
