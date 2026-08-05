import { signal } from '@preact/signals';
import { prefersReducedMotion } from '../../utils/platform';
import { isMobile } from '../../utils/viewport';
import { applyNavFocus, clearNavFocus, navFocusElement } from '../shared/focusMarker';
import { watchUserAction } from '../../utils/userAction';

/** Shared scroll-position signals for the chat area.
 *  ThreadView and CreateThreadView are mounted twice each (desktop SplitLayout
 *  + mobile MobileSwipeContainer), so writers MUST gate on isElementVisible(el)
 *  before mutating these signals — see makeScrollObservers below. The hidden
 *  duplicate's element has 0×0 dimensions and would otherwise overwrite the
 *  visible instance's correct values (e.g. clearing notAtTop because the hidden
 *  el reports isScrollable=false).
 *
 *  Two thresholds, two purposes:
 *  - `scrolledUp` uses an 80px stickiness window: while inside it, content
 *    growth during streaming still auto-scrolls to bottom and keyboard/header
 *    flows treat the user as bottom-pinned. Crossing the window means the
 *    user has chosen to read history, so auto-scroll backs off.
 *  - `awayFromBottom` flips on the very first pixel of scroll-up so the
 *    scroll-to-bottom chevron appears immediately, independent of stickiness. */
export const scrolledUp = signal(false);
export const awayFromBottom = signal(false);
export const notAtTop = signal(false);
/** True while a notification deep-link is resolving a scroll to a specific event
 *  or change. The thread render is WINDOWED (only a tail of exchanges is in the
 *  DOM), so a deep-link target above the window would never render for
 *  scrollToSelectorAndPulse to find. ThreadView reads this and renders ALL
 *  exchanges while it's true (and keeps them rendered afterwards, so the user
 *  isn't snapped when it clears). A signal (not the plain `_pendingEventScrollClaim`
 *  var) so the render subscribes. */
export const deepLinkRenderAll = signal(false);
/** True once the transcript is scrolled even slightly from the very top (2px
 *  subpixel slack). Drives the mobile thread-title fade overlay so it eases in
 *  the moment content slides under the sticky title — unlike `notAtTop`, whose
 *  80px chevron threshold left the fade absent until a clear scroll. */
export const scrolledFromTop = signal(false);

/** The currently-active scroll container element.
 *
 *  Set by useAutoScroll when it attaches listeners to a new element.
 *  Used by scrollToBottom() instead of document.querySelector('.thread-content')
 *  which is fragile — on mobile both desktop (hidden) and mobile scroll containers
 *  exist in the DOM, and querySelector finds the hidden one first.
 *
 *  Design decision: this is a plain mutable variable, not a signal, because
 *  nothing needs to react to it changing — it's only read imperatively by
 *  scrollToBottom(). */
let _activeScrollElement: HTMLElement | null = null;

export function setActiveScrollElement(el: HTMLElement | null) {
  _activeScrollElement = el;
}

/** True if the element is actually visible — not hidden via display:none
 *  AND not clipped by a zero-height ancestor with overflow:hidden (e.g.
 *  mobile .content-row which collapses to height:0 instead of display:none
 *  so that position:fixed children like ThreadDrawer still render). */
export function isElementVisible(el: HTMLElement): boolean {
  const r = el.getBoundingClientRect();
  if (r.width <= 0 || r.height <= 0) return false;
  // Walk up to detect clipping ancestors — an element inside a zero-height
  // overflow:hidden container reports non-zero dimensions from layout but
  // is visually invisible.
  let ancestor = el.parentElement;
  while (ancestor && ancestor !== document.documentElement) {
    // display:contents removes the element's box — getBoundingClientRect()
    // returns 0×0 but children are fully visible. Skip these ancestors.
    if (getComputedStyle(ancestor).display === 'contents') {
      ancestor = ancestor.parentElement;
      continue;
    }
    const ar = ancestor.getBoundingClientRect();
    if (ar.height <= 0 || ar.width <= 0) return false;
    ancestor = ancestor.parentElement;
  }
  return true;
}

/** Fallback for when _activeScrollElement hasn't been set yet.
 *  Uses visibility check to skip hidden duplicates on mobile. */
function findVisibleThreadContent(): HTMLElement | null {
  if (typeof document === 'undefined' || !document.querySelectorAll) return null;
  const elements = document.querySelectorAll('.thread-content');
  for (const el of elements) {
    if (isElementVisible(el as HTMLElement)) return el as HTMLElement;
  }
  return null;
}

export function getActiveScrollElement(): HTMLElement | null {
  return _activeScrollElement;
}

/** Suppression mode for ResizeObserver during scroll-to-bottom.
 *
 *  'scroll' — actively scroll to bottom on each resize (content still rendering)
 *  'ignore' — do nothing (suppression expired, normal mode)
 *
 *  Race condition without this: scrollToBottom() scrolls to current bottom,
 *  then new content renders (pending user message), scrollHeight grows,
 *  ResizeObserver fires and sees isAtBottom()===false → sets scrolledUp=true
 *  → auto-scroll effect skips → user never sees the bottom.
 *
 *  Uses time-based window (SUPPRESSION_MS) instead of rAF counting because
 *  mobile devices render content over many more frames than desktop. */
let _resizeMode: 'scroll' | 'ignore' = 'ignore';
let _suppressTimer: ReturnType<typeof setTimeout> | null = null;
const SUPPRESSION_MS = 500;
const PIN_FRAME_MS = 16;

/** Frames the bottom-pinning loop may still run. Refilled by every
 *  `extendSuppression`, decremented per frame, and a hard stop at 0 even while
 *  `_resizeMode` is still 'scroll'.
 *
 *  It is a BACKSTOP, not the normal exit: the loop's ~16ms frames can only land
 *  slower than the wall clock (never faster), so a live suppression window
 *  always flips `_resizeMode` to 'ignore' first, exactly as before. The budget
 *  only matters when the suppression timer never fires at all, which makes an
 *  unbounded self-rescheduling loop unrepresentable rather than merely unlikely.
 *  That happens whenever the two timers end up on different clocks: a page
 *  suspend/resume dropping one, or a test that swaps in fake timers between the
 *  loop's frames (a flake in the frontend suite, where an already-running real
 *  loop rescheduled itself onto the fake clock and then spun forever inside
 *  `runAllTimersAsync`, since its real suppression timer could not fire there). */
let _pinFramesLeft = 0;

/** Get current resize mode — 'scroll' means ResizeObserver should scroll
 *  to bottom instead of setting scrolledUp. */
export function getResizeMode() {
  return _resizeMode;
}

/** Extend the suppression window — called from ResizeObserver when in 'scroll'
 *  mode to keep the window alive while content is still rendering. Refills the
 *  pin loop's frame budget with the same window, so extending the suppression
 *  extends both halves together. */
export function extendSuppression() {
  _pinFramesLeft = Math.ceil(SUPPRESSION_MS / PIN_FRAME_MS);
  if (_suppressTimer) clearTimeout(_suppressTimer);
  _suppressTimer = setTimeout(() => {
    _resizeMode = 'ignore';
    _suppressTimer = null;
  }, SUPPRESSION_MS);
}

/** Resolve the visible scroll container — re-checks on each call so
 *  layout switches (desktop ↔ mobile) mid-animation don't scroll a stale element. */
function resolveTarget(): HTMLElement | null {
  let el = _activeScrollElement;
  if (el && !isElementVisible(el)) el = null;
  return el ?? findVisibleThreadContent();
}

/** Active scroll loop timer — cleared when a new scrollToBottom() call
 *  starts so only the latest invocation drives the loop. Uses setTimeout
 *  (~16ms) because iOS Safari can silently no-op scrollTo(options) during
 *  viewport transitions — direct scrollTop assignment is more reliable. */
let _scrollTimer: ReturnType<typeof setTimeout> | null = null;

