import { useEffect, useRef } from 'preact/hooks';
import type { RefObject } from 'preact';

import {
  deepLinkHasResolved,
  EVENT_RESOLVE_DEADLINE_MS,
  isFollowScroll,
  markNavigationScroll,
  onDeepLinkClaimed,
  onDeepLinkResolved,
  onFollowArmed,
  resumeFollowingBottom,
  applyFollowSeed,
} from '../components/chat/scrollState';
import { onPageHide, onPageWake } from '../utils/pageVisit';
import { watchUserAction } from '../utils/userAction';

/** True iff the container's current measurement can hold the saved offset
 *  (i.e. content has grown enough). Used to gate ResizeObserver-driven
 *  restore retries while async content is still rendering.
 *  saved=0 (user was at the top) is always restorable, and telling "user
 *  scrolled to top" apart from "no save" matters: the second opens at the top
 *  via `resetOnEmpty`, the first is a position the reader chose. */
export function isFullyRestorable(saved: number, scrollHeight: number, clientHeight: number): boolean {
  if (saved < 0) return false;
  if (saved === 0) return true;
  const max = Math.max(0, scrollHeight - clientHeight);
  return max >= saved;
}

/** The stored form of a reading position that is the LIVE EDGE rather than a
 *  pixel offset: the reader had a standing follow armed in this thread when they
 *  left it. Deliberately not a number, and deliberately not a second key beside
 *  the offset: a thread opens in exactly one place, so the two answers share one
 *  slot and cannot disagree. */
export const LIVE_EDGE_VALUE = 'live-edge';

/** Where a thread opens. Either a pixel offset the reader parked on, or the live
 *  edge they asked to ride (see `LIVE_EDGE_VALUE`). `null` for no saved position
 *  at all, which `resetOnEmpty` turns into the top. */
export type SavedScroll =
  | { kind: 'offset'; top: number }
  | { kind: 'live-edge' };

/** Parse a localStorage scroll value. Returns null on missing/invalid/negative.
 *
 *  Tolerates a trailing `:<revision>` stamp, which positions written before
 *  2026-08-08 carry: the transcript used to retire a position once the thread
 *  had gained a turn, so that opening it would fall through to the
 *  auto-scroll-to-bottom instead. Nothing scrolls to the bottom on its own any
 *  more, and retiring a position now sends the reader to the TOP rather than
 *  returning them, so the stamp and the retirement are gone. `parseFloat` reads
 *  the offset out of an old stamped value and ignores the rest, so a browser
 *  carrying one keeps its position instead of losing it.
 *
 *  A build that predates `LIVE_EDGE_VALUE` reads it as junk and answers null, so
 *  a downgrade opens the thread at the top rather than somewhere wrong. That is
 *  the whole of the compatibility story in both directions: no migration, because
 *  the only thing at stake is where one thread opens once. */
export function parseSavedScroll(raw: string | null): SavedScroll | null {
  if (raw === null || raw === '') return null;
  if (raw === LIVE_EDGE_VALUE) return { kind: 'live-edge' };
  const n = Number.parseFloat(raw);
  if (!Number.isFinite(n) || n < 0) return null;
  return { kind: 'offset', top: Math.floor(n) };
}

/** localStorage key for a chat thread's saved scroll offset. */
export function threadScrollKey(threadId: string): string {
  return `lucidos-scroll-thread-${threadId}`;
}


/** localStorage key for ContentPane's per-view scroll offset. ContentPane
 *  reads/writes this directly; the prefix lives here so resetContentScroll
 *  stays in sync. */
export function contentScrollKey(viewKey: string): string {
  return `lucidos-scroll-content-${viewKey}`;
}

/** Drop a ContentPane view's saved scroll so the next mount lands at the top
 *  instead of restoring (e.g., after saving a form). */
export function resetContentScroll(viewKey: string): void {
  try {
    localStorage.removeItem(contentScrollKey(viewKey));
  } catch { /* quota or disabled, ignore */ }
}

const SAVE_DEBOUNCE_MS = 150;
/** How long to wait for async content to render before giving up on restoring.
 *  Long enough for typical Loadable<T> roundtrips, short enough that a stuck
 *  observer doesn't permanently suppress saves.
 *
 *  Giving up MOVES NOBODY. It used to fall back to `Math.min(saved.top, max)`,
 *  landing the reader as far down as the current content allowed, on the
 *  reasoning that a container which shrank since the last visit is better
 *  answered near their position than at the top. But the fallback can only ever
 *  run when the offset is UNREACHABLE, so `max` was the whole of that
 *  expression: it was "scroll to the live edge", three seconds after the reader
 *  arrived and settled.
 *
 *  The windowed transcript makes that routine rather than rare. ThreadView
 *  renders a trailing slice (`threadWindow.INITIAL_WINDOW`), so a position
 *  recorded against a taller render (a session that scrolled up, a deep-link's
 *  render-all) is out of reach on the next open, and opening a long thread
 *  hauled the reader to the end of the conversation a beat later. Same
 *  conclusion `scrollToSelectorAndPulse`'s deadline reached for a dead
 *  deep-link: the place the reader asked for is not reachable, and the bottom
 *  is not it. */
const RESTORE_DEADLINE_MS = 3000;
/** Grace on top of `EVENT_RESOLVE_DEADLINE_MS` before a stood-down open decides
 *  the deep-link is dead and positions the thread itself. Covers the release
 *  landing a beat after its own deadline; short enough that a dead link is not
 *  left sitting on a borrowed offset for noticeably longer than the toast. */
