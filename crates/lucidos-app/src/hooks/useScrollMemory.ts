import { useEffect, useRef } from 'preact/hooks';
import type { RefObject } from 'preact';

/** True iff the container's current measurement can hold the saved offset
 *  (i.e. content has grown enough). Used to gate ResizeObserver-driven
 *  restore retries while async content is still rendering.
 *  saved=0 (user was at the top) is always restorable — distinguishing
 *  "user scrolled to top" from "no save" is essential to prevent the
 *  auto-scroll-to-bottom from clobbering the restore. */
export function isFullyRestorable(saved: number, scrollHeight: number, clientHeight: number): boolean {
  if (saved < 0) return false;
  if (saved === 0) return true;
  const max = Math.max(0, scrollHeight - clientHeight);
  return max >= saved;
}

/** Parse a localStorage scroll value. Returns null on missing/invalid/negative.
 *  Tolerates a `<offset>:<revision>` stamp (see `formatSavedScroll`) and returns
 *  just the offset, so every reader of a position is unaffected by the stamp. */
export function parseSavedScroll(raw: string | null): number | null {
  if (raw === null || raw === '') return null;
  const n = Number.parseFloat(raw);
  if (!Number.isFinite(n) || n < 0) return null;
  return Math.floor(n);
}

/** Serialize a saved offset, stamped with the REVISION of the content it was
 *  taken in when the caller tracks one. An unstamped value (no revision passed)
 *  keeps the historic bare-number format, so the callers that have no notion of
 *  content revisions (ContentPane's per-view offsets) are untouched. */
export function formatSavedScroll(offset: number, revision?: number): string {
  return revision === undefined ? String(offset) : `${offset}:${revision}`;
}

/** The revision a saved value was stamped with, or null when it carries none:
 *  a revision-less caller's value, or one written before the stamp existed. */
export function parseSavedRevision(raw: string | null): number | null {
  if (raw === null) return null;
  const sep = raw.indexOf(':');
  if (sep < 0) return null;
  const n = Number.parseInt(raw.slice(sep + 1), 10);
  return Number.isFinite(n) && n >= 0 ? n : null;
}

/** Is a saved reading position stale for content that now stands at
 *  `currentRevision`?
 *
 *  A reading position is scoped to the transcript it was taken in. Once the
 *  content has GROWN past that, restoring the offset parks the reader in the
 *  middle of a thread they opened to see the new part of, which reads as
 *  "opening the thread did not go to the end". Below or at the saved revision
 *  there is nothing new, so the position still means what it meant.
 *
 *  Two guards make the answer conservative in both directions:
 *   - `currentRevision <= 0` is "the content is not rendered yet", not "the
 *     content is empty", because the caller's count is 0 until its data loads.
 *     Never discard a position on that.
 *   - An UNSTAMPED value is stale as soon as there is any content, which is
 *     what retires the positions written before the stamp existed. They cannot
 *     be checked, and a wrong restore is the failure this exists to prevent. */
export function savedScrollIsStale(raw: string | null, currentRevision: number): boolean {
  if (raw === null || raw === '') return false;
  if (!Number.isFinite(currentRevision) || currentRevision <= 0) return false;
  const saved = parseSavedRevision(raw);
  if (saved === null) return true;
  return currentRevision > saved;
}

/** Drop the saved reading position at `key` when it was taken in an older
 *  transcript than the one now rendered (see `savedScrollIsStale`). Returns
 *  whether it dropped one.
 *
 *  Callers run this BEFORE anything reads `hasSavedScroll` for the same open,
 *  so the two answer the same question: `hasSavedScroll` is what makes
 *  `focusThread` skip its scroll-to-bottom, and if the restore then declined
 *  separately the reader would land at neither position. */
export function dropStaleSavedScroll(key: string, currentRevision: number): boolean {
  try {
    if (!savedScrollIsStale(localStorage.getItem(key), currentRevision)) return false;
    localStorage.removeItem(key);
    return true;
  } catch {
    return false; // quota or disabled
  }
}

/** localStorage key for a chat thread's saved scroll offset. Shared between
 *  ThreadView (which writes/restores) and focusThread (which checks it before
 *  calling scrollToBottom on focus change). */
export function threadScrollKey(threadId: string): string {
  return `lucidos-scroll-thread-${threadId}`;
}

/** True iff `key` holds a saved offset (including 0 — "user scrolled to top").
 *  Callers that gate other scroll effects (e.g., chat auto-scroll-to-bottom)
 *  on whether a restore is pending use this to avoid clobbering the restore.
 *  Absence of the key means "nothing to restore" (user was at the bottom). */
