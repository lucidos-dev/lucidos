import { focusedPane, type FocusedPane } from '../../store/store';
import { getVisiblePromptInput } from '../chat/promptFocus';
import { isMobile } from '../../utils/viewport';
import { focusIfNeeded, isTextInput } from '../../utils/dom';

/** CSS container for each desktop pane. The drawer is a sibling of the split
 *  layout; the thread/content panes are its two halves — all disjoint subtrees,
 *  each resolved from the `focusedPane` signal via `paneContainer`. */
const PANE_SELECTOR: Record<FocusedPane, string> = {
  drawer: '.thread-drawer',
  thread: '.pane-thread',
  content: '.pane-content',
};

/** Tabbable elements within a pane or an overlay panel: the native controls,
 *  `iframe` (an app content pane), `summary`, media controls and
 *  `contenteditable`. Every native-focusable term ALSO excludes
 *  `[tabindex="-1"]` so an element explicitly removed from the tab order (e.g.
 *  the thread drawer's mouse-only row buttons) is honored exactly as native Tab
 *  does. Without it the per-pane trap would keep cycling a `tabindex=-1`
 *  `<button>`, defeating the drawer's single tab stop. */
const FOCUSABLE = [
  'a[href]', 'button:not([disabled])', 'input:not([disabled])', 'select:not([disabled])',
  'textarea:not([disabled])', 'iframe', 'summary', 'audio[controls]', 'video[controls]',
  '[contenteditable]:not([contenteditable="false"])', '[tabindex]',
].map((term) => `${term}:not([tabindex="-1"])`).join(', ');

/** True when the browser would actually focus `el`: it has a box, no
 *  `visibility: hidden` (a collapsed pane, the drawer list under the filter
 *  panel), and no `inert` ancestor (a closed `ThreadFilterCover`). A candidate
 *  the browser refuses turns the trap's Tab into a dead key. */
function canTakeFocus(el: HTMLElement): boolean {
  if (el.getClientRects().length === 0 || el.closest('[inert]')) return false;
  if (typeof el.checkVisibility === 'function') return el.checkVisibility({ visibilityProperty: true });
  return getComputedStyle(el).visibility !== 'hidden';
}

/** Tabbable elements within `container`, in DOM order. The container comes
 *  first when it is a tab stop itself. Exported for unit testing. */
export function visibleFocusables(container: HTMLElement): HTMLElement[] {
  const self = container.matches(FOCUSABLE) ? [container] : [];
  return [...self, ...container.querySelectorAll<HTMLElement>(FOCUSABLE)].filter(canTakeFocus);
}

export function paneContainer(pane: FocusedPane): HTMLElement | null {
  return document.querySelector<HTMLElement>(PANE_SELECTOR[pane]);
}

/** The pane whose container holds `el`, or null for the header, an overlay or a toast. */
export function paneHolding(el: Element): FocusedPane | null {
  const panes = Object.keys(PANE_SELECTOR) as FocusedPane[];
  return panes.find((pane) => el.closest(PANE_SELECTOR[pane])) ?? null;
}

/** Focus `el` and move the focused-pane marker onto the pane holding it, so
 *  the per-pane Tab trap cycles where focus actually is. Use it for a
 *  programmatic focus that can cross panes, such as the thread-entry focus
 *  after a drawer row click. A pointer needs none: `focusPane` marks the pane. */
export function focusIntoPane(el: HTMLElement | null): void {
  if (!el) return;
  focusIfNeeded(el);
  if (document.activeElement !== el) return;
  const pane = paneHolding(el);
  if (pane) focusedPane.value = pane;
}

/** The keyboard "surface" of each pane — the element that should hold DOM focus
 *  when the focused-pane marker points at the pane, so native scroll keys (Arrow/Page/
 *  Home/End/Space) and the pane's own keydown nav act on it. thread → the
 *  transcript scroll region; content → the content-body scroller; drawer → the
 *  drawer's tree (its ↑/↓/Enter list-nav keydown handler lives on it). The tree
 *  is inside the drawer pane, never the pane itself.
 *
 *  The thread selector is scoped to `.thread-view .thread-content` on purpose:
 *  only a REAL thread (ThreadView, loading or loaded) renders `.thread-content`
 *  inside a `.thread-view`. The compose/welcome view (CreateThreadView) renders a
 *  visible `.thread-content` with NO tabindex and NO `.thread-view` wrapper — it
 *  never becomes a focusable scroll region, so it must NOT match here (else the
 *  mover would retry a permanently non-focusable node instead of falling back to
 *  the prompt). A loading real-thread transcript still matches (it's inside
 *  `.thread-view`) so the mover retries until its `tabindex=0` version mounts.
 *  Exported for the region-map unit test. */
