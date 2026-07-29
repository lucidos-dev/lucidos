import { signal } from '@preact/signals';

/** Mobile breakpoint in px — matches @media (max-width: 768px) in CSS. */
const MOBILE_BREAKPOINT = 768;

/** Non-reactive boolean read — does not subscribe the caller. */
export const isMobile = (): boolean => window.innerWidth <= MOBILE_BREAKPOINT;

/** True when the device exposes touch input. Inclusive OR so any touch
 *  indicator counts — used by the body `.is-touch` class which controls
 *  hover-only styling (the cost of a false positive is a copy button
 *  staying visible on a desktop Chrome with a stale ontouchstart shim). */
export const isTouchDevice = (): boolean =>
  'ontouchstart' in window || navigator.maxTouchPoints > 0;

/** True when the device is mobile-sized OR confirmed touch-capable. The
 *  scroll-lock helper keys off this — iOS Safari ignores overflow:hidden on
 *  body and needs the position:fixed trick. Uses the STRICT touch check
 *  (both indicators) so desktop Chrome with `ontouchstart` exposed but no
 *  touch hardware doesn't trigger a visible scroll-jump on every modal open. */
export const isMobileOrTouch = (): boolean =>
  isMobile() || ('ontouchstart' in window && navigator.maxTouchPoints > 0);

/** Reactive equivalent of `isMobile()` — components reading `.value` re-render
 *  when the viewport crosses the mobile breakpoint. */
export const viewportIsMobile = signal(isMobile());

/** Re-derive from the live `window.innerWidth`. Cheap: peeks + compares, and
 *  only writes (waking subscribers) when the breakpoint side actually flips. */
const syncViewportIsMobile = () => {
  const next = isMobile();
  if (next !== viewportIsMobile.peek()) viewportIsMobile.value = next;
};

// `window.resize` ALONE is unreliable on an iOS standalone PWA: it often does
// not fire on rotation, and the initial `innerWidth` read at module-eval time
// can be wrong during a cold launch (e.g. launched in landscape, or a transient
// pre-layout value). Since App.tsx now mounts only ONE layout subtree gated on
// this signal (perf: no dual-mount fan-out), a stale value strands the app in
// the WRONG layout — the desktop SplitLayout rendered on a portrait phone — for
// the whole session, because nothing ever re-checks. Listen to the same broad
// set of viewport signals the app already trusts for `--app-height`
// (MobileSwipeContainer) so the signal self-corrects on the next rotation / wake
// and App re-mounts into the correct layout. Store signals are module-level, so
// the layout swap preserves all thread/pane state.
window.addEventListener('resize', syncViewportIsMobile);
window.addEventListener('orientationchange', syncViewportIsMobile);
window.addEventListener('pageshow', syncViewportIsMobile);
// visibilitychange is a `document` event — the canonical "PWA resumed from
// background" wake signal. Guarded because this module runs its listener
// registration as an import-time side effect, and the minimal unit-test
// environment can present a `document` without addEventListener.
if (typeof document !== 'undefined' && typeof document.addEventListener === 'function') {
  document.addEventListener('visibilitychange', () => {
    if (document.visibilityState === 'visible') syncViewportIsMobile();
  });
}
// visualViewport.resize is the most reliable "viewport changed" signal on iOS.
// It also fires on keyboard show/hide, but syncViewportIsMobile reads the LAYOUT
// viewport (window.innerWidth), which the keyboard doesn't shrink — so no
// spurious breakpoint flip.
window.visualViewport?.addEventListener('resize', syncViewportIsMobile);

/** `true` when the event with the given `data-event-id` is currently in
 *  the visible viewport on this device. Filters dual-mount copies so a
 *  hidden layout's offscreen card doesn't accidentally count.
 *
 *  Backed by the production-grade implementation in
 *  `components/chat/scrollState.ts`; re-exported here so consumers
 *  (notification routing, audit logging) can pull from `utils/viewport`
 *  without reaching into chat internals. See
 *  `system-knowhow/notifications.md` §3 — used to compute
 *  `event_in_viewport` in the PresenceCheck pong.
 */
export { isEventInViewport as isInViewport } from '../components/chat/scrollState';
