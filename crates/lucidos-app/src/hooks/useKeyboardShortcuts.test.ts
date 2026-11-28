import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('../utils/dom', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../utils/dom')>();
  return { ...actual, isTextInput: vi.fn(() => false), isThreadTranscript: vi.fn(() => false), isInteractiveTarget: vi.fn(() => false) };
});

vi.mock('../components/chat/WaitingBanner', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../components/chat/WaitingBanner')>();
  return { ...actual, applyFocusedThreadChange: vi.fn(), showFocusedThreadDiff: vi.fn() };
});
vi.mock('../components/chat/PromptRowControls', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../components/chat/PromptRowControls')>();
  return { ...actual, pressCallToggleIfShown: vi.fn() };
});
vi.mock('../components/chat/FollowToggle', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../components/chat/FollowToggle')>();
  return { ...actual, toggleFollowLiveEdge: vi.fn() };
});
vi.mock('../components/layout/ContentHeaderActions', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../components/layout/ContentHeaderActions')>();
  return { ...actual, toggleAppFullscreenIfShown: vi.fn(), toggleSourceView: vi.fn(), toggleLineWrap: vi.fn() };
});
vi.mock('../components/drawer/ThreadDrawer', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../components/drawer/ThreadDrawer')>();
  return { ...actual, openHighlightedThreadActions: vi.fn(() => false) };
});
vi.mock('../components/chat/ThreadTitle', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../components/chat/ThreadTitle')>();
  return { ...actual, openThreadTitleMenu: vi.fn() };
});

// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
import { dispatchEscape, classifyChord, dispatchForwardedChord, dispatchPreviewIframeShortcut, shouldTypeToFocusPrompt, isMacTextEditingKey } from './useKeyboardShortcuts';
import { isTextInput, isThreadTranscript, isInteractiveTarget } from '../utils/dom';
import { pushOverlay, _resetOverlayStackForTesting } from '../store/overlayStack';
import { activeMenuItem, appSearchOpen, focusedPane, focusedThreadId, panelOverlay, splitRatio, searchEverywhereAnchor, searchEverywhereOpen, threadMap } from '../store/store';
import { makeThreadState } from '../store/__tests__/thread-events-helpers';
import { findSurface } from '../store/actions/find-bar';
import { openHighlightedThreadActions } from '../components/drawer/ThreadDrawer';
import { openThreadTitleMenu } from '../components/chat/ThreadTitle';
import { promptStopRequested, promptSideQuestionRequested } from '../components/chat/prompt-input-helpers';
import { applyFocusedThreadChange, showFocusedThreadDiff } from '../components/chat/WaitingBanner';
import { pressCallToggleIfShown } from '../components/chat/PromptRowControls';
import { toggleFollowLiveEdge } from '../components/chat/FollowToggle';
import { toggleAppFullscreenIfShown, toggleSourceView, toggleLineWrap } from '../components/layout/ContentHeaderActions';

const here: string = dirname(fileURLToPath(import.meta.url));

beforeEach(() => {
  _resetOverlayStackForTesting();
  vi.mocked(isTextInput).mockReturnValue(false);
  vi.mocked(isThreadTranscript).mockReturnValue(false);
  vi.mocked(isInteractiveTarget).mockReturnValue(false);
});