export const PANE_FOCUS_REGION: Record<FocusedPane, string> = {
  drawer: '.thread-drawer-list > .thread-drawer-tree',
  thread: '.thread-view .thread-content',
  content: '.content-pane-body',
};

/** Pure decision for the gentle (signal-only) reconcile: pull DOM focus into the
 *  focused pane only when we're not on mobile (mobile navigates panes, it doesn't
 *  focus them), no overlay owns focus (overlays manage their own focus/Escape via
 *  `overlayStack`), and DOM focus is not ALREADY inside the pane. The last clause
 *  is what makes reconciling never steal a click's own focus (a control clicked
 *  inside the pane). Exported for unit testing. */
export function shouldReconcilePaneFocus(opts: {
  mobile: boolean;
  overlayOpen: boolean;
  focusInsidePane: boolean;
}): boolean {
  return !opts.mobile && !opts.overlayOpen && !opts.focusInsidePane;
}

/** How many frames the focus mover retries while the pane's scroll surface is not
 *  yet laid out / focusable. Covers two desktop races: (1) a pane toggle that
 *  collapses then re-expands (⌘⇧2), whose single-frame layout lags the focus call,
 *  and (2) opening a not-yet-loaded thread, whose transcript renders as a
 *  loading-state `.thread-content` (no `tabindex`, so `.focus()` no-ops) until
 *  events arrive and the real, focusable transcript mounts. ~0.5s at 60fps; a
 *  slower load falls back to "focus stays put" rather than yanking focus later. */
const RECONCILE_MAX_FRAMES = 30;

/** The pane's SCROLL surface if it's present and laid out, else null. This is the
 *  element native Arrow/Page/Home/End keys should scroll (thread → transcript,
 *  content → body scroller) or the drawer tree whose keydown list-nav needs focus.
 *  For a real thread it returns even the loading-state `.thread-content` (a box
 *  with no tabindex yet) so the caller retries until the focusable one mounts; it
 *  returns null for the compose/welcome view (its `.thread-content` isn't inside a
 *  `.thread-view`, so the selector excludes it) so the caller falls back to the
 *  prompt (force) or leaves focus alone (gentle) instead of retrying forever. */
function paneScrollRegion(pane: FocusedPane, container: HTMLElement): HTMLElement | null {
  const region = container.querySelector<HTMLElement>(PANE_FOCUS_REGION[pane]);
  // A collapsed/hidden region (ratio 0/1, or the inactive dual-mount copy) has no
  // box — treat it as absent so we don't strand focus on an off-screen element.
  return region && region.getClientRects().length > 0 ? region : null;
}

/** Forceful fallback when a pane has no scroll region: the thread pane's prompt
 *  (so ⌘⇧2 on a compose/empty thread lands somewhere useful to type), else the
 *  first tabbable element, else null. Returns null rather than the (non-scrollable,
 *  `tabindex=-1`) pane container so the mover retries / leaves focus instead of
 *  stranding it on a container where scroll keys do nothing. Only the forceful
 *  "focus pane" path uses this; the gentle reconcile path deliberately does not
 *  (it must not re-grab the prompt after Escape blurs it). */
function forceFallbackTarget(pane: FocusedPane, container: HTMLElement): HTMLElement | null {
  if (pane === 'thread') {
    const prompt = getVisiblePromptInput();
    if (prompt && container.contains(prompt) && prompt.getClientRects().length > 0) return prompt;
  }
  return visibleFocusables(container)[0] ?? null;
}

/** Shared focus mover behind `reconcilePaneFocus` (force=false) and
 *  `focusPaneMainControl` (force=true). Lands real DOM focus on `pane`'s scroll
 *  surface so native scroll keys / the pane's keydown nav act on the pane the
 *  focused-pane marker points at. Retries for a bounded window so focus reliably lands even
 *  when the pane's surface is laid out / mounted a frame or more late (toggle
 *  collapse→expand, thread-load). Bails when the focused-pane marker moves to another pane
 *  (a newer intent wins) or an overlay takes over (overlays own focus).
 *
 *  - `force` (explicit "focus this pane" — the ⌘⇧ toggles, Search Everywhere):
 *    always moves focus, and falls back to the prompt / first control when there
 *    is no scroll region.
 *  - gentle (signal-only paths + Escape): never steals a click's own in-pane focus
 *    or an in-progress edit elsewhere, and leaves focus untouched when the pane has
 *    no scroll region (so it can't re-grab a just-blurred prompt).
 *
 *  Desktop-only; `preventScroll` so moving focus never causes a scroll jump. */
