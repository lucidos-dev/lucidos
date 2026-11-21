// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { hapticTick } from './haptic';

describe('hapticTick', () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it('vibrates where the browser can', () => {
    const vibrate = vi.fn();
    Object.defineProperty(navigator, 'vibrate', { value: vibrate, configurable: true });
    try {
      hapticTick();
      expect(vibrate).toHaveBeenCalledOnce();
      expect(document.head.querySelector('label')).toBeNull();
    } finally {
      delete (navigator as { vibrate?: unknown }).vibrate;
    }
  });

  // An open overlay reads a click with no pointerdown behind it as an outside
  // click and closes. The tick's clicks must never reach a page listener.
  it('toggles the switch without any page click listener seeing it', () => {
    const seen = vi.fn();
    document.addEventListener('click', seen, true);
    document.addEventListener('click', seen);
    let toggled = false;
    const observer = (e: Event) => {
      if ((e.target as HTMLInputElement).type === 'checkbox') toggled = true;
    };
    document.addEventListener('change', observer, true);
    try {
      hapticTick();
      expect(seen).not.toHaveBeenCalled();
      expect(toggled).toBe(true);
      expect(document.head.querySelector('label')).toBeNull();
    } finally {
      document.removeEventListener('click', seen, true);
      document.removeEventListener('click', seen);
      document.removeEventListener('change', observer, true);
    }
  });

  it('leaves every other click alone afterwards', () => {
    hapticTick();
    const seen = vi.fn();
    document.addEventListener('click', seen);
    document.body.click();
    document.removeEventListener('click', seen);
    expect(seen).toHaveBeenCalledOnce();
  });
});
