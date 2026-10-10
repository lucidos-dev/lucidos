import { describe, it, expect, afterEach } from 'vitest';
import { viewportIsMobile } from './viewport';

const setWidth = (px: number) => {
  (globalThis as { innerWidth: number }).innerWidth = px;
};

// Regression: an iOS standalone PWA can strand the app in the wrong layout.
// App.tsx mounts exactly ONE layout subtree gated on viewportIsMobile, and the
// signal used to update only on `window.resize` — which iOS PWAs frequently
// suppress on rotation, and which never repairs a wrong initial `innerWidth`
// read from a cold launch. The result: the desktop SplitLayout rendered on a
// portrait phone (narrow thread pane + divider + empty content pane). The fix
// re-derives the signal on the broader viewport-change events the app already
// trusts, so it self-corrects instead of staying stuck.
describe('viewportIsMobile self-correction (iOS PWA)', () => {
  // test-setup seeds innerWidth = 1024 (desktop). Restore it so sibling suites
  // that assume the desktop default are unaffected by our mutations.
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
    // Desktop-width layout viewport; the on-screen keyboard would shrink the
    // VISUAL viewport but not window.innerWidth — so no spurious flip.
    setWidth(1024);
    window.dispatchEvent(new Event('resize'));
    expect(viewportIsMobile.value).toBe(false);
    window.visualViewport?.dispatchEvent(new Event('resize'));
    expect(viewportIsMobile.value).toBe(false);
  });
});
