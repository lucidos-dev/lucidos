// @vitest-environment jsdom
import { describe, it, expect, afterEach } from 'vitest';
import { focusedPane } from '../../store/store';
import {
  trapTargetIndex, paneTabTarget,
  shouldReconcilePaneFocus, PANE_FOCUS_REGION,
  isContentPaneIframeFocus, navigationFocusTarget,
  visibleFocusables, focusIntoPane, tabTargetWithin,
} from './paneFocus';

// jsdom never lays out, so `getClientRects()` is empty on every real element.
// `visibleFocusables` treats that as "not visible", so stub it per-element to
// isolate the `inert` exclusion under test.
function stubVisible(el: HTMLElement): void {
  el.getClientRects = () => [{}] as unknown as DOMRectList;
}

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
// focus into the focused pane when DOM focus is outside it: a pane click sets
// `focusedPane` signal-only, leaving DOM focus on <body> or another pane.
describe('paneTabTarget', () => {
  it('falls through (null) when the focused pane has no tabbable elements', () => {
    expect(paneTabTarget(0, -1, false, false)).toBeNull();
    expect(paneTabTarget(0, 0, true, true)).toBeNull();
  });

  it('moves focus INTO the pane when DOM focus is outside it', () => {
    // forward Tab enters at the first element, Shift+Tab at the last
    expect(paneTabTarget(3, -1, false, false)).toBe(0);
    expect(paneTabTarget(3, -1, true, false)).toBe(2);
  });

  it('wraps at the boundaries when focus is already inside the pane', () => {
    expect(paneTabTarget(3, 2, false, true)).toBe(0); // forward off last → first
    expect(paneTabTarget(3, 0, true, true)).toBe(2);  // shift off first → last
  });

  it('falls through (null) for in-between steps inside the pane', () => {
    // the browser's default Tab keeps focus in the contiguous pane subtree
    expect(paneTabTarget(3, 1, false, true)).toBeNull();
    expect(paneTabTarget(3, 1, true, true)).toBeNull();
    expect(paneTabTarget(3, 0, false, true)).toBeNull();
    expect(paneTabTarget(3, 2, true, true)).toBeNull();
  });

  it('a single tabbable element: enter it from outside, then wrap to itself', () => {
    expect(paneTabTarget(1, -1, false, false)).toBe(0);
    expect(paneTabTarget(1, -1, true, false)).toBe(0);
    expect(paneTabTarget(1, 0, false, true)).toBe(0);
    expect(paneTabTarget(1, 0, true, true)).toBe(0);
  });
});

// Focus can sit inside a pane on an element the trap does not list: the
// tabindex=-1 pane container a click focused, or a dialog body. Native Tab
// from there left the pane while the marker stayed behind.
describe('tabTargetWithin', () => {
  function pane(): { container: HTMLElement; list: HTMLElement[]; mid: HTMLElement } {
    const container = document.createElement('div');
    container.tabIndex = -1;
    container.innerHTML = '<button>A</button><div tabindex="-1" data-id="mid">prose</div><button>B</button>';
    document.body.appendChild(container);
    const list = [...container.querySelectorAll<HTMLElement>('button')];
    return { container, list, mid: container.querySelector<HTMLElement>('[data-id="mid"]')! };
  }

  afterEach(() => { document.body.innerHTML = ''; });

  it('steps from the focused container itself to its first or last control', () => {
    const { container, list } = pane();
    expect(tabTargetWithin(container, list, container, false)).toBe(0);
    expect(tabTargetWithin(container, list, container, true)).toBe(1);
  });

  it('steps from an unlisted element to its neighbour in document order', () => {
    const { container, list, mid } = pane();
    expect(tabTargetWithin(container, list, mid, false)).toBe(1);
    expect(tabTargetWithin(container, list, mid, true)).toBe(0);
  });

  it('leaves in-between steps between listed controls native', () => {
    const { container, list } = pane();
    expect(tabTargetWithin(container, list, list[0], false)).toBeNull();
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
    // The drawer's tree, never the pane. A child of the live list, so a
    // leaving drawing's copy never matches.
    expect(PANE_FOCUS_REGION.drawer).toBe('.thread-drawer-list > .thread-drawer-tree');
  });
});