/** The ONE programmatic-scroll animation, driven by requestAnimationFrame (NOT
 *  setTimeout): rAF is vsync-aligned so the motion stays smooth and "alive",
 *  whereas a setTimeout(16) loop races the refresh cycle and stutters — on iOS
 *  that read as the scroll "dragging / not wanting to move". We assign scrollTop
 *  DIRECTLY (not `scrollTo({behavior:'smooth'})`/`scrollIntoView`, which iOS
 *  silently no-ops during viewport transitions, can't be customized, and can't
 *  reach the post-render-all top).
 *
 *  Shared by EVERY navigation that moves the transcript — the up/down chevrons,
 *  ⌘↑/⌘↓ turn-nav, AND the notification / Changes-panel "navigation to element"
 *  deep-link (which used to use the browser's native `scrollIntoView` smooth
 *  scroll — a separate, un-tunable, slower motion). Routing them all through this
 *  one tween means every jump feels identical, and the deep-link inherits the
 *  same iOS-reliable direct-scrollTop write + moving-target tracking.
 *
 *  The curve is a TIME-BASED easeOutCubic tween over a distance-scaled, clamped
 *  duration — NOT distance-based exponential smoothing. Exponential smoothing
 *  (`current += remaining * fraction`) has an UNBOUNDED asymptotic tail: as it
 *  nears the target every frame moves a tinier amount, the sub-pixel tail rounds
 *  to alternating 0px/1px steps (the "jank on the slowdown settle"), and a hard
 *  SNAP_PX cutoff was needed to end it — leaving a small visible jump. A
 *  fixed-duration eased tween instead:
 *   - Has a single continuous, controlled deceleration that the easing curve owns
 *     end-to-end, and lands EXACTLY on the target at t=1 (no snap, no jump).
 *   - Keeps the responsive feel: easeOutCubic front-loads velocity, so the first
 *     frame still takes a big step and the scroll reacts instantly to the tap.
 *   - Has a SETTLE that looks identical at any distance (the easing SHAPE is the
 *     same whether it scrolled 400px or 20000px — only the speed scales), and is
 *     frame-rate independent because progress is measured in elapsed ms, not
 *     per-frame fractions, so it feels identical at 60 and 120 Hz.
 *   - Is inherently bounded (t reaches 1 within `duration`), so a moving target
 *     (a streaming thread's growing bottom, re-read each frame) can't loop. */
// Pace knobs. Tuned to the smooth "navigation to element" feel the deep-link had
// with native scrollIntoView, but a little quicker at the top end so the old
// deep-link's "a bit slow" landing sits between it and the snappier chevron.
// To make every navigation faster, lower SCROLL_MAX_MS and/or SCROLL_PX_PER_MS;
// slower, raise them.
const SCROLL_MIN_MS = 240;         // floor so a short scroll still reads as a deliberate glide, not a snap
const SCROLL_MAX_MS = 760;         // ceiling: keeps a very long scroll brisk (and bounds the tween), a touch gentler than the old snappy 680
const SCROLL_PX_PER_MS = 6.5;      // distance→duration rate between the floor and ceiling (lower = a more gradual, navigation-like glide)
const SCROLL_FRAME_MS = 1000 / 60; // nominal-frame head start so the first painted frame already steps (responsive, not a dead frame)
// easeOutCubic: strong initial velocity, smooth controlled deceleration to a clean stop.
const easeOutCubic = (t: number) => 1 - Math.pow(1 - t, 3);

/** Active chevron-scroll rAF id (animateScroll + the reduced-motion re-assert).
 *  Mutually exclusive with _scrollTimer (scrollToBottom's pin loop) — each
 *  direction cancels the other so a down-tap right after an up-tap wins cleanly. */
let _scrollAnimRaf: number | null = null;
function cancelScrollAnim() {
  if (_scrollAnimRaf !== null) { cancelAnimationFrame(_scrollAnimRaf); _scrollAnimRaf = null; }
}

/** rAF easeOutCubic scroll of the active container toward a target, shared by
 *  every transcript navigation — the up/down chevrons, ⌘↑/⌘↓ turn-nav, and the
 *  deep-link "scroll to element" (via smoothScrollToElement below).
 *
 *  - `targetOf(el)` is re-read EVERY frame, so a moving target (the bottom of a
 *    streaming thread) is followed rather than chasing a stale position. The eased
 *    fraction is applied between the captured `start` and the LIVE target each
 *    frame (`start + (target - start) * eased`), so a target that grows mid-tween
 *    is tracked yet the curve still lands cleanly at t=1.
 *  - `start` is captured on the FIRST frame from the live scrollTop (after the
 *    render-all scroll-anchoring shift has settled); thereafter the position is a
 *    pure function of elapsed time, so we never READ scrollTop again (Safari lags
 *    scrollTop reads mid-animation, which janks), only write it. There is no
 *    yield-guard, so an explicit chevron tap ALWAYS reaches its target.
 *  - `duration` scales with the initial distance (clamped to [MIN, MAX]_MS), so a
 *    short hop and a long haul share the same deceleration SHAPE, just at different
 *    speeds. The tween ends precisely at the target on the t≥1 frame — no SNAP_PX
 *    cutoff and no end-jump — then `onDone` runs (the down-chevron hands off to
 *    scrollToBottom() for live tailing).
 *  - scrollTop is written FRACTIONAL (no Math.round): on a 2x/3x display the
 *    sub-pixel position is what makes the slow final approach read as smooth
 *    instead of stepping integer CSS pixels. */
function animateScroll(targetOf: (el: HTMLElement) => number, onDone?: () => void) {
  cancelScrollAnim();
  let started = false;
  let start = 0;
  let startTime = 0;
  let duration = SCROLL_MIN_MS;
  const step = (now: number) => {
    const cur = resolveTarget();
    if (!cur) { _scrollAnimRaf = null; return; }
    if (!started) {
      started = true;
      start = cur.scrollTop;
      // Back-date startTime by one nominal frame so the FIRST painted frame already
      // has a frame's worth of eased progress — reacts instantly to the tap instead
      // of spending a dead frame at t=0. It's a constant time offset, not a per-frame
      // fraction, so the deceleration shape stays elapsed-time (frame-rate) based.
      startTime = now - SCROLL_FRAME_MS;
      const distance = Math.abs(targetOf(cur) - start);
      duration = Math.min(SCROLL_MAX_MS, Math.max(SCROLL_MIN_MS, distance / SCROLL_PX_PER_MS));
    }
    const target = targetOf(cur);
    const t = Math.min(1, (now - startTime) / duration);
    if (t >= 1) {
      cur.scrollTop = target;
      _scrollAnimRaf = null;
      onDone?.();
      return;
    }
    cur.scrollTop = start + (target - start) * easeOutCubic(t);
    _scrollAnimRaf = requestAnimationFrame(step);
  };
  _scrollAnimRaf = requestAnimationFrame(step);
}

/** Scroll the active container so `el`'s top lands at the container top, minus the
 *  element's CSS `scroll-margin-top` — the "navigation to element" motion for a
 *  notification / Changes deep-link.
 *
 *  Replaces the browser's native `el.scrollIntoView({ block: 'start', behavior:
 *  'smooth' })` with the shared animateScroll engine (see its doc), so the
 *  deep-link and the chevrons scroll identically. `scroll-margin-top` was already
 *  the deep-link's header/fade clearance under scrollIntoView (defined per
 *  breakpoint in chat/response.css); we read the resolved px and subtract it so
 *  the element lands in exactly the same place, just via our own tween.
 *
 *  The target is recomputed each frame (animateScroll re-reads `targetOf`), so an
 *  element still growing as markdown/images render — or the whole transcript
 *  re-anchoring after a render-all — is tracked, where native scrollIntoView fixed
 *  its target at call time. Reduced-motion jumps instantly (native smooth ignored
 *  the preference; the tween honours it, matching scrollToTop). */
function smoothScrollToElement(el: HTMLElement): void {
  const marginTop =
    typeof getComputedStyle === 'function'
      ? (parseFloat(getComputedStyle(el).scrollMarginTop) || 0)
      : 0;
  const targetOf = (c: HTMLElement) =>
    typeof c.getBoundingClientRect === 'function' && typeof el.getBoundingClientRect === 'function'
      ? el.getBoundingClientRect().top - c.getBoundingClientRect().top + c.scrollTop - marginTop
      : 0;
  if (prefersReducedMotion()) {
    cancelScrollAnim();
    const c = resolveTarget();
    if (c) c.scrollTop = Math.max(0, targetOf(c));
    return;
  }
  animateScroll(targetOf);
}

/** Smoothly scroll the active chat container to the VERY top — the up-chevron's
 *  action. Loads three concerns:
 *
 *  1. **The render-all grow must not pin to the bottom.** The chevron renders the
 *     full (windowed) thread before scrolling, firing the .thread-content
 *     ResizeObserver on a huge content grow. If a recent scrollToBottom() (thread
 *     open, a sent message) left _resizeMode==='scroll', onResize's scroll-mode
 *     branch runs `scrollTop = scrollHeight` and slams us to the BOTTOM — the
 *     intermittent "scroll-to-top lands mid/bottom" flake. Forcing _resizeMode to
 *     'ignore' (and killing the bottom loop + suppression timer) neutralizes it.
 *  2. **Real motion, reliable on iOS.** animateScroll writes scrollTop directly
 *     per rAF frame instead of native smooth scroll, which iOS drops. Reduced-motion
 *     users get an instant jump (with one re-assert to defeat an iOS no-op / late
 *     RO settle) — no animation.
 *  3. **Auto-scroll fought back.** At the top we are definitively scrolled up, so
 *     set scrolledUp/awayFromBottom accordingly (also keeps the down-chevron on).
 *
 *  A manual top jump also supersedes any in-flight notification deep-link claim —
 *  its suppression would otherwise keep deferring our writes. */
