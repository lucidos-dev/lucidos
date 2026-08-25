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

/** A REGISTERED device's stored user-agent as a short human label ("Safari on
 *  iOS"), for a device row and for any prose that has to name a device the user
 *  is not holding. Lives here beside {@link isMobileDeviceUserAgent} because both
 *  read the same stored string, and a store action naming a device must not have
 *  to import from a settings component to do it. */
export function describeDeviceUserAgent(ua: string | null | undefined): string {
  if (!ua) return 'Unknown device';
  const browser = ua.match(/(?:Chrome|Firefox|Safari|Edge|Opera)\/[\d.]+/)?.[0]
    || ua.match(/(?:Chrome|Firefox|Safari|Edge|Opera)/)?.[0]
    || 'Unknown browser';
  const os = ua.includes('iPhone') ? 'iOS'
    : ua.includes('iPad') ? 'iPadOS'
    : ua.includes('Mac') ? 'macOS'
    : ua.includes('Android') ? 'Android'
    : ua.includes('Windows') ? 'Windows'
    : ua.includes('Linux') ? 'Linux'
    : 'Unknown OS';
  return `${browser} on ${os}`;
}

/** Is a REGISTERED device's stored user-agent a phone or tablet? Pure, and takes
 *  the string rather than reading `navigator`, because the caller classifies
 *  OTHER devices' `devices.user_agent` as often as its own (see
 *  `otherPushEnabledMobileDevices` in `store/actions/push.ts`).
 *
 *  The desktop-app token is checked first: {@link registrationUserAgent} appends
 *  it to the WKWebView's own UA, which on macOS contains "Macintosh" and would
 *  fall through to false anyway, but stating it keeps the two functions' contract
 *  visible in one place.
 *
 *  Known blind spot, shared with the engine's `parse_user_agent`: an iPad in
 *  Safari's desktop mode reports "Macintosh" and reads as a desktop here. There
 *  is no UA-side fix, so a caller must never treat "not mobile" as a guarantee. */
export function isMobileDeviceUserAgent(ua: string | null | undefined): boolean {
  if (!ua) return false;
  if (ua.includes(DESKTOP_APP_UA_TOKEN)) return false;
  return /iPhone|iPad|iPod|Android/.test(ua);
}

/** Pure: does this served context indicate the packaged Tauri desktop shell
 *  BEFORE `desktop::launch()` has navigated the window to the gateway? Until that
 *  navigation the window sits on Tauri's bundled asset scheme (macOS
 *  `tauri://localhost`; other OSes `http://tauri.localhost`), where every
 *  same-origin API URL + the service-worker registration are invalid and throw
 *  WebKit's "string did not match the expected pattern". The gateway origin is
 *  always `http(s)://localhost:<port>`, and dev Tauri loads `http://localhost:5173`
 *  (devUrl), so this is false there, in the browser, in the PWA, and after the
 *  navigation. Takes its inputs as arguments so it's testable without
 *  `window`/`location`. */
export function isTauriPreGatewayEntryFor(opts: {
  isTauri: boolean;
  protocol: string;
  hostname: string;
}): boolean {
  if (!opts.isTauri) return false;
  // macOS custom asset scheme (`tauri://localhost`) → non-http protocol.
  if (opts.protocol !== 'http:' && opts.protocol !== 'https:') return true;
  // Other OSes serve the bundled assets from `http://tauri.localhost`.
  return opts.hostname === 'tauri.localhost';
}

/** Live-wired {@link isTauriPreGatewayEntryFor} for the current context.
 *  `main.tsx` uses this to STAY on the inline boot splash (status "Starting
 *  Lucidos…") instead of booting `<App/>` against the asset scheme;
 *  `desktop::launch()` navigates the window to the gateway once it is healthy,
 *  which replaces the splash with the real app. */
export function isTauriPreGatewayEntry(): boolean {
  return isTauriPreGatewayEntryFor({
    isTauri: isTauri(),
    protocol: typeof location !== 'undefined' ? location.protocol : 'https:',
    hostname: typeof location !== 'undefined' ? location.hostname : '',
  });
}

/** Check if running on a Mac or iOS device */
export const isMac = /Mac|iPhone|iPad|iPod/.test(navigator.userAgent);

/** Check if running on iOS (iPhone/iPad) — cached, never changes in a session */
const _isIOS = /iPad|iPhone|iPod/.test(navigator.userAgent) ||
  (navigator.platform === 'MacIntel' && navigator.maxTouchPoints > 1);
export function isIOS(): boolean { return _isIOS; }

