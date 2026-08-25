import { signal, type ReadonlySignal } from '@preact/signals';
import { prefersReducedMotion } from '../../utils/platform';
import { USER_SCROLL_WINDOW_MS } from '../../utils/scrollActivity';
import { isMobile } from '../../utils/viewport';
import { applyNavFocus, clearNavFocus, navFocusElement } from '../shared/focusMarker';

/** Shared scroll-position signals for the chat area.
 *
 *  Writers MUST gate on `isElementVisible(el)` before mutating these signals,
 *  and readers before measuring one. A transcript laid out at 0x0 answers every
 *  geometric question wrongly (`isScrollable` false, so it clears `notAtTop`),
 *  and the app's own chrome routinely produces one. A COLLAPSED pane is the
 *  everyday case: the desktop split at ratio 0, or mobile's `.content-row`,
 *  which collapses to height 0 rather than `display: none` so its
 *  `position: fixed` children still render.
 *
 *  The policy this module implements is ADR 0064, docs/adr/. The transcript's
 *  position belongs to the reader, and the follow toggle is the one standing
 *  request. Read it before changing what arms or retires a follow.
 *
 *  `awayFromBottom` carries the whole weight of the live edge. An unarmed reader
 *  is routinely away from the bottom while a reply streams, and the down chevron
 *  is their only way back. It flips on the first pixel off the bottom and is
 *  reconciled on every scroll AND every resize. */
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
 *  subpixel slack). Drives the mobile thread-title fade overlay, so it eases in
 *  the moment content slides under the sticky title. `notAtTop`'s 80px chevron
 *  threshold is too coarse for that. */
export const scrolledFromTop = signal(false);

/** The currently-active scroll container element, set by `useScrollObservers`
 *  when it attaches listeners to a new element. Resolving it by
 *  `querySelector('.thread-content')` instead is fragile: a container with no
 *  box can be first in document order, and the selector would find it.
 *
 *  A plain mutable variable rather than a signal, since it is only ever read
 *  imperatively. */
let _activeScrollElement: HTMLElement | null = null;

export function setActiveScrollElement(el: HTMLElement | null) {
  _activeScrollElement = el;
}

/** True if the element is actually visible: not `display: none`, and not
 *  clipped by a zero-height `overflow: hidden` ancestor. Mobile's
 *  `.content-row` is the everyday second case, since it collapses to height 0
 *  so `position: fixed` children like ThreadDrawer still render. */
