import { describe, it, expect, vi, beforeEach } from 'vitest';

// isTextInput uses `instanceof HTMLElement`, which isn't available in the
// node test env. Mock just that predicate; keep the rest of the module real.
vi.mock('../utils/dom', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../utils/dom')>();
  return { ...actual, isTextInput: vi.fn(() => false) };
});

import { dispatchEscape, classifyForwardedChord, dispatchForwardedChord } from './useKeyboardShortcuts';
import { isTextInput } from '../utils/dom';
import { pushOverlay, _resetOverlayStackForTesting } from '../store/overlayStack';
import { focusedPane, splitRatio } from '../store/store';

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

describe('classifyForwardedChord (keydowns forwarded from app iframes)', () => {
  const chord = (over: Partial<{ metaKey: boolean; ctrlKey: boolean; shiftKey: boolean; altKey: boolean; key: string }>) => ({
    metaKey: false, ctrlKey: false, shiftKey: false, altKey: false, key: '', ...over,
  });

  it('maps default pane chords to their shortcut id', () => {
    // Defaults from utils/shortcuts.ts (no overrides loaded in the test env).
    expect(classifyForwardedChord(chord({ metaKey: true, shiftKey: true, key: '3' }))).toBe('toggleContentPane');
    expect(classifyForwardedChord(chord({ metaKey: true, shiftKey: true, key: '2' }))).toBe('toggleThreadPane');
    expect(classifyForwardedChord(chord({ metaKey: true, shiftKey: true, key: '1' }))).toBe('toggleThreadDrawer');
    expect(classifyForwardedChord(chord({ metaKey: true, altKey: true, key: 'ArrowLeft' }))).toBe('narrowThreadPane');
    expect(classifyForwardedChord(chord({ metaKey: true, altKey: true, key: 'ArrowRight' }))).toBe('widenThreadPane');
  });

  it('treats Ctrl as the primary modifier too (matches the host leniency)', () => {
    expect(classifyForwardedChord(chord({ ctrlKey: true, shiftKey: true, key: '3' }))).toBe('toggleContentPane');
  });

  it('classifies Escape as the escape policy', () => {
    expect(classifyForwardedChord(chord({ key: 'Escape' }))).toBe('escape');
  });

  it('returns null for a chord that matches no shortcut (host ignores it)', () => {
    expect(classifyForwardedChord(chord({ metaKey: true, key: 'c' }))).toBeNull();
  });
});

describe('dispatchForwardedChord (forwarded chord ⇒ content pane is focused)', () => {
  const chord = (over: Partial<{ metaKey: boolean; ctrlKey: boolean; shiftKey: boolean; altKey: boolean; key: string }>) => ({
    metaKey: false, ctrlKey: false, shiftKey: false, altKey: false, key: '', ...over,
  });

  beforeEach(() => {
    (globalThis as { innerWidth: number }).innerWidth = 1024; // desktop
    // Stale focus: the user is working inside the app iframe, but the host never
    // saw the pointerdown (it can't cross the iframe boundary), so focusedPane
    // still points at the thread pane it was on before opening the app.
    focusedPane.value = 'thread';
    splitRatio.value = 0.5; // content pane visible
  });

  it('⌘⇧3 CLOSES the content pane the app lives in (regression: was a no-op)', () => {
    // Without reconciling focusedPane, toggleContentPane reads 'thread', takes
    // the "focus content" branch, and leaves splitRatio at 0.5 — the pane never
    // closes. Reconciling to 'content' first makes the first press collapse it.
    dispatchForwardedChord(chord({ metaKey: true, shiftKey: true, key: '3' }));
    expect(splitRatio.value).toBe(1); // collapsed = closed
  });

  it('⌘⇧2 focuses the thread pane (focus leaves the app)', () => {
    dispatchForwardedChord(chord({ metaKey: true, shiftKey: true, key: '2' }));
    expect(focusedPane.value).toBe('thread');
    expect(splitRatio.value).toBe(0.5); // thread pane was unfocused → just focus, no collapse
  });

  it('a non-shortcut chord neither dispatches nor touches focus/layout', () => {
    dispatchForwardedChord(chord({ metaKey: true, key: 'c' }));
    expect(splitRatio.value).toBe(0.5);
    expect(focusedPane.value).toBe('thread');
  });
});
