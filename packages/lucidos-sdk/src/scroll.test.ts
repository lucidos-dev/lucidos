import { describe, it, expect, beforeEach, vi } from 'vitest';
import {
  parseAppId,
  scrollKey,
  parseSavedScroll,
  isFullyRestorable,
  installScrollMemory,
} from './scroll';

describe('parseAppId', () => {
  it('extracts app id from canonical app path', () => {
    expect(parseAppId('/api/app/habit-tracker/')).toBe('habit-tracker');
  });

  it('extracts app id when path has subroutes', () => {
    expect(parseAppId('/api/app/habit-tracker/index.html')).toBe('habit-tracker');
  });

  it('extracts app id when path has nested subroutes', () => {
    expect(parseAppId('/api/app/habit-tracker/foo/bar')).toBe('habit-tracker');
  });

  it('decodes percent-encoded segments', () => {
    expect(parseAppId('/api/app/my%20app/')).toBe('my app');
  });

  it('returns null for non-app paths', () => {
    expect(parseAppId('/index.html')).toBeNull();
    expect(parseAppId('/api/v1/preferences')).toBeNull();
    expect(parseAppId('/api/v1/sdk.js')).toBeNull();
    expect(parseAppId('/api/apps')).toBeNull();
    expect(parseAppId('/')).toBeNull();
  });

  it('returns null when app id segment is empty', () => {
    expect(parseAppId('/api/app//')).toBeNull();
  });
});

describe('scrollKey', () => {
  it('namespaces by app id', () => {
    expect(scrollKey('habit-tracker')).toBe('lucidos-scroll-app-habit-tracker');
  });

  it('produces distinct keys for distinct apps', () => {
    expect(scrollKey('a')).not.toBe(scrollKey('b'));
  });
});

describe('parseSavedScroll', () => {
  it('parses non-negative integer string', () => {
    expect(parseSavedScroll('250')).toBe(250);
  });

  it('returns null for null/empty/garbage/negative', () => {
    expect(parseSavedScroll(null)).toBeNull();
    expect(parseSavedScroll('')).toBeNull();
    expect(parseSavedScroll('abc')).toBeNull();
    expect(parseSavedScroll('-5')).toBeNull();
    expect(parseSavedScroll('NaN')).toBeNull();
  });

  it('floors fractional values', () => {
    expect(parseSavedScroll('250.7')).toBe(250);
  });
});

describe('isFullyRestorable', () => {
  it('true when scrollable range covers saved offset', () => {
    expect(isFullyRestorable(200, 1000, 500)).toBe(true);
  });

  it('true at boundary', () => {
    expect(isFullyRestorable(500, 1000, 500)).toBe(true);
  });

  it('false when content has not grown enough', () => {
    expect(isFullyRestorable(300, 600, 500)).toBe(false);
  });

  it('false for non-positive saved', () => {
    expect(isFullyRestorable(0, 1000, 500)).toBe(false);
    expect(isFullyRestorable(-1, 1000, 500)).toBe(false);
  });
});

describe('installScrollMemory', () => {
  beforeEach(() => {
    sessionStorage.clear();
    Object.defineProperty(window, 'location', {
      value: { pathname: '/api/app/habit-tracker/', hash: '' },
      writable: true,
      configurable: true,
    });
    (window as any).scrollTo = vi.fn();
    Object.defineProperty(window, 'scrollY', { value: 0, writable: true, configurable: true });
    Object.defineProperty(document.documentElement, 'scrollHeight', {
      value: 5000,
      writable: true,
      configurable: true,
    });
    Object.defineProperty(document.documentElement, 'clientHeight', {
      value: 800,
      writable: true,
      configurable: true,
    });
  });

  it('no-ops when pathname is not an app path', () => {
    Object.defineProperty(window, 'location', {
      value: { pathname: '/', hash: '' },
      writable: true,
      configurable: true,
    });
    sessionStorage.setItem('lucidos-scroll-app-habit-tracker', '500');
    const cleanup = installScrollMemory();
    expect(window.scrollTo).not.toHaveBeenCalled();
    cleanup();
  });

  it('restores saved scroll on install when content is tall enough', () => {
    sessionStorage.setItem('lucidos-scroll-app-habit-tracker', '500');
    const cleanup = installScrollMemory();
    expect(window.scrollTo).toHaveBeenCalledWith(0, 500);
    cleanup();
  });

  it('does not restore when location.hash is present (anchor wins)', () => {
    sessionStorage.setItem('lucidos-scroll-app-habit-tracker', '500');
    Object.defineProperty(window, 'location', {
      value: { pathname: '/api/app/habit-tracker/', hash: '#section-2' },
      writable: true,
      configurable: true,
    });
    const cleanup = installScrollMemory();
    expect(window.scrollTo).not.toHaveBeenCalled();
    cleanup();
  });

  it('does not restore when no saved value exists', () => {
    const cleanup = installScrollMemory();
    expect(window.scrollTo).not.toHaveBeenCalled();
    cleanup();
  });

  it('saves scrollY on pagehide', () => {
    Object.defineProperty(window, 'scrollY', { value: 320, writable: true, configurable: true });
    const cleanup = installScrollMemory();
    window.dispatchEvent(new Event('pagehide'));
    expect(sessionStorage.getItem('lucidos-scroll-app-habit-tracker')).toBe('320');
    cleanup();
  });

  it('removes saved value when scrollY is 0 at save time', () => {
    sessionStorage.setItem('lucidos-scroll-app-habit-tracker', '500');
    Object.defineProperty(window, 'scrollY', { value: 0, writable: true, configurable: true });
    const cleanup = installScrollMemory();
    window.dispatchEvent(new Event('pagehide'));
    expect(sessionStorage.getItem('lucidos-scroll-app-habit-tracker')).toBeNull();
    cleanup();
  });

  it('cleanup removes listeners (no save after cleanup)', () => {
    const cleanup = installScrollMemory();
    cleanup();
    Object.defineProperty(window, 'scrollY', { value: 999, writable: true, configurable: true });
    window.dispatchEvent(new Event('pagehide'));
    expect(sessionStorage.getItem('lucidos-scroll-app-habit-tracker')).toBeNull();
  });
});
