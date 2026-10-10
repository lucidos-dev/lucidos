import { describe, it, expect, afterEach } from 'vitest';
import { watchPageAway } from './pageVisit';

function setVisibility(state: DocumentVisibilityState) {
  Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => state });
  document.dispatchEvent(new Event('visibilitychange'));
}

afterEach(() => {
  setVisibility('visible');
});

describe('watchPageAway', () => {
  it('reads false while the page stays visible', () => {
    const away = watchPageAway();
    expect(away()).toBe(false);
  });

  it('reads true while the page is hidden', () => {
    const away = watchPageAway();
    setVisibility('hidden');
    expect(away()).toBe(true);
  });

  it('remembers a hide the page has since come back from', () => {
    // The iOS freeze: hidden mid-probe, visible again by the time it settles.
    const away = watchPageAway();
    setVisibility('hidden');
    setVisibility('visible');
    expect(away()).toBe(true);
  });

  it('counts a page that was already hidden when the watch began', () => {
    setVisibility('hidden');
    const away = watchPageAway();
    setVisibility('visible');
    expect(away()).toBe(true);
  });

  it('ignores a hide that ended before the watch began', () => {
    setVisibility('hidden');
    setVisibility('visible');
    const away = watchPageAway();
    expect(away()).toBe(false);
  });
});