const DEAD_DEEP_LINK_SLACK_MS = 500;

export interface ScrollMemoryOptions {
  /** When true, don't restore even if a saved value exists. Useful while
   *  content is still loading. */
  paused?: boolean;
  /** Predicate called once at restore time (effect setup). Return false to SKIP
   *  RESTORING a saved offset for this mount/key-change, while still attaching
   *  the save listener. Defaults to always-restore. For chat: pass
   *  `() => !hasPendingEventScroll()` so a notification deep-link resolving a
   *  scroll to a specific event isn't overridden. Without this, focusing an
   *  UNfocused thread re-runs this hook, and its restore observer (created after
   *  `scrollToEventAndPulse`'s) fires last and snaps back to the saved offset:
   *  the "toast deep-link lands on the saved position, not the event, unless the
   *  thread was already focused" bug.
   *
   *  It gates BOTH the restore and the `resetOnEmpty` reset, because both are
   *  this hook placing the reader and the deep-link's landing must win. The
   *  attach cannot be assumed to happen before that landing: it is parked on
   *  `paused` until the events load, `eventsLoaded` and the rendered exchanges
   *  arrive in the same store write, and a MutationObserver callback (which is
   *  how the deep-link notices its target) is delivered on the microtask
   *  checkpoint of that commit while Preact defers `useEffect` past it. Under
   *  reduced motion the landing is a single synchronous write with nothing to
   *  re-assert it, so an ungated reset simply overwrote it.
   *
   *  Standing down leaves a hole this hook closes itself: if the deep-link
   *  never lands, nothing else ever positions the thread. See the
   *  `!allowRestore` branch in `attachScrollMemory`. */
  shouldRestore?: () => boolean;
  /** When true, this is a SHARED scroll container: the previous view's offset
   *  persists on the DOM, so the hook writes `scrollTop = 0` itself wherever it
   *  has no position to put the reader in. Two such places, and they are the
   *  whole of what this gates: the no-save case (`saved === null`), and the WAIT
   *  for a saved offset the content is not yet tall enough to hold, which is
   *  parked at the top rather than spent on the borrowed one. `saved === 0` is
   *  neither: it is a real restore and always writes scrollTop. Off by default.
   *
   *  For the transcript this is what "a thread with no saved position opens at
   *  the top, the way a document opens" is made of. */
  resetOnEmpty?: boolean;
  /** When true, this container's saved position may be the LIVE EDGE as well as
   *  an offset: an armed standing follow is recorded here and resumed on
   *  re-entry, so a reader watching an agent work is still watching it after
   *  visiting another thread. Off by default.
   *
   *  An opt-in rather than the default because the follow is one global and the
   *  three containers this hook serves are not: without the gate, arming a follow
   *  in the transcript would stamp the live edge onto whatever the content pane
   *  or the thread drawer happened to be showing. Only the transcript can ride a
   *  live edge, so only the transcript records one. */
  followsLiveEdge?: boolean;
}

/** The options an attachment reads LIVE, i.e. whose current value belongs to
 *  whatever the component last rendered rather than to the attachment. The hook
 *  hands them over as one getter, not as values, precisely so the attachment
 *  cannot capture them at setup and go stale; the flip side is that reading one
 *  after the attachment stopped being current reads the NEXT thing's value. See
 *  `observed` in `attachScrollMemory` for the bug that cost. */
export type ScrollMemoryLive = Pick<ScrollMemoryOptions, 'shouldRestore'>;

/** Wire one scroll container to one storage key: restore on attach, persist on
 *  scroll, flush on teardown. Returns the teardown.
 *
 *  Extracted from the hook (which is a thin `useEffect` over it) so the whole
 *  lifecycle, teardown included, is drivable from a test with a fake element,
 *  the way `makeScrollObservers` already is. The teardown's correctness depends
 *  on WHEN each value was read, which no assertion over the hook could reach.
 *
 *  Save is debounced by ~150ms to avoid storage thrash during scroll. */