export function hasSavedScroll(key: string | null): boolean {
  if (!key) return false;
  try {
    return parseSavedScroll(localStorage.getItem(key)) !== null;
  } catch {
    return false;
  }
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

export interface ScrollMemoryOptions {
  /** Called immediately after a saved scroll position is restored. Use for
   *  side-effects like setting `scrolledUp` so auto-scroll logic respects it. */
  onRestored?: (scrollTop: number) => void;
  /** When true, don't restore even if a saved value exists. Useful while
   *  content is still loading. */
  paused?: boolean;
  /** Predicate called before each save. Return false to suppress saving the
   *  current position and clear any prior save. Defaults to always-save.
   *  For chat: pass `() => scrolledUp.value` so only meaningful "scrolled up"
   *  positions are remembered (at-bottom defers to auto-scroll on reload). */
  shouldSave?: () => boolean;
  /** Predicate called once at restore time (effect setup). Return false to
   *  SKIP restoring the saved offset for this mount/key-change while still
   *  attaching the save listener. Defaults to always-restore. For chat: pass
   *  `() => !hasPendingEventScroll()` so a notification deep-link resolving a
   *  scroll to a specific event isn't overridden by the saved-scroll restore.
   *  Without this, focusing an UNfocused thread re-runs this hook, and its
   *  restore observer (created after `scrollToEventAndPulse`'s) fires last and
   *  snaps back to the saved offset — the "toast deep-link lands on the saved
   *  position, not the event, unless the thread was already focused" bug. */
  shouldRestore?: () => boolean;
  /** When true, force `scrollTop=0` for the no-save case (gates only the
   *  `saved === null` branch — `saved === 0` is a real restore and always
   *  writes scrollTop). Use for shared scroll containers where the previous
   *  view's offset would otherwise persist on the DOM. Off by default. */
  resetOnEmpty?: boolean;
  /** How much content the container holds right now, in whatever unit makes a
   *  reading position stale when it grows (for chat: the EXCHANGE count, so a
   *  streaming turn growing under a parked reader does not retire their
   *  position but a new turn does). Every save is stamped with it; the caller
   *  compares at restore time via `dropStaleSavedScroll`. Read at WRITE time,
   *  not at attach time, so a position saved after new content arrived carries
   *  the revision the reader actually saw. Omit it and nothing is stamped. */
  revision?: number;
}

/** The options an attachment reads LIVE, i.e. whose current value belongs to
 *  whatever the component last rendered rather than to the attachment. The hook
 *  hands them over as one getter, not as values, precisely so the attachment
 *  cannot capture them at setup and go stale; the flip side is that reading one
 *  after the attachment stopped being current reads the NEXT thing's value. See
 *  `observed` in `attachScrollMemory` for the bug that cost. */
export type ScrollMemoryLive = Pick<
  ScrollMemoryOptions, 'onRestored' | 'shouldSave' | 'shouldRestore' | 'revision'
>;

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
  opts: { live: () => ScrollMemoryLive; resetOnEmpty?: boolean; isCurrent?: () => boolean },
): () => void {
  const { live, resetOnEmpty = false, isCurrent } = opts;

  let saveTimer: ReturnType<typeof setTimeout> | null = null;
  // hasWritten is the dedup gate: lastSaved alone can't tell "wrote 0"
  // from "wrote nothing yet", and we need the first writeNow to act so
  // any stale localStorage value from a previous session is reconciled.
  // It holds the SERIALIZED value, so a re-save at the same offset under a
  // newer revision still writes rather than being deduped away.
  let lastSaved: string | null = null;
  let hasWritten = false;
  let restoring = true;
  /** The value to commit, captured WHEN THE SCROLL HAPPENED rather than when
   *  the debounce fires. Everything it is built from (the container's offset,
   *  `live().revision`, `live().shouldSave`) belongs to THIS key, and all three
   *  stop belonging to it at teardown: the teardown runs from the hook's effect
   *  cleanup, which Preact defers past the render that changed `key`, so by then
   *  the shared `.thread-content` is already showing the INCOMING thread and
   *  `live()` already answers with ITS values.
   *
   *  Reading them there wrote the outgoing thread's key with the incoming
   *  thread's offset and revision, and the staleness check then retired the
   *  result: switching threads silently discarded the reading position in the
   *  thread you just left. Snapshotting instead leaves nothing at teardown for
   *  the new render to have moved, so the bug is unrepresentable rather than
   *  merely fixed.
   *
   *  Three states, not two: `undefined` is "this key has seen no scroll", and
   *  is distinct from a `null` that means "the caller declined to save, clear
   *  the key". Collapsing them would make an unreached `writeNow` delete a
   *  stored position rather than do nothing. */
  let observed: string | null | undefined;
  let resizeObserver: ResizeObserver | null = null;
  let mutationObserver: MutationObserver | null = null;
  let deadlineTimer: ReturnType<typeof setTimeout> | null = null;

  let saved: number | null = null;
  try {
    saved = parseSavedScroll(localStorage.getItem(key));
  } catch { /* ignore */ }

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
    if (saved !== null && saved > 0 && el.scrollTop === 0) {
      const max = Math.max(0, el.scrollHeight - el.clientHeight);
      if (max > 0) {
        el.scrollTop = Math.min(saved, max);
        live().onRestored?.(el.scrollTop);
      }
    }
    stopRestore();
  };

  const tryRestore = () => {
    if (!restoring || saved === null) return;
    if (!isFullyRestorable(saved, el.scrollHeight, el.clientHeight)) return;
    el.scrollTop = saved;
    live().onRestored?.(saved);
    stopRestore();
  };

  const writeNow = () => {
    const next = observed;
    if (next === undefined) return; // no scroll seen under this key
    if (hasWritten && next === lastSaved) return;
    lastSaved = next;
    hasWritten = true;
    try {
      if (next === null) {
        localStorage.removeItem(key);
      } else {
        localStorage.setItem(key, next);
      }
    } catch { /* quota or disabled, ignore */ }
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
    // position in the thread being LEFT. `focusThread` pins the transcript
    // synchronously when the thread being opened has no saved position, and
    // that write moves the container. When the thread being opened DOES have
    // one, no pin runs, but swapping in its (shorter, or not yet loaded)
    // content clamps `scrollTop` instead. Either way a scroll event reaches
    // this handler carrying the incoming thread's `shouldSave()`, and the flush
    // that follows wrote or cleared the outgoing key from it.
    //
    // Asking whether this attachment is still current answers both, and asks
    // nothing about WHY the container moved: no ordering assumption about which
    // scroll listener runs first, and no carve-out that would also swallow the
    // legitimate clear when a deliberate go-to-bottom lands the reader at the
    // newest turn.
    if (isCurrent && !isCurrent()) return;
    // Save vs. clear is decided by `allow`, not by the value. A scrollTop
    // of 0 with allow=true means "user scrolled to top", and must persist as
    // "0" so hasSavedScroll() returns true and the auto-scroll-to-bottom
    // on next mount is suppressed. Using value to decide cleared the key
    // at the top and snapped back to bottom on remount.
    //
    // Evaluated HERE, with the offset and revision, rather than when the
    // debounce fires: see `observed`. Nothing is lost by reading it a beat
    // earlier, since every scrollTop change fires this handler, so the last
    // event of a burst already carries the settled position; and the chat
    // caller's `scrolledUp` was reconciled for THIS scroll by the transcript's
    // own listener, which is attached to the element ahead of this one.
    const allow = live().shouldSave?.() ?? true;
    observed = allow
      ? formatSavedScroll(Math.floor(el.scrollTop), live().revision)
      : null;
    if (saveTimer !== null) clearTimeout(saveTimer);
    saveTimer = setTimeout(writeNow, SAVE_DEBOUNCE_MS);
  };

  // A higher-priority scroll may own this load, e.g. a notification
  // deep-link resolving a scroll to a specific event. Skip the restore so it
  // can't override that scroll, but still attach the save listener below so
  // the user's post-landing position is remembered. Evaluated once here: the
  // deep-link claim is set (in focusThread) BEFORE this effect runs, and held
  // until scrollToEventAndPulse's deadline, so a setup-time check is enough.
  const allowRestore = live().shouldRestore?.() ?? true;

  if (saved === null) {
    // Browsers preserve scrollTop across children-shrink, so a shared
    // container needs an explicit reset; non-shared containers opt out.
    if (resetOnEmpty) el.scrollTop = 0;
    restoring = false;
  } else if (!allowRestore) {
    restoring = false;
  } else if (saved === 0) {
    // Restore explicitly, same shared-container reason as the null branch.
    el.scrollTop = 0;
    live().onRestored?.(0);
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

  return () => {
    stopRestore();
    el.removeEventListener('scroll', onScroll);
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
  const { paused = false, resetOnEmpty = false } = options;
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
      isCurrent: () => keyRef.current === key,
    });
  }, [ref, key, paused, resetOnEmpty]);
}