export function isElementVisible(el: HTMLElement): boolean {
  const r = el.getBoundingClientRect();
  if (r.width <= 0 || r.height <= 0) return false;
  // An element inside a zero-height overflow:hidden container reports non-zero
  // dimensions from layout, so the clipping ancestor has to be found by walking.
  let ancestor = el.parentElement;
  while (ancestor && ancestor !== document.documentElement) {
    // display:contents removes the element's own box, so it measures 0x0 while
    // its children are fully visible. Skip these ancestors.
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

/** Fallback for when `_activeScrollElement` has not been set yet. */
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

/** Resolve the visible scroll container. Re-checked on each call, so a layout
 *  switch mid-animation cannot scroll a stale element. */
function resolveTarget(): HTMLElement | null {
  let el = _activeScrollElement;
  if (el && !isElementVisible(el)) el = null;
  return el ?? findVisibleThreadContent();
}

/** Pace knobs for the ONE programmatic-scroll animation, `animateScroll` below.
 *  Its shape is ADR 0065, docs/adr/: rAF, a direct fractional `scrollTop` write,
 *  and a time-based easeOutCubic curve over a distance-scaled duration. To make
 *  every navigation faster, lower SCROLL_MAX_MS and SCROLL_PX_PER_MS. */
const SCROLL_MIN_MS = 240;         // floor, so a short scroll reads as a glide rather than a snap
const SCROLL_MAX_MS = 760;         // ceiling, so a very long scroll stays brisk and the tween stays bounded
const SCROLL_PX_PER_MS = 6.5;      // distance to duration rate between the two; lower is more gradual
const SCROLL_FRAME_MS = 1000 / 60; // head start, so the first painted frame already steps
const easeOutCubic = (t: number) => 1 - Math.pow(1 - t, 3);

/** Active navigation rAF id (`animateScroll`, plus the reduced-motion
 *  re-assert). ONE at a time: every entry point cancels the one in flight, so a
 *  down-tap right after an up-tap wins cleanly. */
let _scrollAnimRaf: number | null = null;
/** Is the tween in flight a HELD scroll (see `markHeldScroll`) rather than a
 *  one-off navigation? Set from the marker `animateScroll` was handed, so it
 *  cannot disagree with who is writing the frames. Read by
 *  `stopFollowingBottom` and `cancelLanding`, which must stop our own motion
 *  and no one else's. */
let _heldAnim = false;
/** WHOSE motion the held glide in flight is: the standing follow's RIDE, or a
 *  submit's LANDING. Both aim at the live edge (ADR 0080), so they are told
 *  apart by who owns them rather than by where they go.
 *
 *  It decides two things `_heldAnim` cannot answer. A second call for the same
 *  glide leaves the one in flight alone rather than restarting its easing
 *  part-way. And the reader's own scroll cancels a landing always, a ride only
 *  through the follow's disarm.
 *
 *  Set by each glide right after it starts, and cleared with the tween. A glide
 *  that forgot to set it reads as restartable, the harmless direction. */
type HeldGlide = 'ride' | 'landing';
let _heldAnimTarget: HeldGlide | null = null;
/** Forget the tween: no rAF pending, and nobody owning one. EVERY way a tween
 *  ends goes through this, the natural landing included. So both flags above
 *  mean "in flight" rather than "was in flight last time". */
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
 *  Every such write goes through `markNavigationScroll`, so these cannot fall
 *  out of sync with the writes they describe. Same construction, and the same
 *  reason, as `lastNudgeAt` in utils/iosRepaint.ts.
 *
 *  The element is half the answer rather than bookkeeping. `useScrollMemory`
 *  positions three containers and marks all of them here: the transcript, the
 *  content pane's body, the thread drawer's list. Two of the three consumers ask
 *  only about the transcript. Without the element, a content-pane restore would
 *  claim the transcript's next 64ms of scroll events. */
let _navScrollAt = -Infinity;
let _navScrollEl: HTMLElement | null = null;
/** WHICH KIND of write our own last one was. Every one of them marks a
 *  navigation, so the mobile header, the render-window expansion and the mobile
 *  scroll indicator all stand down for it. What the kind adds is what a consumer
 *  may do INSTEAD of acting on the scroll:
 *
 *  - `placement`: the app took the reader somewhere they asked to go, a chevron
 *    tap, a deep link or a saved-position restore. The mobile header reveals for
 *    one, because `.chat-exchange`'s `scroll-margin-top` clears a VISIBLE header
 *    and a half-hidden one would cover the landing.
 *  - `held`: the app is keeping a rider on the live edge (`markHeldScroll`). The
 *    header reveals for these too, and the platform-scroll correction must NOT
 *    read one as somebody placing the reader: see `isPlacementScroll`.
 *  - `anchor`: the app moved the container to keep the reader on the SAME
 *    content while the layout changed under them (`markAnchorScroll`). Nobody
 *    has been taken anywhere, so the chrome must stay exactly where the reader
 *    left it.
 *
 *  One field rather than a flag per consumer, so a write carrying two kinds at
 *  once is not expressible. */
type NavScrollKind = 'placement' | 'held' | 'anchor';
let _navScrollKind: NavScrollKind = 'placement';

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
 *  EVERY navigation write in this module goes through it, and so does
 *  `useScrollMemory`'s positioning of the transcript on open. A saved-position
 *  restore and the open-at-the-top reset are the app placing the reader, just
 *  as a chevron tap is. Unmarked, the mobile header reads either one as the
 *  reader scrolling down and hides. The render-window expansion reads it as the
 *  reader asking for older turns. */
export function markNavigationScroll(el: HTMLElement, top: number) {
  _navScrollAt = nowMs();
  _navScrollEl = el;
  // A PLACEMENT unless `markHeldScroll` or `markAnchorScroll` says otherwise
  // once this returns. Reset here rather than left alone, so the kind always
  // describes the write being recorded rather than the one before it.
  _navScrollKind = 'placement';
  el.scrollTop = top;
}

/** Write `top` and record it as the app HOLDING the reader on the content they
 *  were already reading, while the layout changed under them. Two writers, and
 *  they are the same act either side of the DOM/layout line:
 *  `withScrollAnchor`'s reveal correction and `restoreAfterReflow`'s pane-resize
 *  correction.
 *
 *  It marks a navigation, like every other write here. That is what stops the
 *  render-window expansion reading a correction landing near the top as a
 *  request for older turns.
 *
 *  It is NOT a placement, and that half is the mobile bug it was added for. The
 *  correction can move the container hundreds of pixels while the reader's eye
 *  stays on one line. The hide-on-scroll header turned that delta into sliding
 *  chrome, so the reader's own line went behind a header and a thread title.
 *  See `isAnchorScroll`. */
export function markAnchorScroll(el: HTMLElement, top: number): void {
  markNavigationScroll(el, top);
  _navScrollKind = 'anchor';
  for (const listener of _anchorScrollListeners) listener(el);
}

/* ── Telling a DELTA consumer the offset was re-based ────────────────────────
 *  `isAnchorScroll` answers a scroll event, and only one arriving inside
 *  `NAV_SCROLL_EVENT_WINDOW_MS`. That is enough for a consumer reading a
 *  POSITION: a late event finds the container settled and reads the same answer
 *  either way.
 *
 *  It is not enough for one reading a DELTA. The mobile hide-on-scroll header
 *  is the one, and it turns every unattributed pixel into chrome sliding. A
 *  correction moves the container hundreds of pixels at once, and a late event
 *  hands the header the whole jump.
 *
 *  Whether the event is late is a race, and a large programmatic jump loses it
 *  on WebKit under load. So a delta consumer is told SYNCHRONOUSLY, at the
 *  write, and re-takes its baseline there. The event then carries a delta of
 *  zero whenever it lands, and the window stops deciding anything. */
const _anchorScrollListeners = new Set<(el: HTMLElement) => void>();

/** Subscribe to the re-base above; returns the unsubscribe. Fires with the
 *  container AFTER the write, so a listener reading `scrollTop` sees where the
 *  browser actually settled it. */
export function onAnchorScroll(listener: (el: HTMLElement) => void): () => void {
  _anchorScrollListeners.add(listener);
  return () => { _anchorScrollListeners.delete(listener); };
}

/** Record that the app just REVEALED something, so the scroll the platform is
 *  about to fire for it is not mistaken for the reader's.
 *
 *  The counterpart of `markNavigationScroll` for a navigation that does not
 *  write `scrollTop` itself: it calls `scrollIntoView` and lets the platform
 *  pick the offset. `choiceCardNav`'s arrow-key step is the one today. Both
 *  signals that would otherwise catch it are blind here. Nothing stamps a
 *  position, and the keydown lands on the choice BUTTON rather than on the
 *  transcript, so no gesture is recorded either.
 *
 *  It routes through `markNavigationScroll`, writing the position back onto
 *  itself, so those two variables keep their single writer. Writing an
 *  unchanged `scrollTop` fires no scroll event, so this moves nobody:
 *  `scrollIntoView` has already settled the offset synchronously.
 *
 *  Three consumers gain by it. The mobile hide-on-scroll header stops sliding
 *  away under an arrow-key step, and the render-window expansion stops reading
 *  one as a request for older turns. The third is the platform-scroll
 *  correction in `makeScrollObservers`' onScroll, for which an unmarked reveal
 *  is indistinguishable from the keyboard adjusting the offset. */
export function markRevealScroll(el?: HTMLElement | null): void {
  const target = el ?? resolveTarget();
  if (!target) return;
  markNavigationScroll(target, target.scrollTop);
}

/** Did one of OUR OWN navigations produce the scroll event being handled?
 *
 *  Those events look exactly like the reader's, and two consumers must tell
 *  them apart. The mobile hide-on-scroll header would otherwise slide away
 *  under a chevron tap, or half-cover the event a deep link just landed on. The
 *  mobile scroll indicator must not read our own write as the reader summoning
 *  it.
 *
 *  Two terms, and the WINDOW is the load-bearing one. A live tween is the easy
 *  half. The hard half is that a write's scroll event lands a frame or more
 *  later. The instant navigations run no tween at all, and even a tween clears
 *  its rAF handle on the frame it lands. So an "is a tween running" test alone
 *  answers false for exactly the events it exists to catch. */
export function isNavigationScroll(el?: HTMLElement | null): boolean {
  if (_scrollAnimRaf !== null) return true;
  if (nowMs() - _navScrollAt >= NAV_SCROLL_EVENT_WINDOW_MS) return false;
  // A caller that names its element is asking about ITS scroll events, so a
  // write to some other container is not an answer. A caller that names none
  // (the mobile header, which follows whichever pane is active) takes any.
  return !el || _navScrollEl === el;
}

/** Did one of our own PLACEMENTS produce this scroll event: a write that put the
 *  reader somewhere on purpose, rather than the ride holding them where they
 *  already were?
 *
 *  The narrow half of `isNavigationScroll`, and the one the platform-scroll
 *  correction asks. It has to be the narrow one, because held writes mark a
 *  navigation as well, and a settling transcript takes one every growth round.
 *  The wide predicate is therefore true almost continuously on exactly the
 *  threads the correction exists for.
 *
 *  A held write's OWN event lands on the stamp, and `isWhereWeHeldIt` excludes
 *  it a term earlier. So a scroll that is NOT on the stamp cannot be that
 *  write's event, and the clock has nothing to add. Only a placement leaves the
 *  reader somewhere the correction must not undo. */
function isPlacementScroll(el: HTMLElement): boolean {
  return _navScrollKind !== 'held' && isNavigationScroll(el);
}

/** Was this scroll event the app holding the reader on their own content, i.e.
 *  a `markAnchorScroll` write? Read by the mobile hide-on-scroll header, which
 *  reveals itself for every other navigation and must not for this one.
 *
 *  Takes no element by default, matching the header: it follows whichever pane
 *  is active rather than one container. */
export function isAnchorScroll(el?: HTMLElement | null): boolean {
  return _navScrollKind === 'anchor' && isNavigationScroll(el);
}

/** Is a navigation OTHER than the caller's own anchor correction driving `el`?
 *
 *  `withScrollAnchor`'s next-frame re-assert asks it. Its own write marks a
 *  navigation, so the plain predicate would stand the re-check down for the
 *  very write it exists to re-assert.
 *
 *  A live tween counts whatever the last write said. It is going where the
 *  reader asked more recently, and a pre-tween offset re-asserted against it is
 *  a frame of jitter for nothing. */
export function isOtherNavigationScroll(el: HTMLElement): boolean {
  if (_scrollAnimRaf !== null) return true;
  return _navScrollKind !== 'anchor' && isNavigationScroll(el);
}

/* ── The standing request to ride the live edge ──────────────────────────────
 *  The flag below is the ARMED half of the follow. ADR 0064 (docs/adr/) is the
 *  policy: what may arm it, what retires it, and every arming rule tried and
 *  rejected. Read it before changing either list.
 *
 *  ARMED and CARRYING are two states, and the flag is only the first. The
 *  request persists across an idle spell untouched. A reader who scrolls away on
 *  a quiet thread keeps the lit toggle and the recorded live-edge reading
 *  position. What stops for them while idle is the WRITING. See
 *  `followIsCarrying`.
 *
 *  TWO functions arm it and no others, both through `armFollowOn`:
 *  `setFollowLiveEdge` (the toggle) and `resumeFollowingBottom` (which replays a
 *  toggle request recorded in this thread). Being AT the bottom arms nothing.
 *
 *  A SIGNAL rather than a plain boolean, because the toggle renders it. It has
 *  to go off by itself when a scroll retires the follow underneath it. Exported
 *  as a `ReadonlySignal<boolean>`, which the compiler refuses to let a component
 *  assign, so reading the state cannot become a way of setting it. */
const _followingBottom = signal(false);

/** Is the standing follow armed? Read by the follow toggle, which RENDERS it.
 *  `ReadonlySignal` on the way out on purpose: see the block above. */
export const followingLiveEdge: ReadonlySignal<boolean> = _followingBottom;

/* ── The follow SEED ─────────────────────────────────────────────────────────
 *  The reader's last PRESS of the toggle, remembered across threads and
 *  reloads, and ARMED until a disarm press says otherwise. It is what a thread
 *  with no *reading position* of its own starts as.
 *  Same shape as `selectedScope` for the destination picker: a last-used value
 *  read only by a thread that does not already remember. That is what keeps it
 *  out of the threads a reader has parked in.
 *
 *  It also gives the toggle something to show in the compose view, which has no
 *  transcript. Showing it there is what lets the follow be armed BEFORE the
 *  first send.
 *
 *  Written by the toggle and by nothing else. A scroll retiring the follow is
 *  about THIS thread and records itself as that thread's offset. It must not
 *  quietly cancel a standing preference for every future thread.
 *
 *  Device-scoped. Whether to ride the live edge is a property of the screen in
 *  front of the reader, not of the account. It must be right on the first paint
 *  with no server round trip. */
const FOLLOW_SEED_KEY = 'lucidos-follow-live-edge';

/** The seed's stored form, as a pure function so the DEFAULT is answerable
 *  without a browser.
 *
 *  ABSENT reads armed, and only the literal `'false'` a disarm press writes
 *  reads the other way. A device that has pressed nothing has not chosen to
 *  stay put, and the two used to be indistinguishable. One press opts out for
 *  good, which is what makes riding the safe side to default to. It is a
 *  departure from ADR 0064's "nothing rides unless the reader asked", recorded
 *  in that ADR's amendment note. */
export function followSeedFromStored(raw: string | null): boolean {
  return raw !== 'false';
}

/** `localStorage` is absent in the DOM-free unit environment this module is
 *  deliberately importable from (see `parseNavigatedTurn` on why it stays free
 *  of the heavy `store` import). Nowhere to remember answers the same as
 *  nothing remembered, which is the default above.
 *
 *  The try/catch is the second half of that, and is not decoration. This runs
 *  at MODULE LOAD, and a browser with storage blocked THROWS on the access
 *  rather than answering null. That would take the whole chat bundle down with
 *  it. `useScrollMemory` guards its own reads for the same reason. */
function readFollowSeed(): boolean {
  if (typeof localStorage === 'undefined') return followSeedFromStored(null);
  try {
    return followSeedFromStored(localStorage.getItem(FOLLOW_SEED_KEY));
  } catch {
    return followSeedFromStored(null);
  }
}

const _followSeed = signal(readFollowSeed());

/** What the toggle shows where there is no transcript to describe, i.e. the
 *  compose view. `ReadonlySignal` for the same reason `followingLiveEdge` is: the
 *  one writer is the press, through `setFollowLiveEdge`. */
export const followLiveEdgeSeed: ReadonlySignal<boolean> = _followSeed;

/** Remember this press. The ONE writer, called only from `setFollowLiveEdge`.
 *
 *  The signal is written FIRST, and the persistence is allowed to fail. A
 *  blocked or full store throws on `setItem`, and this runs inside the toggle's
 *  click handler. A throw here would leave the press doing nothing at all,
 *  which is a far worse loss than the press not surviving a reload. */
function recordFollowSeed(on: boolean): void {
  _followSeed.value = on;
  if (typeof localStorage === 'undefined') return;
  try {
    localStorage.setItem(FOLLOW_SEED_KEY, String(on));
  } catch { /* quota or disabled, the press still stands for this session */ }
}

/** Apply the seed to `el`: arm the follow if that is what the reader last
 *  chose, and report whether the SEED SPOKE. A `live-edge` caller needs the
 *  answer, because its own no-position reset would undo the write.
 *
 *  WHERE the seed may speak is the caller's to decide, and is the load-bearing
 *  half. `attachScrollMemory` calls it only for the transcript, and only in the
 *  two branches with no *reading position* at all. A thread the reader HAS
 *  parked in keeps deciding for itself. The content pane and the thread drawer
 *  must never arm the transcript's follow.
 *
 *  Routed through `resumeFollowingBottom` rather than arming directly. It is the
 *  same act, and reusing it keeps the arming entry points at two.
 *
 *  For `in-place` the return is NOT the same as "the follow is now armed". That
 *  branch declines over a link that landed OFF the live edge, and this cannot
 *  see it. Only the `live-edge` caller reads the return, and the two coincide
 *  there. */
export function applyFollowSeed(el: HTMLElement, from: FollowResumeFrom = 'live-edge'): boolean {
  if (!_followSeed.value) return false;
  resumeFollowingBottom(el, from);
  return true;
}

/** Is the agent LIVE on the thread being shown, i.e. is anything running on it?
 *  Told to this module by `ChatExchange` for its `isLast` turn, because
 *  `scrollState` must not import `store` (see `parseNavigatedTurn`). The
 *  derivation is `exchangeMarksThreadLive`, which needs BOTH the turn's status
 *  and the thread projection's own quiescence. A plain mutable variable, read
 *  imperatively, like `_activeScrollElement`.
 *
 *  It decides what happens to a reader who is somewhere OTHER than the live
 *  edge. On a quiet thread their scroll does not end the ride, and neither does
 *  the up chevron or turn stepping. Growth does not carry them back down either,
 *  since growth there is the transcript finishing its own rendering.
 *
 *  A reader still ON the live edge asks it nothing. The app's own rendering must
 *  not move a rider off an edge they never left. See `keepTheLiveEdge`.
 *
 *  Two things deliberately DO NOT ask it, and both are the reader asking
 *  directly: pressing the toggle (`setFollowLiveEdge`), and resuming a request
 *  recorded in this thread (`resumeFollowingBottom`). Pressing the toggle on a
 *  finished thread must still take the reader to the bottom. */
let _threadLive = false;

/** When the SUBMIT's own claim that the thread is live runs out. A submit says
 *  so before any status can (see `followSubmit`), and that claim must EXPIRE
 *  rather than stand until something contradicts it. What would contradict it
 *  may never come. A Continue whose POST fails leaves the last turn's status
 *  exactly as it was, and so does an unanswered permission decision. So
 *  `ChatExchange`'s effect never re-runs and never writes `false`. Left
 *  standing, the claim would cost the reader their follow the next time they
 *  browsed an idle thread. */
let _submitLiveUntil = -Infinity;

/** How long a submit's claim outlives the submit. It has to cover the POST round
 *  trip AND the whole gap before the thread projection says `running`, which is
 *  the longer of the two. `meta.status` only advances when a per-event aggregate
 *  carrying `running` arrives. `store.ts`'s `isRenderedThreadIdle` documents
 *  that gap running to about eight seconds on a resume.
 *
 *  Being wrong LONG costs a few more seconds in which a scroll retires the
 *  follow, on a thread the reader just submitted to. That is the least likely
 *  moment for them to be idly browsing. Being wrong SHORT re-opens the gap the
 *  claim exists for. */
const SUBMIT_LIVE_CLAIM_MS = 20_000;

/** Tell this module whether the thread on screen has a turn in flight. Called by
 *  `ChatExchange` for its last exchange, and cleared when that exchange
 *  unmounts, so a thread switch cannot leave the previous thread's answer
 *  standing.
 *
 *  A `true` retires a submit's claim, because what the claim was guessing at has
 *  arrived. A `false` DOES NOT, and that asymmetry is the whole point: `false`
 *  is exactly what a lagging source says inside the window the claim covers.
 *  `exchangeMarksThreadLive` needs the thread projection to agree, and the
 *  projection is the slow half. The render right after a send therefore writes
 *  `false` while the agent is on its way. Clearing the claim there would destroy
 *  it in the one window it exists for. Nothing is lost by ignoring `false`,
 *  since the claim expires on its own.
 *
 *  A `true` that WAKES the thread also does the growth round the observer
 *  missed, because this call arrives too late to be seen by it. See
 *  `honourWake`. */
export function setThreadLive(live: boolean): void {
  const waking = live && !_threadLive;
  _threadLive = live;
  if (live) _submitLiveUntil = -Infinity;
  if (waking) honourWake();
}

/** The ride resumes on the WAKE itself, not on the next growth round.
 *
 *  The follow does its work from the transcript's ResizeObserver
 *  (`honourGrowth`), and this module learns the thread is live from a Preact
 *  `useEffect` in `ChatExchange`. Those two arrive in the wrong order. The new
 *  turn's row mounting fires the observer inside the same frame, while Preact
 *  defers its effects to a task after it. So the WAKING resize is handed to
 *  `honourGrowth` while this module still believes the thread is idle.
 *
 *  Usually invisible, because a streaming reply resizes again a moment later. It
 *  is visible for a turn that mounts its row and then produces nothing: a
 *  coding-agent turn RESUMING sits on `SessionStarted` for fifteen to twenty
 *  seconds, stranding an armed reader short of the edge with the toggle lit.
 *
 *  So the transition replays the missed round through `honourGrowth` itself,
 *  guards and all, rather than growing a second copy of the follow's rule.
 *
 *  Only on the false to true EDGE. A repeated `true` describes no new content,
 *  and acting on one would put a reader back at the live edge after they had
 *  scrolled away. It replays the LIVE arm alone, needing no edge reading. */
function honourWake(): void {
  const el = resolveTarget();
  if (el) honourGrowth(el);
}

/** Is the agent live, by the status or by a submit's unexpired claim? */
function threadIsLive(): boolean {
  return _threadLive || nowMs() < _submitLiveUntil;
}

/** Is the standing follow CARRYING the reader right now, as opposed to merely
 *  being armed? Armed AND live. See `_followingBottom` for why those are two
 *  states, and ADR 0064 for the policy.
 *
 *  It answers for a reader who is somewhere OTHER than the live edge, which is
 *  the only place the liveness term decides anything. A reader ON the edge is
 *  kept there whatever the thread is doing, by `keepTheLiveEdge`.
 *
 *  Three callers ask it: the growth branch (`honourGrowth`), the reveal snap
 *  (`honourAnchoredMutation`), and `withScrollAnchor`'s decision to skip the
 *  anchor correction. The last two are ONE act split across the DOM/layout
 *  line, so they must answer the same. Otherwise an idle armed reader gets
 *  neither the correction nor the snap, and drifts on the content above them.
 *
 *  Deliberately NOT asked by the reader's own explicit requests, which write the
 *  live edge directly: pressing the toggle, resuming a recorded request on
 *  re-entry, and the seed. Nor by `keepTheLiveEdge`, whose three events are the
 *  app moving a rider who never left, rather than carrying one who did. */
export function followIsCarrying(): boolean {
  return _followingBottom.value && threadIsLive();
}

/** Subscribers notified when the follow is ARMED, and never when it is retired.
 *  One consumer today: `attachScrollMemory`, which records the request as this
 *  thread's reading position.
 *
 *  The asymmetry is the design. A retirement has two causes that must be
 *  recorded differently, and this side cannot tell them apart. The reader
 *  scrolling away arrives WITH the scroll event that already records the offset
 *  they landed on. `focusThread` retiring on a thread switch must record nothing
 *  at all. Otherwise the thread being LEFT has its live edge overwritten by
 *  whatever offset the shared container happens to hold.
 *
 *  A plain callback set rather than a signal. An exported writable signal would
 *  be an arming point no source scan could stop, and it would broadcast the
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
 *  addressable yet. Asked at submit time and again on every growth round, until
 *  the landing lets go or lapses (see `_pendingLanding`). */
type TurnResolver = (el: HTMLElement) => HTMLElement | null;

/** THE SUBMIT'S LANDING, from the submit until the agent starts (ADR 0080). It
 *  re-aims at the live edge on every growth round, and each round writes
 *  nothing when there is nowhere to go. Null when no submit is in hand.
 *
 *  A HOLD rather than a one-shot, because the agent's opening arrives in
 *  instalments. A single glide catches only the instalments inside it. That
 *  left a send resting on the status line, with the Thinking step below the
 *  fold. It left a card answered from the live edge unmoved entirely.
 *
 *  The turn is resolved ONCE, into `on`, and then kept. Re-asking every round
 *  would let the hold change which turn it is watching underneath itself: a
 *  queued follow-up makes `awaitsNewTurn` answer with a NEWER turn, while
 *  `drawnAtStart` came from the first. It also spares a `querySelectorAll` and
 *  an ancestor walk per round of a streaming reply.
 *
 *  The turn and its row count are ONE field, so a turn without its snapshot is
 *  not expressible. `drawnAtStart` is what that turn had drawn when it
 *  resolved, and the hold's end condition is the count differing from it. */
let _pendingLanding: {
  resolveTurn: TurnResolver;
  /** Does this submit WAIT for the agent to start? True for the four that ask
   *  the agent for something. False for a CANCEL, in either shape, which asks
   *  it to FINISH: no first row is coming, so the landing aims once and lets
   *  go. */
  holds: boolean;
  on: { turn: HTMLElement; drawnAtStart: number } | null;
  at: number;
} | null = null;

/** TWO deadlines, because a landing can be waiting for two different things and
 *  they are not the same length.
 *
 *  Before its turn has a box the landing has NOTHING to show, so it gives up
 *  quickly. That is the second and later queued follow-ups, folded into a
 *  CLOSED `<details class="queued-message-group">`. Waiting longer buys nothing
 *  and costs the reader their NEXT submit, which `followSubmit` swallows while
 *  a landing is in hand.
 *
 *  Once the turn HAS a box the landing is waiting for the agent, and that wait
 *  is seconds. What reaches the end of it is a turn that draws no ROW: a dead
 *  request, a turn the reader has collapsed, and a coding-agent turn running
 *  tool calls with the step log switched off. None of those grows much, so the
 *  reader is barely moved while it runs.
 *
 *  Lapsing never falls back to the live edge: a submit arms nothing, so there
 *  is no ride to honour. Without these the hold would sit forever. */
const LANDING_ADDRESSABLE_MS = 1000;
const LANDING_HOLD_MS = 8000;

/** What the AGENT has drawn into this turn: the rows inside its response body,
 *  its Thinking step and its text among them. Counting rows rather than reading
 *  the status is what makes one signal serve every hold. A send's turn
 *  starts empty. A card's turn already holds whatever the agent said before it
 *  asked, so only the CHANGE is common to both.
 *
 *  The rows, and NOT `.response-body`'s own children. Those are the SECTION
 *  wrappers `splitEventSections` produces, one per `section_break`, and a
 *  resuming agent appends into the wrapper that is already there. Counted at
 *  that level a card's turn never changes, so the hold would run to its
 *  backstop and drag the reader through the reply. See `ChatExchange`. */
function drawnRows(turn: HTMLElement): number {
  if (typeof turn.querySelectorAll !== 'function') return 0;
  return turn.querySelectorAll('.response-content > *').length;
}

/** Is this turn a QUEUED follow-up, which will never draw a response of its
 *  own? Holding for one would keep the reader at the bottom for the whole
 *  backstop, while the reply ABOVE it streamed.
 *
 *  Recognised POSITIVELY, by the remove button its queued status carries
 *  (`isQueuedUserMessage` in `ChatExchange`). Inferring it from a MISSING
 *  `.response-panel` reads true for two turns that are about to draw: a row
 *  whose panel mounts a commit later, and an unanswered question or permission
 *  divider, which renders no panel at all. The second is the turn a card submit
 *  acts on, so the inference abandoned the hold on exactly the case it exists
 *  for. */
function turnIsQueued(turn: HTMLElement): boolean {
  return typeof turn.querySelector === 'function'
    && turn.querySelector('.queued-message-remove') !== null;
}

/** The scroll offset the live edge sits at: the MAX offset rather than
 *  `scrollHeight`, which the browser would clamp to the same place. Naming the
 *  real target keeps every write meaningful instead of leaning on the clamp.
 *  One definition also keeps everything aiming at the live edge, or measuring
 *  against it, from drifting apart. */
function liveEdgeTop(el: HTMLElement): number {
  return Math.max(0, el.scrollHeight - el.clientHeight);
}

/** The ONE at-the-live-edge threshold, asked of an OFFSET. 2px of slack absorbs
 *  subpixel rounding (mobile zoom, device-pixel snapping) and the iOS overscroll
 *  bounce, without making the chevron look stuck.
 *
 *  Taking an offset rather than reading `scrollTop` is what lets a navigation
 *  ask it of a landing it has not made yet. Two do, and both must: turn stepping
 *  reconciles the chevron against its target, and a deep link decides the ride's
 *  fate by where it is about to come to rest. A target BEYOND the edge answers
 *  true, the browser clamping it back to the edge. */
function isLiveEdgeTop(el: HTMLElement, top: number): boolean {
  return top >= liveEdgeTop(el) - 2;
}

/** Is the reader ON the live edge right now? Read by the chevron's reconcile,
 *  the send's "are they already there" test and both observers. One definition,
 *  so those cannot drift from each other or from the offset form above. */
function isAtLiveEdge(el: HTMLElement): boolean {
  return isLiveEdgeTop(el, el.scrollTop);
}

/** Can this transcript be scrolled at all? One definition, read by the up
 *  chevron (`notAtTop`) and the mobile title fade (`scrolledFromTop`). A
 *  transcript with a hair of overflow, from a border or a rounded line height,
 *  then answers the same for both. That is what the 10px absorbs.
 *
 *  The DOWN chevron deliberately asks something else, `isAtLiveEdge`'s 2px. It
 *  describes where the reader IS, not whether the thread can move at all. */
function isScrollable(el: HTMLElement): boolean {
  return el.scrollHeight > el.clientHeight + 10;
}

/* ── Held scrolls, and how the reader's gesture is told from ours ────────────
 *  `_heldEl` / `_heldTop` record WHERE our own last deliberate write left the
 *  container. Two things hold a reader deliberately and both need it: the
 *  standing follow (its per-growth write and its live-edge glide) and a submit's
 *  landing. For the follow the answer retires the request; for the landing it
 *  cancels the glide.
 *
 *  A held write is marked as a navigation scroll like every other write, but
 *  `isNavigationScroll`'s 64ms window cannot answer THIS question. A streaming
 *  thread re-marks itself every frame, so a flick landing inside the window
 *  would read as ours and the reader would fight us. The POSITION answers it
 *  exactly instead. Content growing below the reader changes `scrollHeight` and
 *  never `scrollTop`, while every gesture changes `scrollTop`.
 *
 *  It is also what ends both for a navigation that deliberately puts the reader
 *  somewhere else, with no call site of its own: none of those is a held write,
 *  so the first frame of one already reads as the reader being elsewhere. And it
 *  keeps the follow ALIVE across everything that is not a scroll, such as
 *  resolving a card, granting a permission or expanding a turn. */
let _heldEl: HTMLElement | null = null;
let _heldTop = -1;
/** Was the container ON the live edge when we took that stamp? MEASURED at the
 *  stamp, which is the only moment the answer is knowable: growth moves the
 *  edge away afterwards while leaving `scrollTop` exactly where it was.
 *
 *  It is what lets a placement the app has JUST made answer "the reader is on
 *  the edge" (see `heldOnTheLiveEdge`). `keepTheLiveEdge`'s own reading is taken
 *  at the END of a round, so it is absent for the round that needs it most: the
 *  growth that follows a thread opening onto a recorded ride. */
let _heldAtLiveEdge = false;

/** Record where the reader is WITHOUT writing anything, so a landing that has
 *  not moved them yet can still tell their next gesture from our own writes. A
 *  submit takes this stamp the moment it schedules a landing: until the turn it
 *  is waiting for renders there is no write to stamp, and the reader's flick in
 *  that window must still cancel it.
 *
 *  THE ONE WRITER of all three fields, so a stamp carrying a stale reading of
 *  the edge is not expressible. `null` forgets the stamp entirely. */
function holdPosition(el: HTMLElement | null) {
  _heldEl = el;
  _heldTop = el ? el.scrollTop : -1;
  _heldAtLiveEdge = !!el && isAtLiveEdge(el);
}

/** Write `top` and record it as OURS, so the scroll event it fires a frame later
 *  cannot be read as the reader taking over. Goes through `markNavigationScroll`
 *  like every other write the app makes, so the mobile header and the
 *  render-window expansion keep standing down for it too.
 *
 *  And says which KIND of write it was, since a held write is not a PLACEMENT.
 *  This is the only place `_navScrollKind` is set to `held`. See
 *  `isPlacementScroll` for what turns on the distinction. */
function markHeldScroll(el: HTMLElement, top: number) {
  markNavigationScroll(el, top);
  _navScrollKind = 'held';
  holdPosition(el);
}

/** Arm the standing follow at the position the caller's own scroll just reached,
 *  so the trailing scroll event of that scroll cannot retire the request it just
 *  made. Two callers and no more: `setFollowLiveEdge` (the toggle) and
 *  `resumeFollowingBottom` (which replays a recorded toggle request).
 *
 *  It takes its element rather than resolving one, for the restore. That caller
 *  holds the container it is positioning, and a thread opening mid-layout-swap
 *  can make `resolveTarget` answer with the outgoing mount.
 *
 *  Notifies only on the unarmed to armed transition, so the recording side hears
 *  about a request even when arming produces no scroll event. That case is
 *  ordinary: a reader already at the live edge gets no write, and an idle thread
 *  then grows nothing, so no scroll carries the request anywhere. */
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

/** Where a resumed standing follow starts from, i.e. whether resuming it also
 *  MOVES the reader.
 *
 *  `live-edge` is the ordinary re-entry, and the write is required rather than
 *  tidy. `.thread-content` is one element reused across threads, so on arrival
 *  it holds the OUTGOING thread's offset. Arming alone would leave the reader
 *  there until the next growth round.
 *
 *  `in-place` defers to a DEEP LINK, which owns where the reader is looking on
 *  this open. For a link still IN FLIGHT it writes nothing at all, so one that
 *  turns out dead costs the reader nothing.
 *
 *  A link that LANDED off the live edge ends the ride, and the guard in the
 *  branch below keeps this from reinstating it. One that landed ON the edge is
 *  heading where the ride was heading, so the ride takes its motion over. That
 *  is the one case where `in-place` writes. Asked for by
 *  `standDownForDeepLink` and by `onPageWake`'s claim-held branch. */
export type FollowResumeFrom = 'live-edge' | 'in-place';

/** Resume a standing follow the reader armed in this thread BEFORE they left it:
 *  arm, then write the live edge, so the growth branch carries them from there.
 *
 *  Called only by `attachScrollMemory`, on a thread whose recorded reading
 *  position is the live edge, or through `applyFollowSeed` on one with no record
 *  at all. The write goes through `markHeldScroll` like every other follow
 *  write, so the mobile header stands down for it.
 *
 *  Nothing waits for content here, unlike the offset restore's observer retries.
 *  An offset can only be honoured once the transcript is tall enough to hold it.
 *  The live edge is wherever the content currently ends, so the write lands on
 *  today's bottom and the armed follow rides every later arrival. */
export function resumeFollowingBottom(el: HTMLElement, from: FollowResumeFrom = 'live-edge'): void {
  if (from === 'in-place') {
    // Two guards, and both are about what resuming may NOT undo.
    //
    // ALREADY ARMED means the reader never lost the request. Arming again would
    // clear the held stamp `isFollowScroll` reads, which silently switches what
    // this landing records from the live edge to an offset.
    if (_followingBottom.value) return;
    // LANDED OFF THE LIVE EDGE means the link has ended the ride ON PURPOSE
    // (`scrollToSelectorAndPulse`), because a link is a request to be at ONE
    // place. Resuming afterwards would undo that decision, so the resume is
    // only for a link still IN FLIGHT or one that landed where the ride was
    // heading anyway. Either way it holds the ride open until the answer is
    // known.
    //
    // It asks about the LANDING, never whether the agent is running. Liveness
    // is the wrong proxy in the commonest deep-link case, because
    // `waiting_for_user_answer` is quiescent (`isRenderedThreadIdle`). A thread
    // parked on a question card therefore reads as IDLE.
    //
    // Both callers get it: `standDownForDeepLink`, which can run either side of
    // the landing, and `onPageWake`'s claim-held branch. One guard rather than
    // an ordering rule at each. That is also what makes the two resolve
    // orderings agree, a cached thread resolving before this runs and an
    // unloaded one after it.
    if (deepLinkLandedOffLiveEdge()) return;
    // A link that has ALREADY landed is, by the guard above, heading for the
    // live edge. So the ride TAKES ITS MOTION OVER rather than arming beside
    // it, exactly as the landing itself does when the ride is already armed.
    //
    // Arming beside it is not enough, and the case is the ordinary cross-thread
    // tap. `focusThread` retired the ride before the link resolved, so the
    // landing ran as a plain element tween, and `.thread-content` arrives
    // holding the OUTGOING thread's offset. Those frames therefore travel a
    // long way marking plain navigation, which is not where the follow held
    // anybody. Every one of them records this thread's position as an offset,
    // and the ride the reader never ended is lost on the way back in.
    if (deepLinkHasResolved()) {
      armFollowOn(el);
      rideToLiveEdge(el);
      return;
    }
    // Still in flight, so the link owns a position nobody knows yet and this
    // writes nothing. The stamp follows the POSITION, the reader being wherever
    // the open left them. OFF the edge that is an ordinary offset, which the
    // follow never wrote and must not record as the live edge (see
    // `currentPosition` in `hooks/useScrollMemory.ts`). ON the edge it is
    // exactly what the follow writes, so stamping is honest there.
    armFollowOn(isAtLiveEdge(el) ? el : null);
    return;
  }
  armFollowOn(el);
  markHeldScroll(el, liveEdgeTop(el));
}

/** Is the scroll event being handled the FOLLOW's own write rather than the
 *  reader's gesture? Armed, and the container still exactly where the follow put
 *  it. See `isWhereWeHeldIt` for the 1px slack.
 *
 *  Exported for the recording side, which must write the live edge for the
 *  follow's own writes and a plain offset for everything else. It asks the
 *  POSITION rather than the flag alone on purpose. `.thread-content` carries two
 *  scroll listeners, the disarm here and the save in `attachScrollMemory`. A
 *  save asking only about the flag would answer differently depending on which
 *  ran first. A gesture moves `scrollTop` away from the stamp by definition, so
 *  this answers the same in either order. */
export function isFollowScroll(el: HTMLElement): boolean {
  return _followingBottom.value && isWhereWeHeldIt(el);
}

/** Retire the standing follow, and with it anything we were doing to serve it or
 *  any other request. Called by the disarm in `onScroll`, and exported for the
 *  two navigations that cannot be read off a scroll.
 *
 *  Opening a DIFFERENT thread. A thread the reader just opened is not one they
 *  asked to follow. A restore landing on that thread's saved bottom writes no
 *  scroll the disarm could see. See `focusThread`.
 *
 *  A DEEP-LINK LANDING, on any thread. The reader asked to be at one specific
 *  place, so the ride ends there. The disarm cannot see this one either, and the
 *  case it misses is the ordinary one. Such a link usually points at the
 *  thread's newest turn. The landing therefore leaves the reader AT the live
 *  edge, where the disarm's first condition is false.
 *
 *  It is deliberately NOT gated on the thread being live, unlike every other
 *  retirement here. See `scrollToSelectorAndPulse`, which owns that call, and
 *  ADR 0064. */
export function stopFollowingBottom() {
  // Both of the things that hold a reader are OUR motion, so both stop here.
  // Without it the reader who just scrolled away is dragged back for the rest
  // of the tween. A thread opened mid-glide is scrolled with the previous
  // thread's turn as the target. Only a HELD tween: a deep-link or up-chevron
  // glide belongs to a navigation this has no business cancelling.
  if (_heldAnim) cancelScrollAnim();
  _followingBottom.value = false;
  holdPosition(null);
  _pendingLanding = null;
}

/** Cancel a submit's LANDING: drop one still waiting for its turn to render, and
 *  stop one already gliding. The landing's half of what `stopFollowingBottom`
 *  does, for the reader who has no follow to retire.
 *
 *  Only the landing's OWN tween. A ride's glide is the standing follow's motion
 *  and ends with the follow, on the follow's own two-part test. Ending it here
 *  would retire the ride on a scroll event the disarm deliberately ignores,
 *  such as a shrink clamping the reader down. */
function cancelLanding() {
  if (_heldAnim && _heldAnimTarget === 'landing') cancelScrollAnim();
  _pendingLanding = null;
}

/** Retire a landing that has outlived its deadline, which of the two above
 *  depending on whether its turn ever got a box. Called from the two places a
 *  lapse can be noticed: the growth branch, and the next submit. Neither alone
 *  is enough, because the deadline is wall-clock and the growth branch only
 *  runs when something grows. */
function dropLapsedLanding(): void {
  if (!_pendingLanding) return;
  // A landing that never HOLDS waits on a round trip, not on a box that may
  // never come, so it gets the long budget too. A Stop is the case. The engine
  // only notifies the agent and waits for its answer, allowing seconds before
  // it escalates, and the boundary renders at the end of that.
  const waiting = _pendingLanding.on || !_pendingLanding.holds;
  const deadline = waiting ? LANDING_HOLD_MS : LANDING_ADDRESSABLE_MS;
  if (nowMs() - _pendingLanding.at >= deadline) _pendingLanding = null;
}

/** Is a submit's landing in flight, in either of its two phases: held open for
 *  its turn, or gliding to the live edge. */
function landingInFlight(): boolean {
  return _pendingLanding !== null || (_heldAnim && _heldAnimTarget === 'landing');
}

/** Carry the held stamp onto a scroll THE APP just wrote to hold the reader on
 *  the same content while the layout moved under them. Two writers, and they are
 *  the same act on either side of the DOM/layout line: `restoreAfterReflow` for
 *  a pane resize, `withScrollAnchor` for a toggle. Neither is the reader taking
 *  over, so neither may retire a standing follow or cancel a pending landing.
 *  Without the stamp, the scroll event each fires arrives at a position we do
 *  not recognise and does both.
 *
 *  It CARRIES a hold rather than taking one, hence the guard. With no hold on
 *  this element there is nothing to protect, and stamping would claim a position
 *  nobody asked for.
 *
 *  Re-taking the whole stamp is what keeps the edge reading honest. The
 *  correction holds the reader on their CONTENT, which for a reader parked up in
 *  history is nowhere near the bottom. A stamp that kept an older `true` would
 *  hand `heldOnTheLiveEdge` a position the reader left. */
function carryHeldScroll(el: HTMLElement): void {
  if (_heldEl === el) holdPosition(el);
}

/** What the transcript owes the reader after the APP mutated it and corrected
 *  the scroll to hold them still. `withScrollAnchor`'s side of `honourGrowth`,
 *  and the two say the same thing in the two worlds. Every reveal in the
 *  transcript goes through it: the collapse fold, the per-turn unfold, and the
 *  two transcript-wide turn controls.
 *
 *  ONLY A SCROLL MAY RETIRE THE FOLLOW, so the first line is not optional. A
 *  toggle is a click on a control, not a gesture. The correction moves the
 *  container all the same, and without the stamp that write reads as the reader
 *  taking over. The transcript-wide reveals grow every turn, including those
 *  BELOW the anchored root, so the correction leaves the reader short.
 *
 *  Then, while the follow is CARRYING them, put them back ON the live edge in
 *  ONE held write, in the same frame the caller unfreezes: `snapToLiveEdge`.
 *  That is not a position the reader ever occupied.
 *
 *  Carrying rather than merely ARMED (see `followIsCarrying`). This is the half
 *  of that test `withScrollAnchor` cannot make alone, and the two answers are
 *  one decision. Nothing at all for an unarmed reader. A tween here was tried
 *  and rejected, see ADR 0064. */
export function honourAnchoredMutation(el: HTMLElement): void {
  carryHeldScroll(el);
  if (!followIsCarrying() || isAtLiveEdge(el)) return;
  snapToLiveEdge(el);
}

/** Is the container still exactly where our last held write left it? The exact
 *  reading of "the reader has not taken over since", per the block above. 1px of
 *  slack absorbs a browser re-rounding a fractional position (zoom, device pixel
 *  ratio) and the iOS repaint nudge's deliberate ±1. */
function isWhereWeHeldIt(el: HTMLElement): boolean {
  return _heldEl === el && Math.abs(el.scrollTop - _heldTop) <= 1;
}

/** Did WE put the reader on the live edge, and are they still there? The app's
 *  own knowledge of where it left them, for the moment before anything has
 *  measured one.
 *
 *  Both terms, and the second is what keeps it from outliving its truth. Growth
 *  moves the edge and never `scrollTop`, so a stamp taken at the edge still
 *  describes a reader who was on it. Anything else that moves the container
 *  fails `isWhereWeHeldIt`, and a correction that moves it deliberately re-takes
 *  the stamp (`carryHeldScroll`). */
function heldOnTheLiveEdge(el: HTMLElement): boolean {
  return _heldAtLiveEdge && isWhereWeHeldIt(el);
}

/** RETIRE the edge claim the moment the container leaves the stamp that made
 *  it. Called by `onScroll`, which is where the module watches it move.
 *
 *  Position equality alone cannot carry the claim, because a number the reader
 *  came BACK to reads exactly like one they never left. An armed reader
 *  browsing an idle thread keeps both their ride and their stamp. The offset
 *  they parked on stays matchable while the transcript grows past it. Wandering
 *  back onto it would then claim an edge a screenful below them.
 *
 *  Only the claim. `_heldEl` and `_heldTop` outlive it, because a scroll that
 *  is not the reader taking over must still read as ours (`isFollowScroll`).
 *  Anything that puts the reader back on the edge re-stamps through
 *  `holdPosition`, so nothing has to be restored by hand. */
function forgetHeldLiveEdge(el: HTMLElement): void {
  if (_heldEl === el && !isWhereWeHeldIt(el)) _heldAtLiveEdge = false;
}

/* ── Was this scroll the reader's own GESTURE? ───────────────────────────────
 *  The position test above answers "did the container move away from our
 *  write". That equals "did the reader take over" only because content growth
 *  changes `scrollHeight` and never `scrollTop`. Three things move the
 *  container with no gesture behind them at all:
 *
 *  - The iOS soft keyboard. Opening or closing it rewrites `--app-height` and
 *    WebKit adjusts the offset ASYNCHRONOUSLY through the animation, well after
 *    any write of ours to stamp it against.
 *  - An app backgrounded and resumed. The PWA restores an offset nobody wrote.
 *  - Anything else the platform scrolls on its own, such as a focus ring
 *    brought into view or a restored session.
 *
 *  So the question is asked of the INPUT instead: a scroll may retire the follow
 *  only while a reader gesture is in flight. The gesture term is ADDED to the
 *  position one, so both must hold. A NAVIGATION is not a gesture and says so
 *  itself, which is what the up chevron, turn stepping and the deep link do.
 *  Answering the question is not ACTING on it: `keepTheLiveEdge` is the other
 *  half, and undoes a scroll that was nobody's gesture. */

/** How long after the reader lifts off their scroll events still count as
 *  theirs. `utils/scrollActivity.ts` already defines `USER_SCROLL_WINDOW_MS` as
 *  the drag plus the momentum tail, for the repaint nudge's own stand-down. The
 *  same question, so the same answer, shared rather than restated. It has to
 *  outlive `touchend` because iOS momentum fires its scroll events after the
 *  finger is gone.
 *
 *  The EVENT SET is deliberately not shared. `utils/userAction.ts` owns "the
 *  user did something" for surfaces that stand down on any interaction, and it
 *  includes `pointerdown`, which is wrong here. A press is how the reader
 *  answers a question or grants a permission, and this signal must survive
 *  both. See `_scrollbarPressEl`. */
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
 *  grants a permission or expands a turn. A mouse drag on content selects text
 *  and scrolls nothing. Recording those too would let the jitter every click
 *  carries reach `pointermove` below and stamp a gesture. That would put the
 *  window over exactly the interactions that must KEEP the follow.
 *
 *  What it buys is the length of a drag. The press itself stamps once, and a
 *  slow haul down the scrollbar can outlast the window. So `pointermove` keeps
 *  the stamp fresh while the thumb is held. */
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
 *  fires movement continuously, so a real one keeps re-stamping. A held-down
 *  term would also be a state that can STICK. A release the page never sees,
 *  such as a touch ending while the PWA is backgrounded, would leave the
 *  reader's hand permanently on the transcript. The first platform scroll after
 *  the resume would then retire the follow. */
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
 *  teardown. NOT exported and NOT called by the view. `makeScrollObservers` owns
 *  it, so the signal and the `onScroll` that consumes it are attached together
 *  by construction. Wiring them separately would make "observers attached,
 *  gestures forgotten" expressible. That state is silent until a reader scrolls
 *  away from a live reply and is dragged back.
 *
 *  `pointermove` is the one that needs the press beside it. On a mouse it fires
 *  for the pointer merely CROSSING the transcript, so stamping it
 *  unconditionally would leave a gesture permanently in flight. Gated on the
 *  press it means a drag, which is how a scrollbar is pulled. `touchmove` needs
 *  no such gate, since a finger on the glass is already a press.
 *
 *  The release goes on `window` rather than the element. A drag ending with the
 *  pointer outside the transcript would otherwise leave the press recorded
 *  forever.
 *
 *  A container with no `addEventListener` is a test double and gets a no-op
 *  teardown; such a test drives the signal through `readerGestureForTest`. So is
 *  a `window` without one. */
function attachReaderGestures(el: HTMLElement): () => void {
  const root = typeof window !== 'undefined' && typeof window.addEventListener === 'function'
    ? window
    : null;
  if (typeof el.addEventListener !== 'function' || !root) return () => {};
  const onDown = (e: PointerEvent) => {
    // A press in the SCROLLBAR gutter is a scroll by itself, with no movement
    // to follow it: clicking the track pages the transcript in one jump. It is
    // also the ONLY press that can be a scroll at all. The gutter is outside the
    // client box, so it cannot be a content control.
    //
    // `e.target === el` is load-bearing, not a tidy-up. `offsetX` is measured
    // from the padding box of the TARGET, and `pointerdown` bubbles from the
    // deepest element under the pointer. On a press landing on a descendant the
    // comparison is therefore against the wrong box: in a turn taller than the
    // viewport, an ordinary tap in the body reads as past the gutter. Only a
    // press on the container ITSELF can be in its gutter.
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
    // never saw: over a nested iframe, or while the PWA was backgrounded.
    // Clearing here stops `_scrollbarPressEl` becoming a state that can STICK,
    // where a mouse merely crossing the transcript reads as a drag.
    if (e.buttons === 0) { if (_scrollbarPressEl === el) _scrollbarPressEl = null; return; }
    if (_scrollbarPressEl === el) stampGesture(el);
  };
  // `wheel` and `touchmove` are stamped WHEREVER in the transcript they land,
  // and the asymmetry with the two gated listeners is the point. Those two can
  // be ACTIVATIONS: a press answers a card, and Space on a focused button is
  // the same act from the keyboard. Neither scrolls anything, so each is gated
  // to the container itself. A wheel notch and a finger travelling are scroll
  // intent whatever they land on. The worst they can be wrong about is a nested
  // scroller, where the reader is still scrolling, just not this box.
  const onMove = () => stampGesture(el);
  const onUp = () => { if (_scrollbarPressEl === el) _scrollbarPressEl = null; };
  const onKey = (e: KeyboardEvent) => {
    // A CHORD is a shortcut, not a scroll key. The two overlap: turn stepping
    // is Cmd+Arrow, and its own keystroke stamping a gesture would defeat the
    // one case `stepThreadTurn` deliberately keeps the ride for, a step onto
    // the last turn, by retiring it from `onScroll` mid-glide instead.
    if (e.metaKey || e.ctrlKey || e.altKey) return;
    if (!SCROLL_KEYS.has(e.key)) return;
    // Only a key the CONTAINER itself received stamps a GESTURE. `keydown`
    // bubbles, and the transcript is full of controls that take these very keys
    // and scroll nothing. Space on a focused button ANSWERS a question card, and
    // Arrow or Home/End move a caret in a text field. Treating those as a scroll
    // would put a disarm window over the interactions the press rule above
    // already refuses.
    //
    // A scroll key on a DESCENDANT can still scroll the transcript. The browser
    // scrolls the nearest scrollable ancestor for any key the focused control
    // does not consume. That is the reader scrolling by keyboard, and it is the
    // ordinary case: the choice-card seeding parks focus on a button inside the
    // transcript. So it is marked as a REVEAL rather than stamped as a gesture,
    // which is the narrow half of the two. The correction stands down, and the
    // disarm's four terms are left as they were.
    //
    // Marked generously, for any unconsumed scroll key wherever it lands. A key
    // the control DOES consume moves nothing and fires no scroll event.
    if (e.target !== el) { markRevealScroll(el); return; }
    stampGesture(el);
  };
  /** FOCUS LANDING somewhere inside the transcript, the OTHER way the container
   *  scrolls with nobody writing `scrollTop`. The browser reveals an off-screen
   *  focused control, as it does for Tab, Shift+Tab, a screen reader moving the
   *  cursor, and any `focus()` without `preventScroll`.
   *
   *  It is a NAVIGATION rather than a gesture, which is why it is stamped here
   *  rather than through `stampGesture`. A gesture retires the ride. Tabbing to
   *  a control is the reader going somewhere specific, so they keep the lit
   *  toggle AND the place the browser took them to.
   *
   *  Without it the platform-scroll correction writes the reader back to the
   *  live edge, leaving the control they just tabbed to off screen. Tab then
   *  appears to do nothing, which a keyboard or screen-reader user has no way
   *  around.
   *
   *  `focusin` rather than `focus` because it BUBBLES, so one listener covers
   *  every control inside the container. It is dispatched during the focus
   *  operation, a frame or more before the scroll event it causes. That puts the
   *  stamp inside `NAV_SCROLL_EVENT_WINDOW_MS` of that event. */
  const onFocusIn = () => markRevealScroll(el);

  el.addEventListener('pointerdown', onDown as EventListener, { passive: true });
  el.addEventListener('pointermove', onDrag as EventListener, { passive: true });
  el.addEventListener('touchmove', onMove, { passive: true });
  el.addEventListener('wheel', onMove, { passive: true });
  el.addEventListener('keydown', onKey);
  el.addEventListener('focusin', onFocusIn, { passive: true });
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
    el.removeEventListener('focusin', onFocusIn);
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
 *  event to dispatch and no listener to dispatch it to. This states the same
 *  fact the listeners record, which keeps "the reader scrolls" one line in a
 *  test instead of a fake event system. Production never calls it.
 *
 *  `moved: false` forgets it. A test asserting the PLATFORM moved the container
 *  needs that. So does the shared reset in a `beforeEach`, so one test's flick
 *  cannot carry into the next. */
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
 *  Strictly the last one, never the last VISIBLE one. A backwards scan for the
 *  newest visible panel would answer with an older message whenever the newest
 *  has no box, and the send's landing would then fire on a turn that is not the
 *  one just sent. The case is real: a second queued follow-up folds itself and
 *  the first into a closed `<details>` group. So an invisible newest panel is
 *  reported as "not there yet" and the landing waits (see
 *  `LANDING_ADDRESSABLE_MS`). */
function lastUserMessage(el: HTMLElement): HTMLElement | null {
  if (typeof el.querySelectorAll !== 'function') return null;
  const panels = el.querySelectorAll<HTMLElement>('.initiator-panel-user');
  const last = panels[panels.length - 1];
  return last && isElementVisible(last) ? last : null;
}

/** The turn around the reader's own newest message, which is what a send waits
 *  on. The landing needs the TURN rather than the panel, because the hold reads
 *  what the agent has drawn into it. */
function lastUserTurn(el: HTMLElement): HTMLElement | null {
  const panel = lastUserMessage(el);
  return (panel?.closest?.(TURN_SELECTOR) as HTMLElement | null) ?? null;
}

/** The transcript's newest turn: the LAST `.chat-exchange`. Continue's
 *  counterpart to `lastUserTurn`, and strictly the last one for the same
 *  reason (an invisible newest turn is "not there yet", never an older one). */
function lastTurn(el: HTMLElement): HTMLElement | null {
  if (typeof el.querySelectorAll !== 'function') return null;
  const turns = el.querySelectorAll<HTMLElement>(TURN_SELECTOR);
  const last = turns[turns.length - 1];
  return last && isElementVisible(last) ? last : null;
}

/** The TURN holding the card whose `attr` is `value`, among the elements
 *  `bodySelector` picks out. A card submit's counterpart to `lastUserTurn`, and
 *  the same question: which turn did the reader act on, and has it got a box.
 *  Two callers, the question card and the three permission-shaped cards.
 *
 *  Matched on the attribute rather than through a `[data-tool-use-id="…"]`
 *  selector, so no id has to be escaped into CSS syntax. Both the live body and
 *  the answered one carry the id, so which is in the DOM does not decide whether
 *  the card is found. `QuestionBody` swaps one for the other a frame in. */
function cardTurn(el: HTMLElement, bodySelector: string, attr: string, value: string): HTMLElement | null {
  if (typeof el.querySelectorAll !== 'function') return null;
  for (const body of el.querySelectorAll<HTMLElement>(bodySelector)) {
    if (body.getAttribute?.(attr) !== value) continue;
    if (isElementVisible(body)) return (body.closest?.(TURN_SELECTOR) as HTMLElement | null) ?? null;
  }
  return null;
}

/** Fallback clearance when the computed `scroll-margin-top` is unavailable (the
 *  DOM-free unit environment, or an element with no layout): ~0.5rem, the base
 *  `--deep-link-focus-gap`. */
const TURN_LANDING_FALLBACK_GAP_PX = 8;

/** THE LANDING LINE: how far below the transcript's top edge a turn comes to
 *  rest when the app navigates to it, read off `.chat-exchange`'s computed
 *  `scroll-margin-top` (chat/response.css).
 *
 *  ONE number for both navigations that put a turn at the top, a deep link and
 *  turn stepping. They must agree, or the same turn would rest in two places
 *  depending on what put it there. A SUBMIT rests at the live edge instead and
 *  reads nothing here (ADR 0080).
 *
 *  It is READ rather than written here because what it clears is a stack of
 *  chrome that differs per breakpoint and is described in CSS. That stack is the
 *  desktop top-fade band and floating up-chevron, the mobile app header and
 *  sticky thread-title row, and the focus highlight's own bloom. Keeping the
 *  number beside the things it measures is why this is not a constant.
 *
 *  Read off the TURN, since that is where the rule is declared. A caller holding
 *  an inner panel resolves its `.chat-exchange` first. */
function turnLandingClearancePx(turn: HTMLElement): number {
  if (typeof getComputedStyle !== 'function') return TURN_LANDING_FALLBACK_GAP_PX;
  const px = parseFloat(getComputedStyle(turn).scrollMarginTop);
  return Number.isFinite(px) && px > 0 ? px : TURN_LANDING_FALLBACK_GAP_PX;
}

/* ── THE TRANSCRIPT RESERVES NOTHING UNDER ITS NEWEST TURN ───────────────────
 *  No `min-height` tail room, and nothing else either. The transcript's height
 *  is a function of its content, and no layout rule reads the follow flag. A
 *  reserved screenful was tried and rejected: see ADR 0064, docs/adr/.
 *
 *  It DOES reserve a bottom PADDING, `--prompt-fade + --nav-focus-reach`. That
 *  is the room the composer dissolve paints over, and it is why a submit rests
 *  on the live edge (ADR 0080). Any rest short of the live edge parks content
 *  inside that band. For a fresh send the content parked there is the row the
 *  reader waits for, the agent's status line. */

/** Put the container ON the live edge in one held write, and reconcile the
 *  chevron behind it. The write can land where the container already was, and
 *  then no scroll event arrives to do that reconcile itself.
 *
 *  The instant half of `glideToLiveEdge`, factored out for two callers that want
 *  exactly it: the reduced-motion branch below, and `honourAnchoredMutation`,
 *  where landing inside the frame IS the point. Neither may drift from the other
 *  on where the live edge is.
 *
 *  It supersedes EVERY tween, including a held one that `glideToLiveEdge`
 *  would have stood down for. That stand-down exists so a second call cannot
 *  restart the easing part-way, and there is no easing here. */
function snapToLiveEdge(el: HTMLElement): void {
  cancelScrollAnim();
  markHeldScroll(el, liveEdgeTop(el));
  syncAwayFromBottom();
}

/** Glide to the LIVE EDGE, marking every frame as a held scroll. It is where
 *  BOTH held glides go: the standing follow's ride, and a submit's landing
 *  (ADR 0080). The target is re-read per frame unless `freeze` says otherwise,
 *  so the tween lands on the bottom the transcript has when it ENDS. That
 *  tracks a reply streaming under it, and it covers a response row mounting a
 *  commit after the row the landing waited for.
 *
 *  `owner` says WHOSE glide this is, and the stand-down is scoped to it. A
 *  second call for the same owner finds its own glide in flight and leaves it
 *  alone, because re-targeting would restart the easing part-way. The
 *  composer's two calls for one send are that case. Every other tween is
 *  superseded, a deep-link's and a chevron's included.
 *
 *  A landing is therefore superseded by a RIDE, through `animateScroll`'s own
 *  cancel, since a caller arming the follow outranks a submit's landing. The
 *  reverse cannot arise: arming drops a pending landing (`armFollowOn`). */
function glideToLiveEdge(el: HTMLElement, owner: HeldGlide, freeze = false): void {
  if (_heldAnim && _heldAnimTarget === owner) return;
  if (prefersReducedMotion()) {
    snapToLiveEdge(el);
    return;
  }
  // The hold's LAST glide stops chasing. Its target is the live edge as the
  // agent's first row left it. A second row arriving inside the tween therefore
  // cannot carry the reader on to that one as well. Every other glide keeps the
  // live target, which is what catches the opening instalments.
  const frozen = freeze ? liveEdgeTop(el) : 0;
  const targetOf = freeze ? () => frozen : liveEdgeTop;
  // Reconcile the chevron on landing for the same reason `scrollToBottomAnimated`
  // does: the last frame can write where the previous one already left the
  // container, and then no scroll event arrives to do it.
  //
  // A LANDING also gives its hold the round the glide swallowed. `honourGrowth`
  // stands down for a tween, so a first row drawn mid-glide never reaches the
  // release check. The hold would then wait for a later resize that a turn
  // going quiet never sends. It cannot spin: a fresh glide needs the live edge
  // to have moved again, which needs real growth. A frozen glide replays into
  // a landing already let go, so its round costs nothing.
  animateScroll(targetOf, () => {
    syncAwayFromBottom();
    if (owner !== 'landing') return;
    const cur = resolveTarget();
    if (cur) honourLanding(cur);
  }, markHeldScroll);
  _heldAnimTarget = owner;
}

/** Take a rider to the live edge, writing nothing where they are already on it.
 *  That half is not tidiness. A redundant tween cancels an iOS momentum scroll.
 *  And a write that moves nothing fires no scroll event for the chevron to
 *  settle on, hence the reconcile instead.
 *
 *  It still SUPERSEDES a tween there, which the glide would have done through
 *  `animateScroll`. A tween in flight is taking the rider somewhere else, and
 *  both callers outrank it: pressing the toggle asks to stay at the bottom, and
 *  a link owns the viewport. Without it an up-chevron glide tapped a frame
 *  earlier still reads as at-the-edge. It survives, and carries the reader to
 *  the top with the link's marker left on a turn at the bottom.
 *
 *  THREE callers, and each has armed the follow on the line above. They are the
 *  toggle's own press, a deep link whose landing IS the live edge, and the
 *  resume over such a landing. Arming stays the caller's act, so this never
 *  turns a navigation into a ride. */
function rideToLiveEdge(el: HTMLElement): void {
  if (!isAtLiveEdge(el)) { glideToLiveEdge(el, 'ride'); return; }
  cancelScrollAnim();
  syncAwayFromBottom();
}

/** THE SUBMIT REACTION. One function, because "same reaction everywhere" is a
 *  structural claim rather than five copies kept in step by hand. A submit is
 *  any user action in the transcript the agent must respond to. The five of
 *  them differ only in `resolveTurn` and in whether they HOLD. It ARMS
 *  NOTHING, per ADR 0064.
 *
 *  EVERY submit rests on the LIVE EDGE, armed or not (ADR 0080). The two
 *  branches below differ only in WHEN. A rider goes at once, since the growth
 *  is about to take them there anyway. Everyone else waits for the turn they
 *  acted on to render. A glide started before it would aim at the bottom the
 *  transcript had BEFORE the submit, which is the blind jump the wait exists
 *  for.
 *
 *  It takes a POSITION STAMP before doing anything. The two deferred submits
 *  move nobody for a frame or more, and the reader's flick in that window must
 *  still cancel them (see `holdPosition`).
 *
 *  A landing that has not yet found its turn is KEPT rather than replaced,
 *  which makes the composer's two calls for one send one submit. See the guard
 *  itself for why the rule stops there. */
function followSubmit(resolveTurn: TurnResolver, holds = true): void {
  // A submit CLAIMS the thread is live, whatever the last turn's status says
  // yet. That is what a submit IS: an act the agent is expected to respond to.
  // The status cannot say so for a while, and the gap is not small. Answering a
  // card leaves the turn on `awaiting-answer`, which is not an ACTIVE status,
  // until the engine's resumed status arrives over SSE. A reader who scrolls
  // away inside that window means "stop dragging me" as much as one who scrolls
  // away mid-reply. A CLAIM rather than a fact, so it expires on its own when
  // the response never comes: see `_submitLiveUntil`. `ChatExchange` supersedes
  // it the instant the real status is known.
  // Gated on HOLDS, because the claim's premise is that the agent will respond.
  // A cancel denies that premise: it asks the agent to finish. Claimed anyway,
  // an ARMED reader who pressed Stop would be carried by every growth round.
  // That runs for the claim's whole length, on a thread they had just quieted.
  if (holds) _submitLiveUntil = nowMs() + SUBMIT_LIVE_CLAIM_MS;
  const el = resolveTarget();
  if (!el) return;
  if (_followingBottom.value) {
    // A rider already ON the live edge needs no write, and gets no redundant
    // tween, which on iOS would cancel a momentum scroll. The armed follow
    // carries them through the turn rendering underneath. The LANDING cannot
    // ask the same question here: its turn does not exist yet, so the live edge
    // it would measure is the one BEFORE the submit.
    if (!isAtLiveEdge(el)) glideToLiveEdge(el, 'ride');
    return;
  }
  // A landing already PENDING keeps the floor, unless it has LAPSED. The two
  // checks are the same rule from opposite sides: the growth branch expires it
  // when a round happens to run, and this expires it when one never does. A
  // queued follow-up is that case. It folds into a closed disclosure group, so
  // it never becomes addressable and nothing else grows the transcript. Without
  // this, the reader's NEXT submit returns on the line below without installing
  // its own wait, and never lands.
  dropLapsedLanding();
  // A HOLD that has not FOUND its turn keeps the floor, and one that has is
  // replaced. The floor is for the composer's two calls. Both fire before any
  // row renders, so a second `awaitsNewTurn` built between them would wait for
  // a message that will never come. Past that, a hold stuck on a turn that
  // draws nothing must not swallow the reader's next submit for the backstop.
  //
  // A ONE-SHOT never keeps it. It has no twin call to protect, and it waits on
  // the long budget. The floor would cost the reader their next landing for
  // all of it.
  if (_pendingLanding?.holds && !_pendingLanding.on) return;
  holdPosition(el);
  // Installed FIRST and then run, rather than tried and then installed. A card
  // is addressable the instant it is tapped. The try-first shape therefore spent
  // the landing before the answer had caused anything, and a reader at the
  // bottom got no write at all. Both paths are one function (`honourLanding`).
  _pendingLanding = { resolveTurn, holds, on: null, at: nowMs() };
  honourLanding(el);
}

/** Waits for a turn the submit is about to CREATE: it snapshots what `newest`
 *  answers NOW and resolves only once that answer changes. Both deferred
 *  submits have this shape, and differ only in what "newest" means: the
 *  reader's own message row for a send, the last turn for Continue. */
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
  followSubmit(awaitsNewTurn(lastUserTurn));
}

/** The reader submitted an answer to the question card `toolUseId`. The card is
 *  on screen already, so this landing needs none of the send's deferral.
 *
 *  Called by the two card-submitted answers: `QuestionCard`'s single-select
 *  option tap and `PromptInput`'s multi-select Submit. The THIRD way to answer,
 *  typing into the composer, is a send the engine reroutes as a `FreeText`
 *  answer, so it arrives through `followSentMessage`. */
export function followAnsweredQuestion(toolUseId: string): void {
  followSubmit((el) => cardTurn(el, '.question-body', 'data-tool-use-id', toolUseId));
}

/** The reader decided the permission card `requestId`, on any of the three
 *  permission-shaped cards: the coding-agent tool permission, the command guard
 *  and the MCP tool consent. All three decide through `PermissionCard`'s
 *  `usePermissionDecide`, so all three inherit this from one call site.
 *
 *  A submit like the others, because from the reader's side they are all the
 *  same act. The card is on screen already, so like the answer it glides now. */
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

/** The reader pressed CANCEL on the prompt row. One control, two acts, and both
 *  are submits: cancelling a pending question or permission card, which the
 *  agent resumes from, and STOPPING a running turn, which ends it and draws a
 *  terminal boundary. The reader asked for one reaction to both.
 *
 *  `toolUseId` names the question card when one was pending, so the landing
 *  resolves the same turn `followAnsweredQuestion` would. Without it the act
 *  was a stop, or a permission card, and the turn is the transcript's last:
 *  both are the turn the agent is parked on or running.
 *
 *  Called by `PromptInput`'s `cancelExchangeForTarget`, AFTER its queued-upload
 *  early return. Cancelling an upload that never left sends the agent nothing,
 *  so it is not a submit. */
export function followCanceledTurn(toolUseId?: string): void {
  // NEVER a hold, in either shape. A cancel asks the agent to FINISH, so no
  // first row is coming to release on. `cancel_chat` resolves a pending
  // question as Canceled and then fires the cancel token, and a Stop ends the
  // turn outright. Held, both would run out the whole backstop.
  //
  // They differ in the turn to land on. A cancelled QUESTION card keeps its own
  // turn and grows no boundary, since `exchange-grouping.ts` skips one for a
  // question resolved as Canceled: the card's own button carries the
  // attribution. So that landing resolves the card, which is on screen already.
  //
  // Everything else opens a boundary exchange, a permission card's cancel
  // included, so its landing waits for that to render. Continue waits for its
  // continuation the same way. The caller passes the id only while the thread
  // really is awaiting an answer, since `findLatestPendingQuestion` has no
  // liveness term of its own.
  followSubmit(
    toolUseId
      ? (el) => cardTurn(el, '.question-body', 'data-tool-use-id', toolUseId)
      : awaitsNewTurn(lastTurn),
    false,
  );
}

/** THE SUBMIT'S LANDING: take the reader to the live edge, where a rider's own
 *  glide rests too (ADR 0080). Run once the turn the submit was made on has a
 *  box, so the edge being measured already includes it.
 *
 *  The live edge and not the turn's own bottom edge. The block above says what
 *  the difference is: the transcript's bottom padding, which is the room the
 *  composer dissolve paints over. Resting on the turn parks its last row inside
 *  that band, and on a fresh send that row is the agent's status line.
 *
 *  A reader ALREADY on the edge gets no write, on the module's one threshold.
 *  This branch and the rider's therefore ask the same question the same way.
 *  Writing anyway would buy a tween of a pixel or two, and cancel an iOS
 *  momentum scroll for it.
 *
 *  That also refuses BACKWARDS for free, since `scrollTop` never exceeds the
 *  live edge. A queued follow-up rendering ABOVE where the reader is standing
 *  therefore moves nobody at all.
 *
 *  It is the LANDING half of `glideToLiveEdge`, so the reader's own scroll can
 *  cancel it while a ride survives one. See `_heldAnimTarget`. */
function landAtLiveEdge(el: HTMLElement, freeze = false): void {
  if (isAtLiveEdge(el)) return;
  glideToLiveEdge(el, 'landing', freeze);
}

/** ONE ROUND OF THE HOLD: re-aim at the live edge, then decide whether the
 *  reader has seen the agent start. Run at submit time and on every growth
 *  round after it, so the two paths cannot answer differently.
 *
 *  Nothing to land on yet means the turn has no box, and the landing waits it
 *  out on the shorter of the two deadlines. The aim writes nothing when there
 *  is nowhere to go, so a round costs a reader at the bottom no motion at all.
 *
 *  FOUR ways it lets go, and only the first is what a submit is FOR. The turn
 *  has drawn a row it did not have, so the agent has started. Or the submit
 *  never held at all, which is a CANCEL asking the agent to finish. Or it is a QUEUED
 *  follow-up and will never draw one. Or the backstop elapses. The reader's own
 *  scroll is the fifth and lives in `onScroll`, through `cancelLanding`.
 *
 *  A queued turn still gets its ONE aim first, because the reader submitted.
 *  The ending round's aim is FROZEN, which is the whole of the difference
 *  between resting on the agent's first row and being carried past it.
 *
 *  The row count is compared for CHANGE, not growth. `getCollapsedVisibleEvents`
 *  drops earlier prose when a new text block arrives. A turn's count can
 *  therefore go DOWN as the agent draws, and a greater-than would never fire. */
function honourLanding(el: HTMLElement): void {
  // The lapse is read FIRST, so a hold past its backstop moves nobody on the
  // way out. Asked after the aim, the round that noticed it would still write.
  dropLapsedLanding();
  const landing = _pendingLanding;
  if (!landing) return;
  if (!landing.on) {
    const turn = landing.resolveTurn(el);
    if (!turn) return;
    // A landing that never holds is done the moment its turn is there: aim
    // once, frozen, and let go. It takes no row snapshot, because it has no
    // use for one, so `on` is only ever populated for a real hold.
    if (!landing.holds) { _pendingLanding = null; landAtLiveEdge(el, true); return; }
    landing.on = { turn, drawnAtStart: drawnRows(turn) };
  }
  const { turn, drawnAtStart } = landing.on;
  // A turn that has LEFT the layout can draw nothing, so there is nothing left
  // to wait for. Without this the hold would sit out its whole backstop on a
  // node nobody can see.
  if (turn.isConnected === false) { _pendingLanding = null; return; }
  // DECIDED before it aims, so the ending round can aim differently. Its glide
  // freezes, resting the reader where the agent's first row put them rather
  // than chasing the next one into the same tween.
  const ending = turnIsQueued(turn) || drawnRows(turn) !== drawnAtStart;
  landAtLiveEdge(el, ending);
  if (ending) _pendingLanding = null;
}

/** What one growth round owes the reader, which is at most ONE of two things. A
 *  submit's landing is in hand, so give it its round. Or the follow is CARRYING
 *  them to the live edge, so write it. Never both,
 *  because a submit arms nothing and arming drops a pending landing
 *  (`armFollowOn`). Nothing at all for a reader who asked for neither.
 *
 *  Stands down while a navigation tween owns the scroll, the landing glide
 *  included. A tween re-reads its own target every frame, so a write beside it
 *  would fight the easing rather than help it.
 *
 *  TWO ARMS, and which one answers depends only on where the reader is. A reader
 *  who has SCROLLED AWAY is carried only while the agent is live
 *  (`followIsCarrying`). A reader ON the live edge is kept there either way, and
 *  that arm is not this function's to state: growth is the third event reaching
 *  `keepTheLiveEdge`, beside a box change and a platform scroll, so the caller
 *  hands it in.
 *
 *  `keepEdge` is omitted by a caller with no pre-round reading of where the
 *  reader was, which is `honourWake`. A wake has just declared the thread live,
 *  so the arm above answers for a rider too. */
function honourGrowth(el: HTMLElement, keepEdge?: () => boolean): void {
  if (_scrollAnimRaf !== null) return;
  if (_pendingLanding) { honourLanding(el); return; }
  if (followIsCarrying()) {
    markHeldScroll(el, liveEdgeTop(el));
    return;
  }
  keepEdge?.();
}

/** rAF easeOutCubic scroll of the active container toward a target, shared by
 *  every transcript navigation. ADR 0065 (docs/adr/) is the shape and why.
 *
 *  - `targetOf(el)` is re-read EVERY frame. The eased fraction is applied
 *    between the captured `start` and the LIVE target. A target that grows
 *    mid-tween is therefore tracked, and the curve still lands cleanly at t=1.
 *  - `start` is captured on the FIRST frame, after the render-all
 *    scroll-anchoring shift has settled. Thereafter the position is a pure
 *    function of elapsed time and `scrollTop` is never READ again, only written.
 *    There is no yield guard, so an explicit chevron tap always reaches its
 *    target.
 *  - `duration` scales with the initial distance, clamped. The tween ends
 *    precisely at the target on the t>=1 frame, then `onDone` runs.
 *  - `scrollTop` is written FRACTIONAL, with no `Math.round`.
 *  - `mark` records each frame's write. It defaults to `markNavigationScroll`,
 *    which is right for a one-off navigation. The two held glides pass
 *    `markHeldScroll`, so their own frames are not read as the reader taking
 *    over. */
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
      // Back-date startTime by one nominal frame, so the FIRST painted frame
      // already has a frame's worth of eased progress rather than sitting at
      // t=0. A constant time offset, not a per-frame fraction, so the
      // deceleration shape stays elapsed-time based.
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

/** WHERE `el` comes to rest: the absolute `scrollTop` putting its top at the
 *  container top, minus its CSS `scroll-margin-top`. That margin is the deep
 *  link's header and fade clearance, per breakpoint in chat/response.css.
 *  Reading the resolved px is what rests the element on the shared landing line.
 *
 *  Returned as a per-container closure, because the tween re-reads it every
 *  frame. The margin is resolved once: it is a breakpoint-level constant.
 *
 *  ONE definition, and that is load-bearing rather than tidy. A deep link both
 *  SCROLLS to this number and decides the ride's fate by it (see `tryResolve`).
 *  Two copies could disagree about where the reader is going to end up. */
function landingTargetOf(el: HTMLElement): (c: HTMLElement) => number {
  const marginTop =
    typeof getComputedStyle === 'function'
      ? (parseFloat(getComputedStyle(el).scrollMarginTop) || 0)
      : 0;
  return (c: HTMLElement) =>
    typeof c.getBoundingClientRect === 'function' && typeof el.getBoundingClientRect === 'function'
      ? el.getBoundingClientRect().top - c.getBoundingClientRect().top + c.scrollTop - marginTop
      : 0;
}

/** Scroll the active container so `el` rests on the landing line: the
 *  "navigation to element" motion for a notification or Changes deep link.
 *
 *  It runs on the shared `animateScroll`, so the deep link and the chevrons
 *  scroll identically (ADR 0065). The target is recomputed each frame. An
 *  element still growing as markdown or images render is therefore tracked, and
 *  so is the whole transcript re-anchoring after a render-all. Reduced motion
 *  jumps instantly. */
function smoothScrollToElement(el: HTMLElement): void {
  const targetOf = landingTargetOf(el);
  if (prefersReducedMotion()) {
    cancelScrollAnim();
    const c = resolveTarget();
    if (c) markNavigationScroll(c, Math.max(0, targetOf(c)));
    return;
  }
  animateScroll(targetOf);
}

/** Smoothly scroll the active chat container to the VERY top: the up chevron's
 *  action. Two things beyond the shared tween:
 *
 *  1. **The chevron.** At the top the reader is definitively away from the
 *     bottom, so the signal is set here rather than on the first scroll event.
 *     That keeps the down chevron on from the first frame of the glide.
 *  2. **Reduced motion** jumps, with one rAF re-assert to defeat an iOS no-op or
 *     a late render-all ResizeObserver settle.
 *
 *  A manual top jump also supersedes any in-flight deep-link claim. The link
 *  owns the viewport until it settles, and this is the user saying otherwise. */
export function scrollToTop() {
  clearPendingEventScroll();
  awayFromBottom.value = true;

  const el = resolveTarget();
  if (!el) return;

  // Going to the top ends the ride, and the press has to say so ITSELF. A
  // scroll only speaks for the reader when a gesture is behind it. A chevron
  // tap lands on the button rather than on the transcript. See "Was this
  // scroll the reader's own GESTURE?".
  //
  // Only while the agent is LIVE, matching the scroll disarm. Going back to
  // re-read an idle thread is browsing, and the lit toggle says on screen that
  // the ride survived.
  //
  // After the target resolves, so a press with no transcript to move retires
  // nothing.
  if (threadIsLive()) stopFollowingBottom();

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

/** Smoothly scroll the active chat container to the bottom: the down chevron's
 *  action, and NOTHING more than that. It arms no standing follow, which is the
 *  follow toggle's request alone (ADR 0064).
 *
 *  Eases to the bottom, re-reading the target every frame. A thread that keeps
 *  streaming during the glide is therefore tracked, and the tween lands on the
 *  TRUE grown bottom.
 *
 *  Reduced motion skips straight to `scrollToBottom`'s instant jump. */
export function scrollToBottomAnimated() {
  clearPendingEventScroll();
  const el = resolveTarget();
  if (!el || prefersReducedMotion()) { scrollToBottom(); return; }
  // `liveEdgeTop` (the MAX scroll position), not `scrollHeight`, so the ease
  // lands exactly at the bottom instead of clamping flat for the last screenful.
  animateScroll(
    liveEdgeTop,
    // The landing write may not move the container at all (the tween's last
    // frame can already be there), and then no scroll event fires to reconcile
    // the chevron. Settle it here against the real position instead.
    syncAwayFromBottom,
  );
}

/** Set the standing follow. This is the FOLLOW TOGGLE's whole behaviour, and the
 *  only way to arm one. `resumeFollowingBottom` aside, which replays a request
 *  recorded in this thread and can only ever replay one of these.
 *
 *  ON glides to the live edge and arms, so a reader anywhere in the transcript
 *  is one tap from following. OFF disarms and writes NO SCROLL: turning a mode
 *  off is not a request to be moved.
 *
 *  Turning it off is a CONVENIENCE rather than the mechanism. The reader's own
 *  scroll already retires the follow, and the button follows it off, because
 *  both render this one signal. */
export function setFollowLiveEdge(on: boolean): void {
  // Remember the press first, and on BOTH edges: turning the mode off is as much
  // a standing choice as turning it on, and the off edge returns early below.
  recordFollowSeed(on);
  if (!on) { stopFollowingBottom(); return; }
  clearPendingEventScroll();
  const el = resolveTarget();
  if (!el) return;
  armFollowOn(el);
  rideToLiveEdge(el);
}

/** Jump the transcript to the bottom in one write: the reduced-motion form of
 *  the down chevron, and the compose view's chevron, which has no windowed
 *  render to glide through. Arms nothing, as the animated form does not.
 *
 *  Always an EXPLICIT gesture, so it supersedes any in-flight deep-link claim.
 *  Nothing in the app calls it on the user's behalf. A submit does not either,
 *  though it rests in the same place: its landing WAITS for the turn it was made
 *  on, where this jumps now (see `followSubmit`). */
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

/** How long a deep link waits for its target to appear before giving up. It is
 *  the budget for waiting out ONE lazily loading thread. Exported so a caller
 *  with its own pre-navigation wait on the same thread spends the same budget
 *  rather than a second literal that drifts. `showEventWhereItLives` is that
 *  caller, resolving the anchor before it focuses. */
export const EVENT_RESOLVE_DEADLINE_MS = 4000;
/** How long to keep the deep-link claim alive after a SYNCHRONOUS resolve. A
 *  fallback for browsers where `scrollend` is unsupported or unreliable, and
 *  released earlier if `scrollend` fires first. `smoothScrollToElement`'s tween
 *  settles within `SCROLL_MAX_MS`, and this covers it generously, so competing
 *  scrolls keep deferring across the whole glide. */
const SCROLL_SETTLE_FALLBACK_MS = 1000;

/** The notification deep link currently resolving a scroll, or null when none is
 *  in flight. Identified by a fresh OBJECT per call, never by what it is
 *  scrolling to. Two taps on the SAME notification inside the resolve window
 *  produce the same target. A target-keyed claim therefore let the FIRST call's
 *  deadline mistake the second call's claim for its own. Object identity cannot
 *  collide, so "is the claim still mine" is exact.
 *
 *  A plain mutable variable rather than a signal, read imperatively.
 *
 *  What it guards:
 *   - `useScrollMemory.shouldRestore`, so focusing an UNfocused thread does not
 *     land on the saved position instead of the deep-linked event. Its restore
 *     observers fire on the same lazily-loaded render the link is waiting for,
 *     and would otherwise win by running last.
 *   - `useScrollMemory`'s no-save reset to the top, for the same reason.
 *   - the navigation focus marker's settle guard, so this link's own smooth
 *     scroll cannot dismiss the highlight it just applied.
 *
 *  Held until the deadline, or until the scroll settles on a synchronous
 *  resolve, so it covers the whole landing rather than just the call. */
let _pendingEventScrollClaim: object | null = null;

/** True while a notification deep-link is waiting for its target event to
 *  render (or to scroll to it), so a competing scroll defers to it. */
export function hasPendingEventScroll(): boolean {
  return _pendingEventScrollClaim !== null;
}

/** Subscribers notified when a deep link TAKES the claim, and never when it
 *  releases one. Same shape and same asymmetry as `onFollowArmed`: the claim is
 *  the request, and a release is not a second request anyone needs to hear.
 *
 *  One consumer: `attachScrollMemory`, which retires a saved-position restore
 *  already armed when the claim arrived. Asking `hasPendingEventScroll` once at
 *  attach cannot answer for a claim taken LATER, and two ordinary orderings
 *  reach exactly that. A link into the thread the reader is already in
 *  re-attaches nothing. A thread whose events arrive while the tap is resolving
 *  attaches before the claim. Nor can the restore re-ask: the claim is released
 *  within a second of a synchronous landing, while the restore stays armed for
 *  three. */
const _deepLinkClaimListeners = new Set<() => void>();

/** Subscribe to the claim. Returns the unsubscribe. Fires on every claim,
 *  including one taken while another is live: a second notification tapped
 *  mid-flight is a second navigation request, not a continuation of the first
 *  (the same reason the claim is an OBJECT rather than its target). */
export function onDeepLinkClaimed(listener: () => void): () => void {
  _deepLinkClaimListeners.add(listener);
  return () => { _deepLinkClaimListeners.delete(listener); };
}

/** Subscribers notified when a deep link FINDS its target, the moment it stops
 *  being a link that might be dead. The other half of the pair above. Together
 *  they are what a listener needs to tell a link that landed from one that never
 *  did.
 *
 *  One consumer: `attachScrollMemory`'s dead-link rescue, which stood down for
 *  the claim and would otherwise INFER the landing from the container having
 *  moved. That inference is wrong whenever the landing had nowhere to move, and
 *  the case is ordinary. Arriving in a shorter thread clamps the shared
 *  container to its bottom, and a link to that thread's last turn resolves to
 *  the same offset. The rescue would then read a successful landing as a dead
 *  link, and haul the reader off the event they are looking at. */
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
 *  ordering. Take a tap into a thread the reader is not in. The target renders
 *  and resolves on the microtask checkpoint of the commit that rendered it,
 *  while Preact defers the subscribing effect past that checkpoint. The listener
 *  does not exist yet when the resolve fires, and `resolved` latches, so no
 *  later broadcast follows. A subscriber arriving mid-flight has to be able to
 *  ASK what it missed.
 *
 *  False whenever no claim is held, so it can never describe a link that is
 *  already over. */
let _pendingEventScrollResolved = false;

export function deepLinkHasResolved(): boolean {
  return _pendingEventScrollClaim !== null && _pendingEventScrollResolved;
}

/** Did the LAST LANDING rest somewhere other than the live edge? Reset with each
 *  new claim, beside `_pendingEventScrollResolved`.
 *
 *  The last landing rather than the live claim's, because the write is ungated
 *  exactly as the retirement it describes is. A superseded call still lands and
 *  still acts on the ride, and the reader ends up where IT put them.
 *
 *  It records the PLACE rather than the retirement, and the difference decides a
 *  real case. `focusThread` has already retired the ride on a cross-thread tap,
 *  before the link resolves. So the landing retires nothing there, however high
 *  in the transcript it rests. Reading the retirement would call a link to the
 *  newest turn ride-ending. The resume below would then leave the reader at the
 *  bottom with the toggle dark. */
let _pendingEventScrollLandedOffEdge = false;

/** Did a landing rest off the live edge? The state a resume must not overwrite:
 *  there the landing named a place, and the ride ended on purpose. A landing AT
 *  the live edge names the ride's own place, so a recorded ride is resumed over
 *  it.
 *
 *  It asks the flag ALONE, and adds no `deepLinkHasResolved()` term. The flag is
 *  false until something lands, so the resolve term buys nothing. It also takes
 *  the answer away in one real case. A superseded call can land off the edge
 *  while the NEWER claim is still resolving, and that landing retires the ride.
 *  The newer claim has no resolve to report, so the resume would re-arm what the
 *  landing just ended. A newer link that then turns out dead leaves the reader
 *  following from the older one's event. */
function deepLinkLandedOffLiveEdge(): boolean {
  return _pendingEventScrollLandedOffEdge;
}

/** While true, the mobile hide-on-scroll header stays pinned fully visible (see
 *  `useHideOnScroll.onScroll`). The deep-link scroll lands the element at the
 *  container top minus its `scroll-margin-top`, which adds the fixed app header
 *  and sticky thread-title row back as a STATIC value. That is only correct if
 *  the header's visible portion is deterministic. Without the pin, the smooth
 *  scroll half-hides the header mid-flight and the landed event is partly
 *  covered.
 *
 *  Held for a short window covering the smooth scroll, not the full deep-link
 *  claim, so normal hide-on-scroll resumes the moment the reader reads on.
 *  Desktop ignores it, since its thread-title header is a sibling above the
 *  scroll container. */
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

/** Scroll the element matching `selector` into view and pulse-highlight it, for
 *  the two deep-link wrappers below. The target may not be in the DOM yet (the
 *  thread is lazy-loading), so a MutationObserver retries until it appears or
 *  the deadline fires. Only a target with a box is taken.
 *
 *  `selector` resolves the SCROLL target: a `.chat-exchange`, or an addressable
 *  card inside one. Both carry the landing `scroll-margin-top`, see
 *  chat/response.css.
 *
 *  `pulseTarget`, when given, narrows the PULSE to a descendant of that target,
 *  so a sibling panel in the same exchange is not highlighted too. A string is a
 *  plain descendant selector. A function picks the descendant PER MATCHED
 *  TARGET, which a change deep link needs: its body sits in `.response-panel` on
 *  a proposing turn and in `.initiator-panel` on a resolution card. When the
 *  chosen descendant is absent the pulse falls back to the whole target. */
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
  // `_pendingEventScrollClaim`). Held until the deadline below, and NOT released
  // the instant the scroll lands. The same render that finally shows the event
  // also wakes `useScrollMemory`'s restore observers, which would otherwise land
  // the saved position over this one.
  _pendingEventScrollClaim = claim;
  _pendingEventScrollResolved = false;
  _pendingEventScrollLandedOffEdge = false;
  // Announce the claim to anything holding a positioning decision of its own,
  // which is `useScrollMemory`'s saved-position restore (see
  // `onDeepLinkClaimed`). Notified AFTER the slot is set, so a listener asking
  // `hasPendingEventScroll` sees this claim rather than the state before it.
  for (const listener of _deepLinkClaimListeners) listener();
  // Force ThreadView to render the FULL exchange list, so a windowed-out target
  // can render for `tryResolve` and the MutationObserver to find. It stays true
  // until the claim releases. ThreadView keeps the thread fully rendered
  // afterwards, so the reader is not snapped back to the windowed tail mid-read.
  deepLinkRenderAll.value = true;

  // Release this call's claim, but only if it is still ours. A second deep link
  // started mid-flight has overwritten the slot, and its own release handles
  // that one. Without the guard, this call's deadline would clear the newer
  // claim and un-guard the saved-scroll restore over a live landing. It would
  // also report a failure for a navigation that succeeded.
  const releaseClaim = () => {
    if (_pendingEventScrollClaim === claim) {
      _pendingEventScrollClaim = null;
      _pendingEventScrollResolved = false;
      _pendingEventScrollLandedOffEdge = false;
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
  // SYNCHRONOUS resolve path, the already-focused thread whose events are
  // already in the DOM. Releasing the claim when `smoothScrollToElement` is
  // CALLED is wrong: the tween lands up to `SCROLL_MAX_MS` later, and a
  // competing scroll in that window would override the landing. Release on the
  // container's `scrollend`, which our per-frame writes still fire when they
  // stop, or on a fallback timer where that is unsupported. The async path
  // already holds the claim until its own deadline.
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
        // An event id has one visible copy, so the first wins. A change id can
        // match BOTH the proposing coding-agent turn and its later
        // Applied/Reverted resolution card. `preferLast` lands on the card,
        // which is the turn the user means when reopening the change.
        if (!preferLast) break;
      }
    }
    if (!target) return;
    resolved = true;
    // Stop watching the DOM, but keep the pending claim alive until the deadline
    // so the post-resolve re-renders stay suppressed.
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
    // WHERE THIS LANDING RESTS decides the ride's fate, and it is measured
    // before anything moves. `landingTargetOf` is the number the scroll below
    // aims at, so the two cannot answer differently.
    const container = resolveTarget();
    const landsOnTheEdge =
      !!container && isLiveEdgeTop(container, landingTargetOf(target)(container));
    if (container && landsOnTheEdge && _followingBottom.value) {
      // The link and the ride ask for the same place, so there is nothing to
      // retire. The reader tapped a notification pointing at the newest turn,
      // and the ride was already holding them there.
      //
      // Served by the RIDE's own motion rather than by the element tween. Both
      // rest in the same place, the browser clamping the element's target back
      // to the edge. Only this one marks its frames as HELD, which is what keeps
      // `isFollowScroll` true and records the live edge as the reading position.
      // It also re-reads a growing bottom per frame, where the element's own top
      // stops being the edge the moment the reply resumes.
      rideToLiveEdge(container);
    } else {
      // Going to a link ends the ride: the reader asked to be at THIS place, and
      // the standing follow would carry them off it on the next token. Retired
      // BEFORE the scroll below, so the landing's own frames are marked as a
      // plain navigation. The reading position recorded for this thread is then
      // the offset the link landed on.
      //
      // Not gated on the CLAIM being ours, because the two describe one landing
      // and must not disagree. A superseded call still lands, since the pin, the
      // scroll and the pulse are all ungated. Gating this way would leave a
      // reader who was just moved still following.
      //
      // Not gated on LIVE either, unlike the scroll disarm, the up chevron and
      // turn stepping. Those three are browsing when the thread is quiet, so
      // keeping the ride costs the reader nothing. A LINK names one event, and
      // that ask is durable rather than a moment's position, so it must survive
      // the thread waking. See ADR 0064 for the gate this replaced.
      stopFollowingBottom();
      smoothScrollToElement(target);
    }
    // Record and announce the landing AFTER the scroll above. Two things read
    // this, and the second is why the order matters. One wants to know the link
    // is no longer a candidate for the dead-link rescue. The other records WHERE
    // it landed as the thread's reading position. Under reduced motion the line
    // above IS the whole landing, so announcing first would hand that recorder
    // the position the reader was leaving.
    //
    // WHERE the landing rested is recorded UNGATED, exactly as the retirement
    // above is, and for the one reason: they are two halves of one decision and
    // must not disagree. A superseded call still lands, and still acts on the
    // ride. A flag it declined to write would describe the newer link while the
    // reader sat at the older one's target. The resume would then reinstate a
    // ride that landing had just ended.
    _pendingEventScrollLandedOffEdge = !landsOnTheEdge;
    // The RESOLVE is gated, unlike the line above, because it is about the
    // CLAIM rather than the landing. A superseded call keeps observing until
    // its own deadline. Letting its late resolve speak for a newer link's claim
    // is the collision the claim is an object to prevent.
    if (_pendingEventScrollClaim === claim) {
      _pendingEventScrollResolved = true;
      for (const listener of _deepLinkResolvedListeners) listener();
    }
    // Highlight only the subject panel, never a sibling panel in the same turn.
    // Resolve the requested descendant and pulse it when present, else the whole
    // target. `?.` so jsdom test fakes lacking `querySelector` fall back.
    const pulseChild =
      typeof pulseTarget === 'string'
        ? (target.querySelector?.(pulseTarget) as HTMLElement | null)
        : pulseTarget?.(target) ?? null;
    const pulseEl = pulseTarget ? pulseChild ?? target : target;
    // Apply the shared navigation focus marker (components/shared/focusMarker):
    // a sticky background highlight that stays until the user takes any action,
    // never dissolving before its hold has elapsed. The settle guard defers the
    // dismissal while this link's own smooth scroll is settling, so the landing
    // scroll cannot self-clear the marker.
    applyNavFocus(pulseEl, { settleGuard: hasPendingEventScroll });
  };

  tryResolve();
  if (resolved) {
    // Synchronous resolve: the thread's events were already in the DOM, so no
    // async load follows. Do NOT release the claim synchronously. The deep-link
    // scroll is still settling, and a competing scroll would override the
    // landing, so hold the claim until it settles.
    holdClaimUntilScrollSettles();
    return;
  }

  // `body`, not `.thread-content`. Preact's positional diff cannot preserve the
  // loading-branch `.thread-content` across ThreadView's loading to loaded swap,
  // so a scoped observer strands on a detached node. The hot-path filter
  // re-queries only when a mutation adds a node containing the target. Without
  // it, every streaming token triggers a document-wide scan.
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
  // Deadline. Two outcomes reach it, and only one is a failure.
  //
  //  - The target DID render and `tryResolve` landed on it. The timer then
  //    exists purely to release the claim.
  //  - The target NEVER rendered: it is not in this thread, it renders nothing,
  //    or the thread was still loading when the window closed. That is a dead
  //    deep link, and the user is told through the caller's `onUnresolved`.
  //    `scrollState` stays free of the `store` import, see `parseNavigatedTurn`.
  //
  // It reports WITHOUT moving the transcript. The user asked to go to a place,
  // the place does not exist, and the bottom is not it.
  //
  // A deep link superseded mid-flight (`wasOurs` false) reports nothing. The
  // newer one owns the claim, the viewport and the outcome.
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
  /** Called when the deep link's target never renders and the resolve deadline
   *  expires: a dead link the user has to be told about. The MESSAGE is the
   *  caller's, because `scrollState` stays free of the heavy `store` import
   *  where `showToast` lives. See `parseNavigatedTurn`.
   *
   *  The words are the WHOLE recovery: a dead link leaves the transcript exactly
   *  where it was. */
  onUnresolved?: () => void;
}

/** Land on the element carrying `data-event-id`: a notification deep link
 *  scrolling to the exact event that raised it. Two shapes of match, and the
 *  pulse scope differs per match, so the scope is resolved as a function.
 *
 *   - **An exchange-start event** (`UserQuestionAsked`,
 *     `CodingAgentPermissionRequest`, `CredentialRequested`, and so on) stamps
 *     the whole `.chat-exchange`. Narrow the pulse to its `.initiator-panel`, so
 *     the agent response in the same turn is not highlighted too.
 *   - **A step-level event** stamps the specific card that renders it, today the
 *     `ResponseFailed` failure card. That element already IS the subject, so it
 *     must NOT be narrowed. There is no `.initiator-panel` inside it, and
 *     narrowing would highlight an unrelated descendant.
 *
 *  Discriminating on the match keeps the intent explicit. The `?? target`
 *  fallback still covers a degenerate exchange with no `.initiator-panel`.
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

/** Land on the chat exchange carrying `data-change-id`: the Changes panel deep
 *  linking a row to its change. `ChatExchange` stamps both the proposing
 *  coding-agent turn and any later resolution card with the same id.
 *  `preferLast` resolves to the resolution card when present, and to the
 *  proposing turn for a pending change.
 *
 *  The pulse is scoped to the panel that HOLDS the change, so a sibling panel in
 *  the same turn is not highlighted. Which panel that is depends on the card
 *  type, so the scope is resolved per matched target:
 *   - proposing turn: `.response-panel`, where the `ChangeProposed` step lives,
 *     NOT the user message that started the turn;
 *   - resolution card: `.initiator-panel`, which carries the change body and the
 *     Diff and Revert actions, NOT any folded-in post-apply continuation work
 *     that renders in a `.response-panel` (`changePanelHasContinuation`).
 *  A resolution card is recognised by its `initiator-panel-change-*` accent
 *  class. When the chosen panel is absent the pulse falls back to the whole
 *  target.
 *
 *  Takes the same `onUnresolved` as the event deep link, and through the same
 *  deadline, so both get the identical recovery with no second implementation. */
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
  _pendingEventScrollLandedOffEdge = false;
  deepLinkRenderAll.value = false;
  // A plain focus or explicit scroll is deliberate engagement elsewhere, so drop
  // any persistent focus marker rather than let it leak onto the next thread.
  clearNavFocus();
}