/** Check if running on Android (cached, never changes in a session).
 *
 *  Sits beside {@link isIOS} and is used the same way: to send someone to the
 *  store for the device they are actually holding. Deliberately does NOT try to
 *  exclude Android tablets or ChromeOS, since the Play Store serves those too.
 *  Matched on the UA rather than `navigator.platform`, which Android reports
 *  inconsistently and which is deprecated. */
const _isAndroid = /Android/.test(navigator.userAgent);
export function isAndroid(): boolean { return _isAndroid; }

/** Is the CURRENT client a phone or tablet? Deliberately NOT
 *  `isMobileDeviceUserAgent(navigator.userAgent)`: for our own client we have a
 *  signal the stored user-agent of a remote device does not give us, and
 *  {@link isIOS} already uses it, so this catches an iPad in Safari's desktop
 *  mode (which reports "Macintosh") where the string test cannot.
 *
 *  Use this for the device in the user's hand, and `isMobileDeviceUserAgent`
 *  for another device's recorded user-agent. */
export function thisDeviceIsMobile(): boolean {
  return isIOS() || isAndroid();
}

/** Check if running as an installed PWA (standalone/fullscreen) — cached */
// `navigator.standalone` is iOS Safari only and missing from the standard Navigator type.
const _isStandalone = window.matchMedia('(display-mode: standalone)').matches ||
  (navigator as Navigator & { standalone?: boolean }).standalone === true;
export function isStandalone(): boolean { return _isStandalone; }

/** Check if running as an iOS standalone PWA — cached */
const _isIOSPwa = _isIOS && _isStandalone;
export function isIOSPwa(): boolean { return _isIOSPwa; }

/** What a client can do with the system pasteboard.
 *
 *  Two abilities, not one: `writeText` is widely available and `readText` is
 *  not, and a surface offering Paste where nothing can read is a control that
 *  fails on tap. Neither exists off a secure origin, which is exactly the
 *  plain-http LAN case. */
export interface ClipboardAbilities {
  copy: boolean;
  paste: boolean;
}

/** Pure {@link clipboardAbilities}, so both answers are testable without a
 *  browser. Takes the clipboard rather than reading `navigator`. */
export function clipboardAbilitiesOf(clipboard: Partial<Clipboard> | undefined): ClipboardAbilities {
  return {
    copy: typeof clipboard?.writeText === 'function',
    paste: typeof clipboard?.readText === 'function',
  };
}

/** Live-wired {@link clipboardAbilitiesOf} for this client. */
export function clipboardAbilities(): ClipboardAbilities {
  return clipboardAbilitiesOf(typeof navigator === 'undefined' ? undefined : navigator.clipboard);
}

/** Pure {@link cameraIsAvailable}: can `getUserMedia` open a camera here?
 *
 *  Both halves are needed, and the second is the one that bites. `mediaDevices`
 *  is absent outside a secure context, but a browser can also expose it and
 *  refuse, so the context is checked in its own right. A LAN origin over plain
 *  http is exactly that case, and a Scan button there fails on tap. */
export function cameraIsAvailableIn(env: {
  mediaDevices: { getUserMedia?: unknown } | undefined;
  secureContext: boolean;
}): boolean {
  return env.secureContext && typeof env.mediaDevices?.getUserMedia === 'function';
}

/** Live-wired {@link cameraIsAvailableIn} for this client. */
export function cameraIsAvailable(): boolean {
  if (typeof navigator === 'undefined' || typeof window === 'undefined') return false;
  return cameraIsAvailableIn({
    mediaDevices: navigator.mediaDevices,
    secureContext: window.isSecureContext === true,
  });
}

/** Check if user prefers reduced motion */
export function prefersReducedMotion(): boolean {
  return window.matchMedia('(prefers-reduced-motion: reduce)').matches;
}

/** Does this device have a hover-capable pointer — i.e. a real mouse/trackpad
 *  plus (in practice) a keyboard, as opposed to a touch-only phone/tablet? The
 *  exact JS mirror of the CSS `@media (hover: hover)` gate, so JS focus behaviour
 *  and CSS focus styling stay in lockstep. Used to skip programmatically moving
 *  focus to a button on touch: there's no keyboard to act on it, so the only
 *  effect is a stray focus ring (iOS WebKit paints `:focus-visible` for
 *  programmatic focus). Focusing a text input is different and stays unguarded —
 *  that raises the on-screen keyboard, which IS the point. */
export function hasHoverPointer(): boolean {
  return window.matchMedia('(hover: hover)').matches;
}
