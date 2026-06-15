import { signal } from '@preact/signals';

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

/** Get current resize mode — 'scroll' means ResizeObserver should scroll
 *  to bottom instead of setting scrolledUp. */
export function getResizeMode() {
  return _resizeMode;
}

/** Extend the suppression window — called from ResizeObserver when in 'scroll'
 *  mode to keep the window alive while content is still rendering. */
export function extendSuppression() {
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
export function scrollToBottom() {
  // An explicit go-to-bottom supersedes any in-flight notification deep-link
  // claim — e.g. answering a deep-linked question (addPendingMessage →
  // scrollToBottom) within the ~4s claim window should let the streamed
  // response tail again. Safe because the deep-link's OWN landing never routes
  // through here: focusThread skips scrollToBottom when targetEventId is set,
  // and scrollToEventAndPulse never calls it.
  clearPendingEventScroll();
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

  // Continuous scroll loop — runs every ~16ms until suppression expires.
  // Re-resolves target each frame in case the visible element changed.
  const loop = () => {
    if (_resizeMode !== 'scroll') {
      // The loop clears awayFromBottom every iteration, so a final-frame
      // content grow without an onScroll/onResize would leave the chevron
      // stuck hidden. Reconcile against actual position on exit.
      const el = resolveTarget();
      if (el && el.scrollTop + el.clientHeight < el.scrollHeight - 2) {
        awayFromBottom.value = true;
      }
      _scrollTimer = null;
      return;
    }
    scrolledUp.value = false;
    awayFromBottom.value = false;
    const el = resolveTarget();
    if (el) {
      el.scrollTop = el.scrollHeight;
    }
    _scrollTimer = setTimeout(loop, 16);
  };
  _scrollTimer = setTimeout(loop, 16);

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
const EVENT_PULSE_CLASS = 'event-pulse';
const EVENT_PULSE_MS = 1800;
const EVENT_RESOLVE_DEADLINE_MS = 4000;

/** The event id a notification deep-link is currently resolving a scroll to,
 *  or null when no deep-link scroll is in flight.
 *
 *  Plain mutable variable (not a signal) like `_resizeMode` / `_activeScrollElement`:
 *  it's read imperatively inside ThreadView's events-load effect and
 *  useAutoScroll's layout snap, both of which already re-run on the same
 *  eventsLoaded / eventCount changes — nothing needs to react to it changing.
 *
 *  Why it exists: focusing an UNfocused thread lazily loads its events, and the
 *  scroll-to-bottom that fires on the `eventsLoaded` false→true transition would
 *  otherwise override scrollToEventAndPulse's scrollIntoView the instant the
 *  events render — so the deep-link landed at the bottom instead of the event.
 *  (When the thread is already focused the events are already in the DOM,
 *  tryResolve() succeeds synchronously, and no eventsLoaded transition fires —
 *  which is why the bug only showed for unfocused threads.) Mirrors how a saved
 *  scroll position suppresses the same auto-scroll via `hasSavedScroll`. */
let _pendingEventScrollTarget: string | null = null;

/** True while a notification deep-link is waiting for its target event to
 *  render (or to scroll to it). Auto-scroll-to-bottom callers defer to it so
 *  they don't override the deep-link's scrollIntoView. */
export function hasPendingEventScroll(): boolean {
  return _pendingEventScrollTarget !== null;
}

/** While true, the mobile hide-on-scroll header stays pinned fully visible (see
 *  useHideOnScroll.onScroll). A deep-link `scrollIntoView` ignores the fixed app
 *  header and the sticky thread-title row, so `.chat-exchange`'s scroll-margin-top
 *  adds them back as a STATIC value — which is only correct if the header's
 *  visible portion is deterministic. Without the pin the smooth scroll-down would
 *  half-hide the header mid-flight, leaving the landed event partly covered. The
 *  pin is held for a short window covering the smooth scroll, not the full ~4s
 *  deep-link claim, so normal hide-on-scroll resumes the moment the user reads on.
 *  Desktop ignores this — its thread-title header is a sibling above the scroll
 *  container, so scrollIntoView already lands cleanly there. */
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

/** `selector` resolves the SCROLL target (the `.chat-exchange`, which carries
 *  the scroll-margin-top). `pulseChildSelector`, when given, narrows the PULSE
 *  highlight to a descendant of that target — an event deep-link scopes it to
 *  `.initiator-panel` (the event itself) so the highlight stays on the event and
 *  not the agent response rendered below it in the same exchange. Omitted for a
 *  change deep-link, which highlights the whole turn. */
function scrollToSelectorAndPulse(selector: string, preferLast = false, pulseChildSelector?: string): void {
  if (!selector || typeof document === 'undefined' || !document.querySelectorAll) return;

  let resolved = false;
  let deadlineTimer: ReturnType<typeof setTimeout> | null = null;
  let observer: MutationObserver | null = null;

  // Claim the deep-link scroll so the auto-scroll-to-bottom paths defer (see
  // _pendingEventScrollTarget). The claim is held until the deadline below —
  // NOT released the instant the scroll lands — because the same render that
  // finally shows the event also re-fires ThreadView's events-load effect (on
  // the hasExchanges 0→N flip) and a late ResizeObserver pin, both of which
  // would snap to the bottom a beat after scrollIntoView. Gating those on the
  // claim (not on scrolledUp) is what lets the events-load effect keep its
  // separate slow-load scrolledUp recovery.
  _pendingEventScrollTarget = selector;

  // Release this call's claim — but only if it's still ours. A second deep-link
  // started mid-flight has overwritten the slot with its own target; its own
  // release handles that one. Without the guard, this call's deadline would
  // clear the newer claim and let an auto-scroll-to-bottom override it.
  const releaseClaim = () => {
    if (_pendingEventScrollTarget === selector) _pendingEventScrollTarget = null;
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
    target.scrollIntoView({ block: 'start', behavior: 'smooth' });
    // Highlight only the event, not the response below it: pulse the requested
    // descendant when present, else the whole target. (`?.` so the jsdom test
    // fakes that lack querySelector fall back cleanly.)
    const pulseEl = pulseChildSelector
      ? (target.querySelector?.(pulseChildSelector) as HTMLElement | null) ?? target
      : target;
    pulseEl.classList.add(EVENT_PULSE_CLASS);
    setTimeout(() => pulseEl.classList.remove(EVENT_PULSE_CLASS), EVENT_PULSE_MS);
  };

  tryResolve();
  if (resolved) {
    // Synchronous resolve — the thread's events were already in the DOM (it was
    // already focused), so no async load follows and nothing will try to snap to
    // the bottom. Release the claim immediately; no deadline was scheduled.
    releaseClaim();
    return;
  }

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
  // Deadline: the event resolved a moment ago (load settled) or never rendered
  // (e.g. the source event isn't a rendered chat exchange). Either way, stop
  // watching and release the claim. Bookkeeping only — it deliberately does NOT
  // force a scroll-to-bottom: the auto-scroll paths were suppressed during the
  // wait, so the thread stays where the load (or scrollIntoView) left it. A late
  // snap here would yank a user who scrolled to read history during the window.
  deadlineTimer = setTimeout(() => {
    stopWatching();
    releaseClaim();
  }, EVENT_RESOLVE_DEADLINE_MS);
}

/** Land on the chat exchange carrying `data-event-id` — a notification deep-link
 *  scrolling to the exact event that raised it (e.g. a `UserQuestionAsked`). The
 *  pulse is scoped to the exchange's `.initiator-panel` (the event itself), not
 *  the whole `.chat-exchange`, so the agent response rendered below the event in
 *  the same exchange isn't highlighted too. */
export function scrollToEventAndPulse(eventId: string): void {
  if (!eventId) return;
  scrollToSelectorAndPulse(`[data-event-id="${CSS.escape(eventId)}"]`, false, '.initiator-panel');
}

/** Land on the chat exchange carrying `data-change-id` — the Changes panel
 *  deep-linking a row to its change. `ChatExchange` stamps both the proposing CC
 *  turn (the `ChangeProposed` rides it as a non-rendered step) and any later
 *  Applied/Reverted resolution card with the same id; `preferLast` resolves to
 *  that resolution card when present, and to the proposing turn for a pending
 *  change (its only match) — so the user lands on the change wherever it sits. */
export function scrollToChangeAndPulse(changeId: string): void {
  if (!changeId) return;
  scrollToSelectorAndPulse(`[data-change-id="${CSS.escape(changeId)}"]`, true);
}

/** Cancel any in-flight notification deep-link scroll claim. A plain thread
 *  focus (no target event) calls this so the prior deep-link's suppression
 *  can't leak onto the newly-focused thread's load. */
export function clearPendingEventScroll(): void {
  _pendingEventScrollTarget = null;
}

/** True when the chat exchange with the given `data-event-id` is currently
 *  in the visible viewport on this device. Filters dual-mount copies via
 *  isElementVisible — the hidden layout's copy reports a 0×0 rect and
 *  would otherwise win the visibility check on the wrong layout. */
export function isEventInViewport(eventId: string): boolean {
  if (!eventId || typeof document === 'undefined' || !document.querySelectorAll) return false;
  const matches = document.querySelectorAll<HTMLElement>(`[data-event-id="${CSS.escape(eventId)}"]`);
  const viewportH = window.innerHeight || document.documentElement.clientHeight;
  for (const el of matches) {
    if (!isElementVisible(el)) continue;
    const r = el.getBoundingClientRect();
    if (r.bottom > 0 && r.top < viewportH) return true;
  }
  return false;
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