export function scrollToTop() {
  clearPendingEventScroll();
  if (_scrollTimer !== null) { clearTimeout(_scrollTimer); _scrollTimer = null; }
  _resizeMode = 'ignore';
  if (_suppressTimer) { clearTimeout(_suppressTimer); _suppressTimer = null; }
  scrolledUp.value = true;
  awayFromBottom.value = true;

  const el = resolveTarget();
  if (!el) return;

  // Reduced motion: jump instantly, with one rAF re-assert to defeat an iOS no-op
  // / a late render-all ResizeObserver settle.
  if (prefersReducedMotion()) {
    cancelScrollAnim();
    el.scrollTop = 0;
    _scrollAnimRaf = requestAnimationFrame(() => {
      const t = resolveTarget();
      if (t) t.scrollTop = 0;
      _scrollAnimRaf = null;
    });
    return;
  }

  animateScroll(() => 0);
}

/** Smoothly scroll the active chat container to the bottom — the down-chevron's
 *  action. Eases to the (live, re-read each frame) bottom, then hands off to
 *  scrollToBottom() so live streaming keeps the user pinned ("tailing"). Mirrors
 *  scrollToTop's iOS-reliable direct-scrollTop animation. Reduced motion skips
 *  straight to scrollToBottom()'s instant snap + pin. _resizeMode='ignore' during
 *  the ease so onResize's scroll-mode branch can't instant-pin and skip it. */
export function scrollToBottomAnimated() {
  clearPendingEventScroll();
  if (_scrollTimer !== null) { clearTimeout(_scrollTimer); _scrollTimer = null; }
  const el = resolveTarget();
  if (!el || prefersReducedMotion()) { scrollToBottom(); return; }
  _resizeMode = 'ignore';
  if (_suppressTimer) { clearTimeout(_suppressTimer); _suppressTimer = null; }
  // Target the MAX scroll position (scrollHeight − clientHeight), not scrollHeight,
  // so the ease lands exactly at the bottom instead of clamping flat for the last
  // clientHeight px. scrollToBottom() in onDone then pins to the live bottom.
  animateScroll((c) => c.scrollHeight - c.clientHeight, () => scrollToBottom());
}

/** Reset scrolledUp, immediately scroll the response area to the bottom,
 *  and keep scrolling at frame rate until the suppression window expires.
 *
 *  Called from PromptInput.submit() and sendMessage() — any place where
 *  we KNOW the user wants to be at the bottom and new content is about
 *  to render.
 *
 *  iOS Safari PWA keyboard animations take 300-400ms with many
 *  visualViewport.resize events. The old 2×rAF approach missed most of
 *  the animation. Now we scroll every ~16ms for the full 500ms
 *  suppression window, re-reading scrollHeight each time so layout
 *  changes (keyboard close, content render) are always caught. */
export function scrollToBottom(opts?: { auto?: boolean }) {
  // An EXPLICIT go-to-bottom (the default — the chevron, sending a message,
  // answering a deep-linked question) supersedes any in-flight notification
  // deep-link claim: e.g. answering a deep-linked question (addPendingMessage →
  // scrollToBottom) within the ~4s claim window should let the streamed response
  // tail again. Safe because the deep-link's OWN landing never routes through an
  // explicit call: focusThread skips scrollToBottom when targetEventId is set,
  // and scrollToEventAndPulse never calls it.
  //
  // An AUTOMATIC go-to-bottom (`auto: true` — the lazy-load `eventsLoaded` snap
  // that fires when an UNfocused thread's events finally render) must NOT
  // supersede the claim: it fires during the very load the deep-link is waiting
  // on. Clearing the claim here would un-guard every other auto-scroll path and
  // land the user at the bottom instead of the deep-linked event — the
  // deterministic "cross-thread deep-link doesn't scroll to the event" bug. Defer
  // to the deep-link entirely; its own resolve owns the scroll target.
  if (opts?.auto && hasPendingEventScroll()) return;
  if (!opts?.auto) clearPendingEventScroll();
  // Cancel any in-flight chevron-scroll animation so a down-scroll started right
  // after an up-tap isn't dragged back to the top. (The down-chevron's own
  // animateScroll hands off here via onDone — by then the rAF is already null,
  // so this is a no-op for that path.)
  cancelScrollAnim();
  scrolledUp.value = false;
  awayFromBottom.value = false;
  _resizeMode = 'scroll';

  // Immediate scroll
  const target = resolveTarget();
  if (target) {
    target.scrollTop = target.scrollHeight;
  }

  // Cancel any prior loop so only the latest call drives scrolling
  if (_scrollTimer !== null) clearTimeout(_scrollTimer);

  // Continuous scroll loop, running every ~PIN_FRAME_MS until the suppression
  // window expires (or its frame budget runs out, whichever comes first: see
  // `_pinFramesLeft`, the backstop for a suppression timer that never fires).
  // Re-resolves target each frame in case the visible element changed.
  const loop = () => {
    if (_resizeMode !== 'scroll' || _pinFramesLeft <= 0) {
      // The loop clears awayFromBottom every iteration, so a final-frame
      // content grow without an onScroll/onResize would leave the chevron
      // stuck hidden. Reconcile against actual position on exit.
      const el = resolveTarget();
      if (el && el.scrollTop + el.clientHeight < el.scrollHeight - 2) {
        awayFromBottom.value = true;
      }
      // Land in the state the suppression timer would have left behind. When
      // the frame budget is what ended the loop, that timer is gone (dropped by
      // a suspend, or on a clock that will never run it), so nothing else would
      // ever flip the mode back: onScroll would stay suppressed and onResize
      // would keep force-pinning the user to the bottom. In the ordinary exit
      // the timer already did exactly this, so both lines are no-ops there.
      _resizeMode = 'ignore';
      if (_suppressTimer) clearTimeout(_suppressTimer);
      _suppressTimer = null;
      _scrollTimer = null;
      return;
    }
    _pinFramesLeft--;
    scrolledUp.value = false;
    awayFromBottom.value = false;
    const el = resolveTarget();
    if (el) {
      el.scrollTop = el.scrollHeight;
    }
    _scrollTimer = setTimeout(loop, PIN_FRAME_MS);
  };
  _scrollTimer = setTimeout(loop, PIN_FRAME_MS);

  extendSuppression();
}

/** Re-pin the user to the bottom across a layout shift the user just caused —
 *  typing in the prompt (action buttons grow) or toggling a multi-select
 *  question option (card grows a row). If they were at the bottom, engage
 *  scroll mode so the upcoming ResizeObserver fire is suppressed; if they had
 *  scrolled up, no-op so we don't override their intent.
 *
 *  Note: ANSWERING a question or resolving a permission card instead force-pins
 *  via scrollToBottom() (see answerThreadQuestion / resolveCodingAgentPermission)
 *  — those resume the agent's stream, so we re-activate auto-scroll even when the
 *  user had scrolled up.
 *
 *  Must NOT call scrollToBottom(): handleInput fires this on every keystroke,
 *  and scrollToBottom's 16ms re-pinning loop — sized for async streaming —
 *  then runs continuously at 60fps during typing, doing forced-layout walks
 *  (isElementVisible ancestor traversal + scrollTop write fires scroll
 *  handlers doing more layout reads). Surfaced as severe iOS PWA keystroke
 *  lag in workspaces with many threads.
 *
 *  The preserve-case shift is synchronous (textarea/card resizes this frame),
 *  and useAutoScroll observes .thread-content and each child — the natural
 *  ResizeObserver fire after the shift sees mode='scroll' and pins via
 *  onResize, no active loop required. */
export function preserveAtBottom() {
  if (scrolledUp.value) return;
  awayFromBottom.value = false;
  _resizeMode = 'scroll';
  extendSuppression();
}

/** Loop-free force-pin: mirrors scrollToBottom's pin contract (resets
 *  scrolledUp even when the user had scrolled up) but skips the 16ms
 *  re-pinning loop. Use when the caller fires often enough that the loop
 *  would compound into a forced-layout cascade (visualViewport.resize during
 *  typing) — settling is left to the subsequent .thread-content
 *  ResizeObserver fire, which catches mode='scroll' in onResize. */
export function pinToBottomNow() {
  // Defer to an in-flight notification deep-link. This loop-free pin fires from
  // the iOS visualViewport reflow / header focusout / tab-return paths — none of
  // which route through the hasPendingEventScroll() gate the other auto-scroll
  // callers (onResize, useAutoScroll, useScrollMemory, scrollToBottom) honour.
  // When the PWA foregrounds on a notification tap, that viewport reflow pin
  // would slam the freshly-landed event to the bottom a beat after the deep-link
  // scrolled to it — the deterministic "cross-thread deep-link lands at the
  // bottom on iOS" bug. The claim is held across the smooth scroll (sync resolve)
  // and the async load + deadline, so this stays deferred for the whole window.
  if (hasPendingEventScroll()) return;
  scrolledUp.value = false;
  awayFromBottom.value = false;
  _resizeMode = 'scroll';
  const target = resolveTarget();
  // Skip the write when the user is already pinned (steady state during
  // typing): the assignment would re-fire scroll handlers doing forced
  // layout reads for nothing. 2px slack mirrors isVisuallyAtBottom.
  if (target && target.scrollTop < target.scrollHeight - target.clientHeight - 2) {
    target.scrollTop = target.scrollHeight;
  }
  extendSuppression();
}