// The per-pane Tab trap follows the marker, not DOM focus. A drawer row click
// leaves the marker on the drawer. The thread switch then focuses the thread's
// live choice card or prompt, and Tab jumped back into the drawer. A
// programmatic focus into a pane must take the marker with it.
describe('focusIntoPane', () => {
  afterEach(() => {
    document.body.innerHTML = '';
    focusedPane.value = 'thread';
  });

  function threadPaneButton(): HTMLButtonElement {
    document.body.innerHTML = `
      <div class="thread-drawer"><button>See all</button></div>
      <div class="pane-thread"><button data-id="thread">Option</button></div>`;
    return document.querySelector('[data-id="thread"]')!;
  }

  it('moves the marker onto the pane that now holds focus', () => {
    const button = threadPaneButton();
    focusedPane.value = 'drawer';
    focusIntoPane(button);
    expect(document.activeElement).toBe(button);
    expect(focusedPane.value).toBe('thread');
  });

  it('leaves the marker alone when the element is outside every pane', () => {
    threadPaneButton();
    const loose = document.createElement('button');
    document.body.appendChild(loose);
    focusedPane.value = 'drawer';
    focusIntoPane(loose);
    expect(document.activeElement).toBe(loose);
    expect(focusedPane.value).toBe('drawer');
  });

  it('leaves the marker alone when the element refuses focus', () => {
    const button = threadPaneButton();
    button.disabled = true;
    focusedPane.value = 'drawer';
    focusIntoPane(button);
    expect(focusedPane.value).toBe('drawer');
  });

  it('is a no-op for a null target', () => {
    focusedPane.value = 'drawer';
    focusIntoPane(null);
    expect(focusedPane.value).toBe('drawer');
  });
});

// The per-pane Tab trap's tabbable-elements query. A closed-but-mounted panel
// (the thread drawer's filter view, `ThreadFilterCover`) marks itself `inert`
// rather than unmounting, so its buttons stay real DOM. The browser already
// skips `inert` content on Tab and refuses to focus it, so this query must
// agree.
describe('visibleFocusables', () => {
  it('excludes a button inside an inert wrapper', () => {
    const container = document.createElement('div');
    const cover = document.createElement('div');
    cover.setAttribute('inert', '');
    const hidden = document.createElement('button');
    stubVisible(hidden);
    cover.appendChild(hidden);
    container.appendChild(cover);

    expect(visibleFocusables(container)).toEqual([]);
  });

  it('excludes an inert element itself, not just its descendants', () => {
    const container = document.createElement('div');
    const el = document.createElement('button');
    el.setAttribute('inert', '');
    stubVisible(el);
    container.appendChild(el);

    expect(visibleFocusables(container)).toEqual([]);
  });

  it('still includes an ordinary tabbable button', () => {
    const container = document.createElement('div');
    const button = document.createElement('button');
    stubVisible(button);
    container.appendChild(button);

    expect(visibleFocusables(container)).toEqual([button]);
  });

  it('counts the container itself when it is a tab stop', () => {
    // A stop that also holds controls, as a tree with a button inside would.
    // Without it, Tab bounces on the control beside the stop forever.
    const stop = document.createElement('div');
    stop.tabIndex = 0;
    stubVisible(stop);
    const link = document.createElement('button');
    stubVisible(link);
    stop.appendChild(link);

    expect(visibleFocusables(stop)).toEqual([stop, link]);
  });

  it('skips a container removed from the tab order', () => {
    // `.pane-thread` / `.pane-content` are tabindex=-1 and never a stop.
    const pane = document.createElement('div');
    pane.tabIndex = -1;
    stubVisible(pane);
    expect(visibleFocusables(pane)).toEqual([]);
  });

  it('excludes a laid-out control under visibility: hidden', () => {
    // A collapsed pane and the drawer list under the filter panel keep their
    // boxes but hide by visibility. The browser refuses to focus them.
    const container = document.createElement('div');
    container.style.visibility = 'hidden';
    const button = document.createElement('button');
    stubVisible(button);
    container.appendChild(button);
    document.body.appendChild(container);

    expect(visibleFocusables(container)).toEqual([]);
    container.remove();
  });

  it('includes summary, media controls and contenteditable', () => {
    const container = document.createElement('div');
    const details = document.createElement('details');
    const summary = document.createElement('summary');
    details.appendChild(summary);
    const video = document.createElement('video');
    video.controls = true;
    const editable = document.createElement('div');
    editable.setAttribute('contenteditable', 'true');
    for (const el of [summary, video, editable]) stubVisible(el);
    container.append(details, video, editable);

    expect(visibleFocusables(container)).toEqual([summary, video, editable]);
  });

  it('a real control stays reachable beside an inert closed panel', () => {
    // Mirrors the thread drawer: one live tabbable control alongside a
    // closed, inert filter view holding several radio-row buttons.
    const container = document.createElement('div');
    const live = document.createElement('button');
    stubVisible(live);
    container.appendChild(live);

    const cover = document.createElement('div');
    cover.setAttribute('inert', '');
    for (let i = 0; i < 3; i++) {
      const radio = document.createElement('button');
      stubVisible(radio);
      cover.appendChild(radio);
    }
    container.appendChild(cover);

    expect(visibleFocusables(container)).toEqual([live]);
  });
});