/* ── Turn-by-turn keyboard traversal ─────────────────────────────────────────
 *  The turn-step shortcuts move the transcript one *turn* at a time. They land
 *  the previous or next `.chat-exchange` on the shared landing line and mark it
 *  with the navigation focus marker. It pairs with the focusable
 *  `.thread-content` region: a jump also moves DOM focus into the container, so
 *  the native Arrow and Page keys keep scrolling from there. */
const TURN_SELECTOR = '.chat-exchange';
/** Small slack around the landing line, so a re-press does not re-select the
 *  just-landed turn and subpixel rounding is absorbed. Small and FIXED, never
 *  scaled by the clearance. The gap is folded into the reference position, as
 *  `scrollTop + gap` in `stepThreadTurn`. A larger clearance must not widen the
 *  skip band, or short adjacent turns become unreachable by stepping. */
const TURN_NAV_THRESHOLD_SLACK_PX = 4;

/** Pure pick of the turn to jump to. `tops` are each turn's top in the
 *  container's scroll coordinate space, ascending in DOM order, and `scrollTop`
 *  is the current position. Forward is the first turn below
 *  `scrollTop + threshold`, backward the last turn above `scrollTop - threshold`.
 *  Returns `null` when there is nowhere to go. Exported for unit testing. */
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
 *  `anchorIdx >= 0` means the nav focus marker sits on a listed turn, so the
 *  user has NOT scrolled since the last turn-nav: any real scroll gesture fades
 *  the marker. Step by INDEX from it. That is what makes a cluster of turns
 *  sharing a clamped scroll position reachable. Collapse the last turn, and it
 *  sits with an appended "Change applied" card in the last viewport, where there
 *  is no scroll room. Pure scroll-position stepping keys off a pinned
 *  `scrollTop` there and cannot tell them apart.
 *
 *  With no anchor, fall back to scroll-position stepping via `pickTurnIndex`. It
 *  handles the first press from the current scroll, and the mid-turn read where
 *  "prev" snaps to the current turn's top. Both happen precisely when there is
 *  no marker. Returns null at the list end. Pure, exported for unit testing. */
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
 *  landing it on the landing line and marking it with the navigation focus
 *  marker. A deliberate jump, so like `scrollToTop` it supersedes any in-flight
 *  deep-link claim.
 *
 *  `awayFromBottom` is reconciled against the ACTUAL landing target rather than
 *  assumed. A jump that lands at the bottom must hide the chevron. The last
 *  turn's clamped target often cannot move an already bottomed container, so no
 *  scroll event would arrive to do it.
 *
 *  A no-op when no transcript has a box, or when there is no turn in
 *  `direction`. Desktop moves DOM focus into the focusable container, so the
 *  native scroll keys follow the jump. */
