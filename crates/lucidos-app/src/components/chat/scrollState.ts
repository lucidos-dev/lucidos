import { signal } from '@preact/signals';
import { prefersReducedMotion } from '../../utils/platform';
import { isMobile } from '../../utils/viewport';
import { applyNavFocus, clearNavFocus, navFocusElement } from '../shared/focusMarker';

/** Shared scroll-position signals for the chat area.
 *  Writers MUST gate on isElementVisible(el) before mutating these signals, and
 *  readers before measuring one: a transcript element laid out at 0x0 answers
 *  every geometric question wrongly (isScrollable=false, so it clears notAtTop),
 *  and the app's own chrome routinely produces one. A COLLAPSED pane is the
 *  everyday case, the desktop split at ratio 0 or mobile's `.content-row`, which
 *  collapses to height 0 rather than display:none so its position:fixed children
 *  still render. See makeScrollObservers below.
 *
 *  This used to say the gate exists because ThreadView and CreateThreadView are
 *  each mounted twice, once per layout. They are not, and have not been since
 *  App.tsx started mounting only the visible layout's pane tree (SplitLayout or
 *  MobileSwipeContainer, gated on viewportIsMobile), because dual-mounting fanned
 *  every signal write out to both subtrees. `.thread-content` exists once. The
 *  phrase "the hidden dual-mount copy" survives further down this file and in a
 *  few neighbours as shorthand for the thing the check actually rejects, an
 *  element with no box; read it that way rather than as a live second mount.
 *
 *  ── The transcript's scroll position belongs to the reader ──────────────────
 *  Nothing in this module moves the transcript on its own. The app may move it
 *  ONLY as the direct result of an explicit user action that asks for it, which
 *  is an exhaustive list: the two chevrons (`scrollToTop` /
 *  `scrollToBottomAnimated`), sending a message (`followSentMessage`),
 *  submitting an answer to a question card (`followAnsweredQuestion`), ⌘↑/⌘↓
 *  turn stepping (`stepThreadTurn`), a notification / Changes deep-link
 *  (`scrollToEventAndPulse` / `scrollToChangeAndPulse`), and `useScrollMemory`
 *  returning a reader to the position they left. Everything else leaves the
 *  reader exactly where they are: a streaming reply, a question or permission
 *  card ARRIVING, a thread sync, a thread opening.
 *
 *  Three of those asks are STANDING rather than one-shot: the down chevron, a
 *  send and an answer all mean "take me to the live edge and KEEP me there until
 *  I say otherwise", so they arm the follow described further down, and only the
 *  reader's own scroll retires it. That is a rule about the DURATION of an ask,
 *  not an exception to the rule above.
 *
 *  This module used to hold the opposite policy, and most of its size was the
 *  machinery for deciding WHEN to pin: an 80px stickiness window (`scrolledUp`),
 *  two ResizeObserver modes behind a 500ms suppression window, a 16ms
 *  re-pinning loop with a frame budget, and a per-caller set of "was the user at
 *  the bottom" reads. All of it is gone. What survives is the reader's own
 *  navigation, plus ANCHOR PRESERVATION (holding them on the same content when
 *  layout shifts around them), which is the opposite of a pin.
 *
 *  `awayFromBottom` carries the whole weight of the live edge now: with no pin,
 *  a reader who has not asked to follow is routinely away from the bottom while
 *  a reply streams, and the down chevron is their only way back. It flips on the
 *  first pixel off the bottom and is reconciled on every scroll AND every
 *  resize. */
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
 *  Set by useScrollObservers when it attaches listeners to a new element.
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

/** Resolve the visible scroll container — re-checks on each call so
 *  layout switches (desktop ↔ mobile) mid-animation don't scroll a stale element. */
function resolveTarget(): HTMLElement | null {
  let el = _activeScrollElement;
  if (el && !isElementVisible(el)) el = null;
  return el ?? findVisibleThreadContent();
}

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

/** Active navigation rAF id (animateScroll + the reduced-motion re-assert). One
 *  at a time: every entry point cancels the one in flight, so a down-tap right
 *  after an up-tap wins cleanly. */
let _scrollAnimRaf: number | null = null;
/** Whether the tween in flight is the standing follow's own (the send's landing
 *  glide) rather than a one-off navigation. Set from the marker `animateScroll`
 *  was handed, so it cannot disagree with who is writing the frames. Read by
 *  `stopFollowingBottom`, which must stop the follow's motion and no one
 *  else's. */
let _followOwnsAnim = false;
function cancelScrollAnim() {
  if (_scrollAnimRaf !== null) { cancelAnimationFrame(_scrollAnimRaf); _scrollAnimRaf = null; }
  _followOwnsAnim = false;
}

/** When one of our own navigations last wrote `scrollTop`, and to WHICH element.
 *  Every such write goes through `markNavigationScroll`, so these cannot fall out
 *  of sync with the writes they describe (the same construction, and the same
 *  reason, as `lastNudgeAt` in utils/iosRepaint.ts).
 *
 *  The element is half the answer, not bookkeeping: `useScrollMemory` positions
 *  three different containers (the transcript, the content pane's body, the
 *  thread drawer's list) and marks all of them here, while two of the three
 *  consumers ask only about the transcript. Without the element a content-pane
 *  restore would claim the transcript's next 64ms of scroll events. */
let _navScrollAt = -Infinity;
let _navScrollEl: HTMLElement | null = null;

/** How long after a navigation's last write its scroll event may still arrive.
 *  A `scrollTop` write does not dispatch its event synchronously: the browser
 *  fires it at the next rendering opportunity. Four 60Hz frames, matching
 *  `NUDGE_EVENT_WINDOW_MS` for the identical problem. */
const NAV_SCROLL_EVENT_WINDOW_MS = 64;

function nowMs(): number {
  return typeof performance !== 'undefined' && typeof performance.now === 'function'
    ? performance.now()
    : Date.now();
}

/** Write `top` to `el` and record it as OURS, so the scroll event it fires a
 *  frame later is not mistaken for the reader's (see `isNavigationScroll`).
 *
 *  Every navigation write in this module goes through it, and so does
 *  `useScrollMemory`'s positioning of the transcript on open: a saved-position
 *  restore and the open-at-the-top reset are the app placing the reader just as
 *  much as a chevron tap is, and both fire a scroll event that the mobile
 *  header would otherwise read as the reader scrolling down (hiding it on a
 *  restore) and the window-expansion would read as the reader asking for older
 *  turns. */
