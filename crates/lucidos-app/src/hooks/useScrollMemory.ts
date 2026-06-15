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

/** Parse a localStorage scroll value. Returns null on missing/invalid/negative. */
export function parseSavedScroll(raw: string | null): number | null {
  if (raw === null || raw === '') return null;
  const n = Number.parseFloat(raw);
  if (!Number.isFinite(n) || n < 0) return null;
  return Math.floor(n);
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
  } catch { /* quota or disabled — ignore */ }
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
}

/** Persist and restore a scroll container's vertical scroll position across
 *  reloads via localStorage. The container is identified by `key`; the same key
 *  on next mount restores the saved offset.
 *
 *  When `key` changes (e.g., switching threads), the previous container's
 *  position is flushed and the new key is restored.
 *
 *  Save is debounced by ~150ms to avoid storage thrash during scroll. */
export function useScrollMemory(
  ref: RefObject<HTMLElement>,
  key: string | null,
  options: ScrollMemoryOptions = {},
) {
  const { onRestored, paused = false, shouldSave, shouldRestore, resetOnEmpty = false } = options;
  const onRestoredRef = useRef(onRestored);
  const shouldSaveRef = useRef(shouldSave);
  const shouldRestoreRef = useRef(shouldRestore);
  onRestoredRef.current = onRestored;
  shouldSaveRef.current = shouldSave;
  shouldRestoreRef.current = shouldRestore;

  useEffect(() => {
    if (!key || paused) return;
    const el = ref.current;
    if (!el) return;

    let saveTimer: ReturnType<typeof setTimeout> | null = null;
    // hasWritten is the dedup gate: lastSaved alone can't tell "wrote 0"
    // from "wrote nothing yet", and we need the first writeNow to act so
    // any stale localStorage value from a previous session is reconciled.
    let lastSaved: number | null = null;
    let hasWritten = false;
    let restoring = true;
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
     *  shrunk since last visit" — better to land near the user's last
     *  reading position than to silently abandon them at the top. Skips
     *  if the user has already scrolled during the restore window
     *  (scrollTop > 0) so we never yank them away from their position. */
    const onDeadline = () => {
      if (saved !== null && saved > 0 && el.scrollTop === 0) {
        const max = Math.max(0, el.scrollHeight - el.clientHeight);
        if (max > 0) {
          el.scrollTop = Math.min(saved, max);
          onRestoredRef.current?.(el.scrollTop);
        }
      }
      stopRestore();
    };

    const tryRestore = () => {
      if (!restoring || saved === null) return;
      if (!isFullyRestorable(saved, el.scrollHeight, el.clientHeight)) return;
      el.scrollTop = saved;
      onRestoredRef.current?.(saved);
      stopRestore();
    };

    const writeNow = () => {
      const allow = shouldSaveRef.current?.() ?? true;
      // Save vs. clear is decided by `allow`, not by the value. A scrollTop
      // of 0 with allow=true means "user scrolled to top" — must persist as
      // "0" so hasSavedScroll() returns true and the auto-scroll-to-bottom
      // on next mount is suppressed. Using value to decide cleared the key
      // at the top and snapped back to bottom on remount.
      const next: number | null = allow ? Math.floor(el.scrollTop) : null;
      if (hasWritten && next === lastSaved) return;
      lastSaved = next;
      hasWritten = true;
      try {
        if (next === null) {
          localStorage.removeItem(key);
        } else {
          localStorage.setItem(key, String(next));
        }
      } catch { /* quota or disabled — ignore */ }
    };

    const flush = () => {
      if (saveTimer === null) return;
      clearTimeout(saveTimer);
      saveTimer = null;
      writeNow();
    };

    const onScroll = () => {
      if (restoring) return;
      if (saveTimer !== null) clearTimeout(saveTimer);
      saveTimer = setTimeout(writeNow, SAVE_DEBOUNCE_MS);
    };

    // A higher-priority scroll may own this load — e.g. a notification
    // deep-link resolving a scroll to a specific event. Skip the restore so it
    // can't override that scroll, but still attach the save listener below so
    // the user's post-landing position is remembered. Evaluated once here: the
    // deep-link claim is set (in focusThread) BEFORE this effect runs, and held
    // until scrollToEventAndPulse's deadline, so a setup-time check is enough.
    const allowRestore = shouldRestoreRef.current ? shouldRestoreRef.current() : true;

    if (saved === null) {
      // Browsers preserve scrollTop across children-shrink, so a shared
      // container needs an explicit reset; non-shared containers opt out.
      if (resetOnEmpty) el.scrollTop = 0;
      restoring = false;
    } else if (!allowRestore) {
      restoring = false;
    } else if (saved === 0) {
      // Restore explicitly — same shared-container reason as the null branch.
      el.scrollTop = 0;
      onRestoredRef.current?.(0);
      restoring = false;
    } else {
      // Two observers cover the two ways scrollHeight grows after first paint:
      //   - ResizeObserver: container's own size changes (rare for flex:1
      //     containers in fixed parents, but covers initial 0→layout).
      //   - MutationObserver: subtree content changes — children added by
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
  }, [ref, key, paused]);
}
