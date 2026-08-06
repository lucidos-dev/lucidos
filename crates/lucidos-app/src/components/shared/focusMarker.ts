/** Navigation focus marker — the "focus stick".
 *
 *  Shared by every host navigation that lands on a specific inner element in a
 *  scroll container: a chat event or change (`scrollState.ts`), a settings
 *  item (`SettingsView.tsx`), a plugin row (`StoreTab.tsx`). A persistent
 *  BACKGROUND HIGHLIGHT appears on the landed element and sticks until the user
 *  takes ANY action (a new marker (supersede), an explicit scroll/dismiss, a
 *  scroll gesture, a keypress, or a click), at which point it dissolves, though
 *  never before the HOLD below has elapsed. There is deliberately no
 *  timeout: nothing removes it unless the user engages.
 *
 *  The visual lives in `.nav-focus-stuck` (+ `.nav-focus-fading` during the
 *  fade-out) in styles/global/host-components.css: an accent wash filling the
 *  element's box plus a soft glow blooming past it, Slack-style rather than a
 *  frame around the box, all on one `box-shadow` so it follows border-radius and
 *  never shifts layout.
 *
 *  The lifecycle is a light being switched on: it RAMPS UP from nothing over 0.45s
 *  and stops there, monotonically, with no overshoot to sag back from; it then HOLDS
 *  at full for at least NAV_FOCUS_HOLD_MS however fast the user engages; and only
 *  after that does the user's action DISSOLVE it, over the slower
 *  NAV_FOCUS_FADE_MS. Nothing removes it on a timer, so a landing the user glances
 *  away from is still lit when they look back. See docs/glossary.md § "Navigation
 *  focus marker". */

import { watchUserAction } from '../../utils/userAction';

/** Persistent highlight class (cleared on the first user action) + the transient
 *  fade-out class (runs the wash + glow out to transparent before the marker is
 *  removed, as an ANIMATION rather than a transition: see the rule in
 *  host-components.css for why a transition cannot do this job). */
const NAV_FOCUS_STUCK_CLASS = 'nav-focus-stuck';
const NAV_FOCUS_FADING_CLASS = 'nav-focus-fading';
/** Dissolve duration. Must match the `nav-focus-spotlight-off` animation in
 *  host-components.css (pinned by nav-focus-marker-paint.test.ts, which compares
 *  the two). Long enough to drain rather than blink off, and no longer than that: it
 *  shipped at 2.5s, which left the marker visibly on its way out long after the user
 *  had moved on, because the dismiss is triggered BY the action that moves them on.
 *  Time at FULL brightness is what a landing needs, and that is NAV_FOCUS_HOLD_MS's
 *  job; once the user has engaged, a light that takes its time going out is just
 *  latency. It is ONLY the visual lifetime; how long the marker counts as the current
 *  landing is a separate, frame-based clock, so this can be retuned freely (see
 *  `navFocusElement`). */
export const NAV_FOCUS_FADE_MS = 800;

/** Turn-on duration. Must match the `nav-focus-spotlight-on` animation in
 *  host-components.css (pinned by nav-focus-marker-paint.test.ts, which compares the
 *  two). Known here only so the hold below can start when the ramp ENDS. */
export const NAV_FOCUS_RAMP_MS = 450;

/** How long the marker is guaranteed to sit at FULL brightness before it will begin
 *  to dissolve, however quickly the user engages. Measured from the end of the ramp,
 *  not from the landing: the timer is armed at `NAV_FOCUS_RAMP_MS + NAV_FOCUS_HOLD_MS`
 *  (or just the hold under reduced motion, where there is no ramp and the marker is
 *  at full from the first frame). Arming it at the landing instead would silently
 *  spend the ramp out of the hold and deliver ~1.55s at full rather than the 2s this
 *  says.
 *
 *  Without a hold the lifecycle had no "on" phase to speak of: a landing is nearly
 *  always followed within a moment by a scroll or a tap, so the dissolve began about
 *  as soon as the turn-on finished and the light read as going out rather than as
 *  being on. A dismissal arriving inside the hold is not discarded, it is banked and
 *  runs the instant the hold expires, so acting early shortens nothing except the
 *  wait.
 *
 *  It is WALL-CLOCK time, not visible-painted time, and that is a deliberate limit
 *  rather than an oversight. Land, press a key, and switch tabs immediately, and the
 *  hold and then the dissolve both run to completion in the background, so coming
 *  back after ~3s finds nothing. Making the hold track `document.visibilityState`
 *  would close that, at the cost of another pausable clock in a module whose bugs
 *  have all been clock-interaction bugs. It is also not what the persistence
 *  guarantee protects: that covers a user who has NOT engaged, and this path starts
 *  with a keydown, which has dismissed the marker by design since 96b2c8e2a. The
 *  behaviour is strictly better than before the hold existed, when the same keypress
 *  dismissed it outright and the marker was gone 0.4s later. */
