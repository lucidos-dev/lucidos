import { signal } from '@preact/signals';

/** Mobile breakpoint in px — matches @media (max-width: 768px) in CSS. */
const MOBILE_BREAKPOINT = 768;

/** Non-reactive boolean read — does not subscribe the caller. */
export const isMobile = (): boolean => window.innerWidth <= MOBILE_BREAKPOINT;

/** Reactive equivalent of `isMobile()` — components reading `.value` re-render
 *  when the viewport crosses the mobile breakpoint. */
export const viewportIsMobile = signal(isMobile());

window.addEventListener('resize', () => {
  const next = isMobile();
  if (next !== viewportIsMobile.peek()) viewportIsMobile.value = next;
});