export function markNavigationScroll(el: HTMLElement, top: number) {
  _navScrollAt = nowMs();
  _navScrollEl = el;
  el.scrollTop = top;
}

/** Did one of OUR OWN navigations produce the scroll event being handled?
 *
 *  Those events look exactly like the reader's, and two consumers must tell them
 *  apart: the mobile hide-on-scroll header, which would otherwise slide away
 *  under a chevron tap or half-cover the event a deep-link just landed on, and
 *  the mobile scroll indicator, which must not read our own write as the reader
 *  summoning it.
 *
 *  Two terms, and the window is the load-bearing one. A live tween is the easy
 *  half. The hard half is that a write's scroll event lands a frame or more
 *  LATER: the instant navigations (`scrollToBottom`, and every reduced-motion
 *  path) run no tween at all, and even a tween clears its rAF handle on the
 *  frame it lands, so an "is a tween running" test alone answers false for
 *  exactly the events it exists to catch, and the header hides on a chevron tap.
 *
 *  This replaces the old `getResizeMode() === 'scroll'` read, which covered the
 *  same events only by accident: the bottom-pin's 500ms suppression window
 *  happened to be open across them. The question is now asked directly, and
 *  scoped to the frames it can actually be true for rather than to half a
 *  second. */
export function isNavigationScroll(el?: HTMLElement | null): boolean {
  if (_scrollAnimRaf !== null) return true;
  if (nowMs() - _navScrollAt >= NAV_SCROLL_EVENT_WINDOW_MS) return false;
  // A caller that names its element is asking about ITS scroll events, so a
  // write to some other container is not an answer. A caller that names none
  // (the mobile header, which follows whichever pane is active) takes any.
  return !el || _navScrollEl === el;
}

/* ── The standing request to ride the live edge ──────────────────────────────
 *  Three reader actions mean "take me to the bottom and KEEP me there" rather
 *  than "jump once": pressing the down chevron, sending a message, and
 *  submitting an answer to a question card. All three arm the flag below, and
 *  while it is armed, content growth writes the reader back to the live edge
 *  (the growth branch in `makeScrollObservers`' onResize, which is where the old
 *  force-pin used to live).
 *
 *  The answer is here because it IS a send: the reader produced the content at
 *  the bottom and is owed the reply to it, and which of the three shapes they
 *  used to produce it (typing, which the engine reroutes as a `FreeText` answer
 *  and which therefore already came through `followSentMessage`; tapping an
 *  option; the multi-select Submit) is not something they should be able to feel
 *  in the scroll. What is NOT a request is the question card ARRIVING, which is
 *  the agent's doing and moves nobody.
 *
 *  Nothing else arms it. Not an SSE sync confirming a pending message, not a
 *  change applied / discarded / reverted, not granting a permission, not a
 *  coding-agent action, not a lazy load, not a deep link, not a thread opening.
 *  Being AT the bottom does not arm it either: a position is not a request, and a
 *  reader who merely happens to sit at the live edge has asked for nothing.
 *
 *  The request BELONGS TO A THREAD and outlives leaving it. The flag here is one
 *  global, so `focusThread` retires it on every open (a thread the reader just
 *  opened is not one they asked to follow), and that used to be the end of the
 *  request: coming back landed on the pixel offset the transcript had when they
 *  walked away, with everything the agent produced meanwhile below them and
 *  nothing following. So the request is WRITTEN DOWN per thread, as one of the
 *  two forms a reading position takes (`hooks/useScrollMemory.ts`), and
 *  `resumeFollowingBottom` re-arms it on re-entry. That is the same request
 *  resumed, not a fourth arming point: it can only fire for a thread the reader
 *  armed, and a reader who merely parked at the bottom saves the offset instead
 *  and comes back to it following nothing. `isFollowScroll` and `onFollowArmed`
 *  are the two things the recording side needs; both are below.
 *
 *  The condition for following is this flag and NOTHING ELSE. There is no
 *  proximity term (the retired 80px stickiness window) and no timing term (the
 *  retired 500ms suppression window); both of those tried to INFER the request
 *  that the flag now records.
 *
 *  `_followEl` / `_followTop` are how the follow's own write is told apart from
 *  the reader's gesture, which is the only thing that retires the request. A
 *  follow write is marked as a navigation scroll like every other write the app
 *  makes, but `isNavigationScroll`'s 64ms window cannot answer THIS question: a
 *  streaming thread re-marks itself every frame, so a flick landing inside the
 *  window would read as ours and the reader would fight the follow. The POSITION
 *  answers it exactly instead. Content growing below the reader changes
 *  `scrollHeight` and never `scrollTop`, so growth can never look like a
 *  gesture; every gesture (wheel, scrollbar drag, touch flick, momentum, keys, a
 *  mobile pane swipe) changes `scrollTop`, so one always does.
 *
 *  It is also what retires the follow for a navigation that deliberately puts
 *  the reader somewhere else (the up chevron, turn stepping, a deep link, a
 *  saved-scroll restore), with no call site of its own: none of those is a
 *  follow write, so the first frame of one already reads as leaving. And it is
 *  what keeps the follow ALIVE across everything that is not a scroll: a card
 *  resolving, granting a permission, expanding a turn. Those change content
 *  without moving the reader off the live edge, so `atEdge` alone already
 *  answers them. */
let _followingBottom = false;
let _followEl: HTMLElement | null = null;
let _followTop = -1;

/** Subscribers notified when the follow is ARMED, and never when it is retired.
 *  One consumer today: `attachScrollMemory`, which records the request as this
 *  thread's reading position.
 *
 *  The asymmetry is the design, not an omission. A retirement has two causes that
 *  must be recorded differently, and this side cannot tell them apart: the reader
 *  scrolling away arrives WITH the scroll event that already records the offset
 *  they landed on, while `focusThread` retiring on a thread switch must record
 *  nothing at all, or the thread being LEFT would have its live edge overwritten
 *  by whatever offset the shared container happens to hold. Broadcasting only the
 *  arm leaves the second case with no save path to reach.
 *
 *  A plain callback set rather than a signal, for the same reason `armFollowBottom`
 *  is private: an exported writable `followingBottom` signal would be a fourth
 *  arming point that no source scan could stop, and it would broadcast the
 *  retirement this deliberately does not. */
const _followArmedListeners = new Set<() => void>();

