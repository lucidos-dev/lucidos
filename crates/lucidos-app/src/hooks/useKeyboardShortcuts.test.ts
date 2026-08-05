import { describe, it, expect, vi, beforeEach } from 'vitest';

// isTextInput / isThreadTranscript use `instanceof HTMLElement`, which isn't
// available in the node test env. Mock just those predicates; keep the rest real.
vi.mock('../utils/dom', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../utils/dom')>();
  return { ...actual, isTextInput: vi.fn(() => false), isThreadTranscript: vi.fn(() => false) };
});

// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
import { dispatchEscape, classifyForwardedChord, dispatchForwardedChord, dispatchPreviewIframeShortcut, shouldTypeToFocusPrompt } from './useKeyboardShortcuts';
import { isTextInput, isThreadTranscript } from '../utils/dom';
import { pushOverlay, _resetOverlayStackForTesting } from '../store/overlayStack';
import { focusedPane, splitRatio } from '../store/store';

const here: string = dirname(fileURLToPath(import.meta.url));

beforeEach(() => {
  _resetOverlayStackForTesting();
  vi.mocked(isTextInput).mockReturnValue(false);
  vi.mocked(isThreadTranscript).mockReturnValue(false);
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

  // A pseudo-fullscreen app registers on the same stack (ContentHeaderActions
  // pushes it while active), and a modal an app raises over it is pushed after.
  // LIFO is what keeps one Escape to one effect: the modal closes and the app
  // stays fullscreen. Reversing the pop order would drop the reader out of
  // fullscreen on the keystroke they aimed at the modal.
  it('pops only the modal when an app is pseudo-fullscreen behind it', () => {
    const exitFullscreen = vi.fn();
    const closeModal = vi.fn();
    pushOverlay({ id: 'pseudo-fullscreen', dismiss: exitFullscreen });
    pushOverlay({ id: 'overlay-1', dismiss: closeModal });

    expect(dispatchEscape(null)).toBe('dismissed');
    expect(closeModal).toHaveBeenCalledTimes(1);
    expect(exitFullscreen).not.toHaveBeenCalled();
  });

  // Native fullscreen is the browser's: it takes Escape to exit, and nothing in
  // a keydown handler can stop it (Escape grants no user activation to
  // re-request with, and the only way to consume the close request first is the
  // top layer, which the overlay layer deliberately does not use). Acting anyway
  // would make one Escape close the overlay AND drop fullscreen. Standing down
  // leaves it doing one thing; the overlay is then visible in the normal layout
  // and the next Escape closes it.
  it('stands down while an element is natively fullscreen', () => {
    const dismiss = vi.fn();
    pushOverlay({ id: 'm', dismiss });
    expect(dispatchEscape(null, true)).toBe('fullscreen');
    expect(dismiss).not.toHaveBeenCalled();
  });

  // The stand-down is NOT 'noop'. <Overlay> installs its own bubble-phase
  // Escape listener (useDismissOnOutside), which dismisses unconditionally and
  // is normally shadowed by the stopPropagation the 'dismissed' branch does.
  // A silent fall-through let that listener close the overlay anyway, so one
  // Escape did both things after all. The distinct value is what tells the
  // capture-phase dispatcher to stop the event (without preventDefault, which
  // would be the UA's fullscreen exit).
  it('is distinguishable from a no-op, so the caller can stop the event', () => {
    expect(dispatchEscape(null, true)).not.toBe(dispatchEscape(null, false));
  });

  it('dismisses normally once native fullscreen is gone', () => {
    const dismiss = vi.fn();
    pushOverlay({ id: 'm', dismiss });
    expect(dispatchEscape(null, false)).toBe('dismissed');
    expect(dismiss).toHaveBeenCalledTimes(1);
  });

  // The stand-down must not swallow the blur either: whatever the browser is
  // about to do with this Escape, the host does nothing.
  it('does not blur a focused text input while natively fullscreen', () => {
    vi.mocked(isTextInput).mockReturnValue(true);
    const blur = vi.fn();
    expect(dispatchEscape({ blur } as unknown as Element, true)).toBe('fullscreen');
    expect(blur).not.toHaveBeenCalled();
  });

  // The capture-phase dispatcher must stop the event on the stand-down (or the
  // overlay's own Escape closes it anyway) and must NOT preventDefault (or it
  // would try to suppress the one thing that should happen).
  it('is stopped but not defaulted by the capture dispatcher', () => {
    const src = readFileSync(resolve(here, './useKeyboardShortcuts.ts'), 'utf-8');
    const branch = src.match(/} else if \(result === 'fullscreen'\) \{[\s\S]*?\n {6}\}/)?.[0] ?? '';
    expect(branch).toContain('e.stopPropagation()');
    expect(branch).not.toContain('e.preventDefault()');
  });
});