export function stepThreadTurn(direction: 1 | -1): void {
  const el = resolveTarget();
  if (!el) return;

  // Land focus in the transcript FIRST, so continuous Arrow and Page scrolling
  // follows. Even with no turn to jump to in this direction, pressing the
  // shortcut parks focus on the scroll region to keep reading. Desktop only,
  // since mobile navigates panes and a chord has no mobile path. `preventScroll`
  // so the focus move does not fight the animation below.
  if (!isMobile()) el.focus({ preventScroll: true });

  const turns = Array.from(el.querySelectorAll<HTMLElement>(TURN_SELECTOR)).filter(isElementVisible);
  if (turns.length === 0) return;
  // The shared landing line, the same one a deep link rests on. All turns share
  // the CSS rule, so read it off the first one.
  const gap = turnLandingClearancePx(turns[0]);
  const containerTop = el.getBoundingClientRect().top;
  const tops = turns.map((t) => t.getBoundingClientRect().top - containerTop + el.scrollTop);
  // A landed turn's top rests on the landing line at `scrollTop + gap`. So
  // "next" is the first turn whose top is below that line, and "prev" the last
  // one above it. The gap is folded into the reference here, with only a small
  // slack as the threshold. Widening the threshold by it instead would double
  // the skip band and swallow short adjacent turns when stepping.
  //
  // When the nav focus marker is on one of these turns, step by INDEX from it
  // rather than by scroll position (see `pickTurnTarget`).
  // `closest('.chat-exchange')` also anchors a deep-link marker that landed on
  // an inner `.initiator-panel`. A marker outside the transcript is not in
  // `turns`, giving -1 and the scroll-based fallback.
  const markedTurn = navFocusElement()?.closest?.(TURN_SELECTOR) as HTMLElement | null;
  const anchorIdx = markedTurn ? turns.indexOf(markedTurn) : -1;
  const idx = pickTurnTarget(anchorIdx, tops, el.scrollTop + gap, direction, TURN_NAV_THRESHOLD_SLACK_PX);
  if (idx === null) return; // at the end in this direction; focus already moved
  const turn = turns[idx];

  // We ARE jumping now, and a deliberate jump supersedes any in-flight deep-link
  // claim, exactly as `scrollToTop` does.
  clearPendingEventScroll();

  // Absolute target `scrollTop`, putting the turn's top `gap` px below the
  // container top. Re-read each frame, so a layout shift during streaming is
  // tracked. The term is stable as `scrollTop` changes, because the turn's
  // viewport top moves by the same amount.
  const targetOf = (c: HTMLElement) =>
    typeof c.getBoundingClientRect === 'function'
      ? turn.getBoundingClientRect().top - c.getBoundingClientRect().top + c.scrollTop - gap
      : 0;

  // Reconcile the chevron against the ACTUAL landing target rather than
  // hardcoding "parked mid-thread". Any turn near the end has a landing target
  // at or beyond the live edge, so the browser clamps the scroll to the bottom.
  // When the container is ALREADY at the bottom the clamped write moves nothing,
  // so no scroll event fires and `onScroll` never reconciles the chevron. The
  // threshold is `isLiveEdgeTop`, the same one a deep link's landing asks.
  awayFromBottom.value = !isLiveEdgeTop(el, targetOf(el));

  // A deliberate jump AWAY from the live edge ends the ride, and it has to say
  // so itself. A scroll only speaks for the reader when a gesture is behind it,
  // and a keyboard chord is not one. See "Was this scroll the reader's own
  // GESTURE?".
  //
  // It reuses the line above rather than asking again, so the chevron's state
  // and the ride's cannot disagree. A step onto the LAST turn lands at the
  // clamped live edge, which is where the ride was taking them anyway. And only
  // while the agent is LIVE, matching the scroll disarm.
  if (awayFromBottom.value && threadIsLive()) stopFollowingBottom();

  // Mark the landed turn with the navigation focus marker. Any prior marker was
  // already cleared by `clearPendingEventScroll` above, so this is a clean
  // supersede. No settle guard: the `animateScroll` below is programmatic and
  // emits no wheel, touch or keydown, so it cannot self-clear, and a real user
  // scroll should.
  applyNavFocus(turn);

  if (prefersReducedMotion()) {
    cancelScrollAnim();
    markNavigationScroll(el, Math.max(0, targetOf(el)));
    return;
  }
  animateScroll(targetOf);
}

