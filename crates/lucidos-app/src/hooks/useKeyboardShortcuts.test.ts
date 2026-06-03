import { describe, it, expect, vi, beforeEach } from 'vitest';

// isTextInput uses `instanceof HTMLElement`, which isn't available in the
// node test env. Mock just that predicate; keep the rest of the module real.
vi.mock('../utils/dom', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../utils/dom')>();
  return { ...actual, isTextInput: vi.fn(() => false) };
});

import { dispatchEscape } from './useKeyboardShortcuts';
import { isTextInput } from '../utils/dom';
import { pushOverlay, _resetOverlayStackForTesting } from '../store/overlayStack';

beforeEach(() => {
  _resetOverlayStackForTesting();
  vi.mocked(isTextInput).mockReturnValue(false);
});

describe('dispatchEscape (non-destructive Escape policy)', () => {
  it('dismisses the top overlay first, before any blur', () => {
    const dismiss = vi.fn();
    pushOverlay({ id: 'm', dismiss });
    // Even with a focused text input, an open overlay wins.
    vi.mocked(isTextInput).mockReturnValue(true);
    expect(dispatchEscape({ blur: vi.fn() } as unknown as Element)).toBe('dismissed');
    expect(dismiss).toHaveBeenCalledTimes(1);
  });

  it('blurs a focused text input when no overlay is open', () => {
    vi.mocked(isTextInput).mockReturnValue(true);
    const blur = vi.fn();
    expect(dispatchEscape({ blur } as unknown as Element)).toBe('blurred');
    expect(blur).toHaveBeenCalledTimes(1);
  });

  it('leaves a self-managing text input untouched so its own Escape handler can cancel', () => {
    // The thread-title editor marks its input data-escape-self because a blur
    // there commits a rename — the universal blur-on-Escape would SAVE instead
    // of cancel. dispatchEscape must NOT blur it.
    vi.mocked(isTextInput).mockReturnValue(true);
    const blur = vi.fn();
    const active = { blur, hasAttribute: (n: string) => n === 'data-escape-self' };
    expect(dispatchEscape(active as unknown as Element)).toBe('self-managed');
    expect(blur).not.toHaveBeenCalled();
  });

  it('no-ops when nothing is open and focus is not a text input (never touches the thread)', () => {
    expect(dispatchEscape(null)).toBe('noop');
  });
});