export const NAV_FOCUS_HOLD_MS = 2000;

/** The element wearing the marker's CLASSES (including all the way through the
 *  dissolve), and the teardown for its action listeners + any in-flight timers.
 *  Plain module state: only one marker is ever active across the host (a new one
 *  supersedes the old). */
let _markedEl: HTMLElement | null = null;
/** Whether `_markedEl` still counts as THE current landing, which is a shorter
 *  life than the classes it wears. See `navFocusElement`. */
let _refLive = false;
let _teardown: (() => void) | null = null;
/** The hold: `_holdOver` flips when it expires, `_dismissQueued`
 *  records that the user already engaged during it so the dissolve can start the
 *  moment it does. Both reset on every apply. */
let _holdOver = false;
let _dismissQueued = false;
/** Cancels the pending one-frame ref expiry, if one is armed. Its own slot rather than
 *  part of `_teardown`, because it can be armed from either point where the user
 *  engages (a banked dismissal during the hold, or the dissolve starting) and must be
 *  cancellable from both. */
let _cancelRefExpiry: (() => void) | null = null;
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

/** Stop counting `_markedEl` as the current landing, one frame from now.
 *
 *  A frame, rather than synchronously, so a handler acting on the very event that
 *  retired it can still read the marked element: a keydown-driven consumer reading
 *  `navFocusElement()` in a later (bubble-phase) listener, namely the transcript
 *  turn-toggle Enter, would otherwise see null (`ce327ed24`).
 *
 *  Called from BOTH points where the user engages, which is the whole reason it is a
 *  helper: when a dismissal is banked during the hold, and when the dissolve starts.
 *  The ref clock tracks engagement and the paint clock tracks visibility, and they
 *  are only the same event when there is no hold in between.
 *
 *  `dropEl` also drops the element itself, for the reduced-motion path where nothing
 *  is left painted. Gated on the generation captured by the caller, so a frame left
 *  queued by a hidden tab cannot retire a later marker. Falls back to expiring
 *  synchronously where rAF is unavailable (non-browser), which costs only the Enter
 *  re-assert in an environment that has no frames to defer to anyway. */
function retireRefNextFrame(gen: number, dropEl: boolean): void {
  _cancelRefExpiry?.();
  _cancelRefExpiry = null;
  const expire = () => {
    if (_generation !== gen) return;
    _refLive = false;
    if (dropEl) _markedEl = null;
  };
  if (typeof requestAnimationFrame === 'function') {
    const raf = requestAnimationFrame(expire);
    _cancelRefExpiry = () => cancelAnimationFrame(raf);
  } else {
    expire();
  }
}

function prefersReducedMotion(): boolean {
  return (
    typeof window !== 'undefined' &&
    typeof window.matchMedia === 'function' &&
    window.matchMedia('(prefers-reduced-motion: reduce)').matches
  );
}

/** Apply the navigation focus marker to `el`, superseding any prior marker, and
 *  arm the listeners that dissolve it on the user's next action.
 *
 *  `opts.settleGuard` — when it returns true, a user action is IGNORED (the marker
 *  stays, listeners stay armed). Callers whose landing involves an in-flight
 *  programmatic smooth scroll pass a predicate that's true until that scroll settles
 *  (chat passes `hasPendingEventScroll`), so the landing scroll can't self-clear the
 *  marker before the user has actually engaged. Synchronous landings omit it.
 *
 *  Distinct from the HOLD below, which also defers a dismissal but REMEMBERS it: the
 *  settle guard says "that wasn't you", the hold says "that was you, but the light
 *  has only just come on". */