/** Which collapse store a `.chat-exchange` toggle targets. `response` folds the
 *  response body (`collapsedExchanges`); `initiator` folds the initiator panel
 *  (`collapsedInitiators`), the fallback for a response-less divider or change
 *  turn. Both stores key on `${threadId}:${userSeq}`. */
export type TurnCollapseKind = 'response' | 'initiator';

/** Pure decode of a navigated `.chat-exchange`'s collapse identity from its data
 *  attributes. It returns the target thread id, exchange sequence, and which
 *  panel to toggle. Null when the attributes are missing or malformed, or the
 *  turn is not collapsible. Exported and DOM-free for unit testing.
 *
 *  The store-touching orchestration that consumes it,
 *  `toggleNavigatedTurnCollapsed`, lives in `hooks/useKeyboardShortcuts.ts`.
 *  THIS MODULE STAYS FREE OF THE HEAVY `store` IMPORT, so lean importers such as
 *  `promptFocus` do not drag in `store`'s module-load side effects. */
export function parseNavigatedTurn(
  threadId: string | null,
  userSeqAttr: string | null,
  kind: string | null,
): { threadId: string; userSeq: number; kind: TurnCollapseKind } | null {
  if (!threadId) return null;
  if (kind !== 'response' && kind !== 'initiator') return null;
  // Reject a missing or blank attribute explicitly. `Number(null)` and
  // `Number('')` both coerce to 0, which would let an unstamped turn parse.
  if (userSeqAttr === null || userSeqAttr.trim() === '') return null;
  const userSeq = Number(userSeqAttr);
  if (!Number.isInteger(userSeq)) return null;
  return { threadId, userSeq, kind };
}