function focusPaneSurface(pane: FocusedPane, force: boolean): void {
  if (isMobile()) return;
  // Forceful "focus this pane" also moves the focus-line marker onto it, so the
  // marker, DOM focus, and scroll target all agree — and the retry's stale-marker
  // bail below doesn't abort a legitimate destination focus whose navigation left
  // `focusedPane` unchanged (e.g. Search Everywhere selecting a thread while the
  // drawer is the focused pane: with the split open, `revealThreadPane` switches
  // only from 'content'; it takes the marker unconditionally only when it just
  // re-expanded a collapsed thread pane).
  // (`focusPaneAndControl` already set it; this is a redundant no-op there.)
  if (force) focusedPane.value = pane;
  let frames = 0;
  const attempt = (): void => {
    if (focusedPane.value !== pane) return; // a newer focus-line change wins
    const container = paneContainer(pane);
    if (!container) return; // pane not in the DOM → nothing to focus
    const active = document.activeElement;
    if (!force) {
      // Gentle reconcile yields to an open overlay (it owns focus/Escape), never
      // steals a click's own in-pane focus, and never yanks an in-progress edit.
      // The forceful path deliberately skips these: an explicit "focus this pane",
      // or Search Everywhere focusing its destination AS it closes — where
      // `data-overlay-open` may still be set for a frame while the modal tears down.
      const focusInsidePane = active instanceof HTMLElement && container.contains(active);
      const overlayOpen = document.documentElement.hasAttribute('data-overlay-open');
      if (!shouldReconcilePaneFocus({ mobile: false, overlayOpen, focusInsidePane })) return;
      if (isTextInput(active)) return; // don't yank an in-progress edit
    }
    const scroll = paneScrollRegion(pane, container);
    if (scroll) {
      // A real thread's transcript. It may not be focusable yet (loading-state
      // `.thread-content` has no tabindex) — `.focus()` no-ops then. Keep retrying
      // (below) until the `tabindex=0` transcript mounts; do NOT fall back to the
      // prompt, or ⌘⇧2 / Search Everywhere on a not-yet-loaded thread would land
      // the prompt instead of the transcript.
      scroll.focus({ preventScroll: true });
      if (document.activeElement === scroll) return; // took focus
    } else if (force) {
      // No scroll surface: the compose/welcome view (permanently no transcript) or
      // a transient un-laid-out frame. Force lands the prompt / first control so
      // ⌘⇧2 always focuses something useful (retries below while nothing is laid
      // out yet, e.g. mid-expand).
      const fallback = forceFallbackTarget(pane, container);
      if (fallback) {
        fallback.focus({ preventScroll: true });
        if (document.activeElement === fallback) return;
      }
    } else {
      // Gentle with no scroll surface (compose): leave focus put (never re-grab a
      // just-blurred prompt) and don't spin waiting for a surface that won't appear.
      return;
    }
    if (++frames < RECONCILE_MAX_FRAMES) requestAnimationFrame(attempt);
  };
  requestAnimationFrame(attempt);
}

/** Keep real DOM focus in sync with the focused-pane marker: when the marker
 *  moves onto `pane` but keyboard focus is still elsewhere, pull focus into the
 *  pane's scroll surface so native scroll keys act on the pane the marker points
 *  at. Used by the signal-only focus paths (`focusPane` clicks,
 *  `revealContentPane`/`revealThreadPane`, the drawer-hide fallback) and after
 *  Escape blurs a text input — cases that change the marker or drop focus without
 *  landing it in the pane. Gentle: see `focusPaneSurface`. */
export function reconcilePaneFocus(pane: FocusedPane): void {
  focusPaneSurface(pane, false);
}

/** Move real DOM focus into a pane's scroll surface — the ⌘⇧ "focus pane" toggles
 *  and Search Everywhere navigation. thread → the transcript (so Arrow/Page keys
 *  scroll it; typing still lands in the prompt via type-to-focus), falling back to
 *  the prompt for a compose/empty thread; content → the body scroller; drawer →
 *  the drawer (its list-nav keydown handler lives there). Forceful: see
 *  `focusPaneSurface`. */