/** Scroll the chat exchange matching a CSS `selector` into view and briefly
 *  pulse-highlight it. Two public wrappers feed this: `scrollToEventAndPulse`
 *  targets by `data-event-id` (notification deep-links → a specific event) and
 *  `scrollToChangeAndPulse` targets by `data-change-id` (the Changes panel → the
 *  turn that produced a change, which isn't necessarily the thread's last turn).
 *
 *  The target may not be in the DOM yet (the thread is lazy-loading), so a
 *  MutationObserver retries until either the element appears or the deadline
 *  fires. Both layout copies (desktop + mobile) carry the same attributes, so
 *  we filter to the visible one before scrolling and pulsing — otherwise the
 *  pulse runs invisibly on the hidden copy. */
const EVENT_RESOLVE_DEADLINE_MS = 4000;
/** How long to keep the deep-link claim alive after a SYNCHRONOUS resolve, as a
 *  fallback for browsers where `scrollend` is unsupported/unreliable (older iOS
 *  Safari). smoothScrollToElement's tween settles within SCROLL_MAX_MS; this
 *  generously covers it so competing scrolls keep deferring across the whole
 *  glide. Released earlier if `scrollend` fires first. */
const SCROLL_SETTLE_FALLBACK_MS = 1000;

/** The notification deep-link currently resolving a scroll, or null when none is
 *  in flight. Identified by a fresh OBJECT per call, never by what it is
 *  scrolling to: two taps on the SAME notification inside the resolve window
 *  produce the same target, so a target-keyed claim let the FIRST call's
 *  deadline mistake the second call's claim for its own, release it, and run the
 *  give-up recovery over a navigation that was still live. Object identity
 *  cannot collide, so "is the claim still mine" is exact.
 *
 *  Plain mutable variable (not a signal) like `_resizeMode` / `_activeScrollElement`:
 *  it's read imperatively inside ThreadView's events-load effect and
 *  useAutoScroll's layout snap, both of which already re-run on the same
 *  eventsLoaded / eventCount changes — nothing needs to react to it changing.
 *
 *  Why it exists: focusing an UNfocused thread lazily loads its events, and the
 *  scroll-to-bottom that fires on the `eventsLoaded` false→true transition would
 *  otherwise override scrollToEventAndPulse's scroll the instant the
 *  events render — so the deep-link landed at the bottom instead of the event.
 *  (When the thread is already focused the events are already in the DOM,
 *  tryResolve() succeeds synchronously, and no eventsLoaded transition fires —
 *  which is why the bug only showed for unfocused threads.) Mirrors how a saved
 *  scroll position suppresses the same auto-scroll via `hasSavedScroll`. */
let _pendingEventScrollClaim: object | null = null;

/** True while a notification deep-link is waiting for its target event to
 *  render (or to scroll to it). Auto-scroll-to-bottom callers defer to it so
 *  they don't override the deep-link's scroll. */
export function hasPendingEventScroll(): boolean {
  return _pendingEventScrollClaim !== null;
}

/* The deep-link focus marker (the "focus stick") is the shared navigation focus
 *  marker — see components/shared/focusMarker.ts. scrollToSelectorAndPulse applies
 *  it on resolve (below), passing `hasPendingEventScroll` as the settle guard so
 *  this deep-link's own smooth scroll can't self-clear it; clearPendingEventScroll
 *  clears it. */

/** While true, the mobile hide-on-scroll header stays pinned fully visible (see
 *  useHideOnScroll.onScroll). The deep-link scroll lands the element at the
 *  container top minus its `scroll-margin-top`, which adds the fixed app header
 *  and sticky thread-title row back as a STATIC value — only correct if the
 *  header's visible portion is deterministic. Without the pin the smooth scroll would
 *  half-hide the header mid-flight, leaving the landed event partly covered. The
 *  pin is held for a short window covering the smooth scroll, not the full ~4s
 *  deep-link claim, so normal hide-on-scroll resumes the moment the user reads on.
 *  Desktop ignores this — its thread-title header is a sibling above the scroll
 *  container, so the scroll (offset by the desktop scroll-margin-top) already
 *  lands cleanly there. */
let _headerPinnedForScroll = false;
let _headerPinTimer: ReturnType<typeof setTimeout> | null = null;
const HEADER_PIN_MS = 800;

export function isHeaderPinnedForScroll(): boolean {
  return _headerPinnedForScroll;
}

/** Pin the mobile header visible across the next deep-link smooth scroll.
 *  Re-armable: a second deep-link mid-flight extends the window. */
function pinHeaderForScroll(): void {
  _headerPinnedForScroll = true;
  if (_headerPinTimer) clearTimeout(_headerPinTimer);
  _headerPinTimer = setTimeout(() => {
    _headerPinnedForScroll = false;
    _headerPinTimer = null;
  }, HEADER_PIN_MS);
}

/** `selector` resolves the SCROLL target (a `.chat-exchange`, or an addressable
 *  card inside one; both carry the landing scroll-margin-top, see
 *  chat/response.css). `pulseTarget`, when given, narrows the PULSE highlight
 *  to a descendant of that target so a sibling panel in the same exchange isn't
 *  highlighted too — a string is a plain descendant selector (an event deep-link
 *  scopes it to `.initiator-panel`, the event itself, not the agent response
 *  below it), while a function picks the descendant PER MATCHED TARGET (a change
 *  deep-link needs this: the change body sits in `.response-panel` on a proposing
 *  turn but in `.initiator-panel` on a resolution card — see
 *  `scrollToChangeAndPulse`). When the chosen descendant is absent the pulse
 *  falls back to the whole target via `?? target`. */