/** Subscribe to the arm. Returns the unsubscribe; fires on the unarmed to armed
 *  transition only, so re-arming an already-armed follow notifies nobody (there
 *  is nothing new to record). */
export function onFollowArmed(listener: () => void): () => void {
  _followArmedListeners.add(listener);
  return () => { _followArmedListeners.delete(listener); };
}

/** A send whose own message has not rendered yet, holding the reader's last
 *  message as it was AT SEND TIME and when the send happened. The optimistic row
 *  arrives a frame or more after the send while the composer collapsing has
 *  already fired a resize, so the landing cannot just run on the next growth: it
 *  waits until a DIFFERENT last user message is present, and moves nobody until
 *  then. Null when no send is waiting. */
let _sendLanding: { before: HTMLElement | null; at: number } | null = null;

/** How long that landing waits for the reader's own message before giving up and
 *  riding the live edge instead.
 *
 *  Generous, because it is only ever reached when the message is not
 *  individually addressable rather than merely late: the second and subsequent
 *  queued follow-ups fold into a CLOSED `<details class="queued-message-group">`
 *  (see `CreateThreadView`), whose contents have no box at all, so the message
 *  the reader just sent has no rect to land on. Giving up on the landing must
 *  NOT give up on the follow, which is the part the reader actually asked for,
 *  and the live edge is where their message is either way. Without the deadline
 *  a pending landing would sit forever and hold the whole follow inert. */
const SEND_LANDING_DEADLINE_MS = 1000;

/** The ONE at-the-live-edge threshold. 2px of slack absorbs subpixel rounding
 *  (mobile zoom, device-pixel snapping) and the iOS overscroll bounce without
 *  making the chevron look stuck. Shared by the chevron's reconcile, the send's
 *  "are they already there" test and both observers, so they cannot drift.
 *
 *  There used to be a second, 80px threshold beside it, the stickiness window
 *  inside which growth counted as the reader riding the live edge and re-pinned
 *  them. Riding the live edge is an explicit request now, so one threshold
 *  answers the one remaining geometric question. */
function isAtLiveEdge(el: HTMLElement): boolean {
  return el.scrollTop + el.clientHeight >= el.scrollHeight - 2;
}

/** Write `top` and record it as the FOLLOW's own, so the scroll event it fires a
 *  frame later cannot be read as the reader leaving. Goes through
 *  `markNavigationScroll` like every other write the app makes, so the mobile
 *  header and the render-window expansion keep standing down for it too. */
function markFollowScroll(el: HTMLElement, top: number) {
  markNavigationScroll(el, top);
  _followEl = el;
  _followTop = el.scrollTop;
}

/** Arm the standing follow at the position the caller's own scroll just reached,
 *  so the trailing scroll event of that scroll cannot retire the request it just
 *  made. The chevron's two entry points call this; `followSentMessage` builds on
 *  it. */
function armFollowBottom() {
  const el = resolveTarget();
  armFollowOn(el);
}

/** The half of `armFollowBottom` that takes its element rather than resolving
 *  one, for the restore, which holds the container it is positioning and must not
 *  ask `resolveTarget` for a different one (a thread opening mid-layout-swap can
 *  answer with the outgoing mount).
 *
 *  Notifies only on the unarmed to armed transition, so the recording side is
 *  told about a request the moment it is made even when arming produces no scroll
 *  event at all. That case is ordinary rather than exotic: a reader already at the
 *  live edge who presses the chevron gets a write the browser clamps to where they
 *  already are, and an idle thread then grows nothing, so no scroll ever carries
 *  the request anywhere. */
function armFollowOn(el: HTMLElement | null) {
  const wasArmed = _followingBottom;
  _followingBottom = true;
  _followEl = el;
  _followTop = el ? el.scrollTop : -1;
  _sendLanding = null;
  if (!wasArmed) for (const listener of _followArmedListeners) listener();
}

/** Resume a standing follow the reader armed in this thread BEFORE they left it:
 *  write the live edge and arm, so the growth branch carries them from there.
 *
 *  Called only by `attachScrollMemory`, on a thread whose recorded reading
 *  position is the live edge. The write is required and not merely tidy:
 *  `.thread-content` is one element reused across threads, so on arrival it holds
 *  the OUTGOING thread's offset, and arming alone would leave the reader there
 *  until the next growth round. It goes through `markFollowScroll` like every
 *  other follow write, so the mobile header stands down for it and the render
 *  window does not read it as the reader asking for older turns.
 *
 *  Nothing waits for content here, unlike the offset restore's observer retries.
 *  An offset can only be honoured once the transcript is tall enough to hold it,
 *  whereas the live edge is wherever the content currently ends: the write lands
 *  on today's bottom and the armed follow rides every later arrival to the real
 *  one. */
export function resumeFollowingBottom(el: HTMLElement): void {
  markFollowScroll(el, Math.max(0, el.scrollHeight - el.clientHeight));
  armFollowOn(el);
}

/** Is the scroll event being handled the FOLLOW's own write rather than the
 *  reader's gesture? Armed, and the container still exactly where the follow put
 *  it (see `isWhereTheFollowLeftIt` for the 1px slack and why position is the
 *  right question).
 *
 *  Exported for the recording side, which must write the live edge for the
 *  follow's own writes and a plain offset for everything else. It asks the
 *  POSITION rather than the flag alone on purpose: `.thread-content` carries two
 *  scroll listeners, the disarm lives in `makeScrollObservers` and the save lives
 *  in `attachScrollMemory`, and a save that merely asked whether the flag was
 *  armed would answer differently depending on which of the two ran first. A
 *  reader's gesture moves `scrollTop` away from the stamp by definition, so this
 *  answers the same in either order. */
export function isFollowScroll(el: HTMLElement): boolean {
  return _followingBottom && isWhereTheFollowLeftIt(el);
}

/** Retire the standing follow. Called by the disarm in `onScroll` (the reader
 *  taking the container away from where the follow put it), and exported for the
 *  one navigation that cannot be read off a scroll: opening a DIFFERENT thread.
 *  A thread the reader just opened is not one they asked to follow, and a
 *  restore that happens to land on that thread's saved bottom position writes no
 *  scroll the disarm could see. See `focusThread`. */
