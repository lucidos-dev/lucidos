import { signal, type ReadonlySignal } from '@preact/signals';
import { prefersReducedMotion } from '../../utils/platform';
import { USER_SCROLL_WINDOW_MS } from '../../utils/scrollActivity';
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
 *  `scrollToBottomAnimated`), the FOLLOW TOGGLE (`setFollowLiveEdge`), the four
 *  SUBMITS (sending a message, submitting an answer to a question card, deciding
 *  a permission card, and Continue after an abort, all through `followSubmit`),
 *  ⌘↑/⌘↓ turn stepping (`stepThreadTurn`), a notification / Changes deep-link
 *  (`scrollToEventAndPulse` / `scrollToChangeAndPulse`), and `useScrollMemory`
 *  returning a reader to the position they left. Everything else leaves the
 *  reader exactly where they are: a streaming reply, a question or permission
 *  card ARRIVING, a thread sync, a thread opening.
 *
 *  ONE of those asks is STANDING rather than one-shot: the follow toggle means
 *  "take me to the live edge and KEEP me there until I say otherwise", so it
 *  arms the follow described further down. That is a rule about the DURATION of
 *  an ask, not an exception to the rule above. The down CHEVRON is the one-shot
 *  half of the same journey and arms nothing, which is why they are two
 *  controls: one button cannot be go-there, stay-here and stop-staying at once.
 *
 *  A SUBMIT ARMS NOTHING. It is one ask with one reaction, whichever of the four
 *  shapes it takes: a reader who is already riding the live edge is taken there,
 *  because that is the chevron's standing ask being served; everyone else gets a
 *  one-shot LANDING that rests the turn's agent status line on the bottom of the
 *  viewport, so they see the agent take what they just submitted with the reply
 *  growing in underneath. Neither outcome outlives the moment. This is a change
 *  from the earlier rule, where a send and an answer armed the follow: riding is
 *  the chevron's alone now.
 *
 *  A SUBMIT WRITES NOTHING ONLY WHEN THERE IS NOWHERE TO GO, and that is decided
 *  by measuring the landing rather than by predicting it: if the status line is
 *  already resting at or above the bottom edge, the target is behind the reader
 *  and nothing is written. A thread that fits on screen answers that way by
 *  construction. Two earlier branches tried to reach the same conclusion BEFORE
 *  the turn rendered, and both got the ordinary case wrong; see `followSubmit`.
 *
 *  Four things retire a standing follow, and all four are the reader: their own
 *  scroll GESTURE, a chevron or turn-nav press, opening another thread, and a
 *  DEEP-LINK LANDING. A link is the reader asking to be at one specific place,
 *  so the ride ends where it puts them and the transcript stops moving
 *  underneath them. A GESTURE and not merely a scroll, because the platform
 *  scrolls the container too (see "Was this scroll the reader's own GESTURE?");
 *  the last three are presses rather than gestures, so each retires the ride at
 *  its own call site.
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
/** Whether the tween in flight is a HELD scroll (see `markHeldScroll`) rather
 *  than a one-off navigation. Set from the marker `animateScroll` was handed, so
 *  it cannot disagree with who is writing the frames. Read by
 *  `stopFollowingBottom` and `cancelLanding`, which must stop our own motion and
 *  no one else's. */
let _heldAnim = false;
/** WHICH of the two held glides is in flight, when one is: the live edge (a
 *  submit made while the standing follow was already armed) or the reader's own
 *  turn (the landing every other submit runs). `_heldAnim` cannot answer this,
 *  and the difference decides two things. What a submit arriving mid-tween does:
 *  leave a live-edge glide alone (it is already going where this submit wants),
 *  supersede a landing (its target is the wrong answer for a reader who is now
 *  riding). And what the reader's own scroll cancels: a landing always, a
 *  live-edge glide only through the follow's disarm. Set by each glide right
 *  after it starts, and cleared with the tween, so a glide that forgot to set it
 *  reads as supersede-able, which is the harmless direction. */
let _heldAnimTarget: 'live-edge' | 'own-turn' | null = null;
/** Forget the tween: no rAF pending, and nobody owning one. Every way a tween
 *  ENDS goes through this, the natural landing included, so both flags above
 *  mean "in flight" rather than "was in flight last time". A landing that left
 *  them set would answer for a tween that finished seconds ago, and
 *  `glideToLiveEdge` would stand down for it. */
