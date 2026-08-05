/** Navigation focus marker — the "focus stick".
 *
 *  Shared by every host navigation that lands on a specific inner element in a
 *  scroll container: a chat event or change (`scrollState.ts`), a settings
 *  item (`SettingsView.tsx`), a plugin row (`StoreTab.tsx`). A persistent
 *  BACKGROUND HIGHLIGHT appears on the landed element and sticks until the user
 *  takes ANY action (a new marker (supersede), an explicit scroll/dismiss, a
 *  scroll gesture, a keypress, or a click), at which point it smoothly fades out.
 *  There is deliberately no timeout.
 *
 *  The visual lives in `.nav-focus-stuck` (+ `.nav-focus-fading` during the
 *  fade-out) in styles/global/host-components.css: an accent wash filling the
 *  element's box plus a soft glow blooming past it, Slack-style rather than a
 *  frame around the box, all on one `box-shadow` so it follows border-radius and
 *  never shifts layout. It turns ON over 0.5s (a ramp from nothing, surging past
 *  the resting values and settling into them) and, once dismissed, dissolves over
 *  the much slower NAV_FOCUS_FADE_MS. Because the marker PERSISTS between those two
 *  (there is no timeout), a landing the user glances away from is still marked when
 *  they look back; only the ramp itself can be missed. See docs/glossary.md
 *  § "Navigation focus marker". */

import { watchUserAction } from '../../utils/userAction';

/** Persistent highlight class (cleared on the first user action) + the transient
 *  fade-out class (drives the wash + glow → transparent transition before the
 *  marker is removed). */
const NAV_FOCUS_STUCK_CLASS = 'nav-focus-stuck';
const NAV_FOCUS_FADING_CLASS = 'nav-focus-fading';
/** Dissolve duration. Must match the `nav-focus-spotlight-off` animation in
 *  host-components.css (pinned by nav-focus-marker-paint.test.ts, which compares
 *  the two). Deliberately long: the dismiss is a slow dissolve at the edge of
 *  attention, not a blink-off. It is ONLY the visual lifetime; how long the marker
 *  counts as the current landing is a separate, frame-based clock, so this can be
 *  retuned freely (see `navFocusElement`). */
export const NAV_FOCUS_FADE_MS = 1500;

/** The element wearing the marker's CLASSES (including all the way through the
 *  dissolve), and the teardown for its action listeners + any in-flight timers.
 *  Plain module state: only one marker is ever active across the host (a new one
 *  supersedes the old). */
let _markedEl: HTMLElement | null = null;
/** Whether `_markedEl` still counts as THE current landing, which is a shorter
 *  life than the classes it wears. See `navFocusElement`. */
let _refLive = false;
let _teardown: (() => void) | null = null;
/** Bumped by every event that invalidates work already scheduled: a new marker, an
 *  explicit clear, and a dissolve running to completion. Deferred callbacks capture
 *  it and do nothing if it has moved on.
 *
 *  This exists because cancellation is not guaranteed to be reachable. A dissolve
 *  arms two deferred steps, an rAF and a timer, and `_teardown` cancels both; but
 *  the timer's own completion nulls `_teardown`, and in a HIDDEN TAB the two fire
 *  out of order (rAF callbacks are starved while `setTimeout` keeps running), so the
 *  timer can finish while the rAF is still queued and no longer cancellable. A
 *  re-mark of the SAME element then defeated an identity guard (`_markedEl === el`
 *  is true again for a legitimately fresh marker), and the stale frame retired a
 *  live marker's ref: visible highlight, `navFocusElement()` null, nothing pending
 *  to repair it, so turn-nav lost its anchor and Enter-toggle silently no-opped
 *  until the next navigation. A generation token cannot be fooled that way. */
let _generation = 0;

/** True while a navigation focus marker is the CURRENT landing. Follows
 *  `navFocusElement`, so it goes false a frame after a dismissal begins, not when
 *  the dissolve finishes. */
export function hasNavFocus(): boolean {
  return navFocusElement() !== null;
}

/** The element carrying the current navigation focus marker, or null. Read-only
 *  accessor over the module state, letting a caller act on the "highlighted"
 *  element (e.g. Enter toggling the collapse of the ⌘↑/⌘↓-navigated transcript
 *  turn) without owning a parallel highlight signal.
 *
 *  It reports the marker as CURRENT for a shorter time than the marker is visible:
 *  once dismissed it goes null on the next frame, while the classes stay on for the
 *  whole dissolve. The two are deliberately decoupled, because consumers ask
 *  different questions of this than "is something still painted".
 *
 *  - `scrollState.ts`'s turn-nav anchors index-stepping on the marked turn *because*
 *    a marker means the user has not scrolled since the last nav. A dismissal means
 *    they have. Reporting a dissolving marker as current would make ⌘↑/⌘↓ step from
 *    the turn the user just scrolled away from instead of falling back to the
 *    scroll-position pick, and the longer the dissolve is tuned, the wider that
 *    window gets.
 *  - The Enter-toggles-the-marked-turn shortcut needs the opposite: the ref must
 *    outlive the very keydown that dismissed it, because the capture-phase clear
 *    runs before the bubble-phase handler reads this (see ce327ed24, which fixed
 *    exactly that for reduced motion by deferring its ref-drop one frame).
 *
 *  One frame satisfies both, and it holds however long the dissolve is. */