export function stopFollowingBottom() {
  // The send's landing glide is the follow's OWN motion, so retiring the follow
  // retires it. Without this the reader who just scrolled away is dragged back
  // for the rest of the tween (the disarm would say one thing and the next frame
  // do another), and a thread opened mid-glide is scrolled with the previous
  // thread's message as the target. Only the follow's tween: a deep-link or
  // up-chevron glide belongs to a navigation this has no business cancelling.
  if (_followOwnsAnim) cancelScrollAnim();
  _followingBottom = false;
  _followEl = null;
  _sendLanding = null;
}

/** Is the container still exactly where the follow's last write left it? The
 *  exact reading of "the reader has not moved since", per the block above. 1px
 *  of slack absorbs a browser re-rounding a fractional position (zoom, device
 *  pixel ratio) and the iOS repaint nudge's deliberate ±1. */
function isWhereTheFollowLeftIt(el: HTMLElement): boolean {
  return _followEl === el && Math.abs(el.scrollTop - _followTop) <= 1;
}

/** The reader's own newest message: the LAST `.initiator-panel-user`, which is
 *  the panel a `MessageReceived` turn renders (see `chat-exchange-parts`). The
 *  optimistic row a send inserts carries it too, so this resolves the just-sent
 *  message the moment it renders.
 *
 *  Strictly the last one, never the last one that happens to be visible: a
 *  backwards scan for the newest VISIBLE panel would answer with an older
 *  message whenever the newest has no box, and the send's landing would glide to
 *  the wrong turn. The case is real, not hypothetical: a second queued follow-up
 *  folds itself and the first into a closed `<details>` group. So an invisible
 *  newest panel is reported as "not there yet" and the landing waits it out (see
 *  `SEND_LANDING_DEADLINE_MS`). The visibility test also rejects the hidden
 *  dual-mount copy, and is one call rather than a scan. */
function lastUserMessage(el: HTMLElement): HTMLElement | null {
  if (typeof el.querySelectorAll !== 'function') return null;
  const panels = el.querySelectorAll<HTMLElement>('.initiator-panel-user');
  const last = panels[panels.length - 1];
  return last && isElementVisible(last) ? last : null;
}

/** The turn holding the question card `toolUseId`: the `.initiator-panel` around
 *  it, which is the answer's counterpart to `lastUserMessage`'s panel.
 *
 *  The PANEL and not the `.question-body` inside it, for two reasons that agree.
 *  It is the whole of what the reader produced (the question, their picks, the
 *  chrome around both) and is what the reply then grows underneath, exactly as it
 *  does under a sent message. And it is the part that SURVIVES being answered:
 *  `QuestionBody` swaps its live body for `AnsweredBody`, a different component,
 *  so Preact unmounts the body node a frame in, while the panel around it is the
 *  same vnode in the same position and is reused. Anchoring the landing on the
 *  panel is therefore a resolve-once, exactly like the send's, rather than a
 *  per-frame re-query. Never the enclosing `.chat-exchange`, which also contains
 *  the growing reply.
 *
 *  Matched on the attribute rather than through a `[data-tool-use-id="…"]`
 *  selector so no id has to be escaped into CSS syntax. Both the live body and
 *  the answered one carry the id, so which of the two is in the DOM when this
 *  runs does not decide whether the card is found. */
function questionCardTurn(el: HTMLElement, toolUseId: string): HTMLElement | null {
  if (typeof el.querySelectorAll !== 'function') return null;
  for (const body of el.querySelectorAll<HTMLElement>('.question-body')) {
    if (body.getAttribute?.('data-tool-use-id') !== toolUseId) continue;
    if (!isElementVisible(body)) continue;
    return (body.closest?.('.initiator-panel') as HTMLElement | null) ?? body;
  }
  return null;
}

/** The reader sent a message: the clearest "show me the live edge" there is,
 *  since they just produced the content at the bottom and obviously want to see
 *  it and its answer. Arms the same standing follow the chevron arms, so the
 *  reply streaming in keeps the live edge in view until they scroll away.
 *
 *  It is NOT a blind jump to the bottom, and splits on where they already are:
 *
 *   - **At the live edge**: write no scroll at all. They are already there, the
 *     growth branch keeps them there, and a redundant write on a reader who
 *     never moved is exactly the unrequested movement this module exists to
 *     remove (on iOS it would also cancel an in-flight momentum scroll).
 *   - **Scrolled up**: glide to their own just-sent message, landing its BOTTOM
 *     edge on the bottom of the viewport, so they see what they wrote with the
 *     answer growing in underneath it. Anchored on the message ELEMENT, never
 *     computed from `scrollHeight`: the transcript grows between the send and
 *     the landing (the working indicator mounting is the usual case) and a
 *     `scrollHeight` target then lands PAST the message and hides the very thing
 *     they just wrote.
 *
 *  Called by the two send sites, `store/actions/chat.ts`'s `addPendingMessage`
 *  and `PromptInput`'s `submit`. Both fire for one send from the composer; the
 *  second keeps the first's baseline, so a render landing between them cannot
 *  leave the landing waiting for a message that is already there. */
export function followSentMessage(): void {
  const el = resolveTarget();
  const pending = _sendLanding;
  armFollowBottom();
  if (!el || isAtLiveEdge(el)) return;
  _sendLanding = pending ?? { before: lastUserMessage(el), at: nowMs() };
}

/** The reader submitted an answer to the question card `toolUseId`: the same ask
 *  as a send, so it takes the same two halves. It arms the same standing follow,
 *  so the reply resuming underneath keeps the live edge in view until they scroll
 *  away, and it splits on where they already are exactly as `followSentMessage`
 *  does: at the live edge it writes no scroll at all (they can see it, and a
 *  redundant write cancels an iOS momentum scroll), and scrolled up it glides so
 *  their answered card rests on the bottom of the viewport.
 *
 *  Called by the two card-submitted answers: `QuestionCard`'s single-select
 *  option tap and `PromptInput`'s multi-select Submit. The THIRD way to answer,
 *  typing into the composer, is a send that the engine reroutes as a `FreeText`
 *  answer, so it arrives through `followSentMessage` and needs nothing here.
 *
 *  Unlike a send, this one needs no deferral: a send waits for its optimistic row
 *  to render, whereas the card being answered is already on screen (it is what
 *  the reader just tapped), so the glide starts now. When the card cannot be
 *  resolved at all, the follow is still armed and the reader still rides the live
 *  edge, which is the half they actually asked for. */
