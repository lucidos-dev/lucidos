import { describe, it, expect, beforeEach, vi } from 'vitest';

const isIOS = vi.fn().mockReturnValue(false);
vi.mock('./platform', () => ({
  isIOS: () => isIOS(),
}));

const { isPageActive } = await import('./pageActive');

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
});