export function focusPaneMainControl(pane: FocusedPane): void {
  focusPaneSurface(pane, true);
}

/** Which of a landed element's focusables a navigation should hand focus to:
 *  the first one that is not an **explainer** icon, or the first of any kind
 *  when that is all there is.
 *
 *  An explainer is chrome ABOUT a control, never the control (see
 *  `components/shared/Explainer.tsx`), and it sits inside the row's LABEL, so a
 *  row carrying one puts the icon ahead of the setting in DOM order. Taking the
 *  first focusable there lands a Search Everywhere jump on the info button: the
 *  row is right, but Enter opens a dialog instead of operating the setting the
 *  user searched for. The fallback matters because a container whose only
 *  focusable IS its explainer should still take focus rather than none.
 *
 *  Pure and exported for the unit test, the caller below needing a real DOM. */
export function navigationFocusTarget<T extends { classList: { contains(c: string): boolean } }>(
  focusables: T[],
): T | undefined {
  return focusables.find((el) => !el.classList.contains('explainer-btn')) ?? focusables[0];
}

/** Move real DOM focus to the control a navigation landed on within `el`, the
 *  element-scoped analog of `focusPaneMainControl`, for navigation that lands on
 *  a specific control rather than a whole pane (e.g. a Search Everywhere jump to
 *  a Settings row focusing that row's dropdown). Which focusable counts as the
 *  control is `navigationFocusTarget` above. No-op when `el` has no visible
 *  focusable child (a section-title anchor), which is the "(if any)" case.
 *  Desktop-only (mobile auto-focus pops the on-screen keyboard) and deferred one
 *  frame so a just-rendered target is laid out; `preventScroll` so it never
 *  fights a concurrent `scrollIntoView`. */
export function focusFirstFocusableWithin(el: HTMLElement): void {
  if (isMobile()) return;
  requestAnimationFrame(() => {
    navigationFocusTarget(visibleFocusables(el))?.focus({ preventScroll: true });
  });
}

/** Pure boundary logic for the pane and overlay Tab traps: given the count of tabbable
 *  elements, the active element's index among them, and whether Shift is held,
 *  return the index to WRAP to, or `null` when no wrap is needed (the browser's
 *  default Tab keeps focus inside the contiguous pane subtree). Forward Tab off
 *  the last element wraps to the first; Shift+Tab off the first wraps to the
 *  last. An active element not in the set (index `-1`) never wraps. */
export function trapTargetIndex(
  count: number,
  activeIndex: number,
  shift: boolean,
): number | null {
  if (count === 0 || activeIndex < 0) return null;
  if (shift && activeIndex === 0) return count - 1;
  if (!shift && activeIndex === count - 1) return 0;
  return null;
}

/** Pure target logic for the per-pane Tab trap, anchored on the FOCUSED pane.
 *  Inputs: the pane's tabbable count, the active element's index among them,
 *  Shift, and whether DOM focus sits inside the pane. It returns the index to
 *  focus, or `null` to fall through to the browser's default Tab.
 *
 *  - focus OUTSIDE the focused pane (on `<body>` after a signal-only pane
 *    click, or in another pane): pull focus IN. Forward Tab lands on the first
 *    element, Shift+Tab on the last.
 *  - focus inside the pane on a listed element: wrap at the boundaries via
 *    `trapTargetIndex`. `null` in-between lets the browser step through the
 *    contiguous subtree.
 *  - focus inside the pane on an element the list misses never reaches here:
 *    `tabTargetWithin` steps it to its neighbour first.
 *  - `count === 0`: nothing to focus, fall through.
 *
 *  This is what makes Tab respect the focused panel even when DOM focus never
 *  entered it (a click sets `focusedPane` signal-only, see `focusPane`). */
export function paneTabTarget(
  count: number,
  activeIndex: number,
  shift: boolean,
  activeInPane: boolean,
): number | null {
  if (count === 0) return null;
  if (!activeInPane) return shift ? count - 1 : 0;
  return trapTargetIndex(count, activeIndex, shift);
}

/** The index one Tab press moves to within `container`, or `null` to let the
 *  browser step. Shared by the pane trap and the overlay trap.
 *
 *  Focus can sit inside `container` on an element the list leaves out: a
 *  `tabindex=-1` pane container a click focused, or a dialog body. Native Tab
 *  from there can leave the container, so it steps to the listed neighbour in
 *  document order instead, wrapping at the ends. */