export function followAnsweredQuestion(toolUseId: string): void {
  const el = resolveTarget();
  armFollowBottom();
  if (!el || isAtLiveEdge(el)) return;
  const panel = questionCardTurn(el, toolUseId);
  if (panel) landOnOwnTurn(el, panel);
}

/** Glide so `panel`'s BOTTOM edge rests on the container's bottom edge, marking
 *  every frame as the follow's own. `panel` is the turn the reader just produced:
 *  their optimistic message row for a send, the answered card's initiator panel
 *  for an answer. The target is re-read per frame by `animateScroll`, so the
 *  working indicator mounting under it during the glide is tracked rather than
 *  overshot. Reduced motion writes it once, as everywhere else in this module. A
 *  target at or behind the current position writes nothing: the turn is already
 *  fully in view, and the follow has no business scrolling the reader backwards
 *  to prove it.
 *
 *  A panel that LEAVES the layout mid-glide (the reader opens another thread, a
 *  second queued follow-up reparents this one into its disclosure group) holds
 *  the tween on the last target it measured while the panel was still there, so
 *  the glide finishes where it was heading. Nothing else cancels a tween on a
 *  thread switch, and a detached node reports an all-zero rect, so without the
 *  guard the subtraction of the container's bottom edge would make the target a
 *  whole viewport NEGATIVE and haul the newly-opened thread to its top. The
 *  floor is the same belt for a rect that is merely surprising. */
function landOnOwnTurn(el: HTMLElement, panel: HTMLElement): void {
  let lastTarget = -1;
  const targetOf = (c: HTMLElement) => {
    if (panel.isConnected === false
      || typeof c.getBoundingClientRect !== 'function'
      || typeof panel.getBoundingClientRect !== 'function') {
      return lastTarget >= 0 ? lastTarget : c.scrollTop;
    }
    lastTarget = Math.max(0, c.scrollTop + (panel.getBoundingClientRect().bottom - c.getBoundingClientRect().bottom));
    return lastTarget;
  };
  if (targetOf(el) <= el.scrollTop) return;
  if (prefersReducedMotion()) {
    cancelScrollAnim();
    markFollowScroll(el, targetOf(el));
    return;
  }
  animateScroll(targetOf, undefined, markFollowScroll);
}

/** Honour the standing follow on one growth round. Either the reader is waiting
 *  for their own just-sent message (glide to it, once) or they are riding the
 *  live edge (write it).
 *
 *  Stands down while a navigation tween owns the scroll, including the landing
 *  glide itself: a tween re-reads its own target every frame, so a live-edge
 *  write beside it would drag the glide past the message it is landing on. */
function followTheLiveEdge(el: HTMLElement): void {
  if (_scrollAnimRaf !== null) return;
  if (_sendLanding) {
    const panel = lastUserMessage(el);
    if (panel && panel !== _sendLanding.before) {
      _sendLanding = null;
      landOnOwnTurn(el, panel);
      return;
    }
    // Still the message that was there when they sent, so their own has not
    // rendered yet: there is nothing to land on, and nowhere to jump meanwhile.
    // Past the deadline it is not late, it is unaddressable, and the follow the
    // reader asked for outranks the landing (see SEND_LANDING_DEADLINE_MS): drop
    // the landing and ride the live edge below.
    if (nowMs() - _sendLanding.at < SEND_LANDING_DEADLINE_MS) return;
    _sendLanding = null;
  }
  // The MAX offset rather than `scrollHeight`, which the browser would clamp to
  // the same place: naming the real target keeps the write meaningful instead of
  // leaning on the clamp, the same reason `scrollToBottomAnimated` targets it.
  markFollowScroll(el, Math.max(0, el.scrollHeight - el.clientHeight));
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
 *    cutoff and no end-jump. Then `onDone` runs: the down-chevron uses it to
 *    reconcile the chevron against where the tween actually landed.
 *  - scrollTop is written FRACTIONAL (no Math.round): on a 2x/3x display the
 *    sub-pixel position is what makes the slow final approach read as smooth
 *    instead of stepping integer CSS pixels.
 *  - `mark` is how each frame's write is recorded. It defaults to
 *    `markNavigationScroll`, which is right for a one-off navigation; the send's
 *    landing passes `markFollowScroll` so its own frames are not read as the
 *    reader leaving the follow it just armed. */
function animateScroll(
  targetOf: (el: HTMLElement) => number,
  onDone?: () => void,
  mark: (el: HTMLElement, top: number) => void = markNavigationScroll,
) {
  cancelScrollAnim();
  _followOwnsAnim = mark === markFollowScroll;
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
      mark(cur, target);
      _scrollAnimRaf = null;
      onDone?.();
      return;
    }
    mark(cur, start + (target - start) * easeOutCubic(t));
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
    if (c) markNavigationScroll(c, Math.max(0, targetOf(c)));
    return;
  }
  animateScroll(targetOf);
}

/** Smoothly scroll the active chat container to the VERY top — the up-chevron's
 *  action. Two concerns:
 *
 *  1. **Real motion, reliable on iOS.** animateScroll writes scrollTop directly
 *     per rAF frame instead of native smooth scroll, which iOS drops. Reduced-motion
 *     users get an instant jump (with one re-assert to defeat an iOS no-op / late
 *     RO settle) — no animation.
 *  2. **The chevron.** At the top we are definitively away from the bottom, so
 *     set the signal here rather than waiting for the first scroll event, which
 *     keeps the down-chevron on from the first frame of the glide.
 *
 *  A manual top jump also supersedes any in-flight notification deep-link claim:
 *  the deep-link owns the viewport until it settles, and this is the user saying
 *  otherwise. */
export function scrollToTop() {
  clearPendingEventScroll();
  awayFromBottom.value = true;

  const el = resolveTarget();
  if (!el) return;

  // Reduced motion: jump instantly, with one rAF re-assert to defeat an iOS no-op
  // / a late render-all ResizeObserver settle.
  if (prefersReducedMotion()) {
    cancelScrollAnim();
    markNavigationScroll(el, 0);
    _scrollAnimRaf = requestAnimationFrame(() => {
      const t = resolveTarget();
      if (t) markNavigationScroll(t, 0);
      _scrollAnimRaf = null;
    });
    return;
  }

  animateScroll(() => 0);
}