function endScrollAnim() {
  _scrollAnimRaf = null;
  _heldAnim = false;
  _heldAnimTarget = null;
}
function cancelScrollAnim() {
  if (_scrollAnimRaf !== null) cancelAnimationFrame(_scrollAnimRaf);
  endScrollAnim();
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
 *  ONE reader action means "take me to the bottom and KEEP me there" rather than
 *  "jump once": the FOLLOW TOGGLE in the prompt area (`setFollowLiveEdge`). It
 *  arms the flag below, and while it is armed, content growth writes the reader
 *  back to the live edge (the growth branch in `makeScrollObservers`' onResize,
 *  which is where the old force-pin used to live).
 *
 *  Nothing else arms it. Not the down chevron, which navigates and nothing else,
 *  and not any SUBMIT: a submit gets the one-shot reaction `followSubmit`
 *  describes, so a reader is carried through a reply only when they asked to be
 *  carried through replies. Nor an SSE sync confirming a pending message, a
 *  change applied / discarded / reverted, a granted permission, a coding-agent
 *  action, a lazy load, a deep link, or a thread opening. Nor the question card
 *  ARRIVING, which is the agent's doing. Being AT the bottom does not arm it
 *  either: a position is not a request, and a reader who merely happens to sit
 *  at the live edge has asked for nothing.
 *
 *  Two earlier arming rules are named because both were deliberate and both are
 *  gone. A SEND AND AN ANSWER used to arm it, so the reply dragged the reader
 *  down through itself. And the CHEVRON did, briefly, which was the same mistake
 *  one step along: it left the mode with no visible state, no way off but
 *  scrolling, and no way ON for a reader already at the live edge, since the
 *  chevron is hidden exactly there. Putting the state on the chevron does not
 *  rescue it, because one button cannot hold both jobs: go-to-bottom, then arm,
 *  then disarm is a three-step cycle with no state left over for a plain jump to
 *  the bottom.
 *
 *  A SUBMIT MADE WHILE IT IS ARMED goes to the live edge, whichever of the four
 *  submits it is, and that is this same request being SERVED rather than a
 *  second arming point. Riding the live edge means "keep me at the bottom", so
 *  landing such a reader on their own turn would answer a request they did not
 *  make with one they did not make either. Armed but off the edge is ordinary,
 *  not a corner: the growth branch stands down while a tween owns the scroll, so
 *  a reply that streams entirely during a glide leaves the reader parked above
 *  the bottom with the follow still armed and no growth left to carry them out
 *  of it.
 *
 *  The flag is a SIGNAL rather than a plain boolean because the toggle renders
 *  it: the button has to say whether the reader is riding, and it has to go off
 *  by itself when the reader's own scroll retires the follow underneath it. It
 *  is exported as a `ReadonlySignal<boolean>`, which the compiler refuses to let
 *  a component assign, so reading the state cannot become a way of setting it.
 *  That is the same reasoning `_followArmedListeners` records for staying a
 *  callback set: an exported WRITABLE signal would be an arming point no source
 *  scan could stop.
 *
 *  The request BELONGS TO A THREAD and outlives leaving it. The flag here is one
 *  global, so `focusThread` retires it on every open (a thread the reader just
 *  opened is not one they asked to follow), and that used to be the end of the
 *  request: coming back landed on the pixel offset the transcript had when they
 *  walked away, with everything the agent produced meanwhile below them and
 *  nothing following. So the request is WRITTEN DOWN per thread, as one of the
 *  two forms a reading position takes (`hooks/useScrollMemory.ts`), and
 *  `resumeFollowingBottom` re-arms it on re-entry. That is the same request
 *  resumed, not a second arming point: only a chevron request can ever be
 *  recorded, since only the chevron arms, so the resume can only ever replay one
 *  the reader made in this thread. A reader who merely parked at the bottom
 *  saves the offset instead and comes back to it following nothing.
 *  `isFollowScroll` and `onFollowArmed` are the two things the recording side
 *  needs; both are below.
 *
 *  The condition for following is this flag and NOTHING ELSE. There is no
 *  proximity term (the retired 80px stickiness window) and no timing term (the
 *  retired 500ms suppression window); both of those tried to INFER the request
 *  that the flag now records.
 *
 *  A DEEP LINK is the one navigation that retires the follow by calling
 *  `stopFollowingBottom` rather than being read off the position (see
 *  `markHeldScroll`), because a link into the thread the reader is riding
 *  usually points at its newest turn, and a landing at the live edge is exactly
 *  the shape of "content changed and the reader did not leave". Read off the
 *  position alone it would keep the follow, and the next token would carry the
 *  reader off the event they asked for. */
const _followingBottom = signal(false);

/** Is the standing follow armed? Read by the follow toggle, which RENDERS it.
 *  `ReadonlySignal` on the way out on purpose: see the block above. */
export const followingLiveEdge: ReadonlySignal<boolean> = _followingBottom;

/* ── The follow SEED ─────────────────────────────────────────────────────────
 *  The reader's last PRESS of the toggle, remembered across threads and reloads,
 *  and what a thread with no *reading position* of its own starts as. Same shape
 *  as `selectedScope` for the destination picker: a last-used value that seeds
 *  the NEXT thread and is never read by one that already remembers, which is
 *  exactly what keeps it from leaking into threads the reader has parked in.
 *
 *  It answers the thing the per-thread record cannot. The record is written by
 *  being in a thread, so a BRAND-NEW thread has none by construction, and a
 *  reader who rides everything had to press the button again on every one. The
 *  seed is also the only state the toggle can show in the compose view, which has
 *  no transcript, and showing it there is what lets the follow be armed BEFORE
 *  the first send, which is when a reader most reliably knows they want it.
 *
 *  Written by the toggle and by nothing else. A scroll retiring the follow is
 *  about THIS thread and records itself as that thread's offset; it must not
 *  quietly cancel a standing preference for every future thread.
 *
 *  Device-scoped, because whether to ride the live edge is a property of the
 *  screen in front of the reader rather than of the account, and it has to be
 *  right on the first paint with no server round trip. */
const FOLLOW_SEED_KEY = 'lucidos-follow-live-edge';

/** `localStorage` is absent in the DOM-free unit environment this module is
 *  deliberately importable from (see `parseNavigatedTurn` on why it stays free of
 *  the heavy `store` import), so both sides of the seed check for it. Off is the
 *  right answer when there is nowhere to remember: nothing rides by default. */
function readFollowSeed(): boolean {
  if (typeof localStorage === 'undefined') return false;
  return localStorage.getItem(FOLLOW_SEED_KEY) === 'true';
}

const _followSeed = signal(readFollowSeed());

/** What the toggle shows where there is no transcript to describe, i.e. the
 *  compose view. `ReadonlySignal` for the same reason `followingLiveEdge` is: the
 *  one writer is the press, through `setFollowLiveEdge`. */
export const followLiveEdgeSeed: ReadonlySignal<boolean> = _followSeed;

/** Remember this press. The ONE writer, called only from `setFollowLiveEdge`. */
function recordFollowSeed(on: boolean): void {
  _followSeed.value = on;
  if (typeof localStorage !== 'undefined') localStorage.setItem(FOLLOW_SEED_KEY, String(on));
}

/** Apply the seed to `el`: arm the follow if that is what the reader last chose,
 *  and report whether it did (the caller has to know, because arming writes the
 *  live edge and its own no-position reset would undo it).
 *
 *  Named for what it DOES rather than for when it is right to call, because it
 *  checks only the seed. WHERE the seed may speak is the caller's to decide and
 *  is the load-bearing half: `attachScrollMemory` calls it in the one branch with
 *  no *reading position* at all, and only for the transcript. A thread the reader
 *  HAS parked in must keep deciding for itself, and the content pane and the
 *  thread drawer must never arm the transcript's follow.
 *
 *  Deliberately routed through `resumeFollowingBottom` rather than arming
 *  directly: it is the same act (write the live edge THIS thread has now, then
 *  arm), the write is what stops the reader being left at the outgoing thread's
 *  offset in the shared container, and reusing it keeps the arming entry points at
 *  two. */
export function applyFollowSeed(el: HTMLElement): boolean {
  if (!_followSeed.value) return false;
  resumeFollowingBottom(el);
  return true;
}

/** Is the agent LIVE on the thread being shown, i.e. is anything running on it?
 *  Told to this module by `ChatExchange` for its `isLast` turn, because
 *  `scrollState` must not import `store` (see `parseNavigatedTurn`) and the
 *  answer is the store's to derive. The derivation is
 *  `exchangeMarksThreadLive`, which needs BOTH the turn's status and the thread
 *  projection's own quiescence; read its comment before treating the last
 *  turn's status as the whole answer. A plain mutable variable, like
 *  `_activeScrollElement`:
 *  nothing needs to react to it changing, it is read imperatively.
 *
 *  Read at EXACTLY ONE site, the disarm in `onScroll`, through `threadIsLive`,
 *  and that narrowness is the invariant rather than an implementation detail. It
 *  answers "is the reader fleeing a reply, or merely browsing", which is a
 *  question about what their scroll MEANS. It must never become a second
 *  condition on following, on landing, or on any write: a lull between two tool
 *  calls would then stop a reader being followed, which is the opposite of what
 *  it is for. */
let _threadLive = false;

/** When the SUBMIT's own claim that the thread is live runs out. A submit says
 *  so before any status can (see `followSubmit`), and that claim has to EXPIRE
 *  rather than stand until something contradicts it, because the thing that
 *  would contradict it may never come: a Continue whose POST fails, or a
 *  permission decision the engine never answers, leaves the last turn's status
 *  exactly as it was, so `ChatExchange`'s effect never re-runs and never writes
 *  `false`. Left standing, the claim would quietly cost the reader their follow
 *  the next time they browsed an idle thread, which is the one thing the live
 *  term exists to prevent. */
let _submitLiveUntil = -Infinity;

/** How long a submit's claim outlives the submit. It has to cover the POST round
 *  trip AND the whole gap before the thread projection says `running`, which is
 *  the longer of the two and the reason this is not a couple of seconds: the
 *  client's `meta.status` only advances when a per-event aggregate carrying
 *  `running` arrives, and `store.ts`'s `isRenderedThreadIdle` documents that gap
 *  running to about eight seconds on a resume (its own carve-out covers only the
 *  part before the message is ingested).
 *
 *  Being wrong in the LONG direction merely restores the pre-live-term behaviour
 *  for a few more seconds (a scroll retires the follow) on a thread the reader
 *  just submitted to, which is the least likely moment for them to be idly
 *  browsing. Being wrong in the SHORT direction re-opens the gap the claim exists
 *  for. */
const SUBMIT_LIVE_CLAIM_MS = 20_000;

/** Tell this module whether the thread on screen has a turn in flight. Called by
 *  `ChatExchange` for its last exchange, and cleared when that exchange
 *  unmounts, so a thread switch cannot leave the previous thread's answer
 *  standing.
 *
 *  A `true` retires a submit's claim, because the thing the claim was guessing at
 *  has arrived and there is nothing left to guess. A `false` DOES NOT, and that
 *  asymmetry is the whole point: `false` is exactly what a lagging source says
 *  during the window the claim covers. `exchangeMarksThreadLive` needs the thread
 *  projection to agree, and the projection is the slow half, so the render right
 *  after a send writes `false` while the agent is on its way. Clearing the claim
 *  there destroyed it in the one window it was invented for, and a reader who
 *  submitted and then scrolled away kept a follow they had just fled (reported
 *  2026-08-10, intermittent because it depended on landing inside the gap).
 *  Nothing is lost by ignoring `false`: the claim expires on its own. */
export function setThreadLive(live: boolean): void {
  _threadLive = live;
  if (live) _submitLiveUntil = -Infinity;
}

/** Is the agent live, by the status or by a submit's unexpired claim? */
function threadIsLive(): boolean {
  return _threadLive || nowMs() < _submitLiveUntil;
}

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

/** Resolves the turn a submit was made on, or null while that turn is not
 *  addressable yet. Asked once at submit time and, when it answers null there,
 *  again on every growth round until it answers or the landing lapses (see
 *  `_pendingLanding`). */
type TurnResolver = (el: HTMLElement) => HTMLElement | null;

/** A submit whose own turn has not rendered yet, holding its resolver and when
 *  the submit happened. Two of the four submits are in this shape: the turn a
 *  send lands on is its optimistic message row, which arrives a frame or more
 *  after the send while the composer collapsing has already fired a resize, and
 *  the turn Continue lands on does not exist at all when the button is pressed
 *  (the continuation renders as a fresh `ContinuationStarted` exchange). So the
 *  landing cannot just run on the next growth: it waits until its own turn is
 *  there, and moves nobody until then. Null when no submit is waiting.
 *
 *  The two card submits never use it, because the card the reader just tapped is
 *  on screen by construction, so their landing resolves at submit time and
 *  glides immediately. */
let _pendingLanding: { resolveTurn: TurnResolver; at: number } | null = null;

/** How long a deferred landing waits for its own turn before LAPSING and moving
 *  nobody.
 *
 *  Generous, because it is only ever reached when the turn is not individually
 *  addressable rather than merely late: the second and subsequent queued
 *  follow-ups fold into a CLOSED `<details class="queued-message-group">` (see
 *  `CreateThreadView`), whose contents have no box at all, so the message the
 *  reader just sent has no rect to land on.
 *
 *  Lapsing moves nobody, and does NOT fall back to the live edge. It used to,
 *  because the send had armed a standing follow that had to be honoured somehow;
 *  with no arming there is nothing to honour, and the case the deadline covers
 *  is precisely a turn with no response panel, so there is no agent status line
 *  to show the reader even if they were taken there. Without the deadline a
 *  pending landing would sit forever and hold the growth branch inert. */
const LANDING_DEADLINE_MS = 1000;

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

/** The scroll offset the live edge sits at: the MAX offset rather than
 *  `scrollHeight`, which the browser would clamp to the same place. Naming the
 *  real target keeps every write meaningful instead of leaning on the clamp, and
 *  one definition keeps everything that aims at the live edge (the follow's
 *  per-growth write, its resume, its submit glide, the down chevron's tween) or
 *  measures against it (turn-nav's chevron reconcile) from drifting apart,
 *  exactly as `isAtLiveEdge` does for the threshold. */
function liveEdgeTop(el: HTMLElement): number {
  return Math.max(0, el.scrollHeight - el.clientHeight);
}

/** Can this transcript be scrolled at all? One definition, read by the up
 *  chevron (`notAtTop`), the mobile title fade (`scrolledFromTop`) and the
 *  submit's landing test below, so a transcript with a hair of overflow (a
 *  border, a rounded line height) answers the same for all three: nothing to go
 *  up to, nothing to slide under the title, and nowhere for a submit to take
 *  anybody. That is what the 10px absorbs.
 *
 *  The DOWN chevron deliberately asks something else, `isAtLiveEdge`'s 2px,
 *  because it describes where the reader IS rather than whether the thread can
 *  move at all. */
function isScrollable(el: HTMLElement): boolean {
  return el.scrollHeight > el.clientHeight + 10;
}

/* ── Held scrolls, and how the reader's gesture is told from ours ────────────
 *  `_heldEl` / `_heldTop` record WHERE our own last deliberate write left the
 *  container. Two things hold a reader deliberately and both need it: the
 *  standing follow (its per-growth write and its live-edge glide) and a submit's
 *  landing. For the follow the answer retires the request; for the landing it
 *  cancels the glide, which used to come free as a side effect of the follow's
 *  disarm and needs saying now that a submit arms nothing.
 *
 *  A held write is marked as a navigation scroll like every other write the app
 *  makes, but `isNavigationScroll`'s 64ms window cannot answer THIS question: a
 *  streaming thread re-marks itself every frame, so a flick landing inside the
 *  window would read as ours and the reader would fight us. The POSITION answers
 *  it exactly instead. Content growing below the reader changes `scrollHeight`
 *  and never `scrollTop`, so growth can never look like a gesture; every gesture
 *  (wheel, scrollbar drag, touch flick, momentum, keys, a mobile pane swipe)
 *  changes `scrollTop`, so one always does.
 *
 *  It is also what ends both for a navigation that deliberately puts the reader
 *  somewhere else (the up chevron, turn stepping, a saved-scroll restore), with
 *  no call site of its own: none of those is a held write, so the first frame of
 *  one already reads as the reader being elsewhere. And it is what keeps the
 *  follow ALIVE across everything that is not a scroll: a card resolving,
 *  granting a permission, expanding a turn all change content without moving the
 *  reader off the live edge. */
let _heldEl: HTMLElement | null = null;
let _heldTop = -1;

/** Record where the reader is WITHOUT writing anything, so a landing that has
 *  not moved them yet can still tell their next gesture from our own writes. A
 *  submit takes this stamp the moment it schedules a landing: until the turn it
 *  is waiting for renders there is no write to stamp, and the reader's flick in
 *  that window must still cancel it. */
function holdPosition(el: HTMLElement | null) {
  _heldEl = el;
  _heldTop = el ? el.scrollTop : -1;
}

/** Write `top` and record it as OURS, so the scroll event it fires a frame later
 *  cannot be read as the reader taking over. Goes through `markNavigationScroll`
 *  like every other write the app makes, so the mobile header and the
 *  render-window expansion keep standing down for it too. */
function markHeldScroll(el: HTMLElement, top: number) {
  markNavigationScroll(el, top);
  holdPosition(el);
}

/** Arm the standing follow at the position the caller's own scroll just reached,
 *  so the trailing scroll event of that scroll cannot retire the request it just
 *  made. Two callers and no more: `setFollowLiveEdge` (the follow toggle) and
 *  `resumeFollowingBottom` (which replays a toggle request the reader made in
 *  this thread earlier). It takes its element rather than resolving one, for the
 *  restore, which holds the container it is positioning and must not ask
 *  `resolveTarget` for a different one (a thread opening mid-layout-swap can
 *  answer with the outgoing mount).
 *
 *  Notifies only on the unarmed to armed transition, so the recording side is
 *  told about a request the moment it is made even when arming produces no scroll
 *  event at all. That case is ordinary rather than exotic: a reader already at the
 *  live edge who presses the toggle gets no write at all, and an idle thread then
 *  grows nothing, so no scroll ever carries the request anywhere. */
function armFollowOn(el: HTMLElement | null) {
  const wasArmed = _followingBottom.value;
  _followingBottom.value = true;
  holdPosition(el);
  // A pending landing and an armed follow are mutually exclusive by
  // construction, and this is the line that makes it so: a submit never arms, so
  // the only way to arm over a waiting landing is the reader pressing the
  // chevron, which supersedes it. Everything downstream (the growth branch, the
  // cancel in `onScroll`) may therefore assume at most one of the two.
  _pendingLanding = null;
  if (!wasArmed) for (const listener of _followArmedListeners) listener();
}

/** Resume a standing follow the reader armed in this thread BEFORE they left it:
 *  write the live edge and arm, so the growth branch carries them from there.
 *
 *  Called only by `attachScrollMemory`, on a thread whose recorded reading
 *  position is the live edge. The write is required and not merely tidy:
 *  `.thread-content` is one element reused across threads, so on arrival it holds
 *  the OUTGOING thread's offset, and arming alone would leave the reader there
 *  until the next growth round. It goes through `markHeldScroll` like every
 *  other follow write, so the mobile header stands down for it and the render
 *  window does not read it as the reader asking for older turns.
 *
 *  Nothing waits for content here, unlike the offset restore's observer retries.
 *  An offset can only be honoured once the transcript is tall enough to hold it,
 *  whereas the live edge is wherever the content currently ends: the write lands
 *  on today's bottom and the armed follow rides every later arrival to the real
 *  one. */
export function resumeFollowingBottom(el: HTMLElement): void {
  markHeldScroll(el, liveEdgeTop(el));
  armFollowOn(el);
}

/** Is the scroll event being handled the FOLLOW's own write rather than the
 *  reader's gesture? Armed, and the container still exactly where the follow put
 *  it (see `isWhereWeHeldIt` for the 1px slack and why position is the right
 *  question).
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
  return _followingBottom.value && isWhereWeHeldIt(el);
}

/** Retire the standing follow, and with it anything we were doing to serve it or
 *  any other request. Called by the disarm in `onScroll` (the reader taking the
 *  container away from where a held write put it), and exported for the two
 *  navigations that cannot be read off a scroll.
 *
 *  Opening a DIFFERENT thread: a thread the reader just opened is not one they
 *  asked to follow, and a restore that happens to land on that thread's saved
 *  bottom position writes no scroll the disarm could see. See `focusThread`.
 *
 *  A DEEP-LINK LANDING: the reader asked to be at one specific place, so the
 *  ride ends there and nothing may carry them off it. The disarm cannot see this
 *  one either, and the case it misses is the ordinary one rather than a corner:
 *  a link into the thread the reader is already riding usually points at its
 *  newest turn, so the landing leaves them AT the live edge, where the disarm's
 *  first condition is false and the follow survives to drag them along with the
 *  next token. See `scrollToSelectorAndPulse`. */
export function stopFollowingBottom() {
  // Both of the things that hold a reader are OUR motion, so both stop here.
  // Without it the reader who just scrolled away is dragged back for the rest of
  // the tween (the disarm would say one thing and the next frame do another),
  // and a thread opened mid-glide is scrolled with the previous thread's turn as
  // the target. Only a held tween: a deep-link or up-chevron glide belongs to a
  // navigation this has no business cancelling.
  if (_heldAnim) cancelScrollAnim();
  _followingBottom.value = false;
  _heldEl = null;
  _pendingLanding = null;
}

/** Cancel a submit's LANDING: drop one still waiting for its turn to render, and
 *  stop one already gliding. The landing's half of what `stopFollowingBottom`
 *  does, for the reader who has no follow to retire, which since a submit arms
 *  nothing is every reader who only submitted.
 *
 *  Only the landing's own tween. A live-edge glide is the standing follow's
 *  motion and ends with the follow, on the follow's own two-part test; ending it
 *  here would retire the ride on a scroll event the disarm deliberately ignores
 *  (a shrink clamping the reader down, the reflow correction holding them still
 *  while the layout moves). */
function cancelLanding() {
  if (_heldAnim && _heldAnimTarget === 'own-turn') cancelScrollAnim();
  _pendingLanding = null;
}

/** Retire a pending landing that has outlived `LANDING_DEADLINE_MS`. Called from
 *  the two places a lapse can be noticed: the growth branch, and the next submit.
 *  Neither alone is enough, because the deadline is wall-clock and the growth
 *  branch only runs when something grows. */
function dropLapsedLanding(): void {
  if (_pendingLanding && nowMs() - _pendingLanding.at >= LANDING_DEADLINE_MS) {
    _pendingLanding = null;
  }
}

/** Is a submit's landing in flight, in either of its two phases: waiting for its
 *  own turn to render, or gliding to it. */
function landingInFlight(): boolean {
  return _pendingLanding !== null || (_heldAnim && _heldAnimTarget === 'own-turn');
}

/** Carry the held stamp onto a scroll THE APP just wrote to hold the reader on
 *  the same content while the layout moved under them. Two writers, and they are
 *  the same act on either side of the DOM/layout line: `restoreAfterReflow`
 *  (a pane resize re-wrapped the transcript) and `withScrollAnchor` (a toggle
 *  mutated it). Neither is the reader taking over, so neither may retire a
 *  standing follow or cancel a pending landing, and without the stamp the scroll
 *  event each fires arrives at a position we do not recognise and does both.
 *
 *  It CARRIES a hold rather than taking one, hence the guard: with no hold on
 *  this element there is no follow and no landing to protect, and stamping would
 *  claim a position nobody asked for. */
function carryHeldScroll(el: HTMLElement): void {
  if (_heldEl === el) _heldTop = el.scrollTop;
}

/** What the transcript owes the reader after the APP mutated it and corrected
 *  the scroll to hold them still: `withScrollAnchor`'s side of `honourGrowth`,
 *  and the two say the same thing in the two worlds. Every reveal in the
 *  transcript goes through it: the collapse fold, the per-turn unfold, and the
 *  two transcript-wide turn controls (steps, full response).
 *
 *  ONLY A SCROLL MAY RETIRE THE FOLLOW, so the first line is not optional. A
 *  toggle is a click on a control, not a gesture, and the reader who made it
 *  asked for more of the turn rather than for less of the ride. The correction
 *  moves the container all the same, and without the stamp that write reads as
 *  the reader taking over. The transcript-wide reveals are what make it bite:
 *  they grow every turn, including the ones BELOW the anchored root, so the
 *  correction (which pins that root) leaves the reader short of the live edge
 *  and the follow saw a move it had not made.
 *
 *  Then, ARMED ONLY, put them back ON the live edge in ONE held write, in the
 *  same frame the caller unfreezes: `snapToLiveEdge`. Being short of the edge is
 *  exactly what the standing follow exists to undo, and here it is not a
 *  position the reader ever occupied, only one the mutation left them at between
 *  two writes the browser has not painted yet.
 *
 *  A TWEEN was tried here first, on the reasoning that one click can add the
 *  height of the whole transcript and an instant write teleports the reader. It
 *  is what the reader then reported (2026-08-11): turning the steps on grows
 *  every turn ABOVE them too, so the transcript arrived a screenful or more
 *  short of the live edge and then scrolled itself down. The teleport argument
 *  does not survive contact with what a riding reader is looking at, which is
 *  the newest content: this write keeps that content exactly where it is on
 *  screen and lets the growth appear above it, while it is the ANCHOR
 *  correction, and the pause before a tween, that move it. So the snap is the
 *  SMALLER motion of the two, and it is invisible rather than instant, because
 *  it lands before the paint the mutation causes.
 *
 *  Nothing at all for an unarmed reader: a toggle is not a request to be moved,
 *  and the anchor correction has already kept them on what they were reading. */
export function honourAnchoredMutation(el: HTMLElement): void {
  carryHeldScroll(el);
  if (!_followingBottom.value || isAtLiveEdge(el)) return;
  snapToLiveEdge(el);
}

/** Is the container still exactly where our last held write left it? The exact
 *  reading of "the reader has not taken over since", per the block above. 1px of
 *  slack absorbs a browser re-rounding a fractional position (zoom, device pixel
 *  ratio) and the iOS repaint nudge's deliberate ±1. */
function isWhereWeHeldIt(el: HTMLElement): boolean {
  return _heldEl === el && Math.abs(el.scrollTop - _heldTop) <= 1;
}

/* ── Was this scroll the reader's own GESTURE? ───────────────────────────────
 *  The position test above answers "did the container move away from our
 *  write", and the block over it states the premise that made that the same
 *  question as "did the reader take over": content growth changes
 *  `scrollHeight` and never `scrollTop`, so growth cannot look like a gesture.
 *
 *  That premise covers growth and nothing else, and three things move the
 *  container with no gesture behind them at all:
 *
 *  - The iOS soft keyboard. Opening or closing it rewrites `--app-height`
 *    (`MobileSwipeContainer`), the transcript's height changes with it, and
 *    WebKit adjusts the offset ASYNCHRONOUSLY through the ~350ms animation, so
 *    the correction lands well after any write of ours to stamp it against.
 *  - An app backgrounded and resumed. The PWA restores an offset nobody wrote.
 *  - Anything the platform decides to scroll on its own (a focus ring brought
 *    into view, a restored session).
 *
 *  Each of those retired a follow the reader had armed and never touched, while
 *  a reply was streaming, which is the whole value of the feature gone at the
 *  moment it is worth most. So the question is asked of the INPUT instead: a
 *  scroll may retire the follow only while a reader gesture is in flight. That
 *  is a direct signal where the position was an inference, which is what this
 *  module says it wants everywhere else.
 *
 *  The gesture term is added to the position one rather than replacing it: both
 *  must hold. A real scroll satisfies both by construction (a gesture moves the
 *  container), so nothing that used to retire the follow stops doing so, and
 *  the pairing keeps a stray tap during a growth write from ever standing in
 *  for a scroll.
 *
 *  A NAVIGATION is not a gesture and must say so itself, which is what the up
 *  chevron, turn stepping and the deep link do (see `stopFollowingBottom`). */

/** How long after the reader lifts off their scroll events still count as
 *  theirs: `USER_SCROLL_WINDOW_MS`, which `utils/scrollActivity.ts` already
 *  defines as "the drag plus the momentum tail" for the repaint nudge's own
 *  stand-down. The same question, so the same answer, shared rather than
 *  restated: it has to outlive `touchend` because iOS momentum fires its scroll
 *  events after the finger is gone, and a flick that only crosses the live-edge
 *  threshold during the coast would otherwise read as the platform's. Two
 *  constants that must agree and nothing saying so is exactly the drift the
 *  repo's glossary rule is about.
 *
 *  The EVENT SET is deliberately not shared. `utils/userAction.ts` owns "the
 *  user did something" for the surfaces that stand down on any interaction, and
 *  it includes `pointerdown`, which is right there and wrong here: a press is
 *  how the reader answers a question or grants a permission, and this signal
 *  must survive both. See `_scrollbarPressEl`. */
const READER_GESTURE_WINDOW_MS = USER_SCROLL_WINDOW_MS;

/** The element the last MOVEMENT was on, so a gesture in one pane cannot speak
 *  for the transcript in another. */
let _gestureEl: HTMLElement | null = null;
/** When that movement was. */
let _gestureAt = -Infinity;
/** The container whose SCROLLBAR is currently held, or null.
 *
 *  Only a scrollbar press goes in here, and that narrowness is the whole of the
 *  variable's job. A press on CONTENT is how the reader answers a question,
 *  grants a permission or expands a turn, all inside the transcript and all
 *  changing content; a mouse drag on content selects text and scrolls nothing.
 *  Recording those too would let the jitter every real click carries reach
 *  `pointermove` below and stamp a gesture, putting the window back over
 *  exactly the interactions this module documents as KEEPING the follow. With
 *  only the gutter recorded, that is unrepresentable rather than merely
 *  avoided.
 *
 *  What it buys is the length of a drag: the press itself stamps once, and a
 *  slow haul down the scrollbar can outlast the window, so `pointermove` keeps
 *  it fresh while the thumb is held. */
let _scrollbarPressEl: HTMLElement | null = null;

/** Record MOVEMENT: a wheel notch, a finger travelling, a drag, a scroll key.
 *  Never a bare press. */
function stampGesture(el: HTMLElement) {
  _gestureEl = el;
  _gestureAt = nowMs();
}

/** Did the reader move `el` themselves, counting the coast after a flick?
 *
 *  Freshness is the whole test, with no "still holding" term beside it. A drag
 *  fires movement continuously, so a real one keeps re-stamping for as long as
 *  it lasts, and a finger resting on the transcript scrolls nothing for the
 *  question to be asked about. A held-down term would also be a state that can
 *  STICK: a release the page never sees (a touch that ends while the PWA is
 *  backgrounded) would leave the reader's hand permanently on the transcript,
 *  and the first platform scroll after the resume would retire the follow,
 *  which is one of the three bugs this exists to fix. */
function readerGestureActive(el: HTMLElement): boolean {
  return _gestureEl === el && nowMs() - _gestureAt < READER_GESTURE_WINDOW_MS;
}

/** Keys that scroll a focused container. The transcript is `tabindex=0`, so it
 *  takes them directly; there is no other way for a keyboard reader to move it,
 *  which is why the list is closed rather than "any key". */
const SCROLL_KEYS = new Set([
  'ArrowUp', 'ArrowDown', 'PageUp', 'PageDown', 'Home', 'End', ' ', 'Spacebar',
]);

/** Wire the reader-gesture signal to a scroll container, returning its
 *  teardown. NOT exported and NOT called by the view: `makeScrollObservers`
 *  owns it, so the signal and the `onScroll` that consumes it are attached
 *  together by construction. Wiring them separately would make "observers
 *  attached, gestures forgotten" expressible, and that state is silent: the
 *  transcript behaves normally right up until a reader scrolls away from a live
 *  reply and is dragged back.
 *
 *  `pointermove` is the one that needs the press beside it. On a mouse it fires
 *  for the pointer merely CROSSING the transcript, which is most of a desktop
 *  session, so stamping it unconditionally would leave a gesture permanently
 *  in flight; gated on the press it means a drag, which is how a scrollbar is
 *  pulled. `touchmove` needs no such gate: a finger on the glass is already a
 *  press by definition.
 *
 *  The release goes on `window` rather than the element: a drag that ends with
 *  the pointer outside the transcript (a scrollbar drag flung past the pane
 *  edge) would otherwise leave the press recorded forever.
 *
 *  A container with no `addEventListener` is a test double, and gets a no-op
 *  teardown; such a test drives the signal through `readerGestureForTest`. So
 *  is a `window` without one, which is what the unit-test setup's minimal stub
 *  can be. */
function attachReaderGestures(el: HTMLElement): () => void {
  const root = typeof window !== 'undefined' && typeof window.addEventListener === 'function'
    ? window
    : null;
  if (typeof el.addEventListener !== 'function' || !root) return () => {};
  const onDown = (e: PointerEvent) => {
    // A press in the SCROLLBAR gutter is a scroll by itself, with no movement
    // to follow it: clicking the track pages the transcript in one jump. It is
    // also the ONLY press that can be a scroll at all, which is why it is the
    // only one recorded. The gutter is outside the client box, so it cannot be
    // a content control, and a mouse drag anywhere else in the transcript
    // selects text rather than scrolling.
    //
    // `e.target === el` is load-bearing, not a tidy-up. `offsetX` is measured
    // from the padding box of the TARGET, and `pointerdown` bubbles from the
    // deepest element under the pointer, so on a press that lands on a
    // descendant the comparison is against the wrong box entirely: in a turn
    // taller than the viewport, an ordinary tap in the body reads as past the
    // gutter and stamps a gesture, which is the bare-press case the whole
    // signal exists to refuse. Only a press on the container ITSELF can be in
    // its gutter.
    //
    // Horizontal only: `.thread-content` is `overflow-x: hidden`, so there is
    // no bottom scrollbar for an `offsetY` arm to ever be right about. Always
    // false where scrollbars overlay, which is every touch device.
    if (e.target !== el || e.offsetX <= el.clientWidth) return;
    _scrollbarPressEl = el;
    stampGesture(el);
  };
  const onDrag = (e: PointerEvent) => {
    // A move with no button held means the press ended somewhere this listener
    // never saw: released over a nested iframe, or while the PWA was
    // backgrounded. Clearing here is what stops `_scrollbarPressEl` becoming a
    // state that can STICK, where a mouse merely crossing the transcript would
    // then read as a drag.
    if (e.buttons === 0) { if (_scrollbarPressEl === el) _scrollbarPressEl = null; return; }
    if (_scrollbarPressEl === el) stampGesture(el);
  };
  // `wheel` and `touchmove` are stamped WHEREVER in the transcript they land,
  // and the asymmetry with the two gated listeners is the point rather than an
  // oversight. Those two can be ACTIVATIONS: a press is how a card is answered,
  // Space on a focused button is the same act from the keyboard, and neither
  // scrolls anything, so each is gated to the container itself. A wheel notch
  // and a finger travelling are scroll intent by definition, whatever they land
  // on. The worst they can be wrong about is a nested scroller (a step detail's
  // `<pre>`), where the reader is still scrolling, just not this box.
  const onMove = () => stampGesture(el);
  const onUp = () => { if (_scrollbarPressEl === el) _scrollbarPressEl = null; };
  const onKey = (e: KeyboardEvent) => {
    // Only a key the CONTAINER itself received scrolls it. `keydown` bubbles
    // like everything else here, and the transcript is full of controls that
    // take these very keys and scroll nothing: Space on a focused button is how
    // the reader ANSWERS a question card, Arrow and Home/End move a caret in a
    // text field. Each of those changes content, so stamping them would put a
    // window over the interactions the press rule above already refuses. The
    // container is `tabindex=0`, so a reader scrolling it by keyboard has focus
    // on it and is the target.
    if (e.target !== el) return;
    // A CHORD is a shortcut, not a scroll key. The two overlap: turn stepping
    // is Cmd+Arrow, and its own keystroke stamping a gesture would defeat the
    // one case `stepThreadTurn` deliberately keeps the ride for, a step onto
    // the last turn, by retiring it from `onScroll` mid-glide instead.
    if (e.metaKey || e.ctrlKey || e.altKey) return;
    if (SCROLL_KEYS.has(e.key)) stampGesture(el);
  };

  el.addEventListener('pointerdown', onDown as EventListener, { passive: true });
  el.addEventListener('pointermove', onDrag as EventListener, { passive: true });
  el.addEventListener('touchmove', onMove, { passive: true });
  el.addEventListener('wheel', onMove, { passive: true });
  el.addEventListener('keydown', onKey);
  root.addEventListener('pointerup', onUp, { passive: true });
  root.addEventListener('pointercancel', onUp, { passive: true });
  root.addEventListener('touchend', onUp, { passive: true });
  root.addEventListener('touchcancel', onUp, { passive: true });

  return () => {
    el.removeEventListener('pointerdown', onDown as EventListener);
    el.removeEventListener('pointermove', onDrag as EventListener);
    el.removeEventListener('touchmove', onMove);
    el.removeEventListener('wheel', onMove);
    el.removeEventListener('keydown', onKey);
    root.removeEventListener('pointerup', onUp);
    root.removeEventListener('pointercancel', onUp);
    root.removeEventListener('touchend', onUp);
    root.removeEventListener('touchcancel', onUp);
    if (_gestureEl === el) { _gestureEl = null; _gestureAt = -Infinity; }
    if (_scrollbarPressEl === el) _scrollbarPressEl = null;
  };
}

/** Test seam: record that the reader just MOVED `el`, or forget it.
 *
 *  The observer tests drive plain objects rather than DOM nodes, so there is no
 *  event to dispatch and no listener to dispatch it to. This is the same fact
 *  the listeners record, stated directly, which keeps "the reader scrolls" one
 *  line in a test instead of a fake event system. Production never calls it:
 *  there the fact comes from the input events, and it expires on its own.
 *
 *  `moved: false` forgets it, which is what a test asserting the PLATFORM moved
 *  the container (the keyboard, an app resume) needs, and what the shared reset
 *  in a `beforeEach` uses so one test's flick cannot carry into the next. */
export function readerGestureForTest(el: HTMLElement | null, moved = true): void {
  if (!el || !moved) {
    _gestureEl = null;
    _gestureAt = -Infinity;
    _scrollbarPressEl = null;
    return;
  }
  stampGesture(el);
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
 *  `LANDING_DEADLINE_MS`). The visibility test also rejects the hidden
 *  dual-mount copy, and is one call rather than a scan. */
function lastUserMessage(el: HTMLElement): HTMLElement | null {
  if (typeof el.querySelectorAll !== 'function') return null;
  const panels = el.querySelectorAll<HTMLElement>('.initiator-panel-user');
  const last = panels[panels.length - 1];
  return last && isElementVisible(last) ? last : null;
}

/** The transcript's newest turn: the LAST `.chat-exchange`. Continue's
 *  counterpart to `lastUserMessage`, and strictly the last one for the same
 *  reason (an invisible newest turn is "not there yet", never an older one). */
function lastTurn(el: HTMLElement): HTMLElement | null {
  if (typeof el.querySelectorAll !== 'function') return null;
  const turns = el.querySelectorAll<HTMLElement>(TURN_SELECTOR);
  const last = turns[turns.length - 1];
  return last && isElementVisible(last) ? last : null;
}

/** The turn holding the card whose `attr` is `value`, matched among the elements
 *  `bodySelector` picks out: the `.initiator-panel` around it, which is a card
 *  submit's counterpart to `lastUserMessage`'s panel. Two callers, the question
 *  card and the three permission-shaped cards, which differ only in the class
 *  the body wears and the attribute carrying its id.
 *
 *  The PANEL and not the body inside it, for two reasons that agree. It is the
 *  whole of what the reader produced (the question, their picks, the chrome
 *  around both) and is what the reply then grows underneath, exactly as it does
 *  under a sent message. And it is the part that SURVIVES being answered:
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
function cardTurn(el: HTMLElement, bodySelector: string, attr: string, value: string): HTMLElement | null {
  if (typeof el.querySelectorAll !== 'function') return null;
  for (const body of el.querySelectorAll<HTMLElement>(bodySelector)) {
    if (body.getAttribute?.(attr) !== value) continue;
    if (!isElementVisible(body)) continue;
    return (body.closest?.('.initiator-panel') as HTMLElement | null) ?? body;
  }
  return null;
}

/** The AGENT STATUS LINE of the turn `panel` belongs to: the `.response-header`
 *  of that turn's response panel, the row carrying the executor's name and the
 *  live Requesting / Working label (see `ResponsePanel` in
 *  `chat-exchange-parts`).
 *
 *  It is what a submit's landing actually aims at. The reader's own panel is
 *  where the landing used to stop, and stopping there answers the wrong
 *  question: they can already recall what they just wrote, and what they are
 *  looking for is whether the agent picked it up. One row lower is the whole
 *  difference between a transcript that says "sent" and one that says nothing.
 *
 *  Scoped through the enclosing `.chat-exchange`, never taken as the last header
 *  in the transcript, because a turn with NO response panel is ordinary rather
 *  than exotic: a queued follow-up carries its "Queued" tag in its own bubble and
 *  renders no panel at all. The last header would then belong to an earlier turn,
 *  ABOVE the reader's message, and the landing would go backwards.
 *
 *  Null while the response panel has not mounted yet and for a turn that never
 *  gets one; the landing falls back to the panel in both cases. Never a header
 *  with no box either: a collapsed pane or a detached node reports an all-zero
 *  rect, and subtracting the container's bottom edge from that would haul the
 *  transcript to its top, which is the trap `landOnOwnTurn`'s own guard covers
 *  for the panel. */
function turnStatusLine(panel: HTMLElement): HTMLElement | null {
  if (typeof panel.closest !== 'function') return null;
  const turn = panel.closest(TURN_SELECTOR) as HTMLElement | null;
  const header = turn?.querySelector?.('.response-header') as HTMLElement | null;
  return header && isElementVisible(header) ? header : null;
}

/** Room to leave UNDER a landing, read as the anchor's resolved
 *  `scroll-margin-bottom`.
 *
 *  The transcript's bottom edge is not clear space: the prompt dissolve
 *  (`.prompt-area::before`, panels/content.css) paints a background-coloured
 *  band over it, so a row rested flush against that edge is exactly as invisible
 *  as one below the fold. The status line names its own clearance in
 *  chat/response.css and this reads it, which is the mirror of
 *  `turnNavClearancePx` reading `scroll-margin-top` for the fade at the other
 *  end, and the same reason for keeping the number in CSS: the band's height is
 *  a token, and a literal here would drift from it.
 *
 *  Zero for an anchor that declares none (every anchor but the status line: the
 *  reader's own message panel keeps landing flush, as it always has) and zero
 *  where there is no layout to ask. */
function landingClearancePx(anchor: HTMLElement): number {
  if (typeof getComputedStyle !== 'function') return 0;
  const px = parseFloat(getComputedStyle(anchor).scrollMarginBottom);
  return Number.isFinite(px) && px > 0 ? px : 0;
}

/** Put the container ON the live edge in one held write, and reconcile the
 *  chevron behind it: the write can land where the container already was, and
 *  then no scroll event arrives to do that reconcile itself.
 *
 *  The instant half of `glideToLiveEdge`, factored out because two callers want
 *  exactly it and neither may drift from the other on what the live edge is or
 *  on the bookkeeping a write there owes: the reduced-motion branch below, and
 *  `honourAnchoredMutation`, where landing inside the frame IS the point.
 *
 *  It supersedes EVERY tween, including a live-edge one that `glideToLiveEdge`
 *  would have stood down for. That stand-down exists so a second call cannot
 *  restart the easing part-way, and there is no easing here: a reader who
 *  toggles the steps 200ms into a submit's glide lands on the same live edge,
 *  just at once, which is what they asked for by toggling. */
function snapToLiveEdge(el: HTMLElement): void {
  cancelScrollAnim();
  markHeldScroll(el, liveEdgeTop(el));
  syncAwayFromBottom();
}

/** Glide to the LIVE EDGE, marking every frame as a held scroll: what a submit
 *  does for a reader who was ALREADY riding it (see `followSubmit`). The target
 *  is `scrollHeight - clientHeight` re-read per frame, so a reply streaming
 *  during the glide is tracked and the tween lands on the bottom the transcript
 *  has when it ENDS rather than the one it had when the reader pressed the
 *  button.
 *
 *  Stands down for one tween only: a live-edge glide already in flight, which is
 *  the composer's second call for one send finding the first call's glide.
 *  Re-targeting that would only restart the easing from wherever it had got to.
 *  Every other tween is superseded, a deep-link's and a chevron's included: a
 *  submit is the reader saying otherwise, as it is for every other navigation.
 *
 *  A LANDING glide cannot be the tween it finds. Reaching this branch means the
 *  follow is armed, and the only thing that arms it cancels whatever tween is in
 *  flight on its way (`scrollToBottom`, and `scrollToBottomAnimated` through
 *  `animateScroll`). The supersede-a-landing case the earlier rule described was
 *  reachable only while a send armed the follow itself. */
function glideToLiveEdge(el: HTMLElement): void {
  if (_heldAnim && _heldAnimTarget === 'live-edge') return;
  if (prefersReducedMotion()) {
    snapToLiveEdge(el);
    return;
  }
  // Reconcile the chevron on landing for the same reason `scrollToBottomAnimated`
  // does: the last frame can write where the previous one already left the
  // container, and then no scroll event arrives to do it.
  animateScroll(liveEdgeTop, syncAwayFromBottom, markHeldScroll);
  _heldAnimTarget = 'live-edge';
}

/** THE SUBMIT REACTION. One function, because "same reaction everywhere" is a
 *  structural claim rather than four copies kept in step by hand: a submit is
 *  any user action in the transcript the agent is expected to respond to, and
 *  the four of them (a sent message, an answered question card, a decided
 *  permission card, Continue after an abort) differ only in `resolveTurn`, which
 *  finds the turn the reader acted on.
 *
 *  IT ARMS NOTHING. Whatever it does here is over when it is done; riding the
 *  live edge is the down chevron's request alone. A send and an answer used to
 *  arm, and the reader was then dragged through the whole reply by a request
 *  they never made.
 *
 *  It is NOT a blind jump to the bottom, and splits TWO ways:
 *
 *   - **Already riding the live edge**: glide to the live edge. Riding it is a
 *     STANDING request to be kept at the bottom, so a submit made while it is
 *     armed asks for the bottom and not for a look at the turn they acted on.
 *   - **Everyone else**: glide to the turn's AGENT STATUS LINE, landing that row
 *     on the bottom of the viewport, so they see the agent take what they
 *     submitted with the thing they submitted sitting just above it and the
 *     answer growing in underneath. The status line and not the turn's own bottom
 *     edge, because "did it go through" is the question a submit asks and the
 *     Requesting / Working row is where it is answered. Anchored on an ELEMENT
 *     either way, never computed from `scrollHeight`: the transcript grows
 *     between the submit and the landing and a `scrollHeight` target then lands
 *     PAST the turn and hides the very thing they acted on. See `landOnOwnTurn`
 *     and `turnStatusLine`.
 *
 *  TWO, not four. It shipped with two more branches that declined to move
 *  anybody, and both were the same mistake: they asked a question about the
 *  transcript AS IT STANDS in order to predict where a turn that has not
 *  rendered yet will sit.
 *
 *   - **At the live edge wrote nothing**, on the reasoning that the reader is
 *     already looking at the newest content. They are, and a beat later the
 *     submit APPENDS a turn whose status line is below the fold, so the reader
 *     who sent from the bottom (the ordinary case) was left on the top of their
 *     own message with nothing to take them down. Reported twice on 2026-08-10,
 *     as "does not land on the first response line" and "answering with custom
 *     text does not scroll at all".
 *   - **Nowhere to take anybody** (`hasSomewhereToLand`: scrollable, and holding
 *     at least one turn) wrote nothing for the same shape of reason, and got the
 *     brand-new thread wrong in the same way: it holds no `.chat-exchange` at
 *     submit time, and its first turn's status line can still land below the fold
 *     on a short viewport.
 *
 *  What both were reaching for survives as ONE test, in the right place and
 *  against the right thing: `landOnOwnTurn` writes nothing when its target is at
 *  or behind the current position. That is physics rather than policy, since the
 *  status line is already in view and there is nowhere to go, and it subsumes
 *  both: a thread too short to scroll cannot produce a target ahead of the
 *  reader, and neither can one whose status line is already resting above the
 *  bottom edge. Asked per frame, against the real anchor, after the turn exists.
 *
 *  The landing takes a POSITION STAMP before it does anything, because the two
 *  deferred submits move nobody for a frame or more and the reader's flick in
 *  that window must still cancel them (see `holdPosition`). It used to come free
 *  from the follow's disarm, which no longer fires for a submit that arms
 *  nothing.
 *
 *  A landing already PENDING is kept rather than replaced, which is what makes
 *  the composer's two calls for one send one submit: `PromptInput`'s `submit`
 *  and `store/actions/chat.ts`'s `addPendingMessage` both fire, in one
 *  synchronous task, and a second resolver built after the optimistic row
 *  rendered would wait for a message that will never come. */
function followSubmit(resolveTurn: TurnResolver): void {
  // A submit CLAIMS the thread is live, whatever the last turn's status says
  // yet. That is what a submit IS: an act the agent is expected to respond to.
  // The status cannot say so for a while, and the gap is not small: answering a
  // card leaves the turn on `awaiting-answer`, which is not an ACTIVE status,
  // until the engine's resumed status arrives over SSE a round trip later. A
  // reader who scrolls away inside that window means "stop dragging me" exactly
  // as much as one who scrolls away mid-reply, and reading it as idle browsing
  // would keep their follow armed and haul them back the moment the reply
  // resumed. A CLAIM rather than a fact, so it expires on its own when the
  // response never comes: see `_submitLiveUntil`. `ChatExchange` supersedes it
  // the instant the real status is known.
  _submitLiveUntil = nowMs() + SUBMIT_LIVE_CLAIM_MS;
  const el = resolveTarget();
  if (!el) return;
  if (_followingBottom.value) {
    // The one place an at-the-live-edge test is still legitimate, and the
    // difference from the branch below is the whole reason: this branch's target
    // is the live edge itself, which is exactly where the growth is about to
    // take them anyway, so a rider already on it needs no write and gets no
    // redundant tween (on iOS that would cancel a momentum scroll). The LANDING
    // cannot ask the same question, because its target is a status line that does
    // not exist yet and will render BELOW where the reader is standing.
    if (!isAtLiveEdge(el)) glideToLiveEdge(el);
    return;
  }
  // A landing already PENDING keeps the floor, unless it has LAPSED. The two
  // checks are the same rule from opposite sides: the growth branch expires it
  // when a round happens to run, and this expires it when one never does. A
  // queued follow-up is exactly that case, and it is the common one now that the
  // landing waits for a status line the queued turn will never render: the turn
  // resolves, nothing else grows the transcript, and the stale pending landing
  // sat on `_pendingLanding` forever. The reader's NEXT submit then returned on
  // the line below without installing its own resolver, so it never landed.
  // Found by the Codex reviewer, 2026-08-10.
  dropLapsedLanding();
  if (_pendingLanding) return;
  holdPosition(el);
  // The turn AND its status line, or the landing waits. Resolving on the panel
  // alone is what put the reader on the end of their own message: the response
  // panel can mount a commit or two after the row, and `landOnOwnTurn` asks its
  // do-not-scroll-backwards test once, at the moment it is called. Asked against
  // the panel it either declines (nothing happens) or aims one row short and
  // settles there if the header arrives after the tween has finished. Waiting
  // costs nothing the deferral did not already cost, and hands the deadline the
  // case it was written for: a turn that never gets a response panel at all.
  const panel = resolveTurn(el);
  if (panel && turnStatusLine(panel)) { landOnOwnTurn(el, panel); return; }
  _pendingLanding = { resolveTurn, at: nowMs() };
}

/** A landing that waits for a turn the submit is about to CREATE: it snapshots
 *  what `newest` answers NOW and resolves only once that answer changes. The two
 *  deferred submits both have this shape and differ only in what "newest" means
 *  (the reader's own message row for a send, the last turn for Continue). */
function awaitsNewTurn(newest: (el: HTMLElement) => HTMLElement | null): TurnResolver {
  const el = resolveTarget();
  const before = el ? newest(el) : null;
  return (c) => {
    const now = newest(c);
    return now && now !== before ? now : null;
  };
}

/** The reader sent a message. Its turn is the optimistic row the send inserts,
 *  which arrives a frame or more later while the composer collapsing has already
 *  fired a resize, so the landing is deferred until a DIFFERENT last user
 *  message is present.
 *
 *  Called by the two send sites, `store/actions/chat.ts`'s `addPendingMessage`
 *  and `PromptInput`'s `submit`; see `followSubmit` on why two calls are one
 *  submit. */
export function followSentMessage(): void {
  followSubmit(awaitsNewTurn(lastUserMessage));
}

/** The reader submitted an answer to the question card `toolUseId`. Its turn is
 *  the `.initiator-panel` around that card, which is on screen already (it is
 *  what they just tapped), so this landing needs none of the send's deferral: it
 *  resolves now and glides.
 *
 *  Called by the two card-submitted answers: `QuestionCard`'s single-select
 *  option tap and `PromptInput`'s multi-select Submit. The THIRD way to answer,
 *  typing into the composer, is a send that the engine reroutes as a `FreeText`
 *  answer, so it arrives through `followSentMessage` and needs nothing here. */
export function followAnsweredQuestion(toolUseId: string): void {
  followSubmit((el) => cardTurn(el, '.question-body', 'data-tool-use-id', toolUseId));
}

/** The reader decided the permission card `requestId` (Deny / Allow once / Allow
 *  for this thread / Always allow, on any of the three permission-shaped cards:
 *  the coding-agent tool permission, the command guard and the MCP tool consent,
 *  all of which decide through `PermissionCard`'s `usePermissionDecide` and so
 *  all inherit this from one call site).
 *
 *  A submit like the others. It used to be "the one submit that ARMS NOTHING",
 *  on the reasoning that answering a gate the agent put in its own way is not
 *  producing content: it moved a rider to the live edge and everyone else zero
 *  pixels, so deciding a card resumed the agent below the fold with nothing on
 *  screen saying so. That distinction is gone, because from the reader's side
 *  all four submits are the same act, and this one now lands like the rest.
 *
 *  Its turn is the `.initiator-panel` around the card, which is on screen
 *  already, so like the answer it resolves now and glides. */
export function followResolvedPermission(requestId: string): void {
  followSubmit((el) => cardTurn(el, '.permission-body', 'data-request-id', requestId));
}

/** The reader pressed Continue on an aborted turn, asking the agent to pick the
 *  thread back up.
 *
 *  Its turn does NOT exist when the button is pressed: the continuation renders
 *  as a fresh `ContinuationStarted` exchange (`exchange-grouping.ts`), which
 *  arrives over SSE after the POST. So this is the send's deferred landing with
 *  a different notion of "the turn I am waiting for": a `.chat-exchange` that is
 *  not the one that was last when they pressed, rather than a new user message
 *  (a continuation renders none).
 *
 *  Called by `ContinueButton` on tap, BEFORE the awaited POST, because it is the
 *  button's own tap and must not wait on the round trip. */
export function followContinuedThread(): void {
  followSubmit(awaitsNewTurn(lastTurn));
}

/** Glide so the turn's AGENT STATUS LINE rests on the container's bottom edge,
 *  marking every frame as a held scroll. `panel` is the turn the reader acted
 *  on: their optimistic message row for a send, the answered card's initiator
 *  panel for an answer, the decided card's for a permission, the whole
 *  continuation exchange for Continue. The line is `turnStatusLine(panel)`, and
 *  the panel itself is the fallback for a turn that has no response panel to
 *  carry one.
 *
 *  Landing the STATUS LINE rather than the panel is the point of the whole
 *  glide. A reader who submits from up the transcript is asking "did that go
 *  through, and is it working on it", and the answer is the Requesting / Working
 *  row under what they submitted, not that thing itself, which they produced a
 *  second ago and can see the top of anyway.
 *
 *  It is emphatically NOT the response panel's bottom, which would be the
 *  live-edge chase this landing exists instead of: the panel grows with every
 *  token, so anchoring there would drag the reader down through an answer they
 *  have not read and hide the thing they just submitted, which is exactly what
 *  `followSubmit`'s fourth bullet forbids. The status line is a fixed row at the
 *  TOP of that growth, so it holds still while the reply arrives beneath it.
 *
 *  It rests ABOVE the container's bottom edge by whatever clearance the anchor
 *  asks for (`landingClearancePx`), because that edge is under the prompt
 *  dissolve and a row landed flush against it is painted over.
 *
 *  The target is re-read per frame by `animateScroll`, so a reply streaming
 *  under it during the glide is tracked rather than overshot. Reduced motion
 *  writes it once, as everywhere else in this module. A target at or behind the
 *  current position writes nothing: the turn is already fully in view through
 *  its status line, and a submit has no business scrolling the reader backwards
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
  /** The row the glide rests on the container's bottom edge, and the room to
   *  leave under it: the status line once it is in the DOM, the reader's own
   *  panel until then and for a turn that never gets one.
   *
   *  Re-asked each frame only while the answer is still the panel, because the
   *  response panel can mount a frame or two after the row the landing was
   *  scheduled on, and a target fixed at call time would stop one row short of
   *  the thing the reader is waiting for. Held once found, so a settled glide
   *  costs the one rect read per frame every other tween does rather than a
   *  `closest` + a visibility walk, and it is what keeps the CLEARANCE to a
   *  single `getComputedStyle` instead of one per frame. Dropped again if the
   *  node ever leaves the layout. */
  let anchor: HTMLElement = panel;
  let clearance = landingClearancePx(panel);
  const anchorOf = (): HTMLElement => {
    if (anchor !== panel && anchor.isConnected !== false) return anchor;
    // Only on a CHANGE of anchor, which is what keeps the fallback cheap: a turn
    // that never gets a status line re-asks for one every frame and must not pay
    // a `getComputedStyle` for the same panel each time it hears no.
    const next = turnStatusLine(panel) ?? panel;
    if (next !== anchor) {
      anchor = next;
      clearance = landingClearancePx(next);
    }
    return anchor;
  };
  const targetOf = (c: HTMLElement) => {
    const on = anchorOf();
    if (panel.isConnected === false
      || typeof c.getBoundingClientRect !== 'function'
      || typeof on.getBoundingClientRect !== 'function') {
      return lastTarget >= 0 ? lastTarget : c.scrollTop;
    }
    lastTarget = Math.max(0, c.scrollTop + clearance + (on.getBoundingClientRect().bottom - c.getBoundingClientRect().bottom));
    return lastTarget;
  };
  if (targetOf(el) <= el.scrollTop) return;
  if (prefersReducedMotion()) {
    cancelScrollAnim();
    markHeldScroll(el, targetOf(el));
    return;
  }
  animateScroll(targetOf, undefined, markHeldScroll);
  // Say which of the two held glides this is, so a submit arriving mid-flight
  // knows to supersede it rather than let it finish on a turn the reader has
  // since submitted past, and so the reader's own scroll can cancel it. See
  // `_heldAnimTarget`.
  _heldAnimTarget = 'own-turn';
}