/** True when the element with the given `data-event-id` is currently in the
 *  visible viewport on this device. That is the `.chat-exchange` for an
 *  exchange-start event, or the card that renders it for a step-level event.
 *  Both are stamped by `ChatExchange`, so the notification in-app matrix's
 *  "already looking at it" check works for either. A copy with no box is
 *  filtered out by `isElementOnScreen`. */
export function isEventInViewport(eventId: string): boolean {
  if (!eventId || typeof document === 'undefined' || !document.querySelectorAll) return false;
  const matches = document.querySelectorAll<HTMLElement>(`[data-event-id="${CSS.escape(eventId)}"]`);
  for (const el of matches) {
    if (isElementOnScreen(el)) return true;
  }
  return false;
}

/** True when `el` is actually on screen, not merely laid out. Two distinct
 *  checks, and both are load-bearing. `isElementVisible` rejects an element with
 *  no box and anything inside a collapsed container. The rect test rejects an
 *  element scrolled or translated out of view. That second one is what a
 *  restored scroll position needs, since a card far below the fold passes the
 *  first test.
 *
 *  BOTH axes are tested, and each catches a different layout. Vertically the
 *  band is the ACTIVE SCROLL ELEMENT's rather than the window's, because the
 *  transcript is inset by the app header and the prompt region. An element in
 *  either strip is inside `window.innerHeight` while being hidden. Horizontally,
 *  the mobile swipe layout keeps every pane mounted and translates the inactive
 *  ones aside. An element in an off-screen pane therefore has a full-size rect.
 *
 *  Shared by `isEventInViewport` and by `choiceCardNav`, which must never take
 *  DOM focus on an off-screen choice. That would arm an Enter the user cannot
 *  see, and on a permission card an unseen grant. */
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

