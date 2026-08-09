import { describe, it, expect } from 'vitest';
import {
  trapTargetIndex, paneTabTarget,
  shouldReconcilePaneFocus, PANE_FOCUS_REGION,
  isContentPaneIframeFocus, navigationFocusTarget,
} from './paneFocus';

// A click inside any content-pane iframe (app, preview, PDF plugin, cross-origin
// URL) moves the host's activeElement to the <iframe> and fires window blur; this
// pure check decides whether that focus target should move the content-pane
// focus marker. It
// keys on tagName + a `.pane-content` ancestor (not `instanceof HTMLIFrameElement`)
// so it's testable without jsdom, mirroring appFrame.test.ts's stub style.
describe('isContentPaneIframeFocus', () => {
  const fake = (tagName: string, inContentPane: boolean): Element =>
    ({ tagName, closest: (sel: string) => (sel === '.pane-content' && inContentPane ? {} : null) } as unknown as Element);

  it('is true for an iframe inside the content pane', () => {
    expect(isContentPaneIframeFocus(fake('IFRAME', true))).toBe(true);
  });

  it('is false for a non-iframe element (a normal pane control keeps host focus)', () => {
    expect(isContentPaneIframeFocus(fake('BUTTON', true))).toBe(false);
  });

  it('is false for an iframe outside the content pane (e.g. an app frame elsewhere)', () => {
    expect(isContentPaneIframeFocus(fake('IFRAME', false))).toBe(false);
  });

  it('is false for null activeElement', () => {
    expect(isContentPaneIframeFocus(null)).toBe(false);
  });
});

// Pure boundary logic for the per-pane Tab trap. The DOM handler relies on a
// pane being a contiguous subtree, so the browser's default Tab handles the
// in-between steps and this only decides the wrap at the two ends. Toast reuses
// it for its own (already-focused) button trap, so its semantics stay fixed.
describe('trapTargetIndex', () => {
  it('returns null when the pane has no tabbable elements', () => {
    expect(trapTargetIndex(0, -1, false)).toBeNull();
    expect(trapTargetIndex(0, 0, true)).toBeNull();
  });

  it('returns null for an active element not in the set (index -1)', () => {
    expect(trapTargetIndex(3, -1, false)).toBeNull();
    expect(trapTargetIndex(3, -1, true)).toBeNull();
  });

  it('wraps forward Tab off the last element to the first', () => {
    expect(trapTargetIndex(3, 2, false)).toBe(0);
  });

  it('wraps Shift+Tab off the first element to the last', () => {
    expect(trapTargetIndex(3, 0, true)).toBe(2);
  });

  it('does not wrap in-between (browser default keeps focus in the subtree)', () => {
    expect(trapTargetIndex(3, 1, false)).toBeNull();
    expect(trapTargetIndex(3, 1, true)).toBeNull();
    // forward off a non-last, shift off a non-first
    expect(trapTargetIndex(3, 0, false)).toBeNull();
    expect(trapTargetIndex(3, 2, true)).toBeNull();
  });

  it('a single tabbable element wraps to itself in both directions', () => {
    expect(trapTargetIndex(1, 0, false)).toBe(0);
    expect(trapTargetIndex(1, 0, true)).toBe(0);
  });
});

// Target logic for the focused-pane Tab trap. Unlike trapTargetIndex it MOVES
// focus into the focused pane when DOM focus is currently outside it (index -1),
// which is the case the bug fix targets: a pane click sets `focusedPane`
// signal-only, leaving DOM focus on <body> / the tabindex=-1 container.
describe('paneTabTarget', () => {
  it('falls through (null) when the focused pane has no tabbable elements', () => {
    expect(paneTabTarget(0, -1, false)).toBeNull();
    expect(paneTabTarget(0, 0, true)).toBeNull();
  });

  it('moves focus INTO the pane when DOM focus is outside it (index -1)', () => {
    // forward Tab enters at the first element, Shift+Tab at the last
    expect(paneTabTarget(3, -1, false)).toBe(0);
    expect(paneTabTarget(3, -1, true)).toBe(2);
  });

  it('wraps at the boundaries when focus is already inside the pane', () => {
    expect(paneTabTarget(3, 2, false)).toBe(0); // forward off last → first
    expect(paneTabTarget(3, 0, true)).toBe(2);  // shift off first → last
  });

  it('falls through (null) for in-between steps inside the pane', () => {
    // the browser's default Tab keeps focus in the contiguous pane subtree
    expect(paneTabTarget(3, 1, false)).toBeNull();
    expect(paneTabTarget(3, 1, true)).toBeNull();
    expect(paneTabTarget(3, 0, false)).toBeNull();
    expect(paneTabTarget(3, 2, true)).toBeNull();
  });

  it('a single tabbable element: enter it from outside, then wrap to itself', () => {
    expect(paneTabTarget(1, -1, false)).toBe(0);
    expect(paneTabTarget(1, -1, true)).toBe(0);
    expect(paneTabTarget(1, 0, false)).toBe(0);
    expect(paneTabTarget(1, 0, true)).toBe(0);
  });
});