export function applyNavFocus(el: HTMLElement, opts?: { settleGuard?: () => boolean }): void {
  clearNavFocus();
  el.classList.remove(NAV_FOCUS_FADING_CLASS);
  el.classList.add(NAV_FOCUS_STUCK_CLASS);
  _markedEl = el;
  _refLive = true;
  _generation++;
  _holdOver = false;
  _dismissQueued = false;

  // What dismisses the marker is a USER ACTION, defined once in
  // `utils/userAction.ts` and shared with the chat deep-link's fallback scroll,
  // which stands down on the same signal. Programmatic scrollTop writes (a
  // landing scroll, streaming auto-scroll) are deliberately outside that set, so
  // they can't self-clear the marker.
  const gen = _generation;
  const settleGuard = opts?.settleGuard;
  const removeListeners = watchUserAction(() => {
    // Defer while a programmatic landing scroll is still settling: only an
    // action AFTER it settles means the user is engaging, not the landing.
    if (settleGuard?.()) return;
    // Inside the hold: the user HAS engaged, but the marker has only just lit and
    // must be allowed to read as ON before it starts going. Bank the dismissal and
    // let the hold timer run it. Listeners stay armed; a further action just re-sets
    // the same flag, and `fadeOutNavFocus` is what finally tears them down.
    if (!_holdOver) {
      // Only the FIRST action schedules the expiry. `retireRefNextFrame` cancels any
      // pending one before arming its own, so calling it per action would let a
      // continuous wheel burst (which fires faster than once per frame) push the
      // deadline along ahead of itself and keep the ref live for the whole gesture.
      if (_dismissQueued) return;
      _dismissQueued = true;
      // Retire the REF here, not when the dissolve eventually starts. The user has
      // engaged, and that is the only question `navFocusElement` answers, so holding
      // the paint must not also hold the ref: turn-nav treats a live marker as proof
      // that no scroll has happened since the last nav, and a ref that outlived the
      // scroll by the whole hold would make ⌘↑/⌘↓ step from the turn the user just
      // scrolled away from. Same one-frame deferral as the dissolve path, so the
      // Enter that banks the dismissal can still toggle its own turn.
      retireRefNextFrame(gen, false);
      return;
    }
    fadeOutNavFocus();
  });

  const holdTimer = setTimeout(
    () => {
      if (_generation !== gen) return;
      _holdOver = true;
      if (_dismissQueued) fadeOutNavFocus();
    },
    // The hold is time at FULL brightness, so it starts where the ramp ends. Under
    // reduced motion there is no ramp (the CSS drops it) and the marker is at full
    // from the first frame, so there is nothing to wait out.
    (prefersReducedMotion() ? 0 : NAV_FOCUS_RAMP_MS) + NAV_FOCUS_HOLD_MS,
  );

  _teardown = () => {
    removeListeners();
    clearTimeout(holdTimer);
  };
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

  // Usually a no-op by now, because the ref retires the moment the user engages and
  // that is either this same call or the bank branch in `applyNavFocus`. It still has
  // to run for the paths that reach a dissolve WITHOUT a banked dismissal: reduced
  // motion (which also drops the element here, nothing being left painted), and a
  // programmatic dissolve. Generation-gated, not identity-gated: the same element can
  // be legitimately re-marked, so `_markedEl === el` cannot tell "still mine" from
  // "someone else's turn now".
  const gen = _generation;
  retireRefNextFrame(gen, reduced);

  if (reduced) return;

  const fadeTimer = setTimeout(() => {
    if (_generation !== gen) return;
    el.classList.remove(NAV_FOCUS_STUCK_CLASS);
    el.classList.remove(NAV_FOCUS_FADING_CLASS);
    _markedEl = null;
    _refLive = false;
    _teardown = null;
  }, NAV_FOCUS_FADE_MS);
  // A supersede / explicit clear cancels this and removes the classes; the pending ref
  // expiry is cancelled separately by `clearNavFocus`, which owns `_cancelRefExpiry`
  // because that one can be armed from either engagement point.
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
  _cancelRefExpiry?.();
  _cancelRefExpiry = null;
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