function scrollToSelectorAndPulse(
  selector: string,
  preferLast = false,
  pulseTarget?: string | ((target: HTMLElement) => HTMLElement | null),
  onUnresolved?: () => void,
): void {
  if (!selector || typeof document === 'undefined' || !document.querySelectorAll) return;

  let resolved = false;
  let deadlineTimer: ReturnType<typeof setTimeout> | null = null;
  let observer: MutationObserver | null = null;
  /** Set once the user takes over during the wait (see `watchUserAction`).
   *  Gates the deadline's fallback scroll only: someone who scrolled away to
   *  read history while the deep-link resolved must not be yanked to the bottom
   *  4s later. Armed on the ASYNC path only, below, so the window is exactly
   *  "since we started waiting". */
  let userTookOver = false;
  let stopWatchingUser: (() => void) | null = null;
  /** This call's claim identity. See `_pendingEventScrollClaim`: an object, so
   *  that re-opening the SAME notification within the resolve window is two
   *  distinguishable claims rather than one indistinguishable selector. */
  const claim = {};

  // Claim the deep-link scroll so the auto-scroll-to-bottom paths defer (see
  // _pendingEventScrollClaim). The claim is held until the deadline below, and
  // NOT released the instant the scroll lands, because the same render that
  // finally shows the event also re-fires ThreadView's events-load effect (on
  // the hasExchanges 0→N flip) and a late ResizeObserver pin, both of which
  // would snap to the bottom a beat after the deep-link scroll. Gating those on the
  // claim (not on scrolledUp) is what lets the events-load effect keep its
  // separate slow-load scrolledUp recovery.
  _pendingEventScrollClaim = claim;
  // Force ThreadView to render the FULL exchange list so a windowed-out target
  // can render for tryResolve/the MutationObserver to find. Stays true until the
  // claim releases; ThreadView keeps the thread fully rendered afterward so the
  // user isn't snapped back to the windowed tail mid-read.
  deepLinkRenderAll.value = true;

  // Release this call's claim, but only if it's still ours. A second deep-link
  // started mid-flight has overwritten the slot with its own claim; its own
  // release handles that one. Without the guard, this call's deadline would
  // clear the newer claim and let an auto-scroll-to-bottom override it (and,
  // since the deadline now recovers rather than giving up silently, snap the
  // user to the bottom and report a failure over a live navigation).
  const releaseClaim = () => {
    if (_pendingEventScrollClaim === claim) {
      _pendingEventScrollClaim = null;
      deepLinkRenderAll.value = false;
    }
  };

  const stopWatching = () => {
    if (observer) {
      observer.disconnect();
      observer = null;
    }
    if (deadlineTimer !== null) {
      clearTimeout(deadlineTimer);
      deadlineTimer = null;
    }
    if (stopWatchingUser) {
      stopWatchingUser();
      stopWatchingUser = null;
    }
  };

  // Hold the claim across the smooth scroll's settle, then release. Used by the
  // SYNCHRONOUS resolve path (the already-focused thread, whose events are
  // already in the DOM): releasing the claim the instant smoothScrollToElement is
  // CALLED is wrong, because the tween lands up to SCROLL_MAX_MS later and a
  // competing scroll in that window — the in-app panel closing, an iOS
  // visualViewport reflow — would override the landing and drop the user at the
  // bottom. Release on the scroll container's `scrollend` (the authoritative
  // "scroll finished" signal, which our per-frame scrollTop writes still fire when
  // they stop) or, where that's unsupported, a fallback timer — whichever fires
  // first. (The async path already holds the claim until its own deadline, which
  // generously covers the same settle.)
  const holdClaimUntilScrollSettles = () => {
    const container = resolveTarget();
    let settleTimer: ReturnType<typeof setTimeout> | null = null;
    const finish = () => {
      if (settleTimer !== null) {
        clearTimeout(settleTimer);
        settleTimer = null;
      }
      container?.removeEventListener?.('scrollend', finish);
      releaseClaim();
    };
    container?.addEventListener?.('scrollend', finish, { once: true });
    settleTimer = setTimeout(finish, SCROLL_SETTLE_FALLBACK_MS);
  };

  const tryResolve = () => {
    if (resolved) return;
    const matches = document.querySelectorAll<HTMLElement>(selector);
    let target: HTMLElement | null = null;
    for (const el of matches) {
      if (isElementVisible(el)) {
        target = el;
        // An event id has one visible copy (dual-mount aside), so first wins.
        // A change id can match BOTH the proposing CC turn (earlier) and its
        // later Applied/Reverted resolution card — preferLast lands on that
        // card, which is the turn the user means when reopening the change.
        if (!preferLast) break;
      }
    }
    if (!target) return;
    resolved = true;
    // Stop watching the DOM, but keep the pending claim alive until the deadline
    // so the post-resolve re-renders stay suppressed (see the claim comment).
    if (observer) {
      observer.disconnect();
      observer = null;
    }

    // The user is now parked on a mid-thread event, not the bottom — pin that
    // so the next render's auto-scroll defers instead of snapping back down.
    // (If the event happens to sit at the bottom, the post-scroll onScroll
    // reconciles scrolledUp against the real position.)
    scrolledUp.value = true;
    // Mobile only: reveal the app header now and keep it pinned visible through
    // the smooth scroll below, so the event lands under the header + sticky
    // title row (matching .chat-exchange's scroll-margin-top) instead of behind
    // a half-hidden header. No-op on desktop. See useHideOnScroll.onScroll.
    pinHeaderForScroll();
    if (typeof document !== 'undefined' && document.dispatchEvent) {
      document.dispatchEvent(new Event('reveal-mobile-header'));
    }
    smoothScrollToElement(target);
    // Highlight only the subject panel, not a sibling panel in the same turn:
    // resolve the requested descendant (string selector, or a per-target picker)
    // and pulse it when present, else the whole target. (`?.` so the jsdom test
    // fakes that lack querySelector fall back cleanly.)
    const pulseChild =
      typeof pulseTarget === 'string'
        ? (target.querySelector?.(pulseTarget) as HTMLElement | null)
        : pulseTarget?.(target) ?? null;
    const pulseEl = pulseTarget ? pulseChild ?? target : target;
    // Apply the shared navigation focus marker: a sticky background highlight that
    // stays until the user takes any action, then fades out. The settle guard defers the
    // dismissal while THIS deep-link's own smooth scroll is still settling
    // (hasPendingEventScroll), so the landing scroll can't self-clear the marker.
    applyNavFocus(pulseEl, { settleGuard: hasPendingEventScroll });
  };

  tryResolve();
  if (resolved) {
    // Synchronous resolve — the thread's events were already in the DOM (it was
    // already focused), so no async load follows. Do NOT release the claim
    // synchronously: the deep-link scroll (smoothScrollToElement) is still
    // settling, and a competing scroll (in-app panel close, iOS visualViewport
    // reflow) would override the landing. Hold the claim until the scroll settles
    // so every auto-scroll path keeps deferring across the smooth scroll.
    holdClaimUntilScrollSettles();
    return;
  }

  // From here on we are WAITING, so start counting the user's own input: a
  // fallback scroll at the deadline must stand down if they took over. Torn
  // down by stopWatching (the deadline always runs it, resolved or not).
  stopWatchingUser = watchUserAction(() => { userTookOver = true; });

  // body, not .thread-content: Preact's positional diff can't preserve the
  // loading-branch .thread-content across ThreadView's loading→loaded swap
  // (one child of .thread-view becomes two), so a scoped observer strands on
  // a detached node. Hot-path filter: re-query only when a mutation actually
  // adds a node containing the target — without it, every streaming token
  // would trigger a doc-wide querySelectorAll + isElementVisible walk.
  observer = new MutationObserver((records) => {
    for (const record of records) {
      for (const node of record.addedNodes) {
        if (node.nodeType !== 1) continue;
        const el = node as Element;
        if (el.matches(selector) || el.querySelector(selector)) {
          tryResolve();
          return;
        }
      }
    }
  });
  observer.observe(document.body, { childList: true, subtree: true });
  // Deadline. Two outcomes reach it, and only one of them is a failure.
  //
  //  - The target DID render and `tryResolve` landed on it (the observer path).
  //    The timer then exists purely to release the claim, which kept the
  //    post-resolve re-renders suppressed. Nothing else to do.
  //  - The target NEVER rendered: it isn't in this thread, it renders nothing at
  //    all, or the thread was still loading when the window closed. That is a
  //    dead deep-link, and it used to end here in silence, leaving the tap
  //    looking broken with no feedback whatsoever. Now it lands the user on the
  //    thread's most recent turn (the transcript's bottom) and tells them the
  //    event could not be found, via the caller's `onUnresolved` (scrollState
  //    stays free of the `store` import; see `parseNavigatedTurn`).
  //
  // The no-late-yank guarantee the silent give-up was protecting is kept, and
  // made exact: the fallback SCROLL is suppressed when the user took over during
  // the wait, so someone who scrolled back to read history is left where they
  // are. The message still fires in that case, since it is then the only signal
  // that the link failed.
  //
  // A deep-link superseded mid-flight (`wasOurs` false) reports nothing: the
  // newer one owns the claim, the viewport and the outcome now.
  deadlineTimer = setTimeout(() => {
    const wasOurs = _pendingEventScrollClaim === claim;
    stopWatching();
    releaseClaim();
    if (resolved || !wasOurs) return;
    if (!userTookOver) scrollToBottom();
    onUnresolved?.();
  }, EVENT_RESOLVE_DEADLINE_MS);
}

/** Shared options for the two deep-link entry points below. */
export interface DeepLinkOptions {
  /** Called when the deep-link's target never renders and the resolve deadline
   *  expires: a dead link the user has to be told about. The MESSAGE is the
   *  caller's, not this module's, because `scrollState` deliberately stays free
   *  of the heavy `store` import (`showToast` lives there) so its lean importers
   *  keep working, the same constraint `parseNavigatedTurn` documents. This
   *  module owns the recovery SCROLL; the caller owns the words. */
  onUnresolved?: () => void;
}

/** Land on the element carrying `data-event-id`: a notification deep-link
 *  scrolling to the exact event that raised it. Two shapes of match, and the
 *  pulse scope differs per match, so the scope is resolved as a function.
 *
 *   - **An exchange-start event** (`UserQuestionAsked`, `CodingAgentPermissionRequest`,
 *     `CredentialRequested`, `McpConsentRequested`, …) stamps the whole
 *     `.chat-exchange` (`ChatExchange`'s root). Narrow the pulse to its
 *     `.initiator-panel` (the event itself), so the agent response rendered
 *     below it in the same turn isn't highlighted too.
 *   - **A step-level event** stamps the specific card that renders it, today the
 *     `ResponseFailed` failure card (`.exchange-error`). That element already IS
 *     the subject, so it must NOT be narrowed: there is no `.initiator-panel`
 *     inside it, and narrowing to a sibling panel would highlight the wrong
 *     thing (or, on a card that happened to contain one, an unrelated descendant).
 *
 *  Discriminating on the match rather than relying on the `?? target` fallback
 *  keeps the intent explicit; the fallback still covers a degenerate exchange
 *  with no `.initiator-panel`.
 *
 *  `onUnresolved` is called when the event never renders inside the resolve
 *  deadline (see the deadline in `scrollToSelectorAndPulse`). */
export function scrollToEventAndPulse(eventId: string, opts?: DeepLinkOptions): void {
  if (!eventId) return;
  scrollToSelectorAndPulse(
    `[data-event-id="${CSS.escape(eventId)}"]`,
    false,
    (target) =>
      (target.matches?.('.chat-exchange')
        ? target.querySelector?.('.initiator-panel')
        : null) as HTMLElement | null,
    opts?.onUnresolved,
  );
}