// Pure decision behind `reconcilePaneFocus` — keeps real DOM focus in sync with
// the focused-pane (focusedPane) marker so native scroll keys act on the pane the
// marker points at. The DOM-touching wrapper (rAF + query + `.focus()`) is
// covered by browser e2e; this pins the "should we move focus at all" contract.
describe('shouldReconcilePaneFocus', () => {
  it('pulls focus in when desktop, no overlay, and focus is outside the pane', () => {
    expect(shouldReconcilePaneFocus({ mobile: false, overlayOpen: false, focusInsidePane: false })).toBe(true);
  });

  it('never steals a click\'s own focus — no-op when focus is already inside the pane', () => {
    // This is what composes with focusPaneMainControl (lands focus in-pane → this
    // then no-ops) and never yanks focus off a control clicked inside the pane.
    expect(shouldReconcilePaneFocus({ mobile: false, overlayOpen: false, focusInsidePane: true })).toBe(false);
  });

  it('no-op on mobile (panes are navigated, not focused)', () => {
    expect(shouldReconcilePaneFocus({ mobile: true, overlayOpen: false, focusInsidePane: false })).toBe(false);
  });

  it('no-op while an overlay owns focus (overlayStack manages its own focus)', () => {
    expect(shouldReconcilePaneFocus({ mobile: false, overlayOpen: true, focusInsidePane: false })).toBe(false);
  });

  it('any single blocking condition suppresses the pull-in', () => {
    // Only the all-clear case moves focus; every other combination is a no-op.
    const combos = [
      { mobile: true, overlayOpen: true, focusInsidePane: true },
      { mobile: true, overlayOpen: false, focusInsidePane: false },
      { mobile: false, overlayOpen: true, focusInsidePane: false },
      { mobile: false, overlayOpen: false, focusInsidePane: true },
    ];
    for (const c of combos) expect(shouldReconcilePaneFocus(c)).toBe(false);
  });
});

// Which focusable a navigation landing on a settings row hands focus to. A row
// whose label carries an explainer puts the info icon BEFORE the control in DOM
// order, so "the first focusable" is the icon and Enter opens a dialog instead
// of operating the setting the user searched for.
describe('navigationFocusTarget', () => {
  const fake = (cls: string) => ({ cls, classList: { contains: (c: string) => c === cls } });

  it('skips the explainer icon and lands on the row control behind it', () => {
    const [icon, control] = [fake('explainer-btn'), fake('settings-option')];
    expect(navigationFocusTarget([icon, control])).toBe(control);
  });

  it('takes the first control when the row has no explainer', () => {
    const [first, second] = [fake('settings-option'), fake('action-btn')];
    expect(navigationFocusTarget([first, second])).toBe(first);
  });

  it('falls back to the explainer when it is the only focusable', () => {
    // A section title whose sole focusable IS its icon (Repositories, Connect
    // URLs) should still take focus rather than leaving the landing unfocused.
    const icon = fake('explainer-btn');
    expect(navigationFocusTarget([icon])).toBe(icon);
  });

  it('is undefined for a container with nothing focusable', () => {
    expect(navigationFocusTarget([])).toBeUndefined();
  });
});

// The keyboard "surface" each pane hands focus to. Drawer maps to the pane
// container itself (its list-nav keydown handler lives there); thread/content map
// to their scroll regions so native Arrow/Page keys scroll the focused pane.
describe('PANE_FOCUS_REGION', () => {
  it('maps each pane to its scroll/keyboard-nav surface', () => {
    // Thread is scoped to `.thread-view` so the compose/welcome `.thread-content`
    // (no `.thread-view` wrapper, never focusable) is excluded — only a real
    // thread's transcript matches.
    expect(PANE_FOCUS_REGION.thread).toBe('.thread-view .thread-content');
    expect(PANE_FOCUS_REGION.content).toBe('.content-pane-body');
    expect(PANE_FOCUS_REGION.drawer).toBe('.thread-drawer');
  });
});
