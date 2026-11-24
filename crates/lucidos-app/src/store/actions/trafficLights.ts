import { isTauri } from '../../utils/platform';
import { setTrafficLightOffset } from '../../utils/tauri';

let lastPushedPx = 0;

export function resetTrafficLightPush(): void {
  lastPushedPx = 0;
}

const ROUNDING_SLACK_PX = 1;

export const TITLEBAR_BAND_SELECTOR = '[data-titlebar-band]';

export function measureHeaderBarHeight(): number | null {
  const header = document.querySelector(TITLEBAR_BAND_SELECTOR) as HTMLElement | null;
  if (!header) return null;
  const bottom = header.getBoundingClientRect().bottom;
  if (!Number.isFinite(bottom) || bottom <= 0) return null;
  return bottom < header.offsetHeight - ROUNDING_SLACK_PX ? null : bottom;
}

function hasNativeLights(): boolean {
  return isTauri() && document.documentElement.hasAttribute('data-titlebar-overlay');
}

export function watchTitlebarBand(): () => void {
  const band = document.querySelector(TITLEBAR_BAND_SELECTOR);
  if (!band || !isTauri()) return () => {};
  const push = (): void => pushTrafficLightOffset();
  const observer = new ResizeObserver(push);
  observer.observe(band, { box: 'border-box' });
  window.addEventListener('resize', push);
  return () => {
    observer.disconnect();
    window.removeEventListener('resize', push);
  };
}

export function pushTrafficLightOffset(): void {
  if (!hasNativeLights()) return;
  const barHeightPx = measureHeaderBarHeight();
  if (barHeightPx === null || barHeightPx === lastPushedPx) return;
  lastPushedPx = barHeightPx;
  setTrafficLightOffset(barHeightPx).catch((e) => {
    lastPushedPx = 0;
    console.warn('[titlebar] traffic-light placement failed', e);
  });
}