/** Land on the chat exchange carrying `data-change-id` — the Changes panel
 *  deep-linking a row to its change. `ChatExchange` stamps both the proposing CC
 *  turn (the `ChangeProposed` rides it as a non-rendered step) and any later
 *  Applied/Reverted resolution card with the same id; `preferLast` resolves to
 *  that resolution card when present, and to the proposing turn for a pending
 *  change (its only match) — so the user lands on the change wherever it sits.
 *
 *  The pulse is scoped to the panel that HOLDS the change, so a sibling panel in
 *  the same turn isn't highlighted too — and which panel that is depends on the
 *  card type, so the scope is resolved per matched target:
 *   - proposing CC turn → `.response-panel` (where the `ChangeProposed` step
 *     lives), NOT the user message that started the turn;
 *   - resolution card (ChangeApplied/Discarded/Reverted/Failed) → `.initiator-panel`
 *     (which carries the change body + Diff/Revert actions), NOT any folded-in
 *     post-apply continuation work that renders in a `.response-panel`
 *     (`changePanelHasContinuation` — real thread 76b4ee76). A resolution card is
 *     recognised by its `initiator-panel-change-*` accent class.
 *  When the chosen panel is absent (a degenerate exchange missing it, or a test
 *  DOM fake) the pulse falls back to the whole target via the `?? target` in
 *  scrollToSelectorAndPulse.
 *
 *  Takes the same `onUnresolved` as the event deep-link, and for the same
 *  reason: both run through one `scrollToSelectorAndPulse` deadline, so a change
 *  whose turn never renders gets the identical recovery (land on the newest turn,
 *  say so) with no second implementation to keep in step. */
const CHANGE_RESOLUTION_INITIATOR =
  '.initiator-panel-change-applied,.initiator-panel-change-discarded,.initiator-panel-change-reverted,.initiator-panel-change-failed';
export function scrollToChangeAndPulse(changeId: string, opts?: DeepLinkOptions): void {
  if (!changeId) return;
  scrollToSelectorAndPulse(
    `[data-change-id="${CSS.escape(changeId)}"]`,
    true,
    (target) =>
      (target.querySelector?.(CHANGE_RESOLUTION_INITIATOR)
        ? target.querySelector?.('.initiator-panel')
        : target.querySelector?.('.response-panel')) as HTMLElement | null,
    opts?.onUnresolved,
  );
}

/** Cancel any in-flight notification deep-link scroll claim. A plain thread
 *  focus (no target event) calls this so the prior deep-link's suppression
 *  can't leak onto the newly-focused thread's load. */
export function clearPendingEventScroll(): void {
  _pendingEventScrollClaim = null;
  deepLinkRenderAll.value = false;
  // A plain focus / explicit scroll is deliberate engagement elsewhere — drop any
  // persistent focus marker so it can't leak onto the next thread or linger.
  clearNavFocus();
}

/* ── Turn-by-turn keyboard traversal ─────────────────────────────────────────
 *  The ⌘↑/⌘↓ shortcuts step the transcript one *turn* (a `.chat-exchange`) at a
 *  time — landing the previous/next turn at the top of the scroll container and
 *  marking it with the shared navigation focus marker (the same cue a deep-link
 *  landing uses). Pairs with the focusable `.thread-content` region: a jump also
 *  moves DOM focus into the container so the native Arrow/Page keys keep scrolling
 *  from there. */
const TURN_SELECTOR = '.chat-exchange';
/** Small slack around the landing line so a re-press of the turn shortcut doesn't
 *  re-select the just-landed turn (whose top rests on the landing line) and subpixel
 *  rounding is absorbed. Small and fixed — NOT scaled by the clearance: the gap is
 *  folded into the reference position (scrollTop + gap in stepThreadTurn), so a
 *  larger clearance must not widen the "skip" band, or short adjacent turns become
 *  unreachable by stepping. */
const TURN_NAV_THRESHOLD_SLACK_PX = 4;
/** Fallback clearance when the computed `scroll-margin-top` is unavailable (jsdom /
 *  no layout) — ~0.5rem, the base `--deep-link-focus-gap`. */
const TURN_NAV_FALLBACK_GAP_PX = 8;

/** Room to leave above a turn landed by keyboard turn-nav — the SAME clearance the
 *  deep-link / notification navigation gets for free from `.chat-exchange`'s CSS
 *  `scroll-margin-top` (the top fade band + header stack + focus-highlight gap; see
 *  chat/response.css). Turn-nav animates to an explicit scroll position rather than
 *  via `scrollIntoView`, so `scroll-margin-top` doesn't apply automatically — it
 *  reads the computed value here so both navigation paths share one source of
 *  truth for "make room on top". Falls back to a small gap when the computed style
 *  is unavailable. */
function turnNavClearancePx(turn: HTMLElement): number {
  if (typeof getComputedStyle !== 'function') return TURN_NAV_FALLBACK_GAP_PX;
  const px = parseFloat(getComputedStyle(turn).scrollMarginTop);
  return Number.isFinite(px) && px > 0 ? px : TURN_NAV_FALLBACK_GAP_PX;
}

/** Pure pick of the turn to jump to. `tops` are each turn's top in the container's
 *  scroll coordinate space (ascending, DOM order); `scrollTop` is the current
 *  position. Forward (direction 1) → the first turn below `scrollTop + threshold`;
 *  backward (-1) → the last turn above `scrollTop - threshold`. Returns `null` when
 *  there's nowhere to go (already at the last/first turn). Exported for unit
 *  testing — the DOM wiring around it (below) is thin. */
export function pickTurnIndex(
  tops: number[], scrollTop: number, direction: 1 | -1, threshold: number,
): number | null {
  if (tops.length === 0) return null;
  if (direction === 1) {
    for (let i = 0; i < tops.length; i++) {
      if (tops[i] > scrollTop + threshold) return i;
    }
    return null;
  }
  for (let i = tops.length - 1; i >= 0; i--) {
    if (tops[i] < scrollTop - threshold) return i;
  }
  return null;
}

/** Pick the turn to move to, preferring a marker anchor over scroll position.
 *
 *  When `anchorIdx >= 0` — the nav focus marker sits on a listed turn, which means
 *  the user has NOT scrolled since the last turn-nav (any real scroll gesture fades
 *  the marker) — step by INDEX from it (`anchorIdx + direction`). This is what makes
 *  a cluster of turns sharing a clamped scroll position reachable: after collapsing
 *  the last turn, the collapsed turn and an appended "Change applied" card both sit
 *  in the last (no-scroll-room) viewport, where pure scroll-position stepping keys
 *  off a pinned `scrollTop` and can't distinguish them — so ⌘↓ re-selected the same
 *  turn (or returned null) and the change card was unreachable.
 *
 *  With no anchor (`anchorIdx < 0` — no marker, or it's on a non-turn element) fall
 *  back to scroll-position stepping via `pickTurnIndex`, which handles the
 *  first-press-from-the-current-scroll case and the mid-turn "prev snaps to the
 *  current turn's top" read (both happen precisely when there is no marker). Returns
 *  null at the list end. Pure — exported for unit testing. */
export function pickTurnTarget(
  anchorIdx: number,
  tops: number[],
  scrollTop: number,
  direction: 1 | -1,
  threshold: number,
): number | null {
  if (anchorIdx >= 0) {
    const next = anchorIdx + direction;
    return next >= 0 && next < tops.length ? next : null;
  }
  return pickTurnIndex(tops, scrollTop, direction, threshold);
}

/** Scroll the visible transcript one turn in `direction` (-1 previous, 1 next),
 *  landing it at the top and marking it with the shared navigation focus marker.
 *  A deliberate jump, so — like `scrollToTop` — it supersedes any in-flight
 *  deep-link claim and cancels the bottom-pin loop. The position signals
 *  (scrolledUp / awayFromBottom) are reconciled against the ACTUAL landing target:
 *  a jump that lands mid-thread marks the user parked (so the next render's
 *  auto-scroll doesn't snap to the bottom and the down chevron stays visible); a
 *  jump that lands at the bottom leaves both false (chevron hidden, auto-scroll
 *  enabled) — otherwise the chevron would stick on when the last turn's clamped
 *  target can't move the already-bottomed container. No-op when no transcript is
 *  visible (thread pane collapsed, or the hidden dual-mount copy) or there's no
 *  turn in `direction`. Desktop moves DOM focus into the (focusable) container so
 *  the native scroll keys follow the jump. */