export function attachScrollMemory(
  el: HTMLElement,
  key: string,
  opts: {
    live: () => ScrollMemoryLive;
    resetOnEmpty?: boolean;
    followsLiveEdge?: boolean;
    isCurrent?: () => boolean;
  },
): () => void {
  const { live, resetOnEmpty = false, followsLiveEdge = false, isCurrent } = opts;

  let saveTimer: ReturnType<typeof setTimeout> | null = null;
  // hasWritten is the dedup gate: lastSaved alone can't tell "wrote 0"
  // from "wrote nothing yet", and we need the first writeNow to act so
  // any stale localStorage value from a previous session is reconciled.
  let lastSaved: string | null = null;
  let hasWritten = false;
  let restoring = true;
  /** The value to commit, captured WHEN THE SCROLL HAPPENED rather than when
   *  the debounce fires. The container's offset belongs to THIS key, and stops
   *  belonging to it at teardown: the teardown runs from the hook's effect
   *  cleanup, which Preact defers past the render that changed `key`, so by then
   *  the shared `.thread-content` is already showing the INCOMING thread.
   *
   *  Reading it there wrote the outgoing thread's key with the incoming
   *  thread's offset: switching threads silently moved the reading position in
   *  the thread you just left. Snapshotting instead leaves nothing at teardown
   *  for the new render to have moved, so the bug is unrepresentable rather
   *  than merely fixed.
   *
   *  `undefined` means "this key has seen no scroll", which is what makes an
   *  unreached `writeNow` do nothing rather than delete a stored position. */
  let observed: string | undefined;
  let resizeObserver: ResizeObserver | null = null;
  let mutationObserver: MutationObserver | null = null;
  let deadlineTimer: ReturnType<typeof setTimeout> | null = null;
  /** Teardown for the user-action watch that runs FOR the wait, and `null`
   *  whenever no wait is armed.
   *
   *  The wait can run for three seconds, which is long enough for the reader to
   *  have settled in and started reading, and landing a three-second-old record
   *  on top of them then is the app moving them. So the first thing they do
   *  retires the restore: the position they are at is theirs, and the record is
   *  only ever an offer to put them back.
   *
   *  Asked as a GESTURE (`watchUserAction`: wheel / touchmove / pointerdown /
   *  keydown) and never as a change in `scrollTop`. A pixel delta cannot tell
   *  the reader from the app, and the app writes `scrollTop` all through this
   *  window without going through `markNavigationScroll`: the browser clamps a
   *  shared container when shorter content swaps in, `restoreAfterReflow` holds
   *  the reader still across a pane resize, `ThreadView`'s window-expansion
   *  compensates for the height it just prepended, and the iOS compositor
   *  nudge writes ±1px five times over the first second of every thread open.
   *  Reading any of those as a gesture would abandon the reader's saved position
   *  for good. None of them emits an input event, which is exactly what
   *  `watchUserAction` documents itself for, and it is the same signal the
   *  navigation focus marker fades on. */
  let stopUserWatch: (() => void) | null = null;
  /** Where the container sat when the FIRST deep-link of this open took it, and
   *  `null` while none has. The dead-link rescue's reference point; see
   *  `standDownForDeepLink` for why it is captured once rather than per claim. */
  let inheritedBeforeDeepLink: number | null = null;

  /** What is RECORDED for this key right now. Re-readable rather than read once,
   *  because the attach-time answer goes stale the moment the reader scrolls and
   *  the wake below must not act on a snapshot from before they did. */
  const readSaved = (): SavedScroll | null => {
    let value: SavedScroll | null = null;
    try {
      value = parseSavedScroll(localStorage.getItem(key));
    } catch { /* ignore */ }
    // Only a container that can RECORD the live edge can restore one. Anything
    // else reading the sentinel is reading a value it did not write, so the
    // honest answer is "no saved position" rather than riding a bottom nobody
    // asked for.
    if (value?.kind === 'live-edge' && !followsLiveEdge) return null;
    return value;
  };

  let saved: SavedScroll | null = readSaved();

  const stopRestore = () => {
    restoring = false;
    resizeObserver?.disconnect();
    resizeObserver = null;
    mutationObserver?.disconnect();
    mutationObserver = null;
    if (deadlineTimer !== null) {
      clearTimeout(deadlineTimer);
      deadlineTimer = null;
    }
    stopUserWatch?.();
    stopUserWatch = null;
  };

  /** Is this open OURS to position, or does something higher-priority own it?
   *  One question, asked at each of the four moments this attachment is about
   *  to place the reader: at attach (`allowRestore`), from the dead-deep-link
   *  rescue, on a page wake, and when a deep-link announces a claim. */
  const openIsOurs = () => live().shouldRestore?.() ?? true;

  const tryRestore = () => {
    if (!restoring || saved?.kind !== 'offset') return;
    if (!isFullyRestorable(saved.top, el.scrollHeight, el.clientHeight)) return;
    markNavigationScroll(el, saved.top);
    stopRestore();
  };

  /** The restore window closing: one last look, then stop. No fallback
   *  position, and never a clamp to whatever the content currently allows (see
   *  `RESTORE_DEADLINE_MS`).
   *
   *  The last look is not a formality. Two ways the container grows are
   *  invisible to both observers: an image or a font decoding changes
   *  `scrollHeight` without mutating the DOM, and the container's own box never
   *  changes (it is a flex child of a fixed parent), so a now-reachable offset
   *  would otherwise be dropped for want of a callback.
   *
   *  Nothing gates it here: a reader who has taken over retired the whole wait
   *  when they did (see `stopUserWatch`), so this timer no longer exists for
   *  them. That replaces the old clamp's own `el.scrollTop === 0`, which was
   *  asking the same question of the pixels and could only recognise a reader
   *  who had never left the top. */
  const onDeadline = () => {
    tryRestore();
    stopRestore();
  };

  /** Hand this open to a deep-link: stop positioning the reader ourselves, and
   *  arm the rescue that covers the link turning out DEAD. Both halves, always,
   *  which is why it is one function called from the two places a deep-link can
   *  take this open (see the `!allowRestore` branch and the `onDeepLinkClaimed`
   *  subscription): standing down was already a two-part obligation, and a
   *  second site doing only the first half would leave exactly the hole the
   *  rescue exists to close.
   *
   *  Whatever the link lands on is where the reader asked to be, and
   *  positioning here would overwrite it. But if the link is dead (its target
   *  never renders, because the event is not in this thread or renders nothing)
   *  then nothing positions this thread at all, and `.thread-content` is one
   *  element reused across threads: it keeps showing the OUTGOING thread's
   *  offset, and the save listener then persists that borrowed number as this
   *  thread's remembered position.
   *
   *  So wait out the deep-link's own budget and then position, but ONLY if the
   *  container has not moved a pixel in the meantime. That condition is exactly
   *  "the landing never happened": a successful landing moves it, and so does
   *  the reader scrolling, and in both of those cases there is now a real
   *  position here that is not ours to overwrite. `stopRestore` clears the
   *  timer, so a thread switch inside the window cancels it.
   *
   *  Re-entrant, because a second notification tapped mid-window is a second
   *  claim: it re-arms so the rescue covers the NEWER link's budget too, which
   *  the first link's timer cannot (it expires while the second claim is still
   *  held, declines on `openIsOurs`, and leaves nothing behind).
   *
   *  One landing this cannot see, stated so nobody has to re-derive it: a link
   *  SUPERSEDED by a newer claim still lands (the retire, the scroll and the
   *  pulse in `scrollToSelectorAndPulse` are all ungated) but neither announces
   *  the resolve nor latches it, both being scoped to the claim still being
   *  ours. So a first link that lands with nowhere to move while a second claim
   *  is held is invisible to both of this rescue's tests, and a dead second link
   *  can then position over it. It needs two links into one thread inside the
   *  resolve window, the first resolving late onto a zero-distance target, and
   *  the alternative is letting a superseded call speak for a claim a newer link
   *  owns, which is the collision the claim is an object to prevent. */
  const standDownForDeepLink = () => {
    // Also retires the restore observers, which is a no-op at attach (none are
    // armed yet) and the whole point from the claim broadcast. Clears a rescue
    // already in flight too, which is what the re-arm below replaces.
    stopRestore();
    // The link owns the POSITION on this open; it does not own the REQUEST.
    // Standing down means "do not place the reader", and every branch below this
    // one places them, so the *standing follow* this thread recorded used to be
    // handed over with them: `focusThread` retires the flag on the way in, the
    // resume that answers it lives in the positioning branch, and a deep-linked
    // open is the one open that never reaches it. A notification tap into a
    // thread the reader was riding therefore landed on the event with the toggle
    // dark, which is the report this exists for. So the request is resumed here
    // instead, `in-place` so nothing is written over the landing, and
    // `resumeFollowingBottom` itself declines while the agent is LIVE, where the
    // landing has just ended the ride on purpose.
    //
    // FIRST, before the early return below, because a link that has already
    // landed is exactly the ordinary cross-thread tap and needs this most.
    //
    // The record is RE-READ rather than taken from the attach-time snapshot: a
    // claim broadcast can arrive long after the reader's own scroll changed the
    // answer, which is the same reason `readSaved` is re-readable at all.
    // Gated on `followsLiveEdge` like every other live-edge branch, so the
    // content pane and the thread drawer cannot arm the transcript's follow.
    if (followsLiveEdge) {
      const recorded = readSaved();
      if (recorded?.kind === 'live-edge') resumeFollowingBottom(el, 'in-place');
      // No record at all is the one case the *follow seed* speaks for, and a
      // deep link does not change that this thread has none.
      else if (recorded === null) applyFollowSeed(el, 'in-place');
    }
    // A link that has ALREADY found its target is positioning the reader, so
    // there is no dead link to rescue and arming would only create one more way
    // to move them. ASKED rather than waited for, because the resolve broadcast
    // reaches only listeners that exist when it fires, and in the ordinary tap
    // into a thread the reader is not in the target resolves on the microtask
    // checkpoint of the commit that rendered it, before Preact runs the effect
    // that attaches this. The rescue's own "has anything moved" test cannot
    // stand in for the question: a landing with nowhere to move looks exactly
    // like a dead link, and arriving in a shorter thread clamps the shared
    // container to its bottom, where a link to that thread's last turn resolves
    // to precisely where it already sits.
    //
    // The landing is also this thread's reading position, and this attachment
    // missed the announcement that said so, so it records it here instead. Same
    // pairing as the subscription below, reached from the other side.
    if (deepLinkHasResolved()) {
      recordDeepLinkLanding();
      return;
    }
    // Captured from the FIRST deep-link of this open and never re-read, which is
    // what makes re-arming safe. A first link that LANDED moved the container,
    // and re-reading here would make its landing the new reference point, so a
    // dead SECOND link would then rescue the reader away from the event the
    // first one correctly took them to. Held against the original, that case
    // reads as "something positioned this thread" and the rescue declines.
    if (inheritedBeforeDeepLink === null) inheritedBeforeDeepLink = el.scrollTop;
    const inherited = inheritedBeforeDeepLink;
    deadlineTimer = setTimeout(() => {
      deadlineTimer = null;
      // Same question `onScroll` asks, and for the same reason: the teardown is
      // deferred past the render that changed `key`, so a superseded attachment
      // must not position a container that now belongs to the next thread.
      if (isCurrent && !isCurrent()) return;
      if (el.scrollTop !== inherited) return;
      if (!openIsOurs()) return; // a newer deep-link owns it now
      if (saved?.kind === 'live-edge') {
        resumeFollowingBottom(el);
      } else if (saved !== null && isFullyRestorable(saved.top, el.scrollHeight, el.clientHeight)) {
        markNavigationScroll(el, saved.top);
      } else if (resetOnEmpty) {
        // Either there was no position, or there is one the content cannot
        // hold. Both open the thread where a thread with no position opens, at
        // the top of what is rendered. This used to clamp an unreachable offset
        // to `Math.min(saved.top, max)` instead, which is the live edge and
        // nothing else (see `RESTORE_DEADLINE_MS` for the same expression on
        // the restore's own deadline, and why the windowed transcript reaches
        // it routinely). A container that is not shared writes nothing at all:
        // there is no borrowed offset there for the rescue to displace.
        markNavigationScroll(el, 0);
      }
    }, EVENT_RESOLVE_DEADLINE_MS + DEAD_DEEP_LINK_SLACK_MS);
  };

  const writeNow = () => {
    const next = observed;
    if (next === undefined) return; // no scroll seen under this key
    if (hasWritten && next === lastSaved) return;
    lastSaved = next;
    hasWritten = true;
    try {
      localStorage.setItem(key, next);
    } catch { /* quota or disabled, ignore */ }
  };

  const scheduleSave = () => {
    if (saveTimer !== null) clearTimeout(saveTimer);
    saveTimer = setTimeout(writeNow, SAVE_DEBOUNCE_MS);
  };

  /** Which of the two forms a reading position takes RIGHT NOW: the live edge
   *  when the reader has a standing follow armed here and has not left it, else
   *  the pixel offset the container sits at.
   *
   *  One expression with two callers (the scroll listener and the deep-link
   *  landing below), because the two must not be able to disagree about which
   *  form a position takes.
   *
   *  For the deep-link landing the answer is the OFFSET whenever the landing
   *  MOVED the reader, which is the ordinary case and the one worth stating:
   *  they asked to be at one specific place, so coming back returns them there.
   *  On a LIVE thread that is true twice over, because the landing retires the
   *  standing follow before it records (see `stopFollowingBottom`).
   *
   *  On an IDLE thread the follow survives the landing (a link into a finished
   *  thread is browsing, and nothing will carry the reader off the event
   *  either), so the positional test below is what answers, and it answers
   *  correctly without a second rule: the landing moved the container away from
   *  the follow's stamp, so `isFollowScroll` is false and the offset is
   *  recorded. The one case it answers `live-edge` is a link that lands the
   *  reader exactly where the follow already had them, which IS the live edge
   *  and IS still being ridden. Both were once wrong in the other direction: the
   *  follow survived every landing, so recording the offset would have thrown
   *  away a request that was still live, which is why this reads the position
   *  rather than the flag.
   *
   *  A scroll the FOLLOW made is recorded as the live edge, not as the offset it
   *  happened to produce. Every growth round writes `scrollTop`, so recording the
   *  number would overwrite the request with a pixel value on the next token, and
   *  re-entry would land wherever the stream had got to rather than at the edge.
   *
   *  The question asked is positional (`isFollowScroll`) rather than "is the
   *  follow armed", because `.thread-content` carries two scroll listeners: the
   *  disarm lives in `makeScrollObservers` and the save lives here, and the flag
   *  alone would answer differently depending on which ran first. The reader's
   *  gesture moves the container away from the follow's stamp by definition, so
   *  this answers "the reader's" in either order, and the offset they landed on
   *  is what gets recorded. */
  const currentPosition = (): string =>
    followsLiveEdge && isFollowScroll(el)
      ? LIVE_EDGE_VALUE
      : String(Math.floor(el.scrollTop));

  /** Record where a deep-link landed as this thread's reading position.
   *
   *  Going to a link SETS the memory. The landing is a reading position like any
   *  other: the reader asked to be at that event, that is where they are, and
   *  coming back to the thread has to return them there rather than to whatever
   *  they had parked on before they ever followed the link.
   *
   *  The scroll listener cannot be left to notice it, because two ordinary
   *  landings produce no scroll event it will see. Under reduced motion the
   *  whole landing is one synchronous write that happens before this attachment
   *  exists (the target resolves on the microtask checkpoint of the commit that
   *  rendered it, and Preact defers this effect past it). And a landing with
   *  nowhere to move writes nothing at all: arriving in a shorter thread clamps
   *  the shared container to its bottom, where a link to that thread's last turn
   *  is already exactly in place. In both the thread would keep its stale
   *  position, so the next open would undo the navigation the reader made.
   *
   *  An ANIMATED landing is recorded here at its start and corrected by its own
   *  frames: each writes `scrollTop`, and each resulting scroll event pushes the
   *  debounce out again, so what reaches storage is the settled position and
   *  never the one the tween set off from. */
  const recordDeepLinkLanding = () => {
    // The guard lives HERE rather than at the two call sites, because it belongs
    // to the write: a superseded attachment is still subscribed until its
    // deferred teardown, and the landing it hears is the INCOMING thread's, so
    // recording it would put that offset on the OUTGOING thread's key. That is
    // the corruption `observed` exists to make unrepresentable, and holding the
    // guard at the write means no caller can reintroduce it.
    if (isCurrent && !isCurrent()) return;
    observed = currentPosition();
    scheduleSave();
  };

  const flush = () => {
    if (saveTimer !== null) {
      clearTimeout(saveTimer);
      saveTimer = null;
    }
    // No second "is anything pending" condition: `writeNow` no-ops on an
    // unobserved key, and dedups a snapshot the timer already committed.
    writeNow();
  };

  const onScroll = () => {
    if (restoring) return;
    // A scroll that arrives after this key stopped being the current one is not
    // about this key, whatever it looks like. The teardown is deferred past the
    // render that changed `key` (Preact flushes effect cleanups after paint),
    // so the listener is still attached through a window in which the shared
    // `.thread-content` already belongs to the NEXT thread, and every live
    // option already answers for it.
    //
    // Two routes both landed in that window and both destroyed the reading
    // position in the thread being LEFT. Opening a thread with no saved position
    // resets the container to the top, and that write moves it. When the thread
    // being opened DOES have one, no reset runs, but swapping in its (shorter,
    // or not yet loaded) content clamps `scrollTop` instead. Either way a scroll
    // event reaches this handler carrying the incoming thread's offset, and the
    // flush that follows wrote it to the outgoing key.
    //
    // Asking whether this attachment is still current answers both, and asks
    // nothing about WHY the container moved: no ordering assumption about which
    // scroll listener runs first.
    if (isCurrent && !isCurrent()) return;
    // EVERY position is recorded, including the bottom. The transcript used to
    // pass `shouldSave: () => scrolledUp.value` so an at-bottom reader saved
    // nothing, because the auto-scroll-to-bottom on the next open would put them
    // back there anyway. Nothing does that now, so declining to save would send
    // a reader who finished a thread to the TOP of it on re-entry, which is the
    // app moving them rather than returning them. A scrollTop of 0 persists as
    // "0" (the reader scrolled to the top) and is a real position, distinct from
    // no save at all.
    //
    // The value is captured HERE rather than when the debounce fires: see
    // `observed`. Nothing is lost by reading it a beat earlier, since every
    // scrollTop change fires this handler, so the last event of a burst already
    // carries the settled position.
    observed = currentPosition();
    scheduleSave();
  };

  // A higher-priority scroll may own this load, e.g. a notification deep-link
  // resolving a scroll to a specific event. Skip the RESTORE so we can't
  // override that scroll, but still attach the save listener below so the
  // reader's post-landing position is remembered.
  //
  // Which BRANCH this open takes is decided once, here, and that is complete
  // only for a claim already in place when the effect runs: `focusThread` takes
  // it before Preact defers to this effect, which is the common tap. A claim
  // taken LATER cannot be seen here at all, and the restore this sets up would
  // then still be armed to overrule it, so the claim is DELIVERED instead of
  // re-read (see the `onDeepLinkClaimed` subscription below).
  /** Record an arm as this thread's reading position.
   *
   *  Arming a standing follow can produce NO scroll event at all: a reader
   *  already at the live edge who presses the toggle gets a write the browser
   *  clamps to where they already are, and an idle thread then grows nothing. The
   *  request is real either way, so it is recorded from the arm itself rather
   *  than from a scroll that may never come. Only the arm is broadcast (see
   *  `onFollowArmed`), which is exactly what lets `focusThread` retire the follow
   *  on a thread switch without overwriting the live edge just recorded for the
   *  thread being LEFT. */
  function subscribeToArm() {
    if (!followsLiveEdge) return null;
    return onFollowArmed(() => {
      // The same question `onScroll` asks: a superseded attachment is still
      // subscribed until its deferred teardown, and a follow armed in the
      // thread now on screen is not this key's request.
      if (isCurrent && !isCurrent()) return;
      // Anything this attach still has pending is now stale, because the
      // reader has just asked for the live edge and that outranks any position
      // this hook was going to put them in. Both pending things would land
      // their write ON TOP of the follow and retire it in the same stroke: the
      // restore observers, which are still waiting for the transcript to grow
      // tall enough to hold the saved offset, and the dead-deep-link rescue,
      // whose "has anything moved the container" test an arm at the live edge
      // does not trip (it writes nothing when the reader is already there).
      stopRestore();
      observed = LIVE_EDGE_VALUE;
      scheduleSave();
    });
  }

  // Subscribed BEFORE the positioning branch below, because that branch can arm:
  // the *follow seed* does, on a thread with no reading position. Left until
  // after it, the seeded arm broadcast to nobody, and whether the thread ended up
  // recording `live-edge` came down to whether arming happened to MOVE the
  // container (a shared `.thread-content` arriving on the outgoing thread's
  // offset moves, a thread already at its live edge does not). Two readers doing
  // the same thing got different persistence, which is the shape of bug that
  // shows up months later as "this one thread stopped following". Reported by
  // the Codex reviewer, 2026-08-10.
  //
  // So a SEEDED arm records too, and that is the semantics rather than a side
  // effect: the seed decides a thread's FIRST open, and from then on the thread
  // owns the answer like any other. Turning the seed off later therefore changes
  // what NEW threads do, not what a thread the reader has already ridden does.
  const unsubscribeArm = subscribeToArm();

  const allowRestore = openIsOurs();

  // The deep-link stand-down leads, because it is the one branch that does not
  // care what was saved: it answers for every value of `saved`, including none.
  // The rest read the saved position, and taking them in this order is what lets
  // each one narrow it.
  if (!allowRestore) {
    // A deep-link already owned this open when the effect ran, which is the
    // ordering `focusThread` produces for a thread already in the map. The
    // other orderings arrive later and reach the same place through the claim
    // broadcast below.
    standDownForDeepLink();
  } else if (saved === null) {
    // No reading position at all: a BRAND-NEW thread, or one the reader has never
    // parked in. This is the only branch the *follow seed* speaks for, and the
    // only one it can: every other branch is the reader's own last act on this
    // thread, which outranks a standing preference. A record therefore wins in
    // both directions, a live-edge one arming with the seed off and an offset one
    // declining to with it on.
    //
    // Gated on `followsLiveEdge` like the record's own live-edge branch, since the
    // follow is one global and this hook serves three containers.
    const seeded = followsLiveEdge && applyFollowSeed(el);
    // Browsers preserve scrollTop across children-shrink, so a shared
    // container needs an explicit reset; non-shared containers opt out. Skipped
    // when the seed armed, which wrote the live edge instead and would be undone
    // by a reset to the top.
    if (!seeded && resetOnEmpty) markNavigationScroll(el, 0);
    restoring = false;
  } else if (saved.kind === 'live-edge') {
    // The reader had a standing follow armed here when they left. Resume it:
    // write today's live edge and re-arm, so everything the agent produced while
    // they were away is behind them and the next arrival carries them too. No
    // observer retry loop, unlike the offset branch below: an offset needs the
    // transcript to be tall enough to hold it, while the live edge is wherever
    // the content currently ends and the armed follow rides the rest in.
    resumeFollowingBottom(el);
    restoring = false;
  } else if (saved.top === 0) {
    // Restore explicitly, same shared-container reason as the null branch.
    markNavigationScroll(el, 0);
    restoring = false;
  } else {
    // Land it NOW when the transcript is already tall enough to hold it, which
    // is the ordinary revisit. Deferring even to the next frame would paint the
    // borrowed offset once on the way, and would open a window in which a
    // gesture retires a wait that never needed to happen at all.
    tryRestore();
  }

  if (restoring) {
    // Still too short, so wait for it to grow. Two observers cover the two ways
    // `scrollHeight` does that after first paint:
    //   - ResizeObserver: container's own size changes (rare for flex:1
    //     containers in fixed parents, but covers initial 0→layout).
    //   - MutationObserver: subtree content changes, since children added by
    //     async Loadable<T> data don't change the container's box, so the
    //     ResizeObserver alone never fires for the typical scrollable list.
    //
    // PARK the reader at the top for the wait, on a shared container. Nothing
    // else stands between them and the outgoing thread's offset while it runs:
    // `.thread-content` is one element, and the `resetOnEmpty` write that
    // answers this for a thread with no saved position is gated on there being
    // none. So a saved offset that never became reachable left the reader on a
    // borrowed number, which arriving in a shorter thread clamps to that
    // thread's live edge, and which the save listener then persists here as
    // their own. The top is where a thread whose position cannot be honoured
    // opens, so it is also the honest place to spend the wait.
    if (resetOnEmpty) markNavigationScroll(el, 0);
    resizeObserver = new ResizeObserver(tryRestore);
    resizeObserver.observe(el);
    mutationObserver = new MutationObserver(tryRestore);
    mutationObserver.observe(el, { childList: true, subtree: true });
    deadlineTimer = setTimeout(onDeadline, RESTORE_DEADLINE_MS);
    // The wait belongs to the reader too: the first thing they DO retires it
    // (see `stopUserWatch`). Armed last, so the writes above cannot trip it.
    stopUserWatch = watchUserAction(stopRestore);
  }

  el.addEventListener('scroll', onScroll, { passive: true });


  // A deep-link CLAIMING the open retires a restore that is still armed, for
  // the same reason the arm above does: the reader has asked to be somewhere
  // specific, and that outranks the position this hook was going to put them
  // in. `allowRestore` answers only for a claim that was already in place, and
  // the two orderings it cannot see are ordinary. A deep-link into the thread
  // the reader is ALREADY in re-attaches nothing, so a restore still waiting
  // for the transcript to grow tall enough survives the whole landing. And a
  // thread whose events arrive while the tap is still resolving attaches
  // first, with no claim to see. Either way the claim then renders the FULL
  // exchange list, and that growth is precisely what the waiting restore has
  // been waiting for, so the two collide rather than merely coexist: the old
  // offset lands on top of the event, seconds after the reader arrived on it.
  //
  // `openIsOurs` is what scopes this to the container the claim is about. Only
  // the transcript asks about the deep-link claim; the content pane and the
  // thread drawer answer "ours" and keep their restores.
  //
  // It takes the SAME stand-down the attach-time branch takes, rescue included,
  // rather than merely retiring the restore. Standing down is two obligations,
  // and a claim arriving here leaves the reader on the outgoing thread's
  // borrowed offset exactly as one arriving before the attach would.
  //
  // Two states answer this claim, and nothing else does. A RESTORE is armed
  // (`restoring`, true only in the observer branch), which the stand-down
  // retires; or a RESCUE is already in flight from an earlier claim
  // (`deadlineTimer` while not restoring), whose budget this newer claim
  // extends, because the older timer expires while this claim is still held,
  // declines on `openIsOurs`, and would leave a dead second link with nothing.
  // In neither state has this attachment left the reader on a position it still
  // owes them: the no-save, saved-top-0 and live-edge branches positioned them
  // at attach, and an observer branch that gave up at its own deadline either
  // positioned them or found them already scrolled. The one case this leaves
  // uncovered, a claim arriving after that deadline into a transcript that never
  // grew tall enough, gets no rescue, exactly as it got none before any of this
  // existed.
  //
  // It needs no `isCurrent` guard of its own, unlike the arm above. A superseded
  // attachment hears this too, and standing it down costs the thread it belongs
  // to nothing: its restore would be placing the reader in a container the next
  // thread already owns, its rescue asks `isCurrent` before it writes, and the
  // one branch that touches the record (`recordDeepLinkLanding`, for a claim
  // whose link has already landed) asks the same question at the write.
  const unsubscribeDeepLink = onDeepLinkClaimed(() => {
    if (!restoring && deadlineTimer === null) return;
    if (openIsOurs()) return;
    standDownForDeepLink();
  });

  // The link FOUND its target, which settles both halves of what this
  // attachment owes it.
  //
  // It is positioning the reader, so the rescue has nothing left to cover. Told
  // rather than inferred: the rescue's own "has anything moved the container"
  // test reads a landing with nowhere to move as a dead link, and arriving in a
  // shorter thread (which clamps the shared container to its bottom) then
  // deep-linking to that thread's last turn is exactly that. It would haul the
  // reader off the event they are looking at, 4.5 seconds after they got there.
  // Only ever cancels a rescue in flight: `restoring` true means the restore
  // deadline is in that slot instead, and every other state has none.
  //
  // And where it landed is this thread's reading position, so it is RECORDED.
  //
  // `openIsOurs` scopes both to the container the link is about, which is why
  // the announcement is made while the claim is still held. The other guard, a
  // superseded attachment hearing a landing that belongs to the thread now on
  // screen, is held inside `recordDeepLinkLanding` rather than here, so it
  // covers the other call site too.
  const unsubscribeDeepLinkResolved = onDeepLinkResolved(() => {
    if (openIsOurs()) return;
    if (!restoring && deadlineTimer !== null) stopRestore();
    recordDeepLinkLanding();
  });

  // Backgrounding the app is not a teardown, so nothing here would otherwise
  // commit: the save is debounced and flushed from the cleanup, and a frozen
  // page's pending timer never runs if the page is then discarded. So the
  // reader's last act before leaving would be lost, and the direction that does
  // damage is a lost DISARM: they scroll up, background, and the stale live-edge
  // record outlives them, so the next open drags them to the bottom having asked
  // for the opposite. Every other debounced writer in the app already flushes
  // here (`store/actions/compose.ts`, `utils/perfQueue.ts`).
  //
  // `flush` commits the SNAPSHOT taken while this key was current, which is why
  // this one asks no `isCurrent()` question: there is nothing here for a later
  // render to have moved. It tears nothing down either, so the same attachment
  // keeps recording after the paired wake.
  const unsubscribeHide = onPageHide(flush);

  // Coming back is a re-entry: the reader has been away and returned, so the
  // thread is positioned by the same rule that positions it when they arrive
  // from another thread. Only the transcript, like the arm subscription.
  //
  // It reads the RECORD rather than the follow flag, and that is the whole
  // design. The flag survives a suspend and dies on a discard, and the hazard
  // being closed is that the wake ITSELF destroys it: a bfcache scroll restore
  // fires an event shaped exactly like the reader taking the container away from
  // the follow, which is the disarm (see the same warning in
  // `utils/pageResume.ts`). Reading the record makes the answer independent of
  // whether the browser dispatches that restore before or after this, and gives
  // the suspend path and the discard path one answer. The flush above is what
  // makes the record trustworthy at this moment.
  const unsubscribeWake = followsLiveEdge
    ? onPageWake(() => {
        if (isCurrent && !isCurrent()) return;
        if (readSaved()?.kind !== 'live-edge') return;
        // A notification tap can resume the app and resolve a deep-link in one
        // breath, which is the mobile shape of the whole report: the app comes
        // back, the link lands on the event, and this fires. The deep-link owns
        // the viewport, same as at attach time, so the resume writes nothing
        // there and merely picks the request back up. It used to decline
        // outright, which is right about the write and wrong about the request.
        resumeFollowingBottom(el, openIsOurs() ? 'live-edge' : 'in-place');
      })
    : null;

  return () => {
    stopRestore();
    el.removeEventListener('scroll', onScroll);
    unsubscribeArm?.();
    unsubscribeDeepLink();
    unsubscribeDeepLinkResolved();
    unsubscribeHide();
    unsubscribeWake?.();
    flush();
  };
}

