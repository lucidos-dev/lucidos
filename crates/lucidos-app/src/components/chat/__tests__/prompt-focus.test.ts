import { describe, it, expect, vi } from 'vitest';

if (typeof globalThis.requestAnimationFrame === 'undefined') {
  (globalThis as any).requestAnimationFrame = (cb: any) => { cb(); return 0; };
}

import { focusIfNeeded, focusPromptNow, composeHandlers } from '../promptFocus';

describe('focusIfNeeded', () => {
  it('calls focus({ preventScroll: true }) when element is not the active element', () => {
    const el = { focus: vi.fn() } as any;
    (globalThis as any).document = { activeElement: null };

    focusIfNeeded(el);

    expect(el.focus).toHaveBeenCalledWith({ preventScroll: true });
  });

  it('skips focus() when element is already the active element', () => {
    const el = { focus: vi.fn() } as any;
    (globalThis as any).document = { activeElement: el };

    focusIfNeeded(el);

    expect(el.focus).not.toHaveBeenCalled();
  });

  it('does nothing when element is null', () => {
    (globalThis as any).document = { activeElement: null };
    // Should not throw
    focusIfNeeded(null);
  });
});

describe('focusPromptNow', () => {
  it('focuses the visible prompt-input element (non-zero width)', () => {
    const hiddenEl = { focus: vi.fn(), getBoundingClientRect: () => ({ width: 0, height: 0 }) } as any;
    const visibleEl = { focus: vi.fn(), getBoundingClientRect: () => ({ width: 300, height: 40 }) } as any;
    (globalThis as any).document = {
      ...document,
      querySelectorAll: vi.fn().mockReturnValue([hiddenEl, visibleEl]),
    };

    focusPromptNow();

    expect(hiddenEl.focus).not.toHaveBeenCalled();
    expect(visibleEl.focus).toHaveBeenCalledOnce();
  });

  it('uses preventScroll to avoid iOS Safari auto-scrolling overflow:hidden containers', () => {
    const visibleEl = { focus: vi.fn(), getBoundingClientRect: () => ({ width: 300, height: 40 }) } as any;
    (globalThis as any).document = {
      ...document,
      querySelectorAll: vi.fn().mockReturnValue([visibleEl]),
    };

    focusPromptNow();

    expect(visibleEl.focus).toHaveBeenCalledWith({ preventScroll: true });
  });

  it('falls back to last element when none have dimensions', () => {
    const el1 = { focus: vi.fn(), getBoundingClientRect: () => ({ width: 0, height: 0 }) } as any;
    const el2 = { focus: vi.fn(), getBoundingClientRect: () => ({ width: 0, height: 0 }) } as any;
    (globalThis as any).document = {
      ...document,
      querySelectorAll: vi.fn().mockReturnValue([el1, el2]),
    };

    focusPromptNow();

    expect(el1.focus).not.toHaveBeenCalled();
    expect(el2.focus).toHaveBeenCalledOnce();
  });

  it('does nothing when no prompt-input elements exist', () => {
    (globalThis as any).document = {
      ...document,
      querySelectorAll: vi.fn().mockReturnValue([]),
    };

    // Should not throw
    focusPromptNow();
  });
});

describe('composeHandlers', () => {
  function mockDoc() {
    const visibleEl = {
      focus: vi.fn(),
      getBoundingClientRect: () => ({ width: 300, height: 40 }),
    } as any;
    (globalThis as any).document = {
      ...document,
      querySelectorAll: vi.fn().mockReturnValue([visibleEl]),
    };
    return visibleEl;
  }

  it('onTouchEnd focuses BEFORE action — iOS gesture window must not be spent on re-renders', () => {
    const el = mockDoc();
    const callOrder: string[] = [];
    const action = vi.fn(() => callOrder.push('action'));
    el.focus.mockImplementation(() => callOrder.push('focus'));

    const handlers = composeHandlers(action);
    handlers.onTouchEnd({ preventDefault: vi.fn() } as any);

    expect(callOrder).toEqual(['focus', 'action']);
  });

  it('onTouchEnd prevents default to suppress delayed click', () => {
    mockDoc();
    const handlers = composeHandlers(vi.fn());
    const event = { preventDefault: vi.fn() } as any;

    handlers.onTouchEnd(event);

    expect(event.preventDefault).toHaveBeenCalled();
  });

  it('onClick focuses BEFORE action for consistency', () => {
    const el = mockDoc();
    const callOrder: string[] = [];
    const action = vi.fn(() => callOrder.push('action'));
    el.focus.mockImplementation(() => callOrder.push('focus'));

    const handlers = composeHandlers(action);
    handlers.onClick();

    expect(callOrder).toEqual(['focus', 'action']);
  });

  it('onClick is skipped when touchend already handled', () => {
    mockDoc();
    const action = vi.fn();
    const handlers = composeHandlers(action);

    // Simulate touch first
    handlers.onTouchEnd({ preventDefault: vi.fn() } as any);
    action.mockClear();

    // Then the delayed click fires — should be skipped
    handlers.onClick();

    expect(action).not.toHaveBeenCalled();
  });

  it('onClick works again after the skip-once reset', () => {
    const el = mockDoc();
    const action = vi.fn();
    const handlers = composeHandlers(action);

    // Touch → click (skipped) → click (should work)
    handlers.onTouchEnd({ preventDefault: vi.fn() } as any);
    handlers.onClick(); // skipped
    action.mockClear();
    el.focus.mockClear();

    handlers.onClick(); // should work
    expect(action).toHaveBeenCalledOnce();
    expect(el.focus).toHaveBeenCalledOnce();
  });
});
