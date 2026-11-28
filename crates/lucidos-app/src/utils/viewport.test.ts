import { describe, it, expect, afterEach } from 'vitest';
import { viewportIsMobile } from './viewport';

const setWidth = (px: number) => {
  (globalThis as { innerWidth: number }).innerWidth = px;
};

describe('viewportIsMobile self-correction (iOS PWA)', () => {
  afterEach(() => setWidth(1024));

  it('flips to mobile on orientationchange when resize never fires', () => {
    setWidth(390);
    window.dispatchEvent(new Event('orientationchange'));
    expect(viewportIsMobile.value).toBe(true);
  });

  it('flips back to desktop on orientationchange', () => {
    setWidth(390);
    window.dispatchEvent(new Event('orientationchange'));
    expect(viewportIsMobile.value).toBe(true);

    setWidth(1024);
    window.dispatchEvent(new Event('orientationchange'));
    expect(viewportIsMobile.value).toBe(false);
  });

  it('repairs a stale read on pageshow (cold launch)', () => {
    setWidth(390);
    window.dispatchEvent(new Event('pageshow'));
    expect(viewportIsMobile.value).toBe(true);
  });

  it('re-checks on a visibilitychange wake', () => {
    setWidth(390);
    Object.defineProperty(document, 'visibilityState', {
      value: 'visible',
      configurable: true,
    });
    document.dispatchEvent(new Event('visibilitychange'));
    expect(viewportIsMobile.value).toBe(true);
  });

  it('does not flip on a keyboard-only visualViewport resize (layout width unchanged)', () => {
    setWidth(1024);
    window.dispatchEvent(new Event('resize'));
    expect(viewportIsMobile.value).toBe(false);
    window.visualViewport?.dispatchEvent(new Event('resize'));
    expect(viewportIsMobile.value).toBe(false);
  });
});