/** What one growth round owes the reader, which is at most one of two things: a
 *  submit is waiting for its own turn to render (land on it, once, on the status
 *  line under it), or the reader is riding the live edge (write it). Never both,
 *  because a submit arms nothing and arming drops a pending landing
 *  (`armFollowOn`), and nothing at all for a reader who asked for neither.
 *
 *  Stands down while a navigation tween owns the scroll, including the landing
 *  glide itself: a tween re-reads its own target every frame, so a live-edge
 *  write beside it would drag the glide past the turn it is landing on. */
function honourGrowth(el: HTMLElement): void {
  if (_scrollAnimRaf !== null) return;
  if (_pendingLanding) {
    // Both, for the reason `followSubmit` gives: the status line is the anchor,
    // so a landing started before it exists asks its own guard the wrong
    // question.
    const panel = _pendingLanding.resolveTurn(el);
    if (panel && turnStatusLine(panel)) {
      _pendingLanding = null;
      landOnOwnTurn(el, panel);
      return;
    }
    // The turn the submit is waiting for is not there yet: there is nothing to
    // land on, and nowhere to jump meanwhile. Past the deadline it is not late,
    // it is unaddressable, so the landing LAPSES and moves nobody (see
    // `LANDING_DEADLINE_MS`).
    dropLapsedLanding();
    return;
  }
  if (_followingBottom.value) markHeldScroll(el, liveEdgeTop(el));
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
 *    `markNavigationScroll`, which is right for a one-off navigation; the two
 *    held glides pass `markHeldScroll` so their own frames are not read as the
 *    reader taking over. */
function animateScroll(
  targetOf: (el: HTMLElement) => number,
  onDone?: () => void,
  mark: (el: HTMLElement, top: number) => void = markNavigationScroll,
) {
  cancelScrollAnim();
  _heldAnim = mark === markHeldScroll;
  let started = false;
  let start = 0;
  let startTime = 0;
  let duration = SCROLL_MIN_MS;
  const step = (now: number) => {
    const cur = resolveTarget();
    if (!cur) { endScrollAnim(); return; }
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
      endScrollAnim();
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

  // Going to the top ends the ride, and the press has to say so ITSELF: a
  // scroll only speaks for the reader when a gesture is behind it, and a
  // chevron tap lands on the button rather than on the transcript (see "Was
  // this scroll the reader's own GESTURE?"). It used to come free from the
  // position test, which read this write as a flick.
  //
  // Only while the agent is LIVE, matching the scroll disarm: going back to
  // re-read an idle thread is browsing, not a decision about how the next reply
  // should behave, and the lit toggle says on screen that the ride survived.
  //
  // After the target resolves, so a press with no transcript to move retires
  // nothing.
  if (threadIsLive()) stopFollowingBottom();

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
 *  action, and NOTHING more than that.
 *
 *  Eases to the bottom, re-reading the target every frame so a thread that keeps
 *  streaming during the glide is tracked and the tween lands on the TRUE grown
 *  bottom rather than on the bottom as it was when tapped.
 *
 *  It used to ARM the standing follow on landing, and that is gone: the chevron
 *  is a navigation like the up chevron and turn stepping, and riding the live
 *  edge is the follow toggle's request. Overloading the one button was tried and
 *  does not work, because go-to-bottom, arm and disarm is a three-step cycle
 *  with no state left over for a plain jump to the bottom (see "The standing
 *  request to ride the live edge").
 *
 *  Reduced motion skips straight to scrollToBottom()'s instant jump. */
export function scrollToBottomAnimated() {
  clearPendingEventScroll();
  const el = resolveTarget();
  if (!el || prefersReducedMotion()) { scrollToBottom(); return; }
  // `liveEdgeTop` (the MAX scroll position), not scrollHeight, so the ease lands
  // exactly at the bottom instead of clamping flat for the last clientHeight px.
  animateScroll(
    liveEdgeTop,
    // The landing write may not move the container at all (the tween's last
    // frame can already be there), and then no scroll event fires to reconcile
    // the chevron. Settle it here against the real position instead.
    syncAwayFromBottom,
  );
}

/** Set the standing follow, which is the FOLLOW TOGGLE's whole behaviour and the
 *  only way to arm one (`resumeFollowingBottom` aside, which replays a request
 *  recorded in this thread and can therefore only ever replay one of these).
 *
 *  ON glides to the live edge and arms, which is the whole of what the chevron
 *  used to do, so a reader anywhere in the transcript is one tap from following.
 *  OFF disarms and writes NO SCROLL: turning a mode off is not a request to be
 *  moved, and a reader who stops following almost always wants to stay where
 *  they are reading.
 *
 *  Turning it off is a CONVENIENCE rather than the mechanism. The reader's own
 *  scroll already retires the follow (see the disarm in `onScroll`) and the
 *  button follows it off, because both render this one signal. */
export function setFollowLiveEdge(on: boolean): void {
  // Remember the press first, and on BOTH edges: turning the mode off is as much
  // a standing choice as turning it on, and the off edge returns early below.
  recordFollowSeed(on);
  if (!on) { stopFollowingBottom(); return; }
  clearPendingEventScroll();
  const el = resolveTarget();
  if (!el) return;
  armFollowOn(el);
  if (isAtLiveEdge(el)) { syncAwayFromBottom(); return; }
  glideToLiveEdge(el);
}

/** Jump the transcript to the bottom in one write: the reduced-motion form of
 *  the down chevron, and the compose view's chevron (which has no windowed
 *  render to glide through). Arms nothing, exactly as the animated form no
 *  longer does.
 *
 *  An EXPLICIT gesture, and the only kind left, so it supersedes any in-flight
 *  notification deep-link claim: the deep-link owns the viewport until it
 *  settles, and this is the user saying otherwise. Nothing in the app calls this
 *  on the user's behalf any more, so there is no longer an `auto` variant that
 *  had to defer to the claim instead. A submit does not call it either: a submit
 *  lands on the turn it was made on (see `followSubmit`). */
export function scrollToBottom() {
  clearPendingEventScroll();
  // Cancel any in-flight navigation so a down-tap right after an up-tap isn't
  // dragged back toward the top.
  cancelScrollAnim();

  const target = resolveTarget();
  if (target) markNavigationScroll(target, target.scrollHeight);
  syncAwayFromBottom();
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

/** Subscribers notified when a deep-link TAKES the claim, and never when it
 *  releases one. Same shape and same asymmetry as `onFollowArmed`: the claim is
 *  the request, and a release is not a second request anyone needs to hear.
 *
 *  One consumer: `attachScrollMemory`, which retires a saved-position restore
 *  that was already armed when the claim arrived. Asking `hasPendingEventScroll`
 *  once at attach cannot answer for a claim taken LATER, and two orderings that
 *  reach exactly that are ordinary rather than exotic: a deep-link into the
 *  thread the reader is already in re-attaches nothing at all, and a thread
 *  whose events arrive while the tap is still resolving attaches before the
 *  claim. Nor can the restore simply re-ask, because the claim is released
 *  within a second of a synchronous landing while the restore stays armed for
 *  three: the ask has to be delivered when it is made. */
const _deepLinkClaimListeners = new Set<() => void>();

/** Subscribe to the claim. Returns the unsubscribe. Fires on every claim,
 *  including one taken while another is live: a second notification tapped
 *  mid-flight is a second navigation request, not a continuation of the first
 *  (the same reason the claim is an OBJECT rather than its target). */
export function onDeepLinkClaimed(listener: () => void): () => void {
  _deepLinkClaimListeners.add(listener);
  return () => { _deepLinkClaimListeners.delete(listener); };
}

/** Subscribers notified when a deep-link FINDS its target, which is the moment
 *  it stops being a link that might be dead. The other half of the pair above,
 *  and the two together are the whole of what a listener needs to tell a link
 *  that landed from one that never did.
 *
 *  One consumer: `attachScrollMemory`'s dead-link rescue, which stood down for
 *  the claim and would otherwise have to INFER the landing from the container
 *  having moved. That inference is wrong whenever the landing had nowhere to
 *  move, and the case is ordinary rather than exotic: arriving in a shorter
 *  thread clamps the shared container to its bottom, and a deep-link to that
 *  thread's last turn resolves to the same offset. Inferred, the rescue then
 *  reads a successful landing as a dead link and hauls the reader off the event
 *  they are looking at, seconds after they got there, which is the exact
 *  complaint this whole change exists to fix. */
const _deepLinkResolvedListeners = new Set<() => void>();

/** Subscribe to the resolve. Returns the unsubscribe. */
export function onDeepLinkResolved(listener: () => void): () => void {
  _deepLinkResolvedListeners.add(listener);
  return () => { _deepLinkResolvedListeners.delete(listener); };
}

/** Whether the claim currently held has already found its target. Reset with
 *  each new claim, so it always describes the live one.
 *
 *  The broadcast above cannot answer this, and the gap it leaves is the ORDINARY
 *  ordering rather than an edge case: for a tap into a thread the reader is not
 *  in, the target renders and resolves on the microtask checkpoint of the commit
 *  that rendered it, while Preact defers the effect that would subscribe past
 *  that checkpoint. The listener does not exist yet when the resolve fires, and
 *  `resolved` latches, so no later broadcast ever follows. A subscriber that
 *  arrives mid-flight has to be able to ASK what it missed, which is the mirror
 *  of why the claim itself is delivered rather than re-read.
 *
 *  False whenever no claim is held, so it can never describe a link that is
 *  already over. */
let _pendingEventScrollResolved = false;

export function deepLinkHasResolved(): boolean {
  return _pendingEventScrollClaim !== null && _pendingEventScrollResolved;
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
  _pendingEventScrollResolved = false;
  // Announce the claim to anything holding a positioning decision of its own,
  // which is `useScrollMemory`'s saved-position restore (see
  // `onDeepLinkClaimed`). Notified AFTER the slot is set, so a listener asking
  // `hasPendingEventScroll` sees this claim rather than the state before it.
  for (const listener of _deepLinkClaimListeners) listener();
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
      _pendingEventScrollResolved = false;
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
    // Going to a link ends any ride: the reader asked to be at THIS place, and
    // the standing follow would carry them off it on the next token. Retired
    // before the scroll below rather than after, so the landing's own frames are
    // marked as a plain navigation and the reading position recorded for this
    // thread is the offset the link landed on rather than the live edge.
    //
    // Ungated, and on the line above the scroll on purpose: the two describe one
    // landing and must not disagree. A superseded call still lands (the pin, the
    // scroll and the pulse below are all ungated, unlike the resolve broadcast),
    // so gating only this would leave a reader who was just moved still
    // following.
    stopFollowingBottom();
    smoothScrollToElement(target);
    // Record and announce the landing, AFTER the scroll above rather than
    // before it. Two things read this, and the second is why the order matters:
    // one wants to know the link is no longer a candidate for the dead-link
    // rescue, and the other records WHERE it landed as the thread's reading
    // position. Under reduced motion the line above IS the whole landing, one
    // synchronous write, so announcing first would hand that recorder the
    // position the reader was leaving instead of the one they arrived at. An
    // animated landing has only scheduled its first frame by now, and its own
    // frames correct the record as they run.
    //
    // Only while the claim is still OURS. A superseded call keeps observing
    // until its own deadline, and letting its late resolve speak for the claim
    // a newer link now holds is exactly the collision the claim is an object to
    // prevent.
    if (_pendingEventScrollClaim === claim) {
      _pendingEventScrollResolved = true;
      for (const listener of _deepLinkResolvedListeners) listener();
    }
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
  _pendingEventScrollResolved = false;
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
  // landing target at/beyond the live edge, so the browser clamps the scroll to the
  // bottom — and when we're ALREADY at the bottom the clamped write doesn't move the
  // container, so no scroll event fires and onScroll never reconciles the chevron.
  // Hardcoding awayFromBottom=true there left the down chevron stuck on ("appears
  // the second time you click down arrow"). (2px slack mirrors isVisuallyAtBottom.)
  awayFromBottom.value = targetOf(el) < liveEdgeTop(el) - 2;

  // A deliberate jump AWAY from the live edge ends the ride, and it has to say
  // so itself: a scroll only speaks for the reader when a gesture is behind it,
  // and a keyboard chord is not one (see "Was this scroll the reader's own
  // GESTURE?"). This used to come free from the position test, which read the
  // jump's unstamped write as a flick.
  //
  // Reuses the line above rather than asking again, so the chevron's state and
  // the ride's cannot disagree: a step onto the LAST turn lands at the clamped
  // live edge, which is where the ride was taking them anyway. And only while
  // the agent is LIVE, matching the scroll disarm: stepping back through an
  // idle thread is browsing, and the reader's next submit should still carry
  // them.
  if (awayFromBottom.value && threadIsLive()) stopFollowingBottom();

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
 *  element, and wire the reader-gesture signal `onScroll` reads. The visibility
 *  gate at the top of each handler is required by the dual-mounting contract
 *  documented at the top of this file.
 *
 *  `detachGestures` is the caller's ONE teardown obligation, and it exists
 *  because the gesture listeners are the only thing here that touches the DOM:
 *  the caller attaches `onScroll`/`onResize` itself and removes them itself,
 *  but it never sees these. Call it wherever the `scroll` listener is removed. */
export function makeScrollObservers(el: HTMLElement) {
  const detachGestures = attachReaderGestures(el);
  function isAtTop() {
    return el.scrollTop <= 80;
  }
  // Tighter "at the very top" check than isAtTop's 80px chevron window — the
  // title fade should ease in as soon as content slides under the bar. 2px
  // slack absorbs subpixel rounding / iOS overscroll bounce at the top.
  function isVisuallyAtTop() {
    return el.scrollTop <= 2;
  }
  function syncNotAtTop() {
    notAtTop.value = isScrollable(el) && !isAtTop();
  }
  function syncScrolledFromTop() {
    scrolledFromTop.value = isScrollable(el) && !isVisuallyAtTop();
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
    // The app holding the reader on the same content, not the reader taking
    // over. The growth branch usually re-stamps a line later, but not when it
    // stands down for a tween or a pending landing, which is exactly when a pane
    // resize retired a follow nobody retired. See `carryHeldScroll`.
    carryHeldScroll(el);
  }

  // Scroll events. Whoever moved the container (a gesture, a chevron, a restore,
  // the reflow correction), the answer is the same: reconcile the three position
  // signals against where it now sits, and re-take the reflow anchor. There is
  // no longer anything to suppress, because nothing infers intent from a scroll.
  //
  // Two questions ARE asked of the scroll, and this is the only place a standing
  // follow is retired or a landing cancelled from one: has the reader taken the
  // container away from where our own last held write put it (see
  // `markHeldScroll`), and was a reader GESTURE behind it (see "Was this scroll
  // the reader's own GESTURE?"). The landing asks only the first, deliberately.
  //
  // The FOLLOW takes FOUR terms. Off the live edge alone is not enough, because
  // a shrink clamps the reader down and the app's own anchor correction moves
  // them while holding them on the same content; moved alone is not enough
  // either, because a tween mid-glide is our own. They are false for everything
  // that changes content without moving the reader off the edge, which is why
  // answering a question, granting a permission or expanding a turn all keep
  // the follow.
  //
  // The two of them together were the whole test until 2026-08-11, on the
  // premise that a container that moved away from our write had been moved by
  // the reader. That premise covers CONTENT growth and nothing else, so the
  // keyboard, an app resume and every other platform-driven scroll read as the
  // reader and retired a follow nobody touched (see "Was this scroll the
  // reader's own GESTURE?"). `readerGestureActive` is that missing term, ANDed
  // rather than swapped in: a real scroll satisfies all of them by
  // construction, so nothing that used to retire the follow stops doing so.
  //
  // A NAVIGATION is not a gesture and now retires the follow itself, which is
  // what the up chevron, turn stepping and the deep link do (see
  // `stopFollowingBottom`). Before the gesture term they came free from the
  // position test, and that was always an accident: their write lands where we
  // did not stamp for the same reason a flick does, not because the reader's
  // hand was on the transcript.
  //
  // The third is whether the agent is LIVE (`_threadLive`), and it separates two
  // acts that produce an identical scroll event. Scrolling away from a reply IN
  // FLIGHT means "stop dragging me". Scrolling on an IDLE thread is browsing:
  // nothing is moving, nothing is dragging anybody, and going back to re-read a
  // turn before writing the next message is not a decision about how the next
  // reply should behave. It silently was one until this term existed, and the
  // reader paid for it at their next submit.
  //
  // It applies to a NAVIGATION as much as to a gesture, which is a real
  // consequence rather than an oversight: the up chevron and ⌘↑ turn stepping
  // retire the follow only while the agent is live, so on an idle thread the
  // reader can step back through turns and still have their next submit take
  // them to the live edge. The lit toggle is what makes that fair, since it says
  // on screen that they are still riding. A DEEP LINK is the exception and
  // retires the follow whatever the thread is doing, by calling
  // `stopFollowingBottom` itself: a link is a request to be at ONE place, and
  // nothing may carry the reader off it.
  //
  // The LANDING takes only the moved term, and both differences are deliberate.
  // A reader who flicks down to the live edge mid-glide has gone exactly where
  // the landing is not taking them, and the tween would haul them back up on the
  // next frame; the clamp the follow's edge term guards against cannot strand
  // this one, because a glide re-writes and re-stamps the position every frame.
  // And a landing is answering a submit made a moment ago, so whether the agent
  // has got going yet says nothing about whether the reader wants it.
  function onScroll() {
    if (!isElementVisible(el)) return;
    syncNotAtTop();
    syncScrolledFromTop();
    const atEdge = isAtLiveEdge(el);
    awayFromBottom.value = !atEdge;
    const tookOver = !isWhereWeHeldIt(el);
    if (_followingBottom.value) {
      if (threadIsLive() && !atEdge && tookOver && readerGestureActive(el)) stopFollowingBottom();
    } else if (tookOver && landingInFlight()) {
      cancelLanding();
    }
    // The reader has moved, so the reflow anchor has to follow them.
    recordAnchor();
  }
  // Resize events. A resize moves the reader for exactly two reasons, and both
  // are things they asked for: they ASKED to ride the live edge and have not
  // taken it back, or they just SUBMITTED and the turn their landing is waiting
  // for has finally rendered (see "The standing request to ride the live edge",
  // and `followSubmit`). For everyone else it moves nobody, whatever grew and
  // however far off the bottom it leaves them: a streaming reply, a decoded
  // image, an expanded step, a growing composer all leave the transcript exactly
  // where it is, and the handler's remaining job is to reconcile the signals so
  // the chevrons describe the new geometry.
  //
  // This is where the old force-pin lived, and the two rules that fought over
  // it: a 'scroll'-mode branch that slammed `scrollTop = scrollHeight` on every
  // resize inside a 500ms window, and a follow branch that re-pinned any reader
  // inside the 80px stickiness window. Neither is back. The branch below infers
  // nothing: it reads what the reader's own chevron tap or submit recorded.
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
    // The growth branch. Runs after the reflow correction, so it writes from the
    // corrected position rather than fighting it, and before the signal
    // reconciles below, so the chevron describes where it left the reader rather
    // than where the growth alone would have.
    honourGrowth(el);
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
  return { onScroll, onResize, detachGestures };
}