describe('dispatchEscape (non-destructive Escape policy)', () => {
  it('dismisses the top overlay first, before any blur', () => {
    const dismiss = vi.fn();
    pushOverlay({ id: 'm', dismiss, hasPanel: true });
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
    vi.mocked(isTextInput).mockReturnValue(true);
    const blur = vi.fn();
    const active = { blur, hasAttribute: (n: string) => n === 'data-escape-self' };
    expect(dispatchEscape(active as unknown as Element)).toBe('self-managed');
    expect(blur).not.toHaveBeenCalled();
  });

  it('no-ops when nothing is open and focus is not a text input (never touches the thread)', () => {
    expect(dispatchEscape(null)).toBe('noop');
  });

  it('pops only the modal when an app is pseudo-fullscreen behind it', () => {
    const exitFullscreen = vi.fn();
    const closeModal = vi.fn();
    pushOverlay({ id: 'pseudo-fullscreen', dismiss: exitFullscreen, hasPanel: false });
    pushOverlay({ id: 'overlay-1', dismiss: closeModal, hasPanel: true });

    expect(dispatchEscape(null)).toBe('dismissed');
    expect(closeModal).toHaveBeenCalledTimes(1);
    expect(exitFullscreen).not.toHaveBeenCalled();
  });

  it('stands down while an element is natively fullscreen', () => {
    const dismiss = vi.fn();
    pushOverlay({ id: 'm', dismiss, hasPanel: true });
    expect(dispatchEscape(null, true)).toBe('fullscreen');
    expect(dismiss).not.toHaveBeenCalled();
  });

  it('is distinguishable from a no-op, so the caller can stop the event', () => {
    expect(dispatchEscape(null, true)).not.toBe(dispatchEscape(null, false));
  });

  it('dismisses normally once native fullscreen is gone', () => {
    const dismiss = vi.fn();
    pushOverlay({ id: 'm', dismiss, hasPanel: true });
    expect(dispatchEscape(null, false)).toBe('dismissed');
    expect(dismiss).toHaveBeenCalledTimes(1);
  });

  it('does not blur a focused text input while natively fullscreen', () => {
    vi.mocked(isTextInput).mockReturnValue(true);
    const blur = vi.fn();
    expect(dispatchEscape({ blur } as unknown as Element, true)).toBe('fullscreen');
    expect(blur).not.toHaveBeenCalled();
  });

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

  it('leaves Space to a focused button, which Space presses', () => {
    vi.mocked(isInteractiveTarget).mockReturnValue(true);
    expect(shouldTypeToFocusPrompt(ev({ key: ' ' }), { mobile: false, overlayOpen: false })).toBe(false);
  });

  it('still focuses the prompt for a printable LETTER while a button is focused', () => {
    vi.mocked(isInteractiveTarget).mockReturnValue(true);
    expect(shouldTypeToFocusPrompt(ev({ key: 'a' }), { mobile: false, overlayOpen: false })).toBe(true);
  });

  it('types Space to the prompt when the transcript is NOT focused (unchanged)', () => {
    expect(shouldTypeToFocusPrompt(ev({ key: ' ' }), { mobile: false, overlayOpen: false })).toBe(true);
  });
});