/** Smoothly scroll the active chat container to the bottom — the down-chevron's
 *  action, and the ONLY "take me to the live edge" gesture there is.
 *
 *  Eases to the bottom, re-reading the target every frame so a thread that keeps
 *  streaming during the glide is tracked and the tween lands on the TRUE grown
 *  bottom rather than on the bottom as it was when tapped.
 *
 *  On landing it ARMS the standing follow, because "go to the bottom" means "and
 *  keep me there until I say otherwise": content arriving a beat later carries
 *  the reader with it instead of stranding them one tap above the live edge.
 *  Arming on landing rather than on the tap is deliberate: a tween superseded
 *  mid-flight by another navigation never reaches `onDone`, so it never leaves a
 *  follow armed behind the navigation that beat it.
 *
 *  Reduced motion skips straight to scrollToBottom()'s instant jump, which arms
 *  it the same way. */
export function scrollToBottomAnimated() {
  clearPendingEventScroll();
  const el = resolveTarget();
  if (!el || prefersReducedMotion()) { scrollToBottom(); return; }
  // Target the MAX scroll position (scrollHeight − clientHeight), not scrollHeight,
  // so the ease lands exactly at the bottom instead of clamping flat for the last
  // clientHeight px.
  animateScroll(
    (c) => c.scrollHeight - c.clientHeight,
    // The landing write may not move the container at all (the tween's last
    // frame can already be there), and then no scroll event fires to reconcile
    // the chevron. Settle it here against the real position instead, and arm the
    // follow from where the tween actually landed.
    () => { syncAwayFromBottom(); armFollowBottom(); },
  );
}

/** Jump the transcript to the bottom in one write: the reduced-motion form of
 *  the down chevron, and the compose view's chevron (which has no windowed
 *  render to glide through). Arms the standing follow on arrival, exactly as the
 *  animated form does.
 *
 *  An EXPLICIT gesture, and the only kind left, so it supersedes any in-flight
 *  notification deep-link claim: the deep-link owns the viewport until it
 *  settles, and this is the user saying otherwise. Nothing in the app calls this
 *  on the user's behalf any more, so there is no longer an `auto` variant that
 *  had to defer to the claim instead. A send does not call it either: a send
 *  arms the same follow but lands on the reader's own message rather than
 *  jumping to the bottom (see `followSentMessage`). */
export function scrollToBottom() {
  clearPendingEventScroll();
  // Cancel any in-flight navigation so a down-tap right after an up-tap isn't
  // dragged back toward the top.
  cancelScrollAnim();

  const target = resolveTarget();
  if (target) markNavigationScroll(target, target.scrollHeight);
  syncAwayFromBottom();
  armFollowBottom();
}

/** Reconcile the chevron against where the transcript actually sits. Used by the
 *  two explicit go-to-bottom entry points, whose own write may leave the
 *  container unmoved (and therefore fire no scroll event) when it was already
 *  there. */
function syncAwayFromBottom() {
  const el = resolveTarget();
  if (!el) return;
  awayFromBottom.value = !isAtLiveEdge(el);
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
/** How long a deep-link waits for its target to appear before giving up and
 *  recovering. Exported because it is the budget for waiting out ONE lazily
 *  loading thread, and a caller that has its own pre-navigation wait on the same
 *  thread (`showEventWhereItLives`, resolving the anchor before it focuses)
 *  must spend the same budget rather than a second literal that drifts. */
export const EVENT_RESOLVE_DEADLINE_MS = 4000;
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
 *  Plain mutable variable (not a signal) like `_activeScrollElement`: it's read
 *  imperatively by `useScrollMemory`'s restore gate, which already re-runs on
 *  the key / paused changes that matter, so nothing needs to react to it.
 *
 *  What it still guards, now that no auto-scroll competes for the viewport:
 *   - `useScrollMemory.shouldRestore`, so focusing an UNfocused thread does not
 *     land on the saved position instead of the deep-linked event (its restore
 *     observers fire on the same lazily-loaded render the deep-link is waiting
 *     for, and would otherwise win by running last);
 *   - `useScrollMemory`'s no-save reset to the top, for the same reason;
 *   - the navigation focus marker's settle guard, so this deep-link's own smooth
 *     scroll cannot dismiss the highlight it just applied.
 *  It is held until the deadline (or, on a synchronous resolve, until the scroll
 *  settles) so it covers the whole landing, not just the call. */
let _pendingEventScrollClaim: object | null = null;

/** True while a notification deep-link is waiting for its target event to
 *  render (or to scroll to it), so a competing scroll defers to it. */
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
  /** This call's claim identity. See `_pendingEventScrollClaim`: an object, so
   *  that re-opening the SAME notification within the resolve window is two
   *  distinguishable claims rather than one indistinguishable selector. */
  const claim = {};

  // Claim the deep-link scroll so a competing scroll defers (see
  // _pendingEventScrollClaim). The claim is held until the deadline below, and
  // NOT released the instant the scroll lands: the same render that finally
  // shows the event is also what wakes useScrollMemory's restore observers, and
  // they would otherwise land the saved position over the deep-link's.
  _pendingEventScrollClaim = claim;
  // Force ThreadView to render the FULL exchange list so a windowed-out target
  // can render for tryResolve/the MutationObserver to find. Stays true until the
  // claim releases; ThreadView keeps the thread fully rendered afterward so the
  // user isn't snapped back to the windowed tail mid-read.
  deepLinkRenderAll.value = true;

  // Release this call's claim, but only if it's still ours. A second deep-link
  // started mid-flight has overwritten the slot with its own claim; its own
  // release handles that one. Without the guard, this call's deadline would
  // clear the newer claim, un-guarding the saved-scroll restore over a landing
  // that is still live, and report a failure for a navigation that succeeded.
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
  };

  // Hold the claim across the smooth scroll's settle, then release. Used by the
  // SYNCHRONOUS resolve path (the already-focused thread, whose events are
  // already in the DOM): releasing the claim the instant smoothScrollToElement is
  // CALLED is wrong, because the tween lands up to SCROLL_MAX_MS later and a
  // competing scroll in that window would override the landing. Release on the
  // scroll container's `scrollend` (the authoritative
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
    // stays until the user takes any action (and never dissolves before its hold has
    // elapsed), then dissolves slowly. The settle guard defers the
    // dismissal while THIS deep-link's own smooth scroll is still settling
    // (hasPendingEventScroll), so the landing scroll can't self-clear the marker.
    applyNavFocus(pulseEl, { settleGuard: hasPendingEventScroll });
  };

  tryResolve();
  if (resolved) {
    // Synchronous resolve — the thread's events were already in the DOM (it was
    // already focused), so no async load follows. Do NOT release the claim
    // synchronously: the deep-link scroll (smoothScrollToElement) is still
    // settling, and a competing scroll would override the landing. Hold the
    // claim until the scroll settles so everything keeps deferring across it.
    holdClaimUntilScrollSettles();
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
  // Deadline. Two outcomes reach it, and only one of them is a failure.
  //
  //  - The target DID render and `tryResolve` landed on it (the observer path).
  //    The timer then exists purely to release the claim, which kept the
  //    post-resolve re-renders suppressed. Nothing else to do.
  //  - The target NEVER rendered: it isn't in this thread, it renders nothing at
  //    all, or the thread was still loading when the window closed. That is a
  //    dead deep-link, and it used to end here in silence, leaving the tap
  //    looking broken with no feedback whatsoever. It now tells the user, via
  //    the caller's `onUnresolved` (scrollState stays free of the `store`
  //    import; see `parseNavigatedTurn`).
  //
  // It reports WITHOUT moving the transcript. The user asked to go to a place;
  // the place does not exist, and the bottom is not it. (This used to scroll to
  // the thread's most recent turn, guarded by a `watchUserAction` watcher so a
  // reader who had scrolled away meanwhile was not yanked 4s later. Both the
  // recovery scroll and the watcher it needed are gone: leaving the reader where
  // they are is now the rule rather than the exception, so there is nothing left
  // to stand down from.)
  //
  // A deep-link superseded mid-flight (`wasOurs` false) reports nothing: the
  // newer one owns the claim, the viewport and the outcome now.
  deadlineTimer = setTimeout(() => {
    const wasOurs = _pendingEventScrollClaim === claim;
    stopWatching();
    releaseClaim();
    if (resolved || !wasOurs) return;
    onUnresolved?.();
  }, EVENT_RESOLVE_DEADLINE_MS);
}