export function stepThreadTurn(direction: 1 | -1): void {
  const el = resolveTarget();
  if (!el) return;

  // Land focus in the transcript FIRST so continuous Arrow/Page scrolling follows
  // — even when there's no turn to jump to in this direction (already at the
  // last/first turn), pressing the shortcut still parks focus on the scroll region
  // to keep reading. Desktop only (mobile navigates panes; a chord has no mobile
  // path). preventScroll so the focus move doesn't fight the animation below.
  if (!isMobile()) el.focus({ preventScroll: true });

  const turns = Array.from(el.querySelectorAll<HTMLElement>(TURN_SELECTOR)).filter(isElementVisible);
  if (turns.length === 0) return;
  // Reuse the deep-link navigation's clearance (.chat-exchange scroll-margin-top);
  // all turns share the same CSS rule, so read it off the first one.
  const gap = turnNavClearancePx(turns[0]);
  const containerTop = el.getBoundingClientRect().top;
  const tops = turns.map((t) => t.getBoundingClientRect().top - containerTop + el.scrollTop);
  // A landed turn's top rests on the landing line at `scrollTop + gap`, so "next"
  // is the first turn whose top is below that line and "prev" the last one above it
  // — i.e. compare tops against `scrollTop + gap`, which is exactly "this turn's
  // landing scroll position is forward/backward of the current one". Fold the gap
  // into the reference here (with only a small slack as the threshold) rather than
  // widening the threshold by it — a large threshold would make the skip band
  // ~2×gap and swallow short adjacent turns when stepping.
  //
  // When the nav focus marker is on one of these turns, step by INDEX from it
  // rather than by scroll position (see `pickTurnTarget`). A marker means the user
  // hasn't scrolled since the last nav (a scroll gesture fades it), so index
  // stepping is unambiguous — and it's what makes a cluster of turns sharing a
  // clamped scroll position each reachable: after collapsing the last turn, the
  // collapsed turn + an appended "Change applied" card sit together in the last
  // (no-scroll-room) viewport, where pure scroll-position stepping keys off a
  // pinned scrollTop and re-selects the same turn, so the change card was
  // unreachable. `closest('.chat-exchange')` also anchors a deep-link marker that
  // landed on an inner `.initiator-panel`; a marker outside the transcript (a
  // settings / plugin landing) isn't in `turns` → -1 → the scroll-based fallback.
  const markedTurn = navFocusElement()?.closest?.(TURN_SELECTOR) as HTMLElement | null;
  const anchorIdx = markedTurn ? turns.indexOf(markedTurn) : -1;
  const idx = pickTurnTarget(anchorIdx, tops, el.scrollTop + gap, direction, TURN_NAV_THRESHOLD_SLACK_PX);
  if (idx === null) return; // at the end in this direction — focus moved, nothing to jump to
  const turn = turns[idx];

  // We ARE jumping now. A deliberate jump, so — like scrollToTop — supersede any
  // in-flight deep-link claim and cancel the bottom-pin loop.
  clearPendingEventScroll();
  if (_scrollTimer !== null) { clearTimeout(_scrollTimer); _scrollTimer = null; }
  _resizeMode = 'ignore';
  if (_suppressTimer) { clearTimeout(_suppressTimer); _suppressTimer = null; }

  // Absolute target scrollTop that puts the turn's top `gap` px below the container
  // top. Re-read each frame (via animateScroll) so a layout shift during streaming
  // is tracked; the term is stable as scrollTop changes because the turn's viewport
  // top moves by the same amount.
  const targetOf = (c: HTMLElement) =>
    typeof c.getBoundingClientRect === 'function'
      ? turn.getBoundingClientRect().top - c.getBoundingClientRect().top + c.scrollTop - gap
      : 0;

  // Reconcile the position signals against the ACTUAL landing target instead of
  // hardcoding "parked mid-thread". The last turn (and any turn near the end) has a
  // landing target at/beyond maxScroll, so the browser clamps the scroll to the
  // bottom — and when we're ALREADY at the bottom the clamped write doesn't move the
  // container, so no scroll event fires and onScroll never reconciles the chevron.
  // Hardcoding awayFromBottom=true there left the down chevron stuck on ("appears
  // the second time you click down arrow"). Landing at the bottom means NOT parked:
  // hide the chevron (awayFromBottom=false) and leave auto-scroll enabled
  // (scrolledUp=false). Anywhere above the bottom, mark the user parked so the next
  // render's auto-scroll doesn't snap down and the chevron stays visible. (2px slack
  // mirrors isVisuallyAtBottom.)
  const maxScroll = Math.max(0, el.scrollHeight - el.clientHeight);
  const landsAtBottom = targetOf(el) >= maxScroll - 2;
  scrolledUp.value = !landsAtBottom;
  awayFromBottom.value = !landsAtBottom;

  // Mark the landed turn with the shared navigation focus marker (a background
  // highlight that sticks until the user's next scroll gesture). clearPendingEventScroll
  // above already cleared any prior marker via clearNavFocus, so this is a clean
  // supersede; no settleGuard — the animateScroll below is programmatic (emits no
  // wheel/touch/keydown), so it can't self-clear, and a real user scroll should.
  applyNavFocus(turn);

  if (prefersReducedMotion()) {
    cancelScrollAnim();
    el.scrollTop = Math.max(0, targetOf(el));
    return;
  }
  animateScroll(targetOf);
}

/** Which collapse store a `.chat-exchange` toggle targets: `response` folds the
 *  response body (`collapsedExchanges`), `initiator` folds the initiator panel
 *  (`collapsedInitiators`) — the fallback for a response-less divider / change turn.
 *  Both stores key on `${threadId}:${userSeq}`. */
export type TurnCollapseKind = 'response' | 'initiator';

/** Pure decode of a navigated `.chat-exchange`'s collapse identity from its data
 *  attributes (`data-thread-id`, `data-user-seq`, `data-collapse-kind`). Returns the
 *  target thread id, exchange sequence, and which panel to toggle — or null when the
 *  attributes are missing / malformed / the turn is not collapsible. Exported and
 *  DOM-free for unit testing, mirroring `pickTurnIndex`. The store-touching
 *  orchestration that consumes this (`toggleNavigatedTurnCollapsed`) lives in
 *  `hooks/useKeyboardShortcuts.ts` — this module stays free of the heavy `store`
 *  import so lean importers (`promptFocus` → `scrollState`) don't drag in
 *  `store`'s module-load side effects (`basePath`'s DOM read). */
export function parseNavigatedTurn(
  threadId: string | null,
  userSeqAttr: string | null,
  kind: string | null,
): { threadId: string; userSeq: number; kind: TurnCollapseKind } | null {
  if (!threadId) return null;
  if (kind !== 'response' && kind !== 'initiator') return null;
  // Reject a missing / blank attribute explicitly: `Number(null)` and `Number('')`
  // both coerce to 0 (a valid integer), which would let an unstamped turn parse.
  if (userSeqAttr === null || userSeqAttr.trim() === '') return null;
  const userSeq = Number(userSeqAttr);
  if (!Number.isInteger(userSeq)) return null;
  return { threadId, userSeq, kind };
}

/** True when the element with the given `data-event-id` is currently in the
 *  visible viewport on this device: the `.chat-exchange` for an exchange-start
 *  event, or the card that renders it for a step-level event (the
 *  `ResponseFailed` failure card). Both are stamped by `ChatExchange`, so the
 *  notification in-app matrix's "already looking at it" check works for either.
 *  Filters dual-mount copies via isElementVisible: the hidden layout's copy
 *  reports a 0×0 rect and would otherwise win the visibility check on the wrong
 *  layout. */
export function isEventInViewport(eventId: string): boolean {
  if (!eventId || typeof document === 'undefined' || !document.querySelectorAll) return false;
  const matches = document.querySelectorAll<HTMLElement>(`[data-event-id="${CSS.escape(eventId)}"]`);
  for (const el of matches) {
    if (isElementOnScreen(el)) return true;
  }
  return false;
}

/** True when `el` is actually on screen, not merely laid out. Two distinct
 *  checks, and both are load-bearing: `isElementVisible` rejects the hidden
 *  layout copy (0x0 rect) and anything inside a collapsed container, while the
 *  rect test rejects an element scrolled or translated out of view. The second
 *  is what a restored scroll position (`useScrollMemory`) needs, since a card
 *  far below the fold is perfectly "visible" by the first test alone.
 *
 *  BOTH axes are tested, and each catches a different layout. Vertically, the
 *  band is the ACTIVE SCROLL ELEMENT's when there is one rather than the
 *  window's, because the transcript is inset by the app header above and the
 *  prompt region below, so an element in either strip is inside
 *  `window.innerHeight` while being completely hidden. Horizontally, the mobile
 *  swipe layout keeps every pane mounted and merely translates the inactive
 *  ones aside, so an element in an off-screen pane has a full-size rect and
 *  passes every vertical test there is.
 *
 *  Shared by `isEventInViewport` (deep-link pulse, presence-pong
 *  `event_in_viewport`) and `choiceCardNav`, which must never take DOM focus on
 *  an off-screen choice: that would arm an Enter the user cannot see, and on a
 *  permission card an unseen grant. */
export function isElementOnScreen(el: HTMLElement): boolean {
  if (!isElementVisible(el)) return false;
  const scroller = getActiveScrollElement();
  const bounds = scroller && isElementVisible(scroller)
    ? scroller.getBoundingClientRect()
    : {
        top: 0,
        bottom: window.innerHeight || document.documentElement.clientHeight,
        left: 0,
        right: window.innerWidth || document.documentElement.clientWidth,
      };
  const r = el.getBoundingClientRect();
  return r.bottom > bounds.top && r.top < bounds.bottom
    && r.right > bounds.left && r.left < bounds.right;
}