describe('classifyChord (host keydowns, app-frame forwards, the PDF preview)', () => {
  const chord = (over: Partial<{ metaKey: boolean; ctrlKey: boolean; shiftKey: boolean; altKey: boolean; key: string }>) => ({
    metaKey: false, ctrlKey: false, shiftKey: false, altKey: false, key: '', ...over,
  });

  it('maps default pane chords to their shortcut id', () => {
    expect(classifyChord(chord({ metaKey: true, shiftKey: true, key: '3' }))).toBe('toggleContentPane');
    expect(classifyChord(chord({ metaKey: true, shiftKey: true, key: '2' }))).toBe('toggleThreadPane');
    expect(classifyChord(chord({ metaKey: true, shiftKey: true, key: '1' }))).toBe('toggleThreadDrawer');
    expect(classifyChord(chord({ metaKey: true, altKey: true, key: 'ArrowLeft' }))).toBe('narrowThreadPane');
    expect(classifyChord(chord({ metaKey: true, altKey: true, key: 'ArrowRight' }))).toBe('widenThreadPane');
  });

  it('treats Ctrl as the primary modifier too (matches the host leniency)', () => {
    expect(classifyChord(chord({ ctrlKey: true, shiftKey: true, key: '3' }))).toBe('toggleContentPane');
  });

  it('classifies Escape as the escape policy', () => {
    expect(classifyChord(chord({ key: 'Escape' }))).toBe('escape');
  });

  it('gives Shift+Escape to the composer shortcut when the Escape policy has nothing to do', () => {
    expect(classifyChord(chord({ shiftKey: true, key: 'Escape' }))).toBe('focusComposer');
  });

  it('claims Mod+F for find only where the focused pane can be searched', () => {
    const modF = chord({ metaKey: true, key: 'f' });
    const app = { type: 'app-ui', app: { id: 'habit-tracker' } } as unknown as typeof panelOverlay.value;
    focusedPane.value = 'content';
    panelOverlay.value = null;
    expect(classifyChord(modF), 'a settings page has nothing to find').toBeNull();
    panelOverlay.value = { type: 'file-preview', path: 'artifacts/photo.png' };
    expect(classifyChord(modF), 'nor does a picture').toBeNull();
    panelOverlay.value = { type: 'file-preview', path: 'artifacts/notes.md' };
    expect(classifyChord(modF)).toBe('findInView');
    panelOverlay.value = app;
    expect(classifyChord(modF)).toBe('findInView');
    focusedPane.value = 'drawer';
    expect(classifyChord(modF), 'the drawer has no find bar').toBeNull();
    expect(classifyChord(modF, true), 'a keydown from a content frame judges the content pane').toBe('findInView');
    panelOverlay.value = { type: 'file-preview', path: 'artifacts/report.pdf' };
    expect(classifyChord(modF, true), 'the PDF preview keeps its own find').toBeNull();
    panelOverlay.value = null;
    focusedPane.value = 'thread';
  });

  it('claims Mod+F to close an open find bar, wherever focus is', () => {
    const modF = chord({ metaKey: true, key: 'f' });
    focusedPane.value = 'drawer';
    findSurface.value = 'content';
    expect(classifyChord(modF)).toBe('findInView');
    pushOverlay({ id: 'modal', dismiss: vi.fn(), hasPanel: true });
    expect(classifyChord(modF), 'a modal still covers it').toBeNull();
    findSurface.value = null;
    focusedPane.value = 'thread';
  });

  it('claims Mod+F for the transcript with the thread pane focused, never on a draft or under a modal', () => {
    const modF = chord({ metaKey: true, key: 'f' });
    const sent = makeThreadState(new Map());
    const draft = { ...makeThreadState(new Map()), meta: { ...sent.meta, id: 'draft-1', state: 'composing' as const } };
    threadMap.value = new Map([[sent.meta.id, sent], ['draft-1', draft]]);
    focusedPane.value = 'thread';
    focusedThreadId.value = sent.meta.id;
    expect(classifyChord(modF)).toBe('findInView');
    focusedThreadId.value = 'draft-1';
    expect(classifyChord(modF), 'a draft has no transcript').toBeNull();
    focusedThreadId.value = sent.meta.id;
    pushOverlay({ id: 'modal', dismiss: vi.fn(), hasPanel: true });
    expect(classifyChord(modF), 'a modal covers the pane behind it').toBeNull();
    focusedThreadId.value = null;
    threadMap.value = new Map();
  });

  it('gives Shift+Escape to the Escape policy while an overlay is open, so one press does one thing', () => {
    pushOverlay({ id: 'm', dismiss: vi.fn(), hasPanel: true });
    expect(classifyChord(chord({ shiftKey: true, key: 'Escape' }))).toBe('escape');
  });

  it('gives Shift+Escape to a field that owns its Escape, whose blur would save the edit', () => {
    const g = globalThis as { document?: unknown };
    const saved = g.document;
    g.document = { activeElement: { hasAttribute: (n: string) => n === 'data-escape-self' } };
    try {
      expect(classifyChord(chord({ shiftKey: true, key: 'Escape' }))).toBe('escape');
    } finally {
      g.document = saved;
    }
  });

  it('gives Shift+Escape to the Escape policy while an app is natively fullscreen', () => {
    const g = globalThis as { document?: unknown };
    const saved = g.document;
    g.document = { fullscreenElement: {} };
    try {
      expect(classifyChord(chord({ shiftKey: true, key: 'Escape' }))).toBe('escape');
    } finally {
      g.document = saved;
    }
  });

  it('returns null for a chord that matches no shortcut (host ignores it)', () => {
    expect(classifyChord(chord({ metaKey: true, key: 'c' }))).toBeNull();
  });
});

