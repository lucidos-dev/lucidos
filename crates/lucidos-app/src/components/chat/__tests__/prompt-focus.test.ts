import { describe, it, expect, vi } from 'vitest';

if (typeof globalThis.requestAnimationFrame === 'undefined') {
  (globalThis as any).requestAnimationFrame = (cb: any) => { cb(); return 0; };
}

import { focusPromptNow, isComposeFocusedHere, blurPromptInputIfFocused } from '../promptFocus';
import { composeHandlers } from '../composeHandlers';
import { takePressOutcome } from '../../../utils/tapGesture';

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

  it('returns true even when there are multiple prompt-input elements (only activeElement matters)', () => {
    const focused = { dataset: { role: 'prompt-input', threadId: 't-1' } };
    (globalThis as any).document = {
      ...document,
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

    handlers.onTouchEnd({ preventDefault: vi.fn() } as any);
    action.mockClear();

    handlers.onClick();

    expect(action).not.toHaveBeenCalled();
  });

  it('a touch press claims itself as served', () => {
    mockDoc();
    takePressOutcome(0);
    composeHandlers(vi.fn(), () => {}).onTouchEnd({ preventDefault: vi.fn() } as any);
    expect(takePressOutcome(1_000)).toBe('served');
  });

  it('onClick works again after the skip-once reset', () => {
    const el = mockDoc();
    const action = vi.fn();
    const handlers = composeHandlers(action);

    handlers.onTouchEnd({ preventDefault: vi.fn() } as any);
    handlers.onClick();
    action.mockClear();
    el.focus.mockClear();

    handlers.onClick();
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

    blurPromptInputIfFocused();
  });
});

describe('installActionBtnBlurListener', () => {
  it('registers on click (not pointerdown) so the click dispatches before keyboard dismissal', async () => {
    vi.resetModules();
    const { installActionBtnBlurListener: install } = await import('../promptFocus');

    const registered: string[] = [];
    (globalThis as any).document = {
      ...document,
      activeElement: null,
      addEventListener: vi.fn((evt: string) => { registered.push(evt); }),
    };

    install();

    expect(registered).toContain('click');
    expect(registered).not.toContain('pointerdown');
  });

  it('blurs prompt on .action-btn click, ignores other targets', async () => {
    vi.resetModules();
    const { installActionBtnBlurListener: install } = await import('../promptFocus');

    const promptEl = { dataset: { role: 'prompt-input', threadId: 't-1' }, blur: vi.fn() } as any;
    let captured: ((e: any) => void) | null = null;
    (globalThis as any).document = {
      ...document,
      activeElement: promptEl,
      addEventListener: vi.fn((evt: string, handler: (e: any) => void) => {
        if (evt === 'click') captured = handler;
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