/** Build the scroll- and resize-event handlers for a single `.thread-content`
 *  element, and wire the reader-gesture signal `onScroll` reads. The visibility
 *  gate at the top of each handler is required by the contract at the top of
 *  this file.
 *
 *  `detachGestures` is the caller's ONE teardown obligation, because the gesture
 *  listeners are the only thing here that touches the DOM. The caller attaches
 *  and removes `onScroll` and `onResize` itself, but never sees these. Call it
 *  wherever the `scroll` listener is removed. */
export function makeScrollObservers(el: HTMLElement) {
  const detachGestures = attachReaderGestures(el);
  function isAtTop() {
    return el.scrollTop <= 80;
  }
  // Tighter than `isAtTop`'s 80px chevron window: the title fade eases in as
  // soon as content slides under the bar. The 2px slack absorbs subpixel
  // rounding and the iOS overscroll bounce at the top.
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
   *  `scrollTop` shows a different part of the thread. Narrowing the thread pane
   *  makes the transcript taller and carries the reader UP into older turns.
   *
   *  The browser will not do this for us. Chromium's scroll anchoring treats a
   *  width change on the scroll container as a suppression trigger, and WebKit
   *  has never implemented it at all. Same gap `withScrollAnchor` covers for DOM
   *  mutations.
   *
   *  The anchor must describe the layout BEFORE the reflow, and a ResizeObserver
   *  only runs after it. So both handlers keep a running snapshot of which child
   *  the reader is parked on, and where its top sat. Two MEASURED positions are
   *  all the correction needs, which makes it immune to the browser clamping
   *  `scrollTop`. A correction derived from a delta would read that clamp as the
   *  reader having scrolled.
   *
   *  A reader measured ON THE LIVE EDGE is anchored to the edge instead of to a
   *  child, and `anchorAtLiveEdge` is that half of the snapshot. */
  let lastWidth = el.clientWidth;
  let lastHeight = el.clientHeight;
  let anchorChild: HTMLElement | null = null;
  let anchorRelTop = 0;
  /** Was the reader at the live edge when the anchor was last taken? Starts
   *  false rather than measured: a container with no box answers `isAtLiveEdge`
   *  true (0 + 0 >= 0 - 2), and the first round must not act on that. */
  let anchorAtLiveEdge = false;

  function viewportTop() {
    return el.getBoundingClientRect().top;
  }

  /** Snapshot where the reader is parked, in the two forms the correction can
   *  use. Whether they are ON THE LIVE EDGE, and which child they are parked on:
   *  the last one whose top is at or above the viewport top, with the offset it
   *  sat at. Scanned from the END, since the reader is normally near the newest
   *  turn, so the loop usually stops on its first step. Children with no box are
   *  skipped, because on desktop the mobile title row reports an all-zero rect
   *  that would otherwise read as "far above".
   *
   *  Both are taken together and read together. The ResizeObserver only sees the
   *  geometry AFTER the change, and both questions are about before it.
   *
   *  Cheap on the scroll path despite the rect reads. Both callers run
   *  `isElementVisible(el)` first, which already forces any pending layout
   *  flush, so these reads hit a clean tree rather than triggering one. */
  function recordAnchor() {
    anchorAtLiveEdge = isAtLiveEdge(el);
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
   *  ResizeObserver callback, after layout and before paint, so the correction
   *  is never painted as a jump.
   *
   *  EVERY reader who reaches it is held on their anchor, including one sitting
   *  at the very bottom. Holding the anchor is the honest reading of "keep the
   *  reader on the same content", and being AT the bottom does not change it: a
   *  position is not a request.
   *
   *  The one reader who does NOT reach it is one who ASKED to be kept at the
   *  bottom and was there when the box changed. Their anchor is the edge itself.
   *  That branch is in `onResize` and stands in FRONT of this. It is the same
   *  carve-out `withScrollAnchor` makes on the DOM side, and it is a request
   *  term rather than a proximity one. */
  function restoreAfterReflow() {
    const child = anchorChild;
    if (!child || child.isConnected === false) return;
    const shift = (child.getBoundingClientRect().top - viewportTop()) - anchorRelTop;
    if (shift === 0) return;
    // An ANCHOR write, which is what keeps the mobile header still across it.
    // Unmarked, a wide reflow slid the chrome by its own delta and covered the
    // line this exists to hold. Same act as `withScrollAnchor`'s correction.
    markAnchorScroll(el, el.scrollTop + shift);
    // The app holding the reader on the same content, not the reader taking
    // over. The growth branch usually re-stamps a line later, but not while it
    // stands down for a tween or a pending landing. See `carryHeldScroll`.
    carryHeldScroll(el);
  }

  /** THE FOLLOW'S PROMISE: one write and one rule, reaching the reader through
   *  THREE events. Two of them move the reader without the transcript growing.
   *  Those are the transcript's own BOX changing, and the PLATFORM scrolling the
   *  container with no gesture behind it. GROWTH is the third, and arrives here
   *  as `honourGrowth`'s `keepEdge`. Each caller adds its own event terms and
   *  reads the return.
   *
   *  TWO terms here. ARMED, because a position is not a request. And AT THE LIVE
   *  EDGE before the event, which has TWO sources and needs either.
   *
   *  `anchorAtLiveEdge` is the MEASUREMENT, taken by `recordAnchor` at the end
   *  of every round. `heldOnTheLiveEdge` is the app's own PLACEMENT, and it is
   *  the only one that can answer for the first round after an open. A resume
   *  writes the live edge before any round has run. On a thread that finished
   *  while the reader was away there is no liveness to carry them either. So the
   *  measurement alone left them where the write fell, with the toggle lit,
   *  watching the transcript grow past them.
   *
   *  No LIVENESS term. The platform and the app's own layout move the reader
   *  whether or not the agent is running. Liveness belongs to GROWTH, where it
   *  means there is nothing to be carried toward.
   *
   *  It stands down for a TWEEN, already going somewhere the reader asked for
   *  more recently, and for a deep-link CLAIM that has not landed. Under a
   *  claim, `restoreAfterReflow` is the right answer instead. It cannot loop,
   *  because `markHeldScroll` stamps the position read back AFTER the write, so
   *  a browser clamp is recorded as ours. */
  function keepTheLiveEdge(): boolean {
    if (!_followingBottom.value) return false;
    if (!anchorAtLiveEdge && !heldOnTheLiveEdge(el)) return false;
    if (_scrollAnimRaf !== null || hasPendingEventScroll()) return false;
    markHeldScroll(el, liveEdgeTop(el));
    return true;
  }

  // Scroll events. Whoever moved the container, the answer is the same:
  // reconcile the three position signals against where it now sits, and re-take
  // the reflow anchor. Nothing infers intent from a scroll.
  //
  // Two questions ARE asked of it, and this is the only place a scroll retires a
  // follow or cancels a landing. Has the reader taken the container away from
  // where our last held write put it, and was a reader GESTURE behind it. Those
  // two have THREE answers between them, and the third is `keepTheLiveEdge`: a
  // scroll that moved an armed reader off the edge with no gesture is neither
  // theirs to keep nor ours to ignore.
  //
  // The FOLLOW takes FOUR terms, and ADR 0064 has the reasoning. Off the live
  // edge alone is not enough: a shrink clamps the reader down, and the anchor
  // correction moves them while holding them on the same content. Moved alone is
  // not enough either, because a tween mid-glide is our own.
  //
  // The LANDING takes only the moved term, deliberately. A reader who flicks
  // down to the live edge mid-glide has gone where the landing is not taking
  // them. A landing answers a submit made a moment ago, so whether the agent has
  // got going yet says nothing about whether the reader wants it.
  function onScroll() {
    if (!isElementVisible(el)) return;
    // All three are questions about the scroll being HANDLED, so all three are
    // read before anything below can write over the answer. The gesture once,
    // not once per arm. The two arms are opposite answers to ONE question, and a
    // second call could land the other side of the window's edge.
    const atEdge = isAtLiveEdge(el);
    const tookOver = !isWhereWeHeldIt(el);
    const gesture = readerGestureActive(el);
    // The container has moved off our stamp, so the stamp stops speaking for
    // where the reader is. Ahead of every branch below, and of the reads it
    // cannot change: they took their answers on the line above, and each branch
    // that puts the reader back on the edge re-stamps as it writes.
    forgetHeldLiveEdge(el);
    if (_followingBottom.value) {
      if (threadIsLive() && !atEdge && tookOver && gesture) stopFollowingBottom();
      // NOT A GESTURE IS NOT THE SAME AS THE PLATFORM, which is the fourth term.
      // The app's own NAVIGATIONS are the third thing that moves the container:
      // the up chevron, turn stepping, and `useScrollMemory` positioning a
      // thread on open. Each writes through `markNavigationScroll` rather than
      // `markHeldScroll`, so none is where we held the reader, and a chord is
      // deliberately not a gesture either. On an IDLE thread those three KEEP
      // the ride. Without this term the correction writes the reader back to the
      // bottom on the navigation's own trailing scroll event.
      //
      // `isPlacementScroll` is the module's answer to exactly this question,
      // window and all. It is asked HERE rather than inside `keepTheLiveEdge`,
      // because only a scroll event can be attributed to a write. The box-change
      // caller has none, and re-marks itself every growth round, so asking there
      // would stand the branch down for our own writes.
      //
      // A PLACEMENT and not any navigation, which is the narrower of the two.
      // The ride's own held writes mark a navigation too, for the mobile
      // header's sake. So the wide predicate is true for most of a settling
      // transcript. Read through it, one unattributed scroll clears both
      // readings `keepTheLiveEdge` has and the ride can never write again.
      else if (!atEdge && tookOver && !gesture && !isPlacementScroll(el)) keepTheLiveEdge();
    } else if (tookOver && landingInFlight()) {
      cancelLanding();
    }
    // Reconciled AFTER the correction above, and re-measured rather than reusing
    // `atEdge`. The chevron and the anchor then describe where this handler LEFT
    // the reader, not where the platform put them. Same order as `onResize`.
    syncNotAtTop();
    syncScrolledFromTop();
    awayFromBottom.value = !isAtLiveEdge(el);
    // The reader has moved, so the reflow anchor has to follow them.
    recordAnchor();
  }
  // Resize events. A resize moves the reader for exactly three reasons, and all
  // three are things they asked for:
  //
  //  - they ASKED to ride the live edge and are still ON it, so the app must not
  //    slide the edge out from under them. `keepTheLiveEdge`, whether the BOX
  //    changed or the CONTENT grew;
  //  - they asked to ride, have not taken it back, and there is something live
  //    to ride, from wherever they now are (`followIsCarrying`);
  //  - they just SUBMITTED and the turn their landing waits for has rendered.
  //
  // For everyone else it moves nobody, whatever grew and however far off the
  // bottom it leaves them. A streaming reply, a decoded image, an expanded step
  // and a growing composer all leave the transcript where it is. The handler's
  // remaining job is to reconcile the signals against the new geometry.
  //
  // "Everyone else" includes an ARMED reader who has SCROLLED AWAY on an idle
  // thread, for CONTENT GROWTH. The line is drawn at the reader's POSITION, not
  // at the kind of resize. See `followIsCarrying`, and ADR 0064 for the pin this
  // replaced. The branch below infers nothing: it reads what the reader's own
  // chevron tap or submit recorded.
  function onResize() {
    if (!isElementVisible(el)) return;
    // The transcript's OWN BOX changing is a different event from its content
    // growing, and only the first moves the reader relative to what they were
    // reading. Content growth needs no correction. The content above the reader
    // is unchanged, so the same `scrollTop` shows them the same thing.
    const width = el.clientWidth;
    const height = el.clientHeight;
    const boxChanged = width !== lastWidth || height !== lastHeight;
    // A WIDTH change re-wrapped the transcript, so the content above the reader
    // changed height and the drift it caused has to be undone. A height-only
    // change re-wraps nothing.
    const reflowed = width !== lastWidth;
    lastWidth = width;
    lastHeight = height;
    // Both corrections run before anything below reads the new geometry, and
    // they are ALTERNATIVES rather than a sequence.
    //
    // A reader who ASKED to be kept at the bottom and WAS there keeps the live
    // edge across the change. For them the edge REPLACES the child anchor rather
    // than following it. The two say the same thing before the change and
    // different things after it. Correcting first would be one write to the
    // wrong place and a second to the right one.
    //
    // `keepTheLiveEdge` holds the terms that decide it and reports whether it
    // wrote, which is all this branch needs. Read it for the terms and the
    // stand-downs. Those stand-downs are why `restoreAfterReflow` is the
    // fallthrough rather than an unconditional second write: under a deep link's
    // claim the anchor IS the right answer.
    //
    // This caller's own term is the BOX. Content growth changes no dimension, so
    // a streaming reply reaches the edge write through the growth branch.
    if (!(boxChanged && keepTheLiveEdge()) && reflowed) {
      restoreAfterReflow();
    }
    // The growth branch. It runs after whichever correction above ran, so it
    // writes from the corrected position. It runs before the signals reconcile
    // below, so the chevron describes where it left the reader.
    //
    // It is handed `keepTheLiveEdge` for its on-the-edge arm, so growth is that
    // rule's third caller rather than a second copy of it. Growth adds no term
    // of its own: the event IS the term. Where the branch above already wrote
    // the live edge, this repeats that exact target.
    honourGrowth(el, keepTheLiveEdge);
    syncNotAtTop();
    syncScrolledFromTop();
    // One unconditional reconcile, both directions. Growth below the fold raises
    // the chevron on the very next frame, and an unarmed reader at the live edge
    // is left behind by the first token of a reply. A shrink that leaves them
    // visually at the bottom again hides it. Neither waits for a scroll event,
    // which a pure content change never fires.
    awayFromBottom.value = !isAtLiveEdge(el);
    // Retake the snapshot last, once the layout above has settled. Measuring it
    // earlier would describe a position the reader is no longer at.
    recordAnchor();
  }
  return { onScroll, onResize, detachGestures };
}