describe('dispatchForwardedChord (forwarded chord ⇒ content pane is focused)', () => {
  const chord = (over: Partial<{ metaKey: boolean; ctrlKey: boolean; shiftKey: boolean; altKey: boolean; key: string }>) => ({
    metaKey: false, ctrlKey: false, shiftKey: false, altKey: false, key: '', ...over,
  });

  beforeEach(() => {
    (globalThis as { innerWidth: number }).innerWidth = 1024;
    focusedPane.value = 'thread';
    splitRatio.value = 0.5;
  });

  it('⌘⇧3 CLOSES the content pane the app lives in (regression: was a no-op)', () => {
    dispatchForwardedChord(chord({ metaKey: true, shiftKey: true, key: '3' }));
    expect(splitRatio.value).toBe(1);
  });

  it('⌘⇧2 focuses the thread pane (focus leaves the app)', () => {
    dispatchForwardedChord(chord({ metaKey: true, shiftKey: true, key: '2' }));
    expect(focusedPane.value).toBe('thread');
    expect(splitRatio.value).toBe(0.5);
  });

  it('a non-shortcut chord neither dispatches nor touches focus/layout', () => {
    dispatchForwardedChord(chord({ metaKey: true, key: 'c' }));
    expect(splitRatio.value).toBe(0.5);
    expect(focusedPane.value).toBe('thread');
  });
});

describe('isMacTextEditingKey', () => {
  const ctrlK = { metaKey: false, ctrlKey: true, shiftKey: false, altKey: false, key: 'k', target: null };

  it('leaves Ctrl+letter to a Mac text field, where Ctrl+K deletes to the line end', () => {
    vi.mocked(isTextInput).mockReturnValue(true);
    expect(isMacTextEditingKey(ctrlK, true)).toBe(true);
  });

  it('lets the shortcut have it with ⌘, outside a field, or off a Mac', () => {
    vi.mocked(isTextInput).mockReturnValue(true);
    expect(isMacTextEditingKey({ ...ctrlK, metaKey: true, ctrlKey: false }, true)).toBe(false);
    expect(isMacTextEditingKey(ctrlK, false)).toBe(false);
    vi.mocked(isTextInput).mockReturnValue(false);
    expect(isMacTextEditingKey(ctrlK, true)).toBe(false);
  });

  it('leaves a shifted chord to the shortcut, since the Mac shows those as ⌃⇧', () => {
    vi.mocked(isTextInput).mockReturnValue(true);
    expect(isMacTextEditingKey({ ...ctrlK, shiftKey: true, key: 'O' }, true)).toBe(false);
  });

  const optUp = { metaKey: false, ctrlKey: false, shiftKey: false, altKey: true, key: 'ArrowUp', target: null };

  it('leaves Option+Arrow to a Mac text field, where it moves the caret by paragraph or word', () => {
    vi.mocked(isTextInput).mockReturnValue(true);
    for (const key of ['ArrowUp', 'ArrowDown', 'ArrowLeft', 'ArrowRight']) {
      expect(isMacTextEditingKey({ ...optUp, key }, true)).toBe(true);
    }
  });

  it('lets an Option+Arrow shortcut have it outside a field, off a Mac, or with ⌘ held', () => {
    vi.mocked(isTextInput).mockReturnValue(false);
    expect(isMacTextEditingKey(optUp, true)).toBe(false);
    vi.mocked(isTextInput).mockReturnValue(true);
    expect(isMacTextEditingKey(optUp, false)).toBe(false);
    expect(isMacTextEditingKey({ ...optUp, metaKey: true }, true)).toBe(false);
  });
});

describe('the Stop shortcut', () => {
  it('asks the composer to cancel rather than cancelling behind its back', () => {
    promptStopRequested.value = false;
    dispatchForwardedChord({ metaKey: true, ctrlKey: false, shiftKey: false, altKey: false, key: '.' });
    expect(promptStopRequested.value).toBe(true);
    promptStopRequested.value = false;
  });
});