export function tabTargetWithin(
  container: HTMLElement,
  focusables: HTMLElement[],
  active: Element | null,
  shift: boolean,
): number | null {
  const activeIndex = active ? focusables.indexOf(active as HTMLElement) : -1;
  const inside = !!active && container.contains(active);
  if (inside && activeIndex < 0 && focusables.length > 0) return neighbourIndex(focusables, active!, shift);
  return paneTabTarget(focusables.length, activeIndex, shift, inside);
}

function neighbourIndex(focusables: HTMLElement[], active: Element, shift: boolean): number {
  const follows = (a: Node, b: Node) => (a.compareDocumentPosition(b) & Node.DOCUMENT_POSITION_FOLLOWING) !== 0;
  if (shift) {
    for (let i = focusables.length - 1; i >= 0; i--) if (follows(focusables[i], active)) return i;
    return focusables.length - 1;
  }
  const next = focusables.findIndex((el) => follows(active, el));
  return next >= 0 ? next : 0;
}

/** True when `el` is an `<iframe>` living inside the content pane. A click inside
 *  ANY iframe — an app, a same-origin file/HTML preview, a PDF's native plugin, a
 *  cross-origin URL preview — moves the host's `document.activeElement` to the
 *  `<iframe>` element and fires `window`'s `blur`, but that click never reaches
 *  the content pane's `onPointerDown={() => focusPane('content')}`
 *  (`SplitLayout.tsx`), so the focus marker (and its header pill) would go stale.
 *  This is the one primitive that catches every DOM-iframe surface uniformly,
 *  including the ones the host cannot instrument directly (a PDF plugin swallows
 *  pointer events before they reach `contentDocument`; a cross-origin preview
 *  blocks `contentDocument` access entirely). Exported for unit testing.
 *
 *  The Tauri desktop URL preview is a NATIVE overlay webview, not a DOM iframe —
 *  it has no `<iframe>` element, so neither this check nor `blur` can see it. That
 *  gap is tracked in `docs/known-gaps.md`. */
export function isContentPaneIframeFocus(el: Element | null): boolean {
  return !!el && el.tagName === 'IFRAME' && !!el.closest?.('.pane-content');
}

/** Move the content-pane focus marker when keyboard focus lands inside a
 *  content-pane iframe (see `isContentPaneIframeFocus`). `window`'s `blur` fires
 *  as focus crosses into an iframe; the host's `document.activeElement` updates to
 *  the `<iframe>` a tick later, so the check is deferred a macrotask. Desktop-only
 *  — the focus marker is a desktop affordance and mobile navigates panes rather
 *  than focusing them (`focusedPane` is inert there). Marker-only + idempotent (a
 *  Preact signal write dedupes on an unchanged value), so it never steals DOM
 *  focus from the iframe. Returns a cleanup that removes the listener. */
export function installContentPaneIframeFocusTracking(): () => void {
  const onBlur = (): void => {
    if (isMobile()) return;
    // activeElement is not the <iframe> synchronously on blur — defer one macrotask.
    setTimeout(() => {
      if (isContentPaneIframeFocus(document.activeElement)) focusedPane.value = 'content';
    }, 0);
  };
  window.addEventListener('blur', onBlur);
  return () => window.removeEventListener('blur', onBlur);
}

/** Per-pane Tab trap. Runs after `handleOverlayTab`, so no open overlay owns
 *  the key by now. Tab/Shift+Tab cycle within the FOCUSED pane, and move INTO
 *  it when DOM focus is currently elsewhere. Anchored on `focusedPane` (the
 *  user's intent), not `document.activeElement.closest()`: a pane click sets the
 *  focused pane signal-only and never moves DOM focus, so keying off the active
 *  element let Tab escape to document order (or cycle the wrong pane) after a
 *  click. Switch panes with the ⌘⇧ pane shortcuts or a click. Returns `true`
 *  when it moved focus (caller preventDefaults). Desktop-only. */
export function handlePaneTab(e: KeyboardEvent): boolean {
  if (isMobile()) return false;
  const container = paneContainer(focusedPane.value);
  if (!container) return false; // focused pane not in the DOM → normal Tab
  const focusables = visibleFocusables(container);
  const target = tabTargetWithin(container, focusables, document.activeElement, e.shiftKey);
  if (target === null) return false;
  focusables[target].focus({ preventScroll: true });
  // A focus the browser refused leaves the key to native Tab, never dead.
  return document.activeElement === focusables[target];
}