describe('shouldTypeToFocusPrompt (bare-typing → prompt textarea)', () => {
  const ev = (over: Partial<{ isComposing: boolean; metaKey: boolean; ctrlKey: boolean; altKey: boolean; key: string }> = {}) => ({
    isComposing: false, metaKey: false, ctrlKey: false, altKey: false, key: 'a', target: null, ...over,
  });

  it('focuses the prompt for a bare printable key on desktop with no overlay', () => {
    expect(shouldTypeToFocusPrompt(ev(), { mobile: false, overlayOpen: false })).toBe(true);
  });

  it('does NOT steal the keystroke while an overlay is open — typing must search the dropdown', () => {
    // The reported bug: opening a dropdown and typing wrote into the prompt
    // textarea behind it instead of filtering the dropdown.
    expect(shouldTypeToFocusPrompt(ev(), { mobile: false, overlayOpen: true })).toBe(false);
  });

  it('is disabled on mobile (no type-to-focus there)', () => {
    expect(shouldTypeToFocusPrompt(ev(), { mobile: true, overlayOpen: false })).toBe(false);
  });

  it('skips when a text input already owns focus', () => {
    vi.mocked(isTextInput).mockReturnValue(true);
    expect(shouldTypeToFocusPrompt(ev(), { mobile: false, overlayOpen: false })).toBe(false);
  });

  it('skips modifier chords and non-printable keys', () => {
    expect(shouldTypeToFocusPrompt(ev({ metaKey: true }), { mobile: false, overlayOpen: false })).toBe(false);
    expect(shouldTypeToFocusPrompt(ev({ ctrlKey: true }), { mobile: false, overlayOpen: false })).toBe(false);
    expect(shouldTypeToFocusPrompt(ev({ altKey: true }), { mobile: false, overlayOpen: false })).toBe(false);
    expect(shouldTypeToFocusPrompt(ev({ key: 'Enter' }), { mobile: false, overlayOpen: false })).toBe(false);
    expect(shouldTypeToFocusPrompt(ev({ key: 'ArrowDown' }), { mobile: false, overlayOpen: false })).toBe(false);
  });

  it('skips while an IME composition is active', () => {
    expect(shouldTypeToFocusPrompt(ev({ isComposing: true }), { mobile: false, overlayOpen: false })).toBe(false);
  });

  it('does NOT steal Space while the transcript region is focused — Space must page it down', () => {
    vi.mocked(isThreadTranscript).mockReturnValue(true);
    expect(shouldTypeToFocusPrompt(ev({ key: ' ' }), { mobile: false, overlayOpen: false })).toBe(false);
  });

  it('still focuses the prompt for a printable LETTER while the transcript is focused (type → compose)', () => {
    vi.mocked(isThreadTranscript).mockReturnValue(true);
    expect(shouldTypeToFocusPrompt(ev({ key: 'a' }), { mobile: false, overlayOpen: false })).toBe(true);
  });

  it('types Space to the prompt when the transcript is NOT focused (unchanged)', () => {
    expect(shouldTypeToFocusPrompt(ev({ key: ' ' }), { mobile: false, overlayOpen: false })).toBe(true);
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

describe('dispatchPreviewIframeShortcut (keydown INSIDE a content-pane preview iframe)', () => {
  // KeyboardEvent isn't a global in the node test env, so fake the chord shape
  // the dispatcher reads, with a preventDefault that records defaultPrevented.
  const key = (over: Partial<{ metaKey: boolean; ctrlKey: boolean; shiftKey: boolean; altKey: boolean; key: string }>) => {
    const e = { metaKey: false, ctrlKey: false, shiftKey: false, altKey: false, key: '', defaultPrevented: false, ...over } as unknown as KeyboardEvent & { defaultPrevented: boolean };
    (e as { preventDefault: () => void }).preventDefault = () => { (e as { defaultPrevented: boolean }).defaultPrevented = true; };
    return e;
  };

  beforeEach(() => {
    (globalThis as { innerWidth: number }).innerWidth = 1024; // desktop
    // Focus is inside the preview iframe, but the host never saw a pointerdown
    // cross the iframe boundary, so focusedPane is stale on the thread pane.
    focusedPane.value = 'thread';
    splitRatio.value = 0.5; // content pane visible, not maximized
  });

  it('⌘⇧↵ maximizes the content pane (the reported bug) and suppresses the browser default', () => {
    // Without the bridge this keydown never reaches the host: no maximize, and
    // Chrome runs its own default for ⌘⇧↵ (the page context menu).
    const e = key({ key: 'Enter', metaKey: true, shiftKey: true });
    expect(dispatchPreviewIframeShortcut(e)).toBe(true);
    expect((e as { defaultPrevented: boolean }).defaultPrevented).toBe(true); // browser default suppressed
    expect(focusedPane.value).toBe('content');       // reconciled before dispatch
    expect(splitRatio.value).toBe(0);                // content pane group maximized
  });

  it('⌘⇧3 CLOSES the content pane the preview lives in (reconciles stale focus)', () => {
    expect(dispatchPreviewIframeShortcut(key({ key: '3', metaKey: true, shiftKey: true }))).toBe(true);
    expect(splitRatio.value).toBe(1); // collapsed = closed
  });

  it('leaves a non-shortcut key (plain Enter on a link, normal typing) untouched', () => {
    const e = key({ key: 'Enter' });
    expect(dispatchPreviewIframeShortcut(e)).toBe(false);
    expect((e as { defaultPrevented: boolean }).defaultPrevented).toBe(false); // preview keeps its own behavior
    expect(splitRatio.value).toBe(0.5);
    expect(focusedPane.value).toBe('thread');
  });

  it('Escape dismisses an open host overlay (e.g. a modal over the content)', () => {
    const dismiss = vi.fn();
    pushOverlay({ id: 'm', dismiss });
    const e = key({ key: 'Escape' });
    expect(dispatchPreviewIframeShortcut(e)).toBe(true);
    expect(dismiss).toHaveBeenCalledTimes(1);
    expect((e as { defaultPrevented: boolean }).defaultPrevented).toBe(true);
  });
});