export function navFocusElement(): HTMLElement | null {
  return _refLive ? _markedEl : null;
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
  _refLive = true;
  _generation++;

  // What dismisses the marker is a USER ACTION, defined once in
  // `utils/userAction.ts` and shared with the chat deep-link's fallback scroll,
  // which stands down on the same signal. Programmatic scrollTop writes (a
  // landing scroll, streaming auto-scroll) are deliberately outside that set, so
  // they can't self-clear the marker.
  const settleGuard = opts?.settleGuard;
  const removeListeners = watchUserAction(() => {
    // Defer while a programmatic landing scroll is still settling: only an
    // action AFTER it settles means the user is engaging, not the landing.
    if (settleGuard?.()) return;
    fadeOutNavFocus();
  });

  _teardown = () => removeListeners();
}

/** Dissolve the marker, then remove it. Triggered by the user's first action.
 *  Tears the action listeners down immediately so a follow-up action can't
 *  re-enter, adds the fade class (CSS runs the wash + glow out to transparent),
 *  and removes the classes once the dissolve completes. Under reduced motion the
 *  dissolve is skipped and the classes come off at once.
 *
 *  Two clocks, deliberately: the CLASSES live for the whole dissolve, but the
 *  marker stops counting as the CURRENT landing after one frame (`_refLive`, see
 *  `navFocusElement` for why each consumer needs its own answer). One frame is what
 *  the reduced-motion path has always used, and running both paths on the same
 *  frame-based expiry is what keeps `navFocusElement` independent of however long
 *  the dissolve is tuned to. */
function fadeOutNavFocus(): void {
  const el = _markedEl;
  if (!el) return;
  _teardown?.();
  _teardown = null;

  const reduced = prefersReducedMotion();
  if (reduced) {
    el.classList.remove(NAV_FOCUS_STUCK_CLASS);
    el.classList.remove(NAV_FOCUS_FADING_CLASS);
  } else {
    el.classList.add(NAV_FOCUS_FADING_CLASS);
  }

  // Expire the ref on the NEXT frame, not synchronously, so a handler acting on the
  // very event that triggered this clear can still read the marked element. Without
  // it a keydown-driven consumer reading `navFocusElement()` in a later
  // (bubble-phase) listener, namely the transcript turn-toggle Enter, would see
  // null. Under reduced motion there is nothing left to paint, so the same frame
  // drops the element ref too. A same-event re-assert (`applyNavFocus`) cancels this
  // via `_teardown`. Falls back to expiring synchronously where rAF is unavailable
  // (non-browser), which costs only the Enter re-assert in an environment that has
  // no frames to defer to anyway.
  // Both deferred steps below are gated on the generation captured here, not on
  // `_markedEl === el`: the same element can be legitimately re-marked, so identity
  // does not distinguish "still mine" from "someone else's turn now".
  const gen = _generation;
  const expire = () => {
    if (_generation !== gen) return;
    _refLive = false;
    if (reduced) _markedEl = null;
  };
  let cancelExpiry = () => {};
  if (typeof requestAnimationFrame === 'function') {
    const raf = requestAnimationFrame(() => {
      if (_generation !== gen) return;
      expire();
      if (reduced) _teardown = null;
    });
    cancelExpiry = () => cancelAnimationFrame(raf);
  } else {
    expire();
  }

  if (reduced) {
    _teardown = cancelExpiry;
    return;
  }

  const fadeTimer = setTimeout(() => {
    if (_generation !== gen) return;
    el.classList.remove(NAV_FOCUS_STUCK_CLASS);
    el.classList.remove(NAV_FOCUS_FADING_CLASS);
    _markedEl = null;
    _refLive = false;
    _teardown = null;
    // The cycle is over, so retire the generation. Without this, a sibling rAF that
    // the hidden-tab ordering left queued would still match and could retire the ref
    // of whatever marker comes next.
    _generation++;
  }, NAV_FOCUS_FADE_MS);
  // A supersede / explicit clear cancels BOTH pending steps and removes the classes.
  _teardown = () => {
    cancelExpiry();
    clearTimeout(fadeTimer);
  };
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
  // `_markedEl` outlives `_refLive` during a dissolve, so this is the one place the
  // two are reset together: clearing while an element is mid-dissolve must strip its
  // classes (the cancelled timer will never do it) AND retire the ref.
  _refLive = false;
  // Anything a previous cycle deferred is void from here, whether or not `_teardown`
  // was still around to cancel it.
  _generation++;
}