describe('the Side question shortcut', () => {
  it('asks the composer to offer the side question, and claims Alt+Enter from the send', () => {
    promptSideQuestionRequested.value = false;
    splitRatio.value = 0.5;
    const altEnter = { metaKey: false, ctrlKey: false, shiftKey: false, altKey: true, key: 'Enter' };
    expect(classifyChord(altEnter)).toBe('askSideQuestion');
    dispatchForwardedChord(altEnter);
    expect(promptSideQuestionRequested.value).toBe(true);
    promptSideQuestionRequested.value = false;
  });
});

describe('the Search everywhere shortcut', () => {
  const searchChord = { metaKey: true, ctrlKey: false, shiftKey: false, altKey: false, key: 'k' };

  it('drops the anchor a past button open left, so the palette follows the focused pane', () => {
    searchEverywhereOpen.value = false;
    searchEverywhereAnchor.value = {} as HTMLElement;
    dispatchForwardedChord(searchChord);
    expect(searchEverywhereOpen.value).toBe(true);
    expect(searchEverywhereAnchor.value).toBeNull();
  });

  it('closes an open palette without touching its anchor', () => {
    const button = {} as HTMLElement;
    searchEverywhereOpen.value = true;
    searchEverywhereAnchor.value = button;
    dispatchForwardedChord(searchChord);
    expect(searchEverywhereOpen.value).toBe(false);
    expect(searchEverywhereAnchor.value).toBe(button);
  });
});

describe('dispatchPreviewIframeShortcut (keydown INSIDE a content-pane preview iframe)', () => {
  const key = (over: Partial<{ metaKey: boolean; ctrlKey: boolean; shiftKey: boolean; altKey: boolean; key: string }>) => {
    const e = { metaKey: false, ctrlKey: false, shiftKey: false, altKey: false, key: '', defaultPrevented: false, ...over } as unknown as KeyboardEvent & { defaultPrevented: boolean };
    (e as { preventDefault: () => void }).preventDefault = () => { (e as { defaultPrevented: boolean }).defaultPrevented = true; };
    return e;
  };

  beforeEach(() => {
    (globalThis as { innerWidth: number }).innerWidth = 1024;
    focusedPane.value = 'thread';
    splitRatio.value = 0.5;
  });

  it('⌘⇧↵ maximizes the content pane (the reported bug) and suppresses the browser default', () => {
    const e = key({ key: 'Enter', metaKey: true, shiftKey: true });
    expect(dispatchPreviewIframeShortcut(e)).toBe(true);
    expect((e as { defaultPrevented: boolean }).defaultPrevented).toBe(true);
    expect(focusedPane.value).toBe('content');
    expect(splitRatio.value).toBe(0);
  });

  it('⌘⇧3 CLOSES the content pane the preview lives in (reconciles stale focus)', () => {
    expect(dispatchPreviewIframeShortcut(key({ key: '3', metaKey: true, shiftKey: true }))).toBe(true);
    expect(splitRatio.value).toBe(1);
  });

  it('leaves a non-shortcut key (plain Enter on a link, normal typing) untouched', () => {
    const e = key({ key: 'Enter' });
    expect(dispatchPreviewIframeShortcut(e)).toBe(false);
    expect((e as { defaultPrevented: boolean }).defaultPrevented).toBe(false);
    expect(splitRatio.value).toBe(0.5);
    expect(focusedPane.value).toBe('thread');
  });

  it('Escape dismisses an open host overlay (e.g. a modal over the content)', () => {
    const dismiss = vi.fn();
    pushOverlay({ id: 'm', dismiss, hasPanel: true });
    const e = key({ key: 'Escape' });
    expect(dispatchPreviewIframeShortcut(e)).toBe(true);
    expect(dismiss).toHaveBeenCalledTimes(1);
    expect((e as { defaultPrevented: boolean }).defaultPrevented).toBe(true);
  });
});

describe('the toggle shortcuts reach their actions', () => {
  const ctrlShift = (key: string) => ({ metaKey: false, ctrlKey: true, shiftKey: true, altKey: false, key });

  beforeEach(() => vi.clearAllMocks());

  it.each([
    ['l', toggleFollowLiveEdge],
    ['d', showFocusedThreadDiff],
    ['f', toggleAppFullscreenIfShown],
    ['s', toggleSourceView],
    ['b', toggleLineWrap],
  ])('Ctrl+Shift+%s runs its action', (key, action) => {
    dispatchForwardedChord(ctrlShift(key));
    expect(action).toHaveBeenCalledTimes(1);
  });
});