/** Suppress useAutoScroll for one render so a user-toggled panel collapse /
 *  expand can settle the scroll state without being overridden.
 *
 *  Bug it fixes: in Working mode the response panel's auto-scroll effect
 *  (useAutoScroll) runs on every streaming chunk. When the user clicks
 *  expand, Preact commits a render that bundles the new collapsed=false
 *  state with the latest streaming chunk's state changes. useEffect fires
 *  before ResizeObserver inside that frame, so the auto-scroll sets
 *  el.scrollTop = el.scrollHeight; by the time onResize runs, the user is
 *  already pinned to the new bottom and the chevron escalation never
 *  triggers. Result: expand silently re-snaps you to the bottom and you
 *  cannot tell the panel even grew.
 *
 *  Setting scrolledUp=true here makes useAutoScroll skip; setting
 *  awayFromBottom=true keeps the chevron visible across the render. After
 *  the layout settles, the regular onScroll listener runs (any clamping
 *  fires a scroll event; collapse-from-bottom always clamps) and
 *  reconciles both signals against the actual position — the both-ways
 *  contract in onScroll restores the at-bottom state when the panel
 *  collapse left the user visually at the new bottom. */
export function preserveOnToggle() {
  scrolledUp.value = true;
  awayFromBottom.value = true;
}

/** Persist auto-scroll intent across tab visibility changes.
 *
 *  Bug it fixes: pinned to the bottom of a streaming response, switch to
 *  another browser tab, come back — scroll position is frozen where it
 *  was and new content piles up below the viewport.
 *
 *  Root cause: while the tab is hidden, browsers throttle layout / rendering
 *  and the `el.scrollTop = el.scrollHeight` write inside useAutoScroll's
 *  deps-effect doesn't realize as an actual scroll. On return, the
 *  ResizeObserver fires for accumulated child growth and onResize sees
 *  scrollTop far below scrollHeight, escalating scrolledUp=true. Future
 *  deps-effect fires then skip auto-scroll, locking the user out of
 *  bottom-pinned mode.
 *
 *  Fix: snapshot wasAtBottom on the first hidden of each hide→visible
 *  cycle, and re-pin via pinToBottomNow() on return if so. Three guards:
 *  - Only capture when a chat scroll element is actually registered
 *    (getActiveScrollElement() !== null). Otherwise — Settings tab, no
 *    thread mounted — scrolledUp's default false would leak a spurious
 *    capture, and the re-pin's extendSuppression() would set
 *    _resizeMode='scroll' globally, overriding the next thread's
 *    useScrollMemory restore if it mounts within the 500ms window.
 *  - First-hide-wins (null sentinel): iOS can double-fire visibilitychange
 *    to hidden during background transitions. If a stray ResizeObserver
 *    fires between the two hidden events and escalates scrolledUp, the
 *    second capture would overwrite the correct one with false.
 *  - pinToBottomNow() over scrollToBottom(): the latter's 500ms 16ms
 *    re-pinning loop would fight an immediate user scroll-up on return.
 *    pinToBottomNow does one write + extendSuppression so a racing RO
 *    fire still pins, but a user gesture is honored from the next frame. */
let _wasAtBottomOnHide: boolean | null = null;
let _visibilityCleanup: (() => void) | null = null;

export function startScrollVisibilityHandler(): () => void {
  stopScrollVisibilityHandler();
  if (typeof document === 'undefined') return stopScrollVisibilityHandler;
  const onChange = () => {
    if (document.visibilityState === 'hidden') {
      // First-hide-wins per cycle (null = not yet captured this cycle).
      if (_wasAtBottomOnHide !== null) return;
      _wasAtBottomOnHide = getActiveScrollElement() !== null && !scrolledUp.value;
    } else if (document.visibilityState === 'visible') {
      const shouldRepin = _wasAtBottomOnHide === true;
      _wasAtBottomOnHide = null;
      if (shouldRepin) pinToBottomNow();
    }
  };
  document.addEventListener('visibilitychange', onChange);
  _visibilityCleanup = () => document.removeEventListener('visibilitychange', onChange);
  return stopScrollVisibilityHandler;
}

export function stopScrollVisibilityHandler(): void {
  _visibilityCleanup?.();
  _visibilityCleanup = null;
  _wasAtBottomOnHide = null;
}

/** Build the scroll- and resize-event handlers for a single .thread-content
 *  element. The visibility gate at the top of each handler is required by the
 *  dual-mounting contract documented at the top of this file. */
export function makeScrollObservers(el: HTMLElement) {
  function isAtBottom() {
    return el.scrollTop + el.clientHeight >= el.scrollHeight - 80;
  }
  // Tighter threshold for the chevron — flips on the first pixel of
  // scroll-up. The 2px slack absorbs subpixel rounding (mobile zoom,
  // device-pixel snapping) without making the chevron look stuck.
  function isVisuallyAtBottom() {
    return el.scrollTop + el.clientHeight >= el.scrollHeight - 2;
  }
  function isAtTop() {
    return el.scrollTop <= 80;
  }
  // Tighter "at the very top" check than isAtTop's 80px chevron window — the
  // title fade should ease in as soon as content slides under the bar. 2px
  // slack absorbs subpixel rounding / iOS overscroll bounce at the top.
  function isVisuallyAtTop() {
    return el.scrollTop <= 2;
  }
  function isScrollable() {
    return el.scrollHeight > el.clientHeight + 10;
  }
  function syncNotAtTop() {
    notAtTop.value = isScrollable() && !isAtTop();
  }
  function syncScrolledFromTop() {
    scrolledFromTop.value = isScrollable() && !isVisuallyAtTop();
  }
  // Scroll events: can both set and clear scrolledUp (user gesture or
  // programmatic scrollTop assignment — both produce real scroll events).
  // Skip during suppression ('scroll' mode) — scroll events in this window
  // are from programmatic scrollToBottom() or header scroll compensation
  // (useHideOnScroll adjusts scrollTop on focusin/focusout), not user intent.
  // Without this guard, iOS keyboard dismiss causes: focusout → scrollTop
  // compensation → scroll event → scrolledUp=true → viewport resize handler
  // skips scrollToBottom() → user loses bottom-pinned state.
  //
  // awayFromBottom is updated unconditionally on scroll — programmatic
  // scrolls leave the container at the bottom, so the check returns false
  // and the chevron hides naturally without a special-case branch.
  // (Content growth that doesn't fire a scroll event is handled by
  // useEffect snapping back to bottom; see onResize for the shrink case.)
  function onScroll() {
    if (!isElementVisible(el)) return;
    syncNotAtTop();
    syncScrolledFromTop();
    awayFromBottom.value = !isVisuallyAtBottom();
    if (getResizeMode() === 'scroll') return; // only scrolledUp is suppressed
    scrolledUp.value = !isAtBottom();
  }
  // Resize events: behavior depends on resize mode set by scrollToBottom().
  //
  // 'scroll' mode: content is rendering after a scrollToBottom() call —
  //   actively scroll to bottom on each resize and extend the suppression
  //   window. This keeps us pinned to the bottom as content progressively
  //   renders (especially important on mobile where rendering is slow).
  //
  // 'ignore' mode (normal): can only *escalate* scrolledUp to true.
  //   Must NEVER clear scrolledUp — otherwise a layout change (textarea shrink
  //   after submit, idle banner removal) can falsely reset scrolledUp and
  //   trigger unwanted auto-scroll.
  function onResize() {
    if (!isElementVisible(el)) return;
    // Sync on resize too — if content shrinks below the viewport,
    // clear the chevron even if no scroll event fires.
    syncNotAtTop();
    syncScrolledFromTop();
    // Suppress the force-pin while a notification deep-link owns the scroll.
    // Without this, the 'scroll'-mode snap — kept alive across the thread load
    // by its own extendSuppression() — slams the freshly-loaded thread to the
    // bottom a beat after scrollToEventAndPulse landed on the event. The claim
    // is held until that call's deadline, so this stays suppressed for the whole
    // settle window; a normal scrollToBottom() flow has no claim, so the pin
    // still works (and skipping extendSuppression here lets the mode decay to
    // 'ignore' on its own once the deep-link has landed).
    if (getResizeMode() === 'scroll' && !hasPendingEventScroll()) {
      el.scrollTop = el.scrollHeight;
      extendSuppression();
      return;
    }
    // Gate on the 80px window (see top of file) so streaming tokens don't
    // trip the chevron before useEffect snaps back. Larger growth (panel
    // expand, multi-line code block) crosses the window and the chevron
    // appears immediately, even though no scroll event fired.
    if (!isAtBottom()) {
      scrolledUp.value = true;
      awayFromBottom.value = true;
    }
    // Clear path: if content shrinks so the user is now visually at the
    // bottom (idle banner removed, step collapsed), hide the chevron
    // without waiting for a scroll event.
    if (awayFromBottom.value && isVisuallyAtBottom()) {
      awayFromBottom.value = false;
    }
  }
  return { onScroll, onResize };
}
