import { describe, it, expect, vi } from 'vitest';

if (typeof globalThis.requestAnimationFrame === 'undefined') {
  (globalThis as any).requestAnimationFrame = (cb: any) => { cb(); return 0; };
}

import { focusIfNeeded, focusPromptNow, composeHandlers, isComposeFocusedHere, blurPromptInputIfFocused } from '../promptFocus';

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

describe('isComposeFocusedHere', () => {
  function focusOn(threadId: string | null): { dataset: Record<string, string> } {
    const el = threadId === null ? null : { dataset: { role: 'prompt-input', threadId } };
    (globalThis as any).document = { ...document, activeElement: el };
    return el as any;
  }

  it('returns true when activeElement is a prompt-input bound to the threadId', () => {
    focusOn('t-1');
    expect(isComposeFocusedHere('t-1')).toBe(true);
  });

  it('returns false when activeElement is bound to a DIFFERENT threadId', () => {
    focusOn('t-2');
    expect(isComposeFocusedHere('t-1')).toBe(false);
  });

  it('returns false when activeElement is not a prompt-input', () => {
    (globalThis as any).document = {
      ...document,
      activeElement: { dataset: { role: 'some-button' } },
    };
    expect(isComposeFocusedHere('t-1')).toBe(false);
  });

  it('returns false when activeElement is null', () => {
    focusOn(null);
    expect(isComposeFocusedHere('t-1')).toBe(false);
  });

  // The bug this guards against: SplitLayout (desktop) and MobileSwipeContainer
  // (mobile) both render a PromptInput, so two `[data-role="prompt-input"]`
  // textareas exist in the DOM. During viewport transitions both can be
  // briefly visible. The previous `getVisiblePromptInput()`-then-compare
  // implementation picked the FIRST visible one and returned false when the
  // user was actually focused on the second — at which point an SSE
  // ThreadComposeChanged would slip through and bounce the cursor to the end.
  // Checking `document.activeElement` directly avoids the race entirely.
  it('returns true even when there are multiple prompt-input elements (only activeElement matters)', () => {
    const focused = { dataset: { role: 'prompt-input', threadId: 't-1' } };
    (globalThis as any).document = {
      ...document,
      // Plant additional unfocused prompt-input elements that querySelectorAll
      // would have returned alongside the focused one. Only activeElement
      // determines focus — the rest are noise.
      querySelectorAll: vi.fn().mockReturnValue([
        { dataset: { role: 'prompt-input', threadId: 't-1' } },
        focused,
      ]),
      activeElement: focused,
    };
    expect(isComposeFocusedHere('t-1')).toBe(true);
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

describe('blurPromptInputIfFocused', () => {
  it('blurs the active element when it is the prompt textarea', () => {
    const el = { dataset: { role: 'prompt-input', threadId: 't-1' }, blur: vi.fn() } as any;
    (globalThis as any).document = { ...document, activeElement: el };

    blurPromptInputIfFocused();

    expect(el.blur).toHaveBeenCalledOnce();
  });

  it('does not blur when active element is not the prompt textarea', () => {
    const el = { dataset: { role: 'some-button' }, blur: vi.fn() } as any;
    (globalThis as any).document = { ...document, activeElement: el };

    blurPromptInputIfFocused();

    expect(el.blur).not.toHaveBeenCalled();
  });

  it('is a no-op when activeElement is null', () => {
    (globalThis as any).document = { ...document, activeElement: null };

    // Should not throw
    blurPromptInputIfFocused();
  });
});

describe('installActionBtnBlurListener', () => {
  it('blurs prompt on .action-btn pointerdown, ignores other targets', async () => {
    // Re-import a fresh module so the module-scoped install flag is reset —
    // this test exercises the install plumbing, not just the captured closure.
    vi.resetModules();
    const { installActionBtnBlurListener: install } = await import('../promptFocus');

    const promptEl = { dataset: { role: 'prompt-input', threadId: 't-1' }, blur: vi.fn() } as any;
    let captured: ((e: any) => void) | null = null;
    (globalThis as any).document = {
      ...document,
      activeElement: promptEl,
      addEventListener: vi.fn((evt: string, handler: (e: any) => void) => {
        if (evt === 'pointerdown') captured = handler;
      }),
    };

    install();

    expect(captured).not.toBeNull();
    const actionBtn = { closest: vi.fn().mockReturnValue({}) } as any;
    captured!({ target: actionBtn });
    expect(actionBtn.closest).toHaveBeenCalledWith('.action-btn');
    expect(promptEl.blur).toHaveBeenCalledOnce();

    promptEl.blur.mockClear();
    const other = { closest: vi.fn().mockReturnValue(null) } as any;
    captured!({ target: other });
    expect(other.closest).toHaveBeenCalledWith('.action-btn');
    expect(promptEl.blur).not.toHaveBeenCalled();
  });
});
