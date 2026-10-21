// @vitest-environment jsdom
import { describe, it, expect, beforeAll, afterAll, afterEach, vi } from 'vitest';
import { installNativeContextMenuPolicy } from './nativeContextMenu';

function el(html: string, pick = '[data-pick]'): Element {
  const host = document.createElement('div');
  host.innerHTML = html;
  document.body.appendChild(host);
  return host.querySelector(pick) ?? host.firstElementChild!;
}

describe('installNativeContextMenuPolicy (desktop app)', () => {
  beforeAll(() => {
    (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
    installNativeContextMenuPolicy();
  });
  afterAll(() => {
    delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
  });

  const rightClick = (target: Element, init: MouseEventInit = {}) => {
    const e = new MouseEvent('contextmenu', { bubbles: true, cancelable: true, ...init });
    target.dispatchEvent(e);
    return e;
  };

  it('cancels the native menu over chrome', () => {
    expect(rightClick(el('<span data-pick>header</span>')).defaultPrevented).toBe(true);
  });

  const menuPress = (target: Element, x = 5, y = 5) =>
    target.dispatchEvent(new MouseEvent('pointerdown', { bubbles: true, button: 2, clientX: x, clientY: y }));

  it('keeps it over the text of rendered markdown', () => {
    const p = el('<div class="markdown-content"><p data-pick>Hi</p></div>');
    vi.spyOn(document, 'createRange').mockReturnValue({
      selectNodeContents: () => {},
      getClientRects: () => [{ left: 0, right: 100, top: 0, bottom: 20 }],
    } as unknown as Range);
    menuPress(p);
    const e = rightClick(p, { clientX: 5, clientY: 5 });
    vi.restoreAllMocks();
    expect(e.defaultPrevented).toBe(false);
  });

  it('cancels it over the empty space in the transcript', () => {
    const gap = el('<div data-native-context-menu><div data-pick></div></div>');
    menuPress(gap);
    expect(rightClick(gap, { clientX: 5, clientY: 5 }).defaultPrevented).toBe(true);
  });

  it('keeps it for a menu the keyboard raised in the transcript', () => {
    const focused = el('<div data-native-context-menu><div data-pick></div></div>');
    expect(rightClick(focused).defaultPrevented).toBe(false);
  });

  it('still cancels a keyboard-raised menu on chrome', () => {
    expect(rightClick(el('<span data-pick>header</span>')).defaultPrevented).toBe(true);
  });

  describe('the selection WebKit makes on the way in', () => {
    // A selection whose one rect sits at (10..60, 20..36).
    const fakeSelection = (collapsed: boolean) => {
      const removeAllRanges = vi.fn();
      const sel = {
        isCollapsed: collapsed,
        rangeCount: collapsed ? 0 : 1,
        getRangeAt: () => ({ getClientRects: () => [{ left: 10, right: 60, top: 20, bottom: 36 }] }),
        removeAllRanges,
      } as unknown as Selection;
      return { sel, removeAllRanges };
    };
    const press = (target: Element, init: MouseEventInit) =>
      target.dispatchEvent(new MouseEvent('pointerdown', { bubbles: true, ...init }));
    afterEach(() => vi.restoreAllMocks());

    it('is cleared when the press started with no selection', () => {
      const target = el('<span data-pick>header</span>');
      const before = fakeSelection(true);
      const after = fakeSelection(false);
      const spy = vi.spyOn(window, 'getSelection').mockReturnValue(before.sel);
      press(target, { button: 2, clientX: 30, clientY: 28 });
      spy.mockReturnValue(after.sel);
      const e = rightClick(target, { clientX: 30, clientY: 28 });
      expect(e.defaultPrevented).toBe(true);
      expect(after.removeAllRanges).toHaveBeenCalledTimes(1);
    });

    it('keeps the native menu, and the selection, when the user selected first', () => {
      const target = el('<span data-pick>Settings</span>');
      const held = fakeSelection(false);
      vi.spyOn(window, 'getSelection').mockReturnValue(held.sel);
      press(target, { button: 2, clientX: 30, clientY: 28 });
      const e = rightClick(target, { clientX: 30, clientY: 28 });
      expect(e.defaultPrevented).toBe(false);
      expect(held.removeAllRanges).not.toHaveBeenCalled();
    });

    it('leaves a selection elsewhere alone', () => {
      const target = el('<span data-pick>header</span>');
      const elsewhere = fakeSelection(false);
      vi.spyOn(window, 'getSelection').mockReturnValue(elsewhere.sel);
      press(target, { button: 2, clientX: 300, clientY: 300 });
      const e = rightClick(target, { clientX: 300, clientY: 300 });
      expect(e.defaultPrevented).toBe(true);
      expect(elsewhere.removeAllRanges).not.toHaveBeenCalled();
    });

    it('an ordinary click never reads the selection', () => {
      const spy = vi.spyOn(window, 'getSelection');
      press(el('<span data-pick>x</span>'), { button: 0, clientX: 30, clientY: 28 });
      expect(spy).not.toHaveBeenCalled();
    });
  });

  it('never stops a claimed event from reaching other listeners', () => {
    const target = el('<div data-pick>row</div>');
    let reached = false;
    const after = () => { reached = true; };
    window.addEventListener('contextmenu', after);
    target.addEventListener('contextmenu', (e) => e.preventDefault());
    rightClick(target);
    window.removeEventListener('contextmenu', after);
    expect(reached).toBe(true);
  });
});
