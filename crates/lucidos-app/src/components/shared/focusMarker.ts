/** Navigation focus marker — the "focus stick".
 *
 *  Shared by every host navigation that lands on a specific inner element in a
 *  scroll container: a chat event/change exchange (`scrollState.ts`), a settings
 *  item (`SettingsView.tsx`), a plugin row (`StoreTab.tsx`). A persistent BORDER
 *  appears on the landed element and sticks until the user takes ANY action —
 *  a new marker (supersede), an explicit scroll/dismiss, a scroll gesture, a
 *  keypress, or a click — at which point it smoothly fades out. There is
 *  deliberately no timeout and no entrance animation (just the border).
 *
 *  The visual lives in `.nav-focus-stuck` (+ `.nav-focus-fading` during the
 *  fade-out) in styles/global/host-components.css: the border is an `outline`
 *  with `outline-offset`, so it sits at the element's OUTER edge with a uniform
 *  gap to content, follows border-radius, and never shifts layout. See
 *  docs/glossary.md § "Navigation focus marker". */

/** Persistent border class (cleared on the first user action) + the transient
 *  fade-out class (drives the outline-color → transparent transition before the
 *  marker is removed). */
const NAV_FOCUS_STUCK_CLASS = 'nav-focus-stuck';
const NAV_FOCUS_FADING_CLASS = 'nav-focus-fading';
/** Fade-out duration — must match the `.nav-focus-fading` transition in
 *  host-components.css. */
export const NAV_FOCUS_FADE_MS = 400;

/** The element currently carrying the marker (including while it fades out), and
 *  the teardown for its action listeners + any in-flight fade timer. Plain module
 *  state — only one marker is ever active across the host (a new one supersedes
 *  the old). */
let _markedEl: HTMLElement | null = null;
let _teardown: (() => void) | null = null;

/** User actions that dismiss the marker. `wheel`/`touchmove` cover scrolling,
 *  `pointerdown` covers clicks + taps, `keydown` covers any keypress. Programmatic
 *  scrollTop writes (a landing scroll, streaming auto-scroll) never emit these,
 *  so they can't self-clear the marker. */
const FOCUS_CLEAR_EVENTS = ['wheel', 'touchmove', 'pointerdown', 'keydown'] as const;

/** True while an element is carrying the navigation focus marker (including while
 *  it is fading out). */
export function hasNavFocus(): boolean {
  return _markedEl !== null;
}

/** The element currently carrying the navigation focus marker (including while it
 *  fades out), or null. Read-only accessor over the module state — lets a caller
 *  act on the "highlighted" element (e.g. Enter toggling the collapse of the
 *  ⌘↑/⌘↓-navigated transcript turn) without owning a parallel highlight signal. */
export function navFocusElement(): HTMLElement | null {
  return _markedEl;
}

function prefersReducedMotion(): boolean {
  return (
    typeof window !== 'undefined' &&
    typeof window.matchMedia === 'function' &&
    window.matchMedia('(prefers-reduced-motion: reduce)').matches
  );
}

/** Apply the navigation focus marker to `el`, superseding any prior marker, and
 *  arm the listeners that fade it out on the user's next action.
 *
 *  `opts.settleGuard` — when it returns true, a user action is IGNORED (the marker
 *  stays). Callers whose landing involves an in-flight programmatic smooth scroll
 *  pass a predicate that's true until that scroll settles (chat passes
 *  `hasPendingEventScroll`), so the landing scroll can't self-clear the marker
 *  before the user has actually engaged. Synchronous landings omit it. */
export function applyNavFocus(el: HTMLElement, opts?: { settleGuard?: () => boolean }): void {
  clearNavFocus();
  el.classList.remove(NAV_FOCUS_FADING_CLASS);
  el.classList.add(NAV_FOCUS_STUCK_CLASS);
  _markedEl = el;

  let removeListeners: (() => void) | null = null;
  if (typeof document !== 'undefined' && document.addEventListener) {
    const settleGuard = opts?.settleGuard;
    const onAction = () => {
      // Defer while a programmatic landing scroll is still settling — only an
      // action AFTER it settles means the user is engaging, not the landing.
      if (settleGuard?.()) return;
      fadeOutNavFocus();
    };
    // Capture phase so an action clears regardless of where it lands; passive
    // because we never preventDefault.
    for (const type of FOCUS_CLEAR_EVENTS) {
      document.addEventListener(type, onAction, { capture: true, passive: true });
    }
    removeListeners = () => {
      for (const type of FOCUS_CLEAR_EVENTS) {
        document.removeEventListener(type, onAction, { capture: true } as EventListenerOptions);
      }
    };
  }

  _teardown = () => removeListeners?.();
}

/** Fade the marker out, then remove it. Triggered by the user's first action.
 *  Tears the action listeners down immediately so a follow-up action can't
 *  re-enter, adds the fade class (CSS transitions the outline to transparent),
 *  and removes the marker once the fade completes. Under reduced motion the fade
 *  is skipped and the marker is removed at once. */
function fadeOutNavFocus(): void {
  const el = _markedEl;
  if (!el) return;
  _teardown?.();
  _teardown = null;

  if (prefersReducedMotion()) {
    el.classList.remove(NAV_FOCUS_STUCK_CLASS);
    el.classList.remove(NAV_FOCUS_FADING_CLASS);
    // Drop the ref on the NEXT frame, not synchronously, so a handler acting on
    // the very event that triggered this clear can still read the marked element
    // — matching the normal-motion path, where the fade timer keeps `_markedEl`
    // set past the triggering event. Without this, a keydown-driven consumer that
    // reads `navFocusElement()` in a later (bubble-phase) listener — the
    // transcript turn-toggle Enter — would see null under reduced motion only. A
    // same-event re-assert (`applyNavFocus`) cancels this via `_teardown`. Falls
    // back to a synchronous null where rAF is unavailable (non-browser).
    if (typeof requestAnimationFrame === 'function') {
      const raf = requestAnimationFrame(() => {
        if (_markedEl === el) _markedEl = null;
        _teardown = null;
      });
      _teardown = () => cancelAnimationFrame(raf);
    } else {
      _markedEl = null;
    }
    return;
  }

  el.classList.add(NAV_FOCUS_FADING_CLASS);
  const fadeTimer = setTimeout(() => {
    el.classList.remove(NAV_FOCUS_STUCK_CLASS);
    el.classList.remove(NAV_FOCUS_FADING_CLASS);
    if (_markedEl === el) _markedEl = null;
    _teardown = null;
  }, NAV_FOCUS_FADE_MS);
  // A supersede / explicit clear cancels the pending fade and removes the classes.
  _teardown = () => clearTimeout(fadeTimer);
}

/** Remove the marker immediately (both classes) and tear down its action
 *  listeners + any in-flight fade timer. Idempotent. Used for supersede and for
 *  explicit programmatic clears (e.g. chat's `clearPendingEventScroll`); user
 *  actions go through the fade-out path instead. */
export function clearNavFocus(): void {
  if (_teardown) {
    _teardown();
    _teardown = null;
  }
  if (_markedEl) {
    _markedEl.classList.remove(NAV_FOCUS_STUCK_CLASS);
    _markedEl.classList.remove(NAV_FOCUS_FADING_CLASS);
    _markedEl = null;
  }
}
