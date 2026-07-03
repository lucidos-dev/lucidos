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

window.addEventListener('resize', () => {
  const next = isMobile();
  if (next !== viewportIsMobile.peek()) viewportIsMobile.value = next;
});

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
