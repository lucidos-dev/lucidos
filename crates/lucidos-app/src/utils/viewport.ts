import { signal } from '@preact/signals';
import { PHONE_LAYOUT_QUERY, hasCoarsePointer, viewportGetsPhoneLayout } from './layoutMedia';

export { hasCoarsePointer };

export const isMobile = (): boolean => viewportGetsPhoneLayout();

export const isTouchDevice = (): boolean =>
  'ontouchstart' in window || navigator.maxTouchPoints > 0;

export const isMobileOrTouch = (): boolean =>
  isMobile() || ('ontouchstart' in window && navigator.maxTouchPoints > 0);

export const viewportIsMobile = signal(isMobile());

const syncViewportIsMobile = () => {
  const next = isMobile();
  if (next !== viewportIsMobile.peek()) viewportIsMobile.value = next;
};

window.addEventListener('resize', syncViewportIsMobile);
window.addEventListener('orientationchange', syncViewportIsMobile);
window.addEventListener('pageshow', syncViewportIsMobile);
if (typeof document !== 'undefined' && typeof document.addEventListener === 'function') {
  document.addEventListener('visibilitychange', () => {
    if (document.visibilityState === 'visible') syncViewportIsMobile();
  });
}
window.visualViewport?.addEventListener('resize', syncViewportIsMobile);
window.matchMedia?.(PHONE_LAYOUT_QUERY).addEventListener?.('change', syncViewportIsMobile);

export { isEventInViewport as isInViewport, isReplyEndInViewport, lastExchangeOf } from '../components/chat/scrollStateHandle';