/** Persist and restore a scroll container's vertical scroll position across
 *  reloads via localStorage. The container is identified by `key`; the same key
 *  on next mount restores the saved offset.
 *
 *  When `key` changes (e.g., switching threads), the previous container's
 *  position is flushed and the new key is restored.
 *
 *  A thin `useEffect` over `attachScrollMemory`, which owns the behaviour. The
 *  live options are handed over as a getter closing over the latest render's
 *  values, so the attachment always sees the current ones without the effect
 *  having to re-run (re-running on every new turn would tear down the restore
 *  observers mid-load). */
export function useScrollMemory(
  ref: RefObject<HTMLElement>,
  key: string | null,
  options: ScrollMemoryOptions = {},
) {
  const { paused = false, resetOnEmpty = false, followsLiveEdge = false } = options;
  const liveRef = useRef<ScrollMemoryLive>(options);
  liveRef.current = options;
  // What the LATEST render says the key is, versus the one an attachment was
  // set up for. They differ for exactly as long as a superseded attachment is
  // still listening, which is the window `onScroll`'s first guard closes.
  const keyRef = useRef(key);
  keyRef.current = key;

  useEffect(() => {
    if (!key || paused) return;
    const el = ref.current;
    if (!el) return;
    return attachScrollMemory(el, key, {
      live: () => liveRef.current,
      resetOnEmpty,
      followsLiveEdge,
      isCurrent: () => keyRef.current === key,
    });
  }, [ref, key, paused, resetOnEmpty, followsLiveEdge]);
}