/** Shared options for the two deep-link entry points below. */
export interface DeepLinkOptions {
  /** Called when the deep-link's target never renders and the resolve deadline
   *  expires: a dead link the user has to be told about. The MESSAGE is the
   *  caller's, not this module's, because `scrollState` deliberately stays free
   *  of the heavy `store` import (`showToast` lives there) so its lean importers
   *  keep working, the same constraint `parseNavigatedTurn` documents.
   *
   *  The words are the WHOLE recovery: a dead link leaves the transcript exactly
   *  where it was. */
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
 *  deep-link claim. `awayFromBottom` is reconciled against the ACTUAL landing
 *  target rather than assumed: a jump that lands at the bottom must hide the
 *  chevron, and the last turn's clamped target often can't move an already
 *  bottomed container, so no scroll event would arrive to do it. No-op when no
 *  transcript is visible (thread pane collapsed, or the hidden dual-mount copy)
 *  or there's no turn in `direction`. Desktop moves DOM focus into the
 *  (focusable) container so the native scroll keys follow the jump. */
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
  // hasn't scrolled since the last nav (a scroll gesture retires its ref), so index
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
  // in-flight deep-link claim.
  clearPendingEventScroll();

  // Absolute target scrollTop that puts the turn's top `gap` px below the container
  // top. Re-read each frame (via animateScroll) so a layout shift during streaming
  // is tracked; the term is stable as scrollTop changes because the turn's viewport
  // top moves by the same amount.
  const targetOf = (c: HTMLElement) =>
    typeof c.getBoundingClientRect === 'function'
      ? turn.getBoundingClientRect().top - c.getBoundingClientRect().top + c.scrollTop - gap
      : 0;

  // Reconcile the chevron against the ACTUAL landing target instead of
  // hardcoding "parked mid-thread". The last turn (and any turn near the end) has a
  // landing target at/beyond maxScroll, so the browser clamps the scroll to the
  // bottom — and when we're ALREADY at the bottom the clamped write doesn't move the
  // container, so no scroll event fires and onScroll never reconciles the chevron.
  // Hardcoding awayFromBottom=true there left the down chevron stuck on ("appears
  // the second time you click down arrow"). (2px slack mirrors isVisuallyAtBottom.)
  const maxScroll = Math.max(0, el.scrollHeight - el.clientHeight);
  awayFromBottom.value = targetOf(el) < maxScroll - 2;

  // Mark the landed turn with the shared navigation focus marker (a background
  // highlight that sticks until the user's next scroll gesture, and for its hold even
  // then). clearPendingEventScroll
  // above already cleared any prior marker via clearNavFocus, so this is a clean
  // supersede; no settleGuard — the animateScroll below is programmatic (emits no
  // wheel/touch/keydown), so it can't self-clear, and a real user scroll should.
  applyNavFocus(turn);