describe('the Open thread actions shortcut', () => {
  const menuChord = { metaKey: false, ctrlKey: true, shiftKey: true, altKey: false, key: 'm' };

  beforeEach(() => {
    vi.clearAllMocks();
    vi.stubGlobal('requestAnimationFrame', (cb: FrameRequestCallback) => { cb(0); return 0; });
    splitRatio.value = 0.5;
  });

  it('opens the highlighted drawer row\'s menu, and not the open thread\'s too', () => {
    vi.mocked(openHighlightedThreadActions).mockReturnValueOnce(true);
    focusedThreadId.value = 't1';
    dispatchForwardedChord(menuChord);
    expect(openThreadTitleMenu).not.toHaveBeenCalled();
    focusedThreadId.value = null;
  });

  it('opens the open thread\'s menu when no drawer row has the focus', () => {
    focusedThreadId.value = 't1';
    dispatchForwardedChord(menuChord);
    expect(openThreadTitleMenu).toHaveBeenCalledTimes(1);
    focusedThreadId.value = null;
  });

  it('does nothing with no thread open', () => {
    focusedThreadId.value = null;
    dispatchForwardedChord(menuChord);
    expect(openThreadTitleMenu).not.toHaveBeenCalled();
  });
});

describe('the host-only shortcuts', () => {
  const chord = (key: string) => ({ metaKey: false, ctrlKey: true, shiftKey: true, altKey: false, key });
  const applyChord = chord('a');
  const hostKeydown = (key = 'a') => {
    const e = { ...chord(key), defaultPrevented: false } as unknown as KeyboardEvent & { defaultPrevented: boolean };
    (e as { preventDefault: () => void }).preventDefault = () => { e.defaultPrevented = true; };
    return e;
  };

  beforeEach(() => vi.clearAllMocks());

  it('never applies from a chord a frame forwarded, since its script can forge one', () => {
    dispatchForwardedChord(applyChord);
    expect(applyFocusedThreadChange).not.toHaveBeenCalled();
  });

  it('applies from a keydown the host itself received', () => {
    expect(dispatchPreviewIframeShortcut(hostKeydown())).toBe(true);
    expect(applyFocusedThreadChange).toHaveBeenCalledTimes(1);
  });

  it('never starts a call from a forwarded chord, which would open the microphone', () => {
    dispatchForwardedChord(chord('h'));
    expect(pressCallToggleIfShown).not.toHaveBeenCalled();
  });

  it('starts a call from a keydown the host itself received', () => {
    expect(dispatchPreviewIframeShortcut(hostKeydown('h'))).toBe(true);
    expect(pressCallToggleIfShown).toHaveBeenCalledTimes(1);
  });

  it('does nothing behind an open dialog, where it would stack a second confirm', () => {
    pushOverlay({ id: 'confirm', dismiss: vi.fn(), hasPanel: true });
    dispatchPreviewIframeShortcut(hostKeydown());
    expect(applyFocusedThreadChange).not.toHaveBeenCalled();
  });
});

describe('app-list search shortcut scope', () => {
  const chord = { key: 'Enter', metaKey: true, ctrlKey: false, altKey: true, shiftKey: false };
  it('claims only Apps and Plugins lists and yields under overlays', () => {
    panelOverlay.value = null;
    for (const menu of ['apps', 'plugins'] as const) {
      activeMenuItem.value = menu;
      expect(classifyChord(chord)).toBe('searchApps');
    }
    activeMenuItem.value = 'settings';
    expect(classifyChord(chord)).toBeNull();
    activeMenuItem.value = 'apps';
    panelOverlay.value = { type: 'file-preview', path: 'artifacts/example.md' };
    expect(classifyChord(chord)).toBeNull();
    panelOverlay.value = null;
    pushOverlay({ id: 'dialog', dismiss: () => {}, hasPanel: true });
    expect(classifyChord(chord)).toBeNull();
    appSearchOpen.value = false;
  });
});
