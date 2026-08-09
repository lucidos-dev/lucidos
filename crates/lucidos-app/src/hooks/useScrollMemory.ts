import { useEffect, useRef } from 'preact/hooks';
import type { RefObject } from 'preact';

import {
  EVENT_RESOLVE_DEADLINE_MS,
  isFollowScroll,
  markNavigationScroll,
  onFollowArmed,
  resumeFollowingBottom,
} from '../components/chat/scrollState';
import { onPageHide, onPageWake } from '../utils/pageVisit';

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
// Cap how long we'll wait for async content to render before giving up on
// restoring. Long enough for typical Loadable<T> roundtrips, short enough
// that a stuck observer doesn't permanently suppress saves.
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
  /** When true, force `scrollTop=0` for the no-save case (gates only the
   *  `saved === null` branch — `saved === 0` is a real restore and always
   *  writes scrollTop). Use for shared scroll containers where the previous
   *  view's offset would otherwise persist on the DOM. Off by default.
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
  };

  /** When the deadline expires without a full restore, fall back to
   *  scrolling as far as the current content allows. Covers "content
   *  shrunk since last visit": better to land near the user's last
   *  reading position than to silently abandon them at the top. Skips
   *  if the user has already scrolled during the restore window
   *  (scrollTop > 0) so we never yank them away from their position. */
  const onDeadline = () => {
    if (saved?.kind === 'offset' && saved.top > 0 && el.scrollTop === 0) {
      const max = Math.max(0, el.scrollHeight - el.clientHeight);
      if (max > 0) markNavigationScroll(el, Math.min(saved.top, max));
    }
    stopRestore();
  };

  const tryRestore = () => {
    if (!restoring || saved?.kind !== 'offset') return;
    if (!isFullyRestorable(saved.top, el.scrollHeight, el.clientHeight)) return;
    markNavigationScroll(el, saved.top);
    stopRestore();
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
    //
    // A scroll the FOLLOW made is recorded as the live edge, not as the offset it
    // happened to produce. Every growth round writes `scrollTop`, so recording the
    // number would overwrite the request with a pixel value on the next token, and
    // re-entry would land wherever the stream had got to rather than at the edge.
    //
    // The question asked is positional (`isFollowScroll`) rather than "is the
    // follow armed", because `.thread-content` carries two scroll listeners: the
    // disarm lives in `makeScrollObservers` and this save lives here, and the flag
    // alone would answer differently depending on which ran first. The reader's
    // gesture moves the container away from the follow's stamp by definition, so
    // this answers "the reader's" in either order, and the offset they landed on
    // is what gets recorded.
    observed = followsLiveEdge && isFollowScroll(el)
      ? LIVE_EDGE_VALUE
      : String(Math.floor(el.scrollTop));
    scheduleSave();
  };

  // A higher-priority scroll may own this load, e.g. a notification deep-link
  // resolving a scroll to a specific event. Skip the RESTORE so we can't
  // override that scroll, but still attach the save listener below so the
  // reader's post-landing position is remembered. Evaluated once here: the
  // deep-link claim is set (in focusThread) BEFORE this effect runs, and held
  // until scrollToEventAndPulse's deadline, so a setup-time check is enough.
  const allowRestore = live().shouldRestore?.() ?? true;

  // The deep-link stand-down leads, because it is the one branch that does not
  // care what was saved: it answers for every value of `saved`, including none.
  // The rest read the saved position, and taking them in this order is what lets
  // each one narrow it.
  if (!allowRestore) {
    // A deep-link owns this open, so stand down: whatever it lands on is where
    // the reader asked to be, and positioning here would overwrite it.
    //
    // But standing down cannot be the whole answer. If the link is DEAD (its
    // target never renders, because the event is not in this thread or renders
    // nothing) then nothing positions this thread at all, and `.thread-content`
    // is one element reused across threads: it keeps showing the OUTGOING
    // thread's offset, and the save listener below then persists that borrowed
    // number as this thread's remembered position.
    //
    // So wait out the deep-link's own budget and then position, but ONLY if the
    // container has not moved a pixel in the meantime. That condition is exactly
    // "the landing never happened": a successful landing moves it, and so does
    // the reader scrolling, and in both of those cases there is now a real
    // position here that is not ours to overwrite. `stopRestore` clears the
    // timer, so a thread switch inside the window cancels it.
    const inherited = el.scrollTop;
    restoring = false;
    deadlineTimer = setTimeout(() => {
      deadlineTimer = null;
      // Same question `onScroll` asks, and for the same reason: the teardown is
      // deferred past the render that changed `key`, so a superseded attachment
      // must not position a container that now belongs to the next thread.
      if (isCurrent && !isCurrent()) return;
      if (el.scrollTop !== inherited) return;
      if (!(live().shouldRestore?.() ?? true)) return; // a newer deep-link owns it now
      if (saved?.kind === 'live-edge') {
        resumeFollowingBottom(el);
      } else if (saved !== null) {
        const max = Math.max(0, el.scrollHeight - el.clientHeight);
        if (max > 0) markNavigationScroll(el, Math.min(saved.top, max));
      } else if (resetOnEmpty) {
        markNavigationScroll(el, 0);
      }
    }, EVENT_RESOLVE_DEADLINE_MS + DEAD_DEEP_LINK_SLACK_MS);
  } else if (saved === null) {
    // Browsers preserve scrollTop across children-shrink, so a shared
    // container needs an explicit reset; non-shared containers opt out.
    if (resetOnEmpty) markNavigationScroll(el, 0);
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
    // Two observers cover the two ways scrollHeight grows after first paint:
    //   - ResizeObserver: container's own size changes (rare for flex:1
    //     containers in fixed parents, but covers initial 0→layout).
    //   - MutationObserver: subtree content changes, since children added by
    //     async Loadable<T> data don't change the container's box, so the
    //     ResizeObserver alone never fires for the typical scrollable list.
    requestAnimationFrame(tryRestore);
    resizeObserver = new ResizeObserver(tryRestore);
    resizeObserver.observe(el);
    mutationObserver = new MutationObserver(tryRestore);
    mutationObserver.observe(el, { childList: true, subtree: true });
    deadlineTimer = setTimeout(onDeadline, RESTORE_DEADLINE_MS);
  }

  el.addEventListener('scroll', onScroll, { passive: true });

  // Arming a standing follow can produce NO scroll event at all: a reader already
  // at the live edge who presses the chevron gets a write the browser clamps to
  // where they already are, and an idle thread then grows nothing. The request is
  // real either way, so it is recorded from the arm itself rather than from a
  // scroll that may never come. Only the arm is broadcast (see `onFollowArmed`),
  // which is exactly what lets `focusThread` retire the follow on a thread switch
  // without overwriting the live edge just recorded for the thread being LEFT.
  const unsubscribeArm = followsLiveEdge
    ? onFollowArmed(() => {
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
      })
    : null;

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
        // A notification tap can resume the app and resolve a deep-link in one
        // breath. The deep-link owns the viewport, same as at attach time.
        if (!(live().shouldRestore?.() ?? true)) return;
        if (readSaved()?.kind !== 'live-edge') return;
        resumeFollowingBottom(el);
      })
    : null;

  return () => {
    stopRestore();
    el.removeEventListener('scroll', onScroll);
    unsubscribeArm?.();
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
