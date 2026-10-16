import { describe, it, expect, beforeEach, vi } from 'vitest';

const isIOS = vi.fn().mockReturnValue(false);
vi.mock('./platform', () => ({
  isIOS: () => isIOS(),
}));

const { isPageActive } = await import('./pageActive');
const { setNativeWindowActive } = await import('./nativeWindow');

function setVisibility(state: 'visible' | 'hidden'): void {
  Object.defineProperty(document, 'visibilityState', { configurable: true, value: state });
}

function setHasFocus(focused: boolean): void {
  (document as unknown as { hasFocus: () => boolean }).hasFocus = () => focused;
}

describe('isPageActive', () => {
  beforeEach(() => {
    isIOS.mockReset().mockReturnValue(false);
    setVisibility('visible');
    setHasFocus(true);
    // Default native-window state is active; flipped per-test below. (Always
    // true in the browser / PWA where no native bridge ever fires.)
    setNativeWindowActive(true);
  });

  it('desktop: true when visible AND focused', () => {
    expect(isPageActive()).toBe(true);
  });

  it('desktop: false when hidden, even with focus', () => {
    setVisibility('hidden');
    expect(isPageActive()).toBe(false);
  });

  it('desktop: false when visible but window not focused (covered by another app)', () => {
    setHasFocus(false);
    expect(isPageActive()).toBe(false);
  });

  it('iOS: true when visible, even with hasFocus()=false (the reported bug)', () => {
    isIOS.mockReturnValue(true);
    setHasFocus(false);
    expect(isPageActive()).toBe(true);
  });

  it('iOS: false when visibilityState is hidden, regardless of hasFocus', () => {
    isIOS.mockReturnValue(true);
    setVisibility('hidden');
    setHasFocus(true);
    expect(isPageActive()).toBe(false);
  });

  it('Tauri: false when the native window is inactive (trayed / unfocused), even when visible+focused', () => {
    // The WKWebView still reports visible+focused while trayed via orderOut: —
    // the native-active bridge is what makes it report not-in-use. The reported
    // bug: a trayed/unfocused desktop client got a suppressed, invisible toast
    // instead of an OS native banner.
    setNativeWindowActive(false);
    expect(isPageActive()).toBe(false);
  });

  it('Tauri: true again once the native window becomes active (reshown / refocused)', () => {
    setNativeWindowActive(false);
    setNativeWindowActive(true);
    expect(isPageActive()).toBe(true);
  });
});