  if (prefersReducedMotion()) {
    cancelScrollAnim();
    markNavigationScroll(el, Math.max(0, targetOf(el)));
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

/** Build the scroll- and resize-event handlers for a single .thread-content
 *  element. The visibility gate at the top of each handler is required by the
 *  dual-mounting contract documented at the top of this file. */
export function makeScrollObservers(el: HTMLElement) {
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

  /* ── Reflow anchoring: hold the reader still across a WIDTH change ─────────
   *  Resizing a pane changes the transcript's width, and every wrapped line
   *  re-wraps. The content ABOVE the viewport changes height, so the same
   *  scrollTop shows a different part of the thread: narrowing the thread pane
   *  makes the transcript taller and carries the reader UP into older turns.
   *
   *  The browser will not do this for us. Chromium's scroll anchoring treats a
   *  width change on the scroll container as a suppression trigger, and WebKit
   *  has never implemented scroll anchoring at all (WebKit #171099, the same
   *  gap `withScrollAnchor` covers for DOM mutations).
   *
   *  The anchor has to describe the layout BEFORE the reflow, and a
   *  ResizeObserver only ever runs after it, so both handlers keep a running
   *  snapshot of it: which child the reader is parked on, and where that child's
   *  top sat relative to the viewport. Two MEASURED positions, before and after,
   *  are all the correction needs. That is what makes it immune to the browser
   *  clamping scrollTop, which it does whenever a widening pane leaves the
   *  transcript shorter than the reader's old offset: a correction derived from
   *  a scrollTop delta would read that clamp as the reader having scrolled. */
  let lastWidth = el.clientWidth;
  let anchorChild: HTMLElement | null = null;
  let anchorRelTop = 0;

  function viewportTop() {
    return el.getBoundingClientRect().top;
  }

  /** Snapshot the child the reader is parked on: the last one whose top is at or
   *  above the viewport top. Scanned from the END because the reader is normally
   *  near the newest turn, so the loop usually stops on its first step. Children
   *  with no box are skipped, since on desktop the mobile title row is
   *  `display: none` and reports an all-zero rect that would otherwise read as
   *  "far above".
   *
   *  Cheap on the scroll path despite the rect reads: both callers run
   *  `isElementVisible(el)` first, which already forces any pending style/layout
   *  flush, so these reads hit a clean layout tree rather than triggering one. */
  function recordAnchor() {
    anchorChild = null;
    const kids = el.children;
    if (!kids || typeof el.getBoundingClientRect !== 'function') return;
    const top = viewportTop();
    for (let i = kids.length - 1; i >= 0; i--) {
      const kid = kids[i] as HTMLElement;
      const rect = kid.getBoundingClientRect();
      if (rect.height <= 0) continue;
      if (rect.top - top > 0) continue;
      anchorChild = kid;
      anchorRelTop = rect.top - top;
      return;
    }
    // Nothing starts at or above the viewport top: the reader is at the very top
    // of the transcript, where no content above can grow. Leaving the anchor
    // null keeps the reflow correction a no-op, which is the right answer there.
  }

  /** Put the reader back where the width change moved them. Runs inside the
   *  ResizeObserver callback, i.e. after layout and before paint, so the
   *  correction is never painted as a jump.
   *
   *  Every reader is held on their anchor, including one sitting at the very
   *  bottom. This used to branch: a reader within the 80px stickiness window was
   *  re-pinned to the bottom instead of anchored, on the reasoning that riding
   *  the newest turn means "keep me on the newest turn" whatever the layout
   *  does. That is a bottom-pin wearing anchor preservation's clothes, and it
   *  fired on a reader who had deliberately scrolled 79px up. Holding the anchor
   *  is the honest reading of "keep the reader on the same content", and it is
   *  the same answer for everyone. */
  function restoreAfterReflow() {
    const child = anchorChild;
    if (!child || child.isConnected === false) return;
    const shift = (child.getBoundingClientRect().top - viewportTop()) - anchorRelTop;
    if (shift === 0) return;
    el.scrollTop = el.scrollTop + shift;
    // Carry the follow's stamp onto the correction. This is the app holding the
    // reader on the same content, not the reader leaving, and the scroll event
    // it fires would otherwise arrive off the live edge at a position the follow
    // does not recognise, which is both halves of the disarm. The growth branch
    // usually re-stamps a line later, but not when it stands down for a tween or
    // a pending send landing, which is exactly when a pane resize retired a
    // follow nobody retired.
    if (_followEl === el) _followTop = el.scrollTop;
  }

  // Scroll events. Whoever moved the container (a gesture, a chevron, a restore,
  // the reflow correction), the answer is the same: reconcile the three position
  // signals against where it now sits, and re-take the reflow anchor. There is
  // no longer anything to suppress, because nothing infers intent from a scroll.
  //
  // One question IS asked of the scroll, and it is the only place a standing
  // follow is retired: has the reader taken the container away from where the
  // follow put it. Both halves are required. Off the live edge alone is not
  // enough, because a shrink clamps the reader down and the app's own anchor
  // correction moves them while holding them on the same content; moved alone is
  // not enough either, because a tween mid-glide is our own. Together they are
  // true for a wheel, a scrollbar drag, a touch flick and its momentum, a
  // keypress and a mobile pane swipe, and for a navigation that deliberately
  // lands the reader elsewhere. They are false for everything that changes
  // content without moving the reader off the edge, which is why answering a
  // question, granting a permission or expanding a turn all keep the follow.
  function onScroll() {
    if (!isElementVisible(el)) return;
    syncNotAtTop();
    syncScrolledFromTop();
    const atEdge = isAtLiveEdge(el);
    awayFromBottom.value = !atEdge;
    if (_followingBottom && !atEdge && !isWhereTheFollowLeftIt(el)) stopFollowingBottom();
    // The reader has moved, so the reflow anchor has to follow them.
    recordAnchor();
  }
  // Resize events. A resize moves the reader toward the bottom for exactly one
  // reason: they ASKED to ride the live edge and have not taken it back (see
  // "The standing request to ride the live edge"). For everyone else it moves
  // nobody, whatever grew and however far off the bottom it leaves them: a
  // streaming reply, a decoded image, an expanded step, a growing composer all
  // leave the transcript exactly where it is, and the handler's remaining job is
  // to reconcile the signals so the chevrons describe the new geometry.
  //
  // This is where the old force-pin lived, and the two rules that fought over
  // it: a 'scroll'-mode branch that slammed `scrollTop = scrollHeight` on every
  // resize inside a 500ms window, and a follow branch that re-pinned any reader
  // inside the 80px stickiness window. Neither is back. The branch below infers
  // nothing: it reads the flag the reader's own chevron tap or send set.
  function onResize() {
    if (!isElementVisible(el)) return;
    // A WIDTH change re-wrapped the transcript, so undo the drift it caused
    // before anything below reads the new geometry (see "Reflow anchoring").
    // Height-only growth, the streaming case, needs no correction: the content
    // above the reader is unchanged, so the same scrollTop still shows them the
    // same thing, and the transcript simply grows below them.
    const width = el.clientWidth;
    if (width !== lastWidth) {
      lastWidth = width;
      restoreAfterReflow();
    }
    // The growth branch. Runs after the reflow correction, so the follow writes
    // from the corrected position rather than fighting it, and before the signal
    // reconciles below, so the chevron describes where the follow left the
    // reader rather than where the growth alone would have.
    if (_followingBottom) followTheLiveEdge(el);
    syncNotAtTop();
    syncScrolledFromTop();
    // One unconditional reconcile, both directions. Growth below the fold raises
    // the chevron on the very next frame, which matters more than it used to:
    // an unarmed reader at the live edge is left behind by the first token of a
    // reply and the chevron is their only way back. A shrink that leaves them
    // visually at the bottom again (an idle banner removed, a step collapsed)
    // hides it, without waiting for a scroll event that a pure content change
    // never fires.
    awayFromBottom.value = !isAtLiveEdge(el);
    // Retake the snapshot last, once the layout above has settled: measuring it
    // any earlier would describe a position the reader is no longer at.
    recordAnchor();
  }
  return { onScroll, onResize };
}
